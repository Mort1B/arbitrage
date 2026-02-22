use rust_decimal::Decimal;
use serde::de;
use serde::{Deserialize, Deserializer, Serialize};
use std::borrow::Cow;

const fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OfferData {
    #[serde(deserialize_with = "de_decimal_from_str")]
    pub price: Decimal,
    #[serde(deserialize_with = "de_decimal_from_str")]
    pub size: Decimal,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DepthStreamData {
    pub last_update_id: u64,
    pub bids: Vec<OfferData>,
    pub asks: Vec<OfferData>,
}

pub fn de_decimal_from_str<'de, D>(deserializer: D) -> Result<Decimal, D::Error>
where
    D: Deserializer<'de>,
{
    let str_val = Cow::<str>::deserialize(deserializer)?;
    str_val.parse::<Decimal>().map_err(de::Error::custom)
}

fn de_offer_levels_from_pairs<'de, D>(deserializer: D) -> Result<Vec<OfferData>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw_levels = Vec::<[Cow<'de, str>; 2]>::deserialize(deserializer)?;
    raw_levels
        .into_iter()
        .map(|[price, size]| {
            Ok(OfferData {
                price: price.parse::<Decimal>().map_err(de::Error::custom)?,
                size: size.parse::<Decimal>().map_err(de::Error::custom)?,
            })
        })
        .collect()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DepthStreamWrapper {
    pub stream: String,
    pub data: DepthStreamData,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DiffDepthStreamWrapper {
    pub stream: String,
    pub data: DiffDepthEventData,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DiffDepthEventData {
    #[serde(rename = "E")]
    pub event_time_ms: u64,
    #[serde(rename = "U")]
    pub first_update_id: u64,
    #[serde(rename = "u")]
    pub final_update_id: u64,
    #[serde(default, rename = "pu")]
    pub prev_final_update_id: Option<u64>,
    #[serde(rename = "b", deserialize_with = "de_offer_levels_from_pairs")]
    pub bids: Vec<OfferData>,
    #[serde(rename = "a", deserialize_with = "de_offer_levels_from_pairs")]
    pub asks: Vec<OfferData>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TriangleLegQuote {
    pub pair: String,
    pub ask_price: Decimal,
    pub ask_size: Decimal,
    pub bid_price: Decimal,
    pub bid_size: Decimal,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TriangleOpportunitySignal {
    pub timestamp_ms: u64,
    pub exchange: String,
    pub triangle_parts: [String; 3],
    pub triangle_pairs: [String; 3],
    pub depth_levels_considered: usize,
    pub profitable_levels: usize,
    pub hit_rate: Decimal,
    pub top_profit_bps: Decimal,
    pub best_profit_bps: Decimal,
    pub avg_profit_bps: Decimal,
    pub best_level_index: usize,
    pub assumed_start_amount: Decimal,
    pub executable_profit_bps: Decimal,
    pub adjusted_profit_bps: Decimal,
    pub latency_penalty_bps: Decimal,
    #[serde(default)]
    pub book_receive_timestamp_ms_by_leg: [u64; 3],
    #[serde(default)]
    pub book_age_ms_by_leg: [u64; 3],
    #[serde(default)]
    pub min_book_age_ms: u64,
    #[serde(default)]
    pub max_book_age_ms: u64,
    #[serde(default = "default_true")]
    pub book_freshness_passed: bool,
    #[serde(default)]
    pub pair_synced_by_leg: [bool; 3],
    #[serde(default)]
    pub pair_resync_attempt_count_by_leg: [u64; 3],
    #[serde(default)]
    pub pair_resync_count_by_leg: [u64; 3],
    #[serde(default)]
    pub pair_resync_failure_count_by_leg: [u64; 3],
    #[serde(default)]
    pub pair_gap_count_by_leg: [u64; 3],
    #[serde(default)]
    pub pair_last_resync_timestamp_ms_by_leg: [u64; 3],
    #[serde(default)]
    pub pair_last_resync_failure_timestamp_ms_by_leg: [u64; 3],
    pub execution_filter_passed: bool,
    pub worthy: bool,
    pub min_profit_bps_threshold: Decimal,
    pub min_hit_rate_threshold: Decimal,
    pub rejection_reasons: Vec<String>,
    pub fee_bps_by_leg: [Decimal; 3],
    pub best_level_quotes: [TriangleLegQuote; 3],
}

#[cfg(test)]
mod tests {
    use super::{TriangleLegQuote, TriangleOpportunitySignal};
    use rust_decimal::Decimal;

    #[test]
    fn signal_serializes_decimal_fields_as_json_strings() {
        let signal = TriangleOpportunitySignal {
            timestamp_ms: 1,
            exchange: "binance".to_string(),
            triangle_parts: ["btc".to_string(), "eth".to_string(), "usdt".to_string()],
            triangle_pairs: [
                "ethbtc".to_string(),
                "ethusdt".to_string(),
                "btcusdt".to_string(),
            ],
            depth_levels_considered: 1,
            profitable_levels: 1,
            hit_rate: Decimal::new(5, 1),
            top_profit_bps: Decimal::new(123, 2),
            best_profit_bps: Decimal::new(123, 2),
            avg_profit_bps: Decimal::new(123, 2),
            best_level_index: 0,
            assumed_start_amount: Decimal::new(1000, 1),
            executable_profit_bps: Decimal::new(50, 1),
            adjusted_profit_bps: Decimal::new(30, 1),
            latency_penalty_bps: Decimal::new(20, 1),
            book_receive_timestamp_ms_by_leg: [1, 2, 3],
            book_age_ms_by_leg: [10, 20, 30],
            min_book_age_ms: 10,
            max_book_age_ms: 30,
            book_freshness_passed: true,
            pair_synced_by_leg: [true, true, true],
            pair_resync_attempt_count_by_leg: [1, 2, 3],
            pair_resync_count_by_leg: [1, 2, 3],
            pair_resync_failure_count_by_leg: [0, 1, 0],
            pair_gap_count_by_leg: [0, 4, 1],
            pair_last_resync_timestamp_ms_by_leg: [100, 200, 300],
            pair_last_resync_failure_timestamp_ms_by_leg: [0, 250, 0],
            execution_filter_passed: true,
            worthy: true,
            min_profit_bps_threshold: Decimal::new(80, 1),
            min_hit_rate_threshold: Decimal::new(2, 1),
            rejection_reasons: Vec::new(),
            fee_bps_by_leg: [
                Decimal::new(75, 1),
                Decimal::new(75, 1),
                Decimal::new(75, 1),
            ],
            best_level_quotes: [
                TriangleLegQuote {
                    pair: "ethbtc".to_string(),
                    ask_price: Decimal::new(10, 4),
                    ask_size: Decimal::new(1, 2),
                    bid_price: Decimal::new(9, 4),
                    bid_size: Decimal::new(2, 2),
                },
                TriangleLegQuote {
                    pair: "ethusdt".to_string(),
                    ask_price: Decimal::new(2000, 0),
                    ask_size: Decimal::new(1, 2),
                    bid_price: Decimal::new(1999, 0),
                    bid_size: Decimal::new(2, 2),
                },
                TriangleLegQuote {
                    pair: "btcusdt".to_string(),
                    ask_price: Decimal::new(40000, 0),
                    ask_size: Decimal::new(1, 3),
                    bid_price: Decimal::new(39990, 0),
                    bid_size: Decimal::new(2, 3),
                },
            ],
        };

        let json = serde_json::to_string(&signal).expect("serialize");
        assert!(json.contains("\"best_profit_bps\":\"1.23\""));
        assert!(json.contains("\"adjusted_profit_bps\":\"3.0\""));
        assert!(json.contains("\"ask_price\":\"0.0010\""));
    }

    #[test]
    fn parses_diff_depth_stream_wrapper() {
        let raw = r#"{
          "stream":"btcusdt@depth@100ms",
          "data":{
            "e":"depthUpdate",
            "E":123456789,
            "s":"BTCUSDT",
            "U":100,
            "u":101,
            "b":[["100.1","2.5"]],
            "a":[["100.2","1.0"]]
          }
        }"#;

        let parsed: super::DiffDepthStreamWrapper = serde_json::from_str(raw).expect("parse");
        assert_eq!(parsed.stream, "btcusdt@depth@100ms");
        assert_eq!(parsed.data.first_update_id, 100);
        assert_eq!(parsed.data.final_update_id, 101);
        assert_eq!(
            parsed.data.bids[0].price,
            "100.1".parse::<Decimal>().unwrap()
        );
        assert_eq!(parsed.data.asks[0].size, Decimal::ONE);
    }
}
