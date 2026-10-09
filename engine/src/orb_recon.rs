//! ORB end-to-end reconciliation (Engine v2 Phase 1 gate).
//!
//! Hand-worked tape (UTC 1970-01-01, 1-minute bars, tick 0.01, zero costs,
//! equity 10_000, `FixedQty(1)`):
//!
//! ```text
//! bar0 00:00 o100   h101 l99   c100    } range forms over bars 0..1:
//! bar1 00:01 o100   h102 l99.5 c101    } high 102, low 99
//! bar2 00:02 o101   h103 l100  c102.5  -> close 102.5 > 102: LONG signal
//! bar3 00:03 o102.5 h104 l102  c103.5  -> fill 102.5; stop 2% = 100.45,
//!                                        target 4% = 106.6; no touch
//! bar4 00:04 o103.5 h107 l103  c106    -> high 107 >= 106.6: TP exit 106.6
//! ```
//!
//! Expected: one trade, entry (idx 3 @ 102.5) -> exit (idx 4 @ 106.6),
//! `pnl = 4.1`, equity `10_004.1`, `return_pct = 0.041%`.
//! Equity curve: `[10000, 10000, 10000, 10001.0, 10004.1]`.

#[cfg(test)]
mod tests {
    use crate::backtest::ExitReason;
    use crate::backtest_v2::{Bracket, RunConfig};
    use crate::instrument::Instrument;
    use crate::session::{SessionFilter, MS_PER_MIN};
    use crate::sizing::Sizing;
    use crate::stats::{bars_per_year, compute_stats, compute_v2};
    use crate::strategies::{Orb, OrbConfig};
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

        let result = crate::backtest_v2::run(
            &mut orb,
            &candles,
            &inst,
            Sizing::FixedQty(1.0),
            &bracket,
            &session,
            &cfg,
        )
        .expect("run");

        // Range formed from bars 0..1.
        assert_eq!(orb.range(), Some((102.0, 99.0)));

        // One trade, exact fills.
        assert_eq!(result.trades.len(), 1);
        let t = &result.trades[0];
        assert_eq!(t.entry_idx, 3);
        assert_eq!(t.exit_idx, 4);
        assert!((t.entry_price - 102.5).abs() < 1e-9);
        assert!((t.exit_price - 106.6).abs() < 1e-9);
        assert_eq!(t.reason, ExitReason::TakeProfit);
        assert!((t.return_pct - 0.041).abs() < 1e-9);

        // Equity curve reconciles bar by bar.
        let expected = [10_000.0, 10_000.0, 10_000.0, 10_001.0, 10_004.1];
        assert_eq!(result.equity_curve.len(), expected.len());
        for (i, (got, want)) in result.equity_curve.iter().zip(expected.iter()).enumerate() {
            assert!(
                (got - want).abs() < 1e-9,
                "equity[{i}]: got {got}, want {want}"
            );
        }

        // v1 stats reconcile.
        let bpy = bars_per_year("1m").expect("1m");
        let s = compute_stats(&result, &candles, bpy);
        assert_eq!(s.trades, 1);
        assert_eq!(s.wins, 1);
        assert!((s.total_return_pct - 0.041).abs() < 1e-9);
        assert!((s.max_drawdown_pct - 0.0).abs() < 1e-12);

        // v2 metrics reconcile: 1-win streak, no drawdown, single bucket.
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
}
