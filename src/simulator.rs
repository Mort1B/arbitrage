use crate::models::TriangleOpportunitySignal;
use rust_decimal::prelude::ToPrimitive;
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader},
};

const DEFAULT_SIGNAL_LOG_PATH: &str = "data/triangle_signals.jsonl";
const DEFAULT_RETRIGGER_COOLDOWN_MS: u64 = 1_500;
const DEFAULT_MIN_ADJUSTED_PROFIT_BPS: f64 = 0.0;
const DEFAULT_TOP_ROWS: usize = 15;

#[derive(Debug, Clone)]
struct SimConfig {
    path: String,
    retrigger_cooldown_ms: u64,
    min_adjusted_profit_bps: f64,
    include_unworthy: bool,
    position_size_pct: f64,
    max_position_vs_signal: f64,
    auto_seed_assets: bool,
    seed_multiplier: f64,
    initial_balances: HashMap<String, f64>,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            path: DEFAULT_SIGNAL_LOG_PATH.to_string(),
            retrigger_cooldown_ms: DEFAULT_RETRIGGER_COOLDOWN_MS,
            min_adjusted_profit_bps: DEFAULT_MIN_ADJUSTED_PROFIT_BPS,
            include_unworthy: false,
            position_size_pct: 1.0,
            max_position_vs_signal: 1.0,
            auto_seed_assets: true,
            seed_multiplier: 1.0,
            initial_balances: HashMap::new(),
        }
    }
}

#[derive(Default)]
struct SummaryStats {
    total_lines: usize,
    parsed_lines: usize,
    executable_signals: usize,
    worthy_signals: usize,
    executed_trades: usize,
    skipped_not_executable: usize,
    skipped_not_worthy: usize,
    skipped_profit_threshold: usize,
    skipped_cooldown: usize,
    skipped_no_balance: usize,
    wins: usize,
    losses_or_flat: usize,
    sum_adjusted_bps: f64,
    total_notional_deployed: f64,
}

#[derive(Default)]
struct AssetStats {
    trades: usize,
    wins: usize,
    losses_or_flat: usize,
    sum_pnl_asset: f64,
    sum_adjusted_bps: f64,
    sum_notional_deployed: f64,
    max_adjusted_bps: f64,
    min_adjusted_bps: f64,
}

#[derive(Default)]
struct TriangleStats {
    start_asset: String,
    trades: usize,
    wins: usize,
    sum_pnl_asset: f64,
    sum_adjusted_bps: f64,
    sum_notional_deployed: f64,
    max_adjusted_bps: f64,
}

pub fn run_from_cli_args<I>(mut args: I) -> bool
where
    I: Iterator<Item = String>,
{
    let Some(command) = args.next() else {
        return false;
    };

    if command != "simulate" {
        return false;
    }

    let config = match parse_args(args) {
        Ok(config) => config,
        Err(msg) => {
            eprintln!("simulate: {}", msg);
            eprintln!("usage: cargo run -- simulate [path] [--cooldown-ms N] [--min-adjusted-bps X] [--include-unworthy] [--position-size-pct P] [--max-position-vs-signal M] [--balance asset=amount] [--no-auto-seed] [--seed-multiplier M]");
            return true;
        }
    };

    if let Err(e) = simulate_file(&config) {
        eprintln!("simulate failed for {}: {}", config.path, e);
    }

    true
}

