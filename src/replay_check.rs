use crate::{
    market_state::MarketState,
    orderbook::{ApplyOutcome, OrderBookDiff, PriceLevel},
};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader},
};

const DEFAULT_REPLAY_PATH: &str = "data/replay_market.jsonl";

#[derive(Debug, Clone)]
struct ReplayCheckConfig {
    path: String,
    strict: bool,
}

impl Default for ReplayCheckConfig {
    fn default() -> Self {
        Self {
            path: DEFAULT_REPLAY_PATH.to_string(),
            strict: false,
        }
    }
}

#[derive(Debug, Default)]
struct ReplayStats {
    total_lines: usize,
    parsed_lines: usize,
    ignored_lines: usize,
    snapshot_lines: usize,
    diff_lines: usize,
    diffs_applied: usize,
    diffs_ignored_stale: usize,
    diff_apply_errors: usize,
    buffered_diff_lines: usize,
    buffered_replay_successes: usize,
    buffered_replay_failures: usize,
    buffered_replay_waiting_snapshot: usize,
}

#[derive(Debug, Clone)]
struct BufferedReplayDiff {
    stream_name: String,
    diff: OrderBookDiff,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ReplayRecord {
    Snapshot(ReplaySnapshotRecord),
    Diff(ReplayDiffRecord),
}

#[derive(Debug, Deserialize)]
struct ReplaySnapshotRecord {
    pair: String,
    #[serde(default)]
    stream: Option<String>,
    last_update_id: u64,
    #[serde(default)]
    timestamp_ms: u64,
    #[serde(deserialize_with = "de_price_levels_from_pairs")]
    bids: Vec<PriceLevel>,
    #[serde(deserialize_with = "de_price_levels_from_pairs")]
    asks: Vec<PriceLevel>,
}

#[derive(Debug, Deserialize)]
struct ReplayDiffRecord {
    stream: String,
    first_update_id: u64,
    final_update_id: u64,
    #[serde(default)]
    prev_final_update_id: Option<u64>,
    #[serde(default)]
    event_time_ms: Option<u64>,
    #[serde(default)]
    receive_time_ms: Option<u64>,
    #[serde(deserialize_with = "de_price_levels_from_pairs")]
    bids: Vec<PriceLevel>,
    #[serde(deserialize_with = "de_price_levels_from_pairs")]
    asks: Vec<PriceLevel>,
}

pub fn run_from_cli_args<I>(mut args: I) -> bool
where
    I: Iterator<Item = String>,
{
    let Some(command) = args.next() else {
        return false;
    };
    if command != "replay-check" {
        return false;
    }

    let cfg = match parse_args(args) {
        Ok(cfg) => cfg,
        Err(msg) => {
            eprintln!("replay-check: {}", msg);
            eprintln!("usage: cargo run -- replay-check [path.jsonl] [--strict]");
            return true;
        }
    };

    if let Err(e) = run(cfg) {
        eprintln!("replay-check failed: {}", e);
    }

    true
}

fn parse_args<I>(args: I) -> Result<ReplayCheckConfig, String>
where
    I: Iterator<Item = String>,
{
    let mut cfg = ReplayCheckConfig::default();
    let mut path_set = false;

    for arg in args {
        match arg.as_str() {
            "--strict" => cfg.strict = true,
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg}")),
            _ => {
                if path_set {
                    return Err(format!("unexpected positional argument {arg}"));
                }
                cfg.path = arg;
                path_set = true;
            }
        }
    }

    Ok(cfg)
}

