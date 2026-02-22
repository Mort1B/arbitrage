use serde::Deserialize;

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

#[derive(Debug, Deserialize, Clone)]
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
}
