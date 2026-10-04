use crate::strategy::{Condition, IndicatorKind, Op, Operand, Rules, Side, Strategy};
use crate::{ema, rsi, sma, Candle};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BacktestConfig {
    pub fee_bps: f64,
    pub slippage_bps: f64,
}

impl Default for BacktestConfig {
    fn default() -> Self {
        Self {
            fee_bps: 5.0,
            slippage_bps: 2.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    Signal,
    StopLoss,
    TakeProfit,
    EndOfData,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trade {
    pub entry_idx: usize,
    pub exit_idx: usize,
    pub entry_ts: i64,
    pub exit_ts: i64,
    pub side: Side,
    pub entry_price: f64,
    pub exit_price: f64,
    pub return_pct: f64,
    pub reason: ExitReason,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BacktestResult {
    pub trades: Vec<Trade>,
    pub equity_curve: Vec<f64>,
}

fn kind_key(ind: IndicatorKind) -> u8 {
    match ind {
        IndicatorKind::Open => 0,
        IndicatorKind::High => 1,
        IndicatorKind::Low => 2,
        IndicatorKind::Close => 3,
        IndicatorKind::Sma => 4,
        IndicatorKind::Ema => 5,
        IndicatorKind::Rsi => 6,
    }
}

fn collect_refs(rules_list: &[&Rules], out: &mut Vec<(u8, Option<usize>)>) {
    for rules in rules_list {
        for c in rules.all.iter().chain(rules.any.iter()) {
            for op in [&c.left, &c.right] {
                if let Operand::Indicator(r) = op {
                    let k = (kind_key(r.ind), r.period);
                    if !out.contains(&k) {
                        out.push(k);
                    }
                }
            }
        }
    }
}

fn build_series(
    candles: &[Candle],
    refs: &[(u8, Option<usize>)],
) -> HashMap<(u8, Option<usize>), Vec<f64>> {
    let n = candles.len();
    let opens: Vec<f64> = candles.iter().map(|c| c.open).collect();
    let highs: Vec<f64> = candles.iter().map(|c| c.high).collect();
    let lows: Vec<f64> = candles.iter().map(|c| c.low).collect();
    let closes: Vec<f64> = candles.iter().map(|c| c.close).collect();
    let mut map = HashMap::new();
    for (k, period) in refs {
        let series = match *k {
            0 => opens.clone(),
            1 => highs.clone(),
            2 => lows.clone(),
            3 => closes.clone(),
            4 => sma(&closes, period.unwrap()),
            5 => ema(&closes, period.unwrap()),
            6 => rsi(&closes, period.unwrap()),
            _ => vec![f64::NAN; n],
        };
        map.insert((*k, *period), series);
    }
    map
}

fn operand_value(
    op: &Operand,
    idx: usize,
    map: &HashMap<(u8, Option<usize>), Vec<f64>>,
    opens: &[f64],
    highs: &[f64],
    lows: &[f64],
    closes: &[f64],
) -> f64 {
    match op {
        Operand::Number(v) => *v,
        Operand::Indicator(r) => {
            let key = (kind_key(r.ind), r.period);
            if let Some(s) = map.get(&key) {
                s[idx]
            } else {
                // Fallback (should not happen): compute directly.
                match r.ind {
                    IndicatorKind::Open => opens[idx],
                    IndicatorKind::High => highs[idx],
                    IndicatorKind::Low => lows[idx],
                    IndicatorKind::Close => closes[idx],
                    _ => f64::NAN,
                }
            }
        }
    }
}

fn eval_condition(
    c: &Condition,
    i: usize,
    map: &HashMap<(u8, Option<usize>), Vec<f64>>,
    opens: &[f64],
    highs: &[f64],
    lows: &[f64],
    closes: &[f64],
) -> bool {
    let l = operand_value(&c.left, i, map, opens, highs, lows, closes);
    let r = operand_value(&c.right, i, map, opens, highs, lows, closes);
    if l.is_nan() || r.is_nan() {
        return false;
    }
    match c.op {
        Op::Lt => l < r,
        Op::Gt => l > r,
        Op::CrossesAbove => {
            if i == 0 {
                return false;
            }
            let lp = operand_value(&c.left, i - 1, map, opens, highs, lows, closes);
            let rp = operand_value(&c.right, i - 1, map, opens, highs, lows, closes);
            if lp.is_nan() || rp.is_nan() {
                return false;
            }
            lp <= rp && l > r
        }
        Op::CrossesBelow => {
            if i == 0 {
                return false;
            }
            let lp = operand_value(&c.left, i - 1, map, opens, highs, lows, closes);
            let rp = operand_value(&c.right, i - 1, map, opens, highs, lows, closes);
            if lp.is_nan() || rp.is_nan() {
                return false;
            }
            lp >= rp && l < r
        }
    }
}

fn eval_rules(
    rules: &Rules,
    i: usize,
    map: &HashMap<(u8, Option<usize>), Vec<f64>>,
    opens: &[f64],
    highs: &[f64],
    lows: &[f64],
    closes: &[f64],
) -> bool {
    if rules.all.is_empty() && rules.any.is_empty() {
        return false;
    }
    for c in &rules.all {
        if !eval_condition(c, i, map, opens, highs, lows, closes) {
            return false;
        }
    }
    if rules.any.is_empty() {
        return true;
    }
    for c in &rules.any {
        if eval_condition(c, i, map, opens, highs, lows, closes) {
            return true;
        }
    }
    false
}

struct OpenPosition {
    entry_idx: usize,
    entry_ts: i64,
    raw_open: f64,
    entry_price: f64,
    equity_at_entry: f64,
}

pub fn backtest(
    strategy: &Strategy,
    candles: &[Candle],
    cfg: &BacktestConfig,
) -> Result<BacktestResult, String> {
    strategy.validate()?;
    let n = candles.len();
    if n == 0 {
        return Ok(BacktestResult {
            trades: Vec::new(),
            equity_curve: Vec::new(),
        });
    }

    let opens: Vec<f64> = candles.iter().map(|c| c.open).collect();
    let highs: Vec<f64> = candles.iter().map(|c| c.high).collect();
    let lows: Vec<f64> = candles.iter().map(|c| c.low).collect();
    let closes: Vec<f64> = candles.iter().map(|c| c.close).collect();

    let mut refs: Vec<(u8, Option<usize>)> = Vec::new();
    {
        let mut lists: Vec<&Rules> = vec![&strategy.entry];
        if let Some(exit) = &strategy.exit {
            lists.push(exit);
        }
        // collect_refs takes &[&Rules]
        collect_refs(&lists, &mut refs);
    }
    let map = build_series(candles, &refs);

    let cost = (cfg.fee_bps + cfg.slippage_bps) / 10000.0;
    let side = strategy.side;
    let sign = match side {
        Side::Long => 1.0,
        Side::Short => -1.0,
    };

    let mut trades: Vec<Trade> = Vec::new();
    let mut equity_curve: Vec<f64> = Vec::with_capacity(n);
    let mut equity: f64 = 1.0;
    let mut position: Option<OpenPosition> = None;
    let mut pending_entry = false;
    let mut pending_exit = false;
    let mut armed = true;

    // Precompute entry/exit rule truth per bar? Evaluate on demand at close.
    // We need entry truth per bar for armed logic; evaluate in loop.

    for i in 0..n {
        // (1) at the open, fill pending
        if let Some(_pos) = &position {
            if pending_exit {
                let raw = candles[i].open;
                let exit_price = match side {
                    Side::Long => raw * (1.0 - cost),
                    Side::Short => raw * (1.0 + cost),
                };
                let pos = position.take().unwrap();
                let ret = 100.0 * sign * (exit_price / pos.entry_price - 1.0);
                equity = pos.equity_at_entry * (1.0 + sign * (exit_price / pos.entry_price - 1.0));
                trades.push(Trade {
                    entry_idx: pos.entry_idx,
                    exit_idx: i,
                    entry_ts: pos.entry_ts,
                    exit_ts: candles[i].ts,
                    side,
                    entry_price: pos.entry_price,
                    exit_price,
                    return_pct: ret,
                    reason: ExitReason::Signal,
                });
                pending_exit = false;
                // pending_entry should be false while in position; ensure cleared?
                pending_entry = false;
                armed = false;
            }
        } else if pending_entry {
            let raw = candles[i].open;
            let entry_price = match side {
                Side::Long => raw * (1.0 + cost),
                Side::Short => raw * (1.0 - cost),
            };
            position = Some(OpenPosition {
                entry_idx: i,
                entry_ts: candles[i].ts,
                raw_open: raw,
                entry_price,
                equity_at_entry: equity,
            });
            pending_entry = false;
            pending_exit = false;
        }

        // (2) check stop loss / take profit using this bar's high and low
        if let Some(pos) = &position {
            let raw_entry = pos.raw_open;
            let (stop_level, target_level): (Option<f64>, Option<f64>) = match side {
                Side::Long => (
                    strategy
                        .stop_loss_pct
                        .map(|sl| raw_entry * (1.0 - sl / 100.0)),
                    strategy
                        .take_profit_pct
                        .map(|tp| raw_entry * (1.0 + tp / 100.0)),
                ),
                Side::Short => (
                    strategy
                        .stop_loss_pct
                        .map(|sl| raw_entry * (1.0 + sl / 100.0)),
                    strategy
                        .take_profit_pct
                        .map(|tp| raw_entry * (1.0 - tp / 100.0)),
                ),
            };
            let o = candles[i].open;
            let h = candles[i].high;
            let l = candles[i].low;
            let (sl_hit, tp_hit) = match side {
                Side::Long => (
                    stop_level.map(|s| l <= s || o <= s).unwrap_or(false),
                    target_level.map(|t| h >= t || o >= t).unwrap_or(false),
                ),
                Side::Short => (
                    stop_level.map(|s| h >= s || o >= s).unwrap_or(false),
                    target_level.map(|t| l <= t || o <= t).unwrap_or(false),
                ),
            };
            let exit_raw_opt: Option<(f64, ExitReason)> = if sl_hit && tp_hit {
                let t = target_level.unwrap();
                let tp_gap = match side {
                    Side::Long => o >= t,
                    Side::Short => o <= t,
                };
                if tp_gap {
                    // Bar opened at or beyond take-profit: TP fires at open first.
                    Some((o, ExitReason::TakeProfit))
                } else {
                    // stop wins
                    let s = stop_level.unwrap();
                    let raw = match side {
                        Side::Long => {
                            if o <= s {
                                o
                            } else {
                                s
                            }
                        }
                        Side::Short => {
                            if o >= s {
                                o
                            } else {
                                s
                            }
                        }
                    };
                    Some((raw, ExitReason::StopLoss))
                }
            } else if sl_hit {
                let s = stop_level.unwrap();
                let raw = match side {
                    Side::Long => {
                        if o <= s {
                            o
                        } else {
                            s
                        }
                    }
                    Side::Short => {
                        if o >= s {
                            o
                        } else {
                            s
                        }
                    }
                };
                Some((raw, ExitReason::StopLoss))
            } else if tp_hit {
                let t = target_level.unwrap();
                let raw = match side {
                    Side::Long => {
                        if o >= t {
                            o
                        } else {
                            t
                        }
                    }
                    Side::Short => {
                        if o <= t {
                            o
                        } else {
                            t
                        }
                    }
                };
                Some((raw, ExitReason::TakeProfit))
            } else {
                None
            };
            if let Some((raw_exit, reason)) = exit_raw_opt {
                let exit_price = match side {
                    Side::Long => raw_exit * (1.0 - cost),
                    Side::Short => raw_exit * (1.0 + cost),
                };
                let pos = position.take().unwrap();
                let ret = 100.0 * sign * (exit_price / pos.entry_price - 1.0);
                equity = pos.equity_at_entry * (1.0 + sign * (exit_price / pos.entry_price - 1.0));
                trades.push(Trade {
                    entry_idx: pos.entry_idx,
                    exit_idx: i,
                    entry_ts: pos.entry_ts,
                    exit_ts: candles[i].ts,
                    side,
                    entry_price: pos.entry_price,
                    exit_price,
                    return_pct: ret,
                    reason,
                });
                armed = false;
                // Any pending scheduled for this bar was already filled; no pending for next yet.
                // Clear pending just in case (should already be false).
                pending_entry = false;
                pending_exit = false;
            }
        }

        // (3) at the close, evaluate signals and schedule pending for i+1
        let entry_true = eval_rules(&strategy.entry, i, &map, &opens, &highs, &lows, &closes);
        let exit_true = match &strategy.exit {
            Some(rules) => eval_rules(rules, i, &map, &opens, &highs, &lows, &closes),
            None => false,
        };
        // Re-arm when entry is false (on this bar).
        if !entry_true {
            armed = true;
        }
        if i + 1 < n {
            if position.is_some() {
                if exit_true {
                    pending_exit = true;
                }
            } else if armed && entry_true {
                pending_entry = true;
            }
        }

        // Equity marking at close
        if let Some(pos) = &position {
            let mtm_raw = closes[i];
            let mtm_exit = match side {
                Side::Long => mtm_raw * (1.0 - cost),
                Side::Short => mtm_raw * (1.0 + cost),
            };
            let mtm_equity =
                pos.equity_at_entry * (1.0 + sign * (mtm_exit / pos.entry_price - 1.0));
            equity_curve.push(mtm_equity);
        } else {
            equity_curve.push(equity);
        }
    }

    // If still in a position after the last candle, close at last close with EndOfData.
    if let Some(pos) = position.take() {
        let raw_exit = candles[n - 1].close;
        let exit_price = match side {
            Side::Long => raw_exit * (1.0 - cost),
            Side::Short => raw_exit * (1.0 + cost),
        };
        let ret = 100.0 * sign * (exit_price / pos.entry_price - 1.0);
        equity = pos.equity_at_entry * (1.0 + sign * (exit_price / pos.entry_price - 1.0));
        trades.push(Trade {
            entry_idx: pos.entry_idx,
            exit_idx: n - 1,
            entry_ts: pos.entry_ts,
            exit_ts: candles[n - 1].ts,
            side,
            entry_price: pos.entry_price,
            exit_price,
            return_pct: ret,
            reason: ExitReason::EndOfData,
        });
        // Overwrite last equity mark with final close equity.
        if let Some(last) = equity_curve.last_mut() {
            *last = equity;
        }
    }

    Ok(BacktestResult {
        trades,
        equity_curve,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::{
        Condition, IndicatorKind, IndicatorRef, Op, Operand, Rules, Side, Strategy,
    };

    fn c(ts: i64, open: f64, high: f64, low: f64, close: f64) -> Candle {
        Candle {
            ts,
            open,
            high,
            low,
            close,
        }
    }

    fn ind(ind: IndicatorKind, period: Option<usize>) -> Operand {
        Operand::Indicator(IndicatorRef { ind, period })
    }

    fn num(v: f64) -> Operand {
        Operand::Number(v)
    }

    fn long_strategy(
        entry: Rules,
        exit: Option<Rules>,
        sl: Option<f64>,
        tp: Option<f64>,
    ) -> Strategy {
        Strategy {
            asset: "TEST".to_string(),
            timeframe: "1h".to_string(),
            side: Side::Long,
            entry,
            exit,
            stop_loss_pct: sl,
            take_profit_pct: tp,
            claim: None,
        }
    }

    fn short_strategy(
        entry: Rules,
        exit: Option<Rules>,
        sl: Option<f64>,
        tp: Option<f64>,
    ) -> Strategy {
        Strategy {
            asset: "TEST".to_string(),
            timeframe: "1h".to_string(),
            side: Side::Short,
            entry,
            exit,
            stop_loss_pct: sl,
            take_profit_pct: tp,
            claim: None,
        }
    }

    fn entry_close_gt(threshold: f64) -> Rules {
        Rules {
            all: vec![Condition {
                left: ind(IndicatorKind::Close, None),
                op: Op::Gt,
                right: num(threshold),
            }],
            any: vec![],
        }
    }

    fn entry_close_lt(threshold: f64) -> Rules {
        Rules {
            all: vec![Condition {
                left: ind(IndicatorKind::Close, None),
                op: Op::Lt,
                right: num(threshold),
            }],
            any: vec![],
        }
    }

    fn zero_cost() -> BacktestConfig {
        BacktestConfig {
            fee_bps: 0.0,
            slippage_bps: 0.0,
        }
    }

    #[test]
    fn entry_fill_is_next_bar_open_not_signal_close() {
        // Signal at close of bar 1 (close 110 > 100), fill must be open of bar 2 (200).
        let candles = vec![
            c(0, 90.0, 95.0, 85.0, 90.0),
            c(1, 90.0, 115.0, 90.0, 110.0),
            c(2, 200.0, 210.0, 190.0, 205.0),
            c(3, 205.0, 210.0, 200.0, 206.0),
        ];
        let strat = long_strategy(entry_close_gt(100.0), None, None, None);
        let res = backtest(&strat, &candles, &zero_cost()).unwrap();
        assert_eq!(res.trades.len(), 1);
        let t = &res.trades[0];
        assert_eq!(t.entry_idx, 2, "entry must fill at bar 2 open, not bar 1");
        assert!(
            (t.entry_price - 200.0).abs() < 1e-12,
            "entry_price {}, expected 200",
            t.entry_price
        );
        assert_ne!(t.entry_price, 110.0);
    }

    #[test]
    fn truncated_run_trades_appear_in_full_run_no_lookahead() {
        // Oscillating closes to generate multiple signal round-trips.
        let closes = [
            95.0, 105.0, 95.0, 105.0, 95.0, 105.0, 95.0, 105.0, 95.0, 105.0,
        ];
        let candles: Vec<Candle> = closes
            .iter()
            .enumerate()
            .map(|(i, cl)| c(i as i64, *cl, *cl + 1.0, *cl - 1.0, *cl))
            .collect();
        let entry = entry_close_gt(100.0);
        let exit_rules = Rules {
            all: vec![],
            any: vec![Condition {
                left: ind(IndicatorKind::Close, None),
                op: Op::Lt,
                right: num(100.0),
            }],
        };
        let strat = long_strategy(entry, Some(exit_rules), None, None);
        let cfg = zero_cost();
        let full = backtest(&strat, &candles, &cfg).unwrap();
        let k = 6;
        let trunc_candles = &candles[..k];
        let trunc = backtest(&strat, trunc_candles, &cfg).unwrap();
        // Every non-EndOfData trade in truncated must appear identically in full.
        for tt in trunc
            .trades
            .iter()
            .filter(|t| t.reason != ExitReason::EndOfData)
        {
            let found = full.trades.iter().any(|ft| {
                ft.entry_idx == tt.entry_idx
                    && ft.exit_idx == tt.exit_idx
                    && ft.entry_ts == tt.entry_ts
                    && ft.exit_ts == tt.exit_ts
                    && ft.side == tt.side
                    && (ft.entry_price - tt.entry_price).abs() < 1e-12
                    && (ft.exit_price - tt.exit_price).abs() < 1e-12
                    && (ft.return_pct - tt.return_pct).abs() < 1e-12
                    && ft.reason == tt.reason
            });
            assert!(
                found,
                "truncated trade {:?} not found identically in full {:?}",
                tt, full.trades
            );
        }
        // Sanity: truncated run produced at least one closed signal trade.
        assert!(
            trunc.trades.iter().any(|t| t.reason == ExitReason::Signal),
            "expected at least one signal trade in truncated run, got {:?}",
            trunc.trades
        );
    }

    #[test]
    fn stop_loss_wins_when_both_stop_and_target_hit() {
        let candles = vec![
            c(0, 90.0, 115.0, 90.0, 110.0),
            c(1, 100.0, 101.0, 99.0, 100.0),
            c(2, 100.0, 110.0, 90.0, 100.0),
            c(3, 100.0, 101.0, 99.0, 90.0),
        ];
        let strat = long_strategy(entry_close_gt(100.0), None, Some(2.0), Some(2.0));
        let res = backtest(&strat, &candles, &zero_cost()).unwrap();
        assert!(!res.trades.is_empty());
        let t = &res.trades[0];
        assert_eq!(t.entry_idx, 1);
        assert_eq!(t.exit_idx, 2);
        assert_eq!(t.reason, ExitReason::StopLoss);
        assert!(
            (t.exit_price - 98.0).abs() < 1e-9,
            "exit {} expected 98 (stop)",
            t.exit_price
        );
    }

    #[test]
    fn gap_through_stop_fills_at_open_not_stop_level() {
        let candles = vec![
            c(0, 90.0, 115.0, 90.0, 110.0),
            c(1, 100.0, 101.0, 99.0, 100.0),
            c(2, 90.0, 95.0, 85.0, 92.0),
            c(3, 92.0, 93.0, 91.0, 90.0),
        ];
        let strat = long_strategy(entry_close_gt(100.0), None, Some(2.0), None);
        let res = backtest(&strat, &candles, &zero_cost()).unwrap();
        assert!(!res.trades.is_empty());
        let t = &res.trades[0];
        assert_eq!(t.reason, ExitReason::StopLoss);
        assert_eq!(t.exit_idx, 2);
        assert!(
            (t.exit_price - 90.0).abs() < 1e-9,
            "gap fill: exit {} expected open 90, not stop 98",
            t.exit_price
        );
    }

    #[test]
    fn flat_prices_with_fees_lose_about_point_two_percent() {
        let candles: Vec<Candle> = (0..5).map(|i| c(i, 100.0, 100.0, 100.0, 100.0)).collect();
        // close 100 > 99 always true -> enter at bar 1, hold to end.
        let strat = long_strategy(entry_close_gt(99.0), None, None, None);
        let cfg = BacktestConfig {
            fee_bps: 10.0,
            slippage_bps: 0.0,
        };
        let res = backtest(&strat, &candles, &cfg).unwrap();
        assert_eq!(res.trades.len(), 1);
        let t = &res.trades[0];
        // Expected: cost=0.001, entry=100.1, exit=99.9, ret ~ -0.1998%
        let expected = 100.0 * (99.9 / 100.1 - 1.0);
        assert!(
            (t.return_pct - expected).abs() < 1e-9,
            "return {} expected {expected}",
            t.return_pct
        );
        assert!(
            (t.return_pct - (-0.2)).abs() < 1e-3,
            "return {} should be about -0.2%",
            t.return_pct
        );
    }

    #[test]
    fn reentry_after_stopout_requires_entry_false_bar() {
        let candles = vec![
            c(0, 100.0, 110.0, 100.0, 110.0), // signal true
            c(1, 100.0, 101.0, 90.0, 110.0),  // entry fill 100 + SL hit same bar
            c(2, 100.0, 101.0, 99.0, 110.0),  // entry true but must NOT enter (disarmed)
            c(3, 100.0, 101.0, 99.0, 90.0),   // entry false -> re-arm
            c(4, 100.0, 101.0, 99.0, 110.0),  // entry true -> schedule entry
            c(5, 100.0, 101.0, 99.0, 110.0),  // entry fill
            c(6, 100.0, 101.0, 99.0, 100.0),  // hold to end
        ];
        let strat = long_strategy(entry_close_gt(100.0), None, Some(2.0), None);
        let res = backtest(&strat, &candles, &zero_cost()).unwrap();
        assert_eq!(
            res.trades.len(),
            2,
            "expected exactly 2 trades, got {:?}",
            res.trades
        );
        assert_eq!(res.trades[0].entry_idx, 1);
        assert_eq!(res.trades[0].reason, ExitReason::StopLoss);
        // No trade with entry_idx 2 or 3 (immediate re-entry forbidden).
        assert!(
            !res.trades
                .iter()
                .any(|t| t.entry_idx == 2 || t.entry_idx == 3),
            "must not re-enter while entry still true: {:?}",
            res.trades
        );
        assert_eq!(
            res.trades[1].entry_idx, 5,
            "second entry after false bar, got {:?}",
            res.trades
        );
    }

    #[test]
    fn short_gains_when_price_falls_and_stop_is_above_entry() {
        // Falling: short should gain.
        let fall = vec![
            c(0, 100.0, 100.0, 90.0, 90.0),
            c(1, 100.0, 101.0, 99.0, 95.0),
            c(2, 95.0, 96.0, 90.0, 90.0),
            c(3, 90.0, 91.0, 85.0, 85.0),
        ];
        let strat = short_strategy(entry_close_lt(95.0), None, None, None);
        let res = backtest(&strat, &fall, &zero_cost()).unwrap();
        assert_eq!(res.trades.len(), 1);
        let t = &res.trades[0];
        assert_eq!(t.side, Side::Short);
        assert!(
            t.return_pct > 0.0,
            "short falling must gain, got {}",
            t.return_pct
        );

        // Rising with stop 2%: stop = 102 above entry 100, must trigger StopLoss and lose.
        let rise = vec![
            c(0, 100.0, 100.0, 90.0, 90.0),
            c(1, 100.0, 101.0, 99.0, 95.0),
            c(2, 100.0, 110.0, 99.0, 108.0),
            c(3, 108.0, 109.0, 107.0, 108.0),
        ];
        let strat_sl = short_strategy(entry_close_lt(95.0), None, Some(2.0), None);
        let res2 = backtest(&strat_sl, &rise, &zero_cost()).unwrap();
        assert_eq!(res2.trades.len(), 1);
        let t2 = &res2.trades[0];
        assert_eq!(t2.reason, ExitReason::StopLoss);
        assert!(
            (t2.exit_price - 102.0).abs() < 1e-9,
            "short stop must be above entry: exit {} expected 102",
            t2.exit_price
        );
        assert!(
            t2.return_pct < 0.0,
            "short rising must lose, got {}",
            t2.return_pct
        );
    }

    #[test]
    fn invalid_strategy_returns_err() {
        let bad = long_strategy(
            Rules {
                all: vec![],
                any: vec![],
            },
            None,
            None,
            None,
        );
        let candles = vec![c(0, 100.0, 101.0, 99.0, 100.0)];
        let res = backtest(&bad, &candles, &zero_cost());
        assert!(res.is_err(), "invalid strategy must return Err");
    }

    #[test]
    fn equity_curve_length_equals_candles_and_starts_at_one() {
        let candles: Vec<Candle> = (0..7)
            .map(|i| {
                c(
                    i,
                    100.0 + i as f64,
                    102.0 + i as f64,
                    99.0 + i as f64,
                    101.0 + i as f64,
                )
            })
            .collect();
        let strat = long_strategy(entry_close_gt(100.0), None, None, None);
        let res = backtest(&strat, &candles, &zero_cost()).unwrap();
        assert_eq!(res.equity_curve.len(), candles.len());
        assert!(
            (res.equity_curve[0] - 1.0).abs() < 1e-12,
            "equity must start at 1.0, got {}",
            res.equity_curve[0]
        );
    }

    #[test]
    fn crosses_above_number_triggers_once() {
        let closes = [95.0, 99.0, 101.0, 102.0, 103.0];
        let candles: Vec<Candle> = closes
            .iter()
            .enumerate()
            .map(|(i, cl)| c(i as i64, *cl, *cl, *cl, *cl))
            .collect();
        let entry = Rules {
            all: vec![Condition {
                left: ind(IndicatorKind::Close, None),
                op: Op::CrossesAbove,
                right: num(100.0),
            }],
            any: vec![],
        };
        let strat = long_strategy(entry, None, None, None);
        let res = backtest(&strat, &candles, &zero_cost()).unwrap();
        assert_eq!(
            res.trades.len(),
            1,
            "expected exactly one trade, got {:?}",
            res.trades
        );
        assert_eq!(res.trades[0].entry_idx, 3);
    }

    #[test]
    fn crosses_below_number_triggers_once_short() {
        let closes = [105.0, 101.0, 99.0, 98.0, 97.0];
        let candles: Vec<Candle> = closes
            .iter()
            .enumerate()
            .map(|(i, cl)| c(i as i64, *cl, *cl, *cl, *cl))
            .collect();
        let entry = Rules {
            all: vec![Condition {
                left: ind(IndicatorKind::Close, None),
                op: Op::CrossesBelow,
                right: num(100.0),
            }],
            any: vec![],
        };
        let strat = short_strategy(entry, None, None, None);
        let res = backtest(&strat, &candles, &zero_cost()).unwrap();
        assert_eq!(
            res.trades.len(),
            1,
            "expected exactly one trade, got {:?}",
            res.trades
        );
        assert_eq!(res.trades[0].entry_idx, 3);
    }

    #[test]
    fn take_profit_fills_at_target_level() {
        let candles = vec![
            c(0, 90.0, 115.0, 90.0, 110.0),
            c(1, 100.0, 101.0, 99.0, 100.0),
            c(2, 100.0, 106.0, 99.0, 102.0),
        ];
        let strat = long_strategy(entry_close_gt(100.0), None, None, Some(5.0));
        let res = backtest(&strat, &candles, &zero_cost()).unwrap();
        assert_eq!(
            res.trades.len(),
            1,
            "expected one trade, got {:?}",
            res.trades
        );
        let t = &res.trades[0];
        assert_eq!(t.reason, ExitReason::TakeProfit);
        assert!(
            (t.exit_price - 105.0).abs() < 1e-12,
            "exit {} expected exactly 105.0",
            t.exit_price
        );
    }

    #[test]
    fn any_semantics_entry() {
        let entry = Rules {
            all: vec![],
            any: vec![
                Condition {
                    left: ind(IndicatorKind::Close, None),
                    op: Op::Lt,
                    right: num(90.0),
                },
                Condition {
                    left: ind(IndicatorKind::Close, None),
                    op: Op::Gt,
                    right: num(110.0),
                },
            ],
        };
        // Closes 100, 85, 100, 100: signal at idx 1 fills at idx 2.
        let closes = [100.0, 85.0, 100.0, 100.0];
        let candles: Vec<Candle> = closes
            .iter()
            .enumerate()
            .map(|(i, cl)| c(i as i64, *cl, *cl, *cl, *cl))
            .collect();
        let strat = long_strategy(entry.clone(), None, None, None);
        let res = backtest(&strat, &candles, &zero_cost()).unwrap();
        assert_eq!(
            res.trades.len(),
            1,
            "expected one trade, got {:?}",
            res.trades
        );
        assert_eq!(res.trades[0].entry_idx, 2);
        // All flat: no trades.
        let flat_closes = [100.0, 100.0, 100.0, 100.0];
        let flat: Vec<Candle> = flat_closes
            .iter()
            .enumerate()
            .map(|(i, cl)| c(i as i64, *cl, *cl, *cl, *cl))
            .collect();
        let strat2 = long_strategy(entry, None, None, None);
        let res2 = backtest(&strat2, &flat, &zero_cost()).unwrap();
        assert!(
            res2.trades.is_empty(),
            "expected no trades, got {:?}",
            res2.trades
        );
    }

    #[test]
    fn equity_curve_equals_compounded_trade_returns() {
        let closes = [
            95.0, 105.0, 95.0, 105.0, 95.0, 105.0, 95.0, 105.0, 95.0, 105.0,
        ];
        let candles: Vec<Candle> = closes
            .iter()
            .enumerate()
            .map(|(i, cl)| c(i as i64, *cl, *cl + 1.0, *cl - 1.0, *cl))
            .collect();
        let entry = entry_close_gt(100.0);
        let exit_rules = Rules {
            all: vec![],
            any: vec![Condition {
                left: ind(IndicatorKind::Close, None),
                op: Op::Lt,
                right: num(100.0),
            }],
        };
        let strat = long_strategy(entry, Some(exit_rules), None, None);
        let cfg = BacktestConfig::default();
        let res = backtest(&strat, &candles, &cfg).unwrap();
        let mut expected = 1.0;
        for t in &res.trades {
            expected *= 1.0 + t.return_pct / 100.0;
        }
        let last = *res.equity_curve.last().expect("equity curve non-empty");
        assert!(
            (last - expected).abs() < 1e-9,
            "last equity {last} != compounded {expected}"
        );
    }

    #[test]
    fn no_lookahead_with_indicator_truncation() {
        let n = 40usize;
        let closes: Vec<f64> = (0..n)
            .map(|i| 100.0 + 10.0 * (i as f64 * 0.7).sin())
            .collect();
        let mut candles: Vec<Candle> = Vec::with_capacity(n);
        let mut prev_close: f64 = 100.0;
        for (i, cl) in closes.iter().enumerate() {
            let open: f64 = if i == 0 { 100.0 } else { prev_close };
            let high = open.max(*cl) + 1.0;
            let low = open.min(*cl) - 1.0;
            candles.push(c(i as i64, open, high, low, *cl));
            prev_close = *cl;
        }
        let entry = Rules {
            all: vec![Condition {
                left: ind(IndicatorKind::Close, None),
                op: Op::CrossesAbove,
                right: ind(IndicatorKind::Sma, Some(3)),
            }],
            any: vec![],
        };
        let exit_rules = Rules {
            all: vec![Condition {
                left: ind(IndicatorKind::Close, None),
                op: Op::CrossesBelow,
                right: ind(IndicatorKind::Sma, Some(3)),
            }],
            any: vec![],
        };
        let strat = long_strategy(entry, Some(exit_rules), None, None);
        let cfg = zero_cost();
        let full = backtest(&strat, &candles, &cfg).unwrap();
        let trunc = backtest(&strat, &candles[..25], &cfg).unwrap();
        for tt in trunc
            .trades
            .iter()
            .filter(|t| t.reason != ExitReason::EndOfData)
        {
            let found = full.trades.iter().any(|ft| {
                ft.entry_idx == tt.entry_idx
                    && ft.exit_idx == tt.exit_idx
                    && ft.entry_ts == tt.entry_ts
                    && ft.exit_ts == tt.exit_ts
                    && ft.side == tt.side
                    && (ft.entry_price - tt.entry_price).abs() < 1e-12
                    && (ft.exit_price - tt.exit_price).abs() < 1e-12
                    && (ft.return_pct - tt.return_pct).abs() < 1e-12
                    && ft.reason == tt.reason
            });
            assert!(
                found,
                "truncated trade {:?} not found identically in full {:?}",
                tt, full.trades
            );
        }
        assert!(
            trunc
                .trades
                .iter()
                .any(|t| t.reason != ExitReason::EndOfData),
            "expected at least one non-EndOfData trade in truncated run, got {:?}",
            trunc.trades
        );
    }

    #[test]
    fn gap_up_open_beyond_target_fills_take_profit_at_open() {
        let candles = vec![
            c(0, 90.0, 115.0, 90.0, 110.0),
            c(1, 100.0, 101.0, 99.0, 100.0),
            c(2, 105.0, 106.0, 90.0, 95.0),
        ];
        let strat = long_strategy(entry_close_gt(100.0), None, Some(2.0), Some(2.0));
        let res = backtest(&strat, &candles, &zero_cost()).unwrap();
        assert!(!res.trades.is_empty());
        let t = &res.trades[0];
        assert_eq!(t.reason, ExitReason::TakeProfit);
        assert!(
            (t.exit_price - 105.0).abs() < 1e-12,
            "exit {} expected open 105.0",
            t.exit_price
        );
    }
}
