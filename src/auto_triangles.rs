use crate::config::{AutoTriangleGenerationConfig, PairRuleConfig, TriangleConfig};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct GeneratedTriangleUniverse {
    pub depth_streams: Vec<String>,
    pub triangles: Vec<TriangleConfig>,
    pub pair_rules: HashMap<String, PairRuleConfig>,
    pub used_assets: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct BinanceExchangeInfo {
    symbols: Vec<BinanceSymbol>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BinanceSymbol {
    symbol: String,
    status: String,
    base_asset: String,
    quote_asset: String,
    #[serde(default)]
    is_spot_trading_allowed: bool,
    #[serde(default)]
    filters: Vec<Value>,
}

#[derive(Debug, Clone)]
struct PairEdge {
    symbol: String,
    pair_rule: PairRuleConfig,
}

type UnorderedKey = (String, String);

pub async fn generate_from_binance(
    auto_cfg: &AutoTriangleGenerationConfig,
) -> Result<GeneratedTriangleUniverse, Box<dyn std::error::Error + Send + Sync>> {
    let assets = normalize_and_dedupe_assets(&auto_cfg.assets);
    if assets.len() < 3 {
        return Err("auto_triangle_generation.assets must contain at least 3 unique assets".into());
    }

    let exchange_info = reqwest::Client::new()
        .get(&auto_cfg.exchange_info_url)
        .send()
        .await?
        .error_for_status()?
        .json::<BinanceExchangeInfo>()
        .await?;

    let universe = generate_from_exchange_info(&assets, auto_cfg, exchange_info.symbols);
    if universe.triangles.is_empty() {
        return Err("no valid triangles generated from asset list and exchange symbols".into());
    }

    Ok(universe)
}

fn generate_from_exchange_info(
    assets: &[String],
    auto_cfg: &AutoTriangleGenerationConfig,
    symbols: Vec<BinanceSymbol>,
) -> GeneratedTriangleUniverse {
    let asset_set = assets.iter().cloned().collect::<HashSet<_>>();
    let mut pair_edges: HashMap<UnorderedKey, PairEdge> = HashMap::new();

    for symbol in symbols {
        if symbol.status != "TRADING" || !symbol.is_spot_trading_allowed {
            continue;
        }

        let base = symbol.base_asset.to_ascii_lowercase();
        let quote = symbol.quote_asset.to_ascii_lowercase();
        if !asset_set.contains(&base) || !asset_set.contains(&quote) || base == quote {
            continue;
        }

        let key = unordered_key(&base, &quote);
        pair_edges.entry(key).or_insert_with(|| PairEdge {
            symbol: symbol.symbol.to_ascii_lowercase(),
            pair_rule: pair_rule_from_filters(&symbol.filters),
        });
    }

    let mut triangles = enumerate_triangles(assets, auto_cfg, &pair_edges);
    if auto_cfg.max_triangles > 0 && triangles.len() > auto_cfg.max_triangles {
        triangles.truncate(auto_cfg.max_triangles);
    }

    let mut depth_streams = HashSet::new();
    let mut pair_rules = HashMap::new();
    let mut used_assets = HashSet::new();

    for triangle in &triangles {
        for asset in &triangle.parts {
            used_assets.insert(asset.clone());
        }
        for pair in &triangle.pairs {
            depth_streams.insert(pair.clone());
            if let Some(edge) = find_edge_by_symbol(&pair_edges, pair) {
                pair_rules.insert(pair.clone(), edge.pair_rule.clone());
            }
        }
    }

    let mut depth_streams = depth_streams.into_iter().collect::<Vec<_>>();
    depth_streams.sort();
    let mut used_assets = used_assets.into_iter().collect::<Vec<_>>();
    used_assets.sort();

    GeneratedTriangleUniverse {
        depth_streams,
        triangles,
        pair_rules,
        used_assets,
    }
}

fn enumerate_triangles(
    assets: &[String],
    auto_cfg: &AutoTriangleGenerationConfig,
    pair_edges: &HashMap<UnorderedKey, PairEdge>,
) -> Vec<TriangleConfig> {
    let mut assets_sorted = assets.to_vec();
    assets_sorted.sort();
    assets_sorted.dedup();

    let mut triangles = Vec::new();
    let mut seen = HashSet::new();

    for i in 0..assets_sorted.len() {
        for j in (i + 1)..assets_sorted.len() {
            for k in (j + 1)..assets_sorted.len() {
                let a = &assets_sorted[i];
                let b = &assets_sorted[j];
                let c = &assets_sorted[k];

                if !has_pair(pair_edges, a, b)
                    || !has_pair(pair_edges, b, c)
                    || !has_pair(pair_edges, c, a)
                {
                    continue;
                }

                let mut sequences = Vec::new();
                sequences.push([a.clone(), b.clone(), c.clone()]);
                if auto_cfg.include_reverse_cycles {
                    sequences.push([a.clone(), c.clone(), b.clone()]);
                }

                for seq in sequences {
                    if auto_cfg.include_all_starts {
                        for shift in 0..3 {
                            let order = rotate3(&seq, shift);
                            if let Some(triangle) = triangle_config_from_order(&order, pair_edges) {
                                let key = triangle_key(&triangle);
                                if seen.insert(key) {
                                    triangles.push(triangle);
                                }
                            }
                        }
                    } else if let Some(triangle) = triangle_config_from_order(&seq, pair_edges) {
                        let key = triangle_key(&triangle);
                        if seen.insert(key) {
                            triangles.push(triangle);
                        }
                    }
                }
            }
        }
    }

    triangles.sort_by(|lhs, rhs| lhs.parts.cmp(&rhs.parts).then(lhs.pairs.cmp(&rhs.pairs)));
    triangles
}

fn triangle_config_from_order(
    order: &[String; 3],
    pair_edges: &HashMap<UnorderedKey, PairEdge>,
) -> Option<TriangleConfig> {
    let p1 = pair_for_assets(pair_edges, &order[0], &order[1])?;
    let p2 = pair_for_assets(pair_edges, &order[1], &order[2])?;
    let p3 = pair_for_assets(pair_edges, &order[2], &order[0])?;

    Some(TriangleConfig {
        parts: [order[0].clone(), order[1].clone(), order[2].clone()],
        pairs: [p1, p2, p3],
    })
}

fn pair_for_assets(
    pair_edges: &HashMap<UnorderedKey, PairEdge>,
    a: &str,
    b: &str,
) -> Option<String> {
    pair_edges
        .get(&unordered_key(a, b))
        .map(|edge| edge.symbol.clone())
}

fn has_pair(pair_edges: &HashMap<UnorderedKey, PairEdge>, a: &str, b: &str) -> bool {
    pair_edges.contains_key(&unordered_key(a, b))
}

fn unordered_key(a: &str, b: &str) -> UnorderedKey {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

fn rotate3(order: &[String; 3], shift: usize) -> [String; 3] {
    [
        order[shift % 3].clone(),
        order[(shift + 1) % 3].clone(),
        order[(shift + 2) % 3].clone(),
    ]
}

fn triangle_key(triangle: &TriangleConfig) -> String {
    format!(
        "{}|{}|{}::{}|{}|{}",
        triangle.parts[0],
        triangle.parts[1],
        triangle.parts[2],
        triangle.pairs[0],
        triangle.pairs[1],
        triangle.pairs[2]
    )
}

fn normalize_and_dedupe_assets(assets: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for asset in assets {
        let asset = asset.trim().to_ascii_lowercase();
        if asset.is_empty() {
            continue;
        }
        if seen.insert(asset.clone()) {
            out.push(asset);
        }
    }
    out
}

fn pair_rule_from_filters(filters: &[Value]) -> PairRuleConfig {
    let mut pair_rule = PairRuleConfig::default();

    for filter in filters {
        let Some(filter_type) = filter.get("filterType").and_then(Value::as_str) else {
            continue;
        };
        match filter_type {
            "LOT_SIZE" => {
                pair_rule.min_qty = parse_filter_num(filter, "minQty");
                pair_rule.qty_step = parse_filter_num(filter, "stepSize");
            }
            "MIN_NOTIONAL" => {
                pair_rule.min_notional = parse_filter_num(filter, "minNotional");
            }
            "NOTIONAL" => {
                if pair_rule.min_notional.is_none() {
                    pair_rule.min_notional = parse_filter_num(filter, "minNotional");
                }
            }
            _ => {}
        }
    }

    pair_rule
}

fn parse_filter_num(filter: &Value, key: &str) -> Option<Decimal> {
    filter
        .get(key)
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<Decimal>().ok())
}

fn find_edge_by_symbol<'a>(
    pair_edges: &'a HashMap<UnorderedKey, PairEdge>,
    symbol: &str,
) -> Option<&'a PairEdge> {
    pair_edges.values().find(|edge| edge.symbol == symbol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    fn sym(symbol: &str, base: &str, quote: &str) -> BinanceSymbol {
        BinanceSymbol {
            symbol: symbol.to_string(),
            status: "TRADING".to_string(),
            base_asset: base.to_string(),
            quote_asset: quote.to_string(),
            is_spot_trading_allowed: true,
            filters: Vec::new(),
        }
    }

    #[test]
    fn generates_triangle_from_assets() {
        let assets = vec!["btc".to_string(), "eth".to_string(), "bnb".to_string()];
        let cfg = AutoTriangleGenerationConfig {
            enabled: true,
            assets: assets.clone(),
            include_reverse_cycles: true,
            include_all_starts: false,
            ..AutoTriangleGenerationConfig::default()
        };
        let out = generate_from_exchange_info(
            &assets,
            &cfg,
            vec![
                sym("ETHBTC", "ETH", "BTC"),
                sym("BNBETH", "BNB", "ETH"),
                sym("BNBBTC", "BNB", "BTC"),
            ],
        );

        assert_eq!(out.depth_streams.len(), 3);
        assert!(out
            .triangles
            .iter()
            .any(|t| t.parts == ["bnb".to_string(), "btc".to_string(), "eth".to_string()]));
    }

    #[test]
    fn extracts_pair_rule_filters() {
        let filters = vec![
            serde_json::json!({"filterType":"LOT_SIZE","minQty":"0.001","stepSize":"0.001"}),
            serde_json::json!({"filterType":"MIN_NOTIONAL","minNotional":"10.0"}),
        ];
        let rule = pair_rule_from_filters(&filters);
        assert_eq!(rule.min_qty, Some(Decimal::new(1, 3)));
        assert_eq!(rule.qty_step, Some(Decimal::new(1, 3)));
        assert_eq!(rule.min_notional, Some(Decimal::new(100, 1)));
    }
}
