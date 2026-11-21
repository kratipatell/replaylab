import { describe, expect, it } from "vitest";
import { ApiError, type StrategyTemplate, type WalkForwardRequest } from "./api";
import { sampleWindow, segment } from "./walkforward.fixture";
import templateResponse from "./__fixtures__/templates/response.json";
import {
  WF_MAX_EVALUATIONS,
  buildWalkForwardRequest,
  classifyWalkForwardError,
  applyTemplateGrid,
  createParamRow,
  defaultWalkForwardForm,
  formatEfficiency,
  parseValues,
  sameWalkForwardInputs,
  suggestValues,
  tuneTargets,
  walkForwardProblems,
  windowFlags,
  windowMarkers,
  type WalkForwardForm,
} from "./walkforward";
import {
  createRule,
  defaultForm,
  type RuleState,
  type TestLabForm,
} from "./testlab";

const NOW = Date.UTC(2026, 0, 15, 12, 0);

function rule(left: Partial<RuleState["left"]>, right: Partial<RuleState["right"]>): RuleState {
  const base = createRule();
  return {
    ...base,
    left: { ...base.left, ...left },
    right: { ...base.right, ...right },
  };
}

function formWithEntry(overrides: Partial<TestLabForm> = {}): TestLabForm {
  return {
    ...defaultForm(),
    entry: {
      all: [rule({ mode: "indicator", ind: "close" }, { mode: "number", num: "100" })],
      any: [],
    },
    ...overrides,
  };
}

function wfWith(targetValues: [string, string][], overrides: Partial<WalkForwardForm> = {}): WalkForwardForm {
  return {
    ...defaultWalkForwardForm(),
    params: targetValues.map(([target, values]) => createParamRow(target, values)),
    ...overrides,
  };
}

describe("applyTemplateGrid", () => {
  it("fills suggested parameter values while preserving the other walk-forward fields", () => {
    const templates = templateResponse as unknown as { templates: StrategyTemplate[] };
    const template = templates.templates[0];
    if (template === undefined) {
      throw new Error("template fixture is empty");
    }
    const initial = wfWith([], {
      mode: "anchored",
      windows: "3",
      isBars: "450",
      oosBars: "120",
      objective: "total_return",
    });

    const loaded = applyTemplateGrid(
      initial,
      template.suggested_walk_forward_parameter_grid,
    );

    expect(loaded.params.map(({ target, values }) => ({ target, values }))).toEqual(
      template.suggested_walk_forward_parameter_grid.map((parameter) => ({
        target: parameter.target,
        values: parameter.values.join(", "),
      })),
    );
    expect(loaded.mode).toBe("anchored");
    expect(loaded.windows).toBe("3");
    expect(loaded.isBars).toBe("450");
    expect(loaded.oosBars).toBe("120");
    expect(loaded.objective).toBe("total_return");

    const edited = {
      ...loaded,
      params: loaded.params.map((param, index) =>
        index === 0 ? { ...param, values: "5, 11" } : param,
      ),
    };
    expect(edited.params[0]?.values).toBe("5, 11");
  });
});

describe("tuneTargets", () => {
  it("offers a numeric threshold on the right side of an entry rule", () => {
    const targets = tuneTargets(formWithEntry());
    expect(targets).toEqual([
      {
        target: "entry.all.0.right",
        label: "Entry rule 1 (all), right side: value",
        kind: "float",
        current: 100,
      },
    ]);
  });

  it("offers an indicator period as an integer target", () => {
    const form = formWithEntry({
      entry: {
        all: [rule({ mode: "indicator", ind: "sma", period: "20" }, { mode: "number", num: "50" })],
        any: [],
      },
    });
    const period = tuneTargets(form).find((t) => t.target === "entry.all.0.left.period");
    expect(period).toMatchObject({ kind: "int", current: 20, label: "Entry rule 1 (all), left side: SMA period" });
  });

  it("offers stop loss and take profit only when they are set", () => {
    const form = formWithEntry({ stopLoss: "2", takeProfit: "" });
    const targets = tuneTargets(form).map((t) => t.target);
    expect(targets).toContain("stop_loss_pct");
    expect(targets).not.toContain("take_profit_pct");
  });

  it("offers nothing when the rule has neither a number nor a period", () => {
    const form = formWithEntry({
      entry: { all: [rule({ mode: "indicator", ind: "close" }, { mode: "indicator", ind: "open" })], any: [] },
    });
    expect(tuneTargets(form)).toEqual([]);
  });

  it("skips a number that is still blank", () => {
    const form = formWithEntry({
      entry: { all: [rule({ mode: "indicator", ind: "close" }, { mode: "number", num: "" })], any: [] },
    });
    expect(tuneTargets(form)).toEqual([]);
  });
});

