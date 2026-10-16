// Claim versus reality: the video's win-rate claim next to what the test found.
"use client";

import type { Stats } from "@/lib/api";
import { compareClaim, type CheckClaim } from "@/lib/check";
import { fmtNum, fmtPct, plural } from "@/lib/testlab";

export default function ClaimCard({
  claim,
  outOfSample,
  full,
}: {
  claim: CheckClaim;
  outOfSample: Stats | null;
  full: Stats;
}) {
  const compared = compareClaim(
    claim.win_rate_pct,
    outOfSample?.win_rate_pct ?? null,
    full.win_rate_pct,
  );
  const basis = outOfSample ?? full;
  const basisLabel = outOfSample === null ? "Full period" : "Held-back data";
  return (
    <section
      aria-label="Claim versus reality"
      className="flex flex-col gap-2 rounded border border-[var(--border)] bg-[var(--surface)] p-4"
    >
      <p className="text-sm font-semibold">Claim versus reality</p>
      <p className="text-sm">
        Claimed win rate:{" "}
        {claim.win_rate_pct === null ? (
          "none stated"
        ) : (
          <strong>{fmtPct(claim.win_rate_pct)}</strong>
        )}
        {claim.quote !== null && claim.quote !== "" && (
          <span className="text-[var(--muted)]"> (“{claim.quote}”)</span>
        )}
      </p>
      <p className="text-sm">
        Tested win rate:{" "}
        <strong>
          {outOfSample === null
            ? "not enough held-back trades"
            : fmtPct(outOfSample.win_rate_pct)}
        </strong>{" "}
        <span className="text-[var(--muted)]">
          on {outOfSample === null ? "—" : plural(outOfSample.trades, "trade")}{" "}
          held back; {fmtPct(full.win_rate_pct)} over the full period (
          {plural(full.trades, "trade")}).
        </span>
      </p>
      <p className="text-sm font-medium">{compared.sentence}</p>
      <p className="text-xs text-[var(--muted)]">
        Win rate alone does not show profit. {basisLabel}: profit factor{" "}
        {basis.profit_factor === null
          ? "no losing trades"
          : fmtNum(basis.profit_factor)}
        , average result per trade {fmtPct(basis.expectancy_pct)}.
      </p>
    </section>
  );
}
