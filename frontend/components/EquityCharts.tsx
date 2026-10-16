// Equity curve and drawdown as inline SVG, no chart library.
"use client";

import {
  MAX_CHART_POINTS,
  MONTH_NAMES,
  downsample,
  drawdownPct,
  fmtDate,
  fmtNum,
  fmtPct,
  groupMonthly,
} from "@/lib/testlab";

const W = 640;
const H = 300;
const PAD_L = 52;
const PAD_R = 12;
const PAD_T = 30;
const PAD_B = 44;

function points(values: number[], min: number, max: number): string {
  const span = max - min || 1;
  const plotW = W - PAD_L - PAD_R;
  const plotH = H - PAD_T - PAD_B;
  return values
    .map((v, i) => {
      const x = values.length === 1 ? PAD_L : (i / (values.length - 1)) * plotW + PAD_L;
      const y = PAD_T + plotH - ((v - min) / span) * plotH;
      return `${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
}

function LineChart({
  title,
  sub,
  yLabel,
  values,
  split,
  splitMs,
  startMs,
  endMs,
  color,
  fill,
}: {
  title: string;
  sub: string;
  yLabel: string;
  values: number[];
  split: number | null;
  splitMs: number | null;
  startMs: number | null;
  endMs: number | null;
  color: string;
  fill: boolean;
}) {
  if (values.length === 0) {
    return (
      <div className="flex flex-col gap-1">
        <p className="text-sm font-medium">{title}</p>
        <p className="text-xs text-[var(--muted)]">No data to draw.</p>
      </div>
    );
  }
  const min = Math.min(...values);
  const max = Math.max(...values);
  const d = points(values, min, max);
  const plotW = W - PAD_L - PAD_R;
  const plotH = H - PAD_T - PAD_B;
  const baseY = PAD_T + plotH;
  const splitX = split === null ? null : PAD_L + split * plotW;
  const lastX = W - PAD_R;
  const area = `${d} L${lastX.toFixed(1)},${baseY.toFixed(1)} L${PAD_L.toFixed(1)},${baseY.toFixed(1)} Z`;

  return (
    <div className="flex flex-col gap-1">
      <p className="text-sm font-medium">{title}</p>
      <p className="text-xs text-[var(--muted)]">{sub}</p>
      <svg
        viewBox={`0 0 ${W} ${H}`}
        className="h-auto w-full rounded border border-[var(--border)] bg-[var(--surface)]"
        role="img"
        aria-label={`${title}, from ${fmtNum(min)} to ${fmtNum(max)}`}
      >
        {fill && <path d={area} fill={color} opacity={0.25} />}
        <path d={d} fill="none" stroke={color} strokeWidth={1.5} />
        {splitX !== null && (
          <g>
            <line
              x1={splitX}
              y1={PAD_T - 14}
              x2={splitX}
              y2={baseY + 6}
              stroke="var(--muted)"
              strokeDasharray="4 3"
              strokeWidth={1}
            />
            <text
              x={splitX}
              y={PAD_T - 18}
              fontSize={11}
              fill="var(--muted)"
              textAnchor="middle"
            >
              Checked on data after this line
            </text>
            {splitMs !== null && (
              <text
                x={splitX}
                y={PAD_T - 5}
                fontSize={11}
                fill="var(--muted)"
                textAnchor="middle"
              >
                {fmtDate(splitMs)}
              </text>
            )}
          </g>
        )}
        <text
          x={10}
          y={PAD_T + plotH / 2}
          fontSize={11}
          fill="var(--muted)"
          textAnchor="middle"
          transform={`rotate(-90 10 ${PAD_T + plotH / 2})`}
        >
          {yLabel}
        </text>
        <text x={PAD_L - 6} y={PAD_T + 4} fontSize={11} fill="var(--muted)" textAnchor="end">
          {fmtNum(max)}
        </text>
        <text x={PAD_L - 6} y={baseY} fontSize={11} fill="var(--muted)" textAnchor="end">
          {fmtNum(min)}
        </text>
        {startMs !== null && (
          <text x={PAD_L} y={H - 22} fontSize={11} fill="var(--muted)" textAnchor="start">
            {fmtDate(startMs)}
          </text>
        )}
        {endMs !== null && (
          <text x={W - PAD_R} y={H - 22} fontSize={11} fill="var(--muted)" textAnchor="end">
            {fmtDate(endMs)}
          </text>
        )}
        <text x={PAD_L + plotW / 2} y={H - 8} fontSize={11} fill="var(--muted)" textAnchor="middle">
          dates in UTC
        </text>
      </svg>
    </div>
  );
}

export function MonthlyTable({ monthly }: { monthly: Record<string, number> }) {
  const { years, cells } = groupMonthly(monthly);
  if (years.length === 0) {
    return (
      <div className="flex flex-col gap-1">
        <p className="text-sm font-medium">Monthly returns</p>
        <p className="text-xs text-[var(--muted)]">No monthly data.</p>
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-1">
      <p className="text-sm font-medium">Monthly returns</p>
      <div className="overflow-x-auto">
        <table className="w-full border-collapse text-sm">
          <thead>
            <tr>
              <th className="border border-[var(--border)] bg-[var(--surface)] p-2 text-left font-medium">
                Year (UTC)
              </th>
              {MONTH_NAMES.map((m) => (
                <th
                  key={m}
                  className="border border-[var(--border)] bg-[var(--surface)] p-2 text-right font-medium"
                >
                  {m}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {years.map((year) => (
              <tr key={year}>
                <td className="border border-[var(--border)] p-2 font-medium">{year}</td>
                {MONTH_NAMES.map((_, idx) => {
                  const key = `${year}-${String(idx + 1).padStart(2, "0")}`;
                  const v = cells.get(key);
                  if (v === undefined || v === 0) {
                    return (
                      <td
                        key={key}
                        className="border border-[var(--border)] p-2 text-center text-[var(--muted)]"
                      >
                        —
                      </td>
                    );
                  }
                  const text = fmtPct(v, 1);
                  const neutral = text === "-0.0%" || text === "0.0%";
                  const positive = v >= 0;
                  return (
                    <td
                      key={key}
                      className="border border-[var(--border)] p-2 text-right"
                      style={{
                        color: neutral
                          ? "var(--muted)"
                          : positive
                            ? "var(--verdict-holds)"
                            : "var(--verdict-fails)",
                      }}
                    >
                      {neutral ? "0.0%" : text}
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="text-xs text-[var(--muted)]">
        A dash means a month with no trades or a single candle. Months in UTC.
      </p>
    </div>
  );
}

export default function EquityCharts({
  equity,
  split,
  startMs,
  endMs,
}: {
  equity: number[];
  split: number | null;
  startMs: number | null;
  endMs: number | null;
}) {
  const curve = downsample(equity, MAX_CHART_POINTS);
  const dd = drawdownPct(curve);
  const splitMs =
    split === null || startMs === null || endMs === null
      ? null
      : startMs + (endMs - startMs) * split;
  return (
    <div className="grid gap-4 md:grid-cols-2">
      <LineChart
        title="Account growth"
        sub="Test account value over time; the dashed line marks where held-back checking data starts."
        yLabel="account value, 1.00 = starting value"
        values={curve}
        split={split}
        splitMs={splitMs}
        startMs={startMs}
        endMs={endMs}
        color="var(--verdict-holds)"
        fill={false}
      />
      <LineChart
        title="Losing stretches"
        sub="How far the account fell from its last peak, in percent."
        yLabel="percent below peak"
        values={dd}
        split={split}
        splitMs={splitMs}
        startMs={startMs}
        endMs={endMs}
        color="var(--verdict-fails)"
        fill={true}
      />
    </div>
  );
}
