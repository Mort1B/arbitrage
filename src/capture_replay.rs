use crate::{auto_triangles, binance_rest, config, models};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    time::{Duration, Instant},
};
use tokio_tungstenite::tungstenite::Message as BinanceMessage;

const BINANCE_WS_API: &str = "wss://stream.binance.com:9443";
const DEFAULT_OUTPUT_PATH: &str = "data/replay_market.jsonl";

#[derive(Debug, Clone)]
struct CaptureReplayConfig {
    output_path: String,
    append: bool,
    duration_secs: Option<u64>,
    max_diffs: Option<usize>,
    startup_snapshots: bool,
}

impl Default for CaptureReplayConfig {
    fn default() -> Self {
        Self {
            output_path: DEFAULT_OUTPUT_PATH.to_string(),
            append: false,
            duration_secs: None,
            max_diffs: None,
            startup_snapshots: true,
        }
    }
}

#[derive(Debug, Default)]
struct CaptureStats {
    snapshots_written: usize,
    diffs_written: usize,
    parse_errors: usize,
    non_text_messages: usize,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ReplayOutRecord {
    Snapshot(ReplayOutSnapshot),
    Diff(ReplayOutDiff),
}

#[derive(Serialize)]
struct ReplayOutSnapshot {
    pair: String,
    stream: String,
    last_update_id: u64,
    timestamp_ms: u64,
    bids: Vec<[String; 2]>,
    asks: Vec<[String; 2]>,
}

#[derive(Serialize)]
struct ReplayOutDiff {
    stream: String,
    first_update_id: u64,
    final_update_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    prev_final_update_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_time_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    receive_time_ms: Option<u64>,
    bids: Vec<[String; 2]>,
    asks: Vec<[String; 2]>,
}

pub async fn run_from_cli_args<I>(mut args: I) -> bool
where
    I: Iterator<Item = String>,
{
    let Some(command) = args.next() else {
        return false;
    };
    if command != "capture-replay" {
        return false;
    }

    let cli_cfg = match parse_args(args) {
        Ok(cfg) => cfg,
        Err(msg) => {
            eprintln!("capture-replay: {}", msg);
            eprintln!(
                "usage: cargo run -- capture-replay [output.jsonl] [--append] [--seconds N] [--max-diffs N] [--no-startup-snapshots]"
            );
            return true;
        }
    };

    if let Err(e) = run(cli_cfg).await {
        eprintln!("capture-replay failed: {}", e);
    }

    true
}

fn parse_args<I>(args: I) -> Result<CaptureReplayConfig, String>
where
    I: Iterator<Item = String>,
{
    let mut cfg = CaptureReplayConfig::default();
    let mut path_set = false;
    let mut iter = args.peekable();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--append" => cfg.append = true,
            "--no-startup-snapshots" => cfg.startup_snapshots = false,
            "--seconds" => {
                let Some(v) = iter.next() else {
                    return Err("missing value for --seconds".to_string());
                };
                let secs = v
                    .parse::<u64>()
                    .map_err(|_| "invalid integer for --seconds".to_string())?;
                if secs == 0 {
                    return Err("--seconds must be > 0".to_string());
                }
                cfg.duration_secs = Some(secs);
            }
            "--max-diffs" => {
                let Some(v) = iter.next() else {
                    return Err("missing value for --max-diffs".to_string());
                };
                let n = v
                    .parse::<usize>()
                    .map_err(|_| "invalid integer for --max-diffs".to_string())?;
                if n == 0 {
                    return Err("--max-diffs must be > 0".to_string());
                }
                cfg.max_diffs = Some(n);
            }
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg}")),
            _ => {
                if path_set {
                    return Err(format!("unexpected positional argument {arg}"));
                }
                cfg.output_path = arg;
                path_set = true;
            }
        }
    }

    Ok(cfg)
}

