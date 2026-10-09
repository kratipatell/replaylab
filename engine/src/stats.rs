use crate::backtest::BacktestResult;
use crate::types::Candle;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stats {
    pub trades: usize,
    pub wins: usize,
    pub losses: usize,
    pub win_rate_pct: f64,
    pub avg_win_pct: f64,
    pub avg_loss_pct: f64,
    pub profit_factor: Option<f64>,
    pub expectancy_pct: f64,
    pub total_return_pct: f64,
    pub buy_and_hold_pct: f64,
    pub max_drawdown_pct: f64,
    pub sharpe: f64,
}

fn sharpe_ratio(equity: &[f64], bars_per_year: f64) -> f64 {
    if equity.len() < 2 {
        return 0.0;
    }
    if !bars_per_year.is_finite() || bars_per_year <= 0.0 {
        return 0.0;
    }
    let mut returns = Vec::with_capacity(equity.len() - 1);
    for w in equity.windows(2) {
        let prev = w[0];
        let cur = w[1];
        if !prev.is_finite() || !cur.is_finite() {
            return 0.0;
        }
        if prev == 0.0 {
            return 0.0;
        }
        returns.push(cur / prev - 1.0);
    }
    if returns.is_empty() {
        return 0.0;
    }
    let mean = returns.iter().sum::<f64>() / returns.len() as f64;
    if !mean.is_finite() {
        return 0.0;
    }
    let var = returns.iter().map(|r| (r - mean) * (r - mean)).sum::<f64>() / returns.len() as f64;
    if !var.is_finite() {
        return 0.0;
    }
    let std = var.sqrt();
    if std == 0.0 || !std.is_finite() {
        return 0.0;
    }
    mean / std * bars_per_year.sqrt()
}

/// Returns the number of bars per year for a crypto timeframe.
///
/// Assumes 24/7 crypto markets (365 days per year, no market closes).
///
/// The result is meant to be passed as the `bars_per_year` argument of
/// [`compute_stats`] and [`split_stats`].
pub fn bars_per_year(timeframe: &str) -> Option<f64> {
    match timeframe {
        "1m" => Some(525600.0),
        "5m" => Some(105120.0),
        "15m" => Some(35040.0),
        "1h" => Some(8760.0),
        "4h" => Some(2190.0),
        "1d" => Some(365.0),
        _ => None,
    }
}

pub fn compute_stats(result: &BacktestResult, candles: &[Candle], bars_per_year: f64) -> Stats {
    let trades = result.trades.len();
    let wins = result.trades.iter().filter(|t| t.return_pct > 0.0).count();
    let losses = result.trades.iter().filter(|t| t.return_pct <= 0.0).count();

    let win_rate_pct = if trades == 0 {
        0.0
    } else {
        wins as f64 / trades as f64 * 100.0
    };

    let avg_win_pct = if wins == 0 {
        0.0
    } else {
        result
            .trades
            .iter()
            .filter(|t| t.return_pct > 0.0)
            .map(|t| t.return_pct)
            .sum::<f64>()
            / wins as f64
    };

    let avg_loss_pct = if losses == 0 {
        0.0
    } else {
        result
            .trades
            .iter()
            .filter(|t| t.return_pct <= 0.0)
            .map(|t| t.return_pct)
            .sum::<f64>()
            / losses as f64
    };

    let profit_factor = if losses == 0 {
        None
    } else {
        let sum_win: f64 = result
            .trades
            .iter()
            .filter(|t| t.return_pct > 0.0)
            .map(|t| t.return_pct)
            .sum();
        let sum_loss: f64 = result
            .trades
            .iter()
            .filter(|t| t.return_pct <= 0.0)
            .map(|t| t.return_pct)
            .sum();
        let denom = sum_loss.abs();
        if denom == 0.0 {
            None
        } else {
            Some(sum_win / denom)
        }
    };

    let expectancy_pct = if trades == 0 {
        0.0
    } else {
        result.trades.iter().map(|t| t.return_pct).sum::<f64>() / trades as f64
    };

    let total_return_pct = if result.equity_curve.is_empty() {
        0.0
    } else {
        let first = result.equity_curve[0];
        let last = result.equity_curve[result.equity_curve.len() - 1];
        if first == 0.0 {
            0.0
        } else {
            (last / first - 1.0) * 100.0
        }
    };

    let buy_and_hold_pct = if candles.is_empty() {
        0.0
    } else {
        let first_open = candles[0].open;
        let last_close = candles[candles.len() - 1].close;
        if first_open == 0.0 {
            0.0
        } else {
            (last_close / first_open - 1.0) * 100.0
        }
    };

    let max_drawdown_pct = {
        let mut peak = f64::NEG_INFINITY;
        let mut max_dd = 0.0;
        let mut seen = false;
        for &e in &result.equity_curve {
            if !seen || e > peak {
                peak = e;
                seen = true;
            }
            if peak != 0.0 && peak.is_finite() && e.is_finite() {
                let dd = (peak - e) / peak * 100.0;
                if dd > max_dd {
                    max_dd = dd;
                }
            }
        }
        if !seen {
            0.0
        } else {
            max_dd
        }
    };

    let sharpe = sharpe_ratio(&result.equity_curve, bars_per_year);

    Stats {
        trades,
        wins,
        losses,
        win_rate_pct,
        avg_win_pct,
        avg_loss_pct,
        profit_factor,
        expectancy_pct,
        total_return_pct,
        buy_and_hold_pct,
        max_drawdown_pct,
        sharpe,
    }
}

