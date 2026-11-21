use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::backtest::BacktestConfig;
use crate::optimizer::{self, Objective, OptimizeSpec, ParamSpec, SearchSpec};
use crate::stats::{self, Stats};
use crate::strategy::Strategy;
use crate::types::Candle;
use crate::verdict::{self, Grade, Verdict};
use std::sync::atomic::{AtomicBool, Ordering};

// Resource and verdict gates: at most 10 windows and 2,000 total parameter
// evaluations; Holds requires >=60% profitable OOS windows, >=60% modal
// parameter stability, and OOS/IS return-per-bar efficiency >=0.5.
// Segment judgment reuses verdict::MIN_OOS_TRADES (30 trades).
const MAX_WINDOWS: usize = 10;
const MAX_TOTAL_OPTIMIZER_EVALUATIONS: usize = optimizer::MAX_TRIALS;
const MAX_PARAMETER_MODAL_SHARE_FOR_HOLDS_PCT: f64 = 60.0;
const MIN_PROFITABLE_OOS_SHARE_FOR_HOLDS_PCT: f64 = 60.0;
const MIN_WALK_FORWARD_EFFICIENCY_FOR_HOLDS: f64 = 0.5;
const INITIAL_EQUITY: f64 = 1.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WalkForwardMode {
    #[default]
    Rolling,
    Anchored,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalkForwardRequest {
    pub strategy: Strategy,
    pub start_ms: i64,
    pub end_ms: i64,
    #[serde(default)]
    pub fee_bps: Option<f64>,
    #[serde(default)]
    pub slippage_bps: Option<f64>,
    pub windows: usize,
    pub is_bars: usize,
    pub oos_bars: usize,
    #[serde(default)]
    pub mode: WalkForwardMode,
    pub params: Vec<ParamSpec>,
    pub search: SearchSpec,
    #[serde(default)]
    pub objective: Option<Objective>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowBounds {
    is_start: usize,
    is_end: usize,
    oos_start: usize,
    oos_end: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum WalkForwardError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Optimizer(#[from] optimizer::OptimizerError),
    #[error(transparent)]
    Backtest(#[from] crate::backtest::BacktestError),
    #[error("walk-forward cancelled")]
    Cancelled,
    #[error(transparent)]
    Stats(#[from] crate::stats::StatsError),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SegmentRange {
    pub start_ms: i64,
    /// The timestamp of the final candle in the segment (inclusive).
    pub end_ms: i64,
    pub bars: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SegmentResult {
    pub range: SegmentRange,
    pub return_pct: f64,
    pub drawdown_pct: f64,
    pub trade_count: usize,
    pub too_few_trades_to_judge: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WalkForwardWindow {
    pub window: usize,
    pub in_sample: SegmentResult,
    pub out_of_sample: SegmentResult,
    pub chosen_parameters: BTreeMap<String, f64>,
    pub optimizer_score: f64,
    pub oos_starting_equity: f64,
    pub oos_ending_equity: f64,
    #[serde(skip)]
    pub is_stats: Stats,
    #[serde(skip)]
    pub oos_stats: Stats,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WalkForwardEquityPoint {
    pub bar_ts: i64,
    pub equity: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParameterStability {
    pub parameter: String,
    pub modal_value: f64,
    pub modal_windows: usize,
    pub windows: usize,
    pub modal_share_pct: f64,
    pub unstable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WalkForwardReport {
    pub mode: WalkForwardMode,
    pub objective: String,
    pub window_count: usize,
    pub total_optimizer_evaluations: usize,
    pub windows: Vec<WalkForwardWindow>,
    pub stitched_oos_equity_curve: Vec<WalkForwardEquityPoint>,
    pub total_oos_return_pct: f64,
    pub max_oos_drawdown_pct: f64,
    pub profitable_oos_windows: usize,
    pub profitable_oos_share_pct: f64,
    pub mean_is_return_per_bar_pct: f64,
    pub mean_oos_return_per_bar_pct: f64,
    pub efficiency_ratio: Option<f64>,
    pub efficiency_ratio_definition: &'static str,
    pub parameter_stability: Vec<ParameterStability>,
    pub stability_measure: &'static str,
    pub verdict: Verdict,
}

fn optimize_spec(request: &WalkForwardRequest) -> OptimizeSpec {
    OptimizeSpec {
        strategy: request.strategy.clone(),
        start_ms: request.start_ms,
        end_ms: request.end_ms,
        fee_bps: request.fee_bps,
        slippage_bps: request.slippage_bps,
        params: request.params.clone(),
        search: request.search.clone(),
        validation: None,
        objective: request.objective,
    }
}

fn required_bars(request: &WalkForwardRequest) -> Result<usize, WalkForwardError> {
    request
        .oos_bars
        .checked_mul(request.windows)
        .and_then(|oos| request.is_bars.checked_add(oos))
        .ok_or_else(|| WalkForwardError::Invalid("requested window lengths overflow".to_string()))
}

pub fn validate_request(request: &WalkForwardRequest) -> Result<usize, WalkForwardError> {
    if request.windows < 2 || request.windows > MAX_WINDOWS {
        return Err(WalkForwardError::Invalid(format!(
            "windows must be in 2..={MAX_WINDOWS}"
        )));
    }
    if request.is_bars == 0 || request.oos_bars == 0 {
        return Err(WalkForwardError::Invalid(
            "is_bars and oos_bars must be positive".to_string(),
        ));
    }
    if request.start_ms >= request.end_ms {
        return Err(WalkForwardError::Invalid(
            "start_ms must be less than end_ms".to_string(),
        ));
    }
    let spec = optimize_spec(request);
    let trials = optimizer::build_combos(&spec)?.len();
    let total = trials.checked_mul(request.windows).ok_or_else(|| {
        WalkForwardError::Invalid("optimizer evaluation count overflow".to_string())
    })?;
    if total > MAX_TOTAL_OPTIMIZER_EVALUATIONS {
        return Err(WalkForwardError::Invalid(format!(
            "walk-forward request needs {total} optimizer evaluations; maximum is {MAX_TOTAL_OPTIMIZER_EVALUATIONS}"
        )));
    }
    required_bars(request)?;
    Ok(total)
}

fn window_bounds(
    candle_count: usize,
    request: &WalkForwardRequest,
) -> Result<Vec<WindowBounds>, WalkForwardError> {
    let required = required_bars(request)?;
    if candle_count < required {
        return Err(WalkForwardError::Invalid(format!(
            "not enough candles ({candle_count}) for {} windows of {} IS and {} OOS bars (need {required})",
            request.windows, request.is_bars, request.oos_bars
        )));
    }
    let offset = candle_count - required;
    (0..request.windows)
        .map(|window| {
            let oos_offset = window
                .checked_mul(request.oos_bars)
                .ok_or_else(|| WalkForwardError::Invalid("window boundary overflow".to_string()))?;
            let (is_start, is_end) = match request.mode {
                WalkForwardMode::Rolling => {
                    let start = offset.checked_add(oos_offset).ok_or_else(|| {
                        WalkForwardError::Invalid("window boundary overflow".to_string())
                    })?;
                    let end = start.checked_add(request.is_bars).ok_or_else(|| {
                        WalkForwardError::Invalid("window boundary overflow".to_string())
                    })?;
                    (start, end)
                }
                WalkForwardMode::Anchored => {
                    let end = offset
                        .checked_add(request.is_bars)
                        .and_then(|end| end.checked_add(oos_offset))
                        .ok_or_else(|| {
                            WalkForwardError::Invalid("window boundary overflow".to_string())
                        })?;
                    (offset, end)
                }
            };
            let oos_end = is_end
                .checked_add(request.oos_bars)
                .ok_or_else(|| WalkForwardError::Invalid("window boundary overflow".to_string()))?;
            Ok(WindowBounds {
                is_start,
                is_end,
                oos_start: is_end,
                oos_end,
            })
        })
        .collect()
}

fn segment_result(stats: Stats, candles: &[Candle], start: usize, end: usize) -> SegmentResult {
    SegmentResult {
        range: SegmentRange {
            start_ms: candles[start].ts,
            end_ms: candles[end - 1].ts,
            bars: end - start,
        },
        return_pct: stats.total_return_pct,
        drawdown_pct: stats.max_drawdown_pct,
        too_few_trades_to_judge: stats.trades < verdict::MIN_OOS_TRADES,
        trade_count: stats.trades,
    }
}

fn modal_stability(parameter: &str, values: &[f64]) -> Option<ParameterStability> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mut modal_value = sorted[0];
    let mut modal_windows = 0;
    for &candidate in &sorted {
        let count = sorted.iter().filter(|&&value| value == candidate).count();
        if count > modal_windows {
            modal_value = candidate;
            modal_windows = count;
        }
    }
    let modal_share_pct = modal_windows as f64 / sorted.len() as f64 * 100.0;
    Some(ParameterStability {
        parameter: parameter.to_string(),
        modal_value,
        modal_windows,
        windows: sorted.len(),
        modal_share_pct,
        unstable: modal_share_pct < MAX_PARAMETER_MODAL_SHARE_FOR_HOLDS_PCT,
    })
}

fn parameter_stability(
    windows: &[WalkForwardWindow],
    params: &[ParamSpec],
) -> Vec<ParameterStability> {
    params
        .iter()
        .filter_map(|param| {
            let values: Vec<f64> = windows
                .iter()
                .filter_map(|window| window.chosen_parameters.get(&param.target).copied())
                .collect();
            modal_stability(&param.target, &values)
        })
        .collect()
}

fn stitched_drawdown(curve: &[WalkForwardEquityPoint]) -> f64 {
    let mut peak = f64::NEG_INFINITY;
    let mut max_drawdown: f64 = 0.0;
    for point in curve {
        peak = peak.max(point.equity);
        if peak > 0.0 && peak.is_finite() && point.equity.is_finite() {
            max_drawdown = max_drawdown.max((peak - point.equity) / peak * 100.0);
        }
    }
    max_drawdown
}

fn efficiency_ratio(is_per_bar_pct: f64, oos_per_bar_pct: f64) -> Option<f64> {
    if !is_per_bar_pct.is_finite() || !oos_per_bar_pct.is_finite() || is_per_bar_pct <= 0.0 {
        return None;
    }
    let ratio = oos_per_bar_pct / is_per_bar_pct;
    ratio.is_finite().then_some(ratio)
}

fn walk_forward_verdict(
    windows: &[WalkForwardWindow],
    parameter_stability: &[ParameterStability],
    profitable_share_pct: f64,
    efficiency_ratio: Option<f64>,
) -> Verdict {
    let is_stats: Vec<Stats> = windows
        .iter()
        .map(|window| window.is_stats.clone())
        .collect();
    let oos_stats: Vec<Stats> = windows
        .iter()
        .map(|window| window.oos_stats.clone())
        .collect();
    let mean_is = optimizer::mean_stats(&is_stats);
    let mean_oos = optimizer::mean_stats(&oos_stats);
    let mut base = verdict::assess(&mean_is, &mean_oos);
    let thin_window_count = windows
        .iter()
        .filter(|window| window.out_of_sample.too_few_trades_to_judge)
        .count();
    let unstable: Vec<&str> = parameter_stability
        .iter()
        .filter(|param| param.unstable)
        .map(|param| param.parameter.as_str())
        .collect();
    if thin_window_count > 0 {
        base.reasons.push(format!(
            "{thin_window_count} of {} OOS windows had fewer than {} trades.",
            windows.len(),
            verdict::MIN_OOS_TRADES
        ));
    }
    let thin_is_window_count = windows
        .iter()
        .filter(|window| window.in_sample.too_few_trades_to_judge)
        .count();
    if thin_is_window_count > 0 {
        base.reasons.push(format!(
            "{thin_is_window_count} of {} IS windows had fewer than {} trades.",
            windows.len(),
            verdict::MIN_OOS_TRADES
        ));
    }
    if !unstable.is_empty() {
        base.reasons.push(format!(
            "Unstable parameters (modal selection share below {MAX_PARAMETER_MODAL_SHARE_FOR_HOLDS_PCT:.0}%): {}.",
            unstable.join(", ")
        ));
    }
    if base.grade == Grade::Holds
        && (thin_window_count > 0
            || thin_is_window_count > 0
            || profitable_share_pct < MIN_PROFITABLE_OOS_SHARE_FOR_HOLDS_PCT
            || efficiency_ratio.is_none_or(|ratio| ratio < MIN_WALK_FORWARD_EFFICIENCY_FOR_HOLDS)
            || !unstable.is_empty())
    {
        base.grade = Grade::Fragile;
        base.headline = "Walk-forward results were positive overall but inconsistent across windows or parameters.".to_string();
    }
    base
}

#[cfg(test)]
pub fn run(
    request: &WalkForwardRequest,
    candles: &[Candle],
    cfg: &BacktestConfig,
    bars_per_year: f64,
) -> Result<WalkForwardReport, WalkForwardError> {
    run_cancellable(request, candles, cfg, bars_per_year, None)
}

pub(crate) fn run_cancellable(
    request: &WalkForwardRequest,
    candles: &[Candle],
    cfg: &BacktestConfig,
    bars_per_year: f64,
    cancelled: Option<&AtomicBool>,
) -> Result<WalkForwardReport, WalkForwardError> {
    run_cancellable_with_hook(request, candles, cfg, bars_per_year, cancelled, None)
}

fn run_cancellable_with_hook(
    request: &WalkForwardRequest,
    candles: &[Candle],
    cfg: &BacktestConfig,
    bars_per_year: f64,
    cancelled: Option<&AtomicBool>,
    on_trial_start: Option<&(dyn Fn() + Sync)>,
) -> Result<WalkForwardReport, WalkForwardError> {
    validate_request(request)?;
    if cancellation_requested(cancelled) {
        return Err(WalkForwardError::Cancelled);
    }
    if candles.is_empty() {
        return Err(WalkForwardError::Invalid("no candles in range".to_string()));
    }
    let boundaries = window_bounds(candles.len(), request)?;
    let optimizer_spec = optimize_spec(request);
    let objective = optimizer_spec
        .objective
        .unwrap_or_default()
        .as_str()
        .to_string();
    let mut windows = Vec::with_capacity(boundaries.len());
    let first_oos = boundaries[0].oos_start;
    let mut equity_curve = vec![WalkForwardEquityPoint {
        bar_ts: candles[first_oos - 1].ts,
        equity: INITIAL_EQUITY,
    }];
    let mut starting_equity = INITIAL_EQUITY;
    let mut total_optimizer_evaluations = 0;

    for (index, bounds) in boundaries.iter().enumerate() {
        if cancellation_requested(cancelled) {
            return Err(WalkForwardError::Cancelled);
        }
        let evaluated = match on_trial_start {
            Some(on_trial_start) => optimizer::optimize_window_cancellable_with_hook(
                &optimizer_spec,
                &candles[..bounds.is_end],
                cfg,
                bars_per_year,
                bounds.is_start,
                bounds.is_end,
                cancelled,
                Some(on_trial_start),
            ),
            None => optimizer::optimize_window_cancellable(
                &optimizer_spec,
                &candles[..bounds.is_end],
                cfg,
                bars_per_year,
                bounds.is_start,
                bounds.is_end,
                cancelled,
            ),
        };
        let optimized = match evaluated {
            Ok(optimized) => optimized,
            Err(optimizer::OptimizerError::Cancelled) => {
                return Err(WalkForwardError::Cancelled);
            }
            Err(error) => return Err(error.into()),
        };
        total_optimizer_evaluations += optimized.trials;
        let oos_result = crate::backtest::backtest_strategy_window(
            &optimized.strategy,
            candles,
            cfg,
            bounds.oos_start,
            bounds.oos_end,
            starting_equity,
        )?;
        let oos_stats = stats::window_stats(
            &oos_result,
            candles,
            bounds.oos_start,
            bounds.oos_end,
            bars_per_year,
        )?;
        let ending_equity = *oos_result
            .equity_curve
            .get(bounds.oos_end - 1)
            .ok_or_else(|| WalkForwardError::Invalid("missing OOS equity point".to_string()))?;
        for (idx, candle) in candles
            .iter()
            .enumerate()
            .take(bounds.oos_end)
            .skip(bounds.oos_start)
        {
            let equity = *oos_result
                .equity_curve
                .get(idx)
                .ok_or_else(|| WalkForwardError::Invalid("missing OOS equity point".to_string()))?;
            equity_curve.push(WalkForwardEquityPoint {
                bar_ts: candle.ts,
                equity,
            });
        }
        let is_stats = optimized.stats;
        windows.push(WalkForwardWindow {
            window: index + 1,
            in_sample: segment_result(is_stats.clone(), candles, bounds.is_start, bounds.is_end),
            out_of_sample: segment_result(
                oos_stats.clone(),
                candles,
                bounds.oos_start,
                bounds.oos_end,
            ),
            chosen_parameters: optimized.params,
            optimizer_score: optimized.score,
            oos_starting_equity: starting_equity,
            oos_ending_equity: ending_equity,
            is_stats,
            oos_stats,
        });
        if index + 1 < boundaries.len() && (!ending_equity.is_finite() || ending_equity <= 0.0) {
            return Err(WalkForwardError::Invalid(
                "OOS equity must remain positive to stitch the next window".to_string(),
            ));
        }
        starting_equity = ending_equity;
    }

    if cancellation_requested(cancelled) {
        return Err(WalkForwardError::Cancelled);
    }

    let first_equity = equity_curve
        .first()
        .map(|point| point.equity)
        .unwrap_or(INITIAL_EQUITY);
    let final_equity = equity_curve
        .last()
        .map(|point| point.equity)
        .unwrap_or(first_equity);
    let total_oos_return_pct = if first_equity > 0.0 {
        (final_equity / first_equity - 1.0) * 100.0
    } else {
        0.0
    };
    let max_oos_drawdown_pct = stitched_drawdown(&equity_curve);
    let profitable_oos_windows = windows
        .iter()
        .filter(|window| window.out_of_sample.return_pct > 0.0)
        .count();
    let profitable_oos_share_pct = profitable_oos_windows as f64 / windows.len() as f64 * 100.0;
    let mean_is_return_per_bar_pct = windows
        .iter()
        .map(|window| window.in_sample.return_pct / window.in_sample.range.bars as f64)
        .sum::<f64>()
        / windows.len() as f64;
    let mean_oos_return_per_bar_pct = windows
        .iter()
        .map(|window| window.out_of_sample.return_pct / window.out_of_sample.range.bars as f64)
        .sum::<f64>()
        / windows.len() as f64;
    let efficiency_ratio =
        efficiency_ratio(mean_is_return_per_bar_pct, mean_oos_return_per_bar_pct);
    let stability = parameter_stability(&windows, &request.params);
    let verdict = walk_forward_verdict(
        &windows,
        &stability,
        profitable_oos_share_pct,
        efficiency_ratio,
    );
    Ok(WalkForwardReport {
        mode: request.mode,
        objective,
        window_count: windows.len(),
        total_optimizer_evaluations,
        windows,
        stitched_oos_equity_curve: equity_curve,
        total_oos_return_pct,
        max_oos_drawdown_pct,
        profitable_oos_windows,
        profitable_oos_share_pct,
        mean_is_return_per_bar_pct,
        mean_oos_return_per_bar_pct,
        efficiency_ratio,
        efficiency_ratio_definition: "Mean OOS window return per bar divided by mean IS window return per bar; null when mean IS return per bar is non-positive or non-finite." ,
        parameter_stability: stability,
        stability_measure: "Per parameter, the share of windows selecting its modal exact value; lower shares indicate more parameter movement.",
        verdict,
    })
}

fn cancellation_requested(cancelled: Option<&AtomicBool>) -> bool {
    cancelled.is_some_and(|flag| flag.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::optimizer::{ParamKind, SearchMethod};
    use crate::strategy::{Condition, IndicatorKind, IndicatorRef, Op, Operand, Rules, Side};

    fn candles(count: usize) -> Vec<Candle> {
        let mut previous = 100.0;
        (0..count)
            .map(|idx| {
                let close = 100.0 + ((idx as f64) * 0.73).sin() * 4.0;
                let open = previous;
                previous = close;
                Candle {
                    ts: idx as i64 * 3_600_000,
                    open,
                    high: open.max(close) + 0.5,
                    low: open.min(close) - 0.5,
                    close,
                }
            })
            .collect()
    }

    fn strategy() -> Strategy {
        Strategy {
            asset: "BTCUSDT".to_string(),
            timeframe: "1h".to_string(),
            side: Side::Long,
            entry: Rules {
                all: vec![Condition {
                    left: Operand::Indicator(IndicatorRef {
                        ind: IndicatorKind::Close,
                        period: None,
                    }),
                    op: Op::Gt,
                    right: Operand::Number(100.0),
                }],
                any: vec![],
            },
            exit: Some(Rules {
                all: vec![],
                any: vec![Condition {
                    left: Operand::Indicator(IndicatorRef {
                        ind: IndicatorKind::Close,
                        period: None,
                    }),
                    op: Op::Lt,
                    right: Operand::Number(100.0),
                }],
            }),
            stop_loss_pct: None,
            take_profit_pct: None,
            claim: None,
        }
    }

    fn request(
        mode: WalkForwardMode,
        windows: usize,
        is_bars: usize,
        oos_bars: usize,
    ) -> WalkForwardRequest {
        WalkForwardRequest {
            strategy: strategy(),
            start_ms: 0,
            end_ms: 10_000_000,
            fee_bps: None,
            slippage_bps: None,
            windows,
            is_bars,
            oos_bars,
            mode,
            params: vec![ParamSpec {
                target: "entry.all.0.right".to_string(),
                kind: Some(ParamKind::Float),
                values: Some(vec![99.0, 100.0, 101.0]),
                min: None,
                max: None,
                step: None,
            }],
            search: SearchSpec {
                method: SearchMethod::Grid,
                trials: None,
                seed: None,
            },
            objective: Some(Objective::TotalReturn),
        }
    }

    fn cfg() -> BacktestConfig {
        BacktestConfig::default()
    }

    fn stats(trades: usize, expectancy: f64, pf: Option<f64>, total_return: f64) -> Stats {
        Stats {
            trades,
            wins: trades / 2,
            losses: trades - trades / 2,
            win_rate_pct: 50.0,
            avg_win_pct: expectancy,
            avg_loss_pct: -expectancy,
            profit_factor: pf,
            expectancy_pct: expectancy,
            expectancy_stdev_pct: 0.0,
            total_return_pct: total_return,
            buy_and_hold_pct: 0.0,
            max_drawdown_pct: 2.0,
            sharpe: 1.0,
        }
    }

    fn sample_window(is: Stats, oos: Stats, value: f64) -> WalkForwardWindow {
        WalkForwardWindow {
            window: 1,
            in_sample: SegmentResult {
                range: SegmentRange {
                    start_ms: 0,
                    end_ms: 9,
                    bars: 10,
                },
                return_pct: is.total_return_pct,
                drawdown_pct: is.max_drawdown_pct,
                trade_count: is.trades,
                too_few_trades_to_judge: is.trades < verdict::MIN_OOS_TRADES,
            },
            out_of_sample: SegmentResult {
                range: SegmentRange {
                    start_ms: 10,
                    end_ms: 19,
                    bars: 10,
                },
                return_pct: oos.total_return_pct,
                drawdown_pct: oos.max_drawdown_pct,
                trade_count: oos.trades,
                too_few_trades_to_judge: oos.trades < verdict::MIN_OOS_TRADES,
            },
            chosen_parameters: BTreeMap::from([("p".to_string(), value)]),
            optimizer_score: 0.0,
            oos_starting_equity: 1.0,
            oos_ending_equity: 1.0,
            is_stats: is,
            oos_stats: oos,
        }
    }

    fn stable_parameter(unstable: bool) -> ParameterStability {
        ParameterStability {
            parameter: "p".to_string(),
            modal_value: 100.0,
            modal_windows: if unstable { 1 } else { 2 },
            windows: 2,
            modal_share_pct: if unstable { 50.0 } else { 100.0 },
            unstable,
        }
    }

    #[test]
    fn rolling_and_anchored_boundaries_and_candle_cap() {
        let rolling = request(WalkForwardMode::Rolling, 3, 10, 5);
        let rolling_bounds = window_bounds(40, &rolling).expect("rolling windows");
        assert_eq!(
            rolling_bounds,
            vec![
                WindowBounds {
                    is_start: 15,
                    is_end: 25,
                    oos_start: 25,
                    oos_end: 30
                },
                WindowBounds {
                    is_start: 20,
                    is_end: 30,
                    oos_start: 30,
                    oos_end: 35
                },
                WindowBounds {
                    is_start: 25,
                    is_end: 35,
                    oos_start: 35,
                    oos_end: 40
                },
            ]
        );
        let anchored = request(WalkForwardMode::Anchored, 3, 10, 5);
        let anchored_bounds = window_bounds(40, &anchored).expect("anchored windows");
        assert_eq!(
            anchored_bounds,
            vec![
                WindowBounds {
                    is_start: 15,
                    is_end: 25,
                    oos_start: 25,
                    oos_end: 30
                },
                WindowBounds {
                    is_start: 15,
                    is_end: 30,
                    oos_start: 30,
                    oos_end: 35
                },
                WindowBounds {
                    is_start: 15,
                    is_end: 35,
                    oos_start: 35,
                    oos_end: 40
                },
            ]
        );
        assert!(window_bounds(24, &rolling).is_err());
    }

    #[test]
    fn tampering_with_oos_candles_cannot_change_that_windows_is_parameters() {
        let request = request(WalkForwardMode::Rolling, 2, 20, 10);
        let original = candles(40);
        let bounds = window_bounds(original.len(), &request).expect("bounds")[0].clone();
        let spec = optimize_spec(&request);
        let before = optimizer::optimize_window(
            &spec,
            &original[..bounds.is_end],
            &cfg(),
            8_760.0,
            bounds.is_start,
            bounds.is_end,
        )
        .expect("optimize original IS");

        let mut tampered = original.clone();
        for candle in &mut tampered[bounds.oos_start..bounds.oos_end] {
            candle.open += 1.0;
            candle.close += 1.0;
            candle.high += 1.0;
            candle.low += 1.0;
        }
        let after = optimizer::optimize_window(
            &spec,
            &tampered[..bounds.is_end],
            &cfg(),
            8_760.0,
            bounds.is_start,
            bounds.is_end,
        )
        .expect("optimize tampered IS prefix");
        assert_eq!(before.params, after.params);

        let original_report = run(&request, &original, &cfg(), 8_760.0).expect("original report");
        let tampered_report = run(&request, &tampered, &cfg(), 8_760.0).expect("tampered report");
        assert_eq!(
            original_report.windows[0].chosen_parameters,
            tampered_report.windows[0].chosen_parameters
        );
    }

    #[test]
    fn stitched_oos_equity_starts_each_window_at_the_previous_end() {
        let request = request(WalkForwardMode::Rolling, 3, 12, 8);
        let candles = candles(44);
        let report = run(&request, &candles, &cfg(), 8_760.0).expect("walk forward");
        assert_eq!(report.stitched_oos_equity_curve.len(), 1 + 3 * 8);
        assert_eq!(report.stitched_oos_equity_curve[0].equity, INITIAL_EQUITY);
        for pair in report.windows.windows(2) {
            assert_eq!(pair[0].oos_ending_equity, pair[1].oos_starting_equity);
        }
        assert_eq!(
            report
                .stitched_oos_equity_curve
                .last()
                .map(|point| point.equity),
            report.windows.last().map(|window| window.oos_ending_equity)
        );
    }

    #[test]
    fn efficiency_ratio_is_defined_only_for_positive_is_return_per_bar() {
        assert_eq!(efficiency_ratio(2.0, 1.0), Some(0.5));
        assert_eq!(efficiency_ratio(0.0, 1.0), None);
        assert_eq!(efficiency_ratio(-1.0, 1.0), None);
        assert_eq!(efficiency_ratio(f64::NAN, 1.0), None);
    }

    #[test]
    fn parameter_stability_uses_the_modal_selection_share() {
        let windows = [1.0, 1.0, 2.0]
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                let mut window = sample_window(
                    stats(40, 0.5, Some(1.5), 1.0),
                    stats(40, 0.5, Some(1.5), 1.0),
                    value,
                );
                window.window = index + 1;
                window
            })
            .collect::<Vec<_>>();
        let mut param = request(WalkForwardMode::Rolling, 3, 10, 5).params[0].clone();
        param.target = "p".to_string();
        let stability = parameter_stability(&windows, &[param]);
        assert_eq!(stability[0].modal_value, 1.0);
        assert!((stability[0].modal_share_pct - 200.0 / 3.0).abs() < 1e-9);
        assert!(!stability[0].unstable);

        let unstable = modal_stability("p", &[1.0, 2.0, 3.0]).expect("stability");
        assert!((unstable.modal_share_pct - 100.0 / 3.0).abs() < 1e-9);
        assert!(unstable.unstable);
        assert!(
            modal_stability("p", &[1.0, 2.0])
                .expect("two-window stability")
                .unstable
        );
    }

    #[test]
    fn walk_forward_verdict_can_reach_each_grade() {
        let is = stats(60, 0.5, Some(1.5), 5.0);
        let profitable = stats(40, 0.5, Some(1.5), 5.0);
        let stable = stable_parameter(false);
        let holds = walk_forward_verdict(
            &[
                sample_window(is.clone(), profitable.clone(), 1.0),
                sample_window(is.clone(), profitable.clone(), 1.0),
            ],
            std::slice::from_ref(&stable),
            100.0,
            Some(1.0),
        );
        assert_eq!(holds.grade, Grade::Holds);

        let fragile = walk_forward_verdict(
            &[
                sample_window(is.clone(), profitable.clone(), 1.0),
                sample_window(is.clone(), profitable.clone(), 2.0),
            ],
            &[stable_parameter(true)],
            100.0,
            Some(1.0),
        );
        assert_eq!(fragile.grade, Grade::Fragile);

        let fails = walk_forward_verdict(
            &[sample_window(
                is.clone(),
                stats(40, -0.2, Some(0.8), -5.0),
                1.0,
            )],
            std::slice::from_ref(&stable),
            0.0,
            Some(-1.0),
        );
        assert_eq!(fails.grade, Grade::Fails);

        let thin = walk_forward_verdict(
            &[sample_window(is, stats(10, 0.5, Some(1.5), 1.0), 1.0)],
            &[stable],
            100.0,
            Some(1.0),
        );
        assert_eq!(thin.grade, Grade::NotEnoughData);
    }

    #[test]
    fn repeated_request_is_deterministic() {
        let request = request(WalkForwardMode::Anchored, 2, 30, 10);
        let candles = candles(50);
        let first = run(&request, &candles, &cfg(), 8_760.0).expect("first run");
        let second = run(&request, &candles, &cfg(), 8_760.0).expect("second run");
        assert_eq!(first, second);
    }

    #[test]
    fn cancellation_drops_walk_forward_result_and_retry_is_unchanged() {
        let mut request = request(WalkForwardMode::Rolling, 2, 60, 30);
        request.params[0].values = Some((0..128).map(|idx| 90.0 + idx as f64 * 0.25).collect());
        let candles = candles(180);
        let cancelled = AtomicBool::new(false);
        let started_trials = std::sync::atomic::AtomicUsize::new(0);
        let on_trial_start = || {
            if started_trials.fetch_add(1, Ordering::Relaxed) == 0 {
                cancelled.store(true, Ordering::Relaxed);
            }
        };
        assert!(matches!(
            run_cancellable_with_hook(
                &request,
                &candles,
                &cfg(),
                8_760.0,
                Some(&cancelled),
                Some(&on_trial_start),
            ),
            Err(WalkForwardError::Cancelled)
        ));
        assert!(started_trials.load(Ordering::Relaxed) > 0);
        assert!(
            started_trials.load(Ordering::Relaxed) < 128,
            "the inner search should stop before all trials start"
        );

        let retry = run(&request, &candles, &cfg(), 8_760.0).expect("retry");
        let expected = run(&request, &candles, &cfg(), 8_760.0).expect("deterministic run");
        assert_eq!(retry, expected);
    }
}
