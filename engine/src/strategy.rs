use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Long,
    Short,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IndicatorKind {
    Open,
    High,
    Low,
    Close,
    Sma,
    Ema,
    Rsi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IndicatorRef {
    pub ind: IndicatorKind,
    pub period: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Operand {
    Number(f64),
    Indicator(IndicatorRef),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum Op {
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = "crosses_above")]
    CrossesAbove,
    #[serde(rename = "crosses_below")]
    CrossesBelow,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub left: Operand,
    pub op: Op,
    pub right: Operand,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    #[serde(default)]
    pub all: Vec<Condition>,
    #[serde(default)]
    pub any: Vec<Condition>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub win_rate_pct: Option<f64>,
    pub quote: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Strategy {
    pub asset: String,
    pub timeframe: String,
    pub side: Side,
    pub entry: Rules,
    pub exit: Option<Rules>,
    pub stop_loss_pct: Option<f64>,
    pub take_profit_pct: Option<f64>,
    pub claim: Option<Claim>,
}

fn indicator_name(ind: IndicatorKind) -> &'static str {
    match ind {
        IndicatorKind::Open => "open",
        IndicatorKind::High => "high",
        IndicatorKind::Low => "low",
        IndicatorKind::Close => "close",
        IndicatorKind::Sma => "sma",
        IndicatorKind::Ema => "ema",
        IndicatorKind::Rsi => "rsi",
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum StrategyError {
    #[error("{0} indicator requires a period")]
    MissingPeriod(String),
    #[error("{0} indicator period must be > 0, got 0")]
    BadPeriod(String),
    #[error("{0} indicator must not have a period")]
    UnexpectedPeriod(String),
    #[error("condition compares two plain numbers (no indicator on either side)")]
    PlainNumbers,
    #[error("{0} must not be NaN")]
    Nan(String),
    #[error("{0} must be > 0, got {1}")]
    BadPct(String, f64),
    #[error("entry has no conditions (both `all` and `any` are empty)")]
    EmptyEntry,
    #[cfg(test)]
    #[error("range_bars must be > 0")]
    BadRangeBars,
    #[cfg(test)]
    #[error("buffer_pct must be >= 0, got {0}")]
    BadBuffer(f64),
    #[cfg(test)]
    #[error("max_trades_per_day must be > 0")]
    BadMaxTrades,
    #[error(transparent)]
    Instrument(#[from] crate::instrument::InstrumentError),
}

fn validate_operand(op: &Operand) -> Result<(), StrategyError> {
    if let Operand::Indicator(r) = op {
        match r.ind {
            IndicatorKind::Sma | IndicatorKind::Ema | IndicatorKind::Rsi => match r.period {
                None => {
                    return Err(StrategyError::MissingPeriod(
                        indicator_name(r.ind).to_string(),
                    ));
                }
                Some(0) => {
                    return Err(StrategyError::BadPeriod(indicator_name(r.ind).to_string()));
                }
                Some(_) => {}
            },
            IndicatorKind::Open
            | IndicatorKind::High
            | IndicatorKind::Low
            | IndicatorKind::Close => {
                if r.period.is_some() {
                    return Err(StrategyError::UnexpectedPeriod(
                        indicator_name(r.ind).to_string(),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_condition(c: &Condition) -> Result<(), StrategyError> {
    validate_operand(&c.left)?;
    validate_operand(&c.right)?;
    if matches!(c.left, Operand::Number(_)) && matches!(c.right, Operand::Number(_)) {
        return Err(StrategyError::PlainNumbers);
    }
    Ok(())
}

fn validate_pct(name: &str, v: f64) -> Result<(), StrategyError> {
    if v.is_nan() {
        return Err(StrategyError::Nan(name.to_string()));
    }
    if v <= 0.0 {
        return Err(StrategyError::BadPct(name.to_string(), v));
    }
    Ok(())
}

impl Strategy {
    pub fn validate(&self) -> Result<(), StrategyError> {
        if self.entry.all.is_empty() && self.entry.any.is_empty() {
            return Err(StrategyError::EmptyEntry);
        }
        for c in self.entry.all.iter().chain(self.entry.any.iter()) {
            validate_condition(c)?;
        }
        if let Some(exit) = &self.exit {
            for c in exit.all.iter().chain(exit.any.iter()) {
                validate_condition(c)?;
            }
        }
        if let Some(v) = self.stop_loss_pct {
            validate_pct("stop_loss_pct", v)?;
        }
        if let Some(v) = self.take_profit_pct {
            validate_pct("take_profit_pct", v)?;
        }
        Ok(())
    }
}

#[cfg(test)]
use crate::session::day_id;
use crate::types::Candle;

pub trait BarStrategy {
    fn on_bar(&mut self, idx: usize, candles: &[Candle]) -> Option<Side>;

    fn exit_on_bar(&mut self, _idx: usize, _candles: &[Candle]) -> bool {
        false
    }

    fn on_fill(&mut self, _idx: usize) {}
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbConfig {
    pub range_bars: usize,
    pub buffer_pct: f64,
    pub max_trades_per_day: usize,
}

#[cfg(test)]
impl OrbConfig {
    pub fn new(
        range_bars: usize,
        buffer_pct: f64,
        max_trades_per_day: usize,
    ) -> Result<OrbConfig, StrategyError> {
        if range_bars == 0 {
            return Err(StrategyError::BadRangeBars);
        }
        if !buffer_pct.is_finite() || buffer_pct < 0.0 {
            return Err(StrategyError::BadBuffer(buffer_pct));
        }
        if max_trades_per_day == 0 {
            return Err(StrategyError::BadMaxTrades);
        }
        Ok(OrbConfig {
            range_bars,
            buffer_pct,
            max_trades_per_day,
        })
    }
}

#[cfg(test)]
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

#[cfg(test)]
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

    pub fn range(&self) -> Option<(f64, f64)> {
        if self.range_ready {
            Some((self.range_high, self.range_low))
        } else {
            None
        }
    }
}

#[cfg(test)]
impl BarStrategy for Orb {
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

#[derive(Debug, Clone)]
pub struct RulesStrategy {
    side: Side,
    entry: Rules,
    exit: Option<Rules>,
    armed: bool,
    was_open: bool,
}

impl RulesStrategy {
    pub fn new(spec: &Strategy) -> Self {
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

fn rules_series(ind: IndicatorKind, period: Option<usize>, closes: &[f64]) -> Vec<f64> {
    let p = match period {
        Some(v) => v,
        None => return Vec::new(),
    };
    match ind {
        IndicatorKind::Sma => crate::indicators::sma(closes, p),
        IndicatorKind::Ema => crate::indicators::ema(closes, p),
        IndicatorKind::Rsi => crate::indicators::rsi(closes, p),
        IndicatorKind::Open | IndicatorKind::High | IndicatorKind::Low | IndicatorKind::Close => {
            Vec::new()
        }
    }
}

fn rules_operand(
    op: &Operand,
    idx: usize,
    candles: &[crate::types::Candle],
    closes: &[f64],
) -> f64 {
    match op {
        Operand::Number(v) => *v,
        Operand::Indicator(r) => match r.ind {
            IndicatorKind::Open => match candles.get(idx) {
                Some(c) => c.open,
                None => f64::NAN,
            },
            IndicatorKind::High => match candles.get(idx) {
                Some(c) => c.high,
                None => f64::NAN,
            },
            IndicatorKind::Low => match candles.get(idx) {
                Some(c) => c.low,
                None => f64::NAN,
            },
            IndicatorKind::Close => match candles.get(idx) {
                Some(c) => c.close,
                None => f64::NAN,
            },
            IndicatorKind::Sma | IndicatorKind::Ema | IndicatorKind::Rsi => {
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
    c: &Condition,
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
        Op::Lt => l < r,
        Op::Gt => l > r,
        Op::CrossesAbove => {
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
        Op::CrossesBelow => {
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
    rules: &Rules,
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

impl BarStrategy for RulesStrategy {
    fn on_bar(&mut self, idx: usize, candles: &[crate::types::Candle]) -> Option<Side> {
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

pub fn rules_v2_setup(
    spec: &Strategy,
    cfg: &crate::backtest::BacktestConfig,
) -> Result<
    (
        crate::instrument::Instrument,
        crate::sizing::Sizing,
        crate::backtest::Bracket,
        crate::session::SessionFilter,
        crate::backtest::RunConfig,
    ),
    StrategyError,
> {
    // Tiny tick keeps round_to_tick a no-op within tolerance.
    let instrument = crate::instrument::Instrument::new(
        &spec.asset,
        0.000000001,
        1.0,
        cfg.fee_bps,
        cfg.slippage_bps,
    )?;
    // Full-equity sizing matches v1 behaviour.
    let sizing = crate::sizing::Sizing::RiskPct {
        risk_pct: 100.0,
        stop_pct: 100.0,
    };
    let bracket = crate::backtest::Bracket {
        stop_pct: spec.stop_loss_pct,
        target_pct: spec.take_profit_pct,
        trailing_pct: None,
    };
    let session = crate::session::SessionFilter::all_day();
    let run = crate::backtest::RunConfig::new(1.0, false)
        .map_err(|_| StrategyError::BadPct("initial_equity".to_string(), 1.0))?;
    Ok((instrument, sizing, bracket, session, run))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_example_strategy() {
        let json = r#"{"asset":"BTCUSDT","timeframe":"1h","side":"long","entry":{"all":[{"left":{"ind":"rsi","period":14},"op":"<","right":30},{"left":{"ind":"close"},"op":">","right":{"ind":"sma","period":200}}]},"exit":{"any":[{"left":{"ind":"rsi","period":14},"op":">","right":55}]},"stop_loss_pct":2,"take_profit_pct":4}"#;
        let s: Strategy = serde_json::from_str(json).expect("example strategy parses");

        assert_eq!(s.asset, "BTCUSDT");
        assert_eq!(s.timeframe, "1h");
        assert_eq!(s.side, Side::Long);

        assert_eq!(s.entry.all.len(), 2);
        assert!(s.entry.any.is_empty());

        assert_eq!(
            s.entry.all[0].left,
            Operand::Indicator(IndicatorRef {
                ind: IndicatorKind::Rsi,
                period: Some(14),
            })
        );
        assert_eq!(s.entry.all[0].op, Op::Lt);
        assert_eq!(s.entry.all[0].right, Operand::Number(30.0));

        assert_eq!(
            s.entry.all[1].left,
            Operand::Indicator(IndicatorRef {
                ind: IndicatorKind::Close,
                period: None,
            })
        );
        assert_eq!(s.entry.all[1].op, Op::Gt);
        assert_eq!(
            s.entry.all[1].right,
            Operand::Indicator(IndicatorRef {
                ind: IndicatorKind::Sma,
                period: Some(200),
            })
        );

        let exit = s.exit.expect("exit present");
        assert!(exit.all.is_empty());
        assert_eq!(exit.any.len(), 1);
        assert_eq!(
            exit.any[0].left,
            Operand::Indicator(IndicatorRef {
                ind: IndicatorKind::Rsi,
                period: Some(14),
            })
        );
        assert_eq!(exit.any[0].op, Op::Gt);
        assert_eq!(exit.any[0].right, Operand::Number(55.0));

        assert_eq!(s.stop_loss_pct, Some(2.0));
        assert_eq!(s.take_profit_pct, Some(4.0));
    }

    #[test]
    fn rejects_unknown_op() {
        let json = r#"{"asset":"BTCUSDT","timeframe":"1h","side":"long","entry":{"all":[{"left":{"ind":"rsi","period":14},"op":"~=","right":30}]}}"#;
        let err = serde_json::from_str::<Strategy>(json).expect_err("unknown op must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("~=") || msg.to_lowercase().contains("unknown"),
            "unexpected error message: {msg}"
        );
    }

    fn valid_strategy() -> Strategy {
        let json = r#"{"asset":"BTCUSDT","timeframe":"1h","side":"long","entry":{"all":[{"left":{"ind":"rsi","period":14},"op":"<","right":30},{"left":{"ind":"close"},"op":">","right":{"ind":"sma","period":200}}]},"exit":{"any":[{"left":{"ind":"rsi","period":14},"op":">","right":55}]},"stop_loss_pct":2,"take_profit_pct":4}"#;
        serde_json::from_str(json).expect("valid fixture parses")
    }

    fn ind(ind: IndicatorKind, period: Option<usize>) -> Operand {
        Operand::Indicator(IndicatorRef { ind, period })
    }

    fn cond(left: Operand, op: Op, right: Operand) -> Condition {
        Condition { left, op, right }
    }

    #[test]
    fn valid_strategy_passes_validate() {
        let s = valid_strategy();
        assert!(
            s.validate().is_ok(),
            "valid strategy should pass: {:?}",
            s.validate().err()
        );
    }

    #[test]
    fn validate_rejects_empty_entry() {
        let mut s = valid_strategy();
        s.entry = Rules {
            all: vec![],
            any: vec![],
        };
        let err = s.validate().expect_err("empty entry must fail");
        assert!(
            err.to_string().to_lowercase().contains("entry"),
            "error should mention entry, got: {err}"
        );
    }

    #[test]
    fn validate_rejects_missing_period() {
        let mut s = valid_strategy();
        s.entry.all[0] = cond(ind(IndicatorKind::Rsi, None), Op::Lt, Operand::Number(30.0));
        let err = s
            .validate()
            .expect_err("sma/ema/rsi without period must fail");
        let lower = err.to_string().to_lowercase();
        assert!(
            lower.contains("rsi") && lower.contains("period"),
            "error should mention rsi and period, got: {err}"
        );
    }

    #[test]
    fn validate_rejects_zero_period() {
        let mut s = valid_strategy();
        s.entry.all[0] = cond(
            ind(IndicatorKind::Sma, Some(0)),
            Op::Gt,
            Operand::Number(1.0),
        );
        let err = s.validate().expect_err("period == 0 must fail");
        assert!(
            err.to_string().to_lowercase().contains("period"),
            "error should mention period, got: {err}"
        );
    }

    #[test]
    fn validate_rejects_price_with_period() {
        let mut s = valid_strategy();
        s.entry.all[1] = cond(
            ind(IndicatorKind::Close, Some(14)),
            Op::Gt,
            Operand::Number(1.0),
        );
        let err = s.validate().expect_err("price with period must fail");
        let lower = err.to_string().to_lowercase();
        assert!(
            lower.contains("close") && lower.contains("period"),
            "error should mention close and period, got: {err}"
        );
    }

    #[test]
    fn validate_rejects_bad_stop_loss() {
        let mut s = valid_strategy();
        s.stop_loss_pct = Some(0.0);
        let err = s.validate().expect_err("stop_loss_pct <= 0 must fail");
        assert!(
            err.to_string().contains("stop_loss_pct"),
            "error should mention stop_loss_pct, got: {err}"
        );

        let mut s = valid_strategy();
        s.stop_loss_pct = Some(f64::NAN);
        let err = s.validate().expect_err("stop_loss_pct NaN must fail");
        assert!(
            err.to_string().contains("stop_loss_pct"),
            "error should mention stop_loss_pct, got: {err}"
        );
    }

    #[test]
    fn validate_rejects_bad_take_profit() {
        let mut s = valid_strategy();
        s.take_profit_pct = Some(-1.0);
        let err = s.validate().expect_err("take_profit_pct <= 0 must fail");
        assert!(
            err.to_string().contains("take_profit_pct"),
            "error should mention take_profit_pct, got: {err}"
        );

        let mut s = valid_strategy();
        s.take_profit_pct = Some(f64::NAN);
        let err = s.validate().expect_err("take_profit_pct NaN must fail");
        assert!(
            err.to_string().contains("take_profit_pct"),
            "error should mention take_profit_pct, got: {err}"
        );
    }

    #[test]
    fn validate_rejects_two_plain_numbers() {
        let mut s = valid_strategy();
        s.entry.all[0] = cond(Operand::Number(30.0), Op::Lt, Operand::Number(55.0));
        let err = s.validate().expect_err("two plain numbers must fail");
        assert!(
            err.to_string().to_lowercase().contains("plain number"),
            "error should mention plain numbers, got: {err}"
        );
    }

    #[test]
    fn validate_checks_exit_conditions_too() {
        let mut s = valid_strategy();
        s.exit = Some(Rules {
            all: vec![cond(Operand::Number(1.0), Op::Gt, Operand::Number(2.0))],
            any: vec![],
        });
        let err = s.validate().expect_err("bad exit condition must fail");
        assert!(
            err.to_string().to_lowercase().contains("plain number"),
            "error should mention plain numbers, got: {err}"
        );
    }

    #[test]
    fn rejects_misspelled_stop_loss_field() {
        let json = r#"{"asset":"BTCUSDT","timeframe":"1h","side":"long","entry":{"all":[{"left":{"ind":"rsi","period":14},"op":"<","right":30}]},"stop_loss":2}"#;
        let err = serde_json::from_str::<Strategy>(json).expect_err("misspelled field must fail");
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("stop_loss"),
            "error should mention the unknown field, got: {msg}"
        );
    }
}

#[cfg(test)]
mod orb_tests {
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
        assert_eq!(orb.range(), Some((102.0, 99.0)));
        assert_eq!(orb.on_bar(2, &bars), Some(Side::Long));
        orb.on_fill(3);
        assert_eq!(orb.on_bar(3, &bars), None);
    }

    #[test]
    fn orb_short_breakout_and_new_day_resets() {
        let cfg = OrbConfig::new(1, 0.0, 1).expect("valid");
        let mut orb = Orb::new(cfg);
        let mut bars = day_bars();
        assert_eq!(orb.on_bar(0, &bars), None);
        assert_eq!(orb.range(), Some((101.0, 99.0)));
        assert_eq!(orb.on_bar(1, &bars), None);
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
    use super::{rules_v2_setup, RulesStrategy, Strategy};
    use crate::backtest::{BacktestConfig, BacktestResult, ExitReason};
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
    ) -> Strategy {
        Strategy {
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

    fn run_v2(spec: &Strategy, candles: &[Candle], cfg: &BacktestConfig) -> BacktestResult {
        let (inst, sizing, bracket, session, run_cfg) =
            rules_v2_setup(spec, cfg).expect("v2 setup valid");
        let mut rs = RulesStrategy::new(spec);
        crate::backtest::run(
            &mut rs, candles, &inst, sizing, &bracket, &session, &run_cfg,
        )
        .expect("v2 run valid")
    }

    type ExpTrade = (usize, usize, Side, ExitReason, f64, f64, f64);

    fn assert_zero_golden(v2: &BacktestResult, trades: &[ExpTrade], equity: &[(usize, f64)]) {
        assert_eq!(v2.trades.len(), trades.len(), "trade count {:?}", v2.trades);
        for (got, exp) in v2.trades.iter().zip(trades.iter()) {
            assert_eq!(got.entry_idx, exp.0, "entry_idx {got:?}");
            assert_eq!(got.exit_idx, exp.1, "exit_idx {got:?}");
            assert_eq!(got.side, exp.2);
            assert_eq!(got.reason, exp.3, "reason {got:?}");
            assert!(
                (got.entry_price - exp.4).abs() <= 1e-9,
                "entry_price {} vs {}",
                got.entry_price,
                exp.4
            );
            assert!(
                (got.exit_price - exp.5).abs() <= 1e-9,
                "exit_price {} vs {}",
                got.exit_price,
                exp.5
            );
            assert!(
                (got.return_pct - exp.6).abs() <= 1e-9,
                "return_pct {} vs {}",
                got.return_pct,
                exp.6
            );
        }
        for (idx, want) in equity {
            let got = v2.equity_curve[*idx];
            assert!((got - want).abs() <= 1e-9, "equity[{idx}] {got} vs {want}");
        }
    }

    fn assert_costly_golden(
        v2: &BacktestResult,
        count: usize,
        returns: &[(usize, usize, f64)],
        equity: &[(usize, f64)],
    ) {
        assert_eq!(v2.trades.len(), count, "trade count {:?}", v2.trades);
        assert_eq!(returns.len(), v2.trades.len().min(returns.len()));
        for (got, exp) in v2.trades.iter().zip(returns.iter()) {
            assert_eq!(got.entry_idx, exp.0, "entry_idx {got:?}");
            assert_eq!(got.exit_idx, exp.1, "exit_idx {got:?}");
            assert!(
                (got.return_pct - exp.2).abs() <= 0.05,
                "return_pct {} vs {}",
                got.return_pct,
                exp.2
            );
        }
        for (idx, want) in equity {
            let got = v2.equity_curve[*idx];
            let denom = want.abs().max(1e-12);
            assert!(
                (got - want).abs() / denom <= 1e-3,
                "equity[{idx}] {got} vs {want}"
            );
        }
    }

    fn assert_both_golden(
        spec: &Strategy,
        candles: &[Candle],
        zero_trades: &[ExpTrade],
        zero_equity: &[(usize, f64)],
        costly_count: usize,
        costly_returns: &[(usize, usize, f64)],
        costly_equity: &[(usize, f64)],
    ) {
        let v2 = run_v2(spec, candles, &zero_cost());
        assert_zero_golden(&v2, zero_trades, zero_equity);
        let v2c = run_v2(spec, candles, &costly());
        assert_costly_golden(&v2c, costly_count, costly_returns, costly_equity);
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
        let zero_trades = [(2, 3, Side::Long, ExitReason::EndOfData, 200.0, 206.0, 3.0)];
        let zero_equity = [(0, 1.0), (1, 1.0), (2, 1.025), (3, 1.03)];
        let costly_returns = [(2, 3, 2.8559008693914256)];
        let costly_equity = [(0, 1.0), (2, 1.0235660037973417), (3, 1.0285590086939143)];
        assert_both_golden(
            &spec,
            &candles,
            &zero_trades,
            &zero_equity,
            1,
            &costly_returns,
            &costly_equity,
        );
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
        let zero_trades = [(
            2,
            3,
            Side::Short,
            ExitReason::EndOfData,
            90.0,
            85.0,
            5.555555555555555,
        )];
        let zero_equity = [(0, 1.0), (2, 1.0), (3, 1.0555555555555556)];
        let costly_returns = [(2, 3, 5.4232407129435)];
        let costly_equity = [(0, 1.0), (2, 0.998599019314), (3, 1.054232407129)];
        assert_both_golden(
            &spec,
            &candles,
            &zero_trades,
            &zero_equity,
            1,
            &costly_returns,
            &costly_equity,
        );
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
        let zero_trades = [(5, 5, Side::Long, ExitReason::EndOfData, 110.0, 110.0, 0.0)];
        let zero_equity = [(0, 1.0), (4, 1.0), (5, 1.0)];
        let costly_returns = [(5, 5, -0.13990206855201137)];
        let costly_equity = [(0, 1.0), (4, 1.0), (5, 0.9986009793144799)];
        assert_both_golden(
            &spec,
            &candles,
            &zero_trades,
            &zero_equity,
            1,
            &costly_returns,
            &costly_equity,
        );
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
        let zero_trades = [(3, 3, Side::Long, ExitReason::EndOfData, 106.0, 106.0, 0.0)];
        let zero_equity = [(0, 1.0), (2, 1.0), (3, 1.0)];
        let costly_returns = [(3, 3, -0.13990206855201137)];
        let costly_equity = [(0, 1.0), (2, 1.0), (3, 0.9986009793144799)];
        assert_both_golden(
            &spec,
            &candles,
            &zero_trades,
            &zero_equity,
            1,
            &costly_returns,
            &costly_equity,
        );
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
        let zero_trades = [(1, 2, Side::Long, ExitReason::StopLoss, 100.0, 98.0, -2.0)];
        let zero_equity = [(0, 1.0), (1, 1.0), (2, 0.98), (3, 0.98)];
        let costly_returns = [(1, 2, -2.1371040271809694)];
        let costly_equity = [(0, 1.0), (1, 0.9986009793144799), (2, 0.9786289597281903)];
        assert_both_golden(
            &spec,
            &candles,
            &zero_trades,
            &zero_equity,
            1,
            &costly_returns,
            &costly_equity,
        );
    }

    #[test]
    fn parity_take_profit() {
        let candles = vec![
            c(0, 90.0, 115.0, 90.0, 110.0),
            c(1, 100.0, 101.0, 99.0, 100.0),
            c(2, 100.0, 106.0, 99.0, 102.0),
        ];
        let spec = mk_spec(Side::Long, gt_close(100.0), None, None, Some(5.0));
        let zero_trades = [(1, 2, Side::Long, ExitReason::TakeProfit, 100.0, 105.0, 5.0)];
        let zero_equity = [(0, 1.0), (1, 1.0), (2, 1.05)];
        let costly_returns = [(1, 2, 4.853102828020384)];
        let costly_equity = [(0, 1.0), (1, 0.9986009793144799), (2, 1.0485310282802038)];
        assert_both_golden(
            &spec,
            &candles,
            &zero_trades,
            &zero_equity,
            1,
            &costly_returns,
            &costly_equity,
        );
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
        let zero_trades = [(1, 2, Side::Long, ExitReason::StopLoss, 100.0, 98.0, -2.0)];
        let zero_equity = [(0, 1.0), (1, 1.0), (2, 0.98)];
        let costly_returns = [(1, 2, -2.1371040271809694)];
        let costly_equity = [(0, 1.0), (1, 0.9986009793144799), (2, 0.9786289597281903)];
        assert_both_golden(
            &spec,
            &candles,
            &zero_trades,
            &zero_equity,
            1,
            &costly_returns,
            &costly_equity,
        );
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
        let zero_trades = [(1, 2, Side::Long, ExitReason::StopLoss, 100.0, 90.0, -10.0)];
        let zero_equity = [(0, 1.0), (1, 1.0), (2, 0.9)];
        let costly_returns = [(1, 2, -10.125911861696812)];
        let costly_equity = [(0, 1.0), (1, 0.9986009793144799), (2, 0.8987408813830319)];
        assert_both_golden(
            &spec,
            &candles,
            &zero_trades,
            &zero_equity,
            1,
            &costly_returns,
            &costly_equity,
        );
    }

    #[test]
    fn parity_gap_beyond_target() {
        let candles = vec![
            c(0, 90.0, 115.0, 90.0, 110.0),
            c(1, 100.0, 101.0, 99.0, 100.0),
            c(2, 105.0, 106.0, 90.0, 95.0),
        ];
        let spec = mk_spec(Side::Long, gt_close(100.0), None, Some(2.0), Some(2.0));
        let zero_trades = [(1, 2, Side::Long, ExitReason::TakeProfit, 100.0, 105.0, 5.0)];
        let zero_equity = [(0, 1.0), (1, 1.0), (2, 1.05)];
        let costly_returns = [(1, 2, 4.853102828020384)];
        let costly_equity = [(0, 1.0), (1, 0.9986009793144799), (2, 1.0485310282802038)];
        assert_both_golden(
            &spec,
            &candles,
            &zero_trades,
            &zero_equity,
            1,
            &costly_returns,
            &costly_equity,
        );
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
        let zero_trades = [
            (1, 1, Side::Long, ExitReason::StopLoss, 100.0, 98.0, -2.0),
            (5, 6, Side::Long, ExitReason::EndOfData, 100.0, 100.0, 0.0),
        ];
        let zero_equity = [(0, 1.0), (1, 0.98), (5, 1.078), (6, 0.98)];
        let costly_returns = [(1, 1, -2.1371040271809694), (5, 6, -0.13990206855201137)];
        let costly_equity = [(0, 1.0), (1, 0.9786289597281903), (6, 0.9772598375700815)];
        assert_both_golden(
            &spec,
            &candles,
            &zero_trades,
            &zero_equity,
            2,
            &costly_returns,
            &costly_equity,
        );
    }

    #[test]
    fn parity_open_position_at_end() {
        let candles = vec![
            c(0, 90.0, 91.0, 89.0, 90.0),
            c(1, 90.0, 115.0, 90.0, 110.0),
            c(2, 100.0, 101.0, 99.0, 100.0),
        ];
        let spec = mk_spec(Side::Long, gt_close(100.0), None, None, None);
        let zero_trades = [(2, 2, Side::Long, ExitReason::EndOfData, 100.0, 100.0, 0.0)];
        let zero_equity = [(0, 1.0), (1, 1.0), (2, 1.0)];
        let costly_returns = [(2, 2, -0.13990206855201137)];
        let costly_equity = [(0, 1.0), (1, 1.0), (2, 0.9986009793144799)];
        assert_both_golden(
            &spec,
            &candles,
            &zero_trades,
            &zero_equity,
            1,
            &costly_returns,
            &costly_equity,
        );
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
        let zero_trades = [(
            2,
            4,
            Side::Long,
            ExitReason::Signal,
            110.0,
            90.0,
            -18.181818181818183,
        )];
        let zero_equity = [(0, 1.0), (2, 0.9090909090909091), (4, 0.8181818181818182)];
        let costly_returns = [(2, 4, -18.29628351063347)];
        let costly_equity = [(0, 1.0), (2, 0.9078190721040725), (4, 0.8170371648936653)];
        assert_both_golden(
            &spec,
            &candles,
            &zero_trades,
            &zero_equity,
            1,
            &costly_returns,
            &costly_equity,
        );
    }

    #[test]
    fn parity_no_trades() {
        let candles: Vec<Candle> = (0..4).map(|i| c(i, 100.0, 100.0, 100.0, 100.0)).collect();
        let spec = mk_spec(Side::Long, gt_close(1000.0), None, None, None);
        let zero_trades: &[ExpTrade] = &[];
        let zero_equity = [(0, 1.0), (3, 1.0)];
        let costly_returns: &[(usize, usize, f64)] = &[];
        let costly_equity = [(0, 1.0), (3, 1.0)];
        assert_both_golden(
            &spec,
            &candles,
            zero_trades,
            &zero_equity,
            0,
            costly_returns,
            &costly_equity,
        );
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
        let sets: Vec<Strategy> = vec![
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
        let counts = [1usize, 383, 40, 320, 199];
        let zero_first20: &[&[ExpTrade]] = &[
            &[(
                1,
                4999,
                Side::Long,
                ExitReason::EndOfData,
                100.136460653,
                68.3949485386,
                -31.6982564663,
            )],
            &[
                (
                    11,
                    19,
                    Side::Long,
                    ExitReason::Signal,
                    99.4138211959,
                    99.1533988475,
                    -0.261957890009,
                ),
                (
                    22,
                    25,
                    Side::Long,
                    ExitReason::Signal,
                    100.056480517,
                    99.4796542893,
                    -0.576500617623,
                ),
                (
                    30,
                    43,
                    Side::Long,
                    ExitReason::Signal,
                    100.022350723,
                    102.053987727,
                    2.03118302006,
                ),
                (
                    57,
                    58,
                    Side::Long,
                    ExitReason::Signal,
                    100.246879661,
                    99.3208802006,
                    -0.923718986101,
                ),
                (
                    60,
                    62,
                    Side::Long,
                    ExitReason::Signal,
                    99.9996796746,
                    99.5425210028,
                    -0.457160136274,
                ),
                (
                    72,
                    81,
                    Side::Long,
                    ExitReason::TakeProfit,
                    99.8621699503,
                    103.856656748,
                    4.0,
                ),
                (
                    100,
                    103,
                    Side::Long,
                    ExitReason::Signal,
                    102.94688783,
                    101.955468588,
                    -0.963039547046,
                ),
                (
                    111,
                    112,
                    Side::Long,
                    ExitReason::Signal,
                    101.660068071,
                    101.166229965,
                    -0.485773927532,
                ),
                (
                    115,
                    132,
                    Side::Long,
                    ExitReason::Signal,
                    102.016000231,
                    104.257116113,
                    2.19682782766,
                ),
                (
                    136,
                    137,
                    Side::Long,
                    ExitReason::Signal,
                    104.271483857,
                    103.483035722,
                    -0.756149337627,
                ),
                (
                    157,
                    166,
                    Side::Long,
                    ExitReason::Signal,
                    99.874545983,
                    100.208107703,
                    0.333980712376,
                ),
                (
                    169,
                    171,
                    Side::Long,
                    ExitReason::Signal,
                    100.669997729,
                    100.529877016,
                    -0.139188156153,
                ),
                (
                    200,
                    203,
                    Side::Long,
                    ExitReason::Signal,
                    94.4713242919,
                    93.3856856099,
                    -1.14917271479,
                ),
                (
                    210,
                    212,
                    Side::Long,
                    ExitReason::Signal,
                    94.6375193017,
                    93.8959360127,
                    -0.783603896724,
                ),
                (
                    233,
                    234,
                    Side::Long,
                    ExitReason::Signal,
                    94.092993411,
                    93.8284076614,
                    -0.281196016818,
                ),
                (
                    237,
                    239,
                    Side::Long,
                    ExitReason::Signal,
                    94.4887716777,
                    93.2529147904,
                    -1.30794047303,
                ),
                (
                    250,
                    262,
                    Side::Long,
                    ExitReason::Signal,
                    91.9570795135,
                    92.2323461841,
                    0.299342554151,
                ),
                (
                    280,
                    290,
                    Side::Long,
                    ExitReason::Signal,
                    89.7887188478,
                    90.9420092343,
                    1.28444909483,
                ),
                (
                    294,
                    295,
                    Side::Long,
                    ExitReason::Signal,
                    91.1776759935,
                    90.5297951429,
                    -0.710569603369,
                ),
                (
                    298,
                    299,
                    Side::Long,
                    ExitReason::Signal,
                    90.9205970594,
                    90.1941598944,
                    -0.798979756422,
                ),
            ],
            &[
                (
                    153,
                    182,
                    Side::Long,
                    ExitReason::StopLoss,
                    98.6467375253,
                    96.6738027748,
                    -2.0,
                ),
                (
                    191,
                    194,
                    Side::Long,
                    ExitReason::StopLoss,
                    95.4448256921,
                    93.5359291783,
                    -2.0,
                ),
                (
                    402,
                    420,
                    Side::Long,
                    ExitReason::Signal,
                    85.93234932,
                    87.162755633,
                    1.43183134487,
                ),
                (
                    543,
                    560,
                    Side::Long,
                    ExitReason::Signal,
                    89.7422191448,
                    91.6429630615,
                    2.1180041399,
                ),
                (
                    590,
                    613,
                    Side::Long,
                    ExitReason::Signal,
                    87.5999287889,
                    88.5997804109,
                    1.14138405808,
                ),
                (
                    660,
                    679,
                    Side::Long,
                    ExitReason::StopLoss,
                    82.1593842378,
                    80.516196553,
                    -2.0,
                ),
                (
                    688,
                    700,
                    Side::Long,
                    ExitReason::Signal,
                    78.8624848916,
                    81.2049769301,
                    2.97035027701,
                ),
                (
                    994,
                    1000,
                    Side::Long,
                    ExitReason::StopLoss,
                    85.8175310709,
                    84.1011804495,
                    -2.0,
                ),
                (
                    1223,
                    1230,
                    Side::Long,
                    ExitReason::TakeProfit,
                    83.9738437906,
                    87.3327975423,
                    4.0,
                ),
                (
                    1265,
                    1266,
                    Side::Long,
                    ExitReason::StopLoss,
                    83.3234218582,
                    81.6569534211,
                    -2.0,
                ),
                (
                    1271,
                    1301,
                    Side::Long,
                    ExitReason::StopLoss,
                    81.7527167322,
                    80.1176623975,
                    -2.0,
                ),
                (
                    1445,
                    1458,
                    Side::Long,
                    ExitReason::Signal,
                    84.4719439146,
                    86.8539712513,
                    2.81990353995,
                ),
                (
                    1694,
                    1701,
                    Side::Long,
                    ExitReason::StopLoss,
                    90.2964934295,
                    88.4905635609,
                    -2.0,
                ),
                (
                    1737,
                    1745,
                    Side::Long,
                    ExitReason::Signal,
                    85.8001267461,
                    88.1013407308,
                    2.68206361922,
                ),
                (
                    1901,
                    1917,
                    Side::Long,
                    ExitReason::StopLoss,
                    89.9053259359,
                    88.1072194172,
                    -2.0,
                ),
                (
                    1932,
                    1945,
                    Side::Long,
                    ExitReason::Signal,
                    86.7383971206,
                    88.4084976239,
                    1.92544542992,
                ),
                (
                    1990,
                    1996,
                    Side::Long,
                    ExitReason::StopLoss,
                    84.4666041265,
                    82.777272044,
                    -2.0,
                ),
                (
                    2102,
                    2137,
                    Side::Long,
                    ExitReason::Signal,
                    81.6842605172,
                    82.808267535,
                    1.37603867703,
                ),
                (
                    2275,
                    2286,
                    Side::Long,
                    ExitReason::StopLoss,
                    84.295542146,
                    82.609631303,
                    -2.0,
                ),
                (
                    2318,
                    2333,
                    Side::Long,
                    ExitReason::StopLoss,
                    80.586768032,
                    78.9750326714,
                    -2.0,
                ),
            ],
            &[
                (
                    25,
                    30,
                    Side::Short,
                    ExitReason::Signal,
                    99.4796542893,
                    100.022350723,
                    -0.545535101844,
                ),
                (
                    45,
                    46,
                    Side::Short,
                    ExitReason::Signal,
                    101.407963977,
                    101.730902721,
                    -0.318455012515,
                ),
                (
                    65,
                    67,
                    Side::Short,
                    ExitReason::TakeProfit,
                    100.122502626,
                    98.1200525733,
                    2.0,
                ),
                (
                    77,
                    78,
                    Side::Short,
                    ExitReason::Signal,
                    99.9608061562,
                    100.467545793,
                    -0.506938325241,
                ),
                (
                    93,
                    100,
                    Side::Short,
                    ExitReason::Signal,
                    102.98494317,
                    102.94688783,
                    0.0369523332936,
                ),
                (
                    102,
                    107,
                    Side::Short,
                    ExitReason::TakeProfit,
                    102.652233661,
                    100.599188988,
                    2.0,
                ),
                (
                    134,
                    135,
                    Side::Short,
                    ExitReason::Signal,
                    103.499426374,
                    103.732697263,
                    -0.225383751094,
                ),
                (
                    137,
                    142,
                    Side::Short,
                    ExitReason::TakeProfit,
                    103.483035722,
                    101.413375008,
                    2.0,
                ),
                (
                    160,
                    162,
                    Side::Short,
                    ExitReason::Signal,
                    100.519617677,
                    100.874252762,
                    -0.352801863856,
                ),
                (
                    166,
                    170,
                    Side::Short,
                    ExitReason::Signal,
                    100.208107703,
                    100.853565755,
                    -0.644117593538,
                ),
                (
                    175,
                    178,
                    Side::Short,
                    ExitReason::TakeProfit,
                    100.66417889,
                    98.6508953117,
                    2.0,
                ),
                (
                    212,
                    214,
                    Side::Short,
                    ExitReason::StopLoss,
                    93.8959360127,
                    94.8348953728,
                    -1.0,
                ),
                (
                    216,
                    216,
                    Side::Short,
                    ExitReason::StopLoss,
                    94.2628642664,
                    95.2054929091,
                    -1.0,
                ),
                (
                    224,
                    230,
                    Side::Short,
                    ExitReason::TakeProfit,
                    95.0469848131,
                    93.1460451169,
                    2.0,
                ),
                (
                    236,
                    236,
                    Side::Short,
                    ExitReason::StopLoss,
                    93.8295468236,
                    94.7678422918,
                    -1.0,
                ),
                (
                    238,
                    242,
                    Side::Short,
                    ExitReason::TakeProfit,
                    93.95651384,
                    92.0773835632,
                    2.0,
                ),
                (
                    262,
                    262,
                    Side::Short,
                    ExitReason::StopLoss,
                    92.2323461841,
                    93.1546696459,
                    -1.0,
                ),
                (
                    264,
                    269,
                    Side::Short,
                    ExitReason::TakeProfit,
                    92.1731900806,
                    90.329726279,
                    2.0,
                ),
                (
                    282,
                    283,
                    Side::Short,
                    ExitReason::StopLoss,
                    90.3386797469,
                    91.2420665444,
                    -1.0,
                ),
                (
                    291,
                    293,
                    Side::Short,
                    ExitReason::StopLoss,
                    90.4846908617,
                    91.3895377703,
                    -1.0,
                ),
            ],
            &[
                (
                    22,
                    25,
                    Side::Long,
                    ExitReason::Signal,
                    100.056480517,
                    99.4796542893,
                    -0.576500617623,
                ),
                (
                    30,
                    45,
                    Side::Long,
                    ExitReason::Signal,
                    100.022350723,
                    101.407963977,
                    1.38530362897,
                ),
                (
                    63,
                    65,
                    Side::Long,
                    ExitReason::Signal,
                    100.402954045,
                    100.122502626,
                    -0.279325864162,
                ),
                (
                    78,
                    93,
                    Side::Long,
                    ExitReason::Signal,
                    100.467545793,
                    102.98494317,
                    2.50568216552,
                ),
                (
                    115,
                    134,
                    Side::Long,
                    ExitReason::Signal,
                    102.016000231,
                    103.499426374,
                    1.45411125615,
                ),
                (
                    136,
                    137,
                    Side::Long,
                    ExitReason::Signal,
                    104.271483857,
                    103.483035722,
                    -0.756149337627,
                ),
                (
                    158,
                    160,
                    Side::Long,
                    ExitReason::Signal,
                    100.823543673,
                    100.519617677,
                    -0.301443477216,
                ),
                (
                    162,
                    166,
                    Side::Long,
                    ExitReason::Signal,
                    100.874252762,
                    100.208107703,
                    -0.660371740596,
                ),
                (
                    169,
                    175,
                    Side::Long,
                    ExitReason::Signal,
                    100.669997729,
                    100.66417889,
                    -0.00578011322269,
                ),
                (
                    206,
                    207,
                    Side::Long,
                    ExitReason::Signal,
                    94.4085416166,
                    93.6764574541,
                    -0.77544271946,
                ),
                (
                    213,
                    226,
                    Side::Long,
                    ExitReason::Signal,
                    94.2419977144,
                    94.0760241438,
                    -0.17611423203,
                ),
                (
                    263,
                    265,
                    Side::Long,
                    ExitReason::Signal,
                    93.0921832154,
                    91.781029859,
                    -1.40844624227,
                ),
                (
                    294,
                    296,
                    Side::Long,
                    ExitReason::Signal,
                    91.1776759935,
                    90.2691802124,
                    -0.996401554684,
                ),
                (
                    298,
                    299,
                    Side::Long,
                    ExitReason::Signal,
                    90.9205970594,
                    90.1941598944,
                    -0.798979756422,
                ),
                (
                    330,
                    335,
                    Side::Long,
                    ExitReason::Signal,
                    91.4782517581,
                    90.9114222756,
                    -0.619633051206,
                ),
                (
                    360,
                    368,
                    Side::Long,
                    ExitReason::Signal,
                    91.7649992969,
                    91.4230154176,
                    -0.372673548732,
                ),
                (
                    389,
                    393,
                    Side::Long,
                    ExitReason::Signal,
                    89.3130511075,
                    89.0842653313,
                    -0.256161639769,
                ),
                (
                    418,
                    452,
                    Side::Long,
                    ExitReason::Signal,
                    86.2464989334,
                    92.9159504813,
                    7.73301134574,
                ),
                (
                    454,
                    457,
                    Side::Long,
                    ExitReason::Signal,
                    93.4194463933,
                    92.9530049607,
                    -0.499298005484,
                ),
                (
                    465,
                    468,
                    Side::Long,
                    ExitReason::Signal,
                    93.5457056895,
                    93.0704361724,
                    -0.508061287908,
                ),
            ],
        ];
        let zero_equity: &[&[(usize, f64)]] = &[
            &[
                (0, 1.0),
                (1250, 0.8451712917496921),
                (2500, 0.7626271653626101),
                (3750, 0.6271770913032736),
                (4999, 0.68301743533658),
            ],
            &[
                (0, 1.0),
                (1250, 0.9139920507108271),
                (2500, 0.8861798450932198),
                (3750, 0.8037417529209506),
                (4999, 0.8507724064256508),
            ],
            &[
                (0, 1.0),
                (1250, 1.0347973535441755),
                (2500, 0.9796881649043954),
                (3750, 0.866147223705647),
                (4999, 0.8441404475503815),
            ],
            &[
                (0, 1.0),
                (1250, 0.9813503096033039),
                (2500, 0.7537619586885173),
                (3750, 0.8418921632251205),
                (4999, 0.7973302865368027),
            ],
            &[
                (0, 1.0),
                (1250, 1.05532020854209),
                (2500, 1.061937251038),
                (3750, 0.959936775135796),
                (4999, 1.1181983187990745),
            ],
        ];
        let costly_first20: &[&[(usize, usize, f64)]] = &[
            &[(1, 4999, -31.7938120184)],
            &[
                (11, 19, -0.401493474054),
                (22, 25, -0.715596149886),
                (30, 43, 1.88843928444),
                (57, 58, -1.06232875268),
                (60, 62, -0.596422628339),
                (72, 81, 3.85450184871),
                (100, 103, -1.10159430335),
                (111, 112, -0.624996388311),
                (115, 132, 2.05385235154),
                (136, 137, -0.894993537614),
                (157, 166, 0.193611397899),
                (169, 171, -0.278895497595),
                (200, 203, -1.28746706694),
                (210, 212, -0.922409687215),
                (233, 234, -0.420704686326),
                (237, 239, -1.44601270581),
                (250, 262, 0.159021699174),
                (280, 290, 1.14275005542),
                (294, 295, -0.849477570347),
                (298, 299, -0.937764035768),
            ],
            &[
                (153, 182, -2.13710402718),
                (191, 194, -2.13710402718),
                (402, 420, 1.28992611464),
                (543, 560, 1.97513893975),
                (590, 613, 0.999885169624),
                (660, 679, -2.13710402718),
                (688, 700, 2.82629262698),
                (994, 1000, -2.13710402718),
                (1223, 1230, 3.85450184871),
                (1265, 1266, -2.13710402718),
                (1271, 1301, -2.13710402718),
                (1445, 1458, 2.67605636801),
                (1694, 1701, -2.13710402718),
                (1737, 1745, 2.53840928819),
                (1901, 1917, -2.13710402718),
                (1932, 1945, 1.78284962338),
                (1990, 1996, -2.13710402718),
                (2102, 2137, 1.2342115019),
                (2275, 2286, -2.13710402718),
                (2318, 2333, -2.13710402718),
            ],
            &[
                (25, 30, -0.686397454633),
                (45, 46, -0.458999230485),
                (65, 67, 1.86270389272),
                (77, 78, -0.647746604692),
                (93, 100, -0.103093965849),
                (102, 107, 1.86270389272),
                (134, 135, -0.365797578025),
                (137, 142, 1.86270389272),
                (160, 162, -0.493394201101),
                (166, 170, -0.785118058494),
                (175, 178, 1.86270389272),
                (212, 214, -1.14149904933),
                (216, 216, -1.14149904933),
                (224, 230, 1.86270389272),
                (236, 236, -1.14149904933),
                (238, 242, 1.86270389272),
                (262, 262, -1.14149904933),
                (264, 269, 1.86270389272),
                (282, 283, -1.14149904933),
                (291, 293, -1.14149904933),
            ],
            &[
                (22, 25, -0.715596149886),
                (30, 45, 1.24346349199),
                (63, 65, -0.418837150052),
                (78, 93, 2.36227459578),
                (115, 134, 1.31217485587),
                (136, 137, -0.894993537614),
                (158, 160, -0.440923820108),
                (162, 166, -0.799349935423),
                (169, 175, -0.145674095277),
                (206, 207, -0.914259927607),
                (213, 226, -0.315769913128),
                (263, 265, -1.5463778654),
                (294, 296, -1.13490963685),
                (298, 299, -0.937764035768),
                (330, 335, -0.758668240302),
                (360, 368, -0.51205423928),
                (389, 393, -0.395705332889),
                (418, 452, 7.58229063436),
                (454, 457, -0.638501545798),
                (465, 468, -0.647252568208),
            ],
        ];
        let costly_equity: &[&[(usize, f64)]] = &[
            &[
                (0, 1.0),
                (1250, 0.8439888796297265),
                (2500, 0.7615602341829283),
                (3750, 0.6262996575790559),
                (4999, 0.6820618798159733),
            ],
            &[
                (0, 1.0),
                (1250, 0.800168117479919),
                (2500, 0.6773037693976557),
                (3750, 0.5370428835157467),
                (4999, 0.49767366084639625),
            ],
            &[
                (0, 1.0),
                (1250, 1.0218407030872054),
                (2500, 0.9499737160734502),
                (3750, 0.8282001821738593),
                (4999, 0.7981678220463446),
            ],
            &[
                (0, 1.0),
                (1250, 0.8858253994603942),
                (2500, 0.6026562239159837),
                (3750, 0.606965711597418),
                (4999, 0.5087463418279718),
            ],
            &[
                (0, 1.0),
                (1250, 0.9757432636388962),
                (2500, 0.9090954453245429),
                (3750, 0.7770210089368292),
                (4999, 0.8463000617139805),
            ],
        ];
        for (k, spec) in sets.iter().enumerate() {
            let v2 = run_v2(spec, &candles, &zero_cost());
            assert_eq!(v2.trades.len(), counts[k], "set {k} count");
            assert!(
                v2.trades.len() >= 20 || k == 0,
                "set {k} needs >=20, got {}",
                v2.trades.len()
            );
            let exp = zero_first20[k];
            for (got, e) in v2.trades.iter().take(exp.len()).zip(exp.iter()) {
                assert_eq!(got.entry_idx, e.0, "set {k} entry_idx");
                assert_eq!(got.exit_idx, e.1, "set {k} exit_idx");
                assert_eq!(got.side, e.2);
                assert_eq!(got.reason, e.3);
                assert!((got.entry_price - e.4).abs() <= 1e-9, "set {k} entry_price");
                assert!((got.exit_price - e.5).abs() <= 1e-9, "set {k} exit_price");
                assert!((got.return_pct - e.6).abs() <= 1e-9, "set {k} return");
            }
            assert_eq!(v2.equity_curve.len(), 5000);
            for (idx, want) in zero_equity[k] {
                assert!(
                    (v2.equity_curve[*idx] - want).abs() <= 1e-9,
                    "set {k} equity[{idx}]"
                );
            }
            let v2c = run_v2(spec, &candles, &costly());
            assert_costly_golden(&v2c, counts[k], costly_first20[k], costly_equity[k]);
            assert!(
                v2c.trades.len() >= 20 || k == 0,
                "set {k} costly needs >=20"
            );
        }
    }
}
