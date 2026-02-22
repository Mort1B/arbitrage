use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn default_bind_addr() -> String {
    "127.0.0.1".to_string()
}

const fn default_bind_port() -> u16 {
    8000
}

const fn default_reconnect_delay_ms() -> u64 {
    1_500
}

const fn default_signal_log_enabled() -> bool {
    true
}

fn default_signal_log_path() -> String {
    "data/triangle_signals.jsonl".to_string()
}

const fn default_signal_log_channel_capacity() -> usize {
    2048
}

const fn default_signal_min_profit_bps() -> f64 {
    8.0
}

const fn default_signal_min_hit_rate() -> f64 {
    0.2
}

const fn default_max_book_age_ms() -> u64 {
    1_500
}

const fn default_exchange_rules_enabled() -> bool {
    true
}

fn default_latency_penalty_bps() -> Decimal {
    Decimal::new(20, 1)
}

fn default_default_fee_bps() -> Decimal {
    Decimal::new(75, 1)
}

fn default_default_assumed_start_amount() -> Decimal {
    Decimal::ONE
}

const fn default_auto_triangle_generation_enabled() -> bool {
    false
}

fn default_auto_triangle_generation_exchange_info_url() -> String {
    "https://api.binance.com/api/v3/exchangeInfo".to_string()
}

const fn default_auto_triangle_generation_include_reverse_cycles() -> bool {
    true
}

const fn default_auto_triangle_generation_include_all_starts() -> bool {
    false
}

const fn default_auto_triangle_generation_max_triangles() -> usize {
    0
}

const fn default_auto_triangle_generation_merge_pair_rules() -> bool {
    true
}

const fn default_auto_triangle_generation_exchange_info_cache_enabled() -> bool {
    true
}

fn default_auto_triangle_generation_exchange_info_cache_path() -> String {
    "data/cache/binance_exchange_info.json".to_string()
}

const fn default_auto_triangle_generation_exchange_info_cache_ttl_secs() -> u64 {
    300
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct PairRuleConfig {
    pub min_notional: Option<Decimal>,
    pub min_qty: Option<Decimal>,
    pub qty_step: Option<Decimal>,
    pub fee_bps: Option<Decimal>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct ExchangeRulesConfig {
    pub enabled: bool,
    pub latency_penalty_bps: Decimal,
    pub default_fee_bps: Decimal,
    pub default_assumed_start_amount: Decimal,
    pub assumed_start_amounts: HashMap<String, Decimal>,
    pub pair_rules: HashMap<String, PairRuleConfig>,
}

impl Default for ExchangeRulesConfig {
    fn default() -> Self {
        Self {
            enabled: default_exchange_rules_enabled(),
            latency_penalty_bps: default_latency_penalty_bps(),
            default_fee_bps: default_default_fee_bps(),
            default_assumed_start_amount: default_default_assumed_start_amount(),
            assumed_start_amounts: HashMap::new(),
            pair_rules: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, Decimal};

    #[test]
    fn parses_decimal_exchange_rules_from_yaml_numbers() {
        let yaml = r#"
update_interval: 100
results_limit: 10
depth_streams: [ethbtc]
triangles:
  - parts: [btc, eth, usdt]
    pairs: [ethbtc, ethusdt, btcusdt]
exchange_rules:
  enabled: true
  latency_penalty_bps: 2.0
  default_fee_bps: 7.5
  default_assumed_start_amount: 1.0
  assumed_start_amounts:
    usdt: 100.0
  pair_rules:
    ethbtc:
      min_notional: 0.0001
      min_qty: 0.001
      qty_step: 0.001
      fee_bps: 7.5
"#;

        let cfg: AppConfig = serde_yaml::from_str(yaml).expect("config should parse");
        assert_eq!(cfg.exchange_rules.latency_penalty_bps, Decimal::new(20, 1));
        assert_eq!(cfg.exchange_rules.default_fee_bps, Decimal::new(75, 1));
        assert_eq!(
            cfg.exchange_rules.default_assumed_start_amount,
            Decimal::ONE
        );
        assert_eq!(
            cfg.exchange_rules.assumed_start_amounts.get("usdt"),
            Some(&Decimal::new(1000, 1))
        );
        let rule = cfg
            .exchange_rules
            .pair_rules
            .get("ethbtc")
            .expect("pair rule exists");
        assert_eq!(rule.min_notional, Some(Decimal::new(1, 4)));
        assert_eq!(rule.min_qty, Some(Decimal::new(1, 3)));
        assert_eq!(rule.qty_step, Some(Decimal::new(1, 3)));
        assert_eq!(rule.fee_bps, Some(Decimal::new(75, 1)));
        assert_eq!(cfg.max_book_age_ms, 1_500);
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct AutoTriangleGenerationConfig {
    pub enabled: bool,
    pub assets: Vec<String>,
    pub exchange_info_url: String,
    pub exchange_info_cache_enabled: bool,
    pub exchange_info_cache_path: String,
    pub exchange_info_cache_ttl_secs: u64,
    pub include_reverse_cycles: bool,
    pub include_all_starts: bool,
    pub max_triangles: usize,
    pub merge_pair_rules_from_exchange_info: bool,
}

impl Default for AutoTriangleGenerationConfig {
    fn default() -> Self {
        Self {
            enabled: default_auto_triangle_generation_enabled(),
            assets: Vec::new(),
            exchange_info_url: default_auto_triangle_generation_exchange_info_url(),
            exchange_info_cache_enabled:
                default_auto_triangle_generation_exchange_info_cache_enabled(),
            exchange_info_cache_path: default_auto_triangle_generation_exchange_info_cache_path(),
            exchange_info_cache_ttl_secs:
                default_auto_triangle_generation_exchange_info_cache_ttl_secs(),
            include_reverse_cycles: default_auto_triangle_generation_include_reverse_cycles(),
            include_all_starts: default_auto_triangle_generation_include_all_starts(),
            max_triangles: default_auto_triangle_generation_max_triangles(),
            merge_pair_rules_from_exchange_info: default_auto_triangle_generation_merge_pair_rules(
            ),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TriangleConfig {
    pub parts: [String; 3],
    pub pairs: [String; 3],
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub update_interval: u32,
    pub results_limit: u32,
    pub depth_streams: Vec<String>,
    pub triangles: Vec<TriangleConfig>,
    #[serde(default = "default_bind_addr")]
    pub bind_addr: String,
    #[serde(default = "default_bind_port")]
    pub bind_port: u16,
    #[serde(default = "default_reconnect_delay_ms")]
    pub reconnect_delay_ms: u64,
    #[serde(default = "default_signal_log_enabled")]
    pub signal_log_enabled: bool,
    #[serde(default = "default_signal_log_path")]
    pub signal_log_path: String,
    #[serde(default = "default_signal_log_channel_capacity")]
    pub signal_log_channel_capacity: usize,
    #[serde(default = "default_signal_min_profit_bps")]
    pub signal_min_profit_bps: f64,
    #[serde(default = "default_signal_min_hit_rate")]
    pub signal_min_hit_rate: f64,
    #[serde(default = "default_max_book_age_ms")]
    pub max_book_age_ms: u64,
    #[serde(default)]
    pub exchange_rules: ExchangeRulesConfig,
    #[serde(default)]
    pub auto_triangle_generation: AutoTriangleGenerationConfig,
}
