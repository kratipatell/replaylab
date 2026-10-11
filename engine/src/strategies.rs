use crate::session::day_id;
use crate::strategy::Side;
use crate::types::Candle;

/// Bar-by-bar signal interface for Engine v2.
///
/// The legacy JSON rule engine (`crate::strategy::Strategy`) stays for the
/// HTTP API; new strategies implement this trait and run in
/// `crate::backtest_v2`. Signals fire at the **close** of bar `idx` and are
/// filled at the open of `idx + 1` (same convention as v1).
pub trait Strategy {
    /// Return `Some(side)` to enter at the next open, `None` to stay flat.
    ///
    /// `candles[..=idx]` is visible; future bars must not be read.
    fn on_bar(&mut self, idx: usize, candles: &[Candle]) -> Option<Side>;

    /// Return `true` to exit at the next open, `false` to hold.
    ///
    /// Called at the close of bar `idx` while a position is open, after that
    /// bar's stop/target checks left the position open. Default never exits.
    fn exit_on_bar(&mut self, _idx: usize, _candles: &[Candle]) -> bool {
        false
    }

    /// Called by the runner after a fill so one-trade-per-day strategies
    /// can suppress further signals. Default does nothing.
    fn on_fill(&mut self, _idx: usize) {}
}

/// Opening-range breakout configuration.
///
/// The range is the high/low of the first `range_bars` bars of each UTC day.
/// After the range forms, a close above `high * (1 + buffer)` signals long,
/// a close below `low * (1 - buffer)` signals short.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbConfig {
    pub range_bars: usize,
    pub buffer_pct: f64,
    pub max_trades_per_day: usize,
}

impl OrbConfig {
    pub fn new(
        range_bars: usize,
        buffer_pct: f64,
        max_trades_per_day: usize,
    ) -> Result<OrbConfig, String> {
        if range_bars == 0 {
            return Err("range_bars must be > 0".to_string());
        }
        if !buffer_pct.is_finite() || buffer_pct < 0.0 {
            return Err(format!("buffer_pct must be >= 0, got {buffer_pct}"));
        }
        if max_trades_per_day == 0 {
            return Err("max_trades_per_day must be > 0".to_string());
        }
        Ok(OrbConfig {
            range_bars,
            buffer_pct,
            max_trades_per_day,
        })
    }
}

/// Opening-range breakout strategy.
#[derive(Debug, Clone)]
pub struct Orb {
    cfg: OrbConfig,
    day: Option<i64>,
    day_count: usize,
    range_high: f64,
    range_low: f64,
    range_ready: bool,
    traded: usize,
}

impl Orb {
    pub fn new(cfg: OrbConfig) -> Orb {
        Orb {
            cfg,
            day: None,
            day_count: 0,
            range_high: f64::NEG_INFINITY,
            range_low: f64::INFINITY,
            range_ready: false,
            traded: 0,
        }
    }

    /// Current day's range if formed: `(high, low)`.
    pub fn range(&self) -> Option<(f64, f64)> {
        if self.range_ready {
            Some((self.range_high, self.range_low))
        } else {
            None
        }
    }
}

impl Strategy for Orb {
    fn on_bar(&mut self, idx: usize, candles: &[Candle]) -> Option<Side> {
        if idx >= candles.len() {
            return None;
        }
        let day = day_id(candles[idx].ts);
        if self.day != Some(day) {
            self.day = Some(day);
            self.day_count = 0;
            self.range_high = f64::NEG_INFINITY;
            self.range_low = f64::INFINITY;
            self.range_ready = false;
            self.traded = 0;
        }
        self.day_count += 1;
        if !self.range_ready {
            let c = &candles[idx];
            if c.high > self.range_high {
                self.range_high = c.high;
            }
            if c.low < self.range_low {
                self.range_low = c.low;
            }
            if self.day_count >= self.cfg.range_bars {
                self.range_ready = true;
            }
            return None;
        }
        if self.traded >= self.cfg.max_trades_per_day {
            return None;
        }
        let c = &candles[idx];
        let up = self.range_high * (1.0 + self.cfg.buffer_pct / 100.0);
        let dn = self.range_low * (1.0 - self.cfg.buffer_pct / 100.0);
        if c.close > up {
            Some(Side::Long)
        } else if c.close < dn {
            Some(Side::Short)
        } else {
            None
        }
    }

