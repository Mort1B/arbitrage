use crate::{auto_triangles, config};
use serde::Serialize;
use std::{
    collections::HashMap,
    fs::File,
    io::BufWriter,
    time::{SystemTime, UNIX_EPOCH},
};

const DEFAULT_OUTPUT_PATH: &str = "generated_triangles.yaml";

#[derive(Debug, Clone)]
struct GenerateConfigArgs {
    output_path: String,
    include_pair_rules: bool,
}

impl Default for GenerateConfigArgs {
    fn default() -> Self {
        Self {
            output_path: DEFAULT_OUTPUT_PATH.to_string(),
            include_pair_rules: true,
        }
    }
}

#[derive(Serialize)]
struct GeneratedConfigFile {
    generated_at_ms: u64,
    exchange_info_url: String,
    asset_count_requested: usize,
    used_asset_count: usize,
    triangle_count: usize,
    depth_stream_count: usize,
    depth_streams: Vec<String>,
    triangles: Vec<config::TriangleConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pair_rules: Option<HashMap<String, config::PairRuleConfig>>,
}

pub async fn run_from_cli_args<I>(mut args: I) -> bool
where
    I: Iterator<Item = String>,
{
    let Some(command) = args.next() else {
        return false;
    };

    if command != "generate-config" {
        return false;
    }

    let cli_args = match parse_args(args) {
        Ok(cfg) => cfg,
        Err(msg) => {
            eprintln!("generate-config: {}", msg);
            eprintln!("usage: cargo run -- generate-config [output.yaml] [--no-pair-rules]");
            return true;
        }
    };

    if let Err(e) = run(cli_args).await {
        eprintln!("generate-config failed: {}", e);
    }

    true
}

fn parse_args<I>(args: I) -> Result<GenerateConfigArgs, String>
where
    I: Iterator<Item = String>,
{
    let mut cfg = GenerateConfigArgs::default();
    let mut path_set = false;

    for arg in args {
        match arg.as_str() {
            "--no-pair-rules" => cfg.include_pair_rules = false,
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

async fn run(cli_args: GenerateConfigArgs) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config_file = File::open("config.yaml")?;
    let app_config: config::AppConfig = serde_yaml::from_reader(config_file)?;

    let auto_cfg = &app_config.auto_triangle_generation;
    if auto_cfg.assets.len() < 3 {
        return Err(
            "config.yaml auto_triangle_generation.assets must contain at least 3 assets".into(),
        );
    }

    let generated = auto_triangles::generate_from_binance(auto_cfg).await?;

    let output = GeneratedConfigFile {
        generated_at_ms: now_unix_ms(),
        exchange_info_url: auto_cfg.exchange_info_url.clone(),
        asset_count_requested: auto_cfg.assets.len(),
        used_asset_count: generated.used_assets.len(),
        triangle_count: generated.triangles.len(),
        depth_stream_count: generated.depth_streams.len(),
        depth_streams: generated.depth_streams,
        triangles: generated.triangles,
        pair_rules: cli_args.include_pair_rules.then_some(generated.pair_rules),
    };

    let file = File::create(&cli_args.output_path)?;
    let writer = BufWriter::new(file);
    serde_yaml::to_writer(writer, &output)?;

    println!("Generated config written to {}", cli_args.output_path);
    println!("triangles: {}", output.triangle_count);
    println!("depth_streams: {}", output.depth_stream_count);
    println!(
        "used_assets: {} / {}",
        output.used_asset_count, output.asset_count_requested
    );

    Ok(())
}

fn now_unix_ms() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis().min(u128::from(u64::MAX)) as u64,
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_args;

    #[test]
    fn parse_defaults() {
        let args = parse_args(std::iter::empty::<String>()).expect("parse");
        assert_eq!(args.output_path, "generated_triangles.yaml");
        assert!(args.include_pair_rules);
    }

    #[test]
    fn parse_custom() {
        let args =
            parse_args(vec!["out.yaml".to_string(), "--no-pair-rules".to_string()].into_iter())
                .expect("parse");
        assert_eq!(args.output_path, "out.yaml");
        assert!(!args.include_pair_rules);
    }
}