async fn run(cli_cfg: CaptureReplayConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config_file = File::open("config.yaml")?;
    let mut app_config: config::AppConfig = serde_yaml::from_reader(config_file)?;
    if app_config.auto_triangle_generation.enabled {
        let generated =
            auto_triangles::generate_from_binance(&app_config.auto_triangle_generation).await?;
        app_config.depth_streams = generated.depth_streams;
        app_config.triangles = generated.triangles;
    }
    if app_config.depth_streams.is_empty() {
        return Err("config.yaml has no depth_streams to subscribe to".into());
    }

    let binance_url =
        get_binance_streams_url(&app_config.depth_streams, app_config.update_interval);
    let mut writer = open_writer(&cli_cfg)?;
    let rest_client = reqwest::Client::new();
    let snapshot_limit = app_config.results_limit.clamp(1, 5_000) as u16;

    println!("Capture Replay");
    println!("output: {}", cli_cfg.output_path);
    println!("streams: {}", app_config.depth_streams.len());
    println!("ws: {}", binance_url);
    if let Some(secs) = cli_cfg.duration_secs {
        println!("duration_secs: {}", secs);
    }
    if let Some(max_diffs) = cli_cfg.max_diffs {
        println!("max_diffs: {}", max_diffs);
    }

    if cli_cfg.startup_snapshots {
        for pair in &app_config.depth_streams {
            let snapshot =
                binance_rest::fetch_depth_snapshot(&rest_client, pair, snapshot_limit).await?;
            let record = ReplayOutRecord::Snapshot(ReplayOutSnapshot {
                pair: snapshot.symbol.clone(),
                stream: format!("{}@depth@{}ms", snapshot.symbol, app_config.update_interval),
                last_update_id: snapshot.last_update_id,
                timestamp_ms: snapshot.fetched_at_ms,
                bids: snapshot
                    .bids
                    .into_iter()
                    .map(|l| [l.price.to_string(), l.qty.to_string()])
                    .collect(),
                asks: snapshot
                    .asks
                    .into_iter()
                    .map(|l| [l.price.to_string(), l.qty.to_string()])
                    .collect(),
            });
            write_record(&mut writer, &record)?;
        }
    }

    let (mut socket, _response) = tokio_tungstenite::connect_async(&binance_url).await?;
    let started = Instant::now();
    let mut stats = CaptureStats {
        snapshots_written: if cli_cfg.startup_snapshots {
            app_config.depth_streams.len()
        } else {
            0
        },
        ..CaptureStats::default()
    };

    loop {
        if let Some(secs) = cli_cfg.duration_secs {
            if started.elapsed() >= Duration::from_secs(secs) {
                break;
            }
        }
        if let Some(max_diffs) = cli_cfg.max_diffs {
            if stats.diffs_written >= max_diffs {
                break;
            }
        }

        let Some(msg) = socket.next().await else {
            break;
        };
        let msg = msg?;
        match msg {
            BinanceMessage::Text(text) => {
                let parsed: models::DiffDepthStreamWrapper =
                    match serde_json::from_str(text.as_ref()) {
                        Ok(parsed) => parsed,
                        Err(_) => {
                            stats.parse_errors += 1;
                            continue;
                        }
                    };

                let record = ReplayOutRecord::Diff(ReplayOutDiff {
                    stream: parsed.stream,
                    first_update_id: parsed.data.first_update_id,
                    final_update_id: parsed.data.final_update_id,
                    prev_final_update_id: parsed.data.prev_final_update_id,
                    event_time_ms: Some(parsed.data.event_time_ms),
                    receive_time_ms: Some(now_unix_ms()),
                    bids: parsed
                        .data
                        .bids
                        .into_iter()
                        .map(|o| [o.price.to_string(), o.size.to_string()])
                        .collect(),
                    asks: parsed
                        .data
                        .asks
                        .into_iter()
                        .map(|o| [o.price.to_string(), o.size.to_string()])
                        .collect(),
                });
                write_record(&mut writer, &record)?;
                stats.diffs_written += 1;
            }
            BinanceMessage::Ping(payload) => {
                socket.send(BinanceMessage::Pong(payload)).await?;
            }
            BinanceMessage::Pong(_) => {}
            BinanceMessage::Close(_) => break,
            _ => {
                stats.non_text_messages += 1;
            }
        }
    }

    writer.flush()?;
    println!(
        "written: snapshots={} diffs={} parse_errors={} non_text={}",
        stats.snapshots_written, stats.diffs_written, stats.parse_errors, stats.non_text_messages
    );

    Ok(())
}