fn run(cfg: ReplayCheckConfig) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::open(&cfg.path)?;
    let reader = BufReader::new(file);

    let mut market_state = MarketState::with_capacity(128);
    let mut pending_diffs: HashMap<String, Vec<BufferedReplayDiff>> = HashMap::new();
    let mut stats = ReplayStats::default();

    for line in reader.lines() {
        stats.total_lines += 1;
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let record = match serde_json::from_str::<ReplayRecord>(&line) {
            Ok(record) => record,
            Err(_) => {
                stats.ignored_lines += 1;
                continue;
            }
        };
        stats.parsed_lines += 1;

        match record {
            ReplayRecord::Snapshot(snapshot) => {
                stats.snapshot_lines += 1;
                let pair = snapshot.pair.to_ascii_lowercase();
                let stream_name = snapshot
                    .stream
                    .unwrap_or_else(|| format!("{pair}@depth@100ms"));
                market_state.apply_orderbook_snapshot(
                    &pair,
                    stream_name,
                    snapshot.last_update_id,
                    snapshot.bids,
                    snapshot.asks,
                    snapshot.timestamp_ms,
                );

                if let Some(buffer) = pending_diffs.get_mut(&pair) {
                    if !buffer.is_empty() {
                        match apply_buffered_diffs_after_snapshot_local(
                            &mut market_state,
                            &pair,
                            buffer,
                        ) {
                            Ok(true) => {
                                stats.buffered_replay_successes += 1;
                                stats.buffered_replay_waiting_snapshot += 0;
                            }
                            Ok(false) => {
                                stats.buffered_replay_waiting_snapshot += 1;
                            }
                            Err(_) => {
                                stats.buffered_replay_failures += 1;
                                market_state.mark_pair_unsynced(&pair);
                            }
                        }
                    }
                }
            }
            ReplayRecord::Diff(diff_record) => {
                stats.diff_lines += 1;
                let pair_key = pair_key_from_stream(&diff_record.stream).to_string();
                let diff = OrderBookDiff {
                    first_update_id: diff_record.first_update_id,
                    final_update_id: diff_record.final_update_id,
                    prev_final_update_id: diff_record.prev_final_update_id,
                    bids: diff_record.bids,
                    asks: diff_record.asks,
                    event_time_ms: diff_record.event_time_ms,
                    receive_time_ms: diff_record.receive_time_ms,
                };

                if market_state.is_pair_synced(&pair_key) {
                    match market_state.apply_orderbook_diff(
                        &pair_key,
                        &diff_record.stream,
                        diff.clone(),
                    ) {
                        Ok(ApplyOutcome::Applied) => stats.diffs_applied += 1,
                        Ok(ApplyOutcome::IgnoredStale) => stats.diffs_ignored_stale += 1,
                        Err(_) => {
                            stats.diff_apply_errors += 1;
                            market_state.mark_pair_unsynced(&pair_key);
                            let buf = pending_diffs.entry(pair_key.clone()).or_default();
                            buf.clear();
                            buf.push(BufferedReplayDiff {
                                stream_name: diff_record.stream,
                                diff,
                            });
                            stats.buffered_diff_lines += 1;
                        }
                    }
                } else {
                    pending_diffs
                        .entry(pair_key)
                        .or_default()
                        .push(BufferedReplayDiff {
                            stream_name: diff_record.stream,
                            diff,
                        });
                    stats.buffered_diff_lines += 1;
                }
            }
        }
    }

    let final_synced_pairs = pending_diffs
        .keys()
        .filter(|pair| market_state.is_pair_synced(pair))
        .count();
    let pending_pairs = pending_diffs.values().filter(|buf| !buf.is_empty()).count();
    let pending_updates: usize = pending_diffs.values().map(Vec::len).sum();

    println!("Replay Check");
    println!("file: {}", cfg.path);
    println!(
        "lines: total={} parsed={} ignored={}",
        stats.total_lines, stats.parsed_lines, stats.ignored_lines
    );
    println!(
        "records: snapshots={} diffs={}",
        stats.snapshot_lines, stats.diff_lines
    );
    println!(
        "diffs: applied={} ignored_stale={} apply_errors={} buffered={}",
        stats.diffs_applied,
        stats.diffs_ignored_stale,
        stats.diff_apply_errors,
        stats.buffered_diff_lines
    );
    println!(
        "buffered_replay: success={} waiting_snapshot={} failures={}",
        stats.buffered_replay_successes,
        stats.buffered_replay_waiting_snapshot,
        stats.buffered_replay_failures
    );
    println!(
        "pending: pairs={} updates={} (pairs_with_synced_book={})",
        pending_pairs, pending_updates, final_synced_pairs
    );

    if cfg.strict {
        let strict_fail =
            stats.diff_apply_errors > 0 || stats.buffered_replay_failures > 0 || pending_pairs > 0;
        if strict_fail {
            return Err(
                "strict replay-check failed (errors or pending buffered diffs remain)".into(),
            );
        }
    }

    Ok(())
}

fn pair_key_from_stream(stream: &str) -> &str {
    stream.split_once('@').map_or(stream, |(pair, _)| pair)
}

fn apply_buffered_diffs_after_snapshot_local(
    market_state: &mut MarketState,
    pair_key: &str,
    buffer: &mut Vec<BufferedReplayDiff>,
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

fn de_price_levels_from_pairs<'de, D>(deserializer: D) -> Result<Vec<PriceLevel>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Vec::<[String; 2]>::deserialize(deserializer)?;
    raw.into_iter()
        .map(|[price, qty]| {
            Ok(PriceLevel {
                price: price.parse::<Decimal>().map_err(serde::de::Error::custom)?,
                qty: qty.parse::<Decimal>().map_err(serde::de::Error::custom)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{pair_key_from_stream, parse_args, ReplayRecord};

    #[test]
    fn parse_args_defaults() {
        let cfg = parse_args(std::iter::empty::<String>()).expect("parse");
        assert_eq!(cfg.path, "data/replay_market.jsonl");
        assert!(!cfg.strict);
    }

    #[test]
    fn parse_args_custom() {
        let cfg = parse_args(vec!["tmp.jsonl".to_string(), "--strict".to_string()].into_iter())
            .expect("parse");
        assert_eq!(cfg.path, "tmp.jsonl");
        assert!(cfg.strict);
    }

    #[test]
    fn extracts_pair_key_from_stream() {
        assert_eq!(pair_key_from_stream("btcusdt@depth@100ms"), "btcusdt");
        assert_eq!(pair_key_from_stream("ethbtc"), "ethbtc");
    }

    #[test]
    fn parses_snapshot_and_diff_records() {
        let snapshot = r#"{"kind":"snapshot","pair":"btcusdt","last_update_id":100,"timestamp_ms":1,"bids":[["100","1"]],"asks":[["101","1"]]}"#;
        let diff = r#"{"kind":"diff","stream":"btcusdt@depth@100ms","first_update_id":101,"final_update_id":101,"prev_final_update_id":100,"bids":[["100","2"]],"asks":[["101","0"]]}"#;

        let parsed_snapshot: ReplayRecord = serde_json::from_str(snapshot).expect("snapshot");
        let parsed_diff: ReplayRecord = serde_json::from_str(diff).expect("diff");

        assert!(matches!(parsed_snapshot, ReplayRecord::Snapshot(_)));
        assert!(matches!(parsed_diff, ReplayRecord::Diff(_)));
    }
}
