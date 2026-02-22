use serde::de;
use serde::{Deserialize, Deserializer, Serialize};

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
<<<<<<< HEAD
    pub last_update_id: u64,
=======
    pub last_update_id: u32,
>>>>>>> 8a83541 (test2)
    pub bids: Vec<OfferData>,
    pub asks: Vec<OfferData>,
}

pub fn de_float_from_str<'a, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'a>,
{
    let str_val = String::deserialize(deserializer)?;
    str_val.parse::<f64>().map_err(de::Error::custom)
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DepthStreamWrapper {
    pub stream: String,
    pub data: DepthStreamData,
}
<<<<<<< HEAD

// Using this struct we can send the triangle representing the names of coins making up the triangle arbitrage, the profit values,
// and we are also sending all the trading data we have collected for each trading coin pair.
#[derive(Debug, Serialize, Clone)]
pub struct TriangleArbitrageData {
    pub triangle: [String; 3],
    pub profits: Vec<f64>,
    pub start_pair_data: DepthStreamWrapper,
    pub mid_pair_data: DepthStreamWrapper,
    pub end_pair_data: DepthStreamWrapper,
}
=======
>>>>>>> 8a83541 (test2)
