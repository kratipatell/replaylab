import { describe, expect, it } from "vitest";
import {
  MAX_CANDLES,
  MAX_CHART_POINTS,
  PARTIAL_LOAD_NOTE,
  type SeriesPoint,
  downsample,
  drawdownPct,
  estimateCandles,
  formToStrategy,
  formatLoadNote,
  seriesX,
  strategyToForm,
} from "./testlab";
import type { Strategy } from "./api";

function maxDrawdown(values: number[]): number {
  return Math.max(...drawdownPct(values));
}

function valuesOf(points: SeriesPoint[]): number[] {
  return points.map((p) => p.v);
}

describe("downsample", () => {
  it("returns a short series unchanged with its indices", () => {
    const values = [1, 2, 3];
    expect(downsample(values, 10)).toEqual([
      { i: 0, v: 1 },
      { i: 1, v: 2 },
      { i: 2, v: 3 },
    ]);
  });

  it("always keeps the first and last original points", () => {
    const values = Array.from({ length: 1000 }, (_, i) => Math.sin(i / 7));
    const out = downsample(values, 50);
    expect(out[0]).toEqual({ i: 0, v: values[0] });
    expect(out[out.length - 1]).toEqual({ i: 999, v: values[999] });
  });

  it("returns strictly increasing indices with no duplicates", () => {
    const values = Array.from({ length: 1000 }, (_, i) => Math.sin(i / 7) * 3 + 10);
    const out = downsample(values, 50);
    expect(out.length).toBeLessThanOrEqual(50);
    for (let k = 1; k < out.length; k += 1) {
      expect(out[k].i).toBeGreaterThan(out[k - 1].i);
    }
  });

  it("keeps the true minimum and maximum of the series", () => {
    const values = Array.from({ length: 1000 }, (_, i) => Math.sin(i / 7) * 3 + 10);
    values[517] = 2.5;
    values[733] = 21.0;
    const out = valuesOf(downsample(values, 50));
    expect(out.length).toBeLessThanOrEqual(50);
    expect(Math.min(...out)).toBe(Math.min(...values));
    expect(Math.max(...out)).toBe(Math.max(...values));
  });

  it("keeps a spike that falls between bucket boundaries", () => {
    // Flat except one spike on an index a naive every-other stride steps over.
    const values = Array.from({ length: 300 }, () => 5);
    values[151] = 99;
    const out = downsample(values, 40);
    expect(out).toContainEqual({ i: 151, v: 99 });
    expect(Math.max(...valuesOf(out))).toBe(99);
  });

  it("keeps the maximum drawdown when the trough falls between sample points", () => {
    const values: number[] = [];
    for (let i = 0; i < 1200; i += 1) {
      if (i <= 200) {
        values.push(1 + i / 200);
      } else if (i < 855) {
        values.push(1.2);
      } else if (i === 855) {
        values.push(0.5);
      } else {
        values.push(1.5);
      }
    }
    const sampled = downsample(values, MAX_CHART_POINTS);
    expect(sampled.length).toBeLessThanOrEqual(MAX_CHART_POINTS);
    expect(sampled).toContainEqual({ i: 855, v: 0.5 });
    expect(maxDrawdown(valuesOf(sampled))).toBeCloseTo(maxDrawdown(values), 10);
  });
});

describe("seriesX", () => {
  it("places the split index by original index, not by its slot", () => {
    // Buckets hold one point on the flat stretches and two at the spike, so
    // array slots drift from original indices.
    const values = new Array(60).fill(0);
    values[40] = 100;
    const out = downsample(values, 12);
    const splitIndex = 40;
    const length = values.length;
    expect(out).toContainEqual({ i: splitIndex, v: 100 });
    const slot = out.findIndex((p) => p.i === splitIndex);
    expect(slot).not.toBe(splitIndex);
    expect(seriesX(splitIndex, length)).toBe(splitIndex / (length - 1));
  });
});

function roundTrip(strategy: Strategy): Strategy {
  return formToStrategy(strategyToForm(strategy).form);
}

