use serde::de;
use serde::{Deserialize, Deserializer, Serialize};
use std::borrow::Cow;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OfferData {
    #[serde(deserialize_with = "de_float_from_str")]
    pub price: f64,
    #[serde(deserialize_with = "de_float_from_str")]
    pub size: f64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DepthStreamData {
    pub last_update_id: u64,
    pub bids: Vec<OfferData>,
    pub asks: Vec<OfferData>,
}

pub fn de_float_from_str<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    let str_val = Cow::<str>::deserialize(deserializer)?;
    str_val.parse::<f64>().map_err(de::Error::custom)
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DepthStreamWrapper {
    pub stream: String,
    pub data: DepthStreamData,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TriangleLegQuote {
    pub pair: String,
    pub ask_price: f64,
    pub ask_size: f64,
    pub bid_price: f64,
    pub bid_size: f64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TriangleOpportunitySignal {
    pub timestamp_ms: u64,
    pub exchange: String,
    pub triangle_parts: [String; 3],
    pub triangle_pairs: [String; 3],
    pub depth_levels_considered: usize,
    pub profitable_levels: usize,
    pub hit_rate: f64,
    pub top_profit_bps: f64,
    pub best_profit_bps: f64,
    pub avg_profit_bps: f64,
    pub best_level_index: usize,
    pub worthy: bool,
    pub min_profit_bps_threshold: f64,
    pub min_hit_rate_threshold: f64,
    pub best_level_quotes: [TriangleLegQuote; 3],
}
