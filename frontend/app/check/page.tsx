// Check: paste a video's words, confirm the rule it claims, test it.
"use client";

import { useCallback, useMemo, useState } from "react";
import {
  postBacktest,
  type BacktestResponse,
  type Strategy,
} from "@/lib/api";
import {
  applyParamEdits,
  buildCheckBacktestRequest,
  buildSummary,
  missingParameters,
  type CheckClaim,
  type CheckParameter,
} from "@/lib/check";
import { fixHint, plural } from "@/lib/testlab";
import CheckParamsTable from "@/components/CheckParamsTable";
import ClaimCard from "@/components/ClaimCard";
import EquityCharts, { MonthlyTable } from "@/components/EquityCharts";
import StatsTable from "@/components/StatsTable";
import TradesTable from "@/components/TradesTable";
import VerdictCard from "@/components/VerdictCard";

type ExtractResult =
  | {
      status: "found";
      strategy: Strategy;
      parameters: CheckParameter[];
      claim: CheckClaim;
    }
  | {
      status: "needs_input";
      strategy: Strategy;
      parameters: CheckParameter[];
      claim: CheckClaim;
      missing: { name: string; path: string }[];
      reason: string;
    }
  | { status: "no_rule_found"; reason: string };

interface ErrorShape {
  status: "error";
  message: string;
}

async function postExtract(text: string): Promise<ExtractResult> {
  const res = await fetch("/api/check/extract", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ text }),
  });
  const body = (await res.json()) as unknown as ExtractResult | ErrorShape;
  if (!res.ok || body.status === "error") {
    const message =
      body.status === "error" && body.message !== ""
        ? body.message
        : `Reader failed with status ${res.status}`;
    throw new Error(message);
  }
  return body as ExtractResult;
}

function numericEdits(
  overrides: Record<string, string>,
): { edits: Record<string, number>; bad: string[] } {
  const edits: Record<string, number> = {};
  const bad: string[] = [];
  for (const [path, raw] of Object.entries(overrides)) {
    if (raw.trim() === "") {
      bad.push(path);
      continue;
    }
    const v = Number(raw);
    if (!Number.isFinite(v)) {
      bad.push(path);
      continue;
    }
    if (path.endsWith(".period") && (!Number.isInteger(v) || v < 1)) {
      bad.push(path);
      continue;
    }
    edits[path] = v;
  }
  return { edits, bad };
}

