// Pure logic for the Check page: strict schemas mirroring
// engine/src/strategy.rs, plus summary, claim and provenance helpers.
// No network or model calls here, so vitest can cover it all.

import { z } from "zod";
import { ApiError } from "./api";
import type {
  BacktestRequest,
  BacktestResponse,
  Condition,
  Grade,
  Operand,
  Op,
  Rules,
  Side,
  Stats,
  Strategy,
} from "./api";

export const MAX_CHECK_CHARS = 8000;
export const CHECK_HOLD_BACK_SPLIT = 0.7;
const CHECK_YEARS_MS = 5 * 365 * 86_400_000;

export const MULTI_TIMEFRAMES = ["15m", "1h", "4h", "1d"] as const;
export type MultiTimeframe = (typeof MULTI_TIMEFRAMES)[number];

const TIMEFRAME_STEP_MS: Record<string, number> = {
  "1m": 60_000,
  "5m": 5 * 60_000,
  "15m": 15 * 60_000,
  "1h": 3_600_000,
  "4h": 4 * 3_600_000,
  "1d": 86_400_000,
};
const DEFAULT_STEP_MS = 86_400_000;

export const MAX_CANDLES = 100_000;

export interface CandleRange {
  startMs: number;
  endMs: number;
  candles: number;
}

// Longest range ending now, up to five years, that stays under the candle cap.
export function rangeFit(timeframe: string, nowMs: number): CandleRange {
  const stepMs = TIMEFRAME_STEP_MS[timeframe] ?? DEFAULT_STEP_MS;
  const maxSpan = (MAX_CANDLES - 1) * stepMs;
  const span = Math.min(CHECK_YEARS_MS, maxSpan);
  const endMs = Math.floor(nowMs);
  const startMs = Math.floor(endMs - span);
  const candles = Math.floor((endMs - startMs) / stepMs) + 1;
  return { startMs, endMs, candles };
}

export const MULTI_TIMEFRAME_NOTE =
  "The video did not say which timeframe, so every one is shown, not only the best.";

export const RATE_LIMIT_MESSAGE = "Too many requests; try again in a minute";

const COUNT_WORDS = [
  "zero",
  "one",
  "two",
  "three",
  "four",
  "five",
  "six",
  "seven",
  "eight",
  "nine",
  "ten",
];

function countWord(n: number): string {
  return COUNT_WORDS[n] ?? String(n);
}

const sideSchema = z.enum(["long", "short"]);
const indicatorSchema = z.enum([
  "open",
  "high",
  "low",
  "close",
  "sma",
  "ema",
  "rsi",
]);
const opSchema = z.enum(["<", ">", "crosses_above", "crosses_below"]);

const indicatorRefSchema = z.strictObject({
  ind: indicatorSchema,
  period: z.number().int().min(1).max(100000).optional(),
});

const operandSchema: z.ZodType<Operand> = z.union([
  z.number(),
  indicatorRefSchema,
]) as z.ZodType<Operand>;

const conditionSchema = z
  .strictObject({
    left: operandSchema,
    op: opSchema,
    right: operandSchema,
  })
  .superRefine((c, ctx) => {
    if (typeof c.left === "number" && typeof c.right === "number") {
      ctx.addIssue({
        code: "custom",
        message: "condition compares two plain numbers",
      });
    }
  });

const rulesSchema = z.strictObject({
  all: z.array(conditionSchema).default([]),
  any: z.array(conditionSchema).default([]),
});

