import {
  ApiError,
  type AgentDecision,
  type AgentStatus,
  type HaltLimit,
  type RiskLimits,
  type Strategy,
} from "./api";
import { exitReasonHelp, exitReasonLabel, timeframeStepMs } from "./testlab";

export const DEFAULT_AGENT_RISK: RiskLimits = {
  max_position_pct: 100,
  max_daily_loss_pct: 10,
  max_drawdown_pct: 20,
};

export type RiskField = keyof RiskLimits;
export type RiskDraft = Record<RiskField, string>;
export type RiskErrors = Partial<Record<RiskField, string>>;

export const RISK_FIELDS: { key: RiskField; label: string }[] = [
  { key: "max_position_pct", label: "Max position (%)" },
  { key: "max_daily_loss_pct", label: "Daily loss limit (%)" },
  { key: "max_drawdown_pct", label: "Max drawdown (%)" },
];

export function defaultRiskDraft(): RiskDraft {
  return {
    max_position_pct: String(DEFAULT_AGENT_RISK.max_position_pct),
    max_daily_loss_pct: String(DEFAULT_AGENT_RISK.max_daily_loss_pct),
    max_drawdown_pct: String(DEFAULT_AGENT_RISK.max_drawdown_pct),
  };
}

export function validateAgentRisk(enabled: boolean, draft: RiskDraft): RiskErrors {
  if (!enabled) return {};
  const errors: RiskErrors = {};
  for (const { key } of RISK_FIELDS) {
    const raw = draft[key].trim();
    const value = raw === "" ? 0 : Number(raw);
    if (!Number.isFinite(value)) {
      errors[key] = `${key} must be a finite number greater than 0 and at most 100.`;
    } else if (value <= 0 || value > 100) {
      errors[key] = `${key} must be > 0 and <= 100, got ${value}`;
    }
  }
  return errors;
}

export function riskLimitsFromDraft(draft: RiskDraft): RiskLimits {
  return {
    max_position_pct: Number(draft.max_position_pct),
    max_daily_loss_pct: Number(draft.max_daily_loss_pct),
    max_drawdown_pct: Number(draft.max_drawdown_pct),
  };
}

export function strategyDescription(strategy: Strategy): string {
  const indicatorName: Record<string, string> = {
    close: "Close",
    open: "Open",
    high: "High",
    low: "Low",
    sma: "SMA",
    ema: "EMA",
    rsi: "RSI",
  };
  const operand = (value: number | { ind: string; period?: number }): string =>
    typeof value === "number"
      ? String(value)
      : `${indicatorName[value.ind] ?? value.ind}${value.period === undefined ? "" : `(${value.period})`}`;
  const operator: Record<string, string> = {
    "<": "<",
    ">": ">",
    crosses_above: "crosses above",
    crosses_below: "crosses below",
  };
  const rules = [...strategy.entry.all, ...strategy.entry.any];
  const ruleText = rules
    .slice(0, 2)
    .map((rule) => `${operand(rule.left)} ${operator[rule.op]} ${operand(rule.right)}`)
    .join("; ");
  const remainder = rules.length > 2 ? `; +${rules.length - 2} more` : "";
  return `${strategy.side === "long" ? "Long" : "Short"} · ${strategy.asset} · ${strategy.timeframe}${ruleText ? ` · ${ruleText}${remainder}` : ""}`;
}

export function agentStatusLabel(status: AgentStatus, halt: HaltLimit | null): string {
  if (halt !== null) return `Halted · ${haltLimitLabel(halt)}`;
  switch (status) {
    case "draft":
      return "Draft";
    case "running":
      return "Running";
    case "stopped":
      return "Stopped";
  }
}

export function agentActionErrorMessage(error: unknown): string {
  if (error instanceof ApiError && error.status === 409) {
    if (/kill switch|kill-switch|administrator/i.test(error.message)) {
      return "Trading is paused by an administrator.";
    }
  }
  return error instanceof Error ? error.message : String(error);
}

export interface AgentControlAvailability {
  start: boolean;
  stop: boolean;
  advance: boolean;
}

