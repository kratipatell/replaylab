"use client";

import type { AgentDecision } from "../lib/api";
import { decisionPresentation } from "../lib/agents";
import { fmtDateTime, fmtNum } from "../lib/testlab";

export default function DecisionLog({
  decisions,
  loading,
  error,
  hasMore,
  onRetry,
  onLoadMore,
}: {
  decisions: AgentDecision[];
  loading: boolean;
  error: string | null;
  hasMore: boolean;
  onRetry: () => void;
  onLoadMore: () => void;
}) {
  return (
    <section className="flex flex-col gap-3" aria-labelledby="decision-log-heading">
      <div>
        <h2 id="decision-log-heading" className="text-base font-semibold">Decision log</h2>
        <p className="text-xs text-[var(--muted)]">Newest first · times in UTC</p>
      </div>

      {loading && decisions.length === 0 && (
        <div className="flex flex-col gap-2" aria-label="Loading decisions" aria-busy="true">
          {[0, 1, 2].map((row) => (
            <div key={row} className="h-12 animate-pulse rounded border border-[var(--border)] bg-[var(--surface)]" />
          ))}
        </div>
      )}

      {error !== null && (
        <div role="alert" className="flex flex-col items-start gap-2 rounded border border-[var(--verdict-fails)] p-3 text-sm">
          <p>Could not load decisions: {error}</p>
          <button
            type="button"
            onClick={onRetry}
            className="min-h-10 rounded border border-[var(--border)] px-3 py-2 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent)]"
          >
            Try again
          </button>
        </div>
      )}

      {!loading && error === null && decisions.length === 0 && (
        <p className="rounded border border-[var(--border)] p-4 text-sm text-[var(--muted)]">
          No decisions yet. Signals, fills, exits, and risk halts will appear here.
        </p>
      )}

      {decisions.length > 0 && (
        <div className="overflow-x-auto rounded border border-[var(--border)]">
          <table className="w-full min-w-[38rem] border-collapse text-sm">
            <thead>
              <tr>
                {["Bar time", "Decision", "Price / quantity", "Equity"].map((heading) => (
                  <th key={heading} className="border-b border-[var(--border)] bg-[var(--surface)] p-2 text-left font-medium">
                    {heading}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {decisions.map((event) => {
                const presentation = decisionPresentation(event);
                const quantity =
                  event.qty === null
                    ? null
                    : event.requested_qty !== null
                      ? `${fmtNum(event.qty)} (requested ${fmtNum(event.requested_qty)})`
                      : fmtNum(event.qty);
                return (
                  <tr key={`${event.bar_ts}:${event.seq}:${event.kind}`}>
                    <td className="border-t border-[var(--border)] p-2 whitespace-nowrap">
                      <time dateTime={new Date(event.bar_ts).toISOString()}>{fmtDateTime(event.bar_ts)}</time>
                    </td>
                    <td className="border-t border-[var(--border)] p-2">
                      <span title={presentation.help} className="font-medium">{presentation.label}</span>
                      {event.kind === "exit_filled" && event.reason === "risk_halt" && (
                        <p className="mt-1 max-w-sm text-xs text-[var(--muted)]">{presentation.help}</p>
                      )}
                    </td>
                    <td className="border-t border-[var(--border)] p-2">
                      {event.price === null ? "—" : fmtNum(event.price)}
                      {quantity === null ? "" : ` · Qty ${quantity}`}
                    </td>
                    <td className="border-t border-[var(--border)] p-2">{fmtNum(event.equity)}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      {hasMore && error === null && (
        <button
          type="button"
          disabled={loading}
          onClick={onLoadMore}
          className="min-h-10 w-fit rounded border border-[var(--border)] px-3 py-2 text-sm disabled:cursor-not-allowed disabled:opacity-60 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent)]"
        >
          {loading ? "Loading…" : "Load more"}
        </button>
      )}
    </section>
  );
}