describe("strategy to form mapping", () => {
  it("round-trips a long entry with a number operand and a stop", () => {
    const strategy: Strategy = {
      asset: "BTCUSDT",
      timeframe: "1h",
      side: "long",
      entry: {
        all: [{ left: { ind: "rsi", period: 14 }, op: "<", right: 30 }],
        any: [],
      },
      stop_loss_pct: 2,
    };
    const { form, notes } = strategyToForm(strategy);
    expect(notes).toEqual([]);
    expect(form.preset).toBe("5y");
    expect(form.asset).toBe("BTCUSDT");
    expect(form.timeframe).toBe("1h");
    expect(form.side).toBe("long");
    expect(form.stopLoss).toBe("2");
    expect(form.takeProfit).toBe("");
    expect(roundTrip(strategy)).toEqual(strategy);
  });

  it("round-trips a short idea with any rules and both stop and target", () => {
    const strategy: Strategy = {
      asset: "ETHUSDT",
      timeframe: "4h",
      side: "short",
      entry: {
        all: [],
        any: [{ left: { ind: "close" }, op: "crosses_below", right: { ind: "sma", period: 50 } }],
      },
      exit: {
        all: [{ left: { ind: "rsi", period: 14 }, op: ">", right: 55 }],
        any: [],
      },
      stop_loss_pct: 3,
      take_profit_pct: 6.5,
    };
    expect(strategyToForm(strategy).notes).toEqual([]);
    expect(roundTrip(strategy)).toEqual(strategy);
  });

  it("covers every operator, indicator and operand kind", () => {
    const strategy: Strategy = {
      asset: "SOLUSDT",
      timeframe: "1d",
      side: "long",
      entry: {
        all: [
          { left: { ind: "close" }, op: "crosses_above", right: { ind: "sma", period: 50 } },
          { left: { ind: "rsi", period: 14 }, op: "<", right: 30 },
          { left: { ind: "open" }, op: ">", right: 10 },
        ],
        any: [
          { left: { ind: "ema", period: 9 }, op: ">", right: { ind: "ema", period: 21 } },
          { left: { ind: "high" }, op: "crosses_below", right: 100 },
        ],
      },
      exit: {
        all: [{ left: { ind: "rsi", period: 14 }, op: ">", right: 55 }],
        any: [{ left: { ind: "low" }, op: "<", right: { ind: "sma", period: 200 } }],
      },
      stop_loss_pct: 2,
      take_profit_pct: 4,
    };
    expect(strategyToForm(strategy).notes).toEqual([]);
    expect(roundTrip(strategy)).toEqual(strategy);
  });

  it("drops an unshowable left number and reports it", () => {
    const strategy: Strategy = {
      asset: "BTCUSDT",
      timeframe: "1h",
      side: "long",
      entry: {
        all: [{ left: 30, op: "<", right: { ind: "close" } }],
        any: [],
      },
    };
    const { form, notes } = strategyToForm(strategy);
    expect(form.entry.all).toHaveLength(0);
    expect(notes).toHaveLength(1);
    const note = formatLoadNote(notes);
    expect(note).not.toBeNull();
    expect(note).toContain(PARTIAL_LOAD_NOTE);
    expect(note).toContain("plain number on the left");
  });

  it("notes an unsupported coin and bar size, keeping a usable form", () => {
    const { form, notes } = strategyToForm({
      asset: "FOOUSDT",
      timeframe: "2h",
      side: "long",
      entry: {
        all: [{ left: { ind: "close" }, op: ">", right: 100 }],
        any: [],
      },
    });
    expect(notes).toHaveLength(2);
    expect(form.asset).toBe("BTCUSDT");
    expect(form.timeframe).toBe("1h");
  });

  it("has no note when everything maps", () => {
    const { notes } = strategyToForm({
      asset: "BTCUSDT",
      timeframe: "1h",
      side: "long",
      entry: {
        all: [{ left: { ind: "close" }, op: ">", right: 100 }],
        any: [],
      },
    });
    expect(formatLoadNote(notes)).toBeNull();
  });

  it("picks a range that fits the candle cap for a 15m idea", () => {
    const { form } = strategyToForm({
      asset: "BTCUSDT",
      timeframe: "15m",
      side: "long",
      entry: {
        all: [{ left: { ind: "close" }, op: ">", right: 100 }],
        any: [],
      },
    });
    expect(form.timeframe).toBe("15m");
    expect(form.preset).toBe("1y");
    expect(estimateCandles(form)).toBeLessThanOrEqual(MAX_CANDLES);
  });
});
