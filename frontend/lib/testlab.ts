// Pure logic for the Test Lab page: form defaults, validation mirroring
// what the engine checks, request building, downsampling and formatting.

import type {
  BacktestRequest,
  Condition,
  ExitReason,
  Grade,
  IndicatorKind,
  Operand,
  Op,
  Rules,
  Side,
} from "./api";

export const ASSETS = [
  "BTCUSDT",
  "ETHUSDT",
  "SOLUSDT",
  "BNBUSDT",
  "XRPUSDT",
  "DOGEUSDT",
  "ADAUSDT",
  "AVAXUSDT",
] as const;
export type Asset = (typeof ASSETS)[number];

export const TIMEFRAMES = ["1m", "5m", "15m", "1h", "4h", "1d"] as const;
export type Timeframe = (typeof TIMEFRAMES)[number];

export const INDICATORS: IndicatorKind[] = [
  "close",
  "open",
  "high",
  "low",
  "sma",
  "ema",
  "rsi",
];

export const OPS: Op[] = ["<", ">", "crosses_above", "crosses_below"];

export const DEFAULT_FEE_BPS = 5;
export const DEFAULT_SLIPPAGE_BPS = 2;
export const DEFAULT_HOLD_BACK_PCT = 30;
export const HOLD_BACK_MIN_PCT = 10;
export const HOLD_BACK_MAX_PCT = 50;
export const DEFAULT_SPLIT = 0.7;
export const MAX_CHART_POINTS = 600;
export const MAX_CANDLES = 100_000;

const STEP_MS: Record<Timeframe, number> = {
  "1m": 60_000,
  "5m": 5 * 60_000,
  "15m": 15 * 60_000,
  "1h": 3_600_000,
  "4h": 4 * 3_600_000,
  "1d": 86_400_000,
};

export function holdBackToSplit(pct: number): number {
  return 1 - pct / 100;
}

export function splitToHoldBack(split: number): number {
  return (1 - split) * 100;
}

const YEAR_MS = 365 * 86_400_000;

export type DatePreset = "1y" | "3y" | "5y" | "custom";

export interface RuleOperandState {
  mode: "number" | "indicator";
  num: string;
  ind: IndicatorKind;
  period: string;
}

export interface RuleState {
  id: string;
  left: RuleOperandState;
  op: Op;
  right: RuleOperandState;
}

export interface RuleGroupState {
  all: RuleState[];
  any: RuleState[];
}

export interface TestLabForm {
  asset: Asset;
  timeframe: Timeframe;
  side: Side;
  entry: RuleGroupState;
  exit: RuleGroupState;
  stopLoss: string;
  takeProfit: string;
  preset: DatePreset;
  customStart: string;
  customEnd: string;
  split: string;
  feeBps: string;
  slippageBps: string;
}

let ruleCounter = 0;

export function createRule(): RuleState {
  ruleCounter += 1;
  return {
    id: `rule-${ruleCounter}`,
    left: { mode: "indicator", num: "", ind: "close", period: "14" },
    op: ">",
    right: { mode: "number", num: "", ind: "sma", period: "20" },
  };
}

export function defaultForm(): TestLabForm {
  return {
    asset: "BTCUSDT",
    timeframe: "1h",
    side: "long",
    entry: { all: [], any: [] },
    exit: { all: [], any: [] },
    stopLoss: "",
    takeProfit: "",
    preset: "1y",
    customStart: "",
    customEnd: "",
    split: String(DEFAULT_HOLD_BACK_PCT),
    feeBps: String(DEFAULT_FEE_BPS),
    slippageBps: String(DEFAULT_SLIPPAGE_BPS),
  };
}

// A datetime-local value has no zone; treat it as UTC so everyone sees
// the same range regardless of browser locale.
export function parseCustomUtc(raw: string): number {
  const s = raw.trim();
  if (s === "") {
    return NaN;
  }
  if (/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(s)) {
    return Date.parse(`${s}:00Z`);
  }
  if (/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}$/.test(s)) {
    return Date.parse(`${s}Z`);
  }
  if (/[Zz]$/.test(s) || /[+-]\d{2}:?\d{2}$/.test(s)) {
    return Date.parse(s);
  }
  return Date.parse(`${s}Z`);
}

