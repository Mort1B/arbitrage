use crate::models::TriangleOpportunitySignal;
use rust_decimal::prelude::ToPrimitive;
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, Write},
};

const DEFAULT_SIGNAL_LOG_PATH: &str = "data/triangle_signals.jsonl";
const DEFAULT_TOP_ROWS: usize = 20;

#[derive(Debug, Clone)]
struct PnlReportConfig {
    path: String,
    top_n: usize,
    min_adjusted_bps: f64,
    require_worthy: bool,
    require_execution_passed: bool,
    csv_output_path: Option<String>,
}

impl Default for PnlReportConfig {
    fn default() -> Self {
        Self {
            path: DEFAULT_SIGNAL_LOG_PATH.to_string(),
            top_n: DEFAULT_TOP_ROWS,
            min_adjusted_bps: 0.0,
            require_worthy: true,
            require_execution_passed: true,
            csv_output_path: None,
        }
    }
}

#[derive(Default)]
struct TrianglePnlStats {
    start_asset: String,
    samples: usize,
    positive_adjusted_samples: usize,
    sum_raw_pnl: f64,
    sum_adjusted_pnl: f64,
    max_adjusted_pnl: f64,
    max_adjusted_bps: f64,
    sum_adjusted_bps: f64,
    last_timestamp_ms: u64,
}

pub fn run_from_cli_args<I>(mut args: I) -> bool
where
    I: Iterator<Item = String>,
{
    let Some(command) = args.next() else {
        return false;
    };

    if command != "pnl-report" {
        return false;
    }

    let cfg = match parse_args(args) {
        Ok(cfg) => cfg,
        Err(msg) => {
            eprintln!("pnl-report: {}", msg);
            eprintln!("usage: cargo run -- pnl-report [path] [--top N] [--min-adjusted-bps X] [--include-unworthy] [--include-nonexecutable] [--csv path.csv]");
            return true;
        }
    };

    if let Err(e) = run(cfg) {
        eprintln!("pnl-report failed: {}", e);
    }

    true
}

fn parse_args<I>(args: I) -> Result<PnlReportConfig, String>
where
    I: Iterator<Item = String>,
{
    let mut cfg = PnlReportConfig::default();
    let mut path_set = false;
    let mut iter = args.peekable();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--top" => {
                let Some(v) = iter.next() else {
                    return Err("missing value for --top".to_string());
                };
                cfg.top_n = v
                    .parse::<usize>()
                    .map_err(|_| "invalid integer for --top".to_string())?;
            }
            "--min-adjusted-bps" => {
                let Some(v) = iter.next() else {
                    return Err("missing value for --min-adjusted-bps".to_string());
                };
                cfg.min_adjusted_bps = v
                    .parse::<f64>()
                    .map_err(|_| "invalid number for --min-adjusted-bps".to_string())?;
            }
            "--include-unworthy" => cfg.require_worthy = false,
            "--include-nonexecutable" => cfg.require_execution_passed = false,
            "--csv" => {
                let Some(v) = iter.next() else {
                    return Err("missing value for --csv".to_string());
                };
                cfg.csv_output_path = Some(v);
            }
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

    if cfg.top_n == 0 {
        return Err("--top must be > 0".to_string());
    }

    Ok(cfg)
}

