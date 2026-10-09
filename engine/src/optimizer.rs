use std::collections::BTreeMap;

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::backtest::{backtest, BacktestConfig, BacktestResult};
use crate::stats::{self, Stats};
use crate::strategy::{Operand, Strategy};
use crate::types::Candle;

/// Hard caps to keep sweeps bounded.
pub const MAX_TRIALS: usize = 2000;
pub const MAX_PARAMS: usize = 8;
pub const MAX_FOLDS: usize = 10;
/// Minimum candles per walk-forward window (train or test).
const MIN_WINDOW: usize = 5;

/// Integer or float search dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamKind {
    Int,
    Float,
}

/// One tunable dimension of the base strategy.
///
/// `target` addresses a field of the strategy:
/// * `"stop_loss_pct"`, `"take_profit_pct"`
/// * `"entry.all.{i}.left|right"`, `"entry.any.{i}.left|right"`,
///   `"exit.all.{i}.left|right"`, `"exit.any.{i}.left|right"` — sets a
///   numeric literal operand.
/// * Same condition-operand path with a `.period` suffix
///   (e.g. `"entry.all.0.left.period"`) — sets an indicator period
///   (`sma`/`ema`/`rsi` only, integer >= 1).
///
/// The search values come either from explicit `values` or from a
/// `min`/`max` range (`step` required for grid floats; for random floats
/// without `step` the range is sampled continuously).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamSpec {
    pub target: String,
    #[serde(default)]
    pub kind: Option<ParamKind>,
    #[serde(default)]
    pub values: Option<Vec<f64>>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub step: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchSpec {
    pub method: SearchMethod,
    #[serde(default)]
    pub trials: Option<usize>,
    #[serde(default)]
    pub seed: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMethod {
    Grid,
    Random,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationSpec {
    /// Single in-sample/out-of-sample split fraction, (0, 1). Default 0.7.
    #[serde(default)]
    pub split: Option<f64>,
    /// Walk-forward fold count (anchored, expanding train). 2..=MAX_FOLDS.
    /// Mutually exclusive with `split`.
    #[serde(default)]
    pub folds: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Objective {
    #[default]
    Sharpe,
    TotalReturn,
    Expectancy,
    ProfitFactor,
}

/// Full optimizer request (mirrors the POST /optimize body).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptimizeSpec {
    pub strategy: Strategy,
    pub start_ms: i64,
    pub end_ms: i64,
    #[serde(default)]
    pub fee_bps: Option<f64>,
    #[serde(default)]
    pub slippage_bps: Option<f64>,
    pub params: Vec<ParamSpec>,
    pub search: SearchSpec,
    #[serde(default)]
    pub validation: Option<ValidationSpec>,
    #[serde(default)]
    pub objective: Option<Objective>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    Entry,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Group {
    All,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    StopLoss,
    TakeProfit,
    Operand {
        section: Section,
        group: Group,
        idx: usize,
        side: Side,
        period: bool,
    },
}

fn parse_target(target: &str) -> Result<Target, String> {
    if target == "stop_loss_pct" {
        return Ok(Target::StopLoss);
    }
    if target == "take_profit_pct" {
        return Ok(Target::TakeProfit);
    }
    let parts: Vec<&str> = target.split('.').collect();
    // Expect section.group.idx.side[.period]
    if parts.len() != 4 && parts.len() != 5 {
        return Err(format!(
            "invalid target {target:?}: expected \
             entry|exit.all|any.{{idx}}.left|right[.period], \
             stop_loss_pct or take_profit_pct"
        ));
    }
    let section = match parts[0] {
        "entry" => Section::Entry,
        "exit" => Section::Exit,
        _ => {
            return Err(format!(
                "invalid target {target:?}: unknown section {:?}",
                parts[0]
            ))
        }
    };
    let group = match parts[1] {
        "all" => Group::All,
        "any" => Group::Any,
        _ => {
            return Err(format!(
                "invalid target {target:?}: unknown group {:?}",
                parts[1]
            ))
        }
    };
    let idx: usize = parts[2].parse().map_err(|_| {
        format!(
            "invalid target {target:?}: bad condition index {:?}",
            parts[2]
        )
    })?;
    let side = match parts[3] {
        "left" => Side::Left,
        "right" => Side::Right,
        _ => {
            return Err(format!(
                "invalid target {target:?}: unknown side {:?}",
                parts[3]
            ))
        }
    };
    let period = match parts.get(4) {
        None => false,
        Some(&"period") => true,
        Some(other) => {
            return Err(format!(
                "invalid target {target:?}: unknown suffix {other:?}"
            ));
        }
    };
    Ok(Target::Operand {
        section,
        group,
        idx,
        side,
        period,
    })
}

fn operand_mut(
    strategy: &mut Strategy,
    section: Section,
    group: Group,
    idx: usize,
    side: Side,
) -> Result<&mut Operand, String> {
    let rules = match section {
        Section::Entry => &mut strategy.entry,
        Section::Exit => match strategy.exit.as_mut() {
            Some(r) => r,
            None => return Err("target addresses exit rules but strategy has no exit".to_string()),
        },
    };
    let conds = match group {
        Group::All => &mut rules.all,
        Group::Any => &mut rules.any,
    };
    let n = conds.len();
    let cond = conds.get_mut(idx).ok_or_else(|| {
        format!("target condition index {idx} out of range (group has {n} conditions)")
    })?;
    Ok(match side {
        Side::Left => &mut cond.left,
        Side::Right => &mut cond.right,
    })
}

/// Apply one trial's values to a cloned base strategy.
pub(crate) fn apply_params(
    strategy: &mut Strategy,
    targets: &[Target],
    combo: &[f64],
) -> Result<(), String> {
    debug_assert_eq!(targets.len(), combo.len());
    for (target, value) in targets.iter().zip(combo.iter()) {
        match target {
            Target::StopLoss => {
                if !value.is_finite() || *value <= 0.0 {
                    return Err(format!("stop_loss_pct must be > 0, got {value}"));
                }
                strategy.stop_loss_pct = Some(*value);
            }
            Target::TakeProfit => {
                if !value.is_finite() || *value <= 0.0 {
                    return Err(format!("take_profit_pct must be > 0, got {value}"));
                }
                strategy.take_profit_pct = Some(*value);
            }
            Target::Operand {
                section,
                group,
                idx,
                side,
                period,
            } => {
                let op = operand_mut(strategy, *section, *group, *idx, *side)?;
                if *period {
                    let v = value.round();
                    if !value.is_finite() || v < 1.0 || v > 100_000.0 {
                        return Err(format!(
                            "indicator period must be in 1..=100000, got {value}"
                        ));
                    }
                    match op {
                        Operand::Indicator(r) => match r.ind {
                            crate::strategy::IndicatorKind::Sma
                            | crate::strategy::IndicatorKind::Ema
                            | crate::strategy::IndicatorKind::Rsi => {
                                r.period = Some(v as usize);
                            }
                            other => {
                                return Err(format!(
                                    "target .period addresses a {other:?} indicator \
                                     (only sma/ema/rsi take periods)"
                                ));
                            }
                        },
                        Operand::Number(_) => {
                            return Err("target .period addresses a numeric literal".to_string());
                        }
                    }
                } else {
                    match op {
                        Operand::Number(v) => {
                            if !value.is_finite() {
                                return Err(format!("threshold must be finite, got {value}"));
                            }
                            *v = *value;
                        }
                        Operand::Indicator(_) => {
                            return Err(
                                "target addresses an indicator operand; use the .period suffix \
                                 to tune its period"
                                    .to_string(),
                            );
                        }
                    }
                }
            }
        }
    }
    strategy.validate()
}

impl Objective {
    pub fn score(&self, s: &Stats) -> f64 {
        match self {
            Objective::Sharpe => s.sharpe,
            Objective::TotalReturn => s.total_return_pct,
            Objective::Expectancy => s.expectancy_pct,
            Objective::ProfitFactor => s.profit_factor.unwrap_or(0.0),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Objective::Sharpe => "sharpe",
            Objective::TotalReturn => "total_return",
            Objective::Expectancy => "expectancy",
            Objective::ProfitFactor => "profit_factor",
        }
    }
}

/// Expand one ParamSpec into its discrete grid values.
fn expand_values(spec: &ParamSpec, kind: ParamKind) -> Result<Vec<f64>, String> {
    if let Some(values) = &spec.values {
        if spec.min.is_some() || spec.max.is_some() || spec.step.is_some() {
            return Err(format!(
                "param {:?}: use either `values` or `min`/`max`, not both",
                spec.target
            ));
        }
        if values.is_empty() {
            return Err(format!(
                "param {:?}: `values` must not be empty",
                spec.target
            ));
        }
        for v in values {
            if !v.is_finite() {
                return Err(format!(
                    "param {:?}: values must be finite, got {v}",
                    spec.target
                ));
            }
        }
        let mut out: Vec<f64> = match kind {
            ParamKind::Int => values.iter().map(|v| v.round()).collect(),
            ParamKind::Float => values.clone(),
        };
        out.sort_by(|a, b| a.partial_cmp(b).unwrap());
        out.dedup_by(|a, b| (*a - *b).abs() <= 1e-12);
        if out.len() > 512 {
            return Err(format!(
                "param {:?}: too many distinct values ({} > 512)",
                spec.target,
                out.len()
            ));
        }
        return Ok(out);
    }
    let (min, max) = match (spec.min, spec.max) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            return Err(format!(
                "param {:?}: need `values` or both `min` and `max`",
                spec.target
            ));
        }
    };
    if !min.is_finite() || !max.is_finite() {
        return Err(format!("param {:?}: min/max must be finite", spec.target));
    }
    if min > max {
        return Err(format!("param {:?}: min {min} > max {max}", spec.target));
    }
    let step = spec.step.unwrap_or(f64::NAN);
    match kind {
        ParamKind::Int => {
            let step_v = if step.is_nan() { 1.0 } else { step };
            if !step_v.is_finite() || step_v <= 0.0 {
                return Err(format!("param {:?}: step must be > 0", spec.target));
            }
            let lo = min.ceil() as i64;
            let hi = max.floor() as i64;
            if lo > hi {
                return Err(format!(
                    "param {:?}: no integer in [{min}, {max}]",
                    spec.target
                ));
            }
            let step_i = step_v.round().max(1.0) as i64;
            let mut out = Vec::new();
            let mut v = lo;
            while v <= hi {
                out.push(v as f64);
                if out.len() > 512 {
                    return Err(format!(
                        "param {:?}: range expands to > 512 values",
                        spec.target
                    ));
                }
                v += step_i;
            }
            if (*out.last().unwrap() - hi as f64).abs() > 1e-9 {
                out.push(hi as f64);
            }
            Ok(out)
        }
        ParamKind::Float => {
            if step.is_nan() {
                return Err(format!(
                    "param {:?}: float range needs `step` for grid search \
                     (omit `step` only for random search)",
                    spec.target
                ));
            }
            if !step.is_finite() || step <= 0.0 {
                return Err(format!("param {:?}: step must be > 0", spec.target));
            }
            let mut out = Vec::new();
            let mut v = min;
            while v <= max + 1e-9 {
                out.push(v);
                if out.len() > 512 {
                    return Err(format!(
                        "param {:?}: range expands to > 512 values",
                        spec.target
                    ));
                }
                v += step;
            }
            if (max - out.last().unwrap()).abs() > 1e-9 {
                out.push(max);
            }
            Ok(out)
        }
    }
}

fn validate_specs(spec: &OptimizeSpec) -> Result<(Vec<Target>, Vec<ParamKind>), String> {
    spec.strategy
        .validate()
        .map_err(|e| format!("invalid base strategy: {e}"))?;
    if spec.params.is_empty() {
        return Err("`params` must not be empty".to_string());
    }
    if spec.params.len() > MAX_PARAMS {
        return Err(format!(
            "too many params ({} > {MAX_PARAMS})",
            spec.params.len()
        ));
    }
    let mut targets = Vec::with_capacity(spec.params.len());
    let mut kinds = Vec::with_capacity(spec.params.len());
    for p in &spec.params {
        let target = parse_target(&p.target)?;
        let kind = p.kind.ok_or_else(|| {
            format!(
                "param {:?}: `kind` is required (\"int\" or \"float\")",
                p.target
            )
        })?;
        // Period targets must be int with min >= 1.
        if matches!(target, Target::Operand { period: true, .. }) {
            if kind != ParamKind::Int {
                return Err(format!(
                    "param {:?}: .period targets require kind \"int\"",
                    p.target
                ));
            }
            if let Some(values) = &p.values {
                if values.iter().any(|v| !v.is_finite() || v.round() < 1.0) {
                    return Err(format!(
                        "param {:?}: .period values must all be >= 1",
                        p.target
                    ));
                }
            }
            if let Some(min) = p.min {
                if !min.is_finite() || min.round() < 1.0 {
                    return Err(format!("param {:?}: .period min must be >= 1", p.target));
                }
            }
        }
        // Fail fast if the target does not address a tunable field of the
        // base strategy (bad index, wrong operand kind, missing exit, ...).
        {
            let dummy = match &target {
                Target::Operand { period: true, .. } => 14.0,
                Target::StopLoss | Target::TakeProfit => 1.0,
                Target::Operand { .. } => 0.0,
            };
            let mut probe = spec.strategy.clone();
            if let Err(e) = apply_params(&mut probe, std::slice::from_ref(&target), &[dummy]) {
                return Err(format!("param {:?}: {e}", p.target));
            }
        }
        // Eagerly expand discrete specs so grid-cap errors surface early.
        // (Random float ranges without step stay continuous.)
        let discrete = p.values.is_some() || p.step.is_some() || kind == ParamKind::Int;
        if discrete {
            expand_values(p, kind)?;
        } else {
            match (p.min, p.max) {
                (Some(a), Some(b)) => {
                    if !a.is_finite() || !b.is_finite() || a > b {
                        return Err(format!("param {:?}: invalid min/max", p.target));
                    }
                }
                _ => {
                    return Err(format!(
                        "param {:?}: need `values` or both `min` and `max`",
                        p.target
                    ));
                }
            }
        }
        targets.push(target);
        kinds.push(kind);
    }
    Ok((targets, kinds))
}

/// Build the trial matrix: one `Vec<f64>` per trial, aligned with `params`.
pub fn build_combos(spec: &OptimizeSpec) -> Result<Vec<Vec<f64>>, String> {
    let (_targets, kinds) = validate_specs(spec)?;
    match spec.search.method {
        SearchMethod::Grid => {
            if spec.search.trials.is_some() {
                return Err("`search.trials` is only valid for random search".to_string());
            }
            let mut grids: Vec<Vec<f64>> = Vec::with_capacity(spec.params.len());
            for (p, k) in spec.params.iter().zip(kinds.iter()) {
                grids.push(expand_values(p, *k)?);
            }
            let total: usize = grids.iter().map(|g| g.len()).product();
            if total == 0 {
                return Err("grid is empty".to_string());
            }
            if total > MAX_TRIALS {
                return Err(format!(
                    "grid has {total} combinations (> {MAX_TRIALS} max); \
                     narrow the ranges or use random search"
                ));
            }
            let mut combos = vec![Vec::with_capacity(grids.len()); total];
            // Cartesian product, first param outermost (stable order).
            let mut stride = total;
            for values in grids.iter() {
                stride /= values.len();
                for (i, combo) in combos.iter_mut().enumerate() {
                    combo.push(values[(i / stride) % values.len()]);
                }
            }
            Ok(combos)
        }
        SearchMethod::Random => {
            let trials = spec.search.trials.unwrap_or(200);
            if trials == 0 || trials > MAX_TRIALS {
                return Err(format!("`search.trials` must be in 1..={MAX_TRIALS}"));
            }
            let seed = spec.search.seed.unwrap_or(42);
            let mut rng = StdRng::seed_from_u64(seed);
            // Discrete dims sample from their expansion; continuous float
            // ranges (no step) sample uniformly.
            let mut discrete: Vec<Option<Vec<f64>>> = Vec::with_capacity(spec.params.len());
            for (p, k) in spec.params.iter().zip(kinds.iter()) {
                if p.values.is_some() || p.step.is_some() || *k == ParamKind::Int {
                    discrete.push(Some(expand_values(p, *k)?));
                } else {
                    discrete.push(None);
                }
            }
            let mut combos = Vec::with_capacity(trials);
            for _ in 0..trials {
                let mut combo = Vec::with_capacity(spec.params.len());
                for (i, p) in spec.params.iter().enumerate() {
                    if let Some(values) = &discrete[i] {
                        let j = rng.random_range(0..values.len());
                        combo.push(values[j]);
                    } else {
                        let (min, max) = (p.min.unwrap(), p.max.unwrap());
                        combo.push(rng.random_range(min..=max));
                    }
                }
                combos.push(combo);
            }
            Ok(combos)
        }
    }
}

fn validate_search_trials(spec: &OptimizeSpec) -> Result<(), String> {
    match spec.search.method {
        SearchMethod::Grid => {
            if spec.search.trials.is_some() {
                return Err("`search.trials` is only valid for random search".to_string());
            }
            Ok(())
        }
        SearchMethod::Random => {
            let t = spec.search.trials.unwrap_or(200);
            if t == 0 || t > MAX_TRIALS {
                return Err(format!("`search.trials` must be in 1..={MAX_TRIALS}"));
            }
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FoldDetail {
    pub fold: usize,
    pub train_start: usize,
    pub train_end: usize,
    pub test_start: usize,
    pub test_end: usize,
    pub in_sample: Stats,
    pub out_of_sample: Stats,
}

#[derive(Debug, Clone, Serialize)]
pub struct RankedTrial {
    pub rank: usize,
    pub params: BTreeMap<String, f64>,
    pub in_sample: Stats,
    pub out_of_sample: Stats,
    pub score: f64,
    pub is_score: f64,
    pub objective: String,
    pub overfit: bool,
    pub overfit_gap: f64,
    pub overfit_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folds: Option<Vec<FoldDetail>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SweepReport {
    pub objective: String,
    pub validation: String,
    pub total_trials: usize,
    pub trials: Vec<RankedTrial>,
    pub warnings: Vec<String>,
}

/// Stats over `candles[start..end]` with trades filtered by entry index and
/// the equity curve rebased to 1.0 (mirrors `split_stats` conventions).
fn window_stats(
    result: &BacktestResult,
    candles: &[Candle],
    start: usize,
    end: usize,
    bars_per_year: f64,
) -> Stats {
    let window_candles = &candles[start..end];
    let trades: Vec<_> = result
        .trades
        .iter()
        .filter(|t| t.entry_idx >= start && t.entry_idx < end)
        .cloned()
        .collect();
    let equity: Vec<f64> = if start == 0 {
        result.equity_curve[start..end].to_vec()
    } else {
        let base = result.equity_curve[start];
        let mut v = Vec::with_capacity(end - start + 1);
        v.push(1.0);
        for e in &result.equity_curve[start + 1..end] {
            v.push(e / base);
        }
        // A one-bar window has no post-base equity; keep the seed point.
        if v.len() == 1 {
            v.push(1.0);
        }
        v
    };
    stats::compute_stats(
        &BacktestResult {
            trades,
            equity_curve: equity,
        },
        window_candles,
        bars_per_year,
    )
}

fn mean_stats(all: &[Stats]) -> Stats {
    debug_assert!(!all.is_empty());
    let n = all.len() as f64;
    let mean = |f: fn(&Stats) -> f64| all.iter().map(f).sum::<f64>() / n;
    let pf_vals: Vec<f64> = all.iter().filter_map(|s| s.profit_factor).collect();
    Stats {
        trades: (mean(|s| s.trades as f64)).round() as usize,
        wins: (mean(|s| s.wins as f64)).round() as usize,
        losses: (mean(|s| s.losses as f64)).round() as usize,
        win_rate_pct: mean(|s| s.win_rate_pct),
        avg_win_pct: mean(|s| s.avg_win_pct),
        avg_loss_pct: mean(|s| s.avg_loss_pct),
        profit_factor: if pf_vals.is_empty() {
            None
        } else {
            Some(pf_vals.iter().sum::<f64>() / pf_vals.len() as f64)
        },
        expectancy_pct: mean(|s| s.expectancy_pct),
        total_return_pct: mean(|s| s.total_return_pct),
        buy_and_hold_pct: mean(|s| s.buy_and_hold_pct),
        max_drawdown_pct: mean(|s| s.max_drawdown_pct),
        sharpe: mean(|s| s.sharpe),
    }
}

/// Overfit heuristic on Sharpe (+ return sign-flip guard).
///
/// Returns `(overfit, gap, reason)` with `gap = is_sharpe - oos_sharpe`.
pub fn overfit_flag(is_stats: &Stats, oos_stats: &Stats) -> (bool, f64, Option<String>) {
    let gap = is_stats.sharpe - oos_stats.sharpe;
    if is_stats.sharpe > 0.5 && oos_stats.sharpe < 0.0 {
        return (
            true,
            gap,
            Some(format!(
                "in-sample Sharpe {:.2} but out-of-sample Sharpe {:.2} (sign flip)",
                is_stats.sharpe, oos_stats.sharpe
            )),
        );
    }
    if gap > 1.0 {
        return (
            true,
            gap,
            Some(format!(
                "in-sample Sharpe {:.2} exceeds out-of-sample Sharpe {:.2} by {:.2}",
                is_stats.sharpe, oos_stats.sharpe, gap
            )),
        );
    }
    if is_stats.total_return_pct > 10.0 && oos_stats.total_return_pct < 0.0 {
        return (
            true,
            gap,
            Some(format!(
                "in-sample return {:.1}% but out-of-sample return {:.1}%",
                is_stats.total_return_pct, oos_stats.total_return_pct
            )),
        );
    }
    (false, gap, None)
}

struct Evaluated {
    params: BTreeMap<String, f64>,
    is_stats: Stats,
    oos_stats: Stats,
    is_score: f64,
    score: f64,
    folds: Option<Vec<FoldDetail>>,
}

struct EvalCtx<'a> {
    param_names: &'a [String],
    targets: &'a [Target],
    candles: &'a [Candle],
    cfg: &'a BacktestConfig,
    bars_per_year: f64,
    validation: &'a ResolvedValidation,
    objective: Objective,
}

fn evaluate_combo(base: &Strategy, ctx: &EvalCtx<'_>, combo: &[f64]) -> Result<Evaluated, String> {
    let mut strategy = base.clone();
    apply_params(&mut strategy, ctx.targets, combo)?;
    let result = backtest(&strategy, ctx.candles, ctx.cfg)?;
    let params: BTreeMap<String, f64> = ctx
        .param_names
        .iter()
        .cloned()
        .zip(combo.iter().copied())
        .collect();
    let candles = ctx.candles;
    let bars_per_year = ctx.bars_per_year;
    let objective = ctx.objective;
    match ctx.validation {
        ResolvedValidation::Split { idx } => {
            let (is_stats, oos_stats) = stats::split_stats(&result, candles, *idx, bars_per_year)?;
            Ok(Evaluated {
                params,
                is_score: objective.score(&is_stats),
                score: objective.score(&oos_stats),
                is_stats,
                oos_stats,
                folds: None,
            })
        }
        ResolvedValidation::WalkForward { boundaries } => {
            let folds_n = boundaries.len() - 2;
            let mut fold_details = Vec::with_capacity(folds_n);
            let mut is_list = Vec::with_capacity(folds_n);
            let mut oos_list = Vec::with_capacity(folds_n);
            for f in 0..folds_n {
                let train_end = boundaries[f + 1];
                let test_end = boundaries[f + 2];
                let is_stats = window_stats(&result, candles, 0, train_end, bars_per_year);
                let oos_stats = window_stats(&result, candles, train_end, test_end, bars_per_year);
                is_list.push(objective.score(&is_stats));
                oos_list.push(objective.score(&oos_stats));
                fold_details.push(FoldDetail {
                    fold: f,
                    train_start: 0,
                    train_end,
                    test_start: train_end,
                    test_end,
                    in_sample: is_stats,
                    out_of_sample: oos_stats,
                });
            }
            let is_stats = mean_stats(
                &fold_details
                    .iter()
                    .map(|f| f.in_sample.clone())
                    .collect::<Vec<_>>(),
            );
            let oos_stats = mean_stats(
                &fold_details
                    .iter()
                    .map(|f| f.out_of_sample.clone())
                    .collect::<Vec<_>>(),
            );
            let is_score = is_list.iter().sum::<f64>() / is_list.len() as f64;
            let score = oos_list.iter().sum::<f64>() / oos_list.len() as f64;
            Ok(Evaluated {
                params,
                is_stats,
                oos_stats,
                is_score,
                score,
                folds: Some(fold_details),
            })
        }
    }
}

enum ResolvedValidation {
    Split { idx: usize },
    WalkForward { boundaries: Vec<usize> },
}

fn resolve_validation(
    validation: &ValidationSpec,
    n: usize,
) -> Result<(ResolvedValidation, String), String> {
    if let Some(folds) = validation.folds {
        if validation.split.is_some() {
            return Err(
                "`validation.split` and `validation.folds` are mutually exclusive".to_string(),
            );
        }
        if !(2..=MAX_FOLDS).contains(&folds) {
            return Err(format!("`validation.folds` must be in 2..={MAX_FOLDS}"));
        }
        let chunks = folds + 1;
        if n < chunks * MIN_WINDOW {
            return Err(format!(
                "not enough candles ({n}) for {folds} walk-forward folds \
                 (need at least {})",
                chunks * MIN_WINDOW
            ));
        }
        let mut boundaries = Vec::with_capacity(chunks + 1);
        for i in 0..=chunks {
            boundaries.push(n * i / chunks);
        }
        return Ok((
            ResolvedValidation::WalkForward { boundaries },
            format!("walk_forward(folds={folds})"),
        ));
    }
    let split = validation.split.unwrap_or(0.7);
    if !(split > 0.0 && split < 1.0) || !split.is_finite() {
        return Err(format!(
            "`validation.split` must be strictly between 0 and 1, got {split}"
        ));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let idx = (n as f64 * split).floor() as usize;
    if idx == 0 || idx >= n {
        return Err(format!(
            "split {split} gives invalid split index {idx} for {n} candles"
        ));
    }
    Ok((ResolvedValidation::Split { idx }, format!("split({split})")))
}

/// Run the full sweep: expand combos, evaluate trials in parallel with
/// Rayon, rank by out-of-sample objective (descending).
pub fn sweep(
    spec: &OptimizeSpec,
    candles: &[Candle],
    cfg: &BacktestConfig,
    bars_per_year: f64,
) -> Result<SweepReport, String> {
    if candles.is_empty() {
        return Err("no candles in range".to_string());
    }
    validate_search_trials(spec)?;
    let (targets, _kinds) = validate_specs(spec)?;
    let combos = build_combos(spec)?;
    let param_names: Vec<String> = spec.params.iter().map(|p| p.target.clone()).collect();
    let validation_spec = spec.validation.unwrap_or_default();
    let (validation, validation_name) = resolve_validation(&validation_spec, candles.len())?;
    let objective = spec.objective.unwrap_or_default();

    // Rayon parallel evaluation over trials; `collect` preserves order so
    // the subsequent stable sort is deterministic.
    let ctx = EvalCtx {
        param_names: &param_names,
        targets: &targets,
        candles,
        cfg,
        bars_per_year,
        validation: &validation,
        objective,
    };
    let evaluated: Vec<Result<Evaluated, String>> = combos
        .par_iter()
        .map(|combo| evaluate_combo(&spec.strategy, &ctx, combo))
        .collect();
    let mut evaluated: Vec<Evaluated> = evaluated.into_iter().collect::<Result<_, _>>()?;

    evaluated.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| format!("{:?}", a.params).cmp(&format!("{:?}", b.params)))
    });

    let mut trials = Vec::with_capacity(evaluated.len());
    for (i, e) in evaluated.into_iter().enumerate() {
        let (overfit, gap, reason) = overfit_flag(&e.is_stats, &e.oos_stats);
        trials.push(RankedTrial {
            rank: i + 1,
            params: e.params,
            in_sample: e.is_stats,
            out_of_sample: e.oos_stats,
            score: e.score,
            is_score: e.is_score,
            objective: objective.as_str().to_string(),
            overfit,
            overfit_gap: gap,
            overfit_reason: reason,
            folds: e.folds,
        });
    }

    let mut warnings = Vec::new();
    if let Some(top) = trials.first() {
        if top.overfit {
            warnings.push(format!(
                "top trial (rank 1) looks overfit: {}. \
                 In-sample strength does not carry out-of-sample; \
                 prefer robust mid-rank parameters or add walk-forward folds.",
                top.overfit_reason.as_deref().unwrap_or("IS >> OOS")
            ));
        }
        let overfit_top10 = trials.iter().take(10).filter(|t| t.overfit).count();
        if trials.len() >= 10 && overfit_top10 > 5 {
            warnings.push(format!(
                "{overfit_top10}/10 top trials flagged as overfit; \
                 the search space may be curve-fitted"
            ));
        }
    }
    Ok(SweepReport {
        objective: objective.as_str().to_string(),
        validation: validation_name,
        total_trials: trials.len(),
        trials,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::{Condition, IndicatorKind, IndicatorRef, Op, Operand, Rules, Side};

    fn strat_rsi(threshold: f64, period: usize) -> Strategy {
        Strategy {
            asset: "BTCUSDT".to_string(),
            timeframe: "1h".to_string(),
            side: Side::Long,
            entry: Rules {
                all: vec![Condition {
                    left: Operand::Indicator(IndicatorRef {
                        ind: IndicatorKind::Rsi,
                        period: Some(period),
                    }),
                    op: Op::Lt,
                    right: Operand::Number(threshold),
                }],
                any: vec![],
            },
            exit: Some(Rules {
                all: vec![],
                any: vec![Condition {
                    left: Operand::Indicator(IndicatorRef {
                        ind: IndicatorKind::Rsi,
                        period: Some(period),
                    }),
                    op: Op::Gt,
                    right: Operand::Number(60.0),
                }],
            }),
            stop_loss_pct: Some(2.0),
            take_profit_pct: None,
            claim: None,
        }
    }

    fn wave_candles(n: usize) -> Vec<Candle> {
        let start: i64 = 1_700_006_400_000;
        let step: i64 = 3_600_000;
        let mut prev_close = 100.0;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let close = 100.0 + 10.0 * ((i as f64) * 0.3).sin();
            let open = if i == 0 { 100.0 } else { prev_close };
            out.push(Candle {
                ts: start + i as i64 * step,
                open,
                high: open.max(close) + 1.0,
                low: open.min(close) - 1.0,
                close,
            });
            prev_close = close;
        }
        out
    }

    fn base_spec() -> OptimizeSpec {
        OptimizeSpec {
            strategy: strat_rsi(30.0, 14),
            start_ms: 0,
            end_ms: 1,
            fee_bps: None,
            slippage_bps: None,
            params: vec![],
            search: SearchSpec {
                method: SearchMethod::Grid,
                trials: None,
                seed: None,
            },
            validation: None,
            objective: None,
        }
    }

    fn param(target: &str, kind: ParamKind, values: Vec<f64>) -> ParamSpec {
        ParamSpec {
            target: target.to_string(),
            kind: Some(kind),
            values: Some(values),
            min: None,
            max: None,
            step: None,
        }
    }

    #[test]
    fn grid_cartesian_count_and_order() {
        let mut spec = base_spec();
        spec.params = vec![
            param("entry.all.0.right", ParamKind::Float, vec![25.0, 30.0]),
            param("take_profit_pct", ParamKind::Float, vec![2.0, 3.0, 4.0]),
        ];
        let combos = build_combos(&spec).unwrap();
        assert_eq!(combos.len(), 6);
        // First param outermost.
        assert_eq!(combos[0], vec![25.0, 2.0]);
        assert_eq!(combos[1], vec![25.0, 3.0]);
        assert_eq!(combos[3], vec![30.0, 2.0]);
    }

    #[test]
    fn random_is_seeded_and_bounded() {
        let mut spec = base_spec();
        spec.search = SearchSpec {
            method: SearchMethod::Random,
            trials: Some(50),
            seed: Some(7),
        };
        spec.params = vec![ParamSpec {
            target: "entry.all.0.right".to_string(),
            kind: Some(ParamKind::Float),
            values: None,
            min: Some(20.0),
            max: Some(40.0),
            step: None,
        }];
        let a = build_combos(&spec).unwrap();
        let b = build_combos(&spec).unwrap();
        assert_eq!(a, b, "same seed must give same combos");
        assert_eq!(a.len(), 50);
        for c in &a {
            assert!((20.0..=40.0).contains(&c[0]), "out of range: {}", c[0]);
        }
        spec.search.seed = Some(8);
        let c = build_combos(&spec).unwrap();
        assert_ne!(a, c, "different seeds should differ");
    }

    #[test]
    fn rejects_empty_params_and_bad_targets() {
        let spec = base_spec();
        assert!(build_combos(&spec).is_err());

        let mut spec = base_spec();
        spec.params = vec![param("entry.all.9.right", ParamKind::Float, vec![1.0])];
        assert!(build_combos(&spec).is_err());

        // .period on a numeric literal.
        let mut spec = base_spec();
        spec.params = vec![param(
            "entry.all.0.right.period",
            ParamKind::Int,
            vec![14.0],
        )];
        assert!(build_combos(&spec).is_err());

        // Value target on an indicator operand.
        let mut spec = base_spec();
        spec.params = vec![param("entry.all.0.left", ParamKind::Float, vec![14.0])];
        assert!(build_combos(&spec).is_err());

        // Period below 1.
        let mut spec = base_spec();
        spec.params = vec![param("entry.all.0.left.period", ParamKind::Int, vec![0.0])];
        assert!(build_combos(&spec).is_err());
    }

    #[test]
    fn grid_cap_suggests_random() {
        let mut spec = base_spec();
        // 50 x 50 = 2500 > 2000.
        let v50: Vec<f64> = (1..=50).map(|v| v as f64).collect();
        spec.params = vec![
            param("entry.all.0.right", ParamKind::Float, v50.clone()),
            param("take_profit_pct", ParamKind::Float, v50),
        ];
        let err = build_combos(&spec).unwrap_err();
        assert!(err.contains("2500"), "should mention size, got: {err}");
        assert!(
            err.to_lowercase().contains("random"),
            "should suggest random: {err}"
        );
    }

    #[test]
    fn sweep_ranks_by_oos_and_reports_is_oos() {
        let candles = wave_candles(300);
        let mut spec = base_spec();
        spec.params = vec![param(
            "entry.all.0.right",
            ParamKind::Float,
            vec![20.0, 25.0, 30.0, 35.0],
        )];
        let report = sweep(&spec, &candles, &BacktestConfig::default(), 8760.0).unwrap();
        assert_eq!(report.total_trials, 4);
        assert_eq!(report.trials.len(), 4);
        for (i, t) in report.trials.iter().enumerate() {
            assert_eq!(t.rank, i + 1);
            assert!(t.params.contains_key("entry.all.0.right"));
        }
        for w in report.trials.windows(2) {
            assert!(w[0].score >= w[1].score, "scores must descend");
        }
        // In-sample vs out-of-sample both populated via the default split.
        assert_eq!(report.validation, "split(0.7)");
        for t in &report.trials {
            assert!(t.in_sample.trades + t.out_of_sample.trades >= t.in_sample.trades);
        }
    }

    #[test]
    fn sweep_200_trials_random() {
        let candles = wave_candles(400);
        let mut spec = base_spec();
        spec.search = SearchSpec {
            method: SearchMethod::Random,
            trials: Some(200),
            seed: Some(123),
        };
        spec.params = vec![
            ParamSpec {
                target: "entry.all.0.right".to_string(),
                kind: Some(ParamKind::Float),
                values: None,
                min: Some(15.0),
                max: Some(40.0),
                step: None,
            },
            ParamSpec {
                target: "entry.all.0.left.period".to_string(),
                kind: Some(ParamKind::Int),
                values: None,
                min: Some(5.0),
                max: Some(30.0),
                step: None,
            },
        ];
        let report = sweep(&spec, &candles, &BacktestConfig::default(), 8760.0).unwrap();
        assert_eq!(report.total_trials, 200);
        assert_eq!(report.trials.len(), 200);
        let ranks: Vec<usize> = report.trials.iter().map(|t| t.rank).collect();
        let expected: Vec<usize> = (1..=200).collect();
        assert_eq!(ranks, expected);
    }

    #[test]
    fn walk_forward_folds_reported() {
        let candles = wave_candles(300);
        let mut spec = base_spec();
        spec.validation = Some(ValidationSpec {
            split: None,
            folds: Some(3),
        });
        spec.params = vec![param(
            "entry.all.0.right",
            ParamKind::Float,
            vec![25.0, 30.0],
        )];
        let report = sweep(&spec, &candles, &BacktestConfig::default(), 8760.0).unwrap();
        assert_eq!(report.total_trials, 2);
        assert!(report.validation.contains("walk_forward"));
        for t in &report.trials {
            let folds = t.folds.as_ref().expect("WF must include folds");
            assert_eq!(folds.len(), 3);
            // Anchored: every train starts at 0, test follows train.
            for (i, f) in folds.iter().enumerate() {
                assert_eq!(f.fold, i);
                assert_eq!(f.train_start, 0);
                assert_eq!(f.test_start, f.train_end);
                assert!(f.test_end > f.test_start);
            }
        }
    }

    #[test]
    fn overfit_flag_sign_flip() {
        let mk = |sharpe: f64, ret: f64| Stats {
            trades: 10,
            wins: 5,
            losses: 5,
            win_rate_pct: 50.0,
            avg_win_pct: 1.0,
            avg_loss_pct: -1.0,
            profit_factor: Some(1.0),
            expectancy_pct: 0.0,
            total_return_pct: ret,
            buy_and_hold_pct: 0.0,
            max_drawdown_pct: 5.0,
            sharpe,
        };
        let (flag, gap, reason) = overfit_flag(&mk(1.5, 30.0), &mk(-0.5, -5.0));
        assert!(flag);
        assert!((gap - 2.0).abs() < 1e-12);
        assert!(reason.is_some());
        let (flag2, _, _) = overfit_flag(&mk(0.2, 3.0), &mk(0.1, 2.0));
        assert!(!flag2, "close IS/OOS must not flag");
    }

    #[test]
    fn single_split_matches_split_stats() {
        let candles = wave_candles(100);
        let strategy = strat_rsi(30.0, 14);
        let result = backtest(&strategy, &candles, &BacktestConfig::default()).unwrap();
        let (is_a, oos_a) = stats::split_stats(&result, &candles, 70, 8760.0).unwrap();
        let is_b = window_stats(&result, &candles, 0, 70, 8760.0);
        let oos_b = window_stats(&result, &candles, 70, 100, 8760.0);
        assert_eq!(is_a, is_b);
        assert_eq!(oos_a.trades, oos_b.trades);
        assert!((oos_a.total_return_pct - oos_b.total_return_pct).abs() < 1e-9);
    }
}