export function resolveRange(
  form: TestLabForm,
  nowMs: number,
): { startMs: number; endMs: number } {
  if (form.preset === "custom") {
    return { startMs: parseCustomUtc(form.customStart), endMs: parseCustomUtc(form.customEnd) };
  }
  const years = form.preset === "1y" ? 1 : form.preset === "3y" ? 3 : 5;
  return { startMs: nowMs - years * YEAR_MS, endMs: nowMs };
}

export function candleCountFor(form: TestLabForm, nowMs: number): number | null {
  const { startMs, endMs } = resolveRange(form, nowMs);
  if (!Number.isFinite(startMs) || !Number.isFinite(endMs) || startMs >= endMs) {
    return null;
  }
  return Math.floor((endMs - startMs) / STEP_MS[form.timeframe]) + 1;
}

export function isOverCandleLimit(form: TestLabForm, nowMs: number): boolean {
  const count = candleCountFor(form, nowMs);
  return count !== null && count > MAX_CANDLES;
}

// Pure estimate for render: presets span an exact number of years, so no
// clock read is needed to guess the candle count in the form.
export function estimateCandles(form: TestLabForm): number | null {
  if (form.preset === "custom") {
    const startMs = parseCustomUtc(form.customStart);
    const endMs = parseCustomUtc(form.customEnd);
    if (!Number.isFinite(startMs) || !Number.isFinite(endMs) || startMs >= endMs) {
      return null;
    }
    return Math.floor((endMs - startMs) / STEP_MS[form.timeframe]) + 1;
  }
  const years = form.preset === "1y" ? 1 : form.preset === "3y" ? 3 : 5;
  return Math.floor((years * YEAR_MS) / STEP_MS[form.timeframe]) + 1;
}

export function longestFittingPresetStatic(form: TestLabForm): DatePreset | null {
  const presets: DatePreset[] = ["5y", "3y", "1y"];
  for (const preset of presets) {
    const count = estimateCandles({ ...form, preset });
    if (count !== null && count <= MAX_CANDLES) {
      return preset;
    }
  }
  return null;
}

// Longest date preset that fits under the candle limit for this timeframe.
export function longestFittingPreset(
  form: TestLabForm,
  nowMs: number,
): DatePreset | null {
  const presets: DatePreset[] = ["5y", "3y", "1y"];
  for (const preset of presets) {
    const probe: TestLabForm = { ...form, preset };
    const count = candleCountFor(probe, nowMs);
    if (count !== null && count <= MAX_CANDLES) {
      return preset;
    }
  }
  return null;
}

export function holdBackPct(form: TestLabForm): number {
  return Number(form.split);
}

function needsPeriod(ind: IndicatorKind): boolean {
  return ind === "sma" || ind === "ema" || ind === "rsi";
}

export function plural(count: number, singular: string): string {
  return `${count.toLocaleString()} ${count === 1 ? singular : `${singular}s`}`;
}

function operandValid(o: RuleOperandState): boolean {
  if (o.mode === "number") {
    return o.num.trim() !== "" && Number.isFinite(Number(o.num));
  }
  if (needsPeriod(o.ind)) {
    const p = Number(o.period);
    return Number.isInteger(p) && p >= 1;
  }
  return true;
}

function ruleValid(rule: RuleState): boolean {
  if (!operandValid(rule.left) || !operandValid(rule.right)) {
    return false;
  }
  return !(rule.left.mode === "number" && rule.right.mode === "number");
}

export function hasValidEntryRule(form: TestLabForm): boolean {
  return [...form.entry.all, ...form.entry.any].some(ruleValid);
}

function checkOperand(
  o: RuleOperandState,
  errors: string[],
  where: string,
): void {
  if (operandValid(o)) {
    return;
  }
  if (o.mode === "number") {
    errors.push(`${where}: enter a number on each plain-number side.`);
  } else {
    errors.push(`${where}: ${o.ind} needs a whole period of 1 or more.`);
  }
}

function toOperand(o: RuleOperandState): Operand {
  if (o.mode === "number") {
    return Number(o.num);
  }
  if (needsPeriod(o.ind)) {
    return { ind: o.ind, period: Number(o.period) };
  }
  return { ind: o.ind };
}

