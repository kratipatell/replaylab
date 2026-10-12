// In-sample vs out-of-sample vs full-period stats. Column headers carry
// each column's trade count, since every figure here depends on it.
"use client";

import type { Stats } from "@/lib/api";
import { fmtPct, fmtNum, plural } from "@/lib/testlab";

function header(label: string, trades: number | null): string {
  return trades === null ? label : `${label} (${plural(trades, "trade")})`;
}

export default function StatsTable({
  full,
  inSample,
  outOfSample,
}: {
  full: Stats;
  inSample: Stats | null;
  outOfSample: Stats | null;
}) {
  const cols: { label: string; stats: Stats | null }[] = [
    { label: header("In-sample", inSample?.trades ?? null), stats: inSample },
    { label: header("Out-of-sample", outOfSample?.trades ?? null), stats: outOfSample },
    { label: header("Full period", full.trades), stats: full },
  ];
  const cell = (stats: Stats | null, text: (s: Stats) => string): string =>
    stats === null ? "—" : text(stats);

  const rows: { name: string; sub: string; text: (s: Stats) => string }[] = [
    { name: "Trades", sub: "completed buys and sells", text: (s) => String(s.trades) },
    { name: "Win rate", sub: "share of trades that made money", text: (s) => fmtPct(s.win_rate_pct) },
    {
      name: "Profit factor",
      sub: "money won per money lost",
      text: (s) => (s.profit_factor === null ? "no losing trades" : fmtNum(s.profit_factor)),
    },
    {
      name: "Average result per trade",
      sub: "gain or loss of a typical trade",
      text: (s) => fmtPct(s.expectancy_pct),
    },
    { name: "Total return", sub: "growth of the test account", text: (s) => fmtPct(s.total_return_pct) },
    {
      name: "Buy-and-hold return",
      sub: "simply holding the coin instead",
      text: (s) => fmtPct(s.buy_and_hold_pct),
    },
    {
      name: "Biggest fall from a peak",
      sub: "worst losing stretch",
      text: (s) => fmtPct(s.max_drawdown_pct),
    },
  ];

  return (
    <section aria-label="Results by period" className="overflow-x-auto">
      <table className="w-full border-collapse text-sm">
        <thead>
          <tr>
            <th className="border border-[var(--border)] bg-[var(--surface)] p-2 text-left font-medium" />
            {cols.map((c) => (
              <th
                key={c.label}
                className="border border-[var(--border)] bg-[var(--surface)] p-2 text-left font-medium"
              >
                {c.label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr key={row.name}>
              <td className="border border-[var(--border)] p-2">
                <span className="font-medium">{row.name}</span>
                <br />
                <span className="text-xs text-[var(--muted)]">{row.sub}</span>
              </td>
              {cols.map((c) => (
                <td key={c.label} className="border border-[var(--border)] p-2">
                  {cell(c.stats, row.text)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}
