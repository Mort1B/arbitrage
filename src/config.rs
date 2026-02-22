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

const fn default_exchange_rules_enabled() -> bool {
    true
}

const fn default_latency_penalty_bps() -> f64 {
    2.0
}

const fn default_default_fee_bps() -> f64 {
    7.5
}

const fn default_default_assumed_start_amount() -> f64 {
    1.0
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

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct PairRuleConfig {
    pub min_notional: Option<f64>,
    pub min_qty: Option<f64>,
    pub qty_step: Option<f64>,
    pub fee_bps: Option<f64>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct ExchangeRulesConfig {
    pub enabled: bool,
    pub latency_penalty_bps: f64,
    pub default_fee_bps: f64,
    pub default_assumed_start_amount: f64,
    pub assumed_start_amounts: HashMap<String, f64>,
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

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct AutoTriangleGenerationConfig {
    pub enabled: bool,
    pub assets: Vec<String>,
    pub exchange_info_url: String,
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
    #[serde(default)]
    pub exchange_rules: ExchangeRulesConfig,
    #[serde(default)]
    pub auto_triangle_generation: AutoTriangleGenerationConfig,
}
