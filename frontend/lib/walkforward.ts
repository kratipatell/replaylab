// Pure logic for the walk-forward mode of the Test Lab: the parameters that can
// be tuned (taken from the rules the user already wrote), validation that
// mirrors engine/src/walk_forward.rs and engine/src/optimizer.rs, request
// building, and result helpers.

import {
  ApiError,
  type Objective,
  type ParamSpec,
  type SuggestedTemplateParameter,
  type WalkForwardEquityPoint,
  type WalkForwardMode,
  type WalkForwardRequest,
  type WalkForwardWindow,
} from "./api";
import {
  type RuleGroupState,
  type RuleOperandState,
  type TestLabForm,
  buildRequest,
  estimateCandles,
  validateCore,
} from "./testlab";

export const WF_MIN_WINDOWS = 2;
export const WF_MAX_WINDOWS = 10;
export const WF_MAX_PARAMS = 8;
export const WF_MAX_EVALUATIONS = 2000;
export const WF_MAX_DISTINCT_VALUES = 512;

export const WF_DEFAULT_WINDOWS = 4;
export const WF_DEFAULT_IS_BARS = 1000;
export const WF_DEFAULT_OOS_BARS = 500;

export const WF_MODES: { value: WalkForwardMode; label: string; help: string }[] = [
  {
    value: "rolling",
    label: "Rolling",
    help: "Each in-sample block has the same length and moves forward one out-of-sample block per window.",
  },
  {
    value: "anchored",
    label: "Anchored",
    help: "The in-sample block starts at the beginning of the range and grows by one out-of-sample block per window.",
  },
];

export const WF_OBJECTIVES: { value: Objective; label: string }[] = [
  { value: "sharpe", label: "Sharpe ratio" },
  { value: "total_return", label: "Total return" },
  { value: "expectancy", label: "Expectancy per trade" },
  { value: "profit_factor", label: "Profit factor" },
];

export const EFFICIENCY_NA_EXPLANATION =
  "Not defined: the in-sample return per bar was zero or negative, so there is nothing to divide by.";

export interface ParamRow {
  id: string;
  target: string;
  values: string;
}

export interface WalkForwardForm {
  mode: WalkForwardMode;
  windows: string;
  isBars: string;
  oosBars: string;
  objective: Objective;
  params: ParamRow[];
}

export interface TuneTarget {
  target: string;
  label: string;
  kind: "int" | "float";
  current: number;
}

export interface RunProblem {
  kind: "invalid" | "rate_limited" | "timeout" | "failed" | "cancelled";
  message: string;
}

let rowCounter = 0;

export function createParamRow(target = "", values = ""): ParamRow {
  rowCounter += 1;
  return { id: `param-${rowCounter}`, target, values };
}

export function defaultWalkForwardForm(): WalkForwardForm {
  return {
    mode: "rolling",
    windows: String(WF_DEFAULT_WINDOWS),
    isBars: String(WF_DEFAULT_IS_BARS),
    oosBars: String(WF_DEFAULT_OOS_BARS),
    objective: "sharpe",
    params: [],
  };
}

export function applyTemplateGrid(
  form: WalkForwardForm,
  grid: SuggestedTemplateParameter[],
): WalkForwardForm {
  return {
    ...form,
    params: grid.map((parameter) =>
      createParamRow(parameter.target, parameter.values.map(formatValue).join(", ")),
    ),
  };
}

// The engine picks the integer or float grid by the target itself: a period
// target must be an integer kind.
export function kindForTarget(target: string): "int" | "float" {
  return target.endsWith(".period") ? "int" : "float";
}

const PERIOD_NAMES: Record<string, string> = { sma: "SMA", ema: "EMA", rsi: "RSI" };

function numberOrNull(raw: string): number | null {
  if (raw.trim() === "") {
    return null;
  }
  const v = Number(raw);
  return Number.isFinite(v) ? v : null;
}

function operandTargets(
  operand: RuleOperandState,
  base: string,
  ruleLabel: string,
  side: "left" | "right",
): TuneTarget[] {
  const sideName = side === "left" ? "left side" : "right side";
  if (operand.mode === "number") {
    const current = numberOrNull(operand.num);
    if (current === null) {
      return [];
    }
    return [
      {
        target: `${base}.${side}`,
        label: `${ruleLabel}, ${sideName}: value`,
        kind: "float",
        current,
      },
    ];
  }
  const name = PERIOD_NAMES[operand.ind];
  if (name === undefined) {
    return [];
  }
  const current = Number(operand.period);
  if (!Number.isInteger(current) || current < 1) {
    return [];
  }
  return [
    {
      target: `${base}.${side}.period`,
      label: `${ruleLabel}, ${sideName}: ${name} period`,
      kind: "int",
      current,
    },
  ];
}

function groupTargets(group: RuleGroupState, section: "entry" | "exit", name: string): TuneTarget[] {
  const out: TuneTarget[] = [];
  for (const list of ["all", "any"] as const) {
    group[list].forEach((rule, i) => {
      const ruleLabel = `${name} rule ${i + 1} (${list})`;
      const base = `${section}.${list}.${i}`;
      out.push(
        ...operandTargets(rule.left, base, ruleLabel, "left"),
        ...operandTargets(rule.right, base, ruleLabel, "right"),
      );
    });
  }
  return out;
}

