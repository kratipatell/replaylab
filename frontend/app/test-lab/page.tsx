// Test Lab: describe an idea, run it on past prices, get an honest verdict.
"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import {
  getRun,
  listRuns,
  postBacktest,
  postValidate,
  listTemplates,
  type BacktestRequest,
  type BacktestResponse,
  type CrossMarket,
  type RunSummary,
  type SavedRun,
  type StrategyTemplate,
} from "@/lib/api";
import {
  applyTemplateStrategy,
  buildRequest,
  defaultForm,
  describeStaleInputs,
  fixHint,
  formatLoadNote,
  holdBackToSplit,
  plural,
  resolveRange,
  strategyToForm,
  validateForm,
  type TestLabForm,
} from "@/lib/testlab";
import { HANDOFF_KEY, parseHandoff } from "@/lib/check";
import BacktestForm from "@/components/BacktestForm";
import CrossMarketCard from "@/components/CrossMarketCard";
import EquityCharts, { MonthlyTable } from "@/components/EquityCharts";
import SavedRuns from "@/components/SavedRuns";
import StatsTable from "@/components/StatsTable";
import TradesTable from "@/components/TradesTable";
import TemplatePicker from "@/components/TemplatePicker";
import VerdictCard from "@/components/VerdictCard";
import WalkForwardPanel from "@/components/WalkForwardPanel";

type TestMode = "single" | "walk-forward";

const MODE_TABS: { key: TestMode; label: string }[] = [
  { key: "single", label: "Single test" },
  { key: "walk-forward", label: "Walk-forward" },
];