    fn on_fill(&mut self, _idx: usize) {
        self.traded += 1;
    }
}

/// JSON-rules strategy for Engine v2.
///
/// Mirrors v1 rule evaluation so the same schema runs on both engines.
#[derive(Debug, Clone)]
pub struct RulesStrategy {
    side: crate::strategy::Side,
    entry: crate::strategy::Rules,
    exit: Option<crate::strategy::Rules>,
    armed: bool,
    was_open: bool,
}

impl RulesStrategy {
    pub fn new(spec: &crate::strategy::Strategy) -> Self {
        RulesStrategy {
            side: spec.side,
            entry: spec.entry.clone(),
            exit: spec.exit.clone(),
            armed: true,
            was_open: false,
        }
    }

    fn closes_prefix(candles: &[crate::types::Candle], idx: usize) -> Vec<f64> {
        let mut out = Vec::with_capacity(idx + 1);
        let mut i = 0;
        while i <= idx {
            match candles.get(i) {
                Some(c) => out.push(c.close),
                None => break,
            }
            i += 1;
        }
        out
    }

    fn entry_true(&self, idx: usize, candles: &[crate::types::Candle]) -> bool {
        if idx >= candles.len() {
            return false;
        }
        let closes = Self::closes_prefix(candles, idx);
        rules_eval_rules(&self.entry, idx, candles, &closes)
    }

    fn exit_true(&self, idx: usize, candles: &[crate::types::Candle]) -> bool {
        let rules = match &self.exit {
            Some(r) => r,
            None => return false,
        };
        if idx >= candles.len() {
            return false;
        }
        let closes = Self::closes_prefix(candles, idx);
        rules_eval_rules(rules, idx, candles, &closes)
    }
}

fn rules_series(
    ind: crate::strategy::IndicatorKind,
    period: Option<usize>,
    closes: &[f64],
) -> Vec<f64> {
    let p = match period {
        Some(v) => v,
        None => return Vec::new(),
    };
    match ind {
        crate::strategy::IndicatorKind::Sma => crate::indicators::sma(closes, p),
        crate::strategy::IndicatorKind::Ema => crate::indicators::ema(closes, p),
        crate::strategy::IndicatorKind::Rsi => crate::indicators::rsi(closes, p),
        crate::strategy::IndicatorKind::Open
        | crate::strategy::IndicatorKind::High
        | crate::strategy::IndicatorKind::Low
        | crate::strategy::IndicatorKind::Close => Vec::new(),
    }
}

fn rules_operand(
    op: &crate::strategy::Operand,
    idx: usize,
    candles: &[crate::types::Candle],
    closes: &[f64],
) -> f64 {
    match op {
        crate::strategy::Operand::Number(v) => *v,
        crate::strategy::Operand::Indicator(r) => match r.ind {
            crate::strategy::IndicatorKind::Open => match candles.get(idx) {
                Some(c) => c.open,
                None => f64::NAN,
            },
            crate::strategy::IndicatorKind::High => match candles.get(idx) {
                Some(c) => c.high,
                None => f64::NAN,
            },
            crate::strategy::IndicatorKind::Low => match candles.get(idx) {
                Some(c) => c.low,
                None => f64::NAN,
            },
            crate::strategy::IndicatorKind::Close => match candles.get(idx) {
                Some(c) => c.close,
                None => f64::NAN,
            },
            crate::strategy::IndicatorKind::Sma
            | crate::strategy::IndicatorKind::Ema
            | crate::strategy::IndicatorKind::Rsi => {
                let s = rules_series(r.ind, r.period, closes);
                match s.get(idx) {
                    Some(v) => *v,
                    None => f64::NAN,
                }
            }
        },
    }
}

