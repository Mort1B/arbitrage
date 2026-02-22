use crate::{
    binance_rest,
    config::{AppConfig, ExchangeRulesConfig, PairRuleConfig, TriangleConfig},
    market_state::{MarketDepthView, MarketState},
    models::{self, DepthStreamWrapper},
    orderbook::{ApplyOutcome, OrderBookDiff, PriceLevel},
    Clients, SignalLogSender,
};
use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info, warn};
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use rust_decimal::Decimal;
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
    profit: Decimal,
}

struct TriangleOutputs {
    ws_payload: String,
    signal_line: String,
}

#[derive(Clone)]
struct BufferedDiffUpdate {
    stream_name: String,
    diff: OrderBookDiff,
}

#[derive(Clone, Copy)]
struct TriangleBookTiming {
    signal_timestamp_ms: u64,
    book_receive_timestamp_ms_by_leg: [u64; 3],
    book_age_ms_by_leg: [u64; 3],
    min_book_age_ms: u64,
    max_book_age_ms: u64,
    max_book_age_threshold_ms: u64,
    book_freshness_passed: bool,
}

#[derive(Clone, Copy)]
enum TradeSide {
    SellBase,
    BuyBase,
}

struct ExecutionFilterOutcome {
    assumed_start_amount: Decimal,
    executable_profit_bps: Decimal,
    adjusted_profit_bps: Decimal,
    latency_penalty_bps: Decimal,
    execution_filter_passed: bool,
    rejection_reasons: Vec<String>,
    fee_bps_by_leg: [Decimal; 3],
}

pub async fn main_worker(
    clients: Clients,
    config: AppConfig,
    mut socket: BinanceSocket,
    signal_log_sender: Option<SignalLogSender>,
) {
    let mut market_state = MarketState::with_capacity(config.depth_streams.len());
    let rest_client = reqwest::Client::new();
    let mut pending_diffs: HashMap<String, Vec<BufferedDiffUpdate>> =
        HashMap::with_capacity(config.depth_streams.len());
    let snapshot_limit = config.results_limit.clamp(1, 5_000) as u16;
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
                let parsed: models::DiffDepthStreamWrapper =
                    match serde_json::from_str(text.as_ref()) {
                        Ok(parsed) => parsed,
                        Err(e) => {
                            warn!("failed to parse Binance diff depth payload: {}", e);
                            continue;
                        }
                    };
                let receive_time_ms = now_unix_ms();
                let pair_key = pair_key_from_stream(&parsed.stream).to_string();
                let diff = diff_from_stream_event(&parsed, receive_time_ms);

                debug!(
                    "received {} diff ({} bids / {} asks, {}-{})",
                    parsed.stream,
                    parsed.data.bids.len(),
                    parsed.data.asks.len(),
                    parsed.data.first_update_id,
                    parsed.data.final_update_id
                );

                if market_state.is_pair_synced(&pair_key) {
                    match market_state.apply_orderbook_diff(&pair_key, &parsed.stream, diff.clone())
                    {
                        Ok(ApplyOutcome::Applied | ApplyOutcome::IgnoredStale) => {}
                        Err(err) => {
                            warn!(
                                "order book diff apply failed for {}: {:?}; resyncing",
                                pair_key, err
                            );
                            market_state.mark_pair_unsynced(&pair_key);
                            let buf = pending_diffs.entry(pair_key.clone()).or_default();
                            buf.clear();
                            buf.push(BufferedDiffUpdate {
                                stream_name: parsed.stream.clone(),
                                diff,
                            });
                            if let Err(e) = sync_pair_from_snapshot(
                                &rest_client,
                                &mut market_state,
                                &mut pending_diffs,
                                &pair_key,
                                snapshot_limit,
                            )
                            .await
                            {
                                warn!("failed to resync {} from snapshot: {}", pair_key, e);
                            }
                        }
                    }
                } else {
                    let buf = pending_diffs.entry(pair_key.clone()).or_default();
                    buf.push(BufferedDiffUpdate {
                        stream_name: parsed.stream.clone(),
                        diff,
                    });
                    trim_pending_buffer(buf, 2_048);
                    if let Err(e) = sync_pair_from_snapshot(
                        &rest_client,
                        &mut market_state,
                        &mut pending_diffs,
                        &pair_key,
                        snapshot_limit,
                    )
                    .await
                    {
                        warn!("failed to sync {} from snapshot: {}", pair_key, e);
                    }
                }
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
            publish_triangle_updates(&clients, &config, &market_state, signal_log_sender.as_ref())
                .await
        {
            warn!("failed to publish triangle updates: {}", e);
        }
    }
}

