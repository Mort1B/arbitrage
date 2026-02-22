use crate::{
    config::{AppConfig, ExchangeRulesConfig, PairRuleConfig, TriangleConfig},
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

#[derive(Clone, Copy)]
enum TradeSide {
    SellBase,
    BuyBase,
}

struct ExecutionFilterOutcome {
    assumed_start_amount: f64,
    executable_profit_bps: f64,
    adjusted_profit_bps: f64,
    latency_penalty_bps: f64,
    execution_filter_passed: bool,
    rejection_reasons: Vec<String>,
    fee_bps_by_leg: [f64; 3],
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
        let Some(outputs) = build_triangle_outputs(pairs_data, triangle, config)? else {
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
    config: &AppConfig,
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
        &config.exchange_rules,
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
    let execution_outcome = evaluate_execution_filters(
        triangle_config,
        start_pair_data,
        mid_pair_data,
        end_pair_data,
        best_point.level_index,
        &config.exchange_rules,
    );
    let worthy = execution_outcome.execution_filter_passed
        && execution_outcome.adjusted_profit_bps >= config.signal_min_profit_bps
        && hit_rate >= config.signal_min_hit_rate;

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
        &execution_outcome,
        worthy,
        config.signal_min_profit_bps,
        config.signal_min_hit_rate,
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
    execution_outcome: &ExecutionFilterOutcome,
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
        assumed_start_amount: execution_outcome.assumed_start_amount,
        executable_profit_bps: execution_outcome.executable_profit_bps,
        adjusted_profit_bps: execution_outcome.adjusted_profit_bps,
        latency_penalty_bps: execution_outcome.latency_penalty_bps,
        execution_filter_passed: execution_outcome.execution_filter_passed,
        worthy,
        min_profit_bps_threshold,
        min_hit_rate_threshold,
        rejection_reasons: execution_outcome.rejection_reasons.clone(),
        fee_bps_by_leg: execution_outcome.fee_bps_by_leg,
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

fn evaluate_execution_filters(
    triangle_config: &TriangleConfig,
    start_pair_data: &DepthStreamWrapper,
    mid_pair_data: &DepthStreamWrapper,
    end_pair_data: &DepthStreamWrapper,
    level_index: usize,
    rules: &ExchangeRulesConfig,
) -> ExecutionFilterOutcome {
    let fee_bps_by_leg = [
        fee_bps_for_pair(rules, &triangle_config.pairs[0]),
        fee_bps_for_pair(rules, &triangle_config.pairs[1]),
        fee_bps_for_pair(rules, &triangle_config.pairs[2]),
    ];
    let assumed_start_amount = assumed_start_amount_for_asset(rules, &triangle_config.parts[0]);

    if !rules.enabled {
        let raw_profit = simulate_triangle_path_amount(
            triangle_config,
            start_pair_data,
            mid_pair_data,
            end_pair_data,
            level_index,
            assumed_start_amount,
            &fee_bps_by_leg,
            false,
            rules,
        );
        let executable_profit_bps = raw_profit.map_or(0.0, |final_amount| {
            ((final_amount / assumed_start_amount.max(f64::MIN_POSITIVE)) - 1.0) * 10_000.0
        });
        return ExecutionFilterOutcome {
            assumed_start_amount,
            executable_profit_bps,
            adjusted_profit_bps: executable_profit_bps,
            latency_penalty_bps: 0.0,
            execution_filter_passed: true,
            rejection_reasons: Vec::new(),
            fee_bps_by_leg,
        };
    }

    let mut rejection_reasons = Vec::new();
    let simulated = simulate_triangle_path_amount(
        triangle_config,
        start_pair_data,
        mid_pair_data,
        end_pair_data,
        level_index,
        assumed_start_amount,
        &fee_bps_by_leg,
        true,
        rules,
    );

    let (execution_filter_passed, executable_profit_bps) = match simulated {
        Ok(final_amount) => (
            true,
            ((final_amount / assumed_start_amount.max(f64::MIN_POSITIVE)) - 1.0) * 10_000.0,
        ),
        Err(reason) => {
            rejection_reasons.push(reason);
            (false, 0.0)
        }
    };

    let latency_penalty_bps = rules.latency_penalty_bps.max(0.0);
    let adjusted_profit_bps = executable_profit_bps - latency_penalty_bps;

    ExecutionFilterOutcome {
        assumed_start_amount,
        executable_profit_bps,
        adjusted_profit_bps,
        latency_penalty_bps,
        execution_filter_passed,
        rejection_reasons,
        fee_bps_by_leg,
    }
}

#[allow(clippy::too_many_arguments)]
fn simulate_triangle_path_amount(
    triangle_config: &TriangleConfig,
    start_pair_data: &DepthStreamWrapper,
    mid_pair_data: &DepthStreamWrapper,
    end_pair_data: &DepthStreamWrapper,
    level_index: usize,
    initial_amount: f64,
    fee_bps_by_leg: &[f64; 3],
    enforce_rules: bool,
    rules: &ExchangeRulesConfig,
) -> Result<f64, String> {
    let leg1 = simulate_leg(
        initial_amount,
        &triangle_config.pairs[0],
        &triangle_config.parts[0],
        &start_pair_data.data.asks[level_index],
        &start_pair_data.data.bids[level_index],
        fee_bps_by_leg[0],
        enforce_rules,
        rules,
    )?;
    let leg2 = simulate_leg(
        leg1,
        &triangle_config.pairs[1],
        &triangle_config.parts[1],
        &mid_pair_data.data.asks[level_index],
        &mid_pair_data.data.bids[level_index],
        fee_bps_by_leg[1],
        enforce_rules,
        rules,
    )?;
    simulate_leg(
        leg2,
        &triangle_config.pairs[2],
        &triangle_config.parts[2],
        &end_pair_data.data.asks[level_index],
        &end_pair_data.data.bids[level_index],
        fee_bps_by_leg[2],
        enforce_rules,
        rules,
    )
}

#[allow(clippy::too_many_arguments)]
fn simulate_leg(
    trade_amount: f64,
    pair_name: &str,
    triangle_part: &str,
    ask: &models::OfferData,
    bid: &models::OfferData,
    fee_bps: f64,
    enforce_rules: bool,
    rules: &ExchangeRulesConfig,
) -> Result<f64, String> {
    if trade_amount <= 0.0 || !trade_amount.is_finite() {
        return Err(format!("invalid input amount for {pair_name}"));
    }

    let pair_rule = pair_rule_for_pair(rules, pair_name);
    let fee_multiplier = 1.0 - fee_bps.max(0.0) / 10_000.0;
    let side = trade_side(pair_name, triangle_part);

    match side {
        TradeSide::SellBase => {
            let mut base_qty = trade_amount * fee_multiplier;
            if enforce_rules {
                base_qty = apply_qty_rounding(base_qty, pair_rule);
                validate_base_qty_rules(pair_name, base_qty, pair_rule)?;
                if base_qty > bid.size {
                    return Err(format!("insufficient bid depth on {pair_name}"));
                }
                validate_min_notional(pair_name, base_qty * bid.price, pair_rule)?;
            }
            Ok(base_qty * bid.price)
        }
        TradeSide::BuyBase => {
            let quote_amount = trade_amount * fee_multiplier;
            if enforce_rules {
                validate_min_notional(pair_name, quote_amount, pair_rule)?;
            }

            let mut base_qty = quote_amount / ask.price;
            if !base_qty.is_finite() || base_qty <= 0.0 {
                return Err(format!("invalid ask price on {pair_name}"));
            }

            if enforce_rules {
                base_qty = apply_qty_rounding(base_qty, pair_rule);
                validate_base_qty_rules(pair_name, base_qty, pair_rule)?;
                if base_qty > ask.size {
                    return Err(format!("insufficient ask depth on {pair_name}"));
                }
                validate_min_notional(pair_name, base_qty * ask.price, pair_rule)?;
            }

            Ok(base_qty)
        }
    }
}

fn trade_side(pair_name: &str, triangle_part: &str) -> TradeSide {
    if pair_name.starts_with(triangle_part) {
        TradeSide::SellBase
    } else {
        TradeSide::BuyBase
    }
}

fn pair_rule_for_pair<'a>(
    rules: &'a ExchangeRulesConfig,
    pair_name: &str,
) -> Option<&'a PairRuleConfig> {
    rules
        .pair_rules
        .get(pair_name)
        .or_else(|| rules.pair_rules.get(&pair_name.to_ascii_lowercase()))
}

fn fee_bps_for_pair(rules: &ExchangeRulesConfig, pair_name: &str) -> f64 {
    pair_rule_for_pair(rules, pair_name)
        .and_then(|rule| rule.fee_bps)
        .unwrap_or(rules.default_fee_bps)
        .max(0.0)
}

fn assumed_start_amount_for_asset(rules: &ExchangeRulesConfig, asset: &str) -> f64 {
    rules
        .assumed_start_amounts
        .get(asset)
        .copied()
        .or_else(|| {
            rules
                .assumed_start_amounts
                .get(&asset.to_ascii_lowercase())
                .copied()
        })
        .unwrap_or(rules.default_assumed_start_amount)
        .max(f64::MIN_POSITIVE)
}

fn apply_qty_rounding(quantity: f64, pair_rule: Option<&PairRuleConfig>) -> f64 {
    let Some(step) = pair_rule.and_then(|rule| rule.qty_step) else {
        return quantity;
    };
    if step <= 0.0 {
        return quantity;
    }
    floor_to_step(quantity, step)
}

fn floor_to_step(value: f64, step: f64) -> f64 {
    if !value.is_finite() || !step.is_finite() || step <= 0.0 {
        return value;
    }
    (value / step).floor() * step
}

fn validate_base_qty_rules(
    pair_name: &str,
    base_qty: f64,
    pair_rule: Option<&PairRuleConfig>,
) -> Result<(), String> {
    if base_qty <= 0.0 || !base_qty.is_finite() {
        return Err(format!("non-positive rounded quantity on {pair_name}"));
    }
    if let Some(min_qty) = pair_rule.and_then(|rule| rule.min_qty) {
        if base_qty < min_qty {
            return Err(format!(
                "quantity below min_qty on {pair_name} ({base_qty} < {min_qty})"
            ));
        }
    }
    Ok(())
}

fn validate_min_notional(
    pair_name: &str,
    notional: f64,
    pair_rule: Option<&PairRuleConfig>,
) -> Result<(), String> {
    if let Some(min_notional) = pair_rule.and_then(|rule| rule.min_notional) {
        if notional < min_notional {
            return Err(format!(
                "notional below min_notional on {pair_name} ({notional} < {min_notional})"
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn calculate_triangle_profits(
    start_pair_data: &DepthStreamWrapper,
    mid_pair_data: &DepthStreamWrapper,
    end_pair_data: &DepthStreamWrapper,
    start_pair: &str,
    mid_pair: &str,
    end_pair: &str,
    triangle: [&str; 3],
    exchange_rules: &ExchangeRulesConfig,
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
    let fee1 = fee_bps_for_pair(exchange_rules, start_pair);
    let fee2 = fee_bps_for_pair(exchange_rules, mid_pair);
    let fee3 = fee_bps_for_pair(exchange_rules, end_pair);

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

        let mut amount = calc_triangle_step(1.0, ask1, bid1, start_pair, triangle[0], fee1);
        amount = calc_triangle_step(amount, ask2, bid2, mid_pair, triangle[1], fee2);
        amount = calc_triangle_step(amount, ask3, bid3, end_pair, triangle[2], fee3);

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
    fee_bps: f64,
) -> f64 {
    let trade_amount = trade_amount * (1.0 - fee_bps.max(0.0) / 10_000.0);

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
        let amount = calc_triangle_step(2.0, 10.0, 9.0, "bnbbtc", "bnb", 0.0);
        assert!(amount > 17.9 && amount < 18.1);
    }

    #[test]
    fn buy_side_uses_ask() {
        let amount = calc_triangle_step(1.0, 2.0, 1.9, "ethbtc", "btc", 0.0);
        assert!(amount > 0.49 && amount < 0.51);
    }
}
