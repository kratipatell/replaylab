use crate::stats::Stats;
use serde::Serialize;

pub const MIN_OOS_TRADES: usize = 30;
pub const MIN_NOISE_TRADES: usize = 10;
pub const PROFIT_FACTOR_MIN: f64 = 1.2;
pub const RETENTION_RATIO: f64 = 0.5;
pub const SMALL_SAMPLE_TOTAL: usize = 100;
pub const DEFAULT_VERDICT_SPLIT: f64 = 0.7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Grade {
    Holds,
    Fragile,
    Fails,
    NotEnoughData,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verdict {
    pub grade: Grade,
    pub headline: String,
    pub reasons: Vec<String>,
    pub oos_trades: usize,
}

fn profit_factor_text(pf: Option<f64>) -> String {
    match pf {
        Some(v) => format!("{v:.2}"),
        None => "n/a (no losing trades)".to_string(),
    }
}

fn summary_reason(oos: &Stats) -> String {
    format!(
        "Out-of-sample: {} trades, expectancy {:.2}% per trade, profit factor {}.",
        oos.trades,
        oos.expectancy_pct,
        profit_factor_text(oos.profit_factor)
    )
}

// How far the average trade sits from zero, measured in standard errors.
fn noise_reason(oos: &Stats) -> Option<String> {
    if oos.trades < MIN_NOISE_TRADES {
        return None;
    }
    let stderr = oos.expectancy_stdev_pct / (oos.trades as f64).sqrt();
    let avg = oos.expectancy_pct;
    if avg.abs() > 2.0 * stderr {
        return Some(format!(
            "The average of {avg:.2}% per trade is well outside normal noise."
        ));
    }
    Some(format!(
        "With {} trades, the average of {avg:.2}% per trade is within normal noise (about plus or minus {stderr:.2}%).",
        oos.trades
    ))
}

// Small samples can look profitable by chance, so flags stay advisory only.
fn flag_reasons(in_sample: &Stats, oos: &Stats) -> Vec<String> {
    let mut out = Vec::new();
    if oos.total_return_pct < oos.buy_and_hold_pct {
        out.push(format!(
            "Out-of-sample return {:.2}% lagged buy-and-hold {:.2}% over the same window.",
            oos.total_return_pct, oos.buy_and_hold_pct
        ));
    }
    if in_sample.expectancy_pct > 0.0
        && oos.expectancy_pct < RETENTION_RATIO * in_sample.expectancy_pct
    {
        out.push(format!(
            "In-sample expectancy {:.2}% per trade was much higher than out-of-sample {:.2}% per trade.",
            in_sample.expectancy_pct, oos.expectancy_pct
        ));
    }
    out.push(format!(
        "Max out-of-sample drawdown was {:.2}%.",
        oos.max_drawdown_pct
    ));
    let total = in_sample.trades.saturating_add(oos.trades);
    if total < SMALL_SAMPLE_TOTAL {
        out.push(format!(
            "Only {total} total trades across in-sample and out-of-sample, a small sample."
        ));
    }
    out
}

fn holds(oos: &Stats, in_sample: &Stats) -> bool {
    if oos.expectancy_pct <= 0.0 {
        return false;
    }
    let pf_ok = oos.profit_factor.is_none_or(|v| v >= PROFIT_FACTOR_MIN);
    if !pf_ok {
        return false;
    }
    if in_sample.expectancy_pct <= 0.0 {
        return true;
    }
    oos.expectancy_pct >= RETENTION_RATIO * in_sample.expectancy_pct
}

pub fn assess(in_sample: &Stats, out_of_sample: &Stats) -> Verdict {
    let oos_trades = out_of_sample.trades;
    let mut reasons = vec![summary_reason(out_of_sample)];
    if let Some(noise) = noise_reason(out_of_sample) {
        reasons.push(noise);
    }
    reasons.extend(flag_reasons(in_sample, out_of_sample));
    if oos_trades < MIN_OOS_TRADES {
        return Verdict {
            grade: Grade::NotEnoughData,
            headline: format!(
                "Only {oos_trades} out-of-sample trades; at least {MIN_OOS_TRADES} are needed for a verdict."
            ),
            reasons,
            oos_trades,
        };
    }
    if out_of_sample.expectancy_pct <= 0.0 {
        return Verdict {
            grade: Grade::Fails,
            headline: "Lost money out-of-sample on average per trade.".to_string(),
            reasons,
            oos_trades,
        };
    }
    if holds(out_of_sample, in_sample) {
        return Verdict {
            grade: Grade::Holds,
            headline:
                "Held up out-of-sample, but simulated past returns are not a promise of future profit."
                    .to_string(),
            reasons,
            oos_trades,
        };
    }
    Verdict {
        grade: Grade::Fragile,
        headline: "Profitable out-of-sample but too weak or inconsistent to trust.".to_string(),
        reasons,
        oos_trades,
    }
}

pub fn insufficient_split(candle_count: usize, split: f64) -> Verdict {
    let pct = split * 100.0;
    Verdict {
        grade: Grade::NotEnoughData,
        headline: format!(
            "Only {candle_count} candles cannot be split at {pct:.1}%; at least {MIN_OOS_TRADES} out-of-sample trades are needed for a verdict."
        ),
        reasons: vec![format!(
            "Only {candle_count} candles cannot be split at {pct:.1}% for out-of-sample testing."
        )],
        oos_trades: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(
        trades: usize,
        expectancy_pct: f64,
        profit_factor: Option<f64>,
        total_return_pct: f64,
        buy_and_hold_pct: f64,
        max_drawdown_pct: f64,
    ) -> Stats {
        Stats {
            trades,
            wins: 0,
            losses: 0,
            win_rate_pct: 0.0,
            avg_win_pct: 0.0,
            avg_loss_pct: 0.0,
            profit_factor,
            expectancy_pct,
            expectancy_stdev_pct: 0.0,
            total_return_pct,
            buy_and_hold_pct,
            max_drawdown_pct,
            sharpe: 0.0,
        }
    }

    fn neutral_oos(trades: usize, expectancy_pct: f64, profit_factor: Option<f64>) -> Stats {
        stats(trades, expectancy_pct, profit_factor, 5.0, 5.0, 1.0)
    }

    fn neutral_is() -> Stats {
        stats(200, 0.4, Some(1.5), 10.0, 10.0, 1.0)
    }

    #[test]
    fn grade_table() {
        let cases: Vec<(&str, Stats, Stats, Grade)> = vec![
            (
                "oos 29 trades is not enough data",
                neutral_is(),
                neutral_oos(29, 0.5, Some(1.5)),
                Grade::NotEnoughData,
            ),
            (
                "oos 30 trades can hold",
                stats(200, 0.4, Some(1.5), 10.0, 10.0, 1.0),
                stats(30, 0.5, Some(1.5), 5.0, 5.0, 1.0),
                Grade::Holds,
            ),
            (
                "expectancy zero fails",
                neutral_is(),
                neutral_oos(40, 0.0, Some(1.5)),
                Grade::Fails,
            ),
            (
                "expectancy negative fails",
                neutral_is(),
                neutral_oos(40, -0.3, Some(0.75)),
                Grade::Fails,
            ),
            (
                "positive expectancy with pf 1.1 is fragile",
                stats(200, 0.4, Some(1.5), 10.0, 10.0, 1.0),
                stats(40, 0.5, Some(1.1), 5.0, 5.0, 1.0),
                Grade::Fragile,
            ),
            (
                "oos under half of in-sample is fragile",
                stats(200, 2.0, Some(2.0), 20.0, 20.0, 1.0),
                stats(40, 0.9, Some(1.8), 5.0, 5.0, 1.0),
                Grade::Fragile,
            ),
            (
                "holds with room to spare",
                stats(200, 1.0, Some(2.0), 20.0, 20.0, 2.0),
                stats(60, 1.0, Some(1.8), 8.0, 8.0, 2.0),
                Grade::Holds,
            ),
            (
                "pf exactly 1.2 holds",
                stats(200, 1.0, Some(2.0), 20.0, 20.0, 2.0),
                stats(60, 0.8, Some(1.2), 8.0, 8.0, 2.0),
                Grade::Holds,
            ),
            (
                "pf none holds when expectancy positive",
                stats(200, 0.0, None, 0.0, 0.0, 0.0),
                stats(40, 0.5, None, 5.0, 5.0, 0.5),
                Grade::Holds,
            ),
            (
                "oos at exactly half of in-sample holds",
                stats(200, 1.0, Some(2.0), 20.0, 20.0, 1.0),
                stats(40, 0.5, Some(1.5), 5.0, 5.0, 1.0),
                Grade::Holds,
            ),
            (
                "negative in-sample does not block holds",
                stats(200, -0.2, Some(0.9), -2.0, -2.0, 3.0),
                stats(40, 0.3, Some(1.4), 4.0, 4.0, 1.0),
                Grade::Holds,
            ),
        ];
        for (name, is, oos, want) in cases {
            let got = assess(&is, &oos);
            assert_eq!(got.grade, want, "case: {name}");
            assert_eq!(got.oos_trades, oos.trades, "case: {name}");
            assert!(!got.headline.is_empty(), "case: {name}");
            assert!(!got.reasons.is_empty(), "case: {name}");
            for r in &got.reasons {
                assert!(
                    r.chars().any(|c| c.is_ascii_digit()),
                    "reason without numbers in {name}: {r}"
                );
            }
        }
    }

    #[test]
    fn not_enough_data_headline_states_counts() {
        let v = assess(&neutral_is(), &neutral_oos(12, 0.5, Some(1.5)));
        assert_eq!(v.grade, Grade::NotEnoughData);
        assert!(v.headline.contains("12"), "headline: {}", v.headline);
        assert!(v.headline.contains("30"), "headline: {}", v.headline);
    }

    #[test]
    fn flags_never_change_grade() {
        let is = stats(200, 1.0, Some(2.0), 20.0, 20.0, 1.0);
        let clean_oos = stats(60, 0.8, Some(1.6), 9.0, 5.0, 1.0);
        let flagged_oos = stats(60, 0.8, Some(1.6), 1.0, 9.0, 25.0);
        let clean = assess(&is, &clean_oos);
        let flagged = assess(&is, &flagged_oos);
        assert_eq!(clean.grade, Grade::Holds);
        assert_eq!(flagged.grade, Grade::Holds);
        assert!(flagged.reasons.len() > clean.reasons.len());

        let failing_oos = stats(60, -0.2, Some(0.8), 1.0, 9.0, 25.0);
        let failing = assess(&is, &failing_oos);
        assert_eq!(failing.grade, Grade::Fails);

        let thin_is = stats(10, 1.0, Some(2.0), 20.0, 20.0, 1.0);
        let thin_oos = stats(30, 0.8, Some(1.6), 5.0, 5.0, 1.0);
        let thin = assess(&thin_is, &thin_oos);
        assert_eq!(thin.grade, Grade::Holds);
        assert!(
            thin.reasons.iter().any(|r| r.contains("small sample")),
            "expected small-sample flag: {:?}",
            thin.reasons
        );
    }

    #[test]
    fn serialised_shape_uses_snake_case_grade() {
        let v = assess(&neutral_is(), &neutral_oos(5, 0.1, Some(1.1)));
        let json = serde_json::to_value(&v).expect("serialize");
        assert_eq!(json["grade"], "not_enough_data");
        assert!(json["headline"].is_string());
        assert!(json["reasons"].is_array());
        assert_eq!(json["oos_trades"], 5);
        let holds = assess(
            &stats(200, 1.0, Some(2.0), 20.0, 20.0, 1.0),
            &stats(60, 0.8, Some(1.6), 8.0, 8.0, 1.0),
        );
        let json = serde_json::to_value(&holds).expect("serialize");
        assert_eq!(json["grade"], "holds");
    }

    fn oos_with_noise(trades: usize, expectancy_pct: f64, expectancy_stdev_pct: f64) -> Stats {
        Stats {
            trades,
            wins: 0,
            losses: 0,
            win_rate_pct: 0.0,
            avg_win_pct: 0.0,
            avg_loss_pct: 0.0,
            profit_factor: Some(1.5),
            expectancy_pct,
            expectancy_stdev_pct,
            total_return_pct: 5.0,
            buy_and_hold_pct: 5.0,
            max_drawdown_pct: 1.0,
            sharpe: 0.0,
        }
    }

    #[test]
    fn noise_sentence_table() {
        let cases: Vec<(&str, Stats, Option<&str>)> = vec![
            (
                "average within two standard errors",
                oos_with_noise(100, 0.5, 4.0),
                Some("within normal noise"),
            ),
            (
                "average outside two standard errors",
                oos_with_noise(100, 2.0, 4.0),
                Some("well outside normal noise"),
            ),
            (
                "fewer than ten trades has no sentence",
                oos_with_noise(9, 5.0, 1.0),
                None,
            ),
            (
                "zero variance and zero average stays within",
                oos_with_noise(100, 0.0, 0.0),
                Some("within normal noise"),
            ),
            (
                "zero variance constant gain is outside",
                oos_with_noise(100, 1.0, 0.0),
                Some("well outside normal noise"),
            ),
        ];
        for (name, oos, want) in cases {
            let v = assess(&neutral_is(), &oos);
            let noise: Vec<&String> = v.reasons.iter().filter(|r| r.contains("noise")).collect();
            match want {
                None => assert!(noise.is_empty(), "case {name}: {:?}", v.reasons),
                Some(text) => {
                    assert_eq!(noise.len(), 1, "case {name}: {:?}", v.reasons);
                    assert!(noise[0].contains(text), "case {name}: {}", noise[0]);
                    assert!(!noise[0].contains("NaN"), "case {name}: {}", noise[0]);
                }
            }
        }
    }

    #[test]
    fn noise_sentence_matches_examples() {
        let within = assess(&neutral_is(), &oos_with_noise(100, -0.05, 4.0));
        assert!(within.reasons.iter().any(|r| r
            == "With 100 trades, the average of -0.05% per trade is within normal noise (about plus or minus 0.40%)."));
        let outside = assess(&neutral_is(), &oos_with_noise(100, -0.60, 2.0));
        assert!(outside
            .reasons
            .iter()
            .any(|r| r == "The average of -0.60% per trade is well outside normal noise."));
    }

    #[test]
    fn noise_sentence_appears_before_a_verdict_exists() {
        let v = assess(&neutral_is(), &oos_with_noise(12, 0.0, 1.0));
        assert_eq!(v.grade, Grade::NotEnoughData);
        assert!(
            v.reasons.iter().any(|r| r.contains("within normal noise")),
            "reasons: {:?}",
            v.reasons
        );
    }

    #[test]
    fn noise_sentence_never_changes_grade() {
        let quiet = assess(&neutral_is(), &oos_with_noise(60, 0.5, 10.0));
        let loud = assess(&neutral_is(), &oos_with_noise(60, 0.5, 0.1));
        assert_eq!(quiet.grade, Grade::Holds);
        assert_eq!(loud.grade, Grade::Holds);
        assert!(quiet
            .reasons
            .iter()
            .any(|r| r.contains("within normal noise")));
        assert!(loud
            .reasons
            .iter()
            .any(|r| r.contains("well outside normal noise")));
    }

    #[test]
    fn stats_serialise_expectancy_stdev_pct() {
        let oos = oos_with_noise(100, 0.5, 4.0);
        let json = serde_json::to_value(&oos).expect("serialize");
        assert_eq!(json["expectancy_stdev_pct"].as_f64(), Some(4.0));
        assert_eq!(json["expectancy_pct"].as_f64(), Some(0.5));
        assert_eq!(json["trades"].as_u64(), Some(100));
    }
}