export function agentControlAvailability(
  status: AgentStatus,
  halt: HaltLimit | null,
  busy: boolean,
): AgentControlAvailability {
  const running = status === "running";
  return {
    start: !busy && !running && halt === null,
    stop: !busy && running,
    advance: !busy && running,
  };
}

export async function performAgentActionAndRefresh<T>(
  action: () => Promise<T>,
  update: (result: T) => void,
  refresh: () => Promise<void>,
): Promise<void> {
  const result = await action();
  update(result);
  await refresh();
}

export function haltLimitLabel(limit: HaltLimit): string {
  switch (limit) {
    case "DailyLoss":
      return "Daily loss";
    case "Drawdown":
      return "Drawdown";
    case "KillSwitch":
      return "Kill switch";
  }
}

const HALT_REASON: Record<HaltLimit, string> = {
  DailyLoss: "daily_loss",
  Drawdown: "drawdown",
  KillSwitch: "kill_switch",
};

export function findHaltEvent(
  decisions: AgentDecision[],
  halt: HaltLimit | null,
): AgentDecision | null {
  if (halt === null) return null;
  return (
    decisions.find(
      (decision) =>
        decision.kind === "halt_triggered" && decision.reason === HALT_REASON[halt],
    ) ?? null
  );
}

const EXIT_REASON_CODE: Record<string, Parameters<typeof exitReasonLabel>[0]> = {
  signal: "Signal",
  stop_loss: "StopLoss",
  take_profit: "TakeProfit",
  trailing: "Trailing",
  session_end: "SessionEnd",
  end_of_data: "EndOfData",
  risk_halt: "RiskHalt",
};

export interface DecisionPresentation {
  label: string;
  help: string;
}

export function decisionPresentation(event: AgentDecision): DecisionPresentation {
  switch (event.kind) {
    case "signal_received":
      return {
        label: `Signal · ${event.reason === "short" ? "Short" : "Long"}`,
        help: "The strategy produced an entry signal at this bar's close.",
      };
    case "entry_filled":
      return {
        label: "Entry filled",
        help: "The queued entry filled at this bar's open.",
      };
    case "entry_clamped":
      return {
        label: "Entry clamped",
        help: "The position cap reduced the requested quantity.",
      };
    case "entry_dropped":
      return {
        label: "Entry dropped",
        help:
          event.reason === "blocked_by_halt"
            ? "A risk halt blocked the queued entry."
            : "The capped order was below the venue's minimum quantity.",
      };
    case "exit_filled": {
      const reason = event.reason === null ? undefined : EXIT_REASON_CODE[event.reason];
      return reason === undefined
        ? { label: "Exit filled", help: "The position closed at this bar." }
        : { label: `${exitReasonLabel(reason)} exit`, help: exitReasonHelp(reason) };
    }
    case "halt_triggered": {
      const label =
        event.reason === "daily_loss"
          ? "Daily loss"
          : event.reason === "kill_switch"
            ? "Kill switch"
            : "Drawdown";
      return {
        label: `Halt · ${label}`,
        help: "The configured risk limit was reached at this bar's close.",
      };
    }
    default:
      return { label: "Decision", help: "A paper-agent decision was recorded." };
  }
}

export function mergeDecisions(
  current: AgentDecision[],
  incoming: AgentDecision[],
): AgentDecision[] {
  const unique = new Map<string, AgentDecision>();
  for (const event of [...current, ...incoming]) {
    unique.set(`${event.bar_ts}:${event.seq}:${event.kind}`, event);
  }
  return [...unique.values()].sort(
    (a, b) => b.bar_ts - a.bar_ts || b.seq - a.seq,
  );
}

export function agentStartRange(
  timeframe: string,
  endMs: number,
): { start_ms: number; end_ms: number } {
  const maxSpan = 99_999 * (timeframeStepMs(timeframe) ?? 3_600_000);
  const span = Math.min(365 * 86_400_000, maxSpan);
  return { start_ms: endMs - span, end_ms: endMs };
}
