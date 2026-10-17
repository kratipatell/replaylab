import { describe, expect, it } from "vitest";
import {
  MAX_CHART_POINTS,
  type SeriesPoint,
  downsample,
  drawdownPct,
  seriesX,
} from "./testlab";

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