export const strategySchema = z
  .strictObject({
    asset: z.string().min(1).max(32),
    timeframe: z.string().min(1).max(8),
    side: sideSchema,
    entry: rulesSchema,
    exit: rulesSchema.nullable().optional(),
    stop_loss_pct: z.number().gt(0).max(100).optional(),
    take_profit_pct: z.number().gt(0).max(100).optional(),
  })
  .superRefine((s, ctx) => {
    const entryCount = s.entry.all.length + s.entry.any.length;
    if (entryCount === 0) {
      ctx.addIssue({ code: "custom", message: "entry has no conditions" });
    }
    const operands: Operand[] = [];
    for (const c of [...s.entry.all, ...s.entry.any]) {
      operands.push(c.left, c.right);
    }
    if (s.exit) {
      for (const c of [...s.exit.all, ...s.exit.any]) {
        operands.push(c.left, c.right);
      }
    }
    for (const o of operands) {
      if (typeof o === "object") {
        const needsPeriod =
          o.ind === "sma" || o.ind === "ema" || o.ind === "rsi";
        if (needsPeriod && o.period === undefined) {
          ctx.addIssue({
            code: "custom",
            message: `${o.ind} indicator requires a period`,
          });
        }
        if (!needsPeriod && o.period !== undefined) {
          ctx.addIssue({
            code: "custom",
            message: `${o.ind} indicator must not have a period`,
          });
        }
      }
    }
  });

export const paramSourceSchema = z.enum(["stated", "assumed", "missing"]);

export const checkParameterSchema = z
  .strictObject({
    name: z.string().min(1).max(120),
    path: z.string().min(1).max(120),
    value: z.number().nullable(),
    source: paramSourceSchema,
    quote: z.string().max(500).nullable().optional(),
    reason: z.string().max(500).nullable().optional(),
  })
  .superRefine((p, ctx) => {
    if (p.source === "stated") {
      if (p.value === null || !Number.isFinite(p.value)) {
        ctx.addIssue({
          code: "custom",
          message: `${p.name}: stated needs a numeric value`,
        });
      }
      if (!p.quote || p.quote.trim() === "") {
        ctx.addIssue({
          code: "custom",
          message: `${p.name}: stated needs the exact quote`,
        });
      }
    }
    if (p.source === "assumed") {
      if (p.value === null || !Number.isFinite(p.value)) {
        ctx.addIssue({
          code: "custom",
          message: `${p.name}: assumed needs a numeric value`,
        });
      }
      if (!p.reason || p.reason.trim() === "") {
        ctx.addIssue({
          code: "custom",
          message: `${p.name}: assumed needs a reason`,
        });
      }
    }
    if (p.source === "missing" && p.value !== null) {
      ctx.addIssue({
        code: "custom",
        message: `${p.name}: missing must have a null value`,
      });
    }
  });

export const checkClaimSchema = z.strictObject({
  win_rate_pct: z.number().min(0).max(100).nullable(),
  quote: z.string().max(500).nullable(),
});

export const contextSourceSchema = z.enum(["stated", "assumed"]);

export const contextFieldSchema = z
  .strictObject({
    value: z.string().min(1).max(32).nullable(),
    source: contextSourceSchema,
    quote: z.string().max(500).nullable().optional(),
    reason: z.string().max(500).nullable().optional(),
  })
  .superRefine((f, ctx) => {
    if (f.source === "stated") {
      if (f.value === null) {
        ctx.addIssue({ code: "custom", message: "stated needs a value" });
      }
      if (!f.quote || f.quote.trim() === "") {
        ctx.addIssue({
          code: "custom",
          message: "stated needs the exact quote",
        });
      }
    }
    if (f.source === "assumed" && (!f.reason || f.reason.trim() === "")) {
      ctx.addIssue({ code: "custom", message: "assumed needs a reason" });
    }
  });

export const checkContextSchema = z.strictObject({
  asset: contextFieldSchema,
  timeframe: contextFieldSchema,
  side: contextFieldSchema,
});

const foundSchema = z.strictObject({
  status: z.literal("found"),
  strategy: strategySchema,
  parameters: z.array(checkParameterSchema).min(1),
  claim: checkClaimSchema,
  context: checkContextSchema,
});

const noRuleSchema = z.strictObject({
  status: z.literal("no_rule_found"),
  reason: z.string().min(1).max(1000),
});

export const modelOutputSchema = z.discriminatedUnion("status", [
  foundSchema,
  noRuleSchema,
]);

