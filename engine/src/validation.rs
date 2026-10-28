use serde::Serialize;

use crate::stats::Stats;
use crate::verdict::{Grade, Verdict};

pub const DEFAULT_SYMBOLS: [&str; 3] = ["BTCUSDT", "ETHUSDT", "SOLUSDT"];
pub const MAX_SYMBOLS: usize = 5;
const MIN_MARKETS: usize = 2;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MarketCheck {
    pub symbol: String,
    pub candle_count: usize,
    pub stats: Stats,
    pub in_sample: Option<Stats>,
    pub out_of_sample: Option<Stats>,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UncheckedMarket {
    pub symbol: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CrossMarket {
    pub grade: Grade,
    pub headline: String,
    pub reasons: Vec<String>,
    pub holds: usize,
    pub tested: usize,
    pub markets: Vec<MarketCheck>,
    pub unchecked: Vec<UncheckedMarket>,
}

fn count(markets: &[MarketCheck], grade: Grade) -> usize {
    markets.iter().filter(|m| m.verdict.grade == grade).count()
}

fn market_reason(market: &MarketCheck) -> String {
    let oos_trades = market.out_of_sample.as_ref().map(|s| s.trades).unwrap_or(0);
    format!(
        "{}: {} on {oos_trades} out-of-sample trades.",
        market.symbol,
        market.verdict.grade.as_str()
    )
}

// The worst checked market decides the grade: a rule that only survives on
// one market is a fit to that market, not a general edge.
pub fn assess(markets: Vec<MarketCheck>, unchecked: Vec<UncheckedMarket>) -> CrossMarket {
    let tested = markets.len();
    let requested = tested + unchecked.len();
    let holds = count(&markets, Grade::Holds);
    let fragile = count(&markets, Grade::Fragile);
    let fails = count(&markets, Grade::Fails);
    let thin = count(&markets, Grade::NotEnoughData);

    let mut reasons: Vec<String> = markets.iter().map(market_reason).collect();
    reasons.extend(
        unchecked
            .iter()
            .map(|u| format!("{}: could not be checked ({}).", u.symbol, u.reason)),
    );

    let (grade, headline) = if tested < MIN_MARKETS {
        (
            Grade::NotEnoughData,
            format!(
                "Only {tested} of {requested} markets produced a result; at least {MIN_MARKETS} are needed to test the same rule elsewhere."
            ),
        )
    } else if fails > 0 {
        (
            Grade::Fails,
            format!(
                "Failed out-of-sample on {fails} of {tested} markets; a rule that only works on one market is a fit to that market."
            ),
        )
    } else if !unchecked.is_empty() {
        (
            Grade::Fragile,
            format!(
                "Held up on {holds} of {tested} markets, but {} of {requested} could not be checked.",
                unchecked.len()
            ),
        )
    } else if fragile > 0 {
        (
            Grade::Fragile,
            format!("Held up on {holds} of {tested} markets and was fragile on {fragile}."),
        )
    } else if thin > 0 {
        if holds == 0 {
            (
                Grade::NotEnoughData,
                "No market produced enough out-of-sample trades for a verdict.".to_string(),
            )
        } else {
            (
                Grade::Fragile,
                format!(
                    "Held up on {holds} of {tested} markets; {thin} did not produce enough out-of-sample trades."
                ),
            )
        }
    } else {
        (
            Grade::Holds,
            format!(
                "Held up out-of-sample on all {tested} markets. Simulated past returns are not a promise of future profit."
            ),
        )
    };

    CrossMarket {
        grade,
        headline,
        reasons,
        holds,
        tested,
        markets,
        unchecked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(trades: usize) -> Stats {
        Stats {
            trades,
            wins: 0,
            losses: 0,
            win_rate_pct: 0.0,
            avg_win_pct: 0.0,
            avg_loss_pct: 0.0,
            profit_factor: Some(1.5),
            expectancy_pct: 0.5,
            expectancy_stdev_pct: 1.0,
            total_return_pct: 5.0,
            buy_and_hold_pct: 5.0,
            max_drawdown_pct: 1.0,
            sharpe: 1.0,
        }
    }

    fn market(symbol: &str, grade: Grade, oos_trades: usize) -> MarketCheck {
        MarketCheck {
            symbol: symbol.to_string(),
            candle_count: 1_000,
            stats: stats(oos_trades),
            in_sample: Some(stats(oos_trades)),
            out_of_sample: Some(stats(oos_trades)),
            verdict: Verdict {
                grade,
                headline: format!("{symbol} verdict"),
                reasons: vec![format!("{symbol} reason 1")],
                oos_trades,
            },
        }
    }

    fn unchecked(symbol: &str) -> UncheckedMarket {
        UncheckedMarket {
            symbol: symbol.to_string(),
            reason: "request failed".to_string(),
        }
    }

    #[test]
    fn all_markets_hold_gives_holds() {
        let got = assess(
            vec![
                market("BTCUSDT", Grade::Holds, 40),
                market("ETHUSDT", Grade::Holds, 35),
                market("SOLUSDT", Grade::Holds, 33),
            ],
            vec![],
        );
        assert_eq!(got.grade, Grade::Holds);
        assert_eq!(got.holds, 3);
        assert_eq!(got.tested, 3);
        assert!(got.headline.contains('3'));
        assert_eq!(got.reasons.len(), 3);
        assert_eq!(got.markets.len(), 3);
        assert!(got.unchecked.is_empty());
    }

    #[test]
    fn one_failing_market_fails_the_whole_check() {
        let got = assess(
            vec![
                market("BTCUSDT", Grade::Holds, 40),
                market("ETHUSDT", Grade::Fails, 40),
            ],
            vec![],
        );
        assert_eq!(got.grade, Grade::Fails);
        assert!(got.headline.contains("1 of 2"));
        assert!(got.reasons.iter().any(|r| r.starts_with("ETHUSDT: fails")));
    }

    #[test]
    fn fragile_market_keeps_the_aggregate_fragile() {
        let got = assess(
            vec![
                market("BTCUSDT", Grade::Holds, 40),
                market("ETHUSDT", Grade::Fragile, 40),
                market("SOLUSDT", Grade::Holds, 40),
            ],
            vec![],
        );
        assert_eq!(got.grade, Grade::Fragile);
        assert_eq!(got.holds, 2);
        assert!(got.headline.contains("2 of 3"));
    }

    #[test]
    fn unchecked_market_caps_a_clean_run_at_fragile() {
        let got = assess(
            vec![
                market("BTCUSDT", Grade::Holds, 40),
                market("ETHUSDT", Grade::Holds, 40),
            ],
            vec![unchecked("SOLUSDT")],
        );
        assert_eq!(got.grade, Grade::Fragile);
        assert_eq!(got.tested, 2);
        assert!(got.headline.contains("1 of 3"));
        assert!(got
            .reasons
            .iter()
            .any(|r| r.starts_with("SOLUSDT: could not be checked")));
    }

    #[test]
    fn fewer_than_two_markets_has_no_verdict() {
        for markets in [vec![], vec![market("BTCUSDT", Grade::Holds, 40)]] {
            let got = assess(markets, vec![]);
            assert_eq!(got.grade, Grade::NotEnoughData);
            assert!(got.headline.contains("at least 2"));
        }
    }

    #[test]
    fn no_market_with_enough_trades_is_not_enough_data() {
        let got = assess(
            vec![
                market("BTCUSDT", Grade::NotEnoughData, 3),
                market("ETHUSDT", Grade::NotEnoughData, 0),
            ],
            vec![],
        );
        assert_eq!(got.grade, Grade::NotEnoughData);
        assert!(got.headline.contains("No market"));
    }

    #[test]
    fn thin_market_next_to_a_hold_is_fragile() {
        let got = assess(
            vec![
                market("BTCUSDT", Grade::Holds, 40),
                market("ETHUSDT", Grade::NotEnoughData, 4),
            ],
            vec![],
        );
        assert_eq!(got.grade, Grade::Fragile);
        assert!(got.headline.contains("1 did not produce"));
    }

    #[test]
    fn order_of_markets_is_preserved() {
        let got = assess(
            vec![
                market("SOLUSDT", Grade::Holds, 40),
                market("BTCUSDT", Grade::Holds, 40),
            ],
            vec![],
        );
        assert_eq!(got.markets[0].symbol, "SOLUSDT");
        assert_eq!(got.markets[1].symbol, "BTCUSDT");
    }

    #[test]
    fn serialised_shape_uses_snake_case_grade_and_counts() {
        let got = assess(
            vec![
                market("BTCUSDT", Grade::Holds, 40),
                market("ETHUSDT", Grade::Fails, 40),
            ],
            vec![unchecked("SOLUSDT")],
        );
        let json = serde_json::to_value(&got).expect("serialize");
        assert_eq!(json["grade"], "fails");
        assert_eq!(json["holds"], 1);
        assert_eq!(json["tested"], 2);
        assert_eq!(json["markets"].as_array().expect("markets").len(), 2);
        assert_eq!(json["unchecked"][0]["symbol"], "SOLUSDT");
        assert_eq!(
            json["markets"][0]["verdict"]["grade"], "holds",
            "per-market verdicts keep their own grade"
        );
    }
}
