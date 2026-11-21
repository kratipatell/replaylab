// Sample engine responses for tests. Shapes follow engine/src/walk_forward.rs.
import type {
  SegmentResult,
  WalkForwardReport,
  WalkForwardWindow,
} from "./api";

const DAY_MS = 86_400_000;

export function segment(
  startMs: number,
  bars: number,
  overrides: Partial<SegmentResult> = {},
): SegmentResult {
  return {
    range: { start_ms: startMs, end_ms: startMs + (bars - 1) * 3_600_000, bars },
    return_pct: 1.5,
    drawdown_pct: 2.0,
    trade_count: 40,
    too_few_trades_to_judge: false,
    ...overrides,
  };
}

export function sampleWindow(
  window: number,
  overrides: Partial<WalkForwardWindow> = {},
): WalkForwardWindow {
  const start = 1_700_000_000_000 + (window - 1) * 10 * DAY_MS;
  return {
    window,
    in_sample: segment(start, 100),
    out_of_sample: segment(start + 100 * 3_600_000, 50),
    chosen_parameters: { "entry.all.0.right": 100 },
    optimizer_score: 0.5,
    oos_starting_equity: 1,
    oos_ending_equity: 1.01,
    ...overrides,
  };
}

export function sampleWalkForwardReport(
  overrides: Partial<WalkForwardReport> = {},
): WalkForwardReport {
  const windows = [sampleWindow(1), sampleWindow(2)];
  const curve = [1, 1.005, 1.01, 1.02, 1.015].map((equity, i) => ({
    bar_ts: 1_700_000_000_000 + i * 3_600_000,
    equity,
  }));
  return {
    mode: "rolling",
    objective: "sharpe",
    window_count: windows.length,
    total_optimizer_evaluations: 6,
    windows,
    stitched_oos_equity_curve: curve,
    analytics: {
      underwater_curve: [
        { bar_ts: curve[0]?.bar_ts ?? 0, drawdown_pct: 0 },
        { bar_ts: curve[1]?.bar_ts ?? 0, drawdown_pct: 0 },
        { bar_ts: curve[2]?.bar_ts ?? 0, drawdown_pct: 0 },
        { bar_ts: curve[3]?.bar_ts ?? 0, drawdown_pct: 0 },
        { bar_ts: curve[4]?.bar_ts ?? 0, drawdown_pct: 100 * (1 - 1.015 / 1.02) },
      ],
      longest_drawdown_duration_bars: 1,
      time_to_recover_max_drawdown_bars: null,
      monthly_returns: [
        { year: 2023, month: 11, return_pct: 1.5, bars: 5, partial: true },
      ],
      best_month: { year: 2023, month: 11, return_pct: 1.5 },
      worst_month: { year: 2023, month: 11, return_pct: 1.5 },
      positive_month_share_pct: 100,
    },
    out_of_sample_stats: {
      trades: 80,
      wins: 48,
      losses: 32,
      win_rate_pct: 60,
      avg_win_pct: 0.8,
      avg_loss_pct: -0.45,
      profit_factor: 1.5,
      expectancy_pct: 0.3,
      expectancy_stdev_pct: 0.6,
      total_return_pct: 1.5,
      buy_and_hold_pct: 0,
      max_drawdown_pct: 100 * (1 - 1.015 / 1.02),
      sharpe: 0.75,
    },
    total_oos_return_pct: 1.5,
    max_oos_drawdown_pct: 0.5,
    profitable_oos_windows: 2,
    profitable_oos_share_pct: 100,
    mean_is_return_per_bar_pct: 0.02,
    mean_oos_return_per_bar_pct: 0.015,
    efficiency_ratio: 0.75,
    efficiency_ratio_definition:
      "Mean OOS window return per bar divided by mean IS window return per bar; null when mean IS return per bar is non-positive or non-finite.",
    parameter_stability: [
      {
        parameter: "entry.all.0.right",
        modal_value: 100,
        modal_windows: 2,
        windows: 2,
        modal_share_pct: 100,
        unstable: false,
      },
    ],
    stability_measure:
      "Per parameter, the share of windows selecting its modal exact value; lower shares indicate more parameter movement.",
    verdict: {
      grade: "holds",
      headline: "Held up out-of-sample, but simulated past returns are not a promise of future profit.",
      reasons: ["Out-of-sample: 80 trades, expectancy 0.50% per trade, profit factor 1.50."],
      oos_trades: 80,
    },
    ...overrides,
  };
}
