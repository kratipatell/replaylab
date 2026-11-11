import { describe, expect, it } from "vitest";
import { ApiError, type AgentDecision } from "./api";
import {
  agentActionErrorMessage,
  agentControlAvailability,
  agentStartRange,
  decisionPresentation,
  defaultRiskDraft,
  findHaltEvent,
  mergeDecisions,
  performAgentActionAndRefresh,
  validateAgentRisk,
} from "./agents";

function decision(
  barTs: number,
  seq: number,
  kind: string,
  reason: string | null = null,
): AgentDecision {
  return {
    bar_ts: barTs,
    seq,
    kind,
    reason,
    price: null,
    qty: null,
    requested_qty: null,
    configured: null,
    observed: null,
    equity: 10_000,
  };
}

describe("agent form validation", () => {
  it("uses the engine risk defaults", () => {
    expect(defaultRiskDraft()).toEqual({
      max_position_pct: "100",
      max_daily_loss_pct: "10",
      max_drawdown_pct: "20",
    });
  });

  it("accepts finite risk values in (0, 100] and rejects other values", () => {
    expect(validateAgentRisk(true, {
      max_position_pct: "100",
      max_daily_loss_pct: "0.01",
      max_drawdown_pct: "20",
    })).toEqual({});
    const errors = validateAgentRisk(true, {
      max_position_pct: "0",
      max_daily_loss_pct: "101",
      max_drawdown_pct: "Infinity",
    });
    expect(errors.max_position_pct).toContain("> 0 and <= 100");
    expect(errors.max_daily_loss_pct).toContain("> 0 and <= 100");
    expect(errors.max_drawdown_pct).toContain("finite number");
  });

  it("omits risk validation when the optional risk block is disabled", () => {
    expect(validateAgentRisk(false, {
      max_position_pct: "",
      max_daily_loss_pct: "NaN",
      max_drawdown_pct: "-1",
    })).toEqual({});
  });

  it("matches the engine's administrator-paused 409 and preserves other conflicts", () => {
    expect(
      agentActionErrorMessage(new ApiError(409, "agent is halted by the kill switch; clear it first")),
    ).toBe("Trading is paused by an administrator.");
    expect(agentActionErrorMessage(new ApiError(409, "agent is stopped; start it first"))).toBe(
      "agent is stopped; start it first",
    );
  });

  it("enables controls by status and never enables Start for a halted agent", () => {
    expect(agentControlAvailability("draft", null, false)).toEqual({
      start: true,
      stop: false,
      advance: false,
    });
    expect(agentControlAvailability("stopped", null, false).start).toBe(true);
    expect(agentControlAvailability("running", null, false)).toEqual({
      start: false,
      stop: true,
      advance: true,
    });
    expect(agentControlAvailability("stopped", "Drawdown", false)).toEqual({
      start: false,
      stop: false,
      advance: false,
    });
    expect(agentControlAvailability("running", "Drawdown", false)).toEqual({
      start: false,
      stop: true,
      advance: true,
    });
    expect(agentControlAvailability("running", null, true)).toEqual({
      start: false,
      stop: false,
      advance: false,
    });
  });

  it("updates from a successful action and then refetches the detail views", async () => {
    const order: string[] = [];
    await performAgentActionAndRefresh(
      async () => {
        order.push("action");
        return "updated detail";
      },
      (detail) => {
        expect(detail).toBe("updated detail");
        order.push("state");
      },
      async () => {
        order.push("refresh");
      },
    );
    expect(order).toEqual(["action", "state", "refresh"]);
  });

  it("chooses a warm-up range that never exceeds the engine candle cap", () => {
    const now = 1_800_000_000_000;
    for (const timeframe of ["1m", "5m", "15m", "1h", "4h", "1d"] as const) {
      const range = agentStartRange(timeframe, now);
      const step = { "1m": 60_000, "5m": 300_000, "15m": 900_000, "1h": 3_600_000, "4h": 14_400_000, "1d": 86_400_000 }[timeframe];
      expect(range.end_ms).toBe(now);
      expect(Math.floor((range.end_ms - range.start_ms) / step) + 1).toBeLessThanOrEqual(100_000);
    }
  });
});

describe("agent decision helpers", () => {
  it("labels event kinds and explains RiskHalt exits", () => {
    expect(decisionPresentation(decision(10, 0, "signal_received", "long")).label).toBe("Signal · Long");
    expect(decisionPresentation(decision(10, 1, "entry_filled")).label).toBe("Entry filled");
    expect(decisionPresentation(decision(10, 2, "entry_clamped")).label).toBe("Entry clamped");
    expect(decisionPresentation(decision(10, 3, "entry_dropped", "below_min_qty")).help).toContain("minimum quantity");
    expect(decisionPresentation(decision(10, 3, "entry_dropped", "blocked_by_halt")).help).toContain("risk halt");
    const exit = decisionPresentation(decision(10, 4, "exit_filled", "risk_halt"));
    expect(exit.label).toBe("Risk limit exit");
    expect(exit.help).toContain("next bar's open");
    expect(decisionPresentation(decision(10, 5, "halt_triggered", "drawdown")).label).toBe("Halt · Drawdown");
  });

  it("has readable labels for every event kind emitted by SimState::step", () => {
    const engineEvents = [
      decision(10, 0, "signal_received", "short"),
      decision(10, 1, "entry_filled", "short"),
      decision(10, 2, "entry_clamped", "position_cap"),
      decision(10, 3, "entry_dropped", "below_min_qty"),
      decision(10, 4, "entry_dropped", "blocked_by_halt"),
      decision(10, 5, "exit_filled", "risk_halt"),
      decision(10, 6, "halt_triggered", "daily_loss"),
      decision(10, 7, "halt_triggered", "drawdown"),
      decision(10, 8, "halt_triggered", "kill_switch"),
    ];
    const kinds = new Set(engineEvents.map((event) => event.kind));
    expect(kinds).toEqual(new Set([
      "signal_received",
      "entry_filled",
      "entry_clamped",
      "entry_dropped",
      "exit_filled",
      "halt_triggered",
    ]));
    for (const event of engineEvents) {
      const mapped = decisionPresentation(event);
      expect(mapped.label).not.toBe("Decision");
      expect(mapped.help.length).toBeGreaterThan(0);
    }
  });

  it("finds the halt record matching the agent's current halt", () => {
    const rows = [
      decision(300, 0, "exit_filled", "risk_halt"),
      decision(200, 0, "halt_triggered", "drawdown"),
      decision(100, 0, "halt_triggered", "daily_loss"),
    ];
    expect(findHaltEvent(rows, "Drawdown")?.bar_ts).toBe(200);
    expect(findHaltEvent(rows, "KillSwitch")).toBeNull();
  });

  it("merges cursor pages without duplicates and keeps newest-first order", () => {
    const current = [decision(300, 1, "entry_filled"), decision(200, 0, "signal_received")];
    const next = [
      decision(200, 0, "signal_received"),
      decision(100, 0, "exit_filled", "risk_halt"),
    ];
    expect(mergeDecisions(current, next).map((row) => [row.bar_ts, row.seq])).toEqual([
      [300, 1],
      [200, 0],
      [100, 0],
    ]);
  });
});
