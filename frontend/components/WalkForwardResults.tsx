// Results of a walk-forward run: verdict, aggregate out-of-sample figures,
// the stitched out-of-sample curve, each window, and parameter stability.
"use client";

import type { ReactNode } from "react";
import type { WalkForwardReport } from "@/lib/api";
import {
  EFFICIENCY_NA_EXPLANATION,
  WF_OBJECTIVES,
  formatEfficiency,
  formatParamValue,
  labelFor,
  windowFlags,
  windowMarkers,
} from "@/lib/walkforward";
import {
  MAX_CHART_POINTS,
  downsample,
  downsampleNullable,
  fmtDate,
  fmtPct,
  plural,
} from "@/lib/testlab";
import { LineChart, MonthlyTable } from "./EquityCharts";
import StatsTable from "./StatsTable";
import VerdictCard from "./VerdictCard";

const cellClass = "border border-[var(--border)] p-2 align-top";

function StatCard({
  label,
  value,
  note,
}: {
  label: string;
  value: string;
  note: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1 rounded border border-[var(--border)] bg-[var(--surface)] p-3">
      <p className="text-xs text-[var(--muted)]">{label}</p>
      <p className="text-lg font-semibold">{value}</p>
      <div className="text-xs text-[var(--muted)]">{note}</div>
    </div>
  );
}

