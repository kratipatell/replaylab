import { agents } from "@/lib/agents";

export default function AgentsCard() {
  return (
    <section className="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-4">
      <div className="flex items-start justify-between gap-3">
        <div className="flex items-center gap-2">
          <h2 className="text-base font-semibold">Your agents</h2>
          <span className="rounded border border-[var(--border)] px-1.5 py-0.5 text-[10px] font-semibold tracking-widest text-[var(--muted)]">
            PAPER
          </span>
        </div>
        <button
          type="button"
          disabled
          title="Coming soon"
          className="rounded bg-[var(--accent)] px-3 py-1.5 text-sm font-medium text-[var(--accent-foreground)] disabled:cursor-not-allowed disabled:opacity-50"
        >
          Start paper trading
        </button>
      </div>
      <p className="mt-2 text-sm text-[var(--muted)]">Simulated money only.</p>
      {agents.length === 0 ? (
        <p className="mt-4 text-sm text-[var(--muted)]">No agents yet.</p>
      ) : (
        <ul className="mt-4 flex flex-col gap-2">
          {agents.map((agent) => (
            <li
              key={agent.id}
              className="rounded border border-[var(--border)] px-3 py-2 text-sm"
            >
              {agent.name}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
