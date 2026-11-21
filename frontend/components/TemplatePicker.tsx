// Read-only strategy catalog for loading a starting point into the Test Lab.
"use client";

import type { StrategyTemplate } from "@/lib/api";

export default function TemplatePicker({
  templates,
  loading,
  error,
  selectedId,
  onSelect,
  onRetry,
}: {
  templates: StrategyTemplate[];
  loading: boolean;
  error: string | null;
  selectedId: string | null;
  onSelect: (template: StrategyTemplate) => void;
  onRetry: () => void;
}) {
  return (
    <section aria-label="Strategy templates" className="flex flex-col gap-3">
      <div className="flex flex-col gap-1">
        <h2 className="text-base font-semibold">Strategy templates</h2>
        <p className="text-xs text-[var(--muted)]">
          Load an editable starting point into both test modes. Templates are not
          optimized recommendations.
        </p>
      </div>

      {loading && (
        <p role="status" className="text-sm text-[var(--muted)]">
          Loading strategy templates…
        </p>
      )}

      {error !== null && !loading && (
        <div role="alert" className="flex flex-col items-start gap-2 rounded border border-[var(--verdict-fails)] p-3 text-sm">
          <p>Could not load strategy templates: {error}</p>
          <button
            type="button"
            onClick={onRetry}
            className="rounded border border-[var(--border)] px-3 py-1.5 font-medium focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent)]"
          >
            Retry
          </button>
        </div>
      )}

      {!loading && error === null && templates.length === 0 && (
        <p className="text-sm text-[var(--muted)]">No strategy templates are available.</p>
      )}

      {!loading && error === null && templates.length > 0 && (
        <div className="grid gap-3 sm:grid-cols-2">
          {templates.map((template) => {
            const selected = template.id === selectedId;
            return (
              <article
                key={template.id}
                className="flex flex-col items-start gap-2 rounded border border-[var(--border)] bg-[var(--surface)] p-3"
              >
                <div>
                  <h3 className="font-semibold">{template.name}</h3>
                  <p className="text-xs text-[var(--muted)]">
                    Suggested timeframe: {template.recommended_timeframe}
                  </p>
                </div>
                <p className="text-sm">{template.market_idea}</p>
                <p className="text-xs text-[var(--muted)]">
                  Typical failure: {template.typical_failure_mode}
                </p>
                <p className="text-xs text-[var(--muted)]">{template.description}</p>
                <button
                  type="button"
                  aria-pressed={selected}
                  onClick={() => onSelect(template)}
                  className="mt-auto rounded border border-[var(--border)] px-3 py-1.5 text-sm font-medium hover:bg-[var(--background)] focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent)]"
                >
                  {selected ? "Load again" : "Load template"}
                </button>
              </article>
            );
          })}
        </div>
      )}
    </section>
  );
}