fn parse_args<I>(args: I) -> Result<SimConfig, String>
where
    I: Iterator<Item = String>,
{
    let mut config = SimConfig::default();
    let mut path_set = false;
    let mut iter = args.peekable();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--cooldown-ms" => {
                let Some(value) = iter.next() else {
                    return Err("missing value for --cooldown-ms".to_string());
                };
                config.retrigger_cooldown_ms = value
                    .parse::<u64>()
                    .map_err(|_| "invalid integer for --cooldown-ms".to_string())?;
            }
            "--min-adjusted-bps" => {
                let Some(value) = iter.next() else {
                    return Err("missing value for --min-adjusted-bps".to_string());
                };
                config.min_adjusted_profit_bps = value
                    .parse::<f64>()
                    .map_err(|_| "invalid number for --min-adjusted-bps".to_string())?;
            }
            "--include-unworthy" => {
                config.include_unworthy = true;
            }
            "--position-size-pct" => {
                let Some(value) = iter.next() else {
                    return Err("missing value for --position-size-pct".to_string());
                };
                config.position_size_pct = value
                    .parse::<f64>()
                    .map_err(|_| "invalid number for --position-size-pct".to_string())?;
            }
            "--max-position-vs-signal" => {
                let Some(value) = iter.next() else {
                    return Err("missing value for --max-position-vs-signal".to_string());
                };
                config.max_position_vs_signal = value
                    .parse::<f64>()
                    .map_err(|_| "invalid number for --max-position-vs-signal".to_string())?;
            }
            "--no-auto-seed" => {
                config.auto_seed_assets = false;
            }
            "--seed-multiplier" => {
                let Some(value) = iter.next() else {
                    return Err("missing value for --seed-multiplier".to_string());
                };
                config.seed_multiplier = value
                    .parse::<f64>()
                    .map_err(|_| "invalid number for --seed-multiplier".to_string())?;
            }
            "--balance" => {
                let Some(value) = iter.next() else {
                    return Err("missing value for --balance".to_string());
                };
                let (asset, amount) = parse_balance_arg(&value)?;
                config.initial_balances.insert(asset, amount);
            }
            _ if arg.starts_with("--") => {
                return Err(format!("unknown option {arg}"));
            }
            _ => {
                if path_set {
                    return Err(format!("unexpected positional argument {arg}"));
                }
                config.path = arg;
                path_set = true;
            }
        }
    }

    if !(0.0..=1.0).contains(&config.position_size_pct) || config.position_size_pct == 0.0 {
        return Err("--position-size-pct must be > 0 and <= 1".to_string());
    }
    if config.max_position_vs_signal <= 0.0 {
        return Err("--max-position-vs-signal must be > 0".to_string());
    }
    if config.seed_multiplier <= 0.0 {
        return Err("--seed-multiplier must be > 0".to_string());
    }

    Ok(config)
}