export default function TestLab() {
  const [mode, setMode] = useState<TestMode>("single");
  const [form, setForm] = useState<TestLabForm>(defaultForm);
  const [attempted, setAttempted] = useState(false);
  const [clientErrors, setClientErrors] = useState<string[]>([]);
  const [engineError, setEngineError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [result, setResult] = useState<BacktestResponse | null>(null);
  const [submittedRequest, setSubmittedRequest] = useState<BacktestRequest | null>(null);
  const [submittedSplit, setSubmittedSplit] = useState<number | null>(null);
  const [submittedStart, setSubmittedStart] = useState<number | null>(null);
  const [submittedEnd, setSubmittedEnd] = useState<number | null>(null);
  const [crossMarket, setCrossMarket] = useState<CrossMarket | null>(null);
  const [crossMarketLoading, setCrossMarketLoading] = useState(false);
  const [crossMarketError, setCrossMarketError] = useState<string | null>(null);
  const crossMarketRun = useRef(0);
  const [runs, setRuns] = useState<RunSummary[]>([]);
  const [runsLoading, setRunsLoading] = useState(true);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [saved, setSaved] = useState<SavedRun | null>(null);
  const [savedLoading, setSavedLoading] = useState(false);
  const [savedError, setSavedError] = useState<string | null>(null);
  const [loadNote, setLoadNote] = useState<string | null>(null);
  const [templates, setTemplates] = useState<StrategyTemplate[]>([]);
  const [templatesLoading, setTemplatesLoading] = useState(true);
  const [templatesError, setTemplatesError] = useState<string | null>(null);
  const [selectedTemplateId, setSelectedTemplateId] = useState<string | null>(null);
  const [templateSelection, setTemplateSelection] = useState<{
    template: StrategyTemplate;
  } | null>(null);

  const refreshTemplates = useCallback(async (): Promise<void> => {
    setTemplatesLoading(true);
    setTemplatesError(null);
    try {
      setTemplates(await listTemplates());
    } catch (e) {
      setTemplates([]);
      setTemplatesError(e instanceof Error ? e.message : String(e));
    } finally {
      setTemplatesLoading(false);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    listTemplates()
      .then((loaded) => {
        if (!cancelled) {
          setTemplates(loaded);
        }
      })
      .catch((e: unknown) => {
        if (!cancelled) {
          setTemplates([]);
          setTemplatesError(e instanceof Error ? e.message : String(e));
        }
      })
      .finally(() => {
        if (!cancelled) {
          setTemplatesLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

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

  useEffect(() => {
    let cancelled = false;
    queueMicrotask(() => {
      if (cancelled) {
        return;
      }
      const raw = sessionStorage.getItem(HANDOFF_KEY);
      if (raw === null) {
        return;
      }
      sessionStorage.removeItem(HANDOFF_KEY);
      const handoff = parseHandoff(raw);
      if (handoff === null) {
        return;
      }
      const { form: next, notes } = strategyToForm(handoff.strategy);
      setForm(next);
      setLoadNote(formatLoadNote(notes));
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
    crossMarketRun.current += 1;
    setCrossMarket(null);
    setCrossMarketLoading(false);
    setCrossMarketError(null);
    try {
      const range = resolveRange(form, now);
      const request = buildRequest(form, now);
      const res = await postBacktest(request);
      setResult(res);
      setSubmittedRequest(request);
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

  const handleTemplateSelect = (template: StrategyTemplate): void => {
    setForm((current) => applyTemplateStrategy(current, template));
    setSelectedTemplateId(template.id);
    setTemplateSelection({ template });
    setLoadNote(null);
  };

  const selectRun = useCallback(async (id: number): Promise<void> => {
    setMode("single");
    setSelectedId(id);
    setSavedLoading(true);
    setSavedError(null);
    try {
      setSaved(await getRun(id));
      setResult(null);
      setSubmittedRequest(null);
      crossMarketRun.current += 1;
      setCrossMarket(null);
      setCrossMarketLoading(false);
      setCrossMarketError(null);
      setEngineError(null);
      setClientErrors([]);
    } catch (e) {
      setSaved(null);
      setSavedError(e instanceof Error ? e.message : String(e));
    } finally {
      setSavedLoading(false);
    }
  }, []);

  const checkOtherMarkets = useCallback(async (): Promise<void> => {
    if (submittedRequest === null) {
      return;
    }
    const runId = crossMarketRun.current + 1;
    crossMarketRun.current = runId;
    setCrossMarketLoading(true);
    setCrossMarketError(null);
    try {
      const cross = await postValidate(submittedRequest);
      if (crossMarketRun.current !== runId) {
        return;
      }
      setCrossMarket(cross);
    } catch (e) {
      if (crossMarketRun.current !== runId) {
        return;
      }
      setCrossMarket(null);
      setCrossMarketError(e instanceof Error ? e.message : String(e));
    } finally {
      if (crossMarketRun.current === runId) {
        setCrossMarketLoading(false);
      }
    }
  }, [submittedRequest]);

  const staleInputs =
    result !== null && submittedRequest !== null
      ? describeStaleInputs(form, submittedRequest, submittedRequest.end_ms)
      : [];

  return (
    <div className="flex max-w-4xl flex-col gap-6">
      <div className="flex flex-col gap-1">
        <h1 className="text-xl font-bold">Test Lab</h1>
        <p className="text-sm text-[var(--muted)]">
          Describe a trading idea, run it on past prices, and see whether it
          held up on data it was not tuned for.
        </p>
      </div>

      {loadNote !== null && (
        <p
          role="alert"
          className="rounded border border-[var(--verdict-fragile)] p-3 text-sm"
        >
          {loadNote}
        </p>
      )}

      <TemplatePicker
        templates={templates}
        loading={templatesLoading}
        error={templatesError}
        selectedId={selectedTemplateId}
        onSelect={handleTemplateSelect}
        onRetry={() => void refreshTemplates()}
      />

      <div role="tablist" aria-label="Test type" className="flex gap-2 border-b border-[var(--border)]">
        {MODE_TABS.map((tab) => (
          <button
            key={tab.key}
            type="button"
            role="tab"
            id={`tab-${tab.key}`}
            aria-selected={mode === tab.key}
            onClick={() => setMode(tab.key)}
            className={`-mb-px border-b-2 px-3 py-2 text-sm font-medium ${
              mode === tab.key
                ? "border-[var(--accent)] text-[var(--accent)]"
                : "border-transparent text-[var(--muted)]"
            }`}
          >
            {tab.label}
          </button>
        ))}
      </div>

      {/* Kept mounted while hidden so a running walk-forward survives a tab switch. */}
      <div
        role="tabpanel"
        id="panel-walk-forward"
        aria-labelledby="tab-walk-forward"
        hidden={mode !== "walk-forward"}
      >
        <WalkForwardPanel
          form={form}
          onFormChange={handleFormChange}
          templateSelection={templateSelection}
        />
      </div>

      {mode === "single" && (
        <div
          role="tabpanel"
          id="panel-single"
          aria-labelledby="tab-single"
          className="flex flex-col gap-6"
        >
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
              {staleInputs.length > 0 && (
                <p
                  role="status"
                  className="rounded border border-[var(--verdict-fragile)] bg-[var(--surface)] p-3 text-sm"
                >
                  Results are for earlier inputs: {staleInputs.join(", ")}. Run test to update.
                </p>
              )}
              <p className="text-xs text-[var(--muted)]">
                Run {result.run_id === null ? "" : `#${result.run_id} · `}
                {plural(result.candle_count, "candle")}
              </p>
              <VerdictCard
                verdict={result.verdict}
                onUseLongRange={() => handleFormChange({ ...form, preset: "5y" })}
              />
              <div className="flex flex-col gap-2">
                <button
                  type="button"
                  onClick={() => void checkOtherMarkets()}
                  disabled={crossMarketLoading || submittedRequest === null}
                  className="w-fit rounded border border-[var(--border)] bg-[var(--surface)] px-3 py-1.5 text-sm font-medium disabled:opacity-60"
                >
                  {crossMarketLoading
                    ? "Checking other markets…"
                    : crossMarket === null
                      ? "Check this rule on other markets"
                      : "Run the cross-market check again"}
                </button>
                <p className="text-xs text-[var(--muted)]">
                  Runs the same rule on BTC, ETH and SOL with the costs the results
                  used: fees {submittedRequest?.fee_bps} bps, slippage{" "}
                  {submittedRequest?.slippage_bps} bps.
                </p>
                {crossMarketError !== null && !crossMarketLoading && (
                  <div
                    role="alert"
                    className="rounded border border-[var(--verdict-fails)] p-3 text-sm"
                  >
                    <p className="font-semibold">Could not check other markets:</p>
                    <p>{crossMarketError}</p>
                  </div>
                )}
                {crossMarket !== null && !crossMarketLoading && (
                  <CrossMarketCard result={crossMarket} />
                )}
              </div>
              <StatsTable
                full={result.stats}
                inSample={result.in_sample}
                outOfSample={result.out_of_sample}
                analytics={result.metrics.analytics}
              />
              <EquityCharts
                equity={result.equity_curve}
                split={submittedSplit}
                startMs={submittedStart}
                endMs={submittedEnd}
                analytics={result.metrics.analytics}
              />
              <MonthlyTable monthly={result.metrics.monthly} analytics={result.metrics.analytics} />
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
              <StatsTable
                full={saved.stats}
                inSample={null}
                outOfSample={null}
                analytics={saved.metrics.analytics}
              />
              <EquityCharts
                equity={saved.equity_curve}
                split={null}
                startMs={saved.start_ms}
                endMs={saved.end_ms}
                analytics={saved.metrics.analytics}
              />
              <MonthlyTable monthly={saved.metrics.monthly} analytics={saved.metrics.analytics} />
              <TradesTable trades={saved.trades} />
              <p className="text-xs text-[var(--muted)]">
                These are simulated results on past data. Past results do not
                predict future results.
              </p>
            </div>
          )}
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