fn rules_eval_condition(
    c: &crate::strategy::Condition,
    idx: usize,
    candles: &[crate::types::Candle],
    closes: &[f64],
) -> bool {
    let l = rules_operand(&c.left, idx, candles, closes);
    let r = rules_operand(&c.right, idx, candles, closes);
    if l.is_nan() || r.is_nan() {
        return false;
    }
    match c.op {
        crate::strategy::Op::Lt => l < r,
        crate::strategy::Op::Gt => l > r,
        crate::strategy::Op::CrossesAbove => {
            if idx == 0 {
                return false;
            }
            let lp = rules_operand(&c.left, idx - 1, candles, closes);
            let rp = rules_operand(&c.right, idx - 1, candles, closes);
            if lp.is_nan() || rp.is_nan() {
                return false;
            }
            lp <= rp && l > r
        }
        crate::strategy::Op::CrossesBelow => {
            if idx == 0 {
                return false;
            }
            let lp = rules_operand(&c.left, idx - 1, candles, closes);
            let rp = rules_operand(&c.right, idx - 1, candles, closes);
            if lp.is_nan() || rp.is_nan() {
                return false;
            }
            lp >= rp && l < r
        }
    }
}

fn rules_eval_rules(
    rules: &crate::strategy::Rules,
    idx: usize,
    candles: &[crate::types::Candle],
    closes: &[f64],
) -> bool {
    if rules.all.is_empty() && rules.any.is_empty() {
        return false;
    }
    for c in &rules.all {
        if !rules_eval_condition(c, idx, candles, closes) {
            return false;
        }
    }
    if rules.any.is_empty() {
        return true;
    }
    for c in &rules.any {
        if rules_eval_condition(c, idx, candles, closes) {
            return true;
        }
    }
    false
}

impl Strategy for RulesStrategy {
    fn on_bar(
        &mut self,
        idx: usize,
        candles: &[crate::types::Candle],
    ) -> Option<crate::strategy::Side> {
        if idx >= candles.len() {
            return None;
        }
        let e = self.entry_true(idx, candles);
        if self.was_open {
            self.was_open = false;
            self.armed = false;
            if !e {
                self.armed = true;
            }
        } else if !e {
            self.armed = true;
        }
        if self.armed && e {
            Some(self.side)
        } else {
            None
        }
    }

    fn exit_on_bar(&mut self, idx: usize, candles: &[crate::types::Candle]) -> bool {
        if idx >= candles.len() {
            return false;
        }
        self.was_open = true;
        if !self.entry_true(idx, candles) {
            self.armed = true;
        }
        self.exit_true(idx, candles)
    }

    fn on_fill(&mut self, _idx: usize) {
        self.was_open = true;
    }
}

/// Build the v2 runner inputs matching v1 semantics for a JSON strategy.
pub fn rules_v2_setup(
    spec: &crate::strategy::Strategy,
    cfg: &crate::backtest::BacktestConfig,
) -> Result<
    (
        crate::instrument::Instrument,
        crate::sizing::Sizing,
        crate::backtest_v2::Bracket,
        crate::session::SessionFilter,
        crate::backtest_v2::RunConfig,
    ),
    String,
