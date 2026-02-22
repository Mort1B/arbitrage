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
