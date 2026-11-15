// Contract checks against responses captured from the real engine. The captures
// live in lib/__fixtures__/walk-forward/ and are regenerated with the steps in
// that folder's README.
import { afterEach, describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { ApiError, postWalkForward, type WalkForwardRequest } from "./api";
import { createRule, defaultForm, type TestLabForm } from "./testlab";
import {
  EFFICIENCY_NA_EXPLANATION,
  buildWalkForwardRequest,
  classifyWalkForwardError,
  createParamRow,
  defaultWalkForwardForm,
  formatEfficiency,
  targetLabels,
  tuneTargets,
  windowFlags,
  windowMarkers,
  type WalkForwardForm,
} from "./walkforward";
import WalkForwardResults from "@/components/WalkForwardResults";
import requestRolling from "./__fixtures__/walk-forward/request-rolling.json";
import responseRolling from "./__fixtures__/walk-forward/response-rolling.json";
import responseAnchored from "./__fixtures__/walk-forward/response-anchored.json";
import responseUnstable from "./__fixtures__/walk-forward/response-unstable.json";
import error400 from "./__fixtures__/walk-forward/error-400.json";
import error429 from "./__fixtures__/walk-forward/error-429.json";

// The form the capture was made from: BTCUSDT 1h, custom range, one SMA-period
// target on an entry crossover.
function captureForm(): TestLabForm {
  const rule = createRule();
  return {
    ...defaultForm(),
    preset: "custom",
    customStart: "2026-08-27T03:00",
    customEnd: "2026-10-28T14:00",
    entry: {
      all: [
        {
          ...rule,
          left: { mode: "indicator", num: "", ind: "close", period: "14" },
          op: "crosses_above",
          right: { mode: "indicator", num: "", ind: "sma", period: "20" },
        },
      ],
      any: [],
    },
  };
}

function captureWalkForward(mode: WalkForwardForm["mode"] = "rolling"): WalkForwardForm {
  return {
    ...defaultWalkForwardForm(),
    mode,
    windows: "3",
    isBars: "300",
    oosBars: "150",
    params: [createParamRow("entry.all.0.right.period", "14, 20, 26")],
  };
}

function jsonResponse(value: unknown, status: number): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function stubEngine(body: unknown, status = 200): void {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => jsonResponse(body, status)),
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("UI request builder against the engine", () => {
  it("builds the request the engine accepted in the capture", () => {
    const built = buildWalkForwardRequest(captureForm(), captureWalkForward(), Date.UTC(2026, 10, 15, 14));
    expect(built).toEqual(requestRolling as WalkForwardRequest);
  });
});

describe("captured rolling response", () => {
  it("parses through postWalkForward and renders every field the UI reads", async () => {
    stubEngine(responseRolling);
    const report = await postWalkForward(requestRolling as WalkForwardRequest);

    expect(report.mode).toBe("rolling");
    expect(report.window_count).toBe(3);
    expect(report.total_optimizer_evaluations).toBe(9);
    expect(report.windows).toHaveLength(3);
    expect(report.stitched_oos_equity_curve).toHaveLength(451);
    expect(report.verdict.grade).toBe("not_enough_data");

    // Efficiency is JSON null here: the in-sample return per bar is negative.
    expect(report.efficiency_ratio).toBeNull();
    expect(formatEfficiency(report.efficiency_ratio)).toEqual({ text: "n/a", defined: false });

    // Chosen-parameter keys must be the targets the form's labels are keyed by.
    const labels = targetLabels(captureForm());
    expect(tuneTargets(captureForm()).map((t) => t.target)).toEqual(["entry.all.0.right.period"]);
    for (const window of report.windows) {
      for (const key of Object.keys(window.chosen_parameters)) {
        expect(labels).toHaveProperty([key]);
      }
      expect(windowFlags(window)).toEqual([
        "Too few in-sample trades to judge",
        "Too few out-of-sample trades to judge",
      ]);
    }
    expect(windowMarkers(report.stitched_oos_equity_curve, report.windows).map((m) => m.label)).toEqual([
      "W1",
      "W2",
      "W3",
    ]);

    const html = renderToStaticMarkup(
      <WalkForwardResults report={report} labels={labels} onUseLongRange={() => undefined} />,
    );
    expect(html).toContain("Not enough data");
    expect(html).toContain("n/a");
    expect(html).toContain(EFFICIENCY_NA_EXPLANATION);
    expect(html).toContain("Stable");
    expect(html).toContain("Entry rule 1 (all), right side: SMA period = 26");
    expect(html).not.toMatch(/NaN|Infinity|undefined/);
  });
});

describe("captured anchored response", () => {
  it("keeps the in-sample start fixed and grows the block each window", async () => {
    stubEngine(responseAnchored);
    const report = await postWalkForward({ ...(requestRolling as WalkForwardRequest), mode: "anchored" });
    expect(report.mode).toBe("anchored");
    const starts = report.windows.map((w) => w.in_sample.range.start_ms);
    expect(new Set(starts).size).toBe(1);
    const bars = report.windows.map((w) => w.in_sample.range.bars);
    expect(bars).toEqual([300, 450, 600]);
  });
});

describe("captured unstable-parameter response", () => {
  it("reports the parameter as unstable with its modal share", async () => {
    stubEngine(responseUnstable);
    const report = await postWalkForward(requestRolling as WalkForwardRequest);
    expect(report.parameter_stability).toEqual([
      expect.objectContaining({
        parameter: "entry.all.0.right.period",
        unstable: true,
        windows: 3,
        modal_windows: 1,
      }),
    ]);
    const html = renderToStaticMarkup(
      <WalkForwardResults
        report={report}
        labels={targetLabels(captureForm())}
        onUseLongRange={() => undefined}
      />,
    );
    expect(html).toContain(">Unstable<");
  });
});

describe("captured error bodies", () => {
  it("maps the engine's 400 to an invalid run with its message", async () => {
    stubEngine(error400, 400);
    const error = await postWalkForward(requestRolling as WalkForwardRequest).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({
      status: 400,
      message: error400.error,
    });
    expect(classifyWalkForwardError(error)).toEqual({ kind: "invalid", message: error400.error });
  });

  it("maps the engine's 429 to a rate limit without showing the engine text", async () => {
    stubEngine(error429, 429);
    const error = await postWalkForward(requestRolling as WalkForwardRequest).catch((e: unknown) => e);
    const problem = classifyWalkForwardError(error);
    expect(problem.kind).toBe("rate_limited");
    expect(problem.message).toMatch(/Wait a minute, then try again/);
    expect(problem.message).not.toContain(error429.error);
  });
});