> {
    // Tiny tick so round_to_tick is a no-op within tolerance; much smaller
    // would overflow price / tick, larger would quantize test prices.
    let instrument = crate::instrument::Instrument::new(
        &spec.asset,
        0.000000001,
        1.0,
        cfg.fee_bps,
        cfg.slippage_bps,
    )?;
    // risk 100% with a 100% stop sizes qty = equity / price, i.e. full equity.
    let sizing = crate::sizing::Sizing::RiskPct {
        risk_pct: 100.0,
        stop_pct: 100.0,
    };
    let bracket = crate::backtest_v2::Bracket {
        stop_pct: spec.stop_loss_pct,
        target_pct: spec.take_profit_pct,
        trailing_pct: None,
    };
    let session = crate::session::SessionFilter::all_day();
    let run = crate::backtest_v2::RunConfig::new(1.0, false)?;
    Ok((instrument, sizing, bracket, session, run))
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

    fn day_bars() -> Vec<Candle> {
        // One UTC day, 1-minute bars from 00:00.
        vec![
            c(0, 100.0, 101.0, 99.0, 100.0),
            c(MS_PER_MIN, 100.0, 102.0, 99.5, 101.0),
            c(2 * MS_PER_MIN, 101.0, 103.0, 100.0, 102.5),
            c(3 * MS_PER_MIN, 102.5, 103.0, 101.0, 98.0),
        ]
    }

    #[test]
    fn orb_range_and_long_breakout() {
        let cfg = OrbConfig::new(2, 0.0, 1).expect("valid");
        let mut orb = Orb::new(cfg);
        let bars = day_bars();
        assert_eq!(orb.on_bar(0, &bars), None);
        assert_eq!(orb.on_bar(1, &bars), None);
        // Range = high 102, low 99.
        assert_eq!(orb.range(), Some((102.0, 99.0)));
        // Bar 2 close 102.5 > 102 -> long.
        assert_eq!(orb.on_bar(2, &bars), Some(Side::Long));
        orb.on_fill(3);
        // One per day: suppressed afterwards even though bar 3 breaks down.
        assert_eq!(orb.on_bar(3, &bars), None);
    }

    #[test]
    fn orb_short_breakout_and_new_day_resets() {
        let cfg = OrbConfig::new(1, 0.0, 1).expect("valid");
        let mut orb = Orb::new(cfg);
        let mut bars = day_bars();
        assert_eq!(orb.on_bar(0, &bars), None);
        assert_eq!(orb.range(), Some((101.0, 99.0)));
        // Bar 1 close 101 is inside -> none.
        assert_eq!(orb.on_bar(1, &bars), None);
        // Next UTC day resets the range.
        bars.push(c(crate::session::MS_PER_DAY, 50.0, 51.0, 49.0, 50.0));
        let idx = bars.len() - 1;
        assert_eq!(orb.on_bar(idx, &bars), None);
        assert_eq!(orb.range(), Some((51.0, 49.0)));
    }

    #[test]
    fn orb_rejects_bad_config() {
        assert!(OrbConfig::new(0, 0.0, 1).is_err());
        assert!(OrbConfig::new(2, -1.0, 1).is_err());
        assert!(OrbConfig::new(2, 0.0, 0).is_err());
    }
}

#[cfg(test)]
mod rules_parity_tests {
    use super::{rules_v2_setup, RulesStrategy};
    use crate::backtest::{backtest, BacktestConfig, BacktestResult};
    use crate::strategy::{Condition, IndicatorKind, IndicatorRef, Op, Operand, Rules, Side};
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

    fn ind(ind: IndicatorKind, period: Option<usize>) -> Operand {
        Operand::Indicator(IndicatorRef { ind, period })
    }

    fn num(v: f64) -> Operand {
        Operand::Number(v)
    }

    fn cond(left: Operand, op: Op, right: Operand) -> Condition {
        Condition { left, op, right }
    }

    fn mk_spec(
        side: Side,
        entry: Rules,
        exit: Option<Rules>,
        sl: Option<f64>,
        tp: Option<f64>,
    ) -> crate::strategy::Strategy {
        crate::strategy::Strategy {
            asset: "TEST".to_string(),
            timeframe: "1h".to_string(),
            side,
            entry,
            exit,
            stop_loss_pct: sl,
            take_profit_pct: tp,
            claim: None,
        }
    }

    fn gt_close(th: f64) -> Rules {
        Rules {
            all: vec![cond(ind(IndicatorKind::Close, None), Op::Gt, num(th))],
            any: vec![],
        }
    }

    fn lt_close(th: f64) -> Rules {
        Rules {
            all: vec![cond(ind(IndicatorKind::Close, None), Op::Lt, num(th))],
            any: vec![],
        }
    }

    fn zero_cost() -> BacktestConfig {
        BacktestConfig {
            fee_bps: 0.0,
            slippage_bps: 0.0,
        }
    }

    fn costly() -> BacktestConfig {
        BacktestConfig {
            fee_bps: 5.0,
            slippage_bps: 2.0,
        }
    }

    fn run_both(
        spec: &crate::strategy::Strategy,
        candles: &[Candle],
        cfg: &BacktestConfig,
    ) -> (BacktestResult, BacktestResult) {
        let v1 = backtest(spec, candles, cfg).expect("v1 backtest valid");
        let (inst, sizing, bracket, session, run_cfg) =
            rules_v2_setup(spec, cfg).expect("v2 setup valid");
        let mut rs = RulesStrategy::new(spec);
        let v2 = crate::backtest_v2::run(
            &mut rs, candles, &inst, sizing, &bracket, &session, &run_cfg,
        )
        .expect("v2 run valid");
        (v1, v2)
    }

