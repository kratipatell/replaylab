import type { AgentStatus as AgentStatusValue, HaltLimit, RiskLimits } from "../lib/api";
import { fmtDateTime, fmtPct } from "../lib/testlab";
import { agentStatusLabel, haltLimitLabel } from "../lib/agents";
import type { AgentDecision } from "../lib/api";

export function AgentStatusBadge({
  status,
  halt,
}: {
  status: AgentStatusValue;
  halt: HaltLimit | null;
}) {
  const label = agentStatusLabel(status, halt);
  const tone =
    halt !== null
      ? "border-[var(--verdict-fragile)] text-[var(--verdict-fragile)]"
      : status === "running"
        ? "border-[var(--verdict-holds)] text-[var(--verdict-holds)]"
        : "border-[var(--border)] text-[var(--muted)]";

  return (
    <span className={`inline-flex min-h-8 items-center rounded border px-2 text-xs font-semibold ${tone}`}>
      {label}
    </span>
  );
}

function configuredLimit(risk: RiskLimits, halt: HaltLimit): number | null {
  switch (halt) {
    case "DailyLoss":
      return risk.max_daily_loss_pct;
    case "Drawdown":
      return risk.max_drawdown_pct;
    case "KillSwitch":
      return null;
  }
}

export function AgentHaltSummary({
  halt,
  risk,
  event,
}: {
  halt: HaltLimit;
  risk: RiskLimits;
  event: AgentDecision | null;
}) {
  const configured = event?.configured ?? configuredLimit(risk, halt);

  return (
    <section
      aria-label="Halt details"
      className="flex flex-col gap-2 rounded border border-[var(--verdict-fragile)] p-4"
    >
      <h2 className="text-sm font-semibold">Trading halted · {haltLimitLabel(halt)}</h2>
      <dl className="grid gap-3 text-sm sm:grid-cols-3">
        <div>
          <dt className="text-xs text-[var(--muted)]">Configured</dt>
          <dd>{configured === null ? "Not applicable" : fmtPct(configured)}</dd>
        </div>
        <div>
          <dt className="text-xs text-[var(--muted)]">Observed</dt>
          <dd>{event?.observed === null || event?.observed === undefined ? "—" : fmtPct(event.observed)}</dd>
        </div>
        <div>
          <dt className="text-xs text-[var(--muted)]">Bar time (UTC)</dt>
          <dd>{event === null ? "Not in the loaded decision page" : fmtDateTime(event.bar_ts)}</dd>
        </div>
      </dl>
    </section>
  );
}