// Every number or period the user can tune, read from the current rules.
export function tuneTargets(form: TestLabForm): TuneTarget[] {
  const out = [
    ...groupTargets(form.entry, "entry", "Entry"),
    ...groupTargets(form.exit, "exit", "Exit"),
  ];
  const stop = numberOrNull(form.stopLoss);
  if (stop !== null && stop > 0) {
    out.push({ target: "stop_loss_pct", label: "Stop loss %", kind: "float", current: stop });
  }
  const take = numberOrNull(form.takeProfit);
  if (take !== null && take > 0) {
    out.push({ target: "take_profit_pct", label: "Take profit %", kind: "float", current: take });
  }
  return out;
}

export function targetLabels(form: TestLabForm): Record<string, string> {
  return Object.fromEntries(tuneTargets(form).map((t) => [t.target, t.label]));
}

function formatValue(v: number): string {
  return String(Number(v.toPrecision(6)));
}

// A starting grid around the current value, so the user can edit rather than
// type from nothing.
export function suggestValues(target: TuneTarget): string {
  const c = target.current;
  const raw =
    target.kind === "int"
      ? [Math.max(1, Math.round(c * 0.8)), c, Math.max(1, Math.round(c * 1.2))]
      : [c * 0.9, c, c * 1.1];
  const unique = [...new Set(raw.map((v) => (target.kind === "int" ? v : Number(formatValue(v)))))];
  return unique.map(formatValue).join(", ");
}

export function parseValues(raw: string): { values: number[]; bad: string[] } {
  const values: number[] = [];
  const bad: string[] = [];
  for (const token of raw.split(",")) {
    const trimmed = token.trim();
    if (trimmed === "") {
      continue;
    }
    const v = Number(trimmed);
    if (Number.isFinite(v)) {
      values.push(v);
    } else {
      bad.push(trimmed);
    }
  }
  return { values, bad };
}

function isWholeNumberIn(raw: string, min: number, max: number): boolean {
  if (raw.trim() === "") {
    return false;
  }
  const v = Number(raw);
  return Number.isInteger(v) && v >= min && v <= max;
}

function isPositiveWhole(raw: string): boolean {
  return isWholeNumberIn(raw, 1, Number.MAX_SAFE_INTEGER);
}

// Everything the engine would reject, checked before the request is sent.
// The engine's checks stay authoritative: anything missed here still comes
// back as a 400 and is shown as-is.
export function walkForwardProblems(
  form: TestLabForm,
  wf: WalkForwardForm,
  nowMs: number,
): string[] {
  const errors = validateCore(form, nowMs);
  const windowsOk = isWholeNumberIn(wf.windows, WF_MIN_WINDOWS, WF_MAX_WINDOWS);
  if (!windowsOk) {
    errors.push(`Windows must be a whole number from ${WF_MIN_WINDOWS} to ${WF_MAX_WINDOWS}.`);
  }
  const isOk = isPositiveWhole(wf.isBars);
  if (!isOk) {
    errors.push("In-sample bars must be a whole number of 1 or more.");
  }
  const oosOk = isPositiveWhole(wf.oosBars);
  if (!oosOk) {
    errors.push("Out-of-sample bars must be a whole number of 1 or more.");
  }
  const candles = estimateCandles(form);
  if (windowsOk && isOk && oosOk && candles !== null) {
    const required = Number(wf.isBars) + Number(wf.windows) * Number(wf.oosBars);
    if (required > candles) {
      errors.push(
        `This needs ${required.toLocaleString()} candles (in-sample plus ${wf.windows} out-of-sample blocks), but the date range holds about ${candles.toLocaleString()}. Shorten the blocks or widen the date range.`,
      );
    }
  }
  errors.push(...paramProblems(form, wf, windowsOk ? Number(wf.windows) : null));
  return errors;
}