    fn assert_zero(v1: &BacktestResult, v2: &BacktestResult) {
        assert_eq!(
            v1.trades.len(),
            v2.trades.len(),
            "trade count: v1 {:?} vs v2 {:?}",
            v1.trades,
            v2.trades
        );
        for (a, b) in v1.trades.iter().zip(v2.trades.iter()) {
            assert_eq!(a.entry_idx, b.entry_idx, "entry_idx {a:?} vs {b:?}");
            assert_eq!(a.exit_idx, b.exit_idx, "exit_idx {a:?} vs {b:?}");
            assert_eq!(a.side, b.side);
            assert_eq!(a.reason, b.reason, "reason {a:?} vs {b:?}");
            assert!(
                (a.entry_price - b.entry_price).abs() <= 1e-9,
                "entry_price {} vs {}",
                a.entry_price,
                b.entry_price
            );
            assert!(
                (a.exit_price - b.exit_price).abs() <= 1e-9,
                "exit_price {} vs {}",
                a.exit_price,
                b.exit_price
            );
            assert!(
                (a.return_pct - b.return_pct).abs() <= 1e-9,
                "return_pct {} vs {}",
                a.return_pct,
                b.return_pct
            );
        }
        assert_eq!(v1.equity_curve.len(), v2.equity_curve.len());
        for (i, (a, b)) in v1
            .equity_curve
            .iter()
            .zip(v2.equity_curve.iter())
            .enumerate()
        {
            assert!((a - b).abs() <= 1e-9, "equity[{i}] {a} vs {b}");
        }
    }

    fn assert_costly(v1: &BacktestResult, v2: &BacktestResult) {
        // v1 folds costs into prices multiplicatively while v2 charges them
        // additively per fill, so prices differ by design; only ids, reasons,
        // returns and equity are comparable within tolerance.
        assert_eq!(
            v1.trades.len(),
            v2.trades.len(),
            "trade count: v1 {:?} vs v2 {:?}",
            v1.trades,
            v2.trades
        );
        for (a, b) in v1.trades.iter().zip(v2.trades.iter()) {
            assert_eq!(a.entry_idx, b.entry_idx, "entry_idx {a:?} vs {b:?}");
            assert_eq!(a.exit_idx, b.exit_idx, "exit_idx {a:?} vs {b:?}");
            assert_eq!(a.side, b.side);
            assert_eq!(a.reason, b.reason, "reason {a:?} vs {b:?}");
            assert!(
                (a.return_pct - b.return_pct).abs() <= 0.05,
                "return_pct {} vs {}",
                a.return_pct,
                b.return_pct
            );
        }
        assert_eq!(v1.equity_curve.len(), v2.equity_curve.len());
        for (i, (a, b)) in v1
            .equity_curve
            .iter()
            .zip(v2.equity_curve.iter())
            .enumerate()
        {
            let denom = a.abs().max(1e-12);
            assert!((a - b).abs() / denom <= 1e-3, "equity[{i}] {a} vs {b}");
        }
    }

    fn assert_both(spec: &crate::strategy::Strategy, candles: &[Candle]) {
        let (v1, v2) = run_both(spec, candles, &zero_cost());
        assert_zero(&v1, &v2);
        let (v1c, v2c) = run_both(spec, candles, &costly());
        assert_costly(&v1c, &v2c);
    }

    #[test]
    fn parity_simple_threshold_long() {
        let candles = vec![
            c(0, 90.0, 95.0, 85.0, 90.0),
            c(1, 90.0, 115.0, 90.0, 110.0),
            c(2, 200.0, 210.0, 190.0, 205.0),
            c(3, 205.0, 210.0, 200.0, 206.0),
        ];
        let spec = mk_spec(Side::Long, gt_close(100.0), None, None, None);
        let (v1, _) = run_both(&spec, &candles, &zero_cost());
        assert_eq!(v1.trades.len(), 1);
        assert_both(&spec, &candles);
    }