export default function Check() {
  const [text, setText] = useState("");
  const [reading, setReading] = useState(false);
  const [extract, setExtract] = useState<ExtractResult | null>(null);
  const [extractError, setExtractError] = useState<string | null>(null);
  const [overrides, setOverrides] = useState<Record<string, string>>({});
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<BacktestResponse | null>(null);
  const [engineError, setEngineError] = useState<string | null>(null);
  const [range, setRange] = useState<{ start: number; end: number } | null>(
    null,
  );

  const read = useCallback(async (): Promise<void> => {
    setReading(true);
    setExtractError(null);
    setEngineError(null);
    setResult(null);
    setRange(null);
    try {
      const out = await postExtract(text);
      setExtract(out);
      setOverrides({});
    } catch (e) {
      setExtract(null);
      setExtractError(e instanceof Error ? e.message : String(e));
    } finally {
      setReading(false);
    }
  }, [text]);

  const editable =
    extract !== null && extract.status !== "no_rule_found" ? extract : null;
  const { edits, bad } = useMemo(() => numericEdits(overrides), [overrides]);
  const unfilled = useMemo(
    () =>
      editable === null
        ? []
        : missingParameters(editable.parameters).filter(
            (p) => edits[p.path] === undefined,
          ),
    [editable, edits],
  );
  const canRun =
    editable !== null && !reading && !testing && unfilled.length === 0;

  let summary: string | null = null;
  if (editable !== null) {
    try {
      summary = buildSummary(applyParamEdits(editable.strategy, edits));
    } catch {
      summary = buildSummary(editable.strategy);
    }
  }

  const runTest = useCallback(async (): Promise<void> => {
    if (editable === null || bad.length > 0 || unfilled.length > 0) {
      return;
    }
    setTesting(true);
    setEngineError(null);
    try {
      const strategy = applyParamEdits(editable.strategy, edits);
      const now = Date.now();
      const res = await postBacktest(buildCheckBacktestRequest(strategy, now));
      setResult(res);
      setRange({
        start: Math.floor(now - 5 * 365 * 86_400_000),
        end: Math.floor(now),
      });
    } catch (e) {
      setResult(null);
      setEngineError(e instanceof Error ? e.message : String(e));
    } finally {
      setTesting(false);
    }
  }, [editable, edits, bad, unfilled]);

  return (
    <div className="flex max-w-4xl flex-col gap-6">
      <div className="flex flex-col gap-1">
        <h1 className="text-xl font-bold">Check</h1>
        <p className="text-sm text-[var(--muted)]">
          Paste a video&apos;s words, confirm the rule it claims, then test it
          on past prices.
        </p>
      </div>

      <div className="flex flex-col gap-2">
        <label
          htmlFor="check-text"
          className="flex flex-col gap-1 text-sm font-medium"
        >
          Video words
          <textarea
            id="check-text"
            className="min-h-32 rounded border border-[var(--border)] bg-[var(--background)] px-2 py-1 text-sm"
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="Paste the transcript or caption of the video."
          />
        </label>
        <p className="text-xs text-[var(--muted)]">
          Paste the transcript or caption of the video.
        </p>
        <p className="text-xs text-[var(--muted)]">
          The text you paste is sent to an AI provider to read it.
        </p>
        <button
          type="button"
          onClick={() => void read()}
          disabled={reading || text.trim() === ""}
          className="w-fit rounded bg-[var(--accent)] px-4 py-2 text-sm font-medium text-[var(--accent-foreground)] disabled:cursor-not-allowed disabled:opacity-50"
        >
          {reading ? "Reading…" : "Read"}
        </button>
      </div>

      {extractError !== null && (
        <div
          role="alert"
          className="rounded border border-[var(--verdict-fails)] p-3 text-sm"
        >
          <p className="font-semibold">The reader failed:</p>
          <p>{extractError}</p>
        </div>
      )}

      {extract !== null && extract.status === "no_rule_found" && (
        <div
          role="alert"
          className="rounded border border-[var(--border)] bg-[var(--surface)] p-4 text-sm"
        >
          <p className="font-semibold">No testable rule found.</p>
          <p>{extract.reason}</p>
          <p className="mt-1 text-[var(--muted)]">
            This idea can&apos;t be tested as described.
          </p>
        </div>
      )}

      {editable !== null && (
        <div className="flex flex-col gap-4">
          {editable.status === "needs_input" && (
            <div
              role="alert"
              className="rounded border border-[var(--verdict-fragile)] p-3 text-sm"
            >
              <p className="font-semibold">Something is missing.</p>
              <p>{editable.reason}</p>
            </div>
          )}
          {summary !== null && (
            <p className="rounded border border-[var(--border)] bg-[var(--surface)] p-3 text-sm">
              {summary}
            </p>
          )}
          <CheckParamsTable
            params={editable.parameters}
            overrides={overrides}
            onEdit={(path, raw) =>
              setOverrides((prev) => ({ ...prev, [path]: raw }))
            }
          />
          {editable.claim.win_rate_pct !== null && (
            <p className="text-sm text-[var(--muted)]">
              The video claims a {editable.claim.win_rate_pct}% win rate
              {editable.claim.quote ? (
                <span> (“{editable.claim.quote}”)</span>
              ) : null}
              . The test below checks it.
            </p>
          )}
          {unfilled.length > 0 && (
            <p className="text-sm text-[var(--verdict-fragile)]">
              Fill in {unfilled.map((p) => p.name).join(", ")} to run the test.
            </p>
          )}
          {bad.length > 0 && (
            <p className="text-sm text-[var(--verdict-fails)]">
              Check the numbers you typed: each must be a number
              {bad.some((p) => p.endsWith(".period"))
                ? ", and bar counts must be whole numbers of 1 or more"
                : ""}
              .
            </p>
          )}
          <button
            type="button"
            onClick={() => void runTest()}
            disabled={!canRun || bad.length > 0}
            className="w-fit rounded bg-[var(--accent)] px-4 py-2 text-sm font-medium text-[var(--accent-foreground)] disabled:cursor-not-allowed disabled:opacity-50"
          >
            {testing ? "Running test…" : "Run test"}
          </button>
        </div>
      )}

      {testing && <p className="text-sm text-[var(--muted)]">Running test…</p>}

      {engineError !== null && !testing && (
        <div
          role="alert"
          className="flex flex-col gap-1 rounded border border-[var(--verdict-fails)] p-3 text-sm"
        >
          <p className="font-semibold">The test engine rejected this idea:</p>
          <p>{engineError}</p>
          <p className="text-[var(--muted)]">{fixHint(engineError)}</p>
        </div>
      )}

      {result !== null && editable !== null && !testing && (
        <div className="flex flex-col gap-4">
          <p className="text-xs text-[var(--muted)]">
            Run {result.run_id === null ? "" : `#${result.run_id} · `}
            {plural(result.candle_count, "candle")} · 5-year range, last 30%
            held back
          </p>
          <ClaimCard
            claim={editable.claim}
            outOfSample={result.out_of_sample}
            full={result.stats}
          />
          <VerdictCard verdict={result.verdict} onUseLongRange={() => {}} />
          <StatsTable
            full={result.stats}
            inSample={result.in_sample}
            outOfSample={result.out_of_sample}
          />
          <EquityCharts
            equity={result.equity_curve}
            split={0.7}
            startMs={range?.start ?? null}
            endMs={range?.end ?? null}
          />
          <MonthlyTable monthly={result.metrics.monthly} />
          <TradesTable trades={result.trades} />
          <p className="text-xs text-[var(--muted)]">
            These are simulated results on past data. Past results do not
            predict future results.
          </p>
        </div>
      )}
    </div>
  );
}