export const checkRequestSchema = z.strictObject({
  text: z.string().min(1).max(MAX_CHECK_CHARS),
});

export type ParamSource = z.infer<typeof paramSourceSchema>;
export type CheckParameter = z.infer<typeof checkParameterSchema>;
export type CheckClaim = z.infer<typeof checkClaimSchema>;
export type ContextSource = z.infer<typeof contextSourceSchema>;
export type ContextField = z.infer<typeof contextFieldSchema>;
export type CheckContext = z.infer<typeof checkContextSchema>;
export type ModelOutput = z.infer<typeof modelOutputSchema>;
export type FoundOutput = z.infer<typeof foundSchema>;

export interface NeedsInputView {
  strategy: Strategy;
  parameters: CheckParameter[];
  claim: CheckClaim;
  context: CheckContext;
  missing: CheckParameter[];
}

function operandText(o: Operand): string {
  if (typeof o === "number") {
    return String(o);
  }
  const name = o.ind.toUpperCase();
  return o.period === undefined ? name : `${name}(${o.period})`;
}

function opText(op: Op): string {
  if (op === "crosses_above") {
    return "crosses above";
  }
  if (op === "crosses_below") {
    return "crosses below";
  }
  return op;
}

export function conditionText(c: Condition): string {
  return `${operandText(c.left)} ${opText(c.op)} ${operandText(c.right)}`;
}

export function rulesText(rules: Rules): string {
  const all = rules.all.map(conditionText);
  const any = rules.any.map(conditionText);
  const parts: string[] = [...all];
  if (any.length > 0) {
    parts.push(any.length === 1 ? any[0] : `(${any.join(" or ")})`);
  }
  return parts.join(" and ");
}

function displayAsset(asset: string): string {
  return asset.endsWith("USDT") ? asset.slice(0, -4) : asset;
}

export function buildSummary(strategy: Strategy): string {
  const coin = displayAsset(strategy.asset);
  const entrySide = strategy.side === "long" ? "Buy" : "Sell";
  const entry = `${entrySide} ${coin} when ${rulesText(strategy.entry)}.`;
  const exitRules = strategy.exit;
  const hasExit =
    exitRules !== undefined &&
    exitRules !== null &&
    exitRules.all.length + exitRules.any.length > 0;
  const exitSide = strategy.side === "long" ? "Sell" : "Buy back";
  const exit = hasExit
    ? ` ${exitSide} when ${rulesText(exitRules as Rules)}.`
    : "";
  const stop =
    strategy.stop_loss_pct !== undefined
      ? ` Stop ${String(strategy.stop_loss_pct)}%.`
      : "";
  const target =
    strategy.take_profit_pct !== undefined
      ? ` Target ${String(strategy.take_profit_pct)}%.`
      : "";
  return `${entry}${exit}${stop}${target}`;
}

export function missingParameters(params: CheckParameter[]): CheckParameter[] {
  return params.filter((p) => p.source === "missing");
}

// Only a fully-stated or user-confirmed strategy may reach the engine.
export function canRunBacktest(
  status: string,
  params: CheckParameter[],
): boolean {
  return status === "found" && params.every((p) => p.source !== "missing");
}

export function provenanceLabel(
  param: CheckParameter,
  overridden: boolean,
): string {
  if (overridden) {
    return "You chose";
  }
  if (param.source === "stated") {
    return "Stated in the video";
  }
  if (param.source === "assumed") {
    return "Assumed default";
  }
  return "Needs your input";
}

export function contextLabel(source: ContextSource, overridden: boolean): string {
  if (overridden) {
    return "You chose";
  }
  return source === "stated" ? "Stated in the video" : "Assumed default";
}

export interface ContextEdits {
  asset?: string;
  timeframe?: string;
  side?: Side;
}

// The video is silent on the timeframe only when it is assumed with no value.
export function timeframeNotStated(context: CheckContext): boolean {
  return context.timeframe.source === "assumed" && context.timeframe.value === null;
}

