use crate::{
    config::{AppConfig, TriangleConfig},
    models::{self, DepthStreamWrapper},
    Clients, SignalLogSender,
};
use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info, warn};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::{
    net::TcpStream,
    sync::mpsc::error::TrySendError,
    time::{Duration, Instant},
};
use tokio_tungstenite::{tungstenite::Message as BinanceMessage, MaybeTlsStream, WebSocketStream};
use warp::ws::Message as ClientMessage;

type BinanceSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Serialize)]
struct TriangleArbitragePayload<'a> {
    triangle: [&'a str; 3],
    profits: Vec<f64>,
    start_pair_data: &'a DepthStreamWrapper,
    mid_pair_data: &'a DepthStreamWrapper,
    end_pair_data: &'a DepthStreamWrapper,
}

#[derive(Clone, Copy)]
struct ProfitPoint {
    level_index: usize,
    profit: f64,
}

struct TriangleOutputs {
    ws_payload: String,
    signal_line: String,
}

pub async fn main_worker(
    clients: Clients,
    config: AppConfig,
    mut socket: BinanceSocket,
    signal_log_sender: Option<SignalLogSender>,
) {
    let mut pairs_data: HashMap<String, DepthStreamWrapper> =
        HashMap::with_capacity(config.depth_streams.len());
    let publish_every = Duration::from_millis(u64::from(config.update_interval.max(1)));
    let mut last_publish = Instant::now();

    loop {
        let Some(result) = socket.next().await else {
            warn!("Binance websocket stream ended");
            break;
        };

        let msg = match result {
            Ok(msg) => msg,
            Err(e) => {
                error!("error reading Binance message: {}", e);
                break;
            }
        };

        match msg {
            BinanceMessage::Text(text) => {
                let parsed: models::DepthStreamWrapper = match serde_json::from_str(text.as_ref()) {
                    Ok(parsed) => parsed,
                    Err(e) => {
                        warn!("failed to parse Binance depth payload: {}", e);
                        continue;
                    }
                };

                let pair_key = pair_key_from_stream(&parsed.stream).to_owned();
                debug!(
                    "received {} ({} bids / {} asks)",
                    parsed.stream,
                    parsed.data.bids.len(),
                    parsed.data.asks.len()
                );
                pairs_data.insert(pair_key, parsed);
            }
            BinanceMessage::Ping(payload) => {
                if let Err(e) = socket.send(BinanceMessage::Pong(payload)).await {
                    error!("failed to reply to Binance ping: {}", e);
                    break;
                }
                continue;
            }
            BinanceMessage::Pong(_) => continue,
            BinanceMessage::Close(frame) => {
                info!("Binance websocket closed: {:?}", frame);
                break;
            }
            _ => continue,
        }

        if last_publish.elapsed() < publish_every {
            continue;
        }
        last_publish = Instant::now();

        if let Err(e) =
            publish_triangle_updates(&clients, &config, &pairs_data, signal_log_sender.as_ref())
                .await
        {
            warn!("failed to publish triangle updates: {}", e);
        }
    }
}

fn pair_key_from_stream(stream: &str) -> &str {
    stream.split_once('@').map_or(stream, |(pair, _)| pair)
}

async fn publish_triangle_updates(
    clients: &Clients,
    config: &AppConfig,
    pairs_data: &HashMap<String, DepthStreamWrapper>,
    signal_log_sender: Option<&SignalLogSender>,
) -> Result<(), serde_json::Error> {
    let recipients = {
        let locked = clients.read().await;
        if locked.is_empty() {
            return Ok(());
        }

        locked
            .iter()
            .map(|(id, client)| (id.clone(), client.sender.clone()))
            .collect::<Vec<_>>()
    };

    let mut stale_client_ids = HashSet::new();

    for triangle in &config.triangles {
        let Some(outputs) = build_triangle_outputs(
            pairs_data,
            triangle,
            config.signal_min_profit_bps,
            config.signal_min_hit_rate,
        )?
        else {
            continue;
        };

        if let Some(sender) = signal_log_sender {
            match sender.try_send(outputs.signal_line) {
                Ok(()) => {}
                Err(TrySendError::Closed(_)) => {
                    debug!("signal log writer is closed");
                }
                Err(TrySendError::Full(_)) => {
                    debug!("signal log queue is full; dropping signal line");
                }
            }
        }

        for (client_id, sender) in &recipients {
            match sender.try_send(Ok(ClientMessage::text(outputs.ws_payload.clone()))) {
                Ok(()) => {}
                Err(TrySendError::Closed(_)) => {
                    stale_client_ids.insert(client_id.clone());
                }
                Err(TrySendError::Full(_)) => {
                    debug!("skipping update for slow client {}", client_id);
                }
            }
        }
    }

    if !stale_client_ids.is_empty() {
        let mut locked = clients.write().await;
        for client_id in stale_client_ids {
            locked.remove(&client_id);
        }
    }

    Ok(())
}

