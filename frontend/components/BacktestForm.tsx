// The test setup form. Validation errors are shown by the page; the
// engine's own rejection is shown as-is when it disagrees.
"use client";

import type { Side } from "@/lib/api";
import {
  ASSETS,
  TIMEFRAMES,
  hasValidEntryRule,
  type Asset,
  type DatePreset,
  type TestLabForm,
  type Timeframe,
} from "@/lib/testlab";
import RuleBuilder from "./RuleBuilder";

const inputClass =
  "rounded border border-[var(--border)] bg-[var(--background)] px-2 py-1 text-sm";

const labelClass = "flex flex-col gap-1 text-sm font-medium";

export default function BacktestForm({
  form,
  onChange,
  onSubmit,
  loading,
}: {
  form: TestLabForm;
  onChange: (next: TestLabForm) => void;
  onSubmit: () => void;
  loading: boolean;
}) {
  const set = <K extends keyof TestLabForm>(key: K, value: TestLabForm[K]): void =>
    onChange({ ...form, [key]: value });

  return (
    <form
      className="flex flex-col gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        onSubmit();
      }}
    >
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
              From
              <input
                className={inputClass}
                type="datetime-local"
                value={form.customStart}
                onChange={(e) => set("customStart", e.target.value)}
              />
            </label>
            <label className={labelClass}>
              To
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
      </fieldset>

      <label className={labelClass}>
        Held-back share for checking (0 to 1)
        <input
          className={`${inputClass} w-32`}
          type="number"
          step="any"
          min={0}
          max={1}
          value={form.split}
          onChange={(e) => set("split", e.target.value)}
        />
        <span className="text-xs font-normal text-[var(--muted)]">
          0.7 means the last 30% of data is held back to check the idea.
        </span>
      </label>

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

      <button
        type="submit"
        disabled={loading || !hasValidEntryRule(form)}
        title={
          hasValidEntryRule(form)
            ? undefined
            : "Add at least one valid entry rule first"
        }
        className="w-fit rounded bg-[var(--accent)] px-4 py-2 text-sm font-medium text-[var(--accent-foreground)] disabled:cursor-not-allowed disabled:opacity-50"
      >
        {loading ? "Running test…" : "Run test"}
      </button>
    </form>
  );
}