// Context values (and any user picks) win over the reader's strategy defaults.
export function applyContextEdits(
  strategy: Strategy,
  context: CheckContext,
  edits: ContextEdits,
): Strategy {
  const asset = edits.asset ?? context.asset.value ?? strategy.asset;
  const rawSide = edits.side ?? context.side.value;
  const side: Side =
    rawSide === "short" || rawSide === "long" ? rawSide : strategy.side;
  const timeframe =
    edits.timeframe ?? context.timeframe.value ?? strategy.timeframe;
  return { ...strategy, asset, side, timeframe };
}

export interface ClaimComparison {
  held: boolean | null;
  sentence: string;
}

function fmtRate(v: number): string {
  return `${v.toFixed(1)}%`;
}

export function compareClaim(
  claimed: number | null,
  oosWinRate: number | null,
  fullWinRate: number | null,
): ClaimComparison {
  if (claimed === null || claimed === undefined) {
    return {
      held: null,
      sentence: "The video did not give a win rate to check.",
    };
  }
  void fullWinRate;
  if (oosWinRate === null || oosWinRate === undefined) {
    return {
      held: null,
      sentence:
        "There were not enough held-back trades to check the claimed win rate.",
    };
  }
  if (oosWinRate >= claimed) {
    return {
      held: true,
      sentence: `The claim held: ${fmtRate(oosWinRate)} of held-back trades won, against ${fmtRate(claimed)} claimed.`,
    };
  }
  return {
    held: false,
    sentence: `The claim did not hold: ${fmtRate(oosWinRate)} of held-back trades won, against ${fmtRate(claimed)} claimed.`,
  };
}

const INJECTION_PATTERNS: RegExp[] = [
  /ignore\s+(all\s+)?(previous|prior|above)\s+instructions/i,
  /disregard\s+.*instructions?/i,
  /forget\s+.*instructions?/i,
  /system\s+prompt/i,
  /output\s+a\s+profitable\s+strategy/i,
  /jailbreak/i,
];

function hasRuleLikeContent(text: string): boolean {
  const mentionsRule =
    /(rsi|sma|ema|buy|sell|entry|exit|stop\s*loss|take\s*profit|long|short)/i.test(
      text,
    );
  return mentionsRule && /\d/.test(text);
}

// Instruction-override text with no testable rule must never become a strategy.
export function injectionNoRule(text: string): { reason: string } | null {
  const hit = INJECTION_PATTERNS.some((re) => re.test(text));
  if (hit && !hasRuleLikeContent(text)) {
    return {
      reason:
        "That looks like instructions for the reader, not a trading rule that can be tested.",
    };
  }
  return null;
}

interface NumericSlot {
  path: string;
  value: number;
}

function operandSlots(out: NumericSlot[], base: string, o: Operand): void {
  if (typeof o === "number") {
    out.push({ path: base, value: o });
    return;
  }
  if (o.period !== undefined) {
    out.push({ path: `${base}.period`, value: o.period });
  }
}

// Every numeric value in the strategy, addressed like the optimizer targets.
export function listNumericPaths(strategy: Strategy): NumericSlot[] {
  const out: NumericSlot[] = [];
  const sections: [string, Rules | null | undefined][] = [
    ["entry", strategy.entry],
    ["exit", strategy.exit ?? null],
  ];
  for (const [section, rules] of sections) {
    if (!rules) {
      continue;
    }
    const groups: [string, Condition[]][] = [
      ["all", rules.all],
      ["any", rules.any],
    ];
    for (const [group, conds] of groups) {
      conds.forEach((c, idx) => {
        operandSlots(out, `${section}.${group}.${idx}.left`, c.left);
        operandSlots(out, `${section}.${group}.${idx}.right`, c.right);
      });
    }
  }
  if (strategy.stop_loss_pct !== undefined) {
    out.push({ path: "stop_loss_pct", value: strategy.stop_loss_pct });
  }
  if (strategy.take_profit_pct !== undefined) {
    out.push({ path: "take_profit_pct", value: strategy.take_profit_pct });
  }
  return out;
}