    #[test]
    fn parity_short() {
        let candles = vec![
            c(0, 100.0, 101.0, 99.0, 100.0),
            c(1, 100.0, 101.0, 89.0, 90.0),
            c(2, 90.0, 91.0, 89.0, 90.0),
            c(3, 90.0, 91.0, 84.0, 85.0),
        ];
        let spec = mk_spec(Side::Short, lt_close(95.0), None, None, None);
        let (v1, _) = run_both(&spec, &candles, &zero_cost());
        assert_eq!(v1.trades.len(), 1);
        assert_eq!(v1.trades[0].side, Side::Short);
        assert_both(&spec, &candles);
    }

    #[test]
    fn parity_crossover_with_indicator() {
        let closes = [100.0, 100.0, 100.0, 90.0, 110.0, 110.0];
        let candles: Vec<Candle> = closes
            .iter()
            .enumerate()
            .map(|(i, cl)| {
                let open = if i == 0 { 100.0 } else { closes[i - 1] };
                c(
                    i as i64,
                    open,
                    open.max(*cl) + 0.5,
                    open.min(*cl) - 0.5,
                    *cl,
                )
            })
            .collect();
        let entry = Rules {
            all: vec![cond(
                ind(IndicatorKind::Close, None),
                Op::CrossesAbove,
                ind(IndicatorKind::Sma, Some(3)),
            )],
            any: vec![],
        };
        let spec = mk_spec(Side::Long, entry, None, None, None);
        let (v1, _) = run_both(&spec, &candles, &zero_cost());
        assert_eq!(v1.trades.len(), 1);
        assert_both(&spec, &candles);
    }

    #[test]
    fn parity_all_any_mix() {
        let closes = [100.0, 102.0, 106.0, 106.0];
        let candles: Vec<Candle> = closes
            .iter()
            .enumerate()
            .map(|(i, cl)| c(i as i64, *cl, *cl, *cl, *cl))
            .collect();
        let entry = Rules {
            all: vec![cond(ind(IndicatorKind::Close, None), Op::Gt, num(100.0))],
            any: vec![
                cond(ind(IndicatorKind::Close, None), Op::Gt, num(105.0)),
                cond(ind(IndicatorKind::Close, None), Op::Lt, num(95.0)),
            ],
        };
        let spec = mk_spec(Side::Long, entry, None, None, None);
        let (v1, _) = run_both(&spec, &candles, &zero_cost());
        assert_eq!(v1.trades.len(), 1);
        assert_eq!(v1.trades[0].entry_idx, 3);
        assert_both(&spec, &candles);
    }

    #[test]
    fn parity_stop_loss() {
        let candles = vec![
            c(0, 90.0, 115.0, 90.0, 110.0),
            c(1, 100.0, 101.0, 99.0, 100.0),
            c(2, 100.0, 101.0, 97.0, 100.0),
            c(3, 100.0, 101.0, 99.0, 100.0),
        ];
        let spec = mk_spec(Side::Long, gt_close(100.0), None, Some(2.0), None);
        let (v1, _) = run_both(&spec, &candles, &zero_cost());
        assert!(!v1.trades.is_empty());
        assert_eq!(v1.trades[0].reason, crate::backtest::ExitReason::StopLoss);
        assert_both(&spec, &candles);
    }

    #[test]
    fn parity_take_profit() {
        let candles = vec![
            c(0, 90.0, 115.0, 90.0, 110.0),
            c(1, 100.0, 101.0, 99.0, 100.0),
            c(2, 100.0, 106.0, 99.0, 102.0),
        ];
        let spec = mk_spec(Side::Long, gt_close(100.0), None, None, Some(5.0));
        let (v1, _) = run_both(&spec, &candles, &zero_cost());
        assert_eq!(v1.trades[0].reason, crate::backtest::ExitReason::TakeProfit);
        assert_both(&spec, &candles);
    }

