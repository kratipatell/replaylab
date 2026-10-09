use crate::backtest::{BacktestResult, ExitReason, Trade};
use crate::instrument::Instrument;
use crate::session::SessionFilter;
use crate::sizing::{quantity, Sizing};
use crate::strategies::Strategy;
use crate::strategy::Side;
use crate::types::Candle;

/// Fixed bracket + trailing-stop definition (percentages of entry price).
///
/// * `stop_pct` – fixed stop distance, e.g. `2.0` = 2%.
/// * `target_pct` – fixed target distance.
/// * `trailing_pct` – ratcheting stop: long trails `peak * (1 - t%)`,
///   short trails `trough * (1 + t%)`. When both fixed and trailing stops
///   exist the tighter one governs and the reason reports which fired.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bracket {
    pub stop_pct: Option<f64>,
    pub target_pct: Option<f64>,
    pub trailing_pct: Option<f64>,
}

impl Bracket {
    pub fn validate(&self) -> Result<(), String> {
        for (name, v) in [
            ("stop_pct", self.stop_pct),
            ("target_pct", self.target_pct),
            ("trailing_pct", self.trailing_pct),
        ] {
            if let Some(x) = v {
                if !x.is_finite() || x <= 0.0 {
                    return Err(format!("{name} must be > 0, got {x}"));
                }
            }
        }
        Ok(())
    }
}

/// Runner options.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunConfig {
    pub initial_equity: f64,
    /// Close open positions at the close of the last in-session bar before
    /// a session gap (reason `SessionEnd`).
    pub exit_at_session_end: bool,
}

