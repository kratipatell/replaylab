// Check: paste a video's words, confirm the rule it claims, test it.
"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  postBacktest,
  type BacktestResponse,
  type Strategy,
} from "@/lib/api";
import {
  BACKTEST_TIMEOUT_MS,
  CHECK_CONCURRENCY,
  CHECK_RUN_ORDER,
  MULTI_TIMEFRAME_NOTE,
  SUPPORTED_ASSETS_NOTE,
  anyRunning,
  applyContextEdits,
  applyOutcomeToRows,
  applyParamEdits,
  buildCheckBacktestRequest,
  buildSummary,
  buildTimeframeRequest,
  cancelRows,
  checkRangeFit,
  completedRows,
  errorMessage,
  missingParameters,
  normaliseAsset,
  providerErrorMessage,
  runningRow,
  runLimited,
  startRowStates,
  timeframeCells,
  timeframeNotStated,
  weakEvidence,
  type CheckClaim,
  type CheckContext,
  type CheckParameter,
  type ContextEdits,
  type MultiTimeframe,
  type MultiTimeframeRun,
  type TimeframeOutcome,
  type TimeframeRowState,
} from "@/lib/check";
import {
  fixHint,
  fmtDate,
  fmtNum,
  fmtPct,
  gradeColorVar,
  plural,
} from "@/lib/testlab";
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
      context: CheckContext;
    }
  | {
      status: "needs_input";
      strategy: Strategy;
      parameters: CheckParameter[];
      claim: CheckClaim;
      context: CheckContext;
      missing: { name: string; path: string }[];
      reason: string;
    }
  | { status: "no_rule_found"; reason: string };

interface ErrorShape {
  status: "error";
  message: string;
}

interface MultiState {
  strategy: Strategy;
  nowMs: number;
  rows: TimeframeRowState[];
  runs: Partial<Record<MultiTimeframe, MultiTimeframeRun>>;
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
    throw new Error(providerErrorMessage(res.status, message));
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
  const [contextEdits, setContextEdits] = useState<ContextEdits>({});
  const [everyTimeframe, setEveryTimeframe] = useState(false);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<BacktestResponse | null>(null);
  const [multi, setMulti] = useState<MultiState | null>(null);
  const [retrying, setRetrying] = useState<MultiTimeframe | null>(null);
  const [selected, setSelected] = useState<MultiTimeframe | null>(null);
  const [engineError, setEngineError] = useState<string | null>(null);
  const [range, setRange] = useState<{ start: number; end: number } | null>(
    null,
  );
  const [elapsedNow, setElapsedNow] = useState(0);

  const tokenRef = useRef(0);
  const controllersRef = useRef<Set<AbortController>>(new Set());

  useEffect(() => {
    if (!running && retrying === null) {
      return;
    }
    const id = setInterval(() => setElapsedNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [running, retrying]);

  const read = useCallback(async (): Promise<void> => {
    setReading(true);
    setExtractError(null);
    setEngineError(null);
    setResult(null);
    setMulti(null);
    setSelected(null);
    setRange(null);
    try {
      const out = await postExtract(text);
      setExtract(out);
      setOverrides({});
      setContextEdits({});
      setEveryTimeframe(
        out.status !== "no_rule_found" && timeframeNotStated(out.context),
      );
    } catch (e) {
      setExtract(null);
      setExtractError(e instanceof Error ? e.message : String(e));
    } finally {
      setReading(false);
    }
  }, [text]);

  const editable =
    extract !== null && extract.status !== "no_rule_found" ? extract : null;
  const assetText =
    editable === null
      ? ""
      : (contextEdits.asset ??
        editable.context.asset.value ??
        editable.strategy.asset);
  const asset = editable === null ? null : normaliseAsset(assetText);
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
  const busy = running || retrying !== null;
  const canRun =
    editable !== null &&
    asset !== null &&
    !reading &&
    !busy &&
    unfilled.length === 0;

  const setContextEdit = useCallback(
    (key: keyof ContextEdits, value: string) => {
      setContextEdits((prev) => ({ ...prev, [key]: value }) as ContextEdits);
    },
    [],
  );

  const timeframeKnown =
    editable !== null &&
    (contextEdits.timeframe !== undefined ||
      editable.context.timeframe.value !== null);

  let summary: string | null = null;
  if (editable !== null) {
    const options = {
      timeframe: timeframeKnown
        ? (contextEdits.timeframe ?? editable.context.timeframe.value)
        : null,
      everyTimeframe,
    };
    try {
      summary = buildSummary(
        applyContextEdits(
          applyParamEdits(editable.strategy, edits),
          editable.context,
          contextEdits,
        ),
        options,
      );
    } catch {
      summary = buildSummary(editable.strategy, options);
    }
  }

  const openTimeframe = useCallback((run: MultiTimeframeRun): void => {
    setSelected(run.timeframe);
    setResult(run.response);
    setRange({ start: run.range.startMs, end: run.range.endMs });
  }, []);

  const openOutcome = useCallback(
    (timeframe: MultiTimeframe): void => {
      const run = multi?.runs[timeframe];
      if (run !== undefined) {
        openTimeframe(run);
      }
    },
    [multi, openTimeframe],
  );

  const timedController = useCallback((): {
    controller: AbortController;
    clear: () => void;
  } => {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), BACKTEST_TIMEOUT_MS);
    controllersRef.current.add(controller);
    return {
      controller,
      clear: () => {
        clearTimeout(timer);
        controllersRef.current.delete(controller);
      },
    };
  }, []);

