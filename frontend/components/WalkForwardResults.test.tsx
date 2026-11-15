import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { Grade, WalkForwardReport } from "@/lib/api";
import { EFFICIENCY_NA_EXPLANATION } from "@/lib/walkforward";
import { sampleWalkForwardReport, sampleWindow, segment } from "@/lib/walkforward.fixture";
import WalkForwardResults from "./WalkForwardResults";

const LABELS = { "entry.all.0.right": "Entry rule 1 (all), right side: value" };

function render(report: WalkForwardReport): string {
  return renderToStaticMarkup(
    <WalkForwardResults report={report} labels={LABELS} onUseLongRange={() => undefined} />,
  );
}

function count(haystack: string, needle: string): number {
  return haystack.split(needle).length - 1;
}

describe("WalkForwardResults verdict", () => {
  const cases: [Grade, string][] = [
    ["holds", "Holds"],
    ["fragile", "Fragile"],
    ["fails", "Fails"],
    ["not_enough_data", "Not enough data"],
  ];

  for (const [grade, label] of cases) {
    it(`renders the ${grade} verdict with its label and the engine's headline`, () => {
      const html = render(
        sampleWalkForwardReport({
          verdict: {
            grade,
            headline: `Headline for ${grade}`,
            reasons: [`Reason for ${grade}`],
            oos_trades: 12,
          },
        }),
      );
      expect(html).toContain(label);
      expect(html).toContain(`Headline for ${grade}`);
    });
  }
});

describe("WalkForwardResults aggregates", () => {
  it("shows n/a with an explanation when the efficiency ratio is undefined", () => {
    const html = render(sampleWalkForwardReport({ efficiency_ratio: null }));
    expect(html).toContain("n/a");
    expect(html).toContain(EFFICIENCY_NA_EXPLANATION);
    expect(html).not.toContain("NaN");
    expect(html).not.toContain("Infinity");
  });

  it("shows a defined efficiency ratio to two places without the n/a note", () => {
    const html = render(sampleWalkForwardReport({ efficiency_ratio: 0.75 }));
    expect(html).toContain(">0.75<");
    expect(html).not.toContain(EFFICIENCY_NA_EXPLANATION);
  });

  it("shows the profitable window count out of the total", () => {
    const html = render(sampleWalkForwardReport({ profitable_oos_windows: 1, window_count: 2, profitable_oos_share_pct: 50 }));
    expect(html).toContain("1 of 2");
  });
});

describe("WalkForwardResults windows", () => {
  it("flags a window whose out-of-sample segment is too thin to judge", () => {
    const html = render(
      sampleWalkForwardReport({
        windows: [
          sampleWindow(1),
          sampleWindow(2, {
            out_of_sample: segment(0, 50, { too_few_trades_to_judge: true, trade_count: 3 }),
          }),
        ],
      }),
    );
    expect(count(html, "Too few out-of-sample trades to judge")).toBe(1);
    expect(html).not.toContain("Too few in-sample trades to judge");
  });

  it("shows no flag when every segment has enough trades", () => {
    const html = render(sampleWalkForwardReport());
    expect(html).not.toContain("Too few");
  });

  it("labels each chosen parameter with its rule name", () => {
    const html = render(sampleWalkForwardReport());
    expect(html).toContain("Entry rule 1 (all), right side: value = 100");
  });

  it("marks the start of each window on the chart", () => {
    const html = render(sampleWalkForwardReport());
    expect(html).toContain(">W1<");
    expect(html).toContain(">W2<");
  });
});

describe("WalkForwardResults parameter stability", () => {
  it("flags an unstable parameter", () => {
    const html = render(
      sampleWalkForwardReport({
        parameter_stability: [
          {
            parameter: "entry.all.0.right",
            modal_value: 100,
            modal_windows: 1,
            windows: 2,
            modal_share_pct: 50,
            unstable: true,
          },
        ],
      }),
    );
    expect(html).toContain(">Unstable<");
    expect(html).not.toContain(">Stable<");
    expect(html).toContain("overfitting");
  });

  it("flags a stable parameter", () => {
    const html = render(sampleWalkForwardReport());
    expect(html).toContain(">Stable<");
    expect(html).not.toContain(">Unstable<");
  });
});
