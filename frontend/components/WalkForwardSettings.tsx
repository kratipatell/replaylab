// Walk-forward settings: window layout, objective, and the parameters the
// optimizer may choose from in each window.
"use client";

import {
  WF_MAX_PARAMS,
  WF_MAX_WINDOWS,
  WF_MIN_WINDOWS,
  WF_MODES,
  WF_OBJECTIVES,
  createParamRow,
  kindForTarget,
  labelFor,
  suggestValues,
  type ParamRow,
  type TuneTarget,
  type WalkForwardForm,
} from "@/lib/walkforward";
import type { Objective, WalkForwardMode } from "@/lib/api";
import { inputClass, labelClass } from "./FormSections";

export default function WalkForwardSettings({
  value,
  onChange,
  targets,
  candleCount,
}: {
  value: WalkForwardForm;
  onChange: (next: WalkForwardForm) => void;
  targets: TuneTarget[];
  candleCount: number | null;
}) {
  const set = <K extends keyof WalkForwardForm>(key: K, next: WalkForwardForm[K]): void =>
    onChange({ ...value, [key]: next });

  const windows = Number(value.windows);
  const isBars = Number(value.isBars);
  const oosBars = Number(value.oosBars);
  const required =
    Number.isInteger(windows) && Number.isInteger(isBars) && Number.isInteger(oosBars)
      ? isBars + windows * oosBars
      : null;
  const mode = WF_MODES.find((m) => m.value === value.mode) ?? WF_MODES[0];

  return (
    <fieldset className="flex flex-col gap-4">
      <legend className="px-1 text-sm font-semibold">Walk-forward windows</legend>

      <div className="flex flex-col gap-2">
        <div className="flex flex-wrap gap-2" role="radiogroup" aria-label="Window mode">
          {WF_MODES.map((m) => (
            <button
              key={m.value}
              type="button"
              role="radio"
              aria-checked={value.mode === m.value}
              onClick={() => set("mode", m.value as WalkForwardMode)}
              className={`rounded border px-3 py-1.5 text-sm ${
                value.mode === m.value
                  ? "border-[var(--accent)] text-[var(--accent)]"
                  : "border-[var(--border)]"
              }`}
            >
              {m.label}
            </button>
          ))}
        </div>
        <p className="text-xs text-[var(--muted)]">{mode.help}</p>
      </div>

      <div className="flex flex-wrap gap-3">
        <label className={labelClass}>
          Windows
          <input
            className={`${inputClass} w-24`}
            type="number"
            step={1}
            min={WF_MIN_WINDOWS}
            max={WF_MAX_WINDOWS}
            value={value.windows}
            onChange={(e) => set("windows", e.target.value)}
          />
          <span className="text-xs font-normal text-[var(--muted)]">
            {WF_MIN_WINDOWS} to {WF_MAX_WINDOWS}
          </span>
        </label>
        <label className={labelClass}>
          In-sample bars
          <input
            className={`${inputClass} w-32`}
            type="number"
            step={1}
            min={1}
            value={value.isBars}
            onChange={(e) => set("isBars", e.target.value)}
          />
          <span className="text-xs font-normal text-[var(--muted)]">
            Bars the optimizer searches in each window.
          </span>
        </label>
        <label className={labelClass}>
          Out-of-sample bars
          <input
            className={`${inputClass} w-32`}
            type="number"
            step={1}
            min={1}
            value={value.oosBars}
            onChange={(e) => set("oosBars", e.target.value)}
          />
          <span className="text-xs font-normal text-[var(--muted)]">
            Bars each window is tested on, after its in-sample block.
          </span>
        </label>
      </div>

      {required !== null && candleCount !== null && (
        <p className="text-xs text-[var(--muted)]">
          Needs {required.toLocaleString()} candles; the date range holds about{" "}
          {candleCount.toLocaleString()}.
        </p>
      )}

      <label className={labelClass}>
        Choose the best parameters by
        <select
          className={`${inputClass} w-fit`}
          value={value.objective}
          onChange={(e) => set("objective", e.target.value as Objective)}
        >
          {WF_OBJECTIVES.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      </label>

      <ParamGridEditor
        rows={value.params}
        targets={targets}
        onChange={(params) => set("params", params)}
      />
    </fieldset>
  );
}

function ParamGridEditor({
  rows,
  targets,
  onChange,
}: {
  rows: ParamRow[];
  targets: TuneTarget[];
  onChange: (rows: ParamRow[]) => void;
}) {
  const usedBy = (target: string, except: string): boolean =>
    rows.some((r) => r.id !== except && r.target === target);
  const firstFree = targets.find((t) => !rows.some((r) => r.target === t.target));
  const labels = Object.fromEntries(targets.map((t) => [t.target, t.label]));

  const update = (id: string, patch: Partial<ParamRow>): void =>
    onChange(rows.map((r) => (r.id === id ? { ...r, ...patch } : r)));

  return (
    <div className="flex flex-col gap-2">
      <p className="text-sm font-semibold">Parameters to tune</p>
      <p className="text-xs text-[var(--muted)]">
        In each window the optimizer tries every combination of these values on
        the in-sample bars and keeps the best one. Values are comma-separated.
      </p>

      {targets.length === 0 && (
        <p className="text-sm text-[var(--muted)]">
          Add a threshold, an indicator period, a stop loss or a take profit to tune.
        </p>
      )}

      {rows.map((row) => {
        const known = targets.some((t) => t.target === row.target);
        const kind = row.target === "" ? null : kindForTarget(row.target);
        return (
          <div key={row.id} className="flex flex-wrap items-end gap-3">
            <label className={`${labelClass} min-w-64`}>
              Value to tune
              <select
                className={inputClass}
                value={row.target}
                onChange={(e) => {
                  const next = targets.find((t) => t.target === e.target.value);
                  update(row.id, {
                    target: e.target.value,
                    values: next === undefined ? "" : suggestValues(next),
                  });
                }}
              >
                <option value="">Choose…</option>
                {!known && row.target !== "" && (
                  <option value={row.target}>No longer in the rules</option>
                )}
                {targets.map((t) => (
                  <option key={t.target} value={t.target} disabled={usedBy(t.target, row.id)}>
                    {t.label}
                  </option>
                ))}
              </select>
            </label>
            <label className={labelClass}>
              Values
              <input
                className={`${inputClass} w-56`}
                type="text"
                inputMode="decimal"
                placeholder="e.g. 98, 100, 102"
                aria-label={`Values for ${labelFor(row.target, labels)}`}
                value={row.values}
                onChange={(e) => update(row.id, { values: e.target.value })}
              />
              <span className="text-xs font-normal text-[var(--muted)]">
                {kind === "int"
                  ? "Whole numbers, 1 or more."
                  : kind === "float"
                    ? "Any numbers."
                    : "Pick a value first."}
              </span>
            </label>
            <button
              type="button"
              className="rounded border border-[var(--border)] px-3 py-1.5 text-sm"
              onClick={() => onChange(rows.filter((r) => r.id !== row.id))}
            >
              Remove
            </button>
          </div>
        );
      })}

      <button
        type="button"
        disabled={firstFree === undefined || rows.length >= WF_MAX_PARAMS}
        onClick={() =>
          firstFree !== undefined &&
          onChange([...rows, createParamRow(firstFree.target, suggestValues(firstFree))])
        }
        className="w-fit rounded border border-[var(--border)] px-3 py-1.5 text-sm disabled:opacity-50"
      >
        Add parameter
      </button>
      {rows.length >= WF_MAX_PARAMS && (
        <p className="text-xs text-[var(--muted)]">At most {WF_MAX_PARAMS} parameters.</p>
      )}
    </div>
  );
}
