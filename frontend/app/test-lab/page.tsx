// Test Lab: describe an idea, run it on past prices, get an honest verdict.
"use client";

import { useCallback, useEffect, useState } from "react";
import {
  getRun,
  listRuns,
  postBacktest,
  type BacktestResponse,
  type RunSummary,
  type SavedRun,
} from "@/lib/api";
import {
  buildRequest,
  defaultForm,
  fixHint,
  holdBackToSplit,
  plural,
  resolveRange,
  validateForm,
  type TestLabForm,
} from "@/lib/testlab";
import BacktestForm from "@/components/BacktestForm";
import EquityCharts, { MonthlyTable } from "@/components/EquityCharts";
import SavedRuns from "@/components/SavedRuns";
import StatsTable from "@/components/StatsTable";
import TradesTable from "@/components/TradesTable";
import VerdictCard from "@/components/VerdictCard";

export default function TestLab() {
  const [form, setForm] = useState<TestLabForm>(defaultForm);
  const [attempted, setAttempted] = useState(false);
  const [clientErrors, setClientErrors] = useState<string[]>([]);
  const [engineError, setEngineError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [result, setResult] = useState<BacktestResponse | null>(null);
  const [submittedSplit, setSubmittedSplit] = useState<number | null>(null);
  const [submittedStart, setSubmittedStart] = useState<number | null>(null);
  const [submittedEnd, setSubmittedEnd] = useState<number | null>(null);
  const [runs, setRuns] = useState<RunSummary[]>([]);
  const [runsLoading, setRunsLoading] = useState(true);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [saved, setSaved] = useState<SavedRun | null>(null);
  const [savedLoading, setSavedLoading] = useState(false);
  const [savedError, setSavedError] = useState<string | null>(null);

  const refreshRuns = useCallback(async (): Promise<void> => {
    setRunsLoading(true);
    try {
      setRuns(await listRuns());
    } catch {
      setRuns([]);
    } finally {
      setRunsLoading(false);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    listRuns()
      .then((r) => {
        if (!cancelled) {
          setRuns(r);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setRuns([]);
        }
      })
      .finally(() => {
        if (!cancelled) {
          setRunsLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const submit = useCallback(async (): Promise<void> => {
    const now = Date.now();
    setAttempted(true);
    const problems = validateForm(form, now);
    setClientErrors(problems);
    setEngineError(null);
    if (problems.length > 0) {
      return;
    }
    setLoading(true);
    try {
      const range = resolveRange(form, now);
      const res = await postBacktest(buildRequest(form, now));
      setResult(res);
      setSubmittedSplit(holdBackToSplit(Number(form.split)));
      setSubmittedStart(range.startMs);
      setSubmittedEnd(range.endMs);
      setSaved(null);
      setSelectedId(null);
      setSavedError(null);
      await refreshRuns();
    } catch (e) {
      setResult(null);
      setEngineError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [form, refreshRuns]);

  const handleFormChange = (next: TestLabForm): void => {
    setForm(next);
    if (attempted) {
      setClientErrors(validateForm(next, Date.now()));
    }
  };

  const selectRun = useCallback(async (id: number): Promise<void> => {
    setSelectedId(id);
    setSavedLoading(true);
    setSavedError(null);
    try {
      setSaved(await getRun(id));
      setResult(null);
      setEngineError(null);
      setClientErrors([]);
    } catch (e) {
      setSaved(null);
      setSavedError(e instanceof Error ? e.message : String(e));
    } finally {
      setSavedLoading(false);
    }
  }, []);

  return (
    <div className="flex max-w-4xl flex-col gap-6">
      <div className="flex flex-col gap-1">
        <h1 className="text-xl font-bold">Test Lab</h1>
        <p className="text-sm text-[var(--muted)]">
          Describe a trading idea, run it on past prices, and see whether it
          held up on data it was not tuned for.
        </p>
      </div>

      <BacktestForm
        form={form}
        onChange={handleFormChange}
        onSubmit={() => void submit()}
        loading={loading}
      />

      {attempted && clientErrors.length > 0 && (
        <div role="alert" className="rounded border border-[var(--verdict-fails)] p-3 text-sm">
          <p className="font-semibold">Fix these before running:</p>
          <ul className="list-disc pl-5">
            {clientErrors.map((e) => (
              <li key={e}>{e}</li>
            ))}
          </ul>
        </div>
      )}

      {loading && <p className="text-sm text-[var(--muted)]">Running test…</p>}

      {engineError !== null && !loading && (
        <div role="alert" className="flex flex-col gap-1 rounded border border-[var(--verdict-fails)] p-3 text-sm">
          <p className="font-semibold">The test engine rejected this idea:</p>
          <p>{engineError}</p>
          <p className="text-[var(--muted)]">{fixHint(engineError)}</p>
        </div>
      )}

      {result !== null && !loading && (
        <div className="flex flex-col gap-4">
          <p className="text-xs text-[var(--muted)]">
            Run {result.run_id === null ? "" : `#${result.run_id} · `}
            {plural(result.candle_count, "candle")}
          </p>
          <VerdictCard
            verdict={result.verdict}
            onUseLongRange={() => handleFormChange({ ...form, preset: "5y" })}
          />
          <StatsTable
            full={result.stats}
            inSample={result.in_sample}
            outOfSample={result.out_of_sample}
          />
          <EquityCharts
            equity={result.equity_curve}
            split={submittedSplit}
            startMs={submittedStart}
            endMs={submittedEnd}
          />
          <MonthlyTable monthly={result.metrics.monthly} />
          <TradesTable trades={result.trades} />
          <p className="text-xs text-[var(--muted)]">
            These are simulated results on past data. Past results do not
            predict future results.
          </p>
        </div>
      )}

      {!loading && result === null && engineError === null && saved === null && !savedLoading && savedError === null && (
        <p className="text-sm text-[var(--muted)]">
          No test yet. Fill in the idea above and press Run test, or open a saved test below.
        </p>
      )}

      {savedLoading && <p className="text-sm text-[var(--muted)]">Loading saved test…</p>}

      {savedError !== null && !savedLoading && (
        <div role="alert" className="rounded border border-[var(--verdict-fails)] p-3 text-sm">
          <p className="font-semibold">Could not load that saved test:</p>
          <p>{savedError}</p>
        </div>
      )}

      {saved !== null && !savedLoading && (
        <div className="flex flex-col gap-4">
          <p className="text-xs text-[var(--muted)]">
            Saved test #{saved.id} · {saved.symbol} {saved.interval}
          </p>
          <p className="rounded border border-[var(--border)] bg-[var(--surface)] p-3 text-sm text-[var(--muted)]">
            Verdict is not stored for older runs.
          </p>
          <StatsTable full={saved.stats} inSample={null} outOfSample={null} />
          <EquityCharts
            equity={saved.equity_curve}
            split={null}
            startMs={saved.start_ms}
            endMs={saved.end_ms}
          />
          <MonthlyTable monthly={saved.metrics.monthly} />
          <TradesTable trades={saved.trades} />
          <p className="text-xs text-[var(--muted)]">
            These are simulated results on past data. Past results do not
            predict future results.
          </p>
        </div>
      )}

      <SavedRuns
        runs={runs}
        loading={runsLoading}
        selectedId={selectedId}
        onSelect={(id) => void selectRun(id)}
      />
    </div>
  );
}