pub fn split_stats(
    result: &BacktestResult,
    candles: &[Candle],
    split_idx: usize,
    bars_per_year: f64,
) -> Result<(Stats, Stats), String> {
    if split_idx == 0 || split_idx >= candles.len() {
        return Err(format!(
            "invalid split_idx {split_idx} for {} candles",
            candles.len()
        ));
    }

    let is_candles = &candles[..split_idx];
    let oos_candles = &candles[split_idx..];

    let is_trades = result
        .trades
        .iter()
        .filter(|t| t.entry_idx < split_idx)
        .cloned()
        .collect::<Vec<_>>();
    let oos_trades = result
        .trades
        .iter()
        .filter(|t| t.entry_idx >= split_idx)
        .cloned()
        .collect::<Vec<_>>();

    let is_equity: Vec<f64> = result
        .equity_curve
        .iter()
        .take(split_idx)
        .cloned()
        .collect();
    let base = match result.equity_curve.get(split_idx - 1).copied() {
        Some(b) => b,
        None => {
            return Err(format!("missing equity base at split_idx {split_idx}",));
        }
    };
    if base == 0.0 || !base.is_finite() {
        return Err(format!(
            "invalid equity base {base} at split_idx {split_idx}"
        ));
    }
    let mut oos_equity: Vec<f64> = Vec::with_capacity(candles.len() - split_idx + 1);
    oos_equity.push(1.0);
    for &e in result.equity_curve.iter().skip(split_idx) {
        oos_equity.push(e / base);
    }

    let is_result = BacktestResult {
        trades: is_trades,
        equity_curve: is_equity,
    };
    let oos_result = BacktestResult {
        trades: oos_trades,
        equity_curve: oos_equity,
    };

    let is_stats = compute_stats(&is_result, is_candles, bars_per_year);
    let oos_stats = compute_stats(&oos_result, oos_candles, bars_per_year);
    Ok((is_stats, oos_stats))
}

/// One underwater excursion of the equity curve.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DrawdownPeriod {
    pub start_idx: usize,
    pub trough_idx: usize,
    /// Recovery bar index, or `None` when still underwater at the end.
    pub end_idx: Option<usize>,
    pub depth_pct: f64,
    pub length_bars: usize,
}