  const cancel = useCallback((): void => {
    tokenRef.current += 1;
    for (const controller of controllersRef.current) {
      controller.abort();
    }
    controllersRef.current.clear();
    setMulti((prev) =>
      prev === null ? prev : { ...prev, rows: cancelRows(prev.rows) },
    );
    setRetrying(null);
    setRunning(false);
  }, []);

  const runTest = useCallback(async (): Promise<void> => {
    if (
      editable === null ||
      asset === null ||
      bad.length > 0 ||
      unfilled.length > 0
    ) {
      return;
    }
    const token = tokenRef.current + 1;
    tokenRef.current = token;
    controllersRef.current = new Set();
    const strategy = applyContextEdits(
      applyParamEdits(editable.strategy, edits),
      editable.context,
      contextEdits,
    );
    const now = Date.now();
    setEngineError(null);
    setResult(null);
    setMulti(null);
    setSelected(null);
    setRange(null);
    setRunning(true);

    if (!everyTimeframe) {
      const { controller, clear } = timedController();
      try {
        const request = buildCheckBacktestRequest(strategy, now);
        const response = await postBacktest(request, controller.signal);
        if (tokenRef.current !== token) {
          return;
        }
        setResult(response);
        setRange({ start: request.start_ms, end: request.end_ms });
      } catch (e) {
        if (tokenRef.current === token) {
          setEngineError(errorMessage(e));
        }
      } finally {
        clear();
        if (tokenRef.current === token) {
          setRunning(false);
        }
      }
      return;
    }

    const rows = startRowStates(now);
    setMulti({ strategy, nowMs: now, rows, runs: {} });
    const limiter = runLimited(
      CHECK_RUN_ORDER,
      CHECK_CONCURRENCY,
      async (timeframe) => {
        const { controller, clear } = timedController();
        try {
          const request = buildTimeframeRequest(strategy, timeframe, now);
          const response = await postBacktest(request, controller.signal);
          if (tokenRef.current !== token) {
            return;
          }
          const run: MultiTimeframeRun = {
            timeframe,
            range: checkRangeFit(timeframe, now),
            response,
          };
          setMulti((prev) =>
            prev === null
              ? prev
              : {
                  ...prev,
                  rows: applyOutcomeToRows(prev.rows, { status: "ok", ...run }),
                  runs: { ...prev.runs, [timeframe]: run },
                },
          );
        } catch (e) {
          if (tokenRef.current !== token) {
            return;
          }
          const outcome: TimeframeOutcome = {
            status: "failed",
            timeframe,
            error: errorMessage(e),
          };
          setMulti((prev) =>
            prev === null
              ? prev
              : { ...prev, rows: applyOutcomeToRows(prev.rows, outcome) },
          );
        } finally {
          clear();
        }
      },
    );
    await limiter.done;
    if (tokenRef.current === token) {
      setRunning(false);
    }
  }, [
    editable,
    asset,
    edits,
    contextEdits,
    everyTimeframe,
    bad,
    unfilled,
    timedController,
  ]);

