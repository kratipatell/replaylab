use crate::strategy::Side;
use crate::types::Candle;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ExitReason {
    Signal,
    StopLoss,
    TakeProfit,
    Trailing,
    SessionEnd,
    EndOfData,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum OrderKind {
    /// Fills at the next bar's open.
    Market,
    /// Rests until the bar trades through `price`.
    Limit { price: f64 },
    /// Triggers once the bar trades through `price`.
    Stop { price: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub struct EntryOrder {
    pub side: Side,
    pub kind: OrderKind,
}

/// Price at which `order` fills inside `bar`, or `None` while it rests.
///
/// A gap through the trigger fills at the open, which is the first price the
/// order could actually have traded at. A limit that gaps in the trader's
/// favour fills at the open, never at the older price.
#[allow(dead_code)]
pub fn fill_price(order: &EntryOrder, bar: &Candle) -> Option<f64> {
    match (order.side, order.kind) {
        (_, OrderKind::Market) => Some(bar.open),
        (Side::Long, OrderKind::Limit { price }) => {
            if bar.open <= price {
                Some(bar.open)
            } else if bar.low <= price {
                Some(price)
            } else {
                None
            }
        }
        (Side::Short, OrderKind::Limit { price }) => {
            if bar.open >= price {
                Some(bar.open)
            } else if bar.high >= price {
                Some(price)
            } else {
                None
            }
        }
        (Side::Long, OrderKind::Stop { price }) => {
            if bar.open >= price {
                Some(bar.open)
            } else if bar.high >= price {
                Some(price)
            } else {
                None
            }
        }
        (Side::Short, OrderKind::Stop { price }) => {
            if bar.open <= price {
                Some(bar.open)
            } else if bar.low <= price {
                Some(price)
            } else {
                None
            }
        }
    }
}

/// Trailing stop state. The level ratchets one way only and the peak it
/// follows is the best price the position has seen, not the current bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trail {
    peak: f64,
    percent: Option<f64>,
    base: Option<f64>,
    level: Option<f64>,
}

impl Trail {
    pub fn new(side: Side, entry: f64, percent: Option<f64>) -> Self {
        let level = percent.map(|p| {
            let t = p / 100.0;
            match side {
                Side::Long => entry * (1.0 - t),
                Side::Short => entry * (1.0 + t),
            }
        });
        let level = level.map(|l| (l * 1e12).round() / 1e12);
        Self {
            peak: entry,
            percent,
            base: level,
            level,
        }
    }

    #[allow(dead_code)]
    pub fn peak(&self) -> f64 {
        self.peak
    }

    /// Level measured from the entry price, before any ratchet.
    #[allow(dead_code)]
    pub fn base(&self) -> Option<f64> {
        self.base
    }

    /// Current ratcheted level.
    pub fn level(&self) -> Option<f64> {
        self.level
    }

    /// Advance the ratchet with `bar`, then return the new level.
    pub fn advance(&mut self, side: Side, bar: &Candle) -> Option<f64> {
        let pct = self.percent?;
        let t = pct / 100.0;
        let candidate = match side {
            Side::Long => {
                if bar.high > self.peak {
                    self.peak = bar.high;
                }
                self.peak * (1.0 - t)
            }
            Side::Short => {
                if bar.low < self.peak {
                    self.peak = bar.low;
                }
                self.peak * (1.0 + t)
            }
        };
        let candidate = (candidate * 1e12).round() / 1e12;
        self.level = Some(match (self.level, side) {
            (Some(old), Side::Long) => old.max(candidate),
            (Some(old), Side::Short) => old.min(candidate),
            (None, _) => candidate,
        });
        self.level = self.level.map(|l| (l * 1e12).round() / 1e12);
        self.level
    }
}

fn tighter(side: Side, fixed: Option<f64>, trail: Option<f64>) -> (Option<f64>, bool) {
    match (fixed, trail) {
        (Some(f), Some(t)) => {
            let trail_tighter = match side {
                Side::Long => t >= f,
                Side::Short => t <= f,
            };
            if trail_tighter {
                (Some(t), true)
            } else {
                (Some(f), false)
            }
        }
        (Some(f), None) => (Some(f), false),
        (None, Some(t)) => (Some(t), true),
        (None, None) => (None, false),
    }
}

fn reason_for(is_trail: bool) -> ExitReason {
    if is_trail {
        ExitReason::Trailing
    } else {
        ExitReason::StopLoss
    }
}

fn gapped_through(side: Side, level: f64, open: f64) -> bool {
    match side {
        Side::Long => open <= level,
        Side::Short => open >= level,
    }
}

/// Resolve a bracket against one bar: stop, target and trailing stop.
///
/// The stop is checked before the target when both are touched inside the
/// same bar, because the bar does not say which came first. A gap through a
/// level fills at the open.
pub fn resolve_exit(
    bar: &Candle,
    side: Side,
    fixed_stop: Option<f64>,
    target: Option<f64>,
    trail: &Trail,
) -> Option<(f64, ExitReason)> {
    let (gap_stop, gap_is_trail) = tighter(side, fixed_stop, trail.base());
    let (stop, stop_is_trail) = tighter(side, fixed_stop, trail.level());

    let stop_hit = match side {
        Side::Long => stop.is_some_and(|s| bar.low <= s) || gap_stop.is_some_and(|s| bar.open <= s),
        Side::Short => {
            stop.is_some_and(|s| bar.high >= s) || gap_stop.is_some_and(|s| bar.open >= s)
        }
    };
    let target_hit = match side {
        Side::Long => target.is_some_and(|t| bar.high >= t || bar.open >= t),
        Side::Short => target.is_some_and(|t| bar.low <= t || bar.open <= t),
    };

    let stop_exit = |is_trail: bool| match gap_stop {
        Some(g) if gapped_through(side, g, bar.open) => Some((bar.open, reason_for(is_trail))),
        _ => stop.map(|s| (s, reason_for(is_trail))),
    };

    match (stop_hit, target_hit) {
        (false, false) => None,
        (true, false) => stop_exit(stop_is_trail),
        (false, true) => target.map(|t| {
            let through = match side {
                Side::Long => bar.open >= t,
                Side::Short => bar.open <= t,
            };
            let raw = if through { bar.open } else { t };
            (raw, ExitReason::TakeProfit)
        }),
        (true, true) => {
            let target_gapped = target.is_some_and(|t| match side {
                Side::Long => bar.open >= t,
                Side::Short => bar.open <= t,
            });
            if target_gapped {
                Some((bar.open, ExitReason::TakeProfit))
            } else {
                stop_exit(gap_is_trail)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(open: f64, high: f64, low: f64, close: f64) -> Candle {
        Candle {
            ts: 0,
            open,
            high,
            low,
            close,
        }
    }

    fn order(side: Side, kind: OrderKind) -> EntryOrder {
        EntryOrder { side, kind }
    }

    #[test]
    fn market_fills_at_the_open() {
        let b = bar(100.0, 105.0, 95.0, 102.0);
        assert_eq!(
            fill_price(&order(Side::Long, OrderKind::Market), &b),
            Some(100.0)
        );
        assert_eq!(
            fill_price(&order(Side::Short, OrderKind::Market), &b),
            Some(100.0)
        );
    }

    #[test]
    fn buy_limit_gapping_below_fills_at_the_open() {
        let b = bar(96.0, 99.0, 95.0, 98.0);
        let o = order(Side::Long, OrderKind::Limit { price: 97.0 });
        assert_eq!(fill_price(&o, &b), Some(96.0));
    }

    #[test]
    fn buy_limit_touched_intrabar_fills_at_the_limit() {
        let b = bar(100.0, 101.0, 96.0, 99.0);
        let o = order(Side::Long, OrderKind::Limit { price: 97.0 });
        assert_eq!(fill_price(&o, &b), Some(97.0));
    }

    #[test]
    fn buy_limit_above_the_low_rests() {
        let b = bar(100.0, 101.0, 98.0, 99.0);
        let o = order(Side::Long, OrderKind::Limit { price: 97.0 });
        assert_eq!(fill_price(&o, &b), None);
    }

    #[test]
    fn sell_limit_gapping_above_fills_at_the_open() {
        let b = bar(104.0, 105.0, 102.0, 103.0);
        let o = order(Side::Short, OrderKind::Limit { price: 103.0 });
        assert_eq!(fill_price(&o, &b), Some(104.0));
    }

    #[test]
    fn sell_limit_touched_intrabar_fills_at_the_limit() {
        let b = bar(100.0, 104.0, 99.0, 101.0);
        let o = order(Side::Short, OrderKind::Limit { price: 103.0 });
        assert_eq!(fill_price(&o, &b), Some(103.0));
    }

    #[test]
    fn buy_stop_gapping_above_fills_at_the_open() {
        let b = bar(106.0, 108.0, 105.0, 107.0);
        let o = order(Side::Long, OrderKind::Stop { price: 104.0 });
        assert_eq!(fill_price(&o, &b), Some(106.0));
    }

    #[test]
    fn buy_stop_touched_intrabar_fills_at_the_trigger() {
        let b = bar(100.0, 105.0, 99.0, 104.0);
        let o = order(Side::Long, OrderKind::Stop { price: 104.0 });
        assert_eq!(fill_price(&o, &b), Some(104.0));
    }

    #[test]
    fn sell_stop_gapping_below_fills_at_the_open() {
        let b = bar(94.0, 95.0, 92.0, 93.0);
        let o = order(Side::Short, OrderKind::Stop { price: 96.0 });
        assert_eq!(fill_price(&o, &b), Some(94.0));
    }

    #[test]
    fn sell_stop_untouched_rests() {
        let b = bar(100.0, 101.0, 98.0, 99.0);
        let o = order(Side::Short, OrderKind::Stop { price: 95.0 });
        assert_eq!(fill_price(&o, &b), None);
    }

    #[test]
    fn long_stop_touched_exits_at_the_stop() {
        let b = bar(100.0, 101.0, 97.0, 98.0);
        let trail = Trail::new(Side::Long, 100.0, None);
        let got = resolve_exit(&b, Side::Long, Some(98.0), Some(105.0), &trail);
        assert_eq!(got, Some((98.0, ExitReason::StopLoss)));
    }

    #[test]
    fn long_stop_gapped_through_exits_at_the_open() {
        let b = bar(95.0, 96.0, 94.0, 95.0);
        let trail = Trail::new(Side::Long, 100.0, None);
        let got = resolve_exit(&b, Side::Long, Some(98.0), Some(105.0), &trail);
        assert_eq!(got, Some((95.0, ExitReason::StopLoss)));
    }

    #[test]
    fn long_target_touched_exits_at_the_target() {
        let b = bar(100.0, 106.0, 99.0, 105.0);
        let trail = Trail::new(Side::Long, 100.0, None);
        let got = resolve_exit(&b, Side::Long, Some(98.0), Some(105.0), &trail);
        assert_eq!(got, Some((105.0, ExitReason::TakeProfit)));
    }

    #[test]
    fn long_target_gapped_through_exits_at_the_open() {
        let b = bar(107.0, 108.0, 106.0, 107.5);
        let trail = Trail::new(Side::Long, 100.0, None);
        let got = resolve_exit(&b, Side::Long, Some(98.0), Some(105.0), &trail);
        assert_eq!(got, Some((107.0, ExitReason::TakeProfit)));
    }

    #[test]
    fn both_levels_in_one_bar_favour_the_stop() {
        let b = bar(100.0, 106.0, 97.0, 100.0);
        let trail = Trail::new(Side::Long, 100.0, None);
        let got = resolve_exit(&b, Side::Long, Some(98.0), Some(105.0), &trail);
        assert_eq!(got, Some((98.0, ExitReason::StopLoss)));
    }

    #[test]
    fn both_levels_with_a_gap_up_pay_the_target_at_the_open() {
        let b = bar(106.0, 107.0, 96.0, 100.0);
        let trail = Trail::new(Side::Long, 100.0, None);
        let got = resolve_exit(&b, Side::Long, Some(98.0), Some(105.0), &trail);
        assert_eq!(got, Some((106.0, ExitReason::TakeProfit)));
    }

    #[test]
    fn short_stop_and_target_mirror_the_long_side() {
        let b = bar(100.0, 103.0, 96.0, 102.0);
        let trail = Trail::new(Side::Short, 100.0, None);
        assert_eq!(
            resolve_exit(&b, Side::Short, Some(102.0), Some(96.0), &trail),
            Some((102.0, ExitReason::StopLoss))
        );
    }

    #[test]
    fn long_trailing_ratchets_up_only() {
        let mut trail = Trail::new(Side::Long, 100.0, Some(10.0));
        assert_eq!(trail.level(), Some(90.0));
        assert_eq!(trail.base(), Some(90.0));
        trail.advance(Side::Long, &bar(100.0, 120.0, 99.0, 118.0));
        assert_eq!(trail.level(), Some(108.0));
        trail.advance(Side::Long, &bar(110.0, 112.0, 105.0, 106.0));
        assert_eq!(trail.level(), Some(108.0));
        assert_eq!(trail.base(), Some(90.0));
    }

    #[test]
    fn short_trailing_ratchets_down_only() {
        let mut trail = Trail::new(Side::Short, 100.0, Some(10.0));
        assert_eq!(trail.level(), Some(110.0));
        trail.advance(Side::Short, &bar(100.0, 101.0, 80.0, 82.0));
        assert_eq!(trail.level(), Some(88.0));
        trail.advance(Side::Short, &bar(90.0, 95.0, 89.0, 94.0));
        assert_eq!(trail.level(), Some(88.0));
        assert_eq!(trail.base(), Some(110.0));
    }

    #[test]
    fn gap_check_uses_the_entry_relative_trail() {
        let mut trail = Trail::new(Side::Long, 100.0, Some(10.0));
        trail.advance(Side::Long, &bar(100.0, 150.0, 99.0, 148.0));
        assert_eq!(trail.level(), Some(135.0));
        // Open 130 is under the ratcheted level but above the entry-relative
        // one, so this is a stop hit, not a gap.
        let b = bar(130.0, 132.0, 129.0, 131.0);
        let got = resolve_exit(&b, Side::Long, None, None, &trail);
        assert_eq!(got, Some((135.0, ExitReason::Trailing)));
        // A true gap opens under the entry-relative trail and fills at the open.
        let b = bar(85.0, 86.0, 84.0, 85.0);
        let got = resolve_exit(&b, Side::Long, None, None, &trail);
        assert_eq!(got, Some((85.0, ExitReason::Trailing)));
    }

    #[test]
    fn the_fixed_stop_wins_when_it_is_the_tighter_one() {
        let mut trail = Trail::new(Side::Long, 100.0, Some(20.0));
        trail.advance(Side::Long, &bar(100.0, 105.0, 99.0, 104.0));
        assert_eq!(trail.level(), Some(84.0));
        let b = bar(100.0, 101.0, 92.0, 93.0);
        let got = resolve_exit(&b, Side::Long, Some(95.0), None, &trail);
        assert_eq!(got, Some((95.0, ExitReason::StopLoss)));
    }

    #[test]
    fn no_bracket_never_exits() {
        let b = bar(100.0, 200.0, 1.0, 100.0);
        let trail = Trail::new(Side::Long, 100.0, None);
        assert_eq!(resolve_exit(&b, Side::Long, None, None, &trail), None);
    }

    #[test]
    fn trail_without_a_percentage_stays_empty() {
        let mut trail = Trail::new(Side::Long, 100.0, None);
        assert_eq!(
            trail.advance(Side::Long, &bar(100.0, 120.0, 99.0, 118.0)),
            None
        );
        assert_eq!(trail.level(), None);
    }
}
