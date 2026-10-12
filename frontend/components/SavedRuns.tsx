// List of saved runs from GET /runs. Selecting one loads its detail.
"use client";

import type { RunSummary } from "@/lib/api";
import { fmtDateTime, fmtPct, plural } from "@/lib/testlab";

export default function SavedRuns({
  runs,
  loading,
  selectedId,
  onSelect,
}: {
  runs: RunSummary[];
  loading: boolean;
  selectedId: number | null;
  onSelect: (id: number) => void;
}) {
  return (
    <section aria-label="Saved tests" className="flex flex-col gap-2">
      <h2 className="text-base font-semibold">Saved tests</h2>
      {loading && <p className="text-sm text-[var(--muted)]">Loading saved tests…</p>}
      {!loading && runs.length === 0 && (
        <p className="text-sm text-[var(--muted)]">No saved tests yet.</p>
      )}
      {!loading && runs.length > 0 && (
        <ul className="flex flex-col gap-1">
          {runs.map((r) => (
            <li key={r.id}>
              <button
                type="button"
                onClick={() => onSelect(r.id)}
                aria-pressed={selectedId === r.id}
                className={`flex w-full flex-wrap items-center gap-x-3 gap-y-1 rounded border border-[var(--border)] px-3 py-2 text-left text-sm ${
                  selectedId === r.id ? "bg-[var(--surface)]" : ""
                }`}
              >
                <span className="font-medium">
                  #{r.id} {r.symbol} {r.interval}
                </span>
                <span className="text-[var(--muted)]">{fmtDateTime(r.created_at * 1000)}</span>
                <span>
                  {plural(r.trades, "trade")}, {fmtPct(r.total_return_pct)} return
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
