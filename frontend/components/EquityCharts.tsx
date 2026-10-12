// Equity curve and drawdown as inline SVG, no chart library.
"use client";

import { MAX_CHART_POINTS, downsample, drawdownPct, fmtNum } from "@/lib/testlab";

const W = 640;
const H = 200;
const PAD = 34;

function points(values: number[], min: number, max: number): string {
  const span = max - min || 1;
  return values
    .map((v, i) => {
      const x = values.length === 1 ? 0 : (i / (values.length - 1)) * (W - 2 * PAD) + PAD;
      const y = H - PAD - ((v - min) / span) * (H - 2 * PAD);
      return `${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
}

function LineChart({
  title,
  sub,
  values,
  split,
  color,
  fill,
}: {
  title: string;
  sub: string;
  values: number[];
  split: number | null;
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
  const splitX = split === null ? null : PAD + split * (W - 2 * PAD);
  const lastX = W - PAD;
  const area = `${d} L${lastX.toFixed(1)},${(H - PAD).toFixed(1)} L${PAD.toFixed(1)},${(H - PAD).toFixed(1)} Z`;

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
          <line
            x1={splitX}
            y1={PAD - 8}
            x2={splitX}
            y2={H - PAD + 8}
            stroke="var(--muted)"
            strokeDasharray="4 3"
            strokeWidth={1}
          />
        )}
        <text x={4} y={PAD} fontSize={11} fill="var(--muted)">
          {fmtNum(max)}
        </text>
        <text x={4} y={H - PAD + 12} fontSize={11} fill="var(--muted)">
          {fmtNum(min)}
        </text>
      </svg>
    </div>
  );
}

export default function EquityCharts({
  equity,
  split,
}: {
  equity: number[];
  split: number | null;
}) {
  const curve = downsample(equity, MAX_CHART_POINTS);
  const dd = drawdownPct(curve);
  return (
    <div className="grid gap-4 md:grid-cols-2">
      <LineChart
        title="Account growth"
        sub="Test account value over time; the dashed line marks where held-back checking data starts."
        values={curve}
        split={split}
        color="var(--verdict-holds)"
        fill={false}
      />
      <LineChart
        title="Losing stretches"
        sub="How far the account fell from its last peak, in percent."
        values={dd}
        split={split}
        color="var(--verdict-fails)"
        fill={true}
      />
    </div>
  );
}
