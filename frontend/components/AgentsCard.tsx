import Link from "next/link";

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
        <Link
          href="/agents"
          className="min-h-10 rounded bg-[var(--accent)] px-3 py-2 text-sm font-medium text-[var(--accent-foreground)] focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent)]"
        >
          Manage agents
        </Link>
      </div>
      <p className="mt-2 text-sm text-[var(--muted)]">Simulated money only.</p>
      <p className="mt-4 text-sm text-[var(--muted)]">
        Create, start, and inspect paper agents from the agent workspace.
      </p>
    </section>
  );
}
