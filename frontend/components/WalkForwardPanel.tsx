// Walk-forward mode of the Test Lab. Uses the same strategy, date range and
// cost fields as the single test; the window settings are its own.
"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import {
  postWalkForward,
  type StrategyTemplate,
  type WalkForwardReport,
  type WalkForwardRequest,
} from "@/lib/api";
import {
  estimateCandles,
  type TestLabForm,
} from "@/lib/testlab";
import {
  buildWalkForwardRequest,
  classifyWalkForwardError,
  applyTemplateGrid,
  defaultWalkForwardForm,
  sameWalkForwardInputs,
  targetLabels,
  tuneTargets,
  walkForwardProblems,
  type RunProblem,
  type WalkForwardForm,
} from "@/lib/walkforward";
import { CostFields, DateRangeFields, StrategyFields } from "./FormSections";
import WalkForwardResults from "./WalkForwardResults";
import WalkForwardSettings from "./WalkForwardSettings";

const HINTS: Record<RunProblem["kind"], string | null> = {
  invalid: "Check the settings above against this message, then run again.",
  rate_limited: null,
  timeout: "Try fewer windows, a smaller parameter grid, or a shorter date range.",
  failed: "Try again. If it keeps failing, check that the engine is running.",
  cancelled: null,
};

export function RunProblemNotice({ problem }: { problem: RunProblem }) {
  if (problem.kind === "cancelled") {
    return (
      <p role="status" className="rounded border border-[var(--border)] p-3 text-sm">
        {problem.message} The engine may still finish this run in the background.
      </p>
    );
  }
  const title: Record<Exclude<RunProblem["kind"], "cancelled">, string> = {
    invalid: "The engine rejected these settings:",
    rate_limited: "Too many runs right now:",
    timeout: "The run took too long:",
    failed: "The run failed:",
  };
  const hint = HINTS[problem.kind];
  return (
    <div role="alert" className="flex flex-col gap-1 rounded border border-[var(--verdict-fails)] p-3 text-sm">
      <p className="font-semibold">{title[problem.kind]}</p>
      <p>{problem.message}</p>
      {hint !== null && <p className="text-[var(--muted)]">{hint}</p>}
    </div>
  );
}

export default function WalkForwardPanel({
  form,
  onFormChange,
  templateSelection = null,
}: {
  form: TestLabForm;
  onFormChange: (next: TestLabForm) => void;
  templateSelection?: { template: StrategyTemplate } | null;
}) {
  const [wf, setWf] = useState<WalkForwardForm>(defaultWalkForwardForm);
  const [attempted, setAttempted] = useState(false);
  const [clientErrors, setClientErrors] = useState<string[]>([]);
  const [running, setRunning] = useState(false);
  const [problem, setProblem] = useState<RunProblem | null>(null);
  const [report, setReport] = useState<WalkForwardReport | null>(null);
  const [submitted, setSubmitted] = useState<{
    request: WalkForwardRequest;
    labels: Record<string, string>;
  } | null>(null);
  const controllerRef = useRef<AbortController | null>(null);

  useEffect(() => {
    return () => {
      controllerRef.current?.abort();
    };
  }, []);

  useEffect(() => {
    if (templateSelection === null) {
      return;
    }
    let cancelled = false;
    queueMicrotask(() => {
      if (cancelled) {
        return;
      }
      setWf((current) =>
        applyTemplateGrid(
          current,
          templateSelection.template.suggested_walk_forward_parameter_grid,
        ),
      );
      setAttempted(false);
      setClientErrors([]);
      setProblem(null);
    });
    return () => {
      cancelled = true;
    };
  }, [templateSelection]);

  const submit = useCallback(async (): Promise<void> => {
    if (controllerRef.current !== null) {
      return;
    }
    const now = Date.now();
    setAttempted(true);
    const problems = walkForwardProblems(form, wf, now);
    setClientErrors(problems);
    setProblem(null);
    if (problems.length > 0) {
      return;
    }
    const request = buildWalkForwardRequest(form, wf, now);
    const labels = targetLabels(form);
    const controller = new AbortController();
    controllerRef.current = controller;
    setRunning(true);
    setReport(null);
    setSubmitted(null);
    try {
      const result = await postWalkForward(request, controller.signal);
      setReport(result);
      setSubmitted({ request, labels });
    } catch (e) {
      setProblem(classifyWalkForwardError(e));
    } finally {
      if (controllerRef.current === controller) {
        controllerRef.current = null;
      }
      setRunning(false);
    }
  }, [form, wf]);

  const cancel = (): void => {
    controllerRef.current?.abort();
  };

  // Presets are relative to the clock and are compared by length, so the
  // submitted end time stands in for "now" here; custom dates are absolute.
  const stale =
    submitted !== null &&
    !sameWalkForwardInputs(
      submitted.request,
      buildWalkForwardRequest(form, wf, submitted.request.end_ms),
      form.preset,
    );

  return (
    <div className="flex flex-col gap-6">
      <form
        className="flex flex-col gap-4"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <p className="text-sm text-[var(--muted)]">
          Walk-forward picks the best parameters on one stretch of history, then
          checks them on the next stretch the optimizer never saw. It repeats this
          window by window.
        </p>

        <StrategyFields form={form} onChange={onFormChange} />
        <DateRangeFields form={form} onChange={onFormChange} />
        <WalkForwardSettings
          value={wf}
          onChange={setWf}
          targets={tuneTargets(form)}
          candleCount={estimateCandles(form)}
        />
        <CostFields form={form} onChange={onFormChange} />

        <div className="flex flex-wrap items-center gap-3">
          <button
            type="submit"
            disabled={running}
            className="w-fit rounded bg-[var(--accent)] px-4 py-2 text-sm font-medium text-[var(--accent-foreground)] disabled:cursor-not-allowed disabled:opacity-50"
          >
            {running ? "Running walk-forward…" : "Run walk-forward"}
          </button>
          {running && (
            <button
              type="button"
              onClick={cancel}
              className="w-fit rounded border border-[var(--border)] px-4 py-2 text-sm"
            >
              Cancel run
            </button>
          )}
        </div>
        <p className="text-xs text-[var(--muted)]">
          Walk-forward runs the optimizer once per window, so it can take a while.
          Cancel stops the walk-forward run on the engine.
        </p>
      </form>

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

      {running && (
        <p role="status" className="text-sm text-[var(--muted)]">
          Running walk-forward. The optimizer is working through each window&apos;s in-sample bars.
        </p>
      )}

      {problem !== null && !running && <RunProblemNotice problem={problem} />}

      {!running && report === null && problem === null && (
        <p className="text-sm text-[var(--muted)]">
          No walk-forward run yet. Set the windows and parameters above, then run it.
        </p>
      )}

      {report !== null && submitted !== null && !running && (
        <div className="flex flex-col gap-4">
          {stale && (
            <p
              role="status"
              className="rounded border border-[var(--verdict-fragile)] bg-[var(--surface)] p-3 text-sm"
            >
              The inputs changed since this run. Run walk-forward again to update these results.
            </p>
          )}
          <WalkForwardResults
            report={report}
            labels={submitted.labels}
            onUseLongRange={() => onFormChange({ ...form, preset: "5y" })}
          />
        </div>
      )}
    </div>
  );
}
