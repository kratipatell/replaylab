// Same rule, same costs, other markets. The worst market decides the grade.
"use client";

import type { CrossMarket, MarketCheck } from "@/lib/api";
import { fmtPct, gradeColorVar, gradeLabel, plural } from "@/lib/testlab";

function MarketRow({ market }: { market: MarketCheck }) {
  const color = gradeColorVar(market.verdict.grade);
  const oos = market.out_of_sample;
  return (
    <li className="flex flex-col gap-1 py-2">
      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <span className="font-mono text-sm font-semibold">{market.symbol}</span>
        <span className="text-sm font-semibold" style={{ color }}>
          {gradeLabel(market.verdict.grade)}
        </span>
        <span className="text-xs text-[var(--muted)]">
          {plural(oos?.trades ?? market.verdict.oos_trades, "out-of-sample trade")} ·{" "}
          {fmtPct(oos?.expectancy_pct)} per trade ·{" "}
          {fmtPct(oos?.max_drawdown_pct)} worst fall
        </span>
      </div>
      <p className="text-xs text-[var(--muted)]">{market.verdict.headline}</p>
    </li>
  );
}

export default function CrossMarketCard({ result }: { result: CrossMarket }) {
  const color = gradeColorVar(result.grade);
  return (
    <section
      aria-label="Cross-market check"
      className="flex flex-col gap-2 rounded border border-[var(--border)] bg-[var(--surface)] p-4"
    >
      <div className="flex flex-wrap items-baseline gap-x-3">
        <p className="text-xl font-bold" style={{ color }}>
          {gradeLabel(result.grade)}
        </p>
        <p className="text-xs text-[var(--muted)]">
          {plural(result.tested, "market")} tested
        </p>
      </div>
      <p className="text-sm font-medium">{result.headline}</p>
      <ul className="flex flex-col divide-y divide-[var(--border)]">
        {result.markets.map((market) => (
          <MarketRow key={market.symbol} market={market} />
        ))}
      </ul>
      {result.unchecked.length > 0 && (
        <div className="flex flex-col gap-1 text-sm">
          <p className="font-medium">Not checked</p>
          <ul className="flex list-disc flex-col gap-1 pl-5 text-[var(--muted)]">
            {result.unchecked.map((market) => (
              <li key={market.symbol}>
                {market.symbol}: {market.reason}
              </li>
            ))}
          </ul>
        </div>
      )}
      <p className="text-xs text-[var(--muted)]">
        Same rules and costs on each market&apos;s own history. Simulated past
        results are not a promise of future profit.
      </p>
    </section>
  );
}
