use serde::{Deserialize, Serialize};

/// Trade direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Long,
    Short,
}

/// A named indicator reference, e.g. `{"ind":"rsi","period":14}` or `{"ind":"close"}`.
///
/// Price indicators (`open`, `high`, `low`, `close`) carry no period;
/// computed indicators (`sma`, `ema`, `rsi`) carry a period.
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

/// Either a literal number or an indicator reference.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Operand {
    Number(f64),
    Indicator(IndicatorRef),
}

/// Comparison / crossover operator.
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

fn validate_operand(op: &Operand) -> Result<(), String> {
    if let Operand::Indicator(r) = op {
        match r.ind {
            IndicatorKind::Sma | IndicatorKind::Ema | IndicatorKind::Rsi => match r.period {
                None => {
                    return Err(format!(
                        "{} indicator requires a period",
                        indicator_name(r.ind)
                    ));
                }
                Some(0) => {
                    return Err(format!(
                        "{} indicator period must be > 0, got 0",
                        indicator_name(r.ind)
                    ));
                }
                Some(_) => {}
            },
            IndicatorKind::Open
            | IndicatorKind::High
            | IndicatorKind::Low
            | IndicatorKind::Close => {
                if r.period.is_some() {
                    return Err(format!(
                        "{} indicator must not have a period",
                        indicator_name(r.ind)
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_condition(c: &Condition) -> Result<(), String> {
    validate_operand(&c.left)?;
    validate_operand(&c.right)?;
    if matches!(c.left, Operand::Number(_)) && matches!(c.right, Operand::Number(_)) {
        return Err(
            "condition compares two plain numbers (no indicator on either side)".to_string(),
        );
    }
    Ok(())
}

fn validate_pct(name: &str, v: f64) -> Result<(), String> {
    if v.is_nan() {
        return Err(format!("{name} must not be NaN"));
    }
    if v <= 0.0 {
        return Err(format!("{name} must be > 0, got {v}"));
    }
    Ok(())
}

impl Strategy {
    pub fn validate(&self) -> Result<(), String> {
        if self.entry.all.is_empty() && self.entry.any.is_empty() {
            return Err("entry has no conditions (both `all` and `any` are empty)".to_string());
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

        // all[0]: rsi(14) < 30
        assert_eq!(
            s.entry.all[0].left,
            Operand::Indicator(IndicatorRef {
                ind: IndicatorKind::Rsi,
                period: Some(14),
            })
        );
        assert_eq!(s.entry.all[0].op, Op::Lt);
        assert_eq!(s.entry.all[0].right, Operand::Number(30.0));

        // all[1]: close > sma(200)
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
            err.to_lowercase().contains("entry"),
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
        let lower = err.to_lowercase();
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
            err.to_lowercase().contains("period"),
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
        let lower = err.to_lowercase();
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
            err.contains("stop_loss_pct"),
            "error should mention stop_loss_pct, got: {err}"
        );

        let mut s = valid_strategy();
        s.stop_loss_pct = Some(f64::NAN);
        let err = s.validate().expect_err("stop_loss_pct NaN must fail");
        assert!(
            err.contains("stop_loss_pct"),
            "error should mention stop_loss_pct, got: {err}"
        );
    }

    #[test]
    fn validate_rejects_bad_take_profit() {
        let mut s = valid_strategy();
        s.take_profit_pct = Some(-1.0);
        let err = s.validate().expect_err("take_profit_pct <= 0 must fail");
        assert!(
            err.contains("take_profit_pct"),
            "error should mention take_profit_pct, got: {err}"
        );

        let mut s = valid_strategy();
        s.take_profit_pct = Some(f64::NAN);
        let err = s.validate().expect_err("take_profit_pct NaN must fail");
        assert!(
            err.contains("take_profit_pct"),
            "error should mention take_profit_pct, got: {err}"
        );
    }

    #[test]
    fn validate_rejects_two_plain_numbers() {
        let mut s = valid_strategy();
        s.entry.all[0] = cond(Operand::Number(30.0), Op::Lt, Operand::Number(55.0));
        let err = s.validate().expect_err("two plain numbers must fail");
        assert!(
            err.to_lowercase().contains("plain number"),
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
            err.to_lowercase().contains("plain number"),
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
