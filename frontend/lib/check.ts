// Pure logic for the Check page: strict schemas mirroring
// engine/src/strategy.rs, plus summary, claim and provenance helpers.
// No network or model calls here, so vitest can cover it all.

import { z } from "zod";
import type {
  BacktestRequest,
  Condition,
  Operand,
  Op,
  Rules,
  Strategy,
} from "./api";

export const MAX_CHECK_CHARS = 8000;
export const CHECK_HOLD_BACK_SPLIT = 0.7;
const CHECK_YEARS_MS = 5 * 365 * 86_400_000;

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

const foundSchema = z.strictObject({
  status: z.literal("found"),
  strategy: strategySchema,
  parameters: z.array(checkParameterSchema).min(1),
  claim: checkClaimSchema,
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
export type ModelOutput = z.infer<typeof modelOutputSchema>;
export type FoundOutput = z.infer<typeof foundSchema>;

export interface NeedsInputView {
  strategy: Strategy;
  parameters: CheckParameter[];
  claim: CheckClaim;
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
  return {
    strategy,
    start_ms: Math.floor(nowMs - CHECK_YEARS_MS),
    end_ms: Math.floor(nowMs),
    split: CHECK_HOLD_BACK_SPLIT,
  };
}
