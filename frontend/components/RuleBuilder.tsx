// Editor for one rule group (entry or exit). Each group has an "all of"
// list and an "any of" list; a rule fires when every "all" rule and at
// least one "any" rule (if any exist) is true on a bar's close.
"use client";

import type { IndicatorKind, Op } from "@/lib/api";
import {
  INDICATORS,
  OPS,
  createRule,
  type RuleGroupState,
  type RuleOperandState,
  type RuleState,
} from "@/lib/testlab";

const INDICATOR_LABELS: Record<IndicatorKind, string> = {
  close: "Close price",
  open: "Open price",
  high: "High price",
  low: "Low price",
  sma: "Average (SMA)",
  ema: "Average (EMA)",
  rsi: "Strength (RSI)",
};

const OP_LABELS: Record<Op, string> = {
  "<": "is below",
  ">": "is above",
  crosses_above: "rises through",
  crosses_below: "falls through",
};

function needsPeriod(ind: IndicatorKind): boolean {
  return ind === "sma" || ind === "ema" || ind === "rsi";
}

const inputClass =
  "rounded border border-[var(--border)] bg-[var(--background)] px-2 py-1 text-sm";

function OperandEditor({
  label,
  value,
  onChange,
}: {
  label: string;
  value: RuleOperandState;
  onChange: (next: RuleOperandState) => void;
}) {
  return (
    <span className="flex flex-wrap items-center gap-1">
      <span className="sr-only">{label}</span>
      {value.mode === "indicator" ? (
        <>
          <select
            aria-label={`${label} price line`}
            className={inputClass}
            value={value.ind}
            onChange={(e) =>
              onChange({ ...value, ind: e.target.value as IndicatorKind })
            }
          >
            {INDICATORS.map((ind) => (
              <option key={ind} value={ind}>
                {INDICATOR_LABELS[ind]}
              </option>
            ))}
          </select>
          {needsPeriod(value.ind) && (
            <input
              aria-label={`${label} period in bars`}
              className={`${inputClass} w-16`}
              type="number"
              min={1}
              step={1}
              value={value.period}
              onChange={(e) => onChange({ ...value, period: e.target.value })}
            />
          )}
        </>
      ) : (
        <input
          aria-label={`${label} number`}
          className={`${inputClass} w-24`}
          type="number"
          step="any"
          placeholder="number"
          value={value.num}
          onChange={(e) => onChange({ ...value, num: e.target.value })}
        />
      )}
      <select
        aria-label={`${label} kind`}
        className={inputClass}
        value={value.mode}
        onChange={(e) =>
          onChange({
            ...value,
            mode: e.target.value as "number" | "indicator",
          })
        }
      >
        <option value="indicator">price line</option>
        <option value="number">number</option>
      </select>
    </span>
  );
}

function RuleRow({
  rule,
  onChange,
  onRemove,
}: {
  rule: RuleState;
  onChange: (next: RuleState) => void;
  onRemove: () => void;
}) {
  return (
    <div className="flex flex-wrap items-center gap-1">
      <OperandEditor
        label="Left side"
        value={rule.left}
        onChange={(left) => onChange({ ...rule, left })}
      />
      <select
        aria-label="Comparison"
        className={inputClass}
        value={rule.op}
        onChange={(e) => onChange({ ...rule, op: e.target.value as Op })}
      >
        {OPS.map((op) => (
          <option key={op} value={op}>
            {OP_LABELS[op]}
          </option>
        ))}
      </select>
      <OperandEditor
        label="Right side"
        value={rule.right}
        onChange={(right) => onChange({ ...rule, right })}
      />
      <button
        type="button"
        aria-label="Remove rule"
        onClick={onRemove}
        className="rounded border border-[var(--border)] px-2 py-1 text-sm text-[var(--muted)]"
      >
        Remove
      </button>
    </div>
  );
}

export default function RuleBuilder({
  title,
  hint,
  group,
  onChange,
}: {
  title: string;
  hint: string;
  group: RuleGroupState;
  onChange: (next: RuleGroupState) => void;
}) {
  const patchList = (
    list: "all" | "any",
    id: string,
    next: RuleState | null,
  ): void => {
    const updated =
      next === null
        ? group[list].filter((r) => r.id !== id)
        : group[list].map((r) => (r.id === id ? next : r));
    onChange({ ...group, [list]: updated });
  };

  const sections: { list: "all" | "any"; heading: string }[] = [
    { list: "all", heading: "All of these must be true" },
    { list: "any", heading: "At least one of these must be true" },
  ];
  const empty = group.all.length + group.any.length === 0;

  return (
    <fieldset className="flex flex-col gap-3 rounded border border-[var(--border)] p-3">
      <legend className="px-1 text-sm font-semibold">{title}</legend>
      <p className="text-xs text-[var(--muted)]">{hint}</p>
      {empty && <p className="text-xs text-[var(--muted)]">Add a rule to begin.</p>}
      {sections.map(({ list, heading }) => (
        <div key={list} className="flex flex-col gap-2">
          <p className="text-xs font-medium text-[var(--muted)]">{heading}</p>
          {!empty && group[list].length === 0 && (
            <p className="text-xs text-[var(--muted)]">No rules here yet.</p>
          )}
          {group[list].map((rule) => (
            <RuleRow
              key={rule.id}
              rule={rule}
              onChange={(next) => patchList(list, rule.id, next)}
              onRemove={() => patchList(list, rule.id, null)}
            />
          ))}
          <button
            type="button"
            onClick={() =>
              onChange({ ...group, [list]: [...group[list], createRule()] })
            }
            className="w-fit rounded border border-[var(--border)] px-2 py-1 text-sm"
          >
            Add rule
          </button>
        </div>
      ))}
    </fieldset>
  );
}