    #[test]
    fn parity_both_hit_stop_wins() {
        let candles = vec![
            c(0, 90.0, 115.0, 90.0, 110.0),
            c(1, 100.0, 101.0, 99.0, 100.0),
            c(2, 100.0, 110.0, 90.0, 100.0),
            c(3, 100.0, 101.0, 99.0, 90.0),
        ];
        let spec = mk_spec(Side::Long, gt_close(100.0), None, Some(2.0), Some(2.0));
        let (v1, _) = run_both(&spec, &candles, &zero_cost());
        assert_eq!(v1.trades[0].reason, crate::backtest::ExitReason::StopLoss);
        assert_both(&spec, &candles);
    }

    #[test]
    fn parity_gap_through_stop() {
        let candles = vec![
            c(0, 90.0, 115.0, 90.0, 110.0),
            c(1, 100.0, 101.0, 99.0, 100.0),
            c(2, 90.0, 95.0, 85.0, 92.0),
            c(3, 92.0, 93.0, 91.0, 90.0),
        ];
        let spec = mk_spec(Side::Long, gt_close(100.0), None, Some(2.0), None);
        let (v1, _) = run_both(&spec, &candles, &zero_cost());
        assert_eq!(v1.trades[0].reason, crate::backtest::ExitReason::StopLoss);
        assert_eq!(v1.trades[0].exit_idx, 2);
        assert_both(&spec, &candles);
    }

    #[test]
    fn parity_gap_beyond_target() {
        let candles = vec![
            c(0, 90.0, 115.0, 90.0, 110.0),
            c(1, 100.0, 101.0, 99.0, 100.0),
            c(2, 105.0, 106.0, 90.0, 95.0),
        ];
        let spec = mk_spec(Side::Long, gt_close(100.0), None, Some(2.0), Some(2.0));
        let (v1, _) = run_both(&spec, &candles, &zero_cost());
        assert_eq!(v1.trades[0].reason, crate::backtest::ExitReason::TakeProfit);
        assert_both(&spec, &candles);
    }

    #[test]
    fn parity_reentry_after_stopout() {
        let candles = vec![
            c(0, 100.0, 110.0, 100.0, 110.0),
            c(1, 100.0, 101.0, 90.0, 110.0),
            c(2, 100.0, 101.0, 99.0, 110.0),
            c(3, 100.0, 101.0, 99.0, 90.0),
            c(4, 100.0, 101.0, 99.0, 110.0),
            c(5, 100.0, 101.0, 99.0, 110.0),
            c(6, 100.0, 101.0, 99.0, 100.0),
        ];
        let spec = mk_spec(Side::Long, gt_close(100.0), None, Some(2.0), None);
        let (v1, _) = run_both(&spec, &candles, &zero_cost());
        assert_eq!(v1.trades.len(), 2);
        assert_both(&spec, &candles);
    }

    #[test]
    fn parity_open_position_at_end() {
        let candles = vec![
            c(0, 90.0, 91.0, 89.0, 90.0),
            c(1, 90.0, 115.0, 90.0, 110.0),
            c(2, 100.0, 101.0, 99.0, 100.0),
        ];
        let spec = mk_spec(Side::Long, gt_close(100.0), None, None, None);
        let (v1, _) = run_both(&spec, &candles, &zero_cost());
        assert_eq!(v1.trades.len(), 1);
        assert_eq!(v1.trades[0].reason, crate::backtest::ExitReason::EndOfData);
        assert_both(&spec, &candles);
    }

    #[test]
    fn parity_indicator_exit() {
        let closes = [90.0, 110.0, 100.0, 90.0, 90.0];
        let candles: Vec<Candle> = closes
            .iter()
            .enumerate()
            .map(|(i, cl)| {
                let open = if i == 0 { 90.0 } else { closes[i - 1] };
                c(
                    i as i64,
                    open,
                    open.max(*cl) + 0.5,
                    open.min(*cl) - 0.5,
                    *cl,
                )
            })
            .collect();
        let exit = Rules {
            all: vec![cond(
                ind(IndicatorKind::Close, None),
                Op::Lt,
                ind(IndicatorKind::Sma, Some(3)),
            )],
            any: vec![],
        };
        let spec = mk_spec(Side::Long, gt_close(100.0), Some(exit), None, None);
        let (v1, _) = run_both(&spec, &candles, &zero_cost());
        assert_eq!(v1.trades.len(), 1);
        assert_eq!(v1.trades[0].reason, crate::backtest::ExitReason::Signal);
        assert_both(&spec, &candles);
    }