async fn publish_triangle_updates(
    clients: &Clients,
    config: &AppConfig,
    market_state: &MarketState,
    signal_log_sender: Option<&SignalLogSender>,
) -> Result<(), serde_json::Error> {
    let recipients = {
        let locked = clients.read().await;
        locked
            .iter()
            .map(|(id, client)| (id.clone(), client.sender.clone()))
            .collect::<Vec<_>>()
    };

    if recipients.is_empty() && signal_log_sender.is_none() {
        return Ok(());
    }

    let mut stale_client_ids = HashSet::new();
    let depth_limit = config.results_limit.max(1) as usize;
    let signal_timestamp_ms = now_unix_ms();

    for triangle in &config.triangles {
        let Some(start_view) = market_state.export_depth_view(&triangle.pairs[0], depth_limit)
        else {
            continue;
        };
        let Some(mid_view) = market_state.export_depth_view(&triangle.pairs[1], depth_limit) else {
            continue;
        };
        let Some(end_view) = market_state.export_depth_view(&triangle.pairs[2], depth_limit) else {
            continue;
        };

        let book_timing = build_triangle_book_timing(
            signal_timestamp_ms,
            [&start_view, &mid_view, &end_view],
            config.max_book_age_ms,
        );

        let Some(outputs) = build_triangle_outputs(
            triangle,
            &start_view.depth,
            &mid_view.depth,
            &end_view.depth,
            config,
            book_timing,
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
    triangle_config: &TriangleConfig,
    start_pair_data: &DepthStreamWrapper,
    mid_pair_data: &DepthStreamWrapper,
    end_pair_data: &DepthStreamWrapper,
    config: &AppConfig,
    book_timing: TriangleBookTiming,
) -> Result<Option<TriangleOutputs>, serde_json::Error> {
    let [start_pair, mid_pair, end_pair] = &triangle_config.pairs;
    let [part_a, part_b, part_c] = &triangle_config.parts;

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

    let profits_dec = profit_points
        .iter()
        .map(|point| point.profit)
        .collect::<Vec<_>>();
    let profits = profits_dec
        .iter()
        .map(|value| dec_to_f64(*value))
        .collect::<Vec<_>>();
    let best_point = profit_points
        .iter()
        .copied()
        .max_by(|a, b| a.profit.cmp(&b.profit))
        .expect("profit_points not empty");
    let profitable_levels = profit_points
        .iter()
        .filter(|point| point.profit > Decimal::ZERO)
        .count();
    let hit_rate = if profit_points.is_empty() {
        Decimal::ZERO
    } else {
        dec_from_usize(profitable_levels) / dec_from_usize(profit_points.len())
    };
    let avg_profit_dec =
        profits_dec.iter().copied().sum::<Decimal>() / dec_from_usize(profits_dec.len());
    let top_profit_dec = profits_dec.first().copied().unwrap_or(Decimal::ZERO);
    let best_profit_bps = profit_to_bps(best_point.profit);
    let top_profit_bps = profit_to_bps(top_profit_dec);
    let avg_profit_bps = profit_to_bps(avg_profit_dec);
    let execution_outcome = evaluate_execution_filters(
        triangle_config,
        start_pair_data,
        mid_pair_data,
        end_pair_data,
        best_point.level_index,
        &config.exchange_rules,
    );
    let worthy = execution_outcome.execution_filter_passed
        && execution_outcome.adjusted_profit_bps >= dec_from_f64(config.signal_min_profit_bps)
        && hit_rate >= dec_from_f64(config.signal_min_hit_rate)
        && book_timing.book_freshness_passed;

    if best_point.profit > Decimal::ZERO {
        info!(
            target: "profit",
            "{:?} best profit: {:.5}% ({} {})",
            [part_a, part_b, part_c],
            dec_to_f64(best_point.profit * dec_from_i64(100)),
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
        book_timing,
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
    hit_rate: Decimal,
    top_profit_bps: Decimal,
    best_profit_bps: Decimal,
    avg_profit_bps: Decimal,
    execution_outcome: &ExecutionFilterOutcome,
    book_timing: TriangleBookTiming,
    worthy: bool,
    min_profit_bps_threshold: f64,
    min_hit_rate_threshold: f64,
) -> models::TriangleOpportunitySignal {
    let i = best_point.level_index;
    let mut rejection_reasons = execution_outcome.rejection_reasons.clone();
    if !book_timing.book_freshness_passed {
        rejection_reasons.push(format!(
            "stale_books max_age_ms={} threshold_ms={}",
            book_timing.max_book_age_ms, book_timing.max_book_age_threshold_ms
        ));
    }
    models::TriangleOpportunitySignal {
        timestamp_ms: book_timing.signal_timestamp_ms,
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
        book_receive_timestamp_ms_by_leg: book_timing.book_receive_timestamp_ms_by_leg,
        book_age_ms_by_leg: book_timing.book_age_ms_by_leg,
        min_book_age_ms: book_timing.min_book_age_ms,
        max_book_age_ms: book_timing.max_book_age_ms,
        book_freshness_passed: book_timing.book_freshness_passed,
        execution_filter_passed: execution_outcome.execution_filter_passed,
        worthy,
        min_profit_bps_threshold: dec_from_f64(min_profit_bps_threshold),
        min_hit_rate_threshold: dec_from_f64(min_hit_rate_threshold),
        rejection_reasons,
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

fn pair_key_from_stream(stream: &str) -> &str {
    stream.split_once('@').map_or(stream, |(pair, _)| pair)
}

fn diff_from_stream_event(
    event: &models::DiffDepthStreamWrapper,
    receive_time_ms: u64,
) -> OrderBookDiff {
    OrderBookDiff {
        first_update_id: event.data.first_update_id,
        final_update_id: event.data.final_update_id,
        prev_final_update_id: event.data.prev_final_update_id,
        bids: event
            .data
            .bids
            .iter()
            .map(|offer| PriceLevel {
                price: offer.price,
                qty: offer.size,
            })
            .collect(),
        asks: event
            .data
            .asks
            .iter()
            .map(|offer| PriceLevel {
                price: offer.price,
                qty: offer.size,
            })
            .collect(),
        event_time_ms: Some(event.data.event_time_ms),
        receive_time_ms: Some(receive_time_ms),
    }
}

fn trim_pending_buffer(buffer: &mut Vec<BufferedDiffUpdate>, max_len: usize) {
    if buffer.len() <= max_len {
        return;
    }
    let drop_count = buffer.len() - max_len;
    buffer.drain(0..drop_count);
}

async fn sync_pair_from_snapshot(
    rest_client: &reqwest::Client,
    market_state: &mut MarketState,
    pending_diffs: &mut HashMap<String, Vec<BufferedDiffUpdate>>,
    pair_key: &str,
    snapshot_limit: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(buffer) = pending_diffs.get_mut(pair_key) else {
        return Ok(());
    };
    if buffer.is_empty() {
        return Ok(());
    }

    let latest_stream_name = buffer
        .last()
        .map(|u| u.stream_name.clone())
        .unwrap_or_else(|| format!("{pair_key}@depth@100ms"));

    // Retry a few times because the REST snapshot can lag behind the already-buffered diff stream.
    for _ in 0..3 {
        let snapshot =
            binance_rest::fetch_depth_snapshot(rest_client, pair_key, snapshot_limit).await?;

        market_state.apply_orderbook_snapshot(
            pair_key,
            latest_stream_name.clone(),
            snapshot.last_update_id,
            snapshot
                .bids
                .into_iter()
                .map(|level| PriceLevel {
                    price: level.price,
                    qty: level.qty,
                })
                .collect(),
            snapshot
                .asks
                .into_iter()
                .map(|level| PriceLevel {
                    price: level.price,
                    qty: level.qty,
                })
                .collect(),
            snapshot.fetched_at_ms,
        );

        match apply_buffered_diffs_after_snapshot(market_state, pair_key, buffer) {
            Ok(true) => return Ok(()),
            Ok(false) => {
                // snapshot is older than buffered stream start; fetch a newer one
                continue;
            }
            Err(err) => {
                market_state.mark_pair_unsynced(pair_key);
                return Err(format!("buffered diff replay failed for {pair_key}: {err:?}").into());
            }
        }
    }

    market_state.mark_pair_unsynced(pair_key);
    Err(format!("unable to obtain fresh enough snapshot for {pair_key} after retries").into())
}

fn apply_buffered_diffs_after_snapshot(
    market_state: &mut MarketState,
    pair_key: &str,
    buffer: &mut Vec<BufferedDiffUpdate>,
) -> Result<bool, crate::orderbook::OrderBookError> {
    let Some(last_update_id) = market_state
        .export_depth_view(pair_key, 1)
        .map(|view| view.depth.data.last_update_id)
    else {
        return Ok(false);
    };

    if let Some(first) = buffer.first() {
        if first.diff.first_update_id > last_update_id.saturating_add(1) {
            return Ok(false);
        }
    }

    // Discard stale events and locate the first event that bridges the snapshot.
    buffer.retain(|u| u.diff.final_update_id > last_update_id);
    let bridge_idx = buffer.iter().position(|u| {
        if let Some(prev_final) = u.diff.prev_final_update_id {
            prev_final == last_update_id
        } else {
            u.diff.first_update_id <= last_update_id.saturating_add(1)
                && u.diff.final_update_id >= last_update_id.saturating_add(1)
        }
    });

    let Some(bridge_idx) = bridge_idx else {
        return Ok(false);
    };

    if bridge_idx > 0 {
        buffer.drain(0..bridge_idx);
    }

    let replay = std::mem::take(buffer);
    for update in replay {
        market_state.apply_orderbook_diff(pair_key, &update.stream_name, update.diff)?;
    }
    Ok(true)
}

fn now_unix_ms() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis().min(u128::from(u64::MAX)) as u64,
        Err(_) => 0,
    }
}

fn build_triangle_book_timing(
    signal_timestamp_ms: u64,
    views: [&MarketDepthView; 3],
    max_book_age_threshold_ms: u64,
) -> TriangleBookTiming {
    let receive = [
        views[0].last_receive_time_ms,
        views[1].last_receive_time_ms,
        views[2].last_receive_time_ms,
    ];
    let ages = receive.map(|ts| {
        if ts == 0 {
            u64::MAX
        } else {
            signal_timestamp_ms.saturating_sub(ts)
        }
    });
    let min_age = *ages.iter().min().unwrap_or(&0);
    let max_age = *ages.iter().max().unwrap_or(&0);
    let freshness_passed = if max_book_age_threshold_ms == 0 {
        true
    } else {
        max_age <= max_book_age_threshold_ms
    };

    TriangleBookTiming {
        signal_timestamp_ms,
        book_receive_timestamp_ms_by_leg: receive,
        book_age_ms_by_leg: ages,
        min_book_age_ms: min_age,
        max_book_age_ms: max_age,
        max_book_age_threshold_ms,
        book_freshness_passed: freshness_passed,
    }
}

fn dec_from_f64(value: f64) -> Decimal {
    Decimal::from_f64(value).unwrap_or(Decimal::ZERO)
}

fn dec_from_i64(value: i64) -> Decimal {
    Decimal::from_i64(value).unwrap_or(Decimal::ZERO)
}

fn dec_from_usize(value: usize) -> Decimal {
    Decimal::from_usize(value).unwrap_or(Decimal::ONE)
}

fn dec_to_f64(value: Decimal) -> f64 {
    value.to_f64().unwrap_or(0.0)
}

fn dec_epsilon() -> Decimal {
    Decimal::new(1, 18)
}

fn profit_to_bps(profit: Decimal) -> Decimal {
    profit * dec_from_i64(10_000)
}

fn evaluate_execution_filters(
    triangle_config: &TriangleConfig,
    start_pair_data: &DepthStreamWrapper,
    mid_pair_data: &DepthStreamWrapper,
    end_pair_data: &DepthStreamWrapper,
    level_index: usize,
    rules: &ExchangeRulesConfig,
) -> ExecutionFilterOutcome {
    let fee_bps_by_leg_dec = [
        fee_bps_for_pair(rules, &triangle_config.pairs[0]),
        fee_bps_for_pair(rules, &triangle_config.pairs[1]),
        fee_bps_for_pair(rules, &triangle_config.pairs[2]),
    ];
    let fee_bps_by_leg = fee_bps_by_leg_dec;
    let assumed_start_amount_dec = assumed_start_amount_for_asset(rules, &triangle_config.parts[0]);
    let assumed_start_amount = assumed_start_amount_dec;

    if !rules.enabled {
        let raw_profit = simulate_triangle_path_amount(
            triangle_config,
            start_pair_data,
            mid_pair_data,
            end_pair_data,
            level_index,
            assumed_start_amount_dec,
            &fee_bps_by_leg_dec,
            false,
            rules,
        );
        let executable_profit_bps = raw_profit
            .map(|final_amount| {
                profit_to_bps((final_amount / assumed_start_amount_dec) - Decimal::ONE)
            })
            .unwrap_or(Decimal::ZERO);
        return ExecutionFilterOutcome {
            assumed_start_amount,
            executable_profit_bps,
            adjusted_profit_bps: executable_profit_bps,
            latency_penalty_bps: Decimal::ZERO,
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
        assumed_start_amount_dec,
        &fee_bps_by_leg_dec,
        true,
        rules,
    );

    let (execution_filter_passed, executable_profit_bps) = match simulated {
        Ok(final_amount) => (
            true,
            profit_to_bps((final_amount / assumed_start_amount_dec) - Decimal::ONE),
        ),
        Err(reason) => {
            rejection_reasons.push(reason);
            (false, Decimal::ZERO)
        }
    };

    let latency_penalty_bps = rules.latency_penalty_bps.max(Decimal::ZERO);
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
    initial_amount: Decimal,
    fee_bps_by_leg: &[Decimal; 3],
    enforce_rules: bool,
    rules: &ExchangeRulesConfig,
) -> Result<Decimal, String> {
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
    trade_amount: Decimal,
    pair_name: &str,
    triangle_part: &str,
    ask: &models::OfferData,
    bid: &models::OfferData,
    fee_bps: Decimal,
    enforce_rules: bool,
    rules: &ExchangeRulesConfig,
) -> Result<Decimal, String> {
    if trade_amount <= Decimal::ZERO {
        return Err(format!("invalid input amount for {pair_name}"));
    }

    let pair_rule = pair_rule_for_pair(rules, pair_name);
    let fee_multiplier = Decimal::ONE - (fee_bps.max(Decimal::ZERO) / dec_from_i64(10_000));
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
            if base_qty <= Decimal::ZERO {
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

fn fee_bps_for_pair(rules: &ExchangeRulesConfig, pair_name: &str) -> Decimal {
    pair_rule_for_pair(rules, pair_name)
        .and_then(|rule| rule.fee_bps)
        .unwrap_or(rules.default_fee_bps)
        .max(Decimal::ZERO)
}

fn assumed_start_amount_for_asset(rules: &ExchangeRulesConfig, asset: &str) -> Decimal {
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
        .max(dec_epsilon())
}

fn apply_qty_rounding(quantity: Decimal, pair_rule: Option<&PairRuleConfig>) -> Decimal {
    let Some(step) = pair_rule.and_then(|rule| rule.qty_step) else {
        return quantity;
    };
    if step <= Decimal::ZERO {
        return quantity;
    }
    floor_to_step(quantity, step)
}

fn floor_to_step(value: Decimal, step: Decimal) -> Decimal {
    if step <= Decimal::ZERO {
        return value;
    }
    ((value / step).floor()) * step
}

fn validate_base_qty_rules(
    pair_name: &str,
    base_qty: Decimal,
    pair_rule: Option<&PairRuleConfig>,
) -> Result<(), String> {
    if base_qty <= Decimal::ZERO {
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
    notional: Decimal,
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

        if ask1 <= Decimal::ZERO || ask2 <= Decimal::ZERO || ask3 <= Decimal::ZERO {
            continue;
        }

        let mut amount =
            calc_triangle_step(Decimal::ONE, ask1, bid1, start_pair, triangle[0], fee1);
        amount = calc_triangle_step(amount, ask2, bid2, mid_pair, triangle[1], fee2);
        amount = calc_triangle_step(amount, ask3, bid3, end_pair, triangle[2], fee3);

        let profit = amount - Decimal::ONE;
        profits.push(ProfitPoint {
            level_index: i,
            profit,
        });
    }

    profits
}

fn calc_triangle_step(
    trade_amount: Decimal,
    ask_price: Decimal,
    bid_price: Decimal,
    pair_name: &str,
    triangle_part: &str,
    fee_bps: Decimal,
) -> Decimal {
    let trade_amount =
        trade_amount * (Decimal::ONE - (fee_bps.max(Decimal::ZERO) / dec_from_i64(10_000)));

    if pair_name.starts_with(triangle_part) {
        trade_amount * bid_price
    } else {
        trade_amount / ask_price
    }
}

#[cfg(test)]
mod tests {
    use super::{calc_triangle_step, dec_from_f64, dec_from_i64};
    use rust_decimal::Decimal;

    #[test]
    fn sell_side_uses_bid() {
        let amount = calc_triangle_step(
            dec_from_i64(2),
            dec_from_i64(10),
            dec_from_i64(9),
            "bnbbtc",
            "bnb",
            Decimal::ZERO,
        );
        assert_eq!(amount, dec_from_i64(18));
    }

    #[test]
    fn buy_side_uses_ask() {
        let amount = calc_triangle_step(
            Decimal::ONE,
            dec_from_i64(2),
            dec_from_f64(1.9),
            "ethbtc",
            "btc",
            Decimal::ZERO,
        );
        assert_eq!(amount, dec_from_f64(0.5));
    }
}