fn simulate_file(config: &SimConfig) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::open(&config.path)?;
    let reader = BufReader::new(file);

    let mut summary = SummaryStats::default();
    let mut last_exec_ts_by_triangle: HashMap<String, u64> = HashMap::new();
    let mut active_state_by_triangle: HashMap<String, bool> = HashMap::new();
    let mut by_asset: HashMap<String, AssetStats> = HashMap::new();
    let mut by_triangle: HashMap<String, TriangleStats> = HashMap::new();
    let mut balances = config.initial_balances.clone();

    for line in reader.lines() {
        summary.total_lines += 1;
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let signal: TriangleOpportunitySignal = match serde_json::from_str(&line) {
            Ok(signal) => signal,
            Err(_) => continue,
        };
        summary.parsed_lines += 1;

        if signal.execution_filter_passed {
            summary.executable_signals += 1;
        }
        if signal.worthy {
            summary.worthy_signals += 1;
        }

        if !signal.execution_filter_passed {
            summary.skipped_not_executable += 1;
            set_active(&mut active_state_by_triangle, &signal, false);
            continue;
        }

        if !config.include_unworthy && !signal.worthy {
            summary.skipped_not_worthy += 1;
            set_active(&mut active_state_by_triangle, &signal, false);
            continue;
        }

        let adjusted_profit_bps = signal.adjusted_profit_bps.to_f64().unwrap_or(0.0);
        let assumed_start_amount = signal.assumed_start_amount.to_f64().unwrap_or(0.0);

        if adjusted_profit_bps < config.min_adjusted_profit_bps {
            summary.skipped_profit_threshold += 1;
            set_active(&mut active_state_by_triangle, &signal, false);
            continue;
        }

        let triangle_key = signal.triangle_pairs.join(" -> ");
        let was_active = *active_state_by_triangle
            .get(&triangle_key)
            .unwrap_or(&false);
        let cooldown_ready = last_exec_ts_by_triangle
            .get(&triangle_key)
            .is_none_or(|ts| {
                signal.timestamp_ms.saturating_sub(*ts) >= config.retrigger_cooldown_ms
            });

        if was_active && !cooldown_ready {
            summary.skipped_cooldown += 1;
            active_state_by_triangle.insert(triangle_key, true);
            continue;
        }

        if !was_active && !cooldown_ready {
            summary.skipped_cooldown += 1;
            active_state_by_triangle.insert(triangle_key, true);
            continue;
        }

        let start_asset = signal.triangle_parts[0].clone();
        let asset_key = normalize_asset_key(&start_asset);
        seed_balance_if_needed(&mut balances, &asset_key, &signal, config);
        let available_balance = *balances.get(&asset_key).unwrap_or(&0.0);
        let signal_cap = assumed_start_amount * config.max_position_vs_signal;
        let desired_position = (available_balance * config.position_size_pct).min(signal_cap);
        let position_size = desired_position.min(available_balance);

        if position_size <= 0.0 || !position_size.is_finite() {
            summary.skipped_no_balance += 1;
            set_active(&mut active_state_by_triangle, &signal, false);
            continue;
        }

        let pnl_asset = position_size * adjusted_profit_bps / 10_000.0;
        let is_win = adjusted_profit_bps > 0.0;
        if let Some(balance) = balances.get_mut(&asset_key) {
            *balance += pnl_asset;
        }

        summary.executed_trades += 1;
        summary.sum_adjusted_bps += adjusted_profit_bps;
        summary.total_notional_deployed += position_size;
        if is_win {
            summary.wins += 1;
        } else {
            summary.losses_or_flat += 1;
        }

        let asset_stats = by_asset
            .entry(start_asset.clone())
            .or_insert_with(|| AssetStats {
                min_adjusted_bps: f64::INFINITY,
                ..AssetStats::default()
            });
        asset_stats.trades += 1;
        asset_stats.sum_pnl_asset += pnl_asset;
        asset_stats.sum_adjusted_bps += adjusted_profit_bps;
        asset_stats.sum_notional_deployed += position_size;
        asset_stats.max_adjusted_bps = asset_stats.max_adjusted_bps.max(adjusted_profit_bps);
        asset_stats.min_adjusted_bps = asset_stats.min_adjusted_bps.min(adjusted_profit_bps);
        if is_win {
            asset_stats.wins += 1;
        } else {
            asset_stats.losses_or_flat += 1;
        }

        let triangle_stats = by_triangle
            .entry(signal.triangle_pairs.join(" -> "))
            .or_default();
        if triangle_stats.start_asset.is_empty() {
            triangle_stats.start_asset = start_asset;
            triangle_stats.max_adjusted_bps = f64::NEG_INFINITY;
        }
        triangle_stats.trades += 1;
        triangle_stats.sum_pnl_asset += pnl_asset;
        triangle_stats.sum_adjusted_bps += adjusted_profit_bps;
        triangle_stats.sum_notional_deployed += position_size;
        triangle_stats.max_adjusted_bps = triangle_stats.max_adjusted_bps.max(adjusted_profit_bps);
        if is_win {
            triangle_stats.wins += 1;
        }

        last_exec_ts_by_triangle.insert(triangle_key.clone(), signal.timestamp_ms);
        active_state_by_triangle.insert(triangle_key, true);
    }

    print_summary(config, summary, by_asset, by_triangle, balances);
    Ok(())
}