impl RunConfig {
    pub fn new(initial_equity: f64, exit_at_session_end: bool) -> Result<RunConfig, String> {
        if !initial_equity.is_finite() || initial_equity <= 0.0 {
            return Err(format!("initial_equity must be > 0, got {initial_equity}"));
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

/// Run a `Strategy` over `candles` with brackets, trailing stops and an
/// optional session filter.
///
/// Conventions (match v1 where they overlap):
/// * signals fire at the **close** of bar `i`, fill at the **open** of
///   `i + 1` rounded to the instrument tick; the fill bar must be in session
///   or the signal is dropped;
/// * one position at a time; bracket checks run on the fill bar itself;
/// * when stop and target both trigger intrabar without a gap, the stop
///   wins; gaps fill at the open;
/// * equity is dollar-based: `pnl = sign * qty * point_value * (exit - entry)`
///   minus per-side `cost_rate * qty * point_value * price`.
#[allow(clippy::too_many_arguments)]
pub fn run(
    strategy: &mut dyn Strategy,
    candles: &[Candle],
    instrument: &Instrument,
    sizing: Sizing,
    bracket: &Bracket,
    session: &SessionFilter,
    cfg: &RunConfig,
) -> Result<BacktestResult, String> {
    bracket.validate()?;
    if !cfg.initial_equity.is_finite() || cfg.initial_equity <= 0.0 {
        return Err(format!(
            "initial_equity must be > 0, got {}",
            cfg.initial_equity
        ));
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
        // (1) fill pending entry at this open.
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
                    });
                    strategy.on_fill(i);
                }
            }
        }

        // (2) bracket / trailing check on this bar.
        //
        // The open gaps against the stop as it stood coming INTO the bar;
        // the trailing stop only ratchets on this bar's excursion, so the
        // intrabar touch uses the ratcheted level. (Otherwise a bar that
        // makes a new high would stop itself out on its own open.)
        if let Some(pos) = position.take() {
            let (fixed_stop, target, base_trail) = levels(pos.side, pos.entry_raw, bracket);
            let incoming_trail = base_trail;
            // Ratchet the trailing stop off this bar's excursion.
            let mut peak = pos.peak;
            let mut trail = incoming_trail;
            if let Some(pct) = bracket.trailing_pct {
                let t = pct / 100.0;
                match pos.side {
                    Side::Long => {
                        if candles[i].high > peak {
                            peak = candles[i].high;
                        }
                        let candidate = peak * (1.0 - t);
                        trail = Some(trail.map_or(candidate, |old| old.max(candidate)));
                    }
                    Side::Short => {
                        if candles[i].low < peak {
                            peak = candles[i].low;
                        }
                        let candidate = peak * (1.0 + t);
                        trail = Some(trail.map_or(candidate, |old| old.min(candidate)));
                    }
                }
            }
            let o = candles[i].open;
            let h = candles[i].high;
            let l = candles[i].low;
            // Tighter-of helper: fixed vs trailing for one probe level.
            let tighter = |f: Option<f64>, t: Option<f64>| -> (Option<f64>, bool) {
                match (f, t) {
                    (Some(f), Some(t)) => match pos.side {
                        Side::Long => {
                            if t >= f {
                                (Some(t), true)
                            } else {
                                (Some(f), false)
                            }
                        }
                        Side::Short => {
                            if t <= f {
                                (Some(t), true)
                            } else {
                                (Some(f), false)
                            }
                        }
                    },
                    (Some(f), None) => (Some(f), false),
                    (None, Some(t)) => (Some(t), true),
                    (None, None) => (None, false),
                }
            };
            let (gap_stop, gap_is_trail) = tighter(fixed_stop, incoming_trail);
            let (stop_level, stop_is_trail) = tighter(fixed_stop, trail);
            let (sl_hit, tp_hit) = match pos.side {
                Side::Long => (
                    stop_level.map(|s| l <= s).unwrap_or(false)
                        || gap_stop.map(|s| o <= s).unwrap_or(false),
                    target.map(|t| h >= t || o >= t).unwrap_or(false),
                ),
                Side::Short => (
                    stop_level.map(|s| h >= s).unwrap_or(false)
                        || gap_stop.map(|s| o >= s).unwrap_or(false),
                    target.map(|t| l <= t || o <= t).unwrap_or(false),
                ),
            };
            let exit: Option<(f64, ExitReason)> = if sl_hit && tp_hit {
                // Gap direction decides; both touched intrabar -> stop wins.
                let gap_up = match pos.side {
                    Side::Long => o >= target.unwrap(),
                    Side::Short => o <= target.unwrap(),
                };
                if gap_up {
                    Some((o, ExitReason::TakeProfit))
                } else {
                    let gap_hit = gap_stop.map(|g| match pos.side {
                        Side::Long => o <= g,
                        Side::Short => o >= g,
                    });
                    if gap_hit.unwrap_or(false) {
                        let reason = if gap_is_trail {
                            ExitReason::Trailing
                        } else {
                            ExitReason::StopLoss
                        };
                        Some((o, reason))
                    } else {
                        let s = stop_level.unwrap();
                        let reason = if stop_is_trail {
                            ExitReason::Trailing
                        } else {
                            ExitReason::StopLoss
                        };
                        Some((s, reason))
                    }
                }
            } else if sl_hit {
                let gap_hit = gap_stop.map(|g| match pos.side {
                    Side::Long => o <= g,
                    Side::Short => o >= g,
                });
                if gap_hit.unwrap_or(false) {
                    let reason = if gap_is_trail {
                        ExitReason::Trailing
                    } else {
                        ExitReason::StopLoss
                    };
                    Some((o, reason))
                } else {
                    let s = stop_level.unwrap();
                    let reason = if stop_is_trail {
                        ExitReason::Trailing
                    } else {
                        ExitReason::StopLoss
                    };
                    Some((s, reason))
                }
            } else if tp_hit {
                let t = target.unwrap();
                let raw = match pos.side {
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
            if let Some((raw_exit, reason)) = exit {
                let (after, trade) =
                    close_position(&pos, raw_exit, candles[i].ts, i, reason, equity);
                equity = after;
                trades.push(trade);
            } else {
                position = Some(OpenPos { peak, ..pos });
                // (2b) session time-stop at the last in-session bar.
                if cfg.exit_at_session_end
                    && session.contains(candles[i].ts)
                    && (i + 1 == n || !session.contains(candles[i + 1].ts))
                {
                    let pos = position.take().unwrap();
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

        // (3) at the close, ask the strategy and schedule for i+1.
        if position.is_none() && pending.is_none() && session.contains(candles[i].ts) && i + 1 < n {
            if let Some(side) = strategy.on_bar(i, candles) {
                pending = Some((side, i));
            }
        }

        // (4) mark equity at the close.
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

    impl Strategy for Sig {
        fn on_bar(&mut self, idx: usize, _candles: &[Candle]) -> Option<Side> {
            self.0.get(idx).copied().flatten()
        }
    }

    #[test]
    fn stop_loss_fires_at_level() {
        // Signal at close 0 -> fill open 100 at idx 1; stop 2% -> 98.
        // Bar 1 low 97 touches -> exit 98.
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
        // Equity: 10_000 + (98-100) = 9_998.
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
        // Entry 100. Bar 1 high 110 (low 109, no touch) -> trail 108.9.
        // Bar 2 low 108 -> exit 108.9.
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
        // Session 09:30-16:00; signal bar at 08:00 must not fill.
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
