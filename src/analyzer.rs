use crate::models::TriangleOpportunitySignal;
use rust_decimal::prelude::ToPrimitive;
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader},
};

const DEFAULT_SIGNAL_LOG_PATH: &str = "data/triangle_signals.jsonl";

#[derive(Default)]
struct Stats {
    samples: usize,
    worthy_samples: usize,
    sum_best_profit_bps: f64,
    max_best_profit_bps: f64,
    sum_hit_rate: f64,
}

pub fn run_from_cli_args<I>(mut args: I) -> bool
where
    I: Iterator<Item = String>,
{
    let Some(command) = args.next() else {
        return false;
    };

    if command != "analyze" {
        return false;
    }

    let path = args
        .next()
        .unwrap_or_else(|| DEFAULT_SIGNAL_LOG_PATH.to_string());
    if let Err(e) = analyze_file(&path) {
        eprintln!("analyze failed for {}: {}", path, e);
    }

    true
}

fn analyze_file(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    let mut by_triangle: HashMap<String, Stats> = HashMap::new();
    let mut total_lines = 0usize;
    let mut parsed_lines = 0usize;

    for line in reader.lines() {
        total_lines += 1;
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let signal: TriangleOpportunitySignal = match serde_json::from_str(&line) {
            Ok(signal) => signal,
            Err(_) => continue,
        };
        parsed_lines += 1;

        let key = signal.triangle_pairs.join(" -> ");
        let stats = by_triangle.entry(key).or_default();
        stats.samples += 1;
        stats.worthy_samples += usize::from(signal.worthy);
        let best_profit_bps = signal.best_profit_bps.to_f64().unwrap_or(0.0);
        let hit_rate = signal.hit_rate.to_f64().unwrap_or(0.0);
        stats.sum_best_profit_bps += best_profit_bps;
        stats.max_best_profit_bps = stats.max_best_profit_bps.max(best_profit_bps);
        stats.sum_hit_rate += hit_rate;
    }

    let mut rows = by_triangle.into_iter().collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        let a_avg = if a.1.samples == 0 {
            0.0
        } else {
            a.1.sum_best_profit_bps / a.1.samples as f64
        };
        let b_avg = if b.1.samples == 0 {
            0.0
        } else {
            b.1.sum_best_profit_bps / b.1.samples as f64
        };
        b_avg.total_cmp(&a_avg)
    });

    println!("Triangle Opportunity Analysis");
    println!("file: {}", path);
    println!("lines: {} (parsed {})", total_lines, parsed_lines);
    println!();
    println!(
        "{:<42} {:>8} {:>8} {:>10} {:>10} {:>10}",
        "triangle", "samples", "worthy", "avg_bps", "max_bps", "avg_hit"
    );

    for (triangle, stats) in rows {
        let avg_bps = if stats.samples == 0 {
            0.0
        } else {
            stats.sum_best_profit_bps / stats.samples as f64
        };
        let avg_hit = if stats.samples == 0 {
            0.0
        } else {
            stats.sum_hit_rate / stats.samples as f64
        };

        println!(
            "{:<42} {:>8} {:>8} {:>10.2} {:>10.2} {:>10.3}",
            truncate(&triangle, 42),
            stats.samples,
            stats.worthy_samples,
            avg_bps,
            stats.max_best_profit_bps,
            avg_hit,
        );
    }

    Ok(())
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