/// Extended metrics (Engine v2).
///
/// Conventions: risk-free rate is 0; `bars_per_year` annualises Sharpe and
/// Sortino the same way. `calmar` is the simple ratio
/// `total_return_pct / max_drawdown_pct` (0 when there is no drawdown),
/// deliberately *not* CAGR-based so short intraday windows stay meaningful.
/// Monthly and weekday returns compound per-bar equity returns geometrically.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MetricsV2 {
    pub sortino: f64,
    pub calmar: f64,
    pub max_win_streak: usize,
    pub max_loss_streak: usize,
    pub drawdown_periods: Vec<DrawdownPeriod>,
    pub monthly: std::collections::BTreeMap<String, f64>,
    pub by_weekday: std::collections::BTreeMap<String, f64>,
}

pub const WEEKDAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Days since Unix epoch for `ts_ms` (floored).
fn days_since_epoch(ts_ms: i64) -> i64 {
    ts_ms.div_euclid(86_400_000)
}

/// Monday=0..Sunday=6 for `ts_ms`. 1970-01-01 was a Thursday.
fn weekday_idx(ts_ms: i64) -> usize {
    (days_since_epoch(ts_ms) + 3).rem_euclid(7) as usize
}

/// Proleptic Gregorian (year, month, day) for `ts_ms` (UTC).
/// Howard Hinnant's civil-from-days algorithm, no dependencies.
fn ymd(ts_ms: i64) -> (i32, u32, u32) {
    let z = days_since_epoch(ts_ms) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    if m <= 2 {
        y += 1;
    }
    (y as i32, m, d)
}

fn month_key(ts_ms: i64) -> String {
    let (y, m, _) = ymd(ts_ms);
    format!("{y:04}-{m:02}")
}

fn downside_deviation(equity: &[f64], bars_per_year: f64) -> f64 {
    if equity.len() < 2 || !bars_per_year.is_finite() || bars_per_year <= 0.0 {
        return 0.0;
    }
    let mut downside: Vec<f64> = Vec::new();
    for w in equity.windows(2) {
        let (prev, cur) = (w[0], w[1]);
        if !prev.is_finite() || !cur.is_finite() || prev == 0.0 {
            return 0.0;
        }
        let r = cur / prev - 1.0;
        if r < 0.0 {
            downside.push(r);
        }
    }
    if downside.is_empty() {
        return 0.0;
    }
    let mean = downside.iter().sum::<f64>() / downside.len() as f64;
    if !mean.is_finite() {
        return 0.0;
    }
    let var = downside
        .iter()
        .map(|r| (r - mean) * (r - mean))
        .sum::<f64>()
        / downside.len() as f64;
    if !var.is_finite() {
        return 0.0;
    }
    let std = var.sqrt();
    if std == 0.0 || !std.is_finite() {
        return 0.0;
    }
    // Mean excess return over all bars divided by downside deviation.
    let all_mean = {
        let mut sum = 0.0;
        let mut n = 0usize;
        for w in equity.windows(2) {
            sum += w[1] / w[0] - 1.0;
            n += 1;
        }
        sum / n as f64
    };
    all_mean / std * bars_per_year.sqrt()
}

fn streaks(trades: &[crate::backtest::Trade]) -> (usize, usize) {
    let mut max_w = 0usize;
    let mut max_l = 0usize;
    let mut cur_w = 0usize;
    let mut cur_l = 0usize;
    for t in trades {
        if t.return_pct > 0.0 {
            cur_w += 1;
            cur_l = 0;
            if cur_w > max_w {
                max_w = cur_w;
            }
        } else {
            cur_l += 1;
            cur_w = 0;
            if cur_l > max_l {
                max_l = cur_l;
            }
        }
    }
    (max_w, max_l)
}

