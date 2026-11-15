// The test setup form. Validation errors are shown by the page; the
// engine's own rejection is shown as-is when it disagrees.
"use client";

import {
  HOLD_BACK_MAX_PCT,
  HOLD_BACK_MIN_PCT,
  MAX_CANDLES,
  estimateCandles,
  hasValidEntryRule,
  plural,
  type TestLabForm,
} from "@/lib/testlab";
import { CostFields, DateRangeFields, StrategyFields, inputClass, labelClass } from "./FormSections";

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

  const candleCount = estimateCandles(form);
  const overLimit = candleCount !== null && candleCount > MAX_CANDLES;
  const canRun = hasValidEntryRule(form) && !overLimit;
  const runTitle = overLimit
    ? `That range holds about ${candleCount === null ? "?" : plural(candleCount, "candle")}, over the ${MAX_CANDLES.toLocaleString()} limit`
    : hasValidEntryRule(form)
      ? undefined
      : "Add at least one valid entry rule first";

  return (
    <form
      className="flex flex-col gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        onSubmit();
      }}
    >
      <StrategyFields form={form} onChange={onChange} />
      <DateRangeFields form={form} onChange={onChange} />

      <label className={labelClass}>
        Hold back the last (%) to check the idea
        <input
          className={`${inputClass} w-32`}
          type="number"
          step={1}
          min={HOLD_BACK_MIN_PCT}
          max={HOLD_BACK_MAX_PCT}
          value={form.split}
          onChange={(e) => set("split", e.target.value)}
        />
        <span className="text-xs font-normal text-[var(--muted)]">
          30 means the last 30% of data is held back to check the idea (allowed{" "}
          {HOLD_BACK_MIN_PCT} to {HOLD_BACK_MAX_PCT}).
        </span>
      </label>

      <CostFields form={form} onChange={onChange} />

      <button
        type="submit"
        disabled={loading || !canRun}
        title={runTitle}
        className="w-fit rounded bg-[var(--accent)] px-4 py-2 text-sm font-medium text-[var(--accent-foreground)] disabled:cursor-not-allowed disabled:opacity-50"
      >
        {loading ? "Running test…" : "Run test"}
      </button>
    </form>
  );
}