export default function WalkForwardResults({
  report,
  labels,
  onUseLongRange,
}: {
  report: WalkForwardReport;
  labels: Record<string, string>;
  onUseLongRange: () => void;
}) {
  const curve = report.stitched_oos_equity_curve;
  const underwater = downsampleNullable(
    report.analytics.underwater_curve.map((point) => point.drawdown_pct),
    MAX_CHART_POINTS,
  );
  const efficiency = formatEfficiency(report.efficiency_ratio);
  const objective =
    WF_OBJECTIVES.find((o) => o.value === report.objective)?.label ?? report.objective;
  const modeLabel = report.mode === "anchored" ? "Anchored" : "Rolling";

  return (
    <div className="flex flex-col gap-4">
      <p className="text-xs text-[var(--muted)]">
        {modeLabel} · {plural(report.window_count, "window")} ·{" "}
        {report.total_optimizer_evaluations.toLocaleString()} optimizer runs · best
        parameters chosen by {objective}
      </p>

      <VerdictCard verdict={report.verdict} onUseLongRange={onUseLongRange} />

      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
        <StatCard
          label="Total out-of-sample return"
          value={fmtPct(report.total_oos_return_pct)}
          note="Every out-of-sample window chained together, after costs."
        />
        <StatCard
          label="Worst out-of-sample fall"
          value={fmtPct(report.max_oos_drawdown_pct)}
          note="Largest drop from a peak in the chained out-of-sample equity."
        />
        <StatCard
          label="Longest drawdown"
          value={
            report.analytics.longest_drawdown_duration_bars === null
              ? "n/a"
              : `${report.analytics.longest_drawdown_duration_bars} bars`
          }
          note={
            report.analytics.longest_drawdown_duration_bars === null
              ? "n/a because no positive equity peak was available."
              : "Longest stretch from a peak until recovery, or the latest OOS bar."
          }
        />
        <StatCard
          label="Recovery from deepest drawdown"
          value={
            report.analytics.time_to_recover_max_drawdown_bars === null
              ? "Not recovered / n/a"
              : `${report.analytics.time_to_recover_max_drawdown_bars} bars`
          }
          note={
            report.analytics.time_to_recover_max_drawdown_bars === null
              ? "n/a means no positive baseline or the deepest drawdown remains open."
              : "Bars from the deepest trough back to its prior peak."
          }
        />
        <StatCard
          label="Profitable out-of-sample windows"
          value={`${report.profitable_oos_windows} of ${report.window_count}`}
          note={`${fmtPct(report.profitable_oos_share_pct, 0)} of windows made money. 60% or more is needed for holds.`}
        />
        <StatCard
          label="Efficiency ratio"
          value={efficiency.text}
          note={
            efficiency.defined ? (
              <>
                Out-of-sample return per bar divided by in-sample return per bar.
                0.5 or more is needed for holds.
                <details className="mt-1">
                  <summary className="cursor-pointer">How it is calculated</summary>
                  <p className="mt-1">{report.efficiency_ratio_definition}</p>
                </details>
              </>
            ) : (
              EFFICIENCY_NA_EXPLANATION
            )
          }
        />
      </div>

      <StatsTable
        full={report.out_of_sample_stats}
        inSample={null}
        outOfSample={null}
        fullLabel="Stitched OOS"
        hideBuyAndHold
      />

      <LineChart
        title="Out-of-sample account value"
        sub="Every out-of-sample window in order, each starting where the last one ended. Dotted lines mark where each window starts."
        yLabel="account value, 1.00 = start"
        points={downsample(curve.map((p) => p.equity), MAX_CHART_POINTS)}
        total={curve.length}
        split={null}
        splitMs={null}
        startMs={curve[0]?.bar_ts ?? null}
        endMs={curve[curve.length - 1]?.bar_ts ?? null}
        color="var(--verdict-holds)"
        fill={false}
        markers={windowMarkers(curve, report.windows)}
      />

      <LineChart
        title="Stitched OOS underwater curve"
        sub="Drawdown at each stitched out-of-sample bar; null values have no positive high-water mark."
        yLabel="percent below peak"
        points={underwater}
        total={report.analytics.underwater_curve.length}
        split={null}
        splitMs={null}
        startMs={curve[0]?.bar_ts ?? null}
        endMs={curve[curve.length - 1]?.bar_ts ?? null}
        color="var(--verdict-fails)"
        fill={true}
      />

      <MonthlyTable monthly={{}} analytics={report.analytics} />

      <div className="flex flex-col gap-1">
        <p className="text-sm font-medium">Windows</p>
        <div className="overflow-x-auto">
          <table className="w-full border-collapse text-sm">
            <thead>
              <tr>
                {[
                  "Window",
                  "In-sample dates",
                  "Out-of-sample dates",
                  "Chosen parameters",
                  "In-sample return",
                  "In-sample fall",
                  "In-sample trades",
                  "Out-of-sample return",
                  "Out-of-sample fall",
                  "Out-of-sample trades",
                  "Flags",
                ].map((h) => (
                  <th
                    key={h}
                    className="border border-[var(--border)] bg-[var(--surface)] p-2 text-left font-medium"
                  >
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {report.windows.map((w) => {
                const flags = windowFlags(w);
                return (
                  <tr key={w.window}>
                    <td className={`${cellClass} font-medium`}>W{w.window}</td>
                    <td className={cellClass}>
                      {fmtDate(w.in_sample.range.start_ms)} to {fmtDate(w.in_sample.range.end_ms)}
                    </td>
                    <td className={cellClass}>
                      {fmtDate(w.out_of_sample.range.start_ms)} to{" "}
                      {fmtDate(w.out_of_sample.range.end_ms)}
                    </td>
                    <td className={cellClass}>
                      {Object.entries(w.chosen_parameters).map(([target, value]) => (
                        <div key={target}>
                          {`${labelFor(target, labels)} = ${formatParamValue(value)}`}
                        </div>
                      ))}
                    </td>
                    <td className={`${cellClass} text-right`}>{fmtPct(w.in_sample.return_pct)}</td>
                    <td className={`${cellClass} text-right`}>{fmtPct(w.in_sample.drawdown_pct)}</td>
                    <td className={`${cellClass} text-right`}>{plural(w.in_sample.trade_count, "trade")}</td>
                    <td className={`${cellClass} text-right`}>{fmtPct(w.out_of_sample.return_pct)}</td>
                    <td className={`${cellClass} text-right`}>{fmtPct(w.out_of_sample.drawdown_pct)}</td>
                    <td className={`${cellClass} text-right`}>{plural(w.out_of_sample.trade_count, "trade")}</td>
                    <td className={cellClass}>
                      {flags.length === 0 ? (
                        <span className="text-[var(--muted)]">—</span>
                      ) : (
                        flags.map((f) => (
                          <div key={f} className="text-[var(--verdict-fragile)]">
                            {f}
                          </div>
                        ))
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
        <p className="text-xs text-[var(--muted)]">
          Fewer than 30 trades in a window means too little evidence to judge that window.
        </p>
      </div>

      <section aria-label="Parameter stability" className="flex flex-col gap-2">
        <p className="text-sm font-medium">Parameter stability</p>
        <p className="text-xs text-[var(--muted)]">
          Unstable means the best value kept changing between windows, a sign the
          strategy is overfitting the data rather than finding a stable edge.
        </p>
        {report.parameter_stability.length === 0 ? (
          <p className="text-sm text-[var(--muted)]">No parameters were tuned.</p>
        ) : (
          <ul className="flex flex-col divide-y divide-[var(--border)]">
            {report.parameter_stability.map((p) => (
              <li key={p.parameter} className="flex flex-wrap items-baseline gap-x-3 gap-y-1 py-2">
                <span className="text-sm font-medium">{labelFor(p.parameter, labels)}</span>
                <span className="text-sm">most common value {formatParamValue(p.modal_value)}</span>
                <span className="text-xs text-[var(--muted)]">
                  chosen in {p.modal_windows} of {p.windows} windows (
                  {fmtPct(p.modal_share_pct, 0)})
                </span>
                <span
                  className="text-sm font-semibold"
                  style={{
                    color: p.unstable ? "var(--verdict-fails)" : "var(--verdict-holds)",
                  }}
                >
                  {p.unstable ? "Unstable" : "Stable"}
                </span>
              </li>
            ))}
          </ul>
        )}
        <p className="text-xs text-[var(--muted)]">{report.stability_measure}</p>
      </section>

      <p className="text-xs text-[var(--muted)]">
        These are simulated results on past data. Past results do not predict
        future results.
      </p>
    </div>
  );
}
