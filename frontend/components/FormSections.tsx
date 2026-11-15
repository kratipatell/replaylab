// Strategy, date range and cost fields shared by the single test and the
// walk-forward mode. Each section edits the same TestLabForm.
"use client";

import type { Side } from "@/lib/api";
import {
  ASSETS,
  MAX_CANDLES,
  TIMEFRAMES,
  estimateCandles,
  longestFittingPresetStatic,
  plural,
  type Asset,
  type DatePreset,
  type TestLabForm,
  type Timeframe,
} from "@/lib/testlab";
import RuleBuilder from "./RuleBuilder";

export const inputClass =
  "rounded border border-[var(--border)] bg-[var(--background)] px-2 py-1 text-sm";

export const labelClass = "flex flex-col gap-1 text-sm font-medium";

interface SectionProps {
  form: TestLabForm;
  onChange: (next: TestLabForm) => void;
}

export function StrategyFields({ form, onChange }: SectionProps) {
  const set = <K extends keyof TestLabForm>(key: K, value: TestLabForm[K]): void =>
    onChange({ ...form, [key]: value });

  return (
    <>
      <div className="flex flex-wrap gap-3">
        <label className={labelClass}>
          Coin
          <select
            className={inputClass}
            value={form.asset}
            onChange={(e) => set("asset", e.target.value as Asset)}
          >
            {ASSETS.map((a) => (
              <option key={a} value={a}>
                {a}
              </option>
            ))}
          </select>
        </label>
        <label className={labelClass}>
          Bar size
          <select
            className={inputClass}
            value={form.timeframe}
            onChange={(e) => set("timeframe", e.target.value as Timeframe)}
          >
            {TIMEFRAMES.map((t) => (
              <option key={t} value={t}>
                {t}
              </option>
            ))}
          </select>
        </label>
        <label className={labelClass}>
          Direction
          <select
            className={inputClass}
            value={form.side}
            onChange={(e) => set("side", e.target.value as Side)}
          >
            <option value="long">Buy first (long)</option>
            <option value="short">Sell first (short)</option>
          </select>
        </label>
      </div>

      <RuleBuilder
        title="Entry rules"
        hint="When to open a trade. Checked at each bar's close; the trade opens at the next bar."
        group={form.entry}
        onChange={(entry) => onChange({ ...form, entry })}
      />
      <RuleBuilder
        title="Exit rules"
        hint="When to close a trade. Leave empty to hold until the stop, target, or end of data."
        group={form.exit}
        onChange={(exit) => onChange({ ...form, exit })}
      />

      <div className="flex flex-wrap gap-3">
        <label className={labelClass}>
          Stop loss % (optional)
          <input
            className={`${inputClass} w-32`}
            type="number"
            step="any"
            min={0}
            placeholder="e.g. 2"
            value={form.stopLoss}
            onChange={(e) => set("stopLoss", e.target.value)}
          />
        </label>
        <label className={labelClass}>
          Take profit % (optional)
          <input
            className={`${inputClass} w-32`}
            type="number"
            step="any"
            min={0}
            placeholder="e.g. 4"
            value={form.takeProfit}
            onChange={(e) => set("takeProfit", e.target.value)}
          />
        </label>
      </div>
    </>
  );
}

export function DateRangeFields({ form, onChange }: SectionProps) {
  const set = <K extends keyof TestLabForm>(key: K, value: TestLabForm[K]): void =>
    onChange({ ...form, [key]: value });

  const candleCount = estimateCandles(form);
  const overLimit = candleCount !== null && candleCount > MAX_CANDLES;
  const fitting = overLimit ? longestFittingPresetStatic(form) : null;

  return (
    <fieldset className="flex flex-col gap-2">
      <legend className="px-1 text-sm font-semibold">Date range</legend>
      <div className="flex flex-wrap gap-2" role="radiogroup" aria-label="Date range preset">
        {(
          [
            ["1y", "Past year"],
            ["3y", "Past 3 years"],
            ["5y", "Past 5 years"],
            ["custom", "Custom"],
          ] as [DatePreset, string][]
        ).map(([preset, label]) => (
          <button
            key={preset}
            type="button"
            role="radio"
            aria-checked={form.preset === preset}
            onClick={() => set("preset", preset)}
            className={`rounded border px-3 py-1.5 text-sm ${
              form.preset === preset
                ? "border-[var(--accent)] text-[var(--accent)]"
                : "border-[var(--border)]"
            }`}
          >
            {label}
          </button>
        ))}
      </div>
      {form.preset === "custom" ? (
        <div className="flex flex-wrap gap-3">
          <label className={labelClass}>
            From (UTC)
            <input
              className={inputClass}
              type="datetime-local"
              value={form.customStart}
              onChange={(e) => set("customStart", e.target.value)}
            />
          </label>
          <label className={labelClass}>
            To (UTC)
            <input
              className={inputClass}
              type="datetime-local"
              value={form.customEnd}
              onChange={(e) => set("customEnd", e.target.value)}
            />
          </label>
        </div>
      ) : (
        <p className="text-xs text-[var(--muted)]">Ends now.</p>
      )}
      <p className="text-xs text-[var(--muted)]">
        Dates are in UTC.
        {candleCount !== null &&
          ` About ${plural(candleCount, "candle")} at this bar size (limit ${MAX_CANDLES.toLocaleString()}).`}
      </p>
      {overLimit && (
        <p role="alert" className="text-sm text-[var(--verdict-fails)]">
          That range is over the {MAX_CANDLES.toLocaleString()}-candle limit.{" "}
          {fitting !== null ? (
            <button
              type="button"
              className="underline"
              onClick={() => set("preset", fitting)}
            >
              Use {fitting} instead
            </button>
          ) : (
            "Shorten the range or use a longer bar size."
          )}
        </p>
      )}
    </fieldset>
  );
}

export function CostFields({ form, onChange }: SectionProps) {
  const set = <K extends keyof TestLabForm>(key: K, value: TestLabForm[K]): void =>
    onChange({ ...form, [key]: value });

  return (
    <details className="rounded border border-[var(--border)] p-3">
      <summary className="cursor-pointer text-sm font-semibold">
        Advanced: trading costs
      </summary>
      <div className="mt-2 flex flex-wrap gap-3">
        <label className={labelClass}>
          Fees (per 10,000)
          <input
            className={`${inputClass} w-32`}
            type="number"
            step="any"
            min={0}
            value={form.feeBps}
            onChange={(e) => set("feeBps", e.target.value)}
          />
        </label>
        <label className={labelClass}>
          Slippage (per 10,000)
          <input
            className={`${inputClass} w-32`}
            type="number"
            step="any"
            min={0}
            value={form.slippageBps}
            onChange={(e) => set("slippageBps", e.target.value)}
          />
        </label>
      </div>
      <p className="mt-1 text-xs text-[var(--muted)]">
        Costs are always on. Defaults match the engine: 5 fees, 2 slippage.
      </p>
    </details>
  );
}