fn run(cfg: PnlReportConfig) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::open(&cfg.path)?;
    let reader = BufReader::new(file);

    let mut total_lines = 0usize;
    let mut parsed_lines = 0usize;
    let mut considered = 0usize;
    let mut by_triangle: HashMap<String, TrianglePnlStats> = HashMap::new();

    for line in reader.lines() {
        total_lines += 1;
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let signal: TriangleOpportunitySignal = match serde_json::from_str(&line) {
            Ok(s) => s,
            Err(_) => continue,
        };
        parsed_lines += 1;

        if cfg.require_execution_passed && !signal.execution_filter_passed {
            continue;
        }
        if cfg.require_worthy && !signal.worthy {
            continue;
        }
        let adjusted_profit_bps = signal.adjusted_profit_bps.to_f64().unwrap_or(0.0);
        if adjusted_profit_bps < cfg.min_adjusted_bps {
            continue;
        }
        considered += 1;

        let start_asset = signal.triangle_parts[0].clone();
        let assumed_start_amount = signal.assumed_start_amount.to_f64().unwrap_or(0.0);
        let executable_profit_bps = signal.executable_profit_bps.to_f64().unwrap_or(0.0);
        let raw_pnl = assumed_start_amount * executable_profit_bps / 10_000.0;
        let adjusted_pnl = assumed_start_amount * adjusted_profit_bps / 10_000.0;
        let key = signal.triangle_pairs.join(" -> ");

        let stats = by_triangle.entry(key).or_default();
        if stats.start_asset.is_empty() {
            stats.start_asset = start_asset;
            stats.max_adjusted_pnl = f64::NEG_INFINITY;
            stats.max_adjusted_bps = f64::NEG_INFINITY;
        }
        stats.samples += 1;
        if adjusted_profit_bps > 0.0 {
            stats.positive_adjusted_samples += 1;
        }
        stats.sum_raw_pnl += raw_pnl;
        stats.sum_adjusted_pnl += adjusted_pnl;
        stats.sum_adjusted_bps += adjusted_profit_bps;
        stats.max_adjusted_pnl = stats.max_adjusted_pnl.max(adjusted_pnl);
        stats.max_adjusted_bps = stats.max_adjusted_bps.max(adjusted_profit_bps);
        stats.last_timestamp_ms = stats.last_timestamp_ms.max(signal.timestamp_ms);
    }

    let mut rows = by_triangle.into_iter().collect::<Vec<_>>();
    rows.sort_by(|a, b| b.1.sum_adjusted_pnl.total_cmp(&a.1.sum_adjusted_pnl));

    println!("Positive PnL Opportunity Report");
    println!("file: {}", cfg.path);
    println!(
        "filters: worthy_only={} executable_only={} min_adjusted_bps={:.2}",
        cfg.require_worthy, cfg.require_execution_passed, cfg.min_adjusted_bps
    );
    println!(
        "lines: {} (parsed {}, considered {})",
        total_lines, parsed_lines, considered
    );
    println!();
    println!(
        "{:<46} {:<6} {:>7} {:>7} {:>12} {:>12} {:>10} {:>10}",
        "triangle", "asset", "samples", "pos", "sum_adj_pnl", "sum_raw_pnl", "avg_bps", "max_bps"
    );

    for (triangle, stats) in rows.iter().take(cfg.top_n) {
        let avg_bps = if stats.samples == 0 {
            0.0
        } else {
            stats.sum_adjusted_bps / stats.samples as f64
        };
        println!(
            "{:<46} {:<6} {:>7} {:>7} {:>12.6} {:>12.6} {:>10.2} {:>10.2}",
            truncate(triangle, 46),
            stats.start_asset,
            stats.samples,
            stats.positive_adjusted_samples,
            stats.sum_adjusted_pnl,
            stats.sum_raw_pnl,
            avg_bps,
            stats.max_adjusted_bps,
        );
    }

    if let Some(csv_path) = &cfg.csv_output_path {
        write_csv(csv_path, &rows, cfg.top_n)?;
        println!();
        println!("csv written: {}", csv_path);
    }

    Ok(())
}

fn write_csv(
    path: &str,
    rows: &[(String, TrianglePnlStats)],
    top_n: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut file = File::create(path)?;
    writeln!(
        file,
        "triangle,asset,samples,positive_samples,sum_adjusted_pnl,sum_raw_pnl,avg_adjusted_bps,max_adjusted_bps,last_timestamp_ms"
    )?;

    for (triangle, stats) in rows.iter().take(top_n) {
        let avg_bps = if stats.samples == 0 {
            0.0
        } else {
            stats.sum_adjusted_bps / stats.samples as f64
        };

        writeln!(
            file,
            "\"{}\",{},{},{},{:.8},{:.8},{:.8},{:.8},{}",
            csv_escape(triangle),
            stats.start_asset,
            stats.samples,
            stats.positive_adjusted_samples,
            stats.sum_adjusted_pnl,
            stats.sum_raw_pnl,
            avg_bps,
            stats.max_adjusted_bps,
            stats.last_timestamp_ms
        )?;
    }

    Ok(())
}

fn csv_escape(input: &str) -> String {
    input.replace('\"', "\"\"")
}

fn truncate(input: &str, max_len: usize) -> String {
    if input.chars().count() <= max_len {
        return input.to_string();
    }
    input
        .chars()
        .take(max_len.saturating_sub(3))
        .collect::<String>()
        + "..."
}

#[cfg(test)]
mod tests {
    use super::parse_args;

    #[test]
    fn parse_defaults() {
        let cfg = parse_args(std::iter::empty::<String>()).expect("parse");
        assert_eq!(cfg.top_n, 20);
        assert!(cfg.require_worthy);
        assert!(cfg.require_execution_passed);
        assert!(cfg.csv_output_path.is_none());
    }

    #[test]
    fn parse_custom_flags() {
        let cfg = parse_args(
            vec![
                "signals.jsonl".to_string(),
                "--top".to_string(),
                "5".to_string(),
                "--min-adjusted-bps".to_string(),
                "3.5".to_string(),
                "--include-unworthy".to_string(),
                "--include-nonexecutable".to_string(),
                "--csv".to_string(),
                "out.csv".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse");

        assert_eq!(cfg.path, "signals.jsonl");
        assert_eq!(cfg.top_n, 5);
        assert_eq!(cfg.min_adjusted_bps, 3.5);
        assert!(!cfg.require_worthy);
        assert!(!cfg.require_execution_passed);
        assert_eq!(cfg.csv_output_path.as_deref(), Some("out.csv"));
    }
}