describe("suggestValues", () => {
  it("brackets a float threshold around its current value", () => {
    expect(suggestValues({ target: "x", label: "", kind: "float", current: 100 })).toBe("90, 100, 110");
  });

  it("keeps indicator periods whole and at least 1", () => {
    expect(suggestValues({ target: "x", label: "", kind: "int", current: 14 })).toBe("11, 14, 17");
    expect(suggestValues({ target: "x", label: "", kind: "int", current: 1 })).toBe("1");
  });
});

describe("parseValues", () => {
  it("splits on commas, ignores empty tokens and reports non-numbers", () => {
    expect(parseValues("98, 100,abc,, 102")).toEqual({ values: [98, 100, 102], bad: ["abc"] });
  });
});

describe("walkForwardProblems", () => {
  const base = formWithEntry();

  it("accepts a sensible setup", () => {
    expect(walkForwardProblems(base, wfWith([["entry.all.0.right", "99, 100, 101"]]), NOW)).toEqual([]);
  });

  it("requires a whole number of windows from 2 to 10", () => {
    const params = [["entry.all.0.right", "99, 100"]] as [string, string][];
    for (const windows of ["1", "11", "2.5", ""]) {
      const problems = walkForwardProblems(base, wfWith(params, { windows }), NOW);
      expect(problems.join(" "), `windows ${windows}`).toContain("Windows must be a whole number from 2 to 10");
    }
    expect(walkForwardProblems(base, wfWith(params, { windows: "10" }), NOW)).toEqual([]);
  });

  it("requires positive whole in-sample and out-of-sample lengths", () => {
    const params = [["entry.all.0.right", "99"]] as [string, string][];
    expect(walkForwardProblems(base, wfWith(params, { isBars: "0" }), NOW).join(" ")).toContain("In-sample bars");
    expect(walkForwardProblems(base, wfWith(params, { oosBars: "abc" }), NOW).join(" ")).toContain("Out-of-sample bars");
  });

  it("rejects a setup that needs more candles than the range holds", () => {
    const params = [["entry.all.0.right", "99"]] as [string, string][];
    const problems = walkForwardProblems(base, wfWith(params, { isBars: "9000", oosBars: "500", windows: "2" }), NOW);
    expect(problems.join(" ")).toContain("This needs");
  });

  it("requires at least one parameter", () => {
    expect(walkForwardProblems(base, wfWith([]), NOW)).toContain("Add at least one parameter to tune.");
  });

  it("flags a parameter whose rule was removed", () => {
    const problems = walkForwardProblems(base, wfWith([["entry.all.5.right", "1, 2"]]), NOW);
    expect(problems.join(" ")).toContain("no longer in the rules");
  });

  it("flags a parameter listed twice", () => {
    const problems = walkForwardProblems(
      base,
      wfWith([["entry.all.0.right", "1"], ["entry.all.0.right", "2"]]),
      NOW,
    );
    expect(problems.join(" ")).toContain("listed more than once");
  });

  it("rejects non-numeric values and empty value lists", () => {
    expect(walkForwardProblems(base, wfWith([["entry.all.0.right", "1, x"]]), NOW).join(" ")).toContain('"x" is not a number');
    expect(walkForwardProblems(base, wfWith([["entry.all.0.right", " , "]]), NOW).join(" ")).toContain("enter at least one value");
  });

  it("requires indicator periods to be whole numbers of 1 or more", () => {
    const form = formWithEntry({
      entry: { all: [rule({ mode: "indicator", ind: "sma", period: "20" }, { mode: "number", num: "50" })], any: [] },
    });
    const problems = walkForwardProblems(form, wfWith([["entry.all.0.left.period", "14.5"]]), NOW);
    expect(problems.join(" ")).toContain("whole numbers of 1 or more");
  });

  it("requires stop loss and take profit values above 0", () => {
    const form = formWithEntry({ stopLoss: "2" });
    const problems = walkForwardProblems(form, wfWith([["stop_loss_pct", "0, 1"]]), NOW);
    expect(problems.join(" ")).toContain("above 0");
  });

  it("caps the number of grid combinations", () => {
    const form = formWithEntry({ stopLoss: "2" });
    const values = Array.from({ length: 50 }, (_, i) => i + 1).join(",");
    const problems = walkForwardProblems(form, wfWith([["entry.all.0.right", values], ["stop_loss_pct", values]]), NOW);
    expect(problems.join(" ")).toContain("combinations");
  });

  it("caps windows times combinations at the engine limit", () => {
    const form = formWithEntry({ stopLoss: "2" });
    const values = Array.from({ length: 40 }, (_, i) => i + 1).join(",");
    const problems = walkForwardProblems(
      form,
      wfWith([["entry.all.0.right", values], ["stop_loss_pct", values]], { windows: "2" }),
      NOW,
    );
    expect(WF_MAX_EVALUATIONS).toBe(2000);
    expect(problems.join(" ")).toContain("optimizer runs");
  });

  it("reports rule problems from the shared validator", () => {
    const problems = walkForwardProblems(defaultForm(), wfWith([["entry.all.0.right", "1"]]), NOW);
    expect(problems.join(" ")).toContain("Add at least one entry rule");
  });
});

