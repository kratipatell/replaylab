import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { AgentDecision, RiskLimits } from "../lib/api";
import {
  AgentHaltSummary,
  AgentSampleBadge,
  AgentStatusBadge,
} from "./AgentStatus";

const risk: RiskLimits = {
  max_position_pct: 100,
  max_daily_loss_pct: 10,
  max_drawdown_pct: 20,
};

const haltEvent: AgentDecision = {
  bar_ts: Date.UTC(2026, 0, 2, 3, 4),
  kind: "halt_triggered",
  seq: 0,
  reason: "drawdown",
  price: null,
  qty: null,
  requested_qty: null,
  configured: 20,
  observed: 23.5,
  equity: 7_650,
};

describe("agent status UI", () => {
  it("renders a halted badge with the limit", () => {
    const html = renderToStaticMarkup(
      <AgentStatusBadge status="running" halt="Drawdown" />,
    );
    expect(html).toContain("Halted · Drawdown");
  });

  it("renders configured, observed, and bar time for a halt", () => {
    const html = renderToStaticMarkup(
      <AgentHaltSummary halt="Drawdown" risk={risk} event={haltEvent} />,
    );
    expect(html).toContain("20.00%");
    expect(html).toContain("23.50%");
    expect(html).toContain("2026-01-02 03:04 UTC");
  });

  it("labels sample agents and omits the label for ordinary agents", () => {
    const sample = renderToStaticMarkup(<AgentSampleBadge isSample />);
    const ordinary = renderToStaticMarkup(<AgentSampleBadge isSample={false} />);

    expect(sample).toContain("Sample data, not a live run");
    expect(ordinary).toBe("");
  });
});
