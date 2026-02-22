#![allow(dead_code)]

use rust_decimal::Decimal;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceLevel {
    pub price: Decimal,
    pub qty: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderBookSnapshot {
    pub last_update_id: u64,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
    pub snapshot_time_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderBookDiff {
    pub first_update_id: u64,
    pub final_update_id: u64,
    pub prev_final_update_id: Option<u64>,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
    pub event_time_ms: Option<u64>,
    pub receive_time_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied,
    IgnoredStale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderBookError {
    NotSynced,
    InvalidUpdateRange {
        first_update_id: u64,
        final_update_id: u64,
    },
    SequenceGap {
        expected_next: u64,
        got_first: u64,
        got_final: u64,
        got_prev_final: Option<u64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopOfBook {
    pub bid: Option<PriceLevel>,
    pub ask: Option<PriceLevel>,
}

#[derive(Debug, Clone)]
pub struct OrderBook {
    symbol: String,
    bids: BTreeMap<Decimal, Decimal>,
    asks: BTreeMap<Decimal, Decimal>,
    synced: bool,
    last_update_id: Option<u64>,
    last_snapshot_time_ms: Option<u64>,
    last_event_time_ms: Option<u64>,
    last_receive_time_ms: Option<u64>,
}

impl OrderBook {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            synced: false,
            last_update_id: None,
            last_snapshot_time_ms: None,
            last_event_time_ms: None,
            last_receive_time_ms: None,
        }
    }

    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    pub fn is_synced(&self) -> bool {
        self.synced
    }

    pub fn last_update_id(&self) -> Option<u64> {
        self.last_update_id
    }

    pub fn last_snapshot_time_ms(&self) -> Option<u64> {
        self.last_snapshot_time_ms
    }

    pub fn last_event_time_ms(&self) -> Option<u64> {
        self.last_event_time_ms
    }

    pub fn last_receive_time_ms(&self) -> Option<u64> {
        self.last_receive_time_ms
    }

    pub fn clear_and_mark_unsynced(&mut self) {
        self.bids.clear();
        self.asks.clear();
        self.synced = false;
        self.last_update_id = None;
    }

    pub fn apply_snapshot(&mut self, snapshot: OrderBookSnapshot) {
        let snapshot_time_ms = snapshot.snapshot_time_ms;
        self.bids.clear();
        self.asks.clear();
        apply_side_updates(&mut self.bids, snapshot.bids);
        apply_side_updates(&mut self.asks, snapshot.asks);
        self.synced = true;
        self.last_update_id = Some(snapshot.last_update_id);
        self.last_snapshot_time_ms = snapshot_time_ms;
        self.last_receive_time_ms = snapshot_time_ms;
    }

    pub fn apply_diff(&mut self, diff: OrderBookDiff) -> Result<ApplyOutcome, OrderBookError> {
        if !self.synced {
            return Err(OrderBookError::NotSynced);
        }
        if diff.first_update_id == 0
            || diff.final_update_id == 0
            || diff.first_update_id > diff.final_update_id
        {
            return Err(OrderBookError::InvalidUpdateRange {
                first_update_id: diff.first_update_id,
                final_update_id: diff.final_update_id,
            });
        }

        let Some(last_update_id) = self.last_update_id else {
            return Err(OrderBookError::NotSynced);
        };

        if diff.final_update_id <= last_update_id {
            return Ok(ApplyOutcome::IgnoredStale);
        }

        let expected_next = last_update_id.saturating_add(1);

        if let Some(prev_final) = diff.prev_final_update_id {
            if prev_final != last_update_id {
                self.synced = false;
                return Err(OrderBookError::SequenceGap {
                    expected_next,
                    got_first: diff.first_update_id,
                    got_final: diff.final_update_id,
                    got_prev_final: Some(prev_final),
                });
            }
        } else if diff.first_update_id > expected_next || diff.final_update_id < expected_next {
            self.synced = false;
            return Err(OrderBookError::SequenceGap {
                expected_next,
                got_first: diff.first_update_id,
                got_final: diff.final_update_id,
                got_prev_final: None,
            });
        }

        apply_side_updates(&mut self.bids, diff.bids);
        apply_side_updates(&mut self.asks, diff.asks);
        self.last_update_id = Some(diff.final_update_id);
        self.last_event_time_ms = diff.event_time_ms;
        self.last_receive_time_ms = diff.receive_time_ms;

        Ok(ApplyOutcome::Applied)
    }

    pub fn best_bid(&self) -> Option<PriceLevel> {
        self.bids.iter().next_back().map(|(price, qty)| PriceLevel {
            price: *price,
            qty: *qty,
        })
    }

    pub fn best_ask(&self) -> Option<PriceLevel> {
        self.asks.iter().next().map(|(price, qty)| PriceLevel {
            price: *price,
            qty: *qty,
        })
    }

    pub fn top_of_book(&self) -> TopOfBook {
        TopOfBook {
            bid: self.best_bid(),
            ask: self.best_ask(),
        }
    }

    pub fn bid_depth_len(&self) -> usize {
        self.bids.len()
    }

    pub fn ask_depth_len(&self) -> usize {
        self.asks.len()
    }

    pub fn top_bids(&self, limit: usize) -> Vec<PriceLevel> {
        self.bids
            .iter()
            .rev()
            .take(limit)
            .map(|(price, qty)| PriceLevel {
                price: *price,
                qty: *qty,
            })
            .collect()
    }

    pub fn top_asks(&self, limit: usize) -> Vec<PriceLevel> {
        self.asks
            .iter()
            .take(limit)
            .map(|(price, qty)| PriceLevel {
                price: *price,
                qty: *qty,
            })
            .collect()
    }

    pub fn is_crossed(&self) -> bool {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => bid.price >= ask.price,
            _ => false,
        }
    }
}

fn apply_side_updates(side: &mut BTreeMap<Decimal, Decimal>, levels: Vec<PriceLevel>) {
    for level in levels {
        if level.qty <= Decimal::ZERO {
            side.remove(&level.price);
        } else {
            side.insert(level.price, level.qty);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ApplyOutcome, OrderBook, OrderBookDiff, OrderBookError, OrderBookSnapshot, PriceLevel,
    };
    use rust_decimal::Decimal;

    fn d(value: i64) -> Decimal {
        Decimal::from(value)
    }

    fn dec(mantissa: i64, scale: u32) -> Decimal {
        Decimal::new(mantissa, scale)
    }

    fn lvl(price: Decimal, qty: Decimal) -> PriceLevel {
        PriceLevel { price, qty }
    }

    fn snapshot(last_update_id: u64) -> OrderBookSnapshot {
        OrderBookSnapshot {
            last_update_id,
            bids: vec![lvl(d(100), d(2)), lvl(d(99), d(3))],
            asks: vec![lvl(d(101), d(4)), lvl(d(102), d(5))],
            snapshot_time_ms: Some(1_000),
        }
    }

    #[test]
    fn applies_snapshot_and_exposes_best_levels() {
        let mut book = OrderBook::new("btcusdt");
        book.apply_snapshot(snapshot(10));

        assert!(book.is_synced());
        assert_eq!(book.last_update_id(), Some(10));
        assert_eq!(book.bid_depth_len(), 2);
        assert_eq!(book.ask_depth_len(), 2);
        assert_eq!(book.best_bid(), Some(lvl(d(100), d(2))));
        assert_eq!(book.best_ask(), Some(lvl(d(101), d(4))));
        assert_eq!(book.top_bids(2), vec![lvl(d(100), d(2)), lvl(d(99), d(3))]);
        assert_eq!(book.top_asks(2), vec![lvl(d(101), d(4)), lvl(d(102), d(5))]);
        assert!(!book.is_crossed());
    }

    #[test]
    fn diff_updates_and_removes_levels() {
        let mut book = OrderBook::new("btcusdt");
        book.apply_snapshot(snapshot(10));

        let outcome = book
            .apply_diff(OrderBookDiff {
                first_update_id: 11,
                final_update_id: 12,
                prev_final_update_id: Some(10),
                bids: vec![
                    lvl(d(100), Decimal::ZERO), // remove
                    lvl(dec(985, 1), d(7)),     // 98.5 insert
                ],
                asks: vec![
                    lvl(d(101), d(8)),       // update qty
                    lvl(dec(1035, 1), d(1)), // 103.5 insert
                ],
                event_time_ms: Some(1_100),
                receive_time_ms: Some(1_101),
            })
            .expect("diff applies");

        assert_eq!(outcome, ApplyOutcome::Applied);
        assert_eq!(book.last_update_id(), Some(12));
        assert_eq!(book.last_event_time_ms(), Some(1_100));
        assert_eq!(book.last_receive_time_ms(), Some(1_101));
        assert_eq!(book.best_bid(), Some(lvl(d(99), d(3))));
        assert_eq!(book.best_ask(), Some(lvl(d(101), d(8))));
        assert_eq!(
            book.top_asks(3),
            vec![
                lvl(d(101), d(8)),
                lvl(d(102), d(5)),
                lvl(dec(1035, 1), d(1))
            ]
        );
    }

    #[test]
    fn ignores_stale_diff_events() {
        let mut book = OrderBook::new("btcusdt");
        book.apply_snapshot(snapshot(10));
        let before = book.top_of_book();

        let outcome = book
            .apply_diff(OrderBookDiff {
                first_update_id: 9,
                final_update_id: 10,
                prev_final_update_id: Some(8),
                bids: vec![lvl(d(100), d(999))],
                asks: vec![],
                event_time_ms: Some(2_000),
                receive_time_ms: Some(2_001),
            })
            .expect("stale diffs are ignored");

        assert_eq!(outcome, ApplyOutcome::IgnoredStale);
        assert_eq!(book.top_of_book(), before);
        assert_eq!(book.last_update_id(), Some(10));
    }

    #[test]
    fn accepts_overlapping_buffered_diff_after_snapshot() {
        let mut book = OrderBook::new("ethbtc");
        book.apply_snapshot(snapshot(100));

        let outcome = book
            .apply_diff(OrderBookDiff {
                first_update_id: 98,
                final_update_id: 101,
                prev_final_update_id: None,
                bids: vec![lvl(d(100), d(6))],
                asks: vec![],
                event_time_ms: None,
                receive_time_ms: None,
            })
            .expect("overlapping post-snapshot diff applies");

        assert_eq!(outcome, ApplyOutcome::Applied);
        assert_eq!(book.last_update_id(), Some(101));
        assert_eq!(book.best_bid(), Some(lvl(d(100), d(6))));
    }

    #[test]
    fn detects_sequence_gap_without_prev_final_id() {
        let mut book = OrderBook::new("bnbbtc");
        book.apply_snapshot(snapshot(10));

        let err = book
            .apply_diff(OrderBookDiff {
                first_update_id: 12,
                final_update_id: 13,
                prev_final_update_id: None,
                bids: vec![],
                asks: vec![],
                event_time_ms: None,
                receive_time_ms: None,
            })
            .expect_err("gap should be rejected");

        assert_eq!(
            err,
            OrderBookError::SequenceGap {
                expected_next: 11,
                got_first: 12,
                got_final: 13,
                got_prev_final: None,
            }
        );
        assert!(!book.is_synced());
    }

    #[test]
    fn detects_prev_final_mismatch() {
        let mut book = OrderBook::new("bnbbtc");
        book.apply_snapshot(snapshot(10));

        let err = book
            .apply_diff(OrderBookDiff {
                first_update_id: 11,
                final_update_id: 11,
                prev_final_update_id: Some(9),
                bids: vec![],
                asks: vec![],
                event_time_ms: None,
                receive_time_ms: None,
            })
            .expect_err("prev_final mismatch should be rejected");

        assert_eq!(
            err,
            OrderBookError::SequenceGap {
                expected_next: 11,
                got_first: 11,
                got_final: 11,
                got_prev_final: Some(9),
            }
        );
        assert!(!book.is_synced());
    }
}
