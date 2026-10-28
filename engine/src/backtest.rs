use crate::instrument::Instrument;
use crate::orders;
use crate::orders::Trail;
use crate::session::SessionFilter;
use crate::sizing::{quantity, Sizing};
use crate::strategy::{BarStrategy, Side, Strategy};
use crate::types::Candle;
use serde::Serialize;

pub use crate::orders::ExitReason;

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

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BacktestResult {
    pub trades: Vec<Trade>,
    pub equity_curve: Vec<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bracket {
    pub stop_pct: Option<f64>,
    pub target_pct: Option<f64>,
    pub trailing_pct: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BacktestError {
    #[error("{0} must be > 0, got {1}")]
    BadPct(String, f64),
    #[error("initial_equity must be > 0, got {0}")]
    BadEquity(f64),
    #[error(transparent)]
    Strategy(#[from] crate::strategy::StrategyError),
    #[error(transparent)]
    Instrument(#[from] crate::instrument::InstrumentError),
    #[error(transparent)]
    Sizing(#[from] crate::sizing::SizingError),
    #[error("internal error: {0}")]
    Internal(String),
}

impl Bracket {
    pub fn validate(&self) -> Result<(), BacktestError> {
        for (name, v) in [
            ("stop_pct", self.stop_pct),
            ("target_pct", self.target_pct),
            ("trailing_pct", self.trailing_pct),
        ] {
            if let Some(x) = v {
                if !x.is_finite() || x <= 0.0 {
                    return Err(BacktestError::BadPct(name.to_string(), x));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunConfig {
    pub initial_equity: f64,
    pub exit_at_session_end: bool,
}

impl RunConfig {
    pub fn new(initial_equity: f64, exit_at_session_end: bool) -> Result<RunConfig, BacktestError> {
        if !initial_equity.is_finite() || initial_equity <= 0.0 {
            return Err(BacktestError::BadEquity(initial_equity));
        }
        Ok(RunConfig {
            initial_equity,
            exit_at_session_end,
        })
    }
}

struct OpenPos {
    entry_idx: usize,
    entry_ts: i64,
    entry_raw: f64,
    side: Side,
    qty: f64,
    equity_at_entry: f64,
    entry_cost: f64,
    peak: f64,
    trail: Trail,
}

fn levels(side: Side, entry: f64, b: &Bracket) -> (Option<f64>, Option<f64>, Option<f64>) {
    let pct = |p: f64| p / 100.0;
    match side {
        Side::Long => (
            b.stop_pct.map(|s| entry * (1.0 - pct(s))),
            b.target_pct.map(|t| entry * (1.0 + pct(t))),
            b.trailing_pct.map(|t| entry * (1.0 - pct(t))),
        ),
        Side::Short => (
            b.stop_pct.map(|s| entry * (1.0 + pct(s))),
            b.target_pct.map(|t| entry * (1.0 - pct(t))),
            b.trailing_pct.map(|t| entry * (1.0 + pct(t))),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    strategy: &mut dyn BarStrategy,
    candles: &[Candle],
    instrument: &Instrument,
    sizing: Sizing,
    bracket: &Bracket,
    session: &SessionFilter,
    cfg: &RunConfig,
) -> Result<BacktestResult, BacktestError> {
    bracket.validate()?;
    if !cfg.initial_equity.is_finite() || cfg.initial_equity <= 0.0 {
        return Err(BacktestError::BadEquity(cfg.initial_equity));
    }
    let n = candles.len();
    if n == 0 {
        return Ok(BacktestResult {
            trades: Vec::new(),
            equity_curve: Vec::new(),
        });
    }
    let cost_rate = instrument.cost_rate();
    let pv = instrument.point_value;

    let mut trades: Vec<Trade> = Vec::new();
    let mut equity_curve: Vec<f64> = Vec::with_capacity(n);
    let mut equity = cfg.initial_equity;
    let mut position: Option<OpenPos> = None;
    let mut pending: Option<(Side, usize)> = None;
    let mut pending_exit = false;

    let close_position = |pos: &OpenPos,
                          exit_raw: f64,
                          exit_ts: i64,
                          exit_idx: usize,
                          reason: ExitReason,
                          equity_before: f64|
     -> (f64, Trade) {
        let sign = match pos.side {
            Side::Long => 1.0,
            Side::Short => -1.0,
        };
        let exit_cost = cost_rate * pos.qty * pv * exit_raw;
        let pnl = sign * pos.qty * pv * (exit_raw - pos.entry_raw);
        let after = equity_before + pnl - pos.entry_cost - exit_cost;
        let ret = 100.0 * (after / equity_before - 1.0);
        let trade = Trade {
            entry_idx: pos.entry_idx,
            exit_idx,
            entry_ts: pos.entry_ts,
            exit_ts,
            side: pos.side,
            entry_price: pos.entry_raw,
            exit_price: exit_raw,
            return_pct: ret,
            reason,
        };
        (after, trade)
    };

    for i in 0..n {
        if pending_exit {
            if let Some(pos) = position.take() {
                let raw_exit = candles[i].open;
                let (after, trade) =
                    close_position(&pos, raw_exit, candles[i].ts, i, ExitReason::Signal, equity);
                equity = after;
                trades.push(trade);
            }
            pending_exit = false;
        }
        if position.is_none() {
            if let Some((side, sig_idx)) = pending.take() {
                if session.contains(candles[sig_idx].ts) && session.contains(candles[i].ts) {
                    let entry_raw = instrument.round_to_tick(candles[i].open);
                    let qty = quantity(sizing, equity, entry_raw)?;
                    let entry_cost = cost_rate * qty * pv * entry_raw;
                    position = Some(OpenPos {
                        entry_idx: i,
                        entry_ts: candles[i].ts,
                        entry_raw,
                        side,
                        qty,
                        equity_at_entry: equity,
                        entry_cost,
                        peak: entry_raw,
                        trail: Trail::new(side, entry_raw, bracket.trailing_pct),
                    });
                    strategy.on_fill(i);
                }
            }
        }

        if let Some(mut pos) = position.take() {
            let (fixed_stop, target, _base_trail) = levels(pos.side, pos.entry_raw, bracket);
            let mut peak = pos.peak;
            if bracket.trailing_pct.is_some() {
                match pos.side {
                    Side::Long => {
                        if candles[i].high > peak {
                            peak = candles[i].high;
                        }
                    }
                    Side::Short => {
                        if candles[i].low < peak {
                            peak = candles[i].low;
                        }
                    }
                }
            }
            pos.peak = peak;
            let _ = pos.trail.advance(pos.side, &candles[i]);
            let exit = orders::resolve_exit(&candles[i], pos.side, fixed_stop, target, &pos.trail);
            if let Some((raw_exit, reason)) = exit {
                let (after, trade) =
                    close_position(&pos, raw_exit, candles[i].ts, i, reason, equity);
                equity = after;
                trades.push(trade);
            } else {
                position = Some(pos);
                if cfg.exit_at_session_end
                    && session.contains(candles[i].ts)
                    && (i + 1 == n || !session.contains(candles[i + 1].ts))
                {
                    let Some(pos) = position.take() else {
                        return Err(BacktestError::Internal("missing position".to_string()));
                    };
                    let raw_exit = candles[i].close;
                    let (after, trade) = close_position(
                        &pos,
                        raw_exit,
                        candles[i].ts,
                        i,
                        ExitReason::SessionEnd,
                        equity,
                    );
                    equity = after;
                    trades.push(trade);
                }
            }
        }

        if i + 1 < n {
            if position.is_some() {
                if strategy.exit_on_bar(i, candles) {
                    pending_exit = true;
                }
            } else if pending.is_none() && session.contains(candles[i].ts) {
                if let Some(side) = strategy.on_bar(i, candles) {
                    pending = Some((side, i));
                }
            }
        }

        if let Some(pos) = &position {
            let sign = match pos.side {
                Side::Long => 1.0,
                Side::Short => -1.0,
            };
            let mtm_cost = cost_rate * pos.qty * pv * candles[i].close;
            let mtm = pos.equity_at_entry
                + sign * pos.qty * pv * (candles[i].close - pos.entry_raw)
                - pos.entry_cost
                - mtm_cost;
            equity_curve.push(mtm);
        } else {
            equity_curve.push(equity);
        }
    }

    if let Some(pos) = position.take() {
        let raw_exit = candles[n - 1].close;
        let (after, trade) = close_position(
            &pos,
            raw_exit,
            candles[n - 1].ts,
            n - 1,
            ExitReason::EndOfData,
            equity,
        );
        equity = after;
        trades.push(trade);
        if let Some(last) = equity_curve.last_mut() {
            *last = equity;
        }
    }

    Ok(BacktestResult {
        trades,
        equity_curve,
    })
}

pub fn backtest_strategy(
    spec: &Strategy,
    candles: &[Candle],
    cfg: &BacktestConfig,
) -> Result<BacktestResult, BacktestError> {
    spec.validate()?;
    let (instrument, sizing, bracket, session, run_cfg) =
        crate::strategy::rules_v2_setup(spec, cfg)?;
    let mut strategy = crate::strategy::RulesStrategy::new(spec);
    run(
        &mut strategy,
        candles,
        &instrument,
        sizing,
        &bracket,
        &session,
        &run_cfg,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::MS_PER_MIN;

    fn c(ts: i64, o: f64, h: f64, l: f64, cl: f64) -> Candle {
        Candle {
            ts,
            open: o,
            high: h,
            low: l,
            close: cl,
        }
    }

    fn inst() -> Instrument {
        Instrument::new("TEST", 0.01, 1.0, 0.0, 0.0).expect("valid")
    }

    fn session_all() -> SessionFilter {
        SessionFilter::all_day()
    }

    fn run_cfg() -> RunConfig {
        RunConfig::new(10_000.0, false).expect("valid")
    }

    struct Sig(Vec<Option<Side>>);

    impl BarStrategy for Sig {
        fn on_bar(&mut self, idx: usize, _candles: &[Candle]) -> Option<Side> {
            self.0.get(idx).copied().flatten()
        }
    }

    #[test]
    fn stop_loss_fires_at_level() {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 101.0, 97.0, 100.0),
            c(2 * MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
        ];
        let mut s = Sig(vec![Some(Side::Long), None, None]);
        let b = Bracket {
            stop_pct: Some(2.0),
            target_pct: None,
            trailing_pct: None,
        };
        let r = run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &b,
            &session_all(),
            &run_cfg(),
        )
        .unwrap();
        assert_eq!(r.trades.len(), 1);
        assert_eq!(r.trades[0].entry_idx, 1);
        assert_eq!(r.trades[0].exit_idx, 1);
        assert!((r.trades[0].exit_price - 98.0).abs() < 1e-9);
        assert_eq!(r.trades[0].reason, ExitReason::StopLoss);
        assert!((r.equity_curve[1] - 9_998.0).abs() < 1e-9);
    }

    #[test]
    fn take_profit_fires_at_level() {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 103.0, 100.0, 102.0),
            c(2 * MS_PER_MIN, 102.0, 102.0, 102.0, 102.0),
        ];
        let mut s = Sig(vec![Some(Side::Long), None, None]);
        let b = Bracket {
            stop_pct: None,
            target_pct: Some(2.0),
            trailing_pct: None,
        };
        let r = run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &b,
            &session_all(),
            &run_cfg(),
        )
        .unwrap();
        assert_eq!(r.trades.len(), 1);
        assert!((r.trades[0].exit_price - 102.0).abs() < 1e-9);
        assert_eq!(r.trades[0].reason, ExitReason::TakeProfit);
        assert!((r.equity_curve[1] - 10_002.0).abs() < 1e-9);
    }

    #[test]
    fn trailing_ratchets_and_fires() {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 110.0, 109.0, 109.0),
            c(2 * MS_PER_MIN, 109.0, 109.0, 108.0, 108.5),
            c(3 * MS_PER_MIN, 108.5, 108.5, 108.5, 108.5),
        ];
        let mut s = Sig(vec![Some(Side::Long), None, None, None]);
        let b = Bracket {
            stop_pct: None,
            target_pct: None,
            trailing_pct: Some(1.0),
        };
        let r = run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &b,
            &session_all(),
            &run_cfg(),
        )
        .unwrap();
        assert_eq!(r.trades.len(), 1);
        assert_eq!(r.trades[0].reason, ExitReason::Trailing);
        assert!((r.trades[0].exit_price - 108.9).abs() < 1e-9);
        assert_eq!(r.trades[0].exit_idx, 2);
    }

    #[test]
    fn out_of_session_signal_is_dropped() {
        let eight = 8 * 60 * MS_PER_MIN;
        let ten = 10 * 60 * MS_PER_MIN;
        let candles = vec![
            c(eight, 100.0, 100.0, 100.0, 100.0),
            c(eight + MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
            c(ten, 100.0, 100.0, 100.0, 100.0),
        ];
        let mut s = Sig(vec![Some(Side::Long), None, None]);
        let b = Bracket {
            stop_pct: None,
            target_pct: None,
            trailing_pct: None,
        };
        let session = SessionFilter::new(9 * 60 + 30, 16 * 60).expect("valid");
        let r = run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &b,
            &session,
            &run_cfg(),
        )
        .unwrap();
        assert!(r.trades.is_empty());
    }

    struct SigExit {
        entries: Vec<Option<Side>>,
        exits: Vec<bool>,
    }

    impl BarStrategy for SigExit {
        fn on_bar(&mut self, idx: usize, _candles: &[Candle]) -> Option<Side> {
            self.entries.get(idx).copied().flatten()
        }
        fn exit_on_bar(&mut self, idx: usize, _candles: &[Candle]) -> bool {
            self.exits.get(idx).copied().unwrap_or(false)
        }
    }

    fn flat_bracket() -> Bracket {
        Bracket {
            stop_pct: None,
            target_pct: None,
            trailing_pct: None,
        }
    }

    #[test]
    fn signal_exit_fills_next_open() {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
            c(2 * MS_PER_MIN, 105.0, 105.0, 105.0, 105.0),
            c(3 * MS_PER_MIN, 105.0, 105.0, 105.0, 105.0),
        ];
        let mut s = SigExit {
            entries: vec![Some(Side::Long), None, None, None],
            exits: vec![false, true, false, false],
        };
        let r = run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &flat_bracket(),
            &session_all(),
            &run_cfg(),
        )
        .unwrap();
        assert_eq!(r.trades.len(), 1);
        assert_eq!(r.trades[0].entry_idx, 1);
        assert_eq!(r.trades[0].exit_idx, 2);
        assert!((r.trades[0].exit_price - 105.0).abs() < 1e-9);
        assert_eq!(r.trades[0].reason, ExitReason::Signal);
    }

    #[test]
    fn signal_on_last_bar_is_end_of_data() {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
            c(2 * MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
        ];
        let mut s = SigExit {
            entries: vec![Some(Side::Long), None, None],
            exits: vec![false, false, true],
        };
        let r = run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &flat_bracket(),
            &session_all(),
            &run_cfg(),
        )
        .unwrap();
        assert_eq!(r.trades.len(), 1);
        assert_eq!(r.trades[0].exit_idx, 2);
        assert_eq!(r.trades[0].reason, ExitReason::EndOfData);
    }

    #[test]
    fn pending_signal_exit_beats_gap_through_stop() {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
            c(2 * MS_PER_MIN, 90.0, 91.0, 89.0, 90.0),
        ];
        let mut s = SigExit {
            entries: vec![Some(Side::Long), None, None],
            exits: vec![false, true, false],
        };
        let b = Bracket {
            stop_pct: Some(2.0),
            target_pct: None,
            trailing_pct: None,
        };
        let r = run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &b,
            &session_all(),
            &run_cfg(),
        )
        .unwrap();
        assert_eq!(r.trades.len(), 1);
        assert_eq!(r.trades[0].exit_idx, 2);
        assert!((r.trades[0].exit_price - 90.0).abs() < 1e-9);
        assert_eq!(r.trades[0].reason, ExitReason::Signal);
    }

    #[test]
    fn default_strategy_never_exits() {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
            c(2 * MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
        ];
        let mut s = Sig(vec![Some(Side::Long), None, None]);
        assert!(!s.exit_on_bar(0, &candles));
        assert!(!s.exit_on_bar(1, &candles));
        let r = run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &flat_bracket(),
            &session_all(),
            &run_cfg(),
        )
        .unwrap();
        assert_eq!(r.trades.len(), 1);
        assert_eq!(r.trades[0].reason, ExitReason::EndOfData);
    }

    #[test]
    fn bracket_rejects_bad_pct() {
        let b = Bracket {
            stop_pct: Some(0.0),
            target_pct: None,
            trailing_pct: None,
        };
        assert!(b.validate().is_err());
    }
}