fn open_writer(
    cfg: &CaptureReplayConfig,
) -> Result<BufWriter<File>, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(parent) = std::path::Path::new(&cfg.output_path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let file = if cfg.append {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&cfg.output_path)?
    } else {
        File::create(&cfg.output_path)?
    };
    Ok(BufWriter::new(file))
}

fn write_record(
    writer: &mut BufWriter<File>,
    record: &ReplayOutRecord,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    serde_json::to_writer(&mut *writer, record)?;
    writeln!(&mut *writer)?;
    Ok(())
}

fn get_binance_streams_url(depth_streams: &[String], update_interval: u32) -> String {
    let joined = depth_streams
        .iter()
        .map(|stream| format!("{stream}@depth@{update_interval}ms"))
        .collect::<Vec<_>>()
        .join("/");
    format!("{BINANCE_WS_API}/stream?streams={joined}")
}

fn now_unix_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis().min(u128::from(u64::MAX)) as u64,
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        get_binance_streams_url, parse_args, ReplayOutDiff, ReplayOutRecord, ReplayOutSnapshot,
    };

    #[test]
    fn parse_args_defaults() {
        let cfg = parse_args(std::iter::empty::<String>()).expect("parse");
        assert_eq!(cfg.output_path, "data/replay_market.jsonl");
        assert!(!cfg.append);
        assert!(cfg.startup_snapshots);
        assert!(cfg.duration_secs.is_none());
        assert!(cfg.max_diffs.is_none());
    }

    #[test]
    fn parse_args_custom() {
        let cfg = parse_args(
            vec![
                "out.jsonl".to_string(),
                "--append".to_string(),
                "--seconds".to_string(),
                "30".to_string(),
                "--max-diffs".to_string(),
                "100".to_string(),
                "--no-startup-snapshots".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse");
        assert_eq!(cfg.output_path, "out.jsonl");
        assert!(cfg.append);
        assert_eq!(cfg.duration_secs, Some(30));
        assert_eq!(cfg.max_diffs, Some(100));
        assert!(!cfg.startup_snapshots);
    }

    #[test]
    fn builds_combined_stream_url() {
        let url = get_binance_streams_url(&["btcusdt".to_string(), "ethusdt".to_string()], 100);
        assert!(url.contains("btcusdt@depth@100ms/ethusdt@depth@100ms"));
    }

    #[test]
    fn serializes_replay_records_with_kind_tag() {
        let snapshot = ReplayOutRecord::Snapshot(ReplayOutSnapshot {
            pair: "btcusdt".to_string(),
            stream: "btcusdt@depth@100ms".to_string(),
            last_update_id: 1,
            timestamp_ms: 1,
            bids: vec![["100".to_string(), "1".to_string()]],
            asks: vec![["101".to_string(), "1".to_string()]],
        });
        let diff = ReplayOutRecord::Diff(ReplayOutDiff {
            stream: "btcusdt@depth@100ms".to_string(),
            first_update_id: 2,
            final_update_id: 2,
            prev_final_update_id: Some(1),
            event_time_ms: Some(10),
            receive_time_ms: Some(11),
            bids: vec![["100".to_string(), "2".to_string()]],
            asks: vec![],
        });
        let s = serde_json::to_string(&snapshot).expect("snapshot json");
        let d = serde_json::to_string(&diff).expect("diff json");
        assert!(s.contains("\"kind\":\"snapshot\""));
        assert!(d.contains("\"kind\":\"diff\""));
    }
}
