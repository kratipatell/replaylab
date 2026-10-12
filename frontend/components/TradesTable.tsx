// First 50 trades, then more on demand.
"use client";

import { useState } from "react";
import type { ExitReason, Trade } from "@/lib/api";
import {
  exitReasonHelp,
  exitReasonLabel,
  fmtDateTime,
  fmtNum,
  fmtPct,
} from "@/lib/testlab";

const EXIT_REASONS: ExitReason[] = [
  "Signal",
  "StopLoss",
  "TakeProfit",
  "Trailing",
  "SessionEnd",
  "EndOfData",
];

const PAGE = 50;

export default function TradesTable({ trades }: { trades: Trade[] }) {
  const [shown, setShown] = useState(PAGE);
  if (trades.length === 0) {
    return <p className="text-sm text-[var(--muted)]">No trades happened in this test.</p>;
  }
  const visible = trades.slice(0, shown);
  const remaining = trades.length - visible.length;
  return (
    <section aria-label="Trades" className="flex flex-col gap-2 overflow-x-auto">
      <p className="text-xs text-[var(--muted)]">All times in UTC.</p>
      <table className="w-full border-collapse text-sm">
        <thead>
          <tr>
            {["Opened", "Closed", "Side", "Open price", "Close price", "Return", "Why it closed"].map(
              (h) => (
                <th
                  key={h}
                  className="border border-[var(--border)] bg-[var(--surface)] p-2 text-left font-medium"
                >
                  {h}
                </th>
              ),
            )}
          </tr>
        </thead>
        <tbody>
          {visible.map((t) => (
            <tr key={`${t.entry_idx}-${t.exit_idx}`}>
              <td className="border border-[var(--border)] p-2">{fmtDateTime(t.entry_ts)}</td>
              <td className="border border-[var(--border)] p-2">{fmtDateTime(t.exit_ts)}</td>
              <td className="border border-[var(--border)] p-2">{t.side}</td>
              <td className="border border-[var(--border)] p-2">{fmtNum(t.entry_price)}</td>
              <td className="border border-[var(--border)] p-2">{fmtNum(t.exit_price)}</td>
              <td className="border border-[var(--border)] p-2">{fmtPct(t.return_pct)}</td>
              <td
                className="border border-[var(--border)] p-2"
                title={exitReasonHelp(t.reason)}
              >
                {exitReasonLabel(t.reason)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="text-xs text-[var(--muted)]">
        {EXIT_REASONS.map((r) => `${exitReasonLabel(r)}: ${exitReasonHelp(r)}`).join(" ")}
      </p>
      {remaining > 0 && (
        <button
          type="button"
          onClick={() => setShown((n) => n + PAGE)}
          className="w-fit rounded border border-[var(--border)] px-3 py-1.5 text-sm"
        >
          Show more ({remaining} remaining)
        </button>
      )}
    </section>
  );
}
