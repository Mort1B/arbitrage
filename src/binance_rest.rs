use reqwest::Client;
use rust_decimal::Decimal;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::fs;

#[derive(Debug, Clone)]
pub struct JsonFileCache {
    pub path: PathBuf,
    pub ttl: Duration,
}

impl JsonFileCache {
    pub fn new(path: impl Into<PathBuf>, ttl: Duration) -> Self {
        Self {
            path: path.into(),
            ttl,
        }
    }
}

pub async fn fetch_json_with_cache<T>(
    client: &Client,
    url: &str,
    cache: Option<&JsonFileCache>,
) -> Result<T, Box<dyn std::error::Error + Send + Sync>>
where
    T: DeserializeOwned,
{
    if let Some(cache) = cache {
        if let Some(bytes) = try_read_fresh_cache(&cache.path, cache.ttl).await? {
            if let Ok(parsed) = serde_json::from_slice::<T>(&bytes) {
                return Ok(parsed);
            }
        }
    }

    let response_bytes = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    if let Some(cache) = cache {
        let _ = write_cache_file(&cache.path, response_bytes.as_ref()).await;
    }

    Ok(serde_json::from_slice::<T>(response_bytes.as_ref())?)
}

async fn try_read_fresh_cache(path: &Path, ttl: Duration) -> io::Result<Option<Vec<u8>>> {
    if ttl.is_zero() {
        return Ok(None);
    }

    let metadata = match fs::metadata(path).await {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };

    let modified = match metadata.modified() {
        Ok(modified) => modified,
        Err(_) => return Ok(None),
    };

    if !is_within_ttl(modified, ttl) {
        return Ok(None);
    }

    match fs::read(path).await {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

async fn write_cache_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).await?;
        }
    }
    fs::write(path, bytes).await
}

fn is_within_ttl(modified: SystemTime, ttl: Duration) -> bool {
    match SystemTime::now().duration_since(modified) {
        Ok(age) => age <= ttl,
        Err(_) => true,
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepthSnapshotLevel {
    pub price: Decimal,
    pub qty: Decimal,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinanceDepthSnapshot {
    pub symbol: String,
    pub last_update_id: u64,
    pub bids: Vec<DepthSnapshotLevel>,
    pub asks: Vec<DepthSnapshotLevel>,
    pub fetched_at_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BinanceDepthSnapshotRaw {
    last_update_id: u64,
    bids: Vec<[String; 2]>,
    asks: Vec<[String; 2]>,
}

#[allow(dead_code)]
pub async fn fetch_depth_snapshot(
    client: &Client,
    symbol: &str,
    limit: u16,
) -> Result<BinanceDepthSnapshot, Box<dyn std::error::Error + Send + Sync>> {
    let url = format!(
        "https://api.binance.com/api/v3/depth?symbol={}&limit={}",
        symbol.to_ascii_uppercase(),
        limit.clamp(1, 5000)
    );

    let raw = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json::<BinanceDepthSnapshotRaw>()
        .await?;

    Ok(BinanceDepthSnapshot {
        symbol: symbol.to_ascii_lowercase(),
        last_update_id: raw.last_update_id,
        bids: parse_depth_levels(raw.bids)?,
        asks: parse_depth_levels(raw.asks)?,
        fetched_at_ms: now_unix_ms(),
    })
}

fn parse_depth_levels(
    raw: Vec<[String; 2]>,
) -> Result<Vec<DepthSnapshotLevel>, Box<dyn std::error::Error + Send + Sync>> {
    raw.into_iter()
        .map(|[price, qty]| {
            Ok(DepthSnapshotLevel {
                price: price.parse::<Decimal>()?,
                qty: qty.parse::<Decimal>()?,
            })
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error + Send + Sync>>>()
}

fn now_unix_ms() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis().min(u128::from(u64::MAX)) as u64,
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{is_within_ttl, parse_depth_levels};
    use rust_decimal::Decimal;
    use std::time::{Duration, SystemTime};

    #[test]
    fn ttl_freshness_check_works() {
        let now = SystemTime::now();
        assert!(is_within_ttl(now, Duration::from_secs(1)));
        let old = now - Duration::from_secs(10);
        assert!(!is_within_ttl(old, Duration::from_secs(1)));
    }

    #[test]
    fn parses_depth_levels_from_string_pairs() {
        let parsed = parse_depth_levels(vec![
            ["100.1".to_string(), "2.5".to_string()],
            ["99.9".to_string(), "0".to_string()],
        ])
        .expect("parse");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].price, "100.1".parse::<Decimal>().unwrap());
        assert_eq!(parsed[0].qty, "2.5".parse::<Decimal>().unwrap());
        assert_eq!(parsed[1].qty, Decimal::ZERO);
    }
}
