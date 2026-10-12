// The honest verdict, shown first in the results.
"use client";

import type { Verdict } from "@/lib/api";
import { gradeColorVar, gradeLabel } from "@/lib/testlab";

export default function VerdictCard({
  verdict,
  onUseLongRange,
}: {
  verdict: Verdict;
  onUseLongRange: () => void;
}) {
  const color = gradeColorVar(verdict.grade);
  if (verdict.grade === "not_enough_data") {
    return (
      <section
        aria-label="Verdict"
        className="flex flex-col gap-2 rounded border border-[var(--border)] bg-[var(--surface)] p-4"
      >
        <p className="text-xl font-bold" style={{ color }}>
          {gradeLabel(verdict.grade)}
        </p>
        <p className="text-sm">{verdict.headline}</p>
        {verdict.reasons.length > 0 && (
          <p className="text-sm text-[var(--muted)]">{verdict.reasons[0]}</p>
        )}
        <p className="text-sm">Try a longer date range or a shorter timeframe.</p>
        <button
          type="button"
          onClick={onUseLongRange}
          className="w-fit rounded bg-[var(--accent)] px-3 py-1.5 text-sm font-medium text-[var(--accent-foreground)]"
        >
          Use 5-year range
        </button>
      </section>
    );
  }
  return (
    <section
      aria-label="Verdict"
      className="flex flex-col gap-2 rounded border border-[var(--border)] bg-[var(--surface)] p-4"
    >
      <p className="text-2xl font-bold" style={{ color }}>
        {gradeLabel(verdict.grade)}
      </p>
      <p className="text-sm font-medium">{verdict.headline}</p>
      <ul className="flex list-disc flex-col gap-1 pl-5 text-sm text-[var(--muted)]">
        {verdict.reasons.map((reason) => (
          <li key={reason}>{reason}</li>
        ))}
      </ul>
    </section>
  );
}