fn build_triangle_outputs(
    pairs_data: &HashMap<String, DepthStreamWrapper>,
    triangle_config: &TriangleConfig,
    min_profit_bps_threshold: f64,
    min_hit_rate_threshold: f64,
) -> Result<Option<TriangleOutputs>, serde_json::Error> {
    let [start_pair, mid_pair, end_pair] = &triangle_config.pairs;
    let [part_a, part_b, part_c] = &triangle_config.parts;

    let (start_pair_data, mid_pair_data, end_pair_data) = match (
        pairs_data.get(start_pair),
        pairs_data.get(mid_pair),
        pairs_data.get(end_pair),
    ) {
        (Some(s), Some(m), Some(e)) => (s, m, e),
        _ => return Ok(None),
    };

    let profit_points = calculate_triangle_profits(
        start_pair_data,
        mid_pair_data,
        end_pair_data,
        start_pair,
        mid_pair,
        end_pair,
        [part_a.as_str(), part_b.as_str(), part_c.as_str()],
    );

    if profit_points.is_empty() {
        return Ok(None);
    }

    let profits = profit_points
        .iter()
        .map(|point| point.profit)
        .collect::<Vec<_>>();
    let best_point = profit_points
        .iter()
        .copied()
        .max_by(|a, b| a.profit.total_cmp(&b.profit))
        .expect("profit_points not empty");
    let profitable_levels = profit_points
        .iter()
        .filter(|point| point.profit > 0.0)
        .count();
    let hit_rate = profitable_levels as f64 / profit_points.len() as f64;
    let avg_profit = profits.iter().sum::<f64>() / profits.len() as f64;
    let top_profit = profits.first().copied().unwrap_or_default();
    let best_profit_bps = best_point.profit * 10_000.0;
    let top_profit_bps = top_profit * 10_000.0;
    let avg_profit_bps = avg_profit * 10_000.0;
    let worthy = best_profit_bps >= min_profit_bps_threshold && hit_rate >= min_hit_rate_threshold;

    if best_point.profit > 0.0 {
        info!(
            target: "profit",
            "{:?} best profit: {:.5}% ({} {})",
            [part_a, part_b, part_c],
            best_point.profit * 100.0,
            best_point.profit,
            part_a
        );
    }

    let payload = TriangleArbitragePayload {
        triangle: [part_a.as_str(), part_b.as_str(), part_c.as_str()],
        profits,
        start_pair_data,
        mid_pair_data,
        end_pair_data,
    };

    let signal = build_opportunity_signal(
        triangle_config,
        start_pair_data,
        mid_pair_data,
        end_pair_data,
        &profit_points,
        best_point,
        profitable_levels,
        hit_rate,
        top_profit_bps,
        best_profit_bps,
        avg_profit_bps,
        worthy,
        min_profit_bps_threshold,
        min_hit_rate_threshold,
    );

    Ok(Some(TriangleOutputs {
        ws_payload: serde_json::to_string(&payload)?,
        signal_line: serde_json::to_string(&signal)?,
    }))
}