describe("buildWalkForwardRequest", () => {
  it("sends exactly the fields the engine's WalkForwardRequest accepts", () => {
    const req = buildWalkForwardRequest(
      formWithEntry(),
      wfWith([["entry.all.0.right", "99, 100, 101"]], { windows: "3", isBars: "400", oosBars: "200", mode: "anchored", objective: "total_return" }),
      NOW,
    );
    expect(Object.keys(req).sort()).toEqual(
      ["end_ms", "fee_bps", "is_bars", "mode", "objective", "oos_bars", "params", "search", "slippage_bps", "start_ms", "strategy", "windows"].sort(),
    );
    expect(req).not.toHaveProperty("split");
    expect(req).toMatchObject({
      windows: 3,
      is_bars: 400,
      oos_bars: 200,
      mode: "anchored",
      objective: "total_return",
      search: { method: "grid" },
      params: [{ target: "entry.all.0.right", kind: "float", values: [99, 100, 101] }],
    });
    expect(req.end_ms).toBe(NOW);
  });

  it("marks indicator periods as integer parameters", () => {
    const form = formWithEntry({
      entry: { all: [rule({ mode: "indicator", ind: "sma", period: "20" }, { mode: "number", num: "50" })], any: [] },
    });
    const req = buildWalkForwardRequest(form, wfWith([["entry.all.0.left.period", "14, 20"]]), NOW);
    expect(req.params).toEqual([{ target: "entry.all.0.left.period", kind: "int", values: [14, 20] }]);
  });
});