    #[test]
    fn parity_no_trades() {
        let candles: Vec<Candle> = (0..4).map(|i| c(i, 100.0, 100.0, 100.0, 100.0)).collect();
        let spec = mk_spec(Side::Long, gt_close(1000.0), None, None, None);
        let (v1, _) = run_both(&spec, &candles, &zero_cost());
        assert!(v1.trades.is_empty());
        assert_both(&spec, &candles);
    }

    fn rand_walk(n: usize, seed: u64) -> Vec<Candle> {
        let mut state = seed;
        let mut next01 = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) as f64) / ((1u64 << 31) as f64)
        };
        let mut out = Vec::with_capacity(n);
        let mut prev = 100.0;
        for i in 0..n {
            let drift = (next01() - 0.5) * 0.02;
            let close = prev * (1.0 + drift);
            let open = prev;
            let high = open.max(close) + next01() * 0.5;
            let low = open.min(close) - next01() * 0.5;
            out.push(c(i as i64, open, high, low, close));
            prev = close;
        }
        out
    }

    #[test]
    fn parity_seeded_random_walk() {
        let candles = rand_walk(5000, 42);
        let sets: Vec<crate::strategy::Strategy> = vec![
            mk_spec(Side::Long, gt_close(100.0), None, None, None),
            mk_spec(
                Side::Long,
                Rules {
                    all: vec![cond(
                        ind(IndicatorKind::Close, None),
                        Op::CrossesAbove,
                        ind(IndicatorKind::Sma, Some(10)),
                    )],
                    any: vec![],
                },
                Some(Rules {
                    all: vec![cond(
                        ind(IndicatorKind::Close, None),
                        Op::CrossesBelow,
                        ind(IndicatorKind::Sma, Some(10)),
                    )],
                    any: vec![],
                }),
                Some(2.0),
                Some(4.0),
            ),
            mk_spec(
                Side::Long,
                Rules {
                    all: vec![cond(ind(IndicatorKind::Rsi, Some(14)), Op::Lt, num(30.0))],
                    any: vec![],
                },
                Some(Rules {
                    all: vec![],
                    any: vec![cond(ind(IndicatorKind::Rsi, Some(14)), Op::Gt, num(55.0))],
                }),
                Some(2.0),
                Some(4.0),
            ),
            mk_spec(
                Side::Short,
                Rules {
                    all: vec![cond(
                        ind(IndicatorKind::Close, None),
                        Op::CrossesBelow,
                        ind(IndicatorKind::Ema, Some(20)),
                    )],
                    any: vec![],
                },
                Some(Rules {
                    all: vec![cond(
                        ind(IndicatorKind::Close, None),
                        Op::CrossesAbove,
                        ind(IndicatorKind::Ema, Some(20)),
                    )],
                    any: vec![],
                }),
                Some(1.0),
                Some(2.0),
            ),
            mk_spec(
                Side::Long,
                Rules {
                    all: vec![
                        cond(
                            ind(IndicatorKind::Close, None),
                            Op::Gt,
                            ind(IndicatorKind::Sma, Some(20)),
                        ),
                        cond(ind(IndicatorKind::Rsi, Some(14)), Op::Lt, num(70.0)),
                    ],
                    any: vec![cond(
                        ind(IndicatorKind::Close, None),
                        Op::CrossesAbove,
                        ind(IndicatorKind::Ema, Some(10)),
                    )],
                },
                Some(Rules {
                    all: vec![cond(
                        ind(IndicatorKind::Close, None),
                        Op::Lt,
                        ind(IndicatorKind::Sma, Some(20)),
                    )],
                    any: vec![],
                }),
                Some(2.0),
                None,
            ),
        ];
        for (k, spec) in sets.iter().enumerate() {
            let (v1, v2) = run_both(spec, &candles, &zero_cost());
            assert_zero(&v1, &v2);
            assert!(
                !v1.trades.is_empty() || k == 2,
                "set {k} unexpectedly empty"
            );
            let (v1c, v2c) = run_both(spec, &candles, &costly());
            assert_costly(&v1c, &v2c);
        }
    }
}