fn parse_balance_arg(value: &str) -> Result<(String, f64), String> {
    let (asset, amount) = value
        .split_once('=')
        .ok_or_else(|| "--balance expects asset=amount".to_string())?;
    let asset = normalize_asset_key(asset);
    if asset.is_empty() {
        return Err("asset in --balance asset=amount cannot be empty".to_string());
    }
    let amount = amount
        .parse::<f64>()
        .map_err(|_| "invalid number in --balance asset=amount".to_string())?;
    if amount < 0.0 {
        return Err("balance amount must be >= 0".to_string());
    }
    Ok((asset, amount))
}

fn normalize_asset_key(asset: &str) -> String {
    asset.trim().to_ascii_lowercase()
}

fn seed_balance_if_needed(
    balances: &mut HashMap<String, f64>,
    asset_key: &str,
    signal: &TriangleOpportunitySignal,
    config: &SimConfig,
) {
    if balances.contains_key(asset_key) || !config.auto_seed_assets {
        return;
    }
    balances.insert(
        asset_key.to_string(),
        signal.assumed_start_amount.to_f64().unwrap_or(0.0) * config.seed_multiplier,
    );
}

fn set_active(state: &mut HashMap<String, bool>, signal: &TriangleOpportunitySignal, active: bool) {
    state.insert(signal.triangle_pairs.join(" -> "), active);
}