// Mirrors Strategy::validate plus the range/fee/split checks in post_backtest.
export function validateForm(form: TestLabForm, nowMs: number): string[] {
  const errors: string[] = [];
  const entryCount = form.entry.all.length + form.entry.any.length;
  if (entryCount === 0) {
    errors.push("Add at least one entry rule: the test needs to know when to buy or sell.");
  }
  const groups: [RuleGroupState, string][] = [
    [form.entry, "Entry"],
    [form.exit, "Exit"],
  ];
  for (const [group, name] of groups) {
    const rules = [...group.all, ...group.any];
    rules.forEach((rule, i) => {
      const where = `${name} rule ${i + 1}`;
      checkOperand(rule.left, errors, where);
      checkOperand(rule.right, errors, where);
      if (rule.left.mode === "number" && rule.right.mode === "number") {
        errors.push(`${where}: compare a price line against something; two plain numbers cannot trigger a trade.`);
      }
    });
  }
  for (const [raw, name] of [
    [form.stopLoss, "Stop loss"],
    [form.takeProfit, "Take profit"],
  ] as const) {
    if (raw.trim() !== "" && (!Number.isFinite(Number(raw)) || Number(raw) <= 0)) {
      errors.push(`${name} must be a number above 0, or left empty.`);
    }
  }
  const { startMs, endMs } = resolveRange(form, nowMs);
  if (!Number.isFinite(startMs) || !Number.isFinite(endMs)) {
    errors.push("Pick a valid start and end date.");
  } else if (startMs >= endMs) {
    errors.push("The start date must be before the end date.");
  } else {
    const count = Math.floor((endMs - startMs) / STEP_MS[form.timeframe]) + 1;
    if (count > MAX_CANDLES) {
      errors.push(
        `That range holds about ${plural(count, "candle")}, over the 100,000 limit. Shorten the range or use a longer timeframe.`,
      );
    }
  }
  const pct = Number(form.split);
  if (!Number.isFinite(pct) || pct < HOLD_BACK_MIN_PCT || pct > HOLD_BACK_MAX_PCT) {
    errors.push(
      `Hold back the last ${HOLD_BACK_MIN_PCT} to ${HOLD_BACK_MAX_PCT}% to check the idea (30 means the last 30% of data is held back).`,
    );
  }
  for (const [raw, name] of [
    [form.feeBps, "Fees"],
    [form.slippageBps, "Slippage"],
  ] as const) {
    const v = Number(raw);
    if (!Number.isFinite(v) || v < 0 || v > 1000) {
      errors.push(`${name} must be between 0 and 1000.`);
    }
  }
  return errors;
}

function toCondition(rule: RuleState): Condition {
  return { left: toOperand(rule.left), op: rule.op, right: toOperand(rule.right) };
}

function toRules(group: RuleGroupState): Rules {
  return {
    all: group.all.map(toCondition),
    any: group.any.map(toCondition),
  };
}

export function buildRequest(form: TestLabForm, nowMs: number): BacktestRequest {
  const { startMs, endMs } = resolveRange(form, nowMs);
  const exitCount = form.exit.all.length + form.exit.any.length;
  const body: BacktestRequest = {
    strategy: {
      asset: form.asset,
      timeframe: form.timeframe,
      side: form.side,
      entry: toRules(form.entry),
    },
    start_ms: Math.floor(startMs),
    end_ms: Math.floor(endMs),
    fee_bps: Number(form.feeBps),
    slippage_bps: Number(form.slippageBps),
    split: holdBackToSplit(Number(form.split)),
  };
  if (exitCount > 0) {
    body.strategy.exit = toRules(form.exit);
  }
  if (form.stopLoss.trim() !== "") {
    body.strategy.stop_loss_pct = Number(form.stopLoss);
  }
  if (form.takeProfit.trim() !== "") {
    body.strategy.take_profit_pct = Number(form.takeProfit);
  }
  return body;
}

// Even stride, always keeping the last point so the curve ends on time.
export function downsample(values: number[], max: number): number[] {
  if (values.length <= max) {
    return values;
  }
  const stride = Math.ceil(values.length / max);
  const out: number[] = [];
  for (let i = 0; i < values.length; i += stride) {
    out.push(values[i]);
  }
  const last = values[values.length - 1];
  if (out[out.length - 1] !== last) {
    out.push(last);
  }
  return out;
}

export function drawdownPct(equity: number[]): number[] {
  let peak = Number.NEGATIVE_INFINITY;
  return equity.map((e) => {
    if (e > peak) {
      peak = e;
    }
    if (!Number.isFinite(peak) || peak === 0) {
      return 0;
    }
    return ((peak - e) / peak) * 100;
  });
}

export function gradeLabel(grade: Grade): string {
  switch (grade) {
    case "holds":
      return "Holds";
    case "fragile":
      return "Fragile";
    case "fails":
      return "Fails";
    case "not_enough_data":
      return "Not enough data";
  }
}

