"use client";

import Link from "next/link";
import { useCallback, useEffect, useState } from "react";
import {
  advanceAgent,
  getAgent,
  getAgentDecisions,
  getAllAgentEquity,
  startAgent,
  stopAgent,
  type AgentDecision,
  type AgentDetail,
  type AgentEquityPoint,
} from "../lib/api";
import {
  agentActionErrorMessage,
  agentControlAvailability,
  agentStartRange,
  findHaltEvent,
  mergeDecisions,
  performAgentActionAndRefresh,
  strategyDescription,
} from "../lib/agents";
import { fmtDateTime, fmtNum } from "../lib/testlab";
import DecisionLog from "./DecisionLog";
import EquityCharts from "./EquityCharts";
import {
  AgentHaltSummary,
  AgentSampleBadge,
  AgentStatusBadge,
} from "./AgentStatus";

type Action = "start" | "stop" | "advance";

interface DecisionRequest {
  before: string | undefined;
  append: boolean;
}

export default function AgentDetailView({ id }: { id: string }) {
  const [agent, setAgent] = useState<AgentDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(true);
  const [detailError, setDetailError] = useState<string | null>(null);
  const [equity, setEquity] = useState<AgentEquityPoint[]>([]);
  const [equityLoading, setEquityLoading] = useState(true);
  const [equityError, setEquityError] = useState<string | null>(null);
  const [decisions, setDecisions] = useState<AgentDecision[]>([]);
  const [decisionsLoading, setDecisionsLoading] = useState(true);
  const [decisionsError, setDecisionsError] = useState<string | null>(null);
  const [decisionCursor, setDecisionCursor] = useState<string | null>(null);
  const [decisionRequest, setDecisionRequest] = useState<DecisionRequest>({
    before: undefined,
    append: false,
  });
  const [busyAction, setBusyAction] = useState<Action | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  const loadAgent = useCallback(async (): Promise<void> => {
    setDetailLoading(true);
    setDetailError(null);
    try {
      setAgent(await getAgent(id));
    } catch (error) {
      setDetailError(error instanceof Error ? error.message : String(error));
    } finally {
      setDetailLoading(false);
    }
  }, [id]);

  const loadEquity = useCallback(async (): Promise<void> => {
    setEquityLoading(true);
    setEquityError(null);
    try {
      setEquity(await getAllAgentEquity(id));
    } catch (error) {
      setEquityError(error instanceof Error ? error.message : String(error));
    } finally {
      setEquityLoading(false);
    }
  }, [id]);

  const loadDecisions = useCallback(
    async (before: string | undefined, append: boolean): Promise<void> => {
      setDecisionRequest({ before, append });
      setDecisionsLoading(true);
      setDecisionsError(null);
      try {
        const page = await getAgentDecisions(id, { limit: 100, before });
        setDecisions((current) =>
          append ? mergeDecisions(current, page.decisions) : page.decisions,
        );
        setDecisionCursor(page.next_before);
      } catch (error) {
        setDecisionsError(error instanceof Error ? error.message : String(error));
      } finally {
        setDecisionsLoading(false);
      }
    },
    [id],
  );

  const refreshAll = useCallback(async (): Promise<void> => {
    setRefreshing(true);
    await Promise.all([
      loadAgent(),
      loadEquity(),
      loadDecisions(undefined, false),
    ]);
    setRefreshing(false);
  }, [loadAgent, loadDecisions, loadEquity]);

  useEffect(() => {
    let cancelled = false;
    queueMicrotask(() => {
      if (!cancelled) void refreshAll();
    });
    return () => {
      cancelled = true;
    };
  }, [refreshAll]);

  async function runAction(action: Action): Promise<void> {
    if (agent === null) return;
    setBusyAction(action);
    setActionError(null);
    try {
      await performAgentActionAndRefresh(
        () => {
          if (action === "start") {
            return startAgent(id, agentStartRange(agent.strategy.timeframe, Date.now()));
          }
          if (action === "stop") return stopAgent(id);
          return advanceAgent(id, { end_ms: Date.now() });
        },
        setAgent,
        refreshAll,
      );
    } catch (error) {
      setActionError(agentActionErrorMessage(error));
    } finally {
      setBusyAction(null);
    }
  }

  if (detailLoading && agent === null) {
    return (
      <div className="flex max-w-6xl flex-col gap-3" aria-label="Loading agent" aria-busy="true">
        <div className="h-8 w-48 animate-pulse rounded bg-[var(--surface)]" />
        <div className="h-28 animate-pulse rounded border border-[var(--border)] bg-[var(--surface)]" />
        <div className="h-64 animate-pulse rounded border border-[var(--border)] bg-[var(--surface)]" />
      </div>
    );
  }

  if (agent === null) {
    return (
      <div className="flex max-w-3xl flex-col gap-3">
        <h1 className="text-xl font-bold">Agent unavailable</h1>
        <p role="alert" className="text-sm text-[var(--verdict-fails)]">
          {detailError ?? "This agent could not be loaded."}
        </p>
        <div className="flex gap-2">
          <button
            type="button"
            onClick={() => void loadAgent()}
            className="min-h-10 rounded border border-[var(--border)] px-3 py-2 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent)]"
          >
            Try again
          </button>
          <Link className="min-h-10 rounded border border-[var(--border)] px-3 py-2" href="/agents">Back to agents</Link>
        </div>
      </div>
    );
  }

  const halted = agent.halt !== null;
  const controls = agentControlAvailability(agent.agent.status, agent.halt, busyAction !== null);
  const haltEvent = findHaltEvent(decisions, agent.halt);
  const decisionRetry = (): void => {
    void loadDecisions(decisionRequest.before, decisionRequest.append);
  };

  return (
    <div className="flex max-w-6xl flex-col gap-6">
      <header className="flex flex-col gap-3">
        <Link href="/agents" className="min-h-10 w-fit py-2 text-sm text-[var(--muted)] underline focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent)]">
          ← All agents
        </Link>
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="flex flex-col gap-1">
            <div className="flex flex-wrap items-center gap-3">
              <h1 className="text-xl font-bold">{agent.agent.name}</h1>
              <AgentSampleBadge isSample={agent.agent.is_sample} />
              <AgentStatusBadge status={agent.agent.status} halt={agent.halt} />
            </div>
            <p className="text-sm text-[var(--muted)]">{strategyDescription(agent.strategy)}</p>
            <p className="text-xs text-[var(--muted)]">
              Last bar {agent.agent.last_ts === null ? "—" : fmtDateTime(agent.agent.last_ts)} · {agent.candle_count.toLocaleString()} bars processed
            </p>
          </div>
          <button
            type="button"
            onClick={() => void refreshAll()}
            disabled={refreshing || busyAction !== null}
            className="min-h-10 rounded border border-[var(--border)] px-3 py-2 text-sm disabled:opacity-60 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent)]"
          >
            {refreshing ? "Refreshing…" : "Refresh"}
          </button>
        </div>
      </header>

      {detailError !== null && (
        <div role="alert" className="flex flex-col items-start gap-2 rounded border border-[var(--verdict-fails)] p-3 text-sm">
          <p>Could not refresh agent status: {detailError}</p>
          <button type="button" onClick={() => void loadAgent()} className="min-h-10 rounded border border-[var(--border)] px-3 py-2">Try again</button>
        </div>
      )}

      {halted && agent.halt !== null && (
        <AgentHaltSummary halt={agent.halt} risk={agent.risk} event={haltEvent} />
      )}

      <section className="flex flex-col gap-3 rounded border border-[var(--border)] bg-[var(--surface)] p-4" aria-label="Agent controls">
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            disabled={!controls.start}
            aria-busy={busyAction === "start"}
            onClick={() => void runAction("start")}
            className="min-h-10 rounded bg-[var(--accent)] px-4 py-2 text-sm font-semibold text-[var(--accent-foreground)] disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent)]"
          >
            {busyAction === "start" ? "Starting…" : "Start"}
          </button>
          <button
            type="button"
            disabled={!controls.stop}
            aria-busy={busyAction === "stop"}
            onClick={() => void runAction("stop")}
            className="min-h-10 rounded border border-[var(--border)] px-4 py-2 text-sm disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent)]"
          >
            {busyAction === "stop" ? "Stopping…" : "Stop"}
          </button>
          <button
            type="button"
            disabled={!controls.advance}
            aria-busy={busyAction === "advance"}
            onClick={() => void runAction("advance")}
            className="min-h-10 rounded border border-[var(--border)] px-4 py-2 text-sm disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent)]"
          >
            {busyAction === "advance" ? "Advancing…" : "Advance"}
          </button>
        </div>
        <p className="text-xs text-[var(--muted)]">
          Start warms up on at most 100,000 closed bars, within one year of history.
        </p>
        {halted && (
          <p className="text-sm text-[var(--verdict-fragile)]">
            {agent.halt === "KillSwitch" ? "Trading is paused by an administrator." : "Risk limits are holding this agent flat."}
          </p>
        )}
        {actionError !== null && (
          <p role="alert" className="rounded border border-[var(--verdict-fails)] p-3 text-sm">{actionError}</p>
        )}
      </section>

      <section className="flex flex-col gap-3" aria-labelledby="equity-heading">
        <div>
          <h2 id="equity-heading" className="text-base font-semibold">Equity history</h2>
          <p className="text-xs text-[var(--muted)]">Last equity <strong>{fmtNum(agent.agent.equity)}</strong></p>
        </div>
        {equityLoading && equity.length === 0 && (
          <div className="grid gap-3 md:grid-cols-2" aria-label="Loading equity history" aria-busy="true">
            <div className="h-56 animate-pulse rounded border border-[var(--border)] bg-[var(--surface)]" />
            <div className="h-56 animate-pulse rounded border border-[var(--border)] bg-[var(--surface)]" />
          </div>
        )}
        {equityError !== null && (
          <div role="alert" className="flex flex-col items-start gap-2 rounded border border-[var(--verdict-fails)] p-3 text-sm">
            <p>Could not load equity history: {equityError}</p>
            <button type="button" onClick={() => void loadEquity()} className="min-h-10 rounded border border-[var(--border)] px-3 py-2">Try again</button>
          </div>
        )}
        {!equityLoading && equityError === null && equity.length === 0 && (
          <p className="rounded border border-[var(--border)] p-4 text-sm text-[var(--muted)]">
            No equity points yet. Start the agent to record its first closed bars.
          </p>
        )}
        {equity.length > 0 && (
          <EquityCharts
            equity={equity.map((point) => point.equity)}
            split={null}
            startMs={equity[0]?.bar_ts ?? null}
            endMs={equity.at(-1)?.bar_ts ?? null}
            mode="agent"
          />
        )}
      </section>

      <DecisionLog
        decisions={decisions}
        loading={decisionsLoading}
        error={decisionsError}
        hasMore={decisionCursor !== null}
        onRetry={decisionRetry}
        onLoadMore={() => void loadDecisions(decisionCursor ?? undefined, true)}
      />
    </div>
  );
}
