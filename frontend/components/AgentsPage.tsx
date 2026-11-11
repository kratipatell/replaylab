"use client";

import { useCallback, useEffect, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import AgentForm from "./AgentForm";
import { AgentStatusBadge } from "./AgentStatus";
import { createAgent, getAgent, listAgents, type AgentSummary, type HaltLimit, type Strategy } from "../lib/api";
import { strategyDescription } from "../lib/agents";
import { fmtNum } from "../lib/testlab";

interface AgentListRow {
  summary: AgentSummary;
  strategy: Strategy | null;
  halt: HaltLimit | null;
  detailError: string | null;
}

export default function AgentsPage() {
  const router = useRouter();
  const [rows, setRows] = useState<AgentListRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  const refresh = useCallback(async (): Promise<void> => {
    setLoading(true);
    setLoadError(null);
    try {
      const summaries = await listAgents();
      const loaded = await Promise.all(
        summaries.map(async (summary): Promise<AgentListRow> => {
          try {
            const detail = await getAgent(summary.id);
            return {
              summary,
              strategy: detail.strategy,
              halt: detail.halt,
              detailError: null,
            };
          } catch (error) {
            return {
              summary,
              strategy: null,
              halt: null,
              detailError: error instanceof Error ? error.message : String(error),
            };
          }
        }),
      );
      setRows(loaded);
    } catch (error) {
      setLoadError(error instanceof Error ? error.message : String(error));
      setRows([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    queueMicrotask(() => {
      if (!cancelled) void refresh();
    });
    return () => {
      cancelled = true;
    };
  }, [refresh]);

  async function handleCreate(body: Parameters<typeof createAgent>[0]): Promise<void> {
    setCreating(true);
    setCreateError(null);
    try {
      const created = await createAgent(body);
      router.push(`/agents/${encodeURIComponent(created.agent.id)}`);
    } catch (error) {
      setCreateError(error instanceof Error ? error.message : String(error));
    } finally {
      setCreating(false);
    }
  }

  return (
    <div className="flex max-w-6xl flex-col gap-6">
      <header className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex flex-col gap-1">
          <h1 className="text-xl font-bold">Paper agents</h1>
          <p className="text-sm text-[var(--muted)]">Test strategies forward on closed bars with simulated money.</p>
        </div>
        <button
          type="button"
          onClick={() => void refresh()}
          disabled={loading}
          className="min-h-10 rounded border border-[var(--border)] px-3 py-2 text-sm disabled:opacity-60 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent)]"
        >
          {loading ? "Refreshing…" : "Refresh list"}
        </button>
      </header>

      <AgentForm creating={creating} error={createError} onCreate={(body) => void handleCreate(body)} />

      <section className="flex flex-col gap-3" aria-labelledby="agent-list-heading">
        <h2 id="agent-list-heading" className="text-base font-semibold">Your agents</h2>
        {loading && rows.length === 0 && (
          <div className="flex flex-col gap-2" aria-label="Loading agents" aria-busy="true">
            {[0, 1, 2].map((item) => (
              <div key={item} className="h-24 animate-pulse rounded border border-[var(--border)] bg-[var(--surface)]" />
            ))}
          </div>
        )}
        {loadError !== null && (
          <div role="alert" className="flex flex-col items-start gap-2 rounded border border-[var(--verdict-fails)] p-3 text-sm">
            <p>Could not load agents: {loadError}</p>
            <button
              type="button"
              onClick={() => void refresh()}
              className="min-h-10 rounded border border-[var(--border)] px-3 py-2 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent)]"
            >
              Try again
            </button>
          </div>
        )}
        {!loading && loadError === null && rows.length === 0 && (
          <p className="rounded border border-[var(--border)] p-4 text-sm text-[var(--muted)]">
            No agents yet. Create one above to start testing a strategy.
          </p>
        )}
        {rows.length > 0 && (
          <ul className="flex flex-col gap-2">
            {rows.map(({ summary, strategy, halt, detailError }) => (
              <li key={summary.id}>
                <Link
                  href={`/agents/${encodeURIComponent(summary.id)}`}
                  className="flex min-h-24 flex-col gap-3 rounded border border-[var(--border)] bg-[var(--surface)] p-4 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent)] sm:flex-row sm:items-center sm:justify-between"
                >
                  <div className="flex min-w-0 flex-col gap-1">
                    <span className="font-semibold">{summary.name}</span>
                    <span className="truncate text-sm text-[var(--muted)]">
                      {strategy === null
                        ? `${summary.symbol} · ${summary.interval}`
                        : strategyDescription(strategy)}
                    </span>
                    {detailError !== null && (
                      <span className="text-xs text-[var(--verdict-fails)]">Strategy details unavailable.</span>
                    )}
                  </div>
                  <div className="flex flex-wrap items-center gap-3">
                    <AgentStatusBadge status={summary.status} halt={halt} />
                    <span className="text-sm">Equity <strong>{fmtNum(summary.equity)}</strong></span>
                  </div>
                </Link>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