export function gradeColorVar(grade: Grade): string {
  switch (grade) {
    case "holds":
      return "var(--verdict-holds)";
    case "fragile":
      return "var(--verdict-fragile)";
    case "fails":
      return "var(--verdict-fails)";
    case "not_enough_data":
      return "var(--verdict-not-enough-data)";
  }
}

export function exitReasonLabel(reason: ExitReason): string {
  switch (reason) {
    case "Signal":
      return "Exit rule";
    case "StopLoss":
      return "Stop loss";
    case "TakeProfit":
      return "Take profit";
    case "Trailing":
      return "Trailing stop";
    case "SessionEnd":
      return "End of session";
    case "EndOfData":
      return "End of data";
  }
}

export function exitReasonHelp(reason: ExitReason): string {
  switch (reason) {
    case "Signal":
      return "An exit rule you wrote became true.";
    case "StopLoss":
      return "Price fell to the stop-loss percent you set.";
    case "TakeProfit":
      return "Price rose to the take-profit percent you set.";
    case "Trailing":
      return "A trailing stop followed the price and closed the trade.";
    case "SessionEnd":
      return "The holding window ended before any other exit fired.";
    case "EndOfData":
      return "The data ran out while the trade was still open.";
  }
}

export function fmtPct(v: number | null | undefined, digits = 2): string {
  if (v === null || v === undefined || !Number.isFinite(v)) {
    return "—";
  }
  return `${v.toFixed(digits)}%`;
}

export function fmtNum(v: number | null | undefined, digits = 2): string {
  if (v === null || v === undefined || !Number.isFinite(v)) {
    return "—";
  }
  return v.toFixed(digits);
}

export function fmtDateTime(ms: number): string {
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) {
    return "—";
  }
  return `${d.toISOString().slice(0, 16).replace("T", " ")} UTC`;
}

export function fmtDate(ms: number): string {
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) {
    return "—";
  }
  return `${d.toISOString().slice(0, 10)}`;
}

export const MONTH_NAMES = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
] as const;

export interface MonthlyGrid {
  years: number[];
  cells: Map<string, number>;
}

export function groupMonthly(monthly: Record<string, number>): MonthlyGrid {
  const cells = new Map<string, number>();
  const yearSet = new Set<number>();
  for (const [key, value] of Object.entries(monthly)) {
    const m = /^(\d{4})-(\d{2})$/.exec(key);
    if (m === null) {
      continue;
    }
    const year = Number(m[1]);
    const month = Number(m[2]);
    if (!Number.isInteger(year) || !Number.isInteger(month) || month < 1 || month > 12) {
      continue;
    }
    if (typeof value !== "number" || !Number.isFinite(value)) {
      continue;
    }
    cells.set(key, value);
    yearSet.add(year);
  }
  return { years: [...yearSet].sort((a, b) => a - b), cells };
}

// Points at the likely form field behind an engine rejection.
export function fixHint(message: string): string {
  const m = message.toLowerCase();
  if (m.includes("period")) {
    return "Check the bar count next to Average and Strength lines: it must be a whole number of 1 or more.";
  }
  if (m.includes("entry")) {
    return "Add at least one entry rule above so the test knows when to trade.";
  }
  if (m.includes("plain number")) {
    return "Each rule must involve a price line on at least one side; two plain numbers cannot trigger a trade.";
  }
  if (m.includes("stop_loss") || m.includes("take_profit")) {
    return "Stop loss and take profit must be numbers above 0, or left empty.";
  }
  if (m.includes("split")) {
    return "Set the held-back percent between 10 and 50, for example 30.";
  }
  if (m.includes("fee") || m.includes("slippage")) {
    return "Check Fees and Slippage under Advanced: both must be between 0 and 1000.";
  }
  if (m.includes("symbol")) {
    return `Use one of the listed coins (${ASSETS.join(", ")}).`;
  }
  if (m.includes("interval") || m.includes("timeframe")) {
    return `Use one of the listed bar sizes (${TIMEFRAMES.join(", ")}).`;
  }
  if (
    m.includes("start") ||
    m.includes("end") ||
    m.includes("range") ||
    m.includes("no candles") ||
    m.includes("100000") ||
    m.includes("limit")
  ) {
    return "Shorten the date range or pick a longer bar size so fewer than 100,000 candles are loaded.";
  }
  return "Check the form above against this message, then run the test again.";
}
