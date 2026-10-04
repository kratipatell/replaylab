use serde::Deserialize;

/// Trade direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Long,
    Short,
}

/// A named indicator reference, e.g. `{"ind":"rsi","period":14}` or `{"ind":"close"}`.
///
/// Price indicators (`open`, `high`, `low`, `close`) carry no period;
/// computed indicators (`sma`, `ema`, `rsi`) carry a period.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct IndicatorRef {
    pub ind: IndicatorKind,
    pub period: Option<usize>,
}

/// Either a literal number or an indicator reference.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Operand {
    Number(f64),
    Indicator(IndicatorRef),
}

/// Comparison / crossover operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Condition {
    pub left: Operand,
    pub op: Op,
    pub right: Operand,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Rules {
    #[serde(default)]
    pub all: Vec<Condition>,
    #[serde(default)]
    pub any: Vec<Condition>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Claim {
    pub win_rate_pct: Option<f64>,
    pub quote: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
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
}