  const retryTimeframe = useCallback(
    async (timeframe: MultiTimeframe): Promise<void> => {
      if (multi === null) {
        return;
      }
      const token = tokenRef.current;
      setRetrying(timeframe);
      setMulti((prev) =>
        prev === null
          ? prev
          : { ...prev, rows: runningRow(prev.rows, timeframe, Date.now()) },
      );
      const { controller, clear } = timedController();
      try {
        const request = buildTimeframeRequest(
          multi.strategy,
          timeframe,
          multi.nowMs,
        );
        const response = await postBacktest(request, controller.signal);
        if (tokenRef.current !== token) {
          return;
        }
        const run: MultiTimeframeRun = {
          timeframe,
          range: checkRangeFit(timeframe, multi.nowMs),
          response,
        };
        setMulti((prev) =>
          prev === null
            ? prev
            : {
                ...prev,
                rows: applyOutcomeToRows(prev.rows, { status: "ok", ...run }),
                runs: { ...prev.runs, [timeframe]: run },
              },
        );
      } catch (e) {
        if (tokenRef.current !== token) {
          return;
        }
        const outcome: TimeframeOutcome = {
          status: "failed",
          timeframe,
          error: errorMessage(e),
        };
        setMulti((prev) =>
          prev === null
            ? prev
            : { ...prev, rows: applyOutcomeToRows(prev.rows, outcome) },
        );
      } finally {
        clear();
        if (tokenRef.current === token) {
          setRetrying(null);
        }
      }
    },
    [multi, timedController],
  );

  const rows = useMemo(() => multi?.rows ?? [], [multi]);
  const completed = useMemo(() => completedRows(rows), [rows]);
  const weak = useMemo(() => weakEvidence(completed), [completed]);
  const stillRunning = anyRunning(rows);

  const claimed =
    editable !== null && editable.claim.win_rate_pct !== null;

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
          {asset === null && (
            <p className="text-sm text-[var(--verdict-fails)]">
              This video&apos;s coin is not one we test. {SUPPORTED_ASSETS_NOTE}
            </p>
          )}
          <CheckParamsTable
            params={editable.parameters}
            overrides={overrides}
            onEdit={(path, raw) =>
              setOverrides((prev) => ({ ...prev, [path]: raw }))
            }
            context={editable.context}
            contextEdits={contextEdits}
            onContextEdit={setContextEdit}
            everyTimeframe={everyTimeframe}
            onEveryTimeframeChange={setEveryTimeframe}
            assetText={assetText}
          />
          {claimed && (
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
          <div className="flex flex-wrap items-center gap-3">
            <button
              type="button"
              onClick={() => void runTest()}
              disabled={!canRun || bad.length > 0}
              className="w-fit rounded bg-[var(--accent)] px-4 py-2 text-sm font-medium text-[var(--accent-foreground)] disabled:cursor-not-allowed disabled:opacity-50"
            >
              {running ? "Running test…" : "Run test"}
            </button>
            {running && (
              <button
                type="button"
                onClick={cancel}
                className="w-fit rounded border border-[var(--border)] bg-[var(--surface)] px-4 py-2 text-sm font-medium"
              >
                Cancel
              </button>
            )}
          </div>
        </div>
      )}

      {engineError !== null && !running && (
        <div
          role="alert"
          className="flex flex-col gap-1 rounded border border-[var(--verdict-fails)] p-3 text-sm"
        >
          <p className="font-semibold">The test engine rejected this idea:</p>
          <p>{engineError}</p>
          <p className="text-[var(--muted)]">{fixHint(engineError)}</p>
        </div>
      )}