#[allow(clippy::too_many_arguments)]
fn build_opportunity_signal(
    triangle_config: &TriangleConfig,
    start_pair_data: &DepthStreamWrapper,
    mid_pair_data: &DepthStreamWrapper,
    end_pair_data: &DepthStreamWrapper,
    profit_points: &[ProfitPoint],
    best_point: ProfitPoint,
    profitable_levels: usize,
    hit_rate: f64,
    top_profit_bps: f64,
    best_profit_bps: f64,
    avg_profit_bps: f64,
    worthy: bool,
    min_profit_bps_threshold: f64,
    min_hit_rate_threshold: f64,
) -> models::TriangleOpportunitySignal {
    let i = best_point.level_index;
    models::TriangleOpportunitySignal {
        timestamp_ms: now_unix_ms(),
        exchange: "binance".to_string(),
        triangle_parts: triangle_config.parts.clone(),
        triangle_pairs: triangle_config.pairs.clone(),
        depth_levels_considered: profit_points.len(),
        profitable_levels,
        hit_rate,
        top_profit_bps,
        best_profit_bps,
        avg_profit_bps,
        best_level_index: i,
        worthy,
        min_profit_bps_threshold,
        min_hit_rate_threshold,
        best_level_quotes: [
            models::TriangleLegQuote {
                pair: triangle_config.pairs[0].clone(),
                ask_price: start_pair_data.data.asks[i].price,
                ask_size: start_pair_data.data.asks[i].size,
                bid_price: start_pair_data.data.bids[i].price,
                bid_size: start_pair_data.data.bids[i].size,
            },
            models::TriangleLegQuote {
                pair: triangle_config.pairs[1].clone(),
                ask_price: mid_pair_data.data.asks[i].price,
                ask_size: mid_pair_data.data.asks[i].size,
                bid_price: mid_pair_data.data.bids[i].price,
                bid_size: mid_pair_data.data.bids[i].size,
            },
            models::TriangleLegQuote {
                pair: triangle_config.pairs[2].clone(),
                ask_price: end_pair_data.data.asks[i].price,
                ask_size: end_pair_data.data.asks[i].size,
                bid_price: end_pair_data.data.bids[i].price,
                bid_size: end_pair_data.data.bids[i].size,
            },
        ],
    }
}

fn now_unix_ms() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis().min(u128::from(u64::MAX)) as u64,
        Err(_) => 0,
    }
}

fn calculate_triangle_profits(
    start_pair_data: &DepthStreamWrapper,
    mid_pair_data: &DepthStreamWrapper,
    end_pair_data: &DepthStreamWrapper,
    start_pair: &str,
    mid_pair: &str,
    end_pair: &str,
    triangle: [&str; 3],
) -> Vec<ProfitPoint> {
    let depth = [
        start_pair_data.data.asks.len(),
        start_pair_data.data.bids.len(),
        mid_pair_data.data.asks.len(),
        mid_pair_data.data.bids.len(),
        end_pair_data.data.asks.len(),
        end_pair_data.data.bids.len(),
    ]
    .into_iter()
    .min()
    .unwrap_or(0);

    if depth == 0 {
        return Vec::new();
    }

    let mut profits = Vec::with_capacity(depth);

    for i in 0..depth {
        let ask1 = start_pair_data.data.asks[i].price;
        let bid1 = start_pair_data.data.bids[i].price;
        let ask2 = mid_pair_data.data.asks[i].price;
        let bid2 = mid_pair_data.data.bids[i].price;
        let ask3 = end_pair_data.data.asks[i].price;
        let bid3 = end_pair_data.data.bids[i].price;

        if ask1 <= 0.0 || ask2 <= 0.0 || ask3 <= 0.0 {
            continue;
        }

        let mut amount = calc_triangle_step(1.0, ask1, bid1, start_pair, triangle[0]);
        amount = calc_triangle_step(amount, ask2, bid2, mid_pair, triangle[1]);
        amount = calc_triangle_step(amount, ask3, bid3, end_pair, triangle[2]);

        let profit = amount - 1.0;
        if profit.is_finite() {
            profits.push(ProfitPoint {
                level_index: i,
                profit,
            });
        }
    }

    profits
}

fn calc_triangle_step(
    trade_amount: f64,
    ask_price: f64,
    bid_price: f64,
    pair_name: &str,
    triangle_part: &str,
) -> f64 {
    let trade_amount = match pair_name {
        "btcusdt" | "btcbusd" | "btcusdc" => trade_amount,
        _ => trade_amount * (1.0 - 0.00075),
    };

    if pair_name.starts_with(triangle_part) {
        trade_amount * bid_price
    } else {
        trade_amount / ask_price
    }
}

#[cfg(test)]
mod tests {
    use super::calc_triangle_step;

    #[test]
    fn sell_side_uses_bid() {
        let amount = calc_triangle_step(2.0, 10.0, 9.0, "bnbbtc", "bnb");
        assert!(amount > 17.9 && amount < 18.1);
    }

    #[test]
    fn buy_side_uses_ask() {
        let amount = calc_triangle_step(1.0, 2.0, 1.9, "ethbtc", "btc");
        assert!(amount > 0.49 && amount < 0.51);
    }
}