fn print_summary(
    config: &SimConfig,
    summary: SummaryStats,
    by_asset: HashMap<String, AssetStats>,
    by_triangle: HashMap<String, TriangleStats>,
    balances: HashMap<String, f64>,
) {
    println!("Paper Trade Simulation");
    println!("file: {}", config.path);
    println!(
        "filters: include_unworthy={} cooldown_ms={} min_adjusted_bps={:.2} pos_pct={:.2} max_vs_signal={:.2} auto_seed={} seed_mult={:.2}",
        config.include_unworthy,
        config.retrigger_cooldown_ms,
        config.min_adjusted_profit_bps,
        config.position_size_pct,
        config.max_position_vs_signal,
        config.auto_seed_assets,
        config.seed_multiplier
    );
    println!();
    println!(
        "lines: {} (parsed {})",
        summary.total_lines, summary.parsed_lines
    );
    println!("executable signals: {}", summary.executable_signals);
    println!("worthy signals: {}", summary.worthy_signals);
    println!("executed trades: {}", summary.executed_trades);
    println!("skipped not executable: {}", summary.skipped_not_executable);
    println!("skipped not worthy: {}", summary.skipped_not_worthy);
    println!(
        "skipped profit threshold: {}",
        summary.skipped_profit_threshold
    );
    println!("skipped cooldown: {}", summary.skipped_cooldown);
    println!("skipped no balance: {}", summary.skipped_no_balance);
    println!(
        "wins / losses-flat: {} / {}",
        summary.wins, summary.losses_or_flat
    );
    if summary.executed_trades > 0 {
        println!(
            "avg adjusted bps per trade: {:.2}",
            summary.sum_adjusted_bps / summary.executed_trades as f64
        );
        println!(
            "avg deployed capital per trade: {:.6}",
            summary.total_notional_deployed / summary.executed_trades as f64
        );
    }
    println!();

    let mut asset_rows = by_asset.into_iter().collect::<Vec<_>>();
    asset_rows.sort_by(|a, b| b.1.sum_pnl_asset.total_cmp(&a.1.sum_pnl_asset));
    println!("By Start Asset");
    println!(
        "{:<8} {:>8} {:>8} {:>12} {:>12} {:>10} {:>10} {:>10}",
        "asset", "trades", "wins", "est_pnl", "deployed", "avg_bps", "max_bps", "min_bps"
    );
    for (asset, stats) in asset_rows {
        let avg_bps = if stats.trades == 0 {
            0.0
        } else {
            stats.sum_adjusted_bps / stats.trades as f64
        };
        let min_bps = if stats.trades == 0 || !stats.min_adjusted_bps.is_finite() {
            0.0
        } else {
            stats.min_adjusted_bps
        };
        println!(
            "{:<8} {:>8} {:>8} {:>12.6} {:>12.6} {:>10.2} {:>10.2} {:>10.2}",
            asset,
            stats.trades,
            stats.wins,
            stats.sum_pnl_asset,
            stats.sum_notional_deployed,
            avg_bps,
            stats.max_adjusted_bps,
            min_bps,
        );
    }
    println!();

    let mut triangle_rows = by_triangle.into_iter().collect::<Vec<_>>();
    triangle_rows.sort_by(|a, b| {
        let a_avg = if a.1.trades == 0 {
            0.0
        } else {
            a.1.sum_adjusted_bps / a.1.trades as f64
        };
        let b_avg = if b.1.trades == 0 {
            0.0
        } else {
            b.1.sum_adjusted_bps / b.1.trades as f64
        };
        b_avg.total_cmp(&a_avg)
    });

    println!("Top Triangles (by avg adjusted bps)");
    println!(
        "{:<46} {:<6} {:>7} {:>10} {:>10} {:>12} {:>12}",
        "triangle", "asset", "trades", "avg_bps", "max_bps", "est_pnl", "deployed"
    );
    for (triangle, stats) in triangle_rows.into_iter().take(DEFAULT_TOP_ROWS) {
        let avg_bps = if stats.trades == 0 {
            0.0
        } else {
            stats.sum_adjusted_bps / stats.trades as f64
        };
        println!(
            "{:<46} {:<6} {:>7} {:>10.2} {:>10.2} {:>12.6} {:>12.6}",
            truncate(&triangle, 46),
            stats.start_asset,
            stats.trades,
            avg_bps,
            stats.max_adjusted_bps,
            stats.sum_pnl_asset,
            stats.sum_notional_deployed,
        );
    }

    println!();
    let mut balance_rows = balances.into_iter().collect::<Vec<_>>();
    balance_rows.sort_by(|a, b| a.0.cmp(&b.0));
    println!("Final Virtual Balances");
    println!("{:<8} {:>14}", "asset", "balance");
    for (asset, balance) in balance_rows {
        println!("{:<8} {:>14.8}", asset, balance);
    }
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
    fn parse_args_defaults() {
        let cfg = parse_args(std::iter::empty::<String>()).expect("parse");
        assert_eq!(cfg.retrigger_cooldown_ms, 1_500);
        assert!(!cfg.include_unworthy);
        assert_eq!(cfg.position_size_pct, 1.0);
        assert!(cfg.auto_seed_assets);
    }

    #[test]
    fn parse_args_custom() {
        let args = vec![
            "signals.jsonl".to_string(),
            "--cooldown-ms".to_string(),
            "5000".to_string(),
            "--min-adjusted-bps".to_string(),
            "12.5".to_string(),
            "--include-unworthy".to_string(),
            "--position-size-pct".to_string(),
            "0.25".to_string(),
            "--max-position-vs-signal".to_string(),
            "2.0".to_string(),
            "--seed-multiplier".to_string(),
            "5".to_string(),
            "--balance".to_string(),
            "usdt=1000".to_string(),
        ];
        let cfg = parse_args(args.into_iter()).expect("parse");
        assert_eq!(cfg.path, "signals.jsonl");
        assert_eq!(cfg.retrigger_cooldown_ms, 5_000);
        assert_eq!(cfg.min_adjusted_profit_bps, 12.5);
        assert!(cfg.include_unworthy);
        assert_eq!(cfg.position_size_pct, 0.25);
        assert_eq!(cfg.max_position_vs_signal, 2.0);
        assert_eq!(cfg.seed_multiplier, 5.0);
        assert_eq!(cfg.initial_balances.get("usdt").copied(), Some(1000.0));
    }
}