      {multi !== null && editable !== null && (
        <div className="flex flex-col gap-3">
          <p className="text-sm">
            Claimed win rate:{" "}
            {editable.claim.win_rate_pct === null ? (
              "none stated"
            ) : (
              <strong>{fmtPct(editable.claim.win_rate_pct)}</strong>
            )}
            {editable.claim.quote !== null && editable.claim.quote !== "" && (
              <span className="text-[var(--muted)]">
                {" "}
                (“{editable.claim.quote}”)
              </span>
            )}
          </p>
          <div className="overflow-x-auto">
            <table className="w-full border-collapse text-sm">
              <thead>
                <tr>
                  {[
                    "Timeframe",
                    "Range used",
                    "Trades (held back)",
                    "Win rate (held back)",
                    "Profit factor",
                    "Avg result per trade",
                    "Verdict",
                  ].map((h) => (
                    <th
                      key={h}
                      className="border border-[var(--border)] bg-[var(--surface)] p-2 text-left font-medium"
                    >
                      {h}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {rows.map((state) => {
                  const rangeText = `${fmtDate(state.range.startMs)} to ${fmtDate(
                    state.range.endMs,
                  )}`;
                  if (state.status === "done" && state.row !== null) {
                    const row = state.row;
                    const cells = timeframeCells(row);
                    return (
                      <tr
                        key={state.timeframe}
                        onClick={() => openOutcome(state.timeframe)}
                        className="cursor-pointer"
                      >
                        <td className="border border-[var(--border)] p-2">
                          <button
                            type="button"
                            className="font-medium underline"
                            onClick={() => openOutcome(state.timeframe)}
                          >
                            {state.timeframe}
                          </button>
                        </td>
                        <td className="border border-[var(--border)] p-2">
                          {rangeText}
                        </td>
                        <td className="border border-[var(--border)] p-2">
                          {row.trades === null ? "—" : row.trades}
                        </td>
                        <td className="border border-[var(--border)] p-2">
                          {cells.winRatePct === null
                            ? "—"
                            : fmtPct(cells.winRatePct)}
                        </td>
                        <td className="border border-[var(--border)] p-2">
                          {cells.profitFactor === null
                            ? "—"
                            : fmtNum(cells.profitFactor)}
                        </td>
                        <td className="border border-[var(--border)] p-2">
                          {cells.expectancyPct === null
                            ? "—"
                            : fmtPct(cells.expectancyPct)}
                        </td>
                        <td
                          className="border border-[var(--border)] p-2 font-medium"
                          style={{ color: gradeColorVar(row.grade) }}
                        >
                          {cells.verdictLabel}
                        </td>
                      </tr>
                    );
                  }
                  if (state.status === "running") {
                    const seconds = Math.max(
                      0,
                      Math.floor((elapsedNow - state.startedAt) / 1000),
                    );
                    return (
                      <tr key={state.timeframe}>
                        <td className="border border-[var(--border)] p-2 font-medium">
                          {state.timeframe}
                        </td>
                        <td className="border border-[var(--border)] p-2">
                          {rangeText}
                        </td>
                        <td
                          colSpan={5}
                          className="border border-[var(--border)] p-2 text-[var(--muted)]"
                        >
                          Running… {seconds}s
                        </td>
                      </tr>
                    );
                  }
                  return (
                    <tr key={state.timeframe} className="align-top">
                      <td className="border border-[var(--border)] p-2 font-medium">
                        {state.timeframe}
                      </td>
                      <td className="border border-[var(--border)] p-2">
                        {rangeText}
                      </td>
                      <td colSpan={5} className="border border-[var(--border)] p-2">
                        <div className="flex items-center justify-between gap-3">
                          <span className="text-[var(--verdict-fails)]">
                            {state.error}
                          </span>
                          <button
                            type="button"
                            onClick={() => void retryTimeframe(state.timeframe)}
                            disabled={retrying !== null}
                            className="shrink-0 rounded border border-[var(--border)] bg-[var(--surface)] px-2 py-1 text-xs font-medium disabled:cursor-not-allowed disabled:opacity-50"
                          >
                            {retrying === state.timeframe
                              ? "Trying…"
                              : "Try again"}
                          </button>
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
          {stillRunning && (
            <p className="text-xs text-[var(--muted)]">
              First run for a timeframe downloads price history and can take a
              minute. Later runs are fast.
            </p>
          )}
          <p className="text-xs text-[var(--muted)]">
            {MULTI_TIMEFRAME_NOTE}
          </p>
          {weak !== null && (
            <p className="text-sm text-[var(--verdict-fragile)]">{weak}</p>
          )}
        </div>
      )}

      {result !== null && editable !== null && !running && (
        <div className="flex flex-col gap-4">
          {selected !== null && (
            <p className="text-sm font-medium">
              Timeframe {selected} full result
            </p>
          )}
          <p className="text-xs text-[var(--muted)]">
            Run {result.run_id === null ? "" : `#${result.run_id} · `}
            {plural(result.candle_count, "candle")} ·{" "}
            {range === null
              ? "range"
              : `${fmtDate(range.start)} to ${fmtDate(range.end)}`}{" "}
            · last 30% held back
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
