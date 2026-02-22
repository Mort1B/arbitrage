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
}
