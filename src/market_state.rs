use crate::{
    models::{DepthStreamData, DepthStreamWrapper, OfferData},
    orderbook::{
        ApplyOutcome, OrderBook, OrderBookDiff, OrderBookError, OrderBookSnapshot, PriceLevel,
    },
};
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct BookEntry {
    book: OrderBook,
    last_stream: String,
}

#[derive(Debug, Clone)]
pub struct MarketDepthView {
    pub depth: DepthStreamWrapper,
    pub last_receive_time_ms: u64,
}

#[derive(Debug, Default, Clone)]
pub struct MarketState {
    books: HashMap<String, BookEntry>,
}

impl MarketState {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            books: HashMap::with_capacity(capacity),
        }
    }

    #[cfg(test)]
    pub fn apply_depth_snapshot(&mut self, payload: DepthStreamWrapper, receive_time_ms: u64) {
        let pair = pair_key_from_stream(&payload.stream).to_string();
        let last_stream = payload.stream.clone();
        let snapshot = OrderBookSnapshot {
            last_update_id: payload.data.last_update_id,
            bids: payload
                .data
                .bids
                .into_iter()
                .map(offer_to_price_level)
                .collect::<Vec<_>>(),
            asks: payload
                .data
                .asks
                .into_iter()
                .map(offer_to_price_level)
                .collect::<Vec<_>>(),
            snapshot_time_ms: Some(receive_time_ms),
        };

        let entry = self.books.entry(pair.clone()).or_insert_with(|| BookEntry {
            book: OrderBook::new(pair),
            last_stream: last_stream.clone(),
        });
        entry.last_stream = last_stream;
        entry.book.apply_snapshot(snapshot);
    }

    pub fn is_pair_synced(&self, pair: &str) -> bool {
        self.books
            .get(pair)
            .is_some_and(|entry| entry.book.is_synced())
    }

    pub fn mark_pair_unsynced(&mut self, pair: &str) {
        if let Some(entry) = self.books.get_mut(pair) {
            entry.book.clear_and_mark_unsynced();
        }
    }

    pub fn apply_orderbook_snapshot(
        &mut self,
        pair: &str,
        stream_name: String,
        last_update_id: u64,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        snapshot_time_ms: u64,
    ) {
        let entry = self
            .books
            .entry(pair.to_string())
            .or_insert_with(|| BookEntry {
                book: OrderBook::new(pair.to_string()),
                last_stream: stream_name.clone(),
            });
        entry.last_stream = stream_name;
        entry.book.apply_snapshot(OrderBookSnapshot {
            last_update_id,
            bids,
            asks,
            snapshot_time_ms: Some(snapshot_time_ms),
        });
    }

    pub fn apply_orderbook_diff(
        &mut self,
        pair: &str,
        stream_name: &str,
        diff: OrderBookDiff,
    ) -> Result<ApplyOutcome, OrderBookError> {
        let entry = self
            .books
            .entry(pair.to_string())
            .or_insert_with(|| BookEntry {
                book: OrderBook::new(pair.to_string()),
                last_stream: stream_name.to_string(),
            });
        entry.last_stream = stream_name.to_string();
        entry.book.apply_diff(diff)
    }

    #[cfg(test)]
    pub fn export_depth_wrapper(
        &self,
        pair: &str,
        depth_limit: usize,
    ) -> Option<DepthStreamWrapper> {
        self.export_depth_view(pair, depth_limit)
            .map(|view| view.depth)
    }

    pub fn export_depth_view(&self, pair: &str, depth_limit: usize) -> Option<MarketDepthView> {
        let entry = self.books.get(pair)?;
        if !entry.book.is_synced() {
            return None;
        }

        let bids = entry
            .book
            .top_bids(depth_limit)
            .into_iter()
            .map(price_level_to_offer)
            .collect::<Vec<_>>();
        let asks = entry
            .book
            .top_asks(depth_limit)
            .into_iter()
            .map(price_level_to_offer)
            .collect::<Vec<_>>();

        Some(MarketDepthView {
            depth: DepthStreamWrapper {
                stream: entry.last_stream.clone(),
                data: DepthStreamData {
                    last_update_id: entry.book.last_update_id().unwrap_or_default(),
                    bids,
                    asks,
                },
            },
            last_receive_time_ms: entry
                .book
                .last_receive_time_ms()
                .or(entry.book.last_snapshot_time_ms())
                .unwrap_or_default(),
        })
    }
}