fn drawdown_periods(equity: &[f64]) -> Vec<DrawdownPeriod> {
    let mut out = Vec::new();
    if equity.is_empty() {
        return out;
    }
    let mut peak = equity[0];
    let mut peak_idx = 0usize;
    let mut trough_idx = 0usize;
    let mut in_dd = false;
    for (i, &e) in equity.iter().enumerate() {
        if !e.is_finite() || !peak.is_finite() {
            continue;
        }
        if e > peak {
            if in_dd {
                let depth = if peak != 0.0 {
                    (peak - equity[trough_idx]) / peak * 100.0
                } else {
                    0.0
                };
                out.push(DrawdownPeriod {
                    start_idx: peak_idx,
                    trough_idx,
                    end_idx: Some(i),
                    depth_pct: depth,
                    length_bars: i - peak_idx,
                });
                in_dd = false;
            }
            peak = e;
            peak_idx = i;
        } else if e < peak {
            if !in_dd {
                in_dd = true;
                trough_idx = i;
            } else if e < equity[trough_idx] {
                trough_idx = i;
            }
        } else if in_dd && e >= peak {
            let depth = if peak != 0.0 {
                (peak - equity[trough_idx]) / peak * 100.0
            } else {
                0.0
            };
            out.push(DrawdownPeriod {
                start_idx: peak_idx,
                trough_idx,
                end_idx: Some(i),
                depth_pct: depth,
                length_bars: i - peak_idx,
            });
            in_dd = false;
        }
    }
    if in_dd {
        let depth = if peak != 0.0 && peak.is_finite() {
            (peak - equity[trough_idx]) / peak * 100.0
        } else {
            0.0
        };
        let last = equity.len() - 1;
        out.push(DrawdownPeriod {
            start_idx: peak_idx,
            trough_idx,
            end_idx: None,
            depth_pct: depth,
            length_bars: last - peak_idx,
        });
    }
    out
}

/// Attribute per-bar equity returns to month and weekday buckets.
///
/// `candles[i].ts` dates bar `i`; the return `equity[i+1]/equity[i]-1`
/// is credited to the month/weekday of bar `i+1`.
fn attribute_buckets(
    equity: &[f64],
    candles: &[Candle],
) -> (
    std::collections::BTreeMap<String, f64>,
    std::collections::BTreeMap<String, f64>,
) {
    use std::collections::BTreeMap;
    let mut monthly_factor: BTreeMap<String, f64> = BTreeMap::new();
    let mut weekday_factor: BTreeMap<String, f64> = BTreeMap::new();
    let n = equity.len().min(candles.len());
    if n < 2 {
        return (BTreeMap::new(), BTreeMap::new());
    }
    for i in 1..n {
        let (prev, cur) = (equity[i - 1], equity[i]);
        if !prev.is_finite() || !cur.is_finite() || prev == 0.0 {
            continue;
        }
        let growth = cur / prev;
        let mk = month_key(candles[i].ts);
        *monthly_factor.entry(mk).or_insert(1.0) *= growth;
        let wd = WEEKDAY_NAMES[weekday_idx(candles[i].ts)].to_string();
        *weekday_factor.entry(wd).or_insert(1.0) *= growth;
    }
    let monthly = monthly_factor
        .into_iter()
        .map(|(k, f)| (k, (f - 1.0) * 100.0))
        .collect();
    let by_weekday = weekday_factor
        .into_iter()
        .map(|(k, f)| (k, (f - 1.0) * 100.0))
        .collect();
    (monthly, by_weekday)
}