describe("sameWalkForwardInputs", () => {
  const form = formWithEntry();
  const wf = wfWith([["entry.all.0.right", "99, 100"]]);

  it("treats a preset range as unchanged as the clock moves on", () => {
    const earlier = buildWalkForwardRequest(form, wf, NOW);
    const later = buildWalkForwardRequest(form, wf, NOW + 3_600_000);
    expect(sameWalkForwardInputs(earlier, later, form.preset)).toBe(true);
  });

  it("detects an edited setting", () => {
    const before: WalkForwardRequest = buildWalkForwardRequest(form, wf, NOW);
    const after = buildWalkForwardRequest(form, { ...wf, windows: "5" }, NOW);
    expect(sameWalkForwardInputs(before, after, form.preset)).toBe(false);
  });

  it("compares a custom range by its exact dates", () => {
    const custom = { ...formWithEntry(), preset: "custom" as const, customStart: "2025-01-01T00:00", customEnd: "2025-06-01T00:00" };
    const before = buildWalkForwardRequest(custom, wf, NOW);
    const after = buildWalkForwardRequest({ ...custom, customStart: "2025-02-01T00:00" }, wf, NOW);
    expect(sameWalkForwardInputs(before, after, "custom")).toBe(false);
  });
});

describe("classifyWalkForwardError", () => {
  it("keeps the engine's 400 message", () => {
    expect(classifyWalkForwardError(new ApiError(400, "not enough candles (10) for 4 windows"))).toEqual({
      kind: "invalid",
      message: "not enough candles (10) for 4 windows",
    });
  });

  it("explains a 429 as a rate limit to retry later", () => {
    const problem = classifyWalkForwardError(new ApiError(429, "too many optimizer requests; try again shortly"));
    expect(problem.kind).toBe("rate_limited");
    expect(problem.message).toMatch(/Wait a minute, then try again/);
  });

  it("separates a timeout from other failures", () => {
    expect(classifyWalkForwardError(new ApiError(408, "walk-forward timed out after 60s")).kind).toBe("timeout");
    expect(classifyWalkForwardError(new ApiError(502, "binance unreachable"))).toEqual({
      kind: "failed",
      message: "binance unreachable",
    });
  });

  it("treats an abort as a cancellation", () => {
    expect(classifyWalkForwardError(Object.assign(new Error("aborted"), { name: "AbortError" })).kind).toBe("cancelled");
  });

  it("maps network errors and non-error values to a failure", () => {
    expect(classifyWalkForwardError(new TypeError("Failed to fetch")).kind).toBe("failed");
    expect(classifyWalkForwardError("boom")).toEqual({ kind: "failed", message: "boom" });
  });
});

describe("formatEfficiency", () => {
  it("shows n/a for an undefined ratio, never NaN or Infinity", () => {
    expect(formatEfficiency(null)).toEqual({ text: "n/a", defined: false });
    expect(formatEfficiency(Number.NaN)).toEqual({ text: "n/a", defined: false });
    expect(formatEfficiency(Number.POSITIVE_INFINITY)).toEqual({ text: "n/a", defined: false });
  });

  it("formats a defined ratio to two places", () => {
    expect(formatEfficiency(0.5)).toEqual({ text: "0.50", defined: true });
    expect(formatEfficiency(1.234)).toEqual({ text: "1.23", defined: true });
  });
});

describe("windowFlags", () => {
  it("flags each segment the engine marks as too thin to judge", () => {
    const window = sampleWindow(1, {
      in_sample: segment(0, 100, { too_few_trades_to_judge: true, trade_count: 4 }),
      out_of_sample: segment(0, 50, { too_few_trades_to_judge: true, trade_count: 2 }),
    });
    expect(windowFlags(window)).toEqual([
      "Too few in-sample trades to judge",
      "Too few out-of-sample trades to judge",
    ]);
  });

  it("returns no flags when both segments have enough trades", () => {
    expect(windowFlags(sampleWindow(1))).toEqual([]);
  });
});

describe("windowMarkers", () => {
  it("points at the first bar of each out-of-sample block in the stitched curve", () => {
    const curve = [10, 20, 30, 40, 50].map((bar_ts) => ({ bar_ts, equity: 1 }));
    const window = sampleWindow(2, {
      out_of_sample: segment(30, 2),
    });
    expect(windowMarkers(curve, [window])).toEqual([{ index: 2, label: "W2" }]);
  });
});