#[cfg(test)]
fn pair_key_from_stream(stream: &str) -> &str {
    stream.split_once('@').map_or(stream, |(pair, _)| pair)
}

#[cfg(test)]
fn offer_to_price_level(offer: OfferData) -> PriceLevel {
    PriceLevel {
        price: offer.price,
        qty: offer.size,
    }
}

fn price_level_to_offer(level: PriceLevel) -> OfferData {
    OfferData {
        price: level.price,
        size: level.qty,
    }
}

#[cfg(test)]
mod tests {
    use super::MarketState;
    use crate::models::{DepthStreamData, DepthStreamWrapper, OfferData};
    use rust_decimal::Decimal;

    fn offer(price: &str, size: &str) -> OfferData {
        OfferData {
            price: price.parse::<Decimal>().expect("price"),
            size: size.parse::<Decimal>().expect("size"),
        }
    }

    fn wrapper(
        stream: &str,
        last_update_id: u64,
        bids: Vec<OfferData>,
        asks: Vec<OfferData>,
    ) -> DepthStreamWrapper {
        DepthStreamWrapper {
            stream: stream.to_string(),
            data: DepthStreamData {
                last_update_id,
                bids,
                asks,
            },
        }
    }

    #[test]
    fn stores_snapshot_and_exports_depth_view() {
        let mut state = MarketState::with_capacity(1);
        state.apply_depth_snapshot(
            wrapper(
                "btcusdt@depth20@100ms",
                42,
                vec![offer("100", "2"), offer("99", "3")],
                vec![offer("101", "4"), offer("102", "5")],
            ),
            1_234,
        );

        let view = state
            .export_depth_wrapper("btcusdt", 2)
            .expect("depth view should exist");
        assert_eq!(view.stream, "btcusdt@depth20@100ms");
        assert_eq!(view.data.last_update_id, 42);
        assert_eq!(view.data.bids.len(), 2);
        assert_eq!(view.data.asks.len(), 2);
        assert_eq!(view.data.bids[0].price, Decimal::from(100));
        assert_eq!(view.data.asks[0].price, Decimal::from(101));

        let timed_view = state
            .export_depth_view("btcusdt", 2)
            .expect("depth view should exist");
        assert_eq!(timed_view.last_receive_time_ms, 1_234);
    }

    #[test]
    fn export_respects_depth_limit_and_replaces_previous_snapshot() {
        let mut state = MarketState::with_capacity(2);
        state.apply_depth_snapshot(
            wrapper(
                "ethbtc@depth20@100ms",
                10,
                vec![offer("0.05", "1"), offer("0.049", "2"), offer("0.048", "3")],
                vec![
                    offer("0.051", "1"),
                    offer("0.052", "2"),
                    offer("0.053", "3"),
                ],
            ),
            100,
        );
        state.apply_depth_snapshot(
            wrapper(
                "ethbtc@depth20@100ms",
                11,
                vec![offer("0.0505", "4")],
                vec![offer("0.0515", "6")],
            ),
            200,
        );

        let view = state
            .export_depth_wrapper("ethbtc", 5)
            .expect("depth view should exist");
        assert_eq!(view.data.last_update_id, 11);
        assert_eq!(view.data.bids.len(), 1);
        assert_eq!(view.data.asks.len(), 1);
        assert_eq!(
            view.data.bids[0].price,
            "0.0505".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            view.data.asks[0].price,
            "0.0515".parse::<Decimal>().unwrap()
        );
        let timed_view = state
            .export_depth_view("ethbtc", 5)
            .expect("depth view should exist");
        assert_eq!(timed_view.last_receive_time_ms, 200);
    }
}