function setPathValue(strategy: Strategy, path: string, value: number): void {
  if (path === "stop_loss_pct") {
    strategy.stop_loss_pct = value;
    return;
  }
  if (path === "take_profit_pct") {
    strategy.take_profit_pct = value;
    return;
  }
  const m =
    /^(entry|exit)\.(all|any)\.(\d+)\.(left|right)(\.period)?$/.exec(path);
  if (m === null) {
    throw new Error(`Unknown parameter path: ${path}`);
  }
  const section = m[1] as "entry" | "exit";
  const group = m[2] as "all" | "any";
  const idx = Number(m[3]);
  const side = m[4] as "left" | "right";
  const wantPeriod = m[5] === ".period";
  const rules = section === "entry" ? strategy.entry : strategy.exit;
  if (!rules) {
    throw new Error(`Unknown parameter path: ${path}`);
  }
  const cond = rules[group][idx];
  if (!cond) {
    throw new Error(`Unknown parameter path: ${path}`);
  }
  const current = cond[side];
  if (wantPeriod) {
    if (typeof current !== "object") {
      throw new Error(`Unknown parameter path: ${path}`);
    }
    const rounded = Math.round(value);
    if (!Number.isFinite(rounded) || rounded < 1) {
      throw new Error(`Period must be a whole number of 1 or more: ${path}`);
    }
    current.period = rounded;
    return;
  }
  if (typeof current !== "number") {
    throw new Error(`Unknown parameter path: ${path}`);
  }
  if (!Number.isFinite(value)) {
    throw new Error(`Value must be a number: ${path}`);
  }
  cond[side] = value as never;
}

export function applyParamEdits(
  strategy: Strategy,
  edits: Record<string, number>,
): Strategy {
  const next = JSON.parse(JSON.stringify(strategy)) as Strategy;
  for (const [path, value] of Object.entries(edits)) {
    if (!Number.isFinite(value)) {
      throw new Error(`Value must be a number: ${path}`);
    }
    setPathValue(next, path, value);
  }
  return next;
}

export function buildCheckBacktestRequest(
  strategy: Strategy,
  nowMs: number,
): BacktestRequest {
  const range = rangeFit(strategy.timeframe, nowMs);
  return {
    strategy,
    start_ms: range.startMs,
    end_ms: range.endMs,
    split: CHECK_HOLD_BACK_SPLIT,
  };
}

export function buildTimeframeRequest(
  strategy: Strategy,
  timeframe: MultiTimeframe,
  nowMs: number,
): BacktestRequest {
  const range = rangeFit(timeframe, nowMs);
  return {
    strategy: { ...strategy, timeframe },
    start_ms: range.startMs,
    end_ms: range.endMs,
    split: CHECK_HOLD_BACK_SPLIT,
  };
}

export interface TimeframeRun {
  timeframe: string;
  range: CandleRange;
  outOfSample: Stats | null;
  grade: Grade;
}

export interface TimeframeRow {
  timeframe: string;
  rangeStart: number;
  rangeEnd: number;
  trades: number | null;
  winRatePct: number | null;
  profitFactor: number | null;
  expectancyPct: number | null;
  grade: Grade;
}

export function timeframeRow(run: TimeframeRun): TimeframeRow {
  return {
    timeframe: run.timeframe,
    rangeStart: run.range.startMs,
    rangeEnd: run.range.endMs,
    trades: run.outOfSample?.trades ?? null,
    winRatePct: run.outOfSample?.win_rate_pct ?? null,
    profitFactor: run.outOfSample?.profit_factor ?? null,
    expectancyPct: run.outOfSample?.expectancy_pct ?? null,
    grade: run.grade,
  };
}

export function timeframeRows(runs: TimeframeRun[]): TimeframeRow[] {
  return runs.map(timeframeRow);
}