#[cfg(test)]
mod legacy_tests {
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
        let candles = vec![
            c(0, 90.0, 95.0, 85.0, 90.0),
            c(1, 90.0, 115.0, 90.0, 110.0),
            c(2, 200.0, 210.0, 190.0, 205.0),
            c(3, 205.0, 210.0, 200.0, 206.0),
        ];
        let strat = long_strategy(entry_close_gt(100.0), None, None, None);
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
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
        let full = backtest_strategy(&strat, &candles, &cfg).unwrap();
        let k = 6;
        let trunc_candles = &candles[..k];
        let trunc = backtest_strategy(&strat, trunc_candles, &cfg).unwrap();
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
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
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
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
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
        let strat = long_strategy(entry_close_gt(99.0), None, None, None);
        let cfg = BacktestConfig {
            fee_bps: 10.0,
            slippage_bps: 0.0,
        };
        let res = backtest_strategy(&strat, &candles, &cfg).unwrap();
        assert_eq!(res.trades.len(), 1);
        let t = &res.trades[0];
        assert!(
            (t.return_pct - (-0.2)).abs() < 1e-9,
            "return {} expected -0.2",
            t.return_pct
        );
    }

    #[test]
    fn reentry_after_stopout_requires_entry_false_bar() {
        let candles = vec![
            c(0, 100.0, 110.0, 100.0, 110.0),
            c(1, 100.0, 101.0, 90.0, 110.0), // entry fill 100 + SL hit same bar,
            c(2, 100.0, 101.0, 99.0, 110.0), // entry true but must NOT enter (disarmed),
            c(3, 100.0, 101.0, 99.0, 90.0),  // entry false -> re-arm,
            c(4, 100.0, 101.0, 99.0, 110.0), // entry true -> schedule entry,
            c(5, 100.0, 101.0, 99.0, 110.0), // entry fill,
            c(6, 100.0, 101.0, 99.0, 100.0), // hold to end,
        ];
        let strat = long_strategy(entry_close_gt(100.0), None, Some(2.0), None);
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
        assert_eq!(
            res.trades.len(),
            2,
            "expected exactly 2 trades, got {:?}",
            res.trades
        );
        assert_eq!(res.trades[0].entry_idx, 1);
        assert_eq!(res.trades[0].reason, ExitReason::StopLoss);
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
        let fall = vec![
            c(0, 100.0, 100.0, 90.0, 90.0),
            c(1, 100.0, 101.0, 99.0, 95.0),
            c(2, 95.0, 96.0, 90.0, 90.0),
            c(3, 90.0, 91.0, 85.0, 85.0),
        ];
        let strat = short_strategy(entry_close_lt(95.0), None, None, None);
        let res = backtest_strategy(&strat, &fall, &zero_cost()).unwrap();
        assert_eq!(res.trades.len(), 1);
        let t = &res.trades[0];
        assert_eq!(t.side, Side::Short);
        assert!(
            t.return_pct > 0.0,
            "short falling must gain, got {}",
            t.return_pct
        );

        let rise = vec![
            c(0, 100.0, 100.0, 90.0, 90.0),
            c(1, 100.0, 101.0, 99.0, 95.0),
            c(2, 100.0, 110.0, 99.0, 108.0),
            c(3, 108.0, 109.0, 107.0, 108.0),
        ];
        let strat_sl = short_strategy(entry_close_lt(95.0), None, Some(2.0), None);
        let res2 = backtest_strategy(&strat_sl, &rise, &zero_cost()).unwrap();
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
        let res = backtest_strategy(&bad, &candles, &zero_cost());
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
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
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
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
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
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
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
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
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
        let closes = [100.0, 85.0, 100.0, 100.0];
        let candles: Vec<Candle> = closes
            .iter()
            .enumerate()
            .map(|(i, cl)| c(i as i64, *cl, *cl, *cl, *cl))
            .collect();
        let strat = long_strategy(entry.clone(), None, None, None);
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
        assert_eq!(
            res.trades.len(),
            1,
            "expected one trade, got {:?}",
            res.trades
        );
        assert_eq!(res.trades[0].entry_idx, 2);
        let flat_closes = [100.0, 100.0, 100.0, 100.0];
        let flat: Vec<Candle> = flat_closes
            .iter()
            .enumerate()
            .map(|(i, cl)| c(i as i64, *cl, *cl, *cl, *cl))
            .collect();
        let strat2 = long_strategy(entry, None, None, None);
        let res2 = backtest_strategy(&strat2, &flat, &zero_cost()).unwrap();
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
        let res = backtest_strategy(&strat, &candles, &cfg).unwrap();
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
        let full = backtest_strategy(&strat, &candles, &cfg).unwrap();
        let trunc = backtest_strategy(&strat, &candles[..25], &cfg).unwrap();
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
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
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

#[cfg(test)]
mod orb_recon_tests {
    use crate::backtest::ExitReason;
    use crate::backtest::{Bracket, RunConfig};
    use crate::instrument::Instrument;
    use crate::session::{SessionFilter, MS_PER_MIN};
    use crate::sizing::Sizing;
    use crate::stats::{bars_per_year, compute_stats, compute_v2};
    use crate::strategy::{Orb, OrbConfig};
    use crate::types::Candle;

    fn c(ts: i64, o: f64, h: f64, l: f64, cl: f64) -> Candle {
        Candle {
            ts,
            open: o,
            high: h,
            low: l,
            close: cl,
        }
    }

    #[test]
    fn orb_run_reconciles_by_hand() {
        let candles = vec![
            c(0, 100.0, 101.0, 99.0, 100.0),
            c(MS_PER_MIN, 100.0, 102.0, 99.5, 101.0),
            c(2 * MS_PER_MIN, 101.0, 103.0, 100.0, 102.5),
            c(3 * MS_PER_MIN, 102.5, 104.0, 102.0, 103.5),
            c(4 * MS_PER_MIN, 103.5, 107.0, 103.0, 106.0),
        ];
        let mut orb = Orb::new(OrbConfig::new(2, 0.0, 1).expect("valid"));
        let inst = Instrument::new("TEST", 0.01, 1.0, 0.0, 0.0).expect("valid");
        let bracket = Bracket {
            stop_pct: Some(2.0),
            target_pct: Some(4.0),
            trailing_pct: None,
        };
        let session = SessionFilter::all_day();
        let cfg = RunConfig::new(10_000.0, false).expect("valid");

        let result = crate::backtest::run(
            &mut orb,
            &candles,
            &inst,
            Sizing::FixedQty(1.0),
            &bracket,
            &session,
            &cfg,
        )
        .expect("run");

        let rng = orb.range().expect("range");
        assert!((rng.0 - 102.0).abs() < 1e-9);
        assert!((rng.1 - 99.0).abs() < 1e-9);

        assert_eq!(result.trades.len(), 1);
        let t = &result.trades[0];
        assert_eq!(t.entry_idx, 3);
        assert_eq!(t.exit_idx, 4);
        assert!((t.entry_price - 102.5).abs() < 1e-9);
        assert!((t.exit_price - 106.6).abs() < 1e-9);
        assert_eq!(t.reason, ExitReason::TakeProfit);
        assert!((t.return_pct - 0.041).abs() < 1e-9);

        let expected = [10_000.0, 10_000.0, 10_000.0, 10_001.0, 10_004.1];
        assert_eq!(result.equity_curve.len(), expected.len());
        for (i, (got, want)) in result.equity_curve.iter().zip(expected.iter()).enumerate() {
            assert!(
                (got - want).abs() < 1e-9,
                "equity[{i}]: got {got}, want {want}"
            );
        }

        let bpy = bars_per_year("1m").expect("1m");
        let s = compute_stats(&result, &candles, bpy);
        assert_eq!(s.trades, 1);
        assert_eq!(s.wins, 1);
        assert!((s.total_return_pct - 0.041).abs() < 1e-9);
        assert!((s.max_drawdown_pct - 0.0).abs() < 1e-12);

        let m = compute_v2(&result, &candles, bpy);
        assert_eq!(m.max_win_streak, 1);
        assert_eq!(m.max_loss_streak, 0);
        assert!(m.drawdown_periods.is_empty());
        assert!((m.calmar - 0.0).abs() < 1e-12);
        assert!((m.sortino - 0.0).abs() < 1e-12);
        assert_eq!(m.monthly.len(), 1);
        assert!((m.monthly["1970-01"] - 0.041).abs() < 1e-9);
        assert!((m.by_weekday["Thu"] - 0.041).abs() < 1e-9);
    }

    #[test]
    fn golden_bracket_orders_via_resolve_exit() {
        use crate::types::Candle;
        let candles = vec![
            Candle {
                ts: 0,
                open: 100.0,
                high: 100.0,
                low: 100.0,
                close: 100.0,
            },
            Candle {
                ts: crate::session::MS_PER_MIN,
                open: 100.0,
                high: 101.0,
                low: 97.0,
                close: 100.0,
            },
            Candle {
                ts: 2 * crate::session::MS_PER_MIN,
                open: 100.0,
                high: 100.0,
                low: 100.0,
                close: 100.0,
            },
        ];
        struct Sig2(Vec<Option<crate::strategy::Side>>);
        impl crate::strategy::BarStrategy for Sig2 {
            fn on_bar(&mut self, idx: usize, _candles: &[Candle]) -> Option<crate::strategy::Side> {
                self.0.get(idx).copied().flatten()
            }
        }
        let mut s = Sig2(vec![Some(crate::strategy::Side::Long), None, None]);
        let b = Bracket {
            stop_pct: Some(2.0),
            target_pct: None,
            trailing_pct: None,
        };
        let inst2 = Instrument::new("TEST", 0.01, 1.0, 0.0, 0.0).expect("valid");
        let r = crate::backtest::run(
            &mut s,
            &candles,
            &inst2,
            Sizing::FixedQty(1.0),
            &b,
            &SessionFilter::all_day(),
            &RunConfig::new(10000.0, false).unwrap(),
        )
        .unwrap();
        assert_eq!(r.trades.len(), 1);
        assert_eq!(r.trades[0].entry_idx, 1);
        assert_eq!(r.trades[0].exit_idx, 1);
        assert!((r.trades[0].exit_price - 98.0).abs() < 1e-9);
        assert_eq!(r.trades[0].reason, ExitReason::StopLoss);
        assert!((r.equity_curve[1] - 9_998.0).abs() < 1e-9);
    }
}