pub fn compute_v2(result: &BacktestResult, candles: &[Candle], bars_per_year: f64) -> MetricsV2 {
    let base = compute_stats(result, candles, bars_per_year);
    let sortino = downside_deviation(&result.equity_curve, bars_per_year);
    let calmar = if base.max_drawdown_pct == 0.0 {
        0.0
    } else {
        base.total_return_pct / base.max_drawdown_pct
    };
    let (max_win_streak, max_loss_streak) = streaks(&result.trades);
    let dd = drawdown_periods(&result.equity_curve);
    let (monthly, by_weekday) = attribute_buckets(&result.equity_curve, candles);
    MetricsV2 {
        sortino,
        calmar,
        max_win_streak,
        max_loss_streak,
        drawdown_periods: dd,
        monthly,
        by_weekday,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backtest::{BacktestResult, ExitReason, Trade};
    use crate::strategy::Side;
    use crate::types::Candle;

    fn mk_trade(return_pct: f64, entry_idx: usize) -> Trade {
        Trade {
            entry_idx,
            exit_idx: entry_idx + 1,
            entry_ts: 0,
            exit_ts: 0,
            side: Side::Long,
            entry_price: 100.0,
            exit_price: 100.0,
            return_pct,
            reason: ExitReason::Signal,
        }
    }

    fn mk_candle(open: f64, close: f64) -> Candle {
        Candle {
            ts: 0,
            open,
            high: open.max(close),
            low: open.min(close),
            close,
        }
    }

    fn assert_approx(actual: f64, expected: f64, eps: f64) {
        assert!(
            (actual - expected).abs() <= eps,
            "approx failed: actual {actual} vs expected {expected} (eps={eps})"
        );
    }

    #[test]
    fn mixed_returns_stats() {
        let result = BacktestResult {
            trades: vec![mk_trade(4.0, 0), mk_trade(-2.0, 1), mk_trade(6.0, 2)],
            equity_curve: vec![1.0, 1.04, 1.02, 1.08],
        };
        let candles: Vec<Candle> = Vec::new();
        let stats = compute_stats(&result, &candles, 252.0);
        assert_approx(stats.win_rate_pct, 66.666_666_666_666_67, 1e-6);
        assert_eq!(stats.wins, 2);
        assert_eq!(stats.losses, 1);
        assert_approx(stats.avg_loss_pct, -2.0, 1e-12);
        assert_approx(stats.expectancy_pct, 2.666_666_666_666_666_5, 1e-9);
        let pf = stats.profit_factor.expect("profit_factor should be Some");
        assert_approx(pf, 5.0, 1e-12);
    }

    #[test]
    fn only_winning_trades_profit_factor_none() {
        let result = BacktestResult {
            trades: vec![mk_trade(5.0, 0), mk_trade(3.0, 1)],
            equity_curve: vec![1.0, 1.05, 1.08],
        };
        let stats = compute_stats(&result, &[], 252.0);
        assert!(stats.profit_factor.is_none());
    }

    #[test]
    fn max_drawdown_from_peak() {
        let result = BacktestResult {
            trades: Vec::new(),
            equity_curve: vec![1.0, 1.2, 0.9, 1.1],
        };
        let stats = compute_stats(&result, &[], 252.0);
        assert_approx(stats.max_drawdown_pct, 25.0, 1e-9);
    }

    #[test]
    fn flat_equity_sharpe_and_drawdown_zero() {
        let result = BacktestResult {
            trades: Vec::new(),
            equity_curve: vec![1.0; 5],
        };
        let stats = compute_stats(&result, &[], 252.0);
        assert_approx(stats.sharpe, 0.0, 1e-12);
        assert_approx(stats.max_drawdown_pct, 0.0, 1e-12);
    }

    #[test]
    fn empty_result_and_candles_all_zero() {
        let result = BacktestResult {
            trades: Vec::new(),
            equity_curve: Vec::new(),
        };
        let candles: Vec<Candle> = Vec::new();
        let stats = compute_stats(&result, &candles, 252.0);
        assert_eq!(stats.trades, 0);
        assert_eq!(stats.wins, 0);
        assert_eq!(stats.losses, 0);
        assert_approx(stats.win_rate_pct, 0.0, 1e-12);
        assert_approx(stats.avg_win_pct, 0.0, 1e-12);
        assert_approx(stats.avg_loss_pct, 0.0, 1e-12);
        assert!(stats.profit_factor.is_none());
        assert_approx(stats.expectancy_pct, 0.0, 1e-12);
        assert_approx(stats.total_return_pct, 0.0, 1e-12);
        assert_approx(stats.buy_and_hold_pct, 0.0, 1e-12);
        assert_approx(stats.max_drawdown_pct, 0.0, 1e-12);
        assert_approx(stats.sharpe, 0.0, 1e-12);
    }

    #[test]
    fn split_stats_in_and_out_of_sample() {
        let candles = vec![mk_candle(100.0, 100.0); 10];
        let result = BacktestResult {
            trades: vec![
                mk_trade(1.0, 1),
                mk_trade(1.0, 3),
                mk_trade(1.0, 6),
                mk_trade(1.0, 8),
            ],
            equity_curve: vec![1.0, 1.0, 1.0, 1.0, 2.0, 2.2, 2.2, 2.2, 2.2, 2.2],
        };
        let (is_stats, oos_stats) =
            split_stats(&result, &candles, 5, 252.0).expect("split should succeed");
        assert_eq!(is_stats.trades, 2);
        assert_eq!(oos_stats.trades, 2);
        assert_approx(is_stats.total_return_pct, 100.0, 1e-9);
        assert_approx(oos_stats.total_return_pct, 10.0, 1e-9);
    }

    #[test]
    fn split_stats_rejects_bad_idx() {
        let candles = vec![mk_candle(100.0, 100.0); 10];
        let result = BacktestResult {
            trades: Vec::new(),
            equity_curve: vec![1.0; 10],
        };
        assert!(split_stats(&result, &candles, 0, 252.0).is_err());
        assert!(split_stats(&result, &candles, candles.len(), 252.0).is_err());
    }

    #[test]
    fn stats_serialize_profit_factor_null() {
        let result = BacktestResult {
            trades: vec![mk_trade(5.0, 0), mk_trade(3.0, 1)],
            equity_curve: vec![1.0, 1.05, 1.08],
        };
        let stats = compute_stats(&result, &[], 252.0);
        let json = serde_json::to_string(&stats).expect("serialize");
        assert!(
            json.contains("win_rate_pct"),
            "json missing win_rate_pct: {json}"
        );
        assert!(
            json.contains("\"profit_factor\":null"),
            "json missing null profit_factor: {json}"
        );
    }

    #[test]
    fn total_return_and_buy_and_hold() {
        let result = BacktestResult {
            trades: Vec::new(),
            equity_curve: vec![1.0, 1.1, 1.21],
        };
        let candles = vec![
            mk_candle(100.0, 100.0),
            mk_candle(100.0, 100.0),
            mk_candle(100.0, 150.0),
        ];
        let stats = compute_stats(&result, &candles, 252.0);
        assert_approx(stats.total_return_pct, 21.0, 1e-9);
        assert_approx(stats.buy_and_hold_pct, 50.0, 1e-9);
    }

    #[test]
    fn bars_per_year_known_timeframes() {
        assert_eq!(bars_per_year("1m"), Some(525600.0));
        assert_eq!(bars_per_year("5m"), Some(105120.0));
        assert_eq!(bars_per_year("15m"), Some(35040.0));
        assert_eq!(bars_per_year("1h"), Some(8760.0));
        assert_eq!(bars_per_year("4h"), Some(2190.0));
        assert_eq!(bars_per_year("1d"), Some(365.0));
    }

    #[test]
    fn bars_per_year_unknown_timeframes() {
        assert_eq!(bars_per_year("7m"), None);
        assert_eq!(bars_per_year(""), None);
        assert_eq!(bars_per_year("1H"), None);
    }

    #[test]
    fn bars_per_year_consistency() {
        let cases = [
            ("1m", 1.0),
            ("5m", 5.0),
            ("15m", 15.0),
            ("1h", 60.0),
            ("4h", 240.0),
            ("1d", 1440.0),
        ];
        for (timeframe, bar_minutes) in cases {
            let bpy = bars_per_year(timeframe).expect("known timeframe should return Some");
            assert_approx(bpy * bar_minutes, 525600.0, 1e-9);
        }
    }

    #[test]
    fn bars_per_year_annualisation_scaling() {
        let equity_curve = vec![1.0, 1.01, 1.0, 1.02, 1.01, 1.03];
        let result = BacktestResult {
            trades: Vec::new(),
            equity_curve,
        };
        let bpy_1h = bars_per_year("1h").expect("1h should return Some");
        let bpy_1d = bars_per_year("1d").expect("1d should return Some");
        let sharpe_1h = compute_stats(&result, &[], bpy_1h).sharpe;
        let sharpe_1d = compute_stats(&result, &[], bpy_1d).sharpe;
        let expected = (8760.0_f64 / 365.0).sqrt();
        assert_approx(sharpe_1h / sharpe_1d, expected, 1e-9);
    }

    #[test]
    fn v2_streaks_win_and_loss() {
        let result = BacktestResult {
            trades: vec![
                mk_trade(1.0, 0),
                mk_trade(1.0, 1),
                mk_trade(-1.0, 2),
                mk_trade(-1.0, 3),
                mk_trade(-1.0, 4),
                mk_trade(2.0, 5),
            ],
            equity_curve: vec![1.0; 7],
        };
        let m = compute_v2(&result, &[], 252.0);
        assert_eq!(m.max_win_streak, 2);
        assert_eq!(m.max_loss_streak, 3);
    }

    #[test]
    fn v2_drawdown_period_recovered() {
        // Peak 1.2 at idx 1, trough 0.9 at idx 2, recovered at idx 4.
        let result = BacktestResult {
            trades: Vec::new(),
            equity_curve: vec![1.0, 1.2, 0.9, 1.1, 1.2],
        };
        let m = compute_v2(&result, &[], 252.0);
        assert_eq!(m.drawdown_periods.len(), 1);
        let dd = &m.drawdown_periods[0];
        assert_eq!(dd.start_idx, 1);
        assert_eq!(dd.trough_idx, 2);
        assert_eq!(dd.end_idx, Some(4));
        assert_approx(dd.depth_pct, 25.0, 1e-9);
        assert_eq!(dd.length_bars, 3);
        // Calmar = total 20% / maxDD 25% = 0.8.
        assert_approx(m.calmar, 0.8, 1e-9);
    }

    #[test]
    fn v2_drawdown_open_when_never_recovered() {
        let result = BacktestResult {
            trades: Vec::new(),
            equity_curve: vec![1.0, 1.5, 1.0],
        };
        let m = compute_v2(&result, &[], 252.0);
        assert_eq!(m.drawdown_periods.len(), 1);
        assert_eq!(m.drawdown_periods[0].end_idx, None);
    }

    #[test]
    fn v2_sortino_zero_without_losses() {
        let result = BacktestResult {
            trades: vec![mk_trade(1.0, 0)],
            equity_curve: vec![1.0, 1.01, 1.02],
        };
        let m = compute_v2(&result, &[], 252.0);
        assert_approx(m.sortino, 0.0, 1e-12);
    }

    #[test]
    fn v2_monthly_and_weekday_attribution() {
        // 1970-01-01 is a Thursday. Two bars: equity 1.0 -> 1.1.
        let candles = vec![
            Candle {
                ts: 0,
                open: 100.0,
                high: 100.0,
                low: 100.0,
                close: 100.0,
            },
            Candle {
                ts: 3_600_000,
                open: 100.0,
                high: 100.0,
                low: 100.0,
                close: 100.0,
            },
        ];
        let result = BacktestResult {
            trades: Vec::new(),
            equity_curve: vec![1.0, 1.1],
        };
        let m = compute_v2(&result, &candles, 252.0);
        assert_approx(m.monthly["1970-01"], 10.0, 1e-9);
        assert_approx(m.by_weekday["Thu"], 10.0, 1e-9);
        assert_eq!(m.monthly.len(), 1);
    }

    #[test]
    fn v2_ymd_known_dates() {
        assert_eq!(ymd(0), (1970, 1, 1));
        assert_eq!(weekday_idx(0), 3); // Thursday
                                       // 1970-01-05 is a Monday.
        assert_eq!(weekday_idx(4 * 86_400_000), 0);
    }
}