function paramProblems(form: TestLabForm, wf: WalkForwardForm, windows: number | null): string[] {
  const errors: string[] = [];
  if (wf.params.length === 0) {
    errors.push("Add at least one parameter to tune.");
    return errors;
  }
  if (wf.params.length > WF_MAX_PARAMS) {
    errors.push(`Tune at most ${WF_MAX_PARAMS} parameters.`);
  }
  const targets = new Set(tuneTargets(form).map((t) => t.target));
  const seen = new Set<string>();
  let combinations = 1;
  let rowsOk = true;
  wf.params.forEach((row, i) => {
    const n = i + 1;
    if (!targets.has(row.target)) {
      rowsOk = false;
      errors.push(`Parameter ${n}: that value is no longer in the rules. Pick another.`);
      return;
    }
    if (seen.has(row.target)) {
      rowsOk = false;
      errors.push(`Parameter ${n} is listed more than once.`);
      return;
    }
    seen.add(row.target);
    const { values, bad } = parseValues(row.values);
    if (bad.length > 0) {
      rowsOk = false;
      errors.push(`Parameter ${n}: "${bad[0]}" is not a number.`);
      return;
    }
    if (values.length === 0) {
      rowsOk = false;
      errors.push(`Parameter ${n}: enter at least one value, separated by commas.`);
      return;
    }
    const kind = kindForTarget(row.target);
    if (kind === "int" && values.some((v) => !Number.isInteger(v) || v < 1)) {
      rowsOk = false;
      errors.push(`Parameter ${n}: indicator periods must be whole numbers of 1 or more.`);
      return;
    }
    if (
      (row.target === "stop_loss_pct" || row.target === "take_profit_pct") &&
      values.some((v) => v <= 0)
    ) {
      rowsOk = false;
      errors.push(`Parameter ${n}: stop loss and take profit must be above 0.`);
      return;
    }
    const distinct = new Set(values).size;
    if (distinct > WF_MAX_DISTINCT_VALUES) {
      rowsOk = false;
      errors.push(`Parameter ${n}: at most ${WF_MAX_DISTINCT_VALUES} distinct values.`);
      return;
    }
    combinations *= distinct;
  });
  if (!rowsOk) {
    return errors;
  }
  if (combinations > WF_MAX_EVALUATIONS) {
    errors.push(
      `The grid has ${combinations.toLocaleString()} combinations; the limit is ${WF_MAX_EVALUATIONS.toLocaleString()}. Use fewer values.`,
    );
  } else if (windows !== null && combinations * windows > WF_MAX_EVALUATIONS) {
    errors.push(
      `${windows} windows × ${combinations.toLocaleString()} combinations is ${(combinations * windows).toLocaleString()} optimizer runs; the limit is ${WF_MAX_EVALUATIONS.toLocaleString()}. Use fewer values or windows.`,
    );
  }
  return errors;
}

export function buildWalkForwardRequest(
  form: TestLabForm,
  wf: WalkForwardForm,
  nowMs: number,
): WalkForwardRequest {
  const single = buildRequest(form, nowMs);
  const params: ParamSpec[] = wf.params.map((row) => ({
    target: row.target,
    kind: kindForTarget(row.target),
    values: parseValues(row.values).values,
  }));
  return {
    strategy: single.strategy,
    start_ms: single.start_ms,
    end_ms: single.end_ms,
    fee_bps: single.fee_bps,
    slippage_bps: single.slippage_bps,
    windows: Number(wf.windows),
    is_bars: Number(wf.isBars),
    oos_bars: Number(wf.oosBars),
    mode: wf.mode,
    params,
    search: { method: "grid" },
    objective: wf.objective,
  };
}

// The date range moves with the clock for presets, so a preset is compared by
// its length; a custom range is compared by its exact dates.
export function sameWalkForwardInputs(
  submitted: WalkForwardRequest,
  current: WalkForwardRequest,
  preset: TestLabForm["preset"],
): boolean {
  const normalise = (r: WalkForwardRequest): string =>
    JSON.stringify(
      preset === "custom"
        ? r
        : { ...r, start_ms: 0, end_ms: r.end_ms - r.start_ms },
    );
  return normalise(submitted) === normalise(current);
}

export function classifyWalkForwardError(error: unknown): RunProblem {
  if (error instanceof Error && error.name === "AbortError") {
    return { kind: "cancelled", message: "Run cancelled." };
  }
  if (error instanceof ApiError) {
    if (error.status === 400) {
      return { kind: "invalid", message: error.message };
    }
    if (error.status === 429) {
      return {
        kind: "rate_limited",
        message: "Too many walk-forward runs from this network. Wait a minute, then try again.",
      };
    }
    if (error.status === 408) {
      return { kind: "timeout", message: error.message };
    }
    return { kind: "failed", message: error.message };
  }
  return {
    kind: "failed",
    message: error instanceof Error ? error.message : String(error),
  };
}

export function formatEfficiency(ratio: number | null | undefined): {
  text: string;
  defined: boolean;
} {
  if (ratio === null || ratio === undefined || !Number.isFinite(ratio)) {
    return { text: "n/a", defined: false };
  }
  return { text: ratio.toFixed(2), defined: true };
}

export function windowFlags(window: WalkForwardWindow): string[] {
  const flags: string[] = [];
  if (window.in_sample.too_few_trades_to_judge) {
    flags.push("Too few in-sample trades to judge");
  }
  if (window.out_of_sample.too_few_trades_to_judge) {
    flags.push("Too few out-of-sample trades to judge");
  }
  return flags;
}

export interface WindowMarker {
  index: number;
  label: string;
}

// Index in the stitched curve where each out-of-sample block starts.
export function windowMarkers(
  curve: WalkForwardEquityPoint[],
  windows: WalkForwardWindow[],
): WindowMarker[] {
  const markers: WindowMarker[] = [];
  for (const w of windows) {
    const index = curve.findIndex((p) => p.bar_ts === w.out_of_sample.range.start_ms);
    if (index >= 0) {
      markers.push({ index, label: `W${w.window}` });
    }
  }
  return markers;
}

export function formatParamValue(v: number): string {
  return Number.isFinite(v) ? formatValue(v) : "n/a";
}

export function labelFor(target: string, labels: Record<string, string>): string {
  return labels[target] ?? target;
}
