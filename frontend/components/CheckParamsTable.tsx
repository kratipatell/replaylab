// Each number in the extracted strategy with where it came from.
// Stated values are read-only; assumed and missing values are editable.
"use client";

import { provenanceLabel, type CheckParameter } from "@/lib/check";

const inputClass =
  "rounded border border-[var(--border)] bg-[var(--background)] px-2 py-1 text-sm w-36";

export default function CheckParamsTable({
  params,
  overrides,
  onEdit,
}: {
  params: CheckParameter[];
  overrides: Record<string, string>;
  onEdit: (path: string, raw: string) => void;
}) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full border-collapse text-sm">
        <thead>
          <tr>
            {["Number", "Source", "Value"].map((h) => (
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
          {params.map((p) => {
            const raw = overrides[p.path];
            const overridden = raw !== undefined;
            const editable = p.source !== "stated" || overridden;
            return (
              <tr key={p.path}>
                <td className="border border-[var(--border)] p-2">
                  <span className="font-medium">{p.name}</span>
                  <br />
                  <span className="text-xs text-[var(--muted)]">{p.path}</span>
                </td>
                <td className="border border-[var(--border)] p-2">
                  <span className="rounded border border-[var(--border)] px-1.5 py-0.5 text-xs font-medium">
                    {provenanceLabel(p, overridden)}
                  </span>
                  <br />
                  {p.source === "stated" && p.quote && (
                    <span className="text-xs text-[var(--muted)]">
                      “{p.quote}”
                    </span>
                  )}
                  {p.source === "assumed" && p.reason && (
                    <span className="text-xs text-[var(--muted)]">
                      {p.reason}
                    </span>
                  )}
                  {p.source === "missing" && !overridden && (
                    <span className="text-xs text-[var(--muted)]">
                      Not in the video. Type a number to test.
                    </span>
                  )}
                </td>
                <td className="border border-[var(--border)] p-2">
                  {editable ? (
                    <input
                      className={inputClass}
                      type="number"
                      step="any"
                      aria-label={p.name}
                      placeholder={p.value === null ? "e.g. 14" : undefined}
                      value={overridden ? raw : String(p.value ?? "")}
                      onChange={(e) => onEdit(p.path, e.target.value)}
                    />
                  ) : (
                    <span>{p.value === null ? "—" : String(p.value)}</span>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