export interface MultiTimeframeRun {
  timeframe: MultiTimeframe;
  range: CandleRange;
  response: BacktestResponse;
}

export type TimeframeOutcome =
  | ({ status: "ok" } & MultiTimeframeRun)
  | { status: "failed"; timeframe: MultiTimeframe; error: string };

export type MultiTimeframeRow =
  | { status: "ok"; timeframe: MultiTimeframe; row: TimeframeRow }
  | { status: "failed"; timeframe: MultiTimeframe; error: string };

export function errorMessage(e: unknown): string {
  if (e instanceof ApiError && e.status === 429) {
    return RATE_LIMIT_MESSAGE;
  }
  return e instanceof Error ? e.message : String(e);
}

export function timeframeOutcome(
  timeframe: MultiTimeframe,
  result: PromiseSettledResult<{ range: CandleRange; response: BacktestResponse }>,
): TimeframeOutcome {
  if (result.status === "fulfilled") {
    return {
      status: "ok",
      timeframe,
      range: result.value.range,
      response: result.value.response,
    };
  }
  return { status: "failed", timeframe, error: errorMessage(result.reason) };
}

// One settled result per timeframe, in MULTI_TIMEFRAMES order.
export function multiTimeframeOutcomes(
  results: PromiseSettledResult<{
    range: CandleRange;
    response: BacktestResponse;
  }>[],
): TimeframeOutcome[] {
  return results.map((result, index) =>
    timeframeOutcome(MULTI_TIMEFRAMES[index], result),
  );
}

export function multiTimeframeRows(
  outcomes: TimeframeOutcome[],
): MultiTimeframeRow[] {
  return outcomes.map((outcome) => {
    if (outcome.status === "failed") {
      return {
        status: "failed",
        timeframe: outcome.timeframe,
        error: outcome.error,
      };
    }
    return {
      status: "ok",
      timeframe: outcome.timeframe,
      row: timeframeRow({
        timeframe: outcome.timeframe,
        range: outcome.range,
        outOfSample: outcome.response.out_of_sample,
        grade: outcome.response.verdict.grade,
      }),
    };
  });
}

// Only a timeframe that produced a result may count toward the verdict.
export function successfulRows(rows: MultiTimeframeRow[]): TimeframeRow[] {
  return rows.flatMap((row) => (row.status === "ok" ? [row.row] : []));
}

// One pass out of several is the kind of luck that a wider search produces.
export function weakEvidence(rows: TimeframeRow[]): string | null {
  if (rows.length < 2) {
    return null;
  }
  const holds = rows.filter((row) => row.grade === "holds").length;
  if (holds !== 1) {
    return null;
  }
  return `Only one of ${countWord(rows.length)} timeframes passed. Testing several variations makes a lucky result more likely, so treat this as weak evidence.`;
}

export interface DailyHit {
  count: number;
  resetAt: number;
}

export interface DailyLimitDecision {
  allowed: boolean;
  retryAt: number;
}

function nextUtcMidnight(nowMs: number): number {
  const now = new Date(nowMs);
  return Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate() + 1);
}

// Counts one hit against the key's UTC day and reports when the cap lifts.
export function takeDailyHit(
  nowMs: number,
  hits: Map<string, DailyHit>,
  key: string,
  limit: number,
): DailyLimitDecision {
  const entry = hits.get(key);
  if (!entry || nowMs >= entry.resetAt) {
    const resetAt = nextUtcMidnight(nowMs);
    hits.set(key, { count: 1, resetAt });
    return { allowed: true, retryAt: resetAt };
  }
  if (entry.count >= limit) {
    return { allowed: false, retryAt: entry.resetAt };
  }
  entry.count += 1;
  return { allowed: true, retryAt: entry.resetAt };
}

// First hop of x-forwarded-for; requests without one share the "unknown" bucket.
export function forwardedIp(header: string | null): string {
  const first = (header ?? "").split(",")[0].trim();
  return first === "" ? "unknown" : first;
}
