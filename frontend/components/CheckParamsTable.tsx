// Context rows and each number in the extracted strategy, with where each
// came from. Stated values are read-only; assumed and missing values are editable.
"use client";

import {
  ASSET_NOT_RECOGNISED,
  ASSET_OPTIONS,
  ASSUMED_TIMEFRAME_NOTE,
  DEFAULT_TIMEFRAME,
  EVERY_TIMEFRAME_NOTE,
  contextLabel,
  normaliseAsset,
  provenanceLabel,
  showAssumedTimeframeSelect,
  timeframeNotStated,
  type CheckContext,
  type CheckMode,
  type CheckParameter,
  type ContextEdits,
} from "@/lib/check";
import { TIMEFRAMES } from "@/lib/testlab";

const inputClass =
  "rounded border border-[var(--border)] bg-[var(--background)] px-2 py-1 text-sm w-36";

const wideInputClass =
  "rounded border border-[var(--border)] bg-[var(--background)] px-2 py-1 text-sm w-56";

function optionsWith(current: string, known: readonly string[]): string[] {
  return known.includes(current) ? [...known] : [current, ...known];
}

export default function CheckParamsTable({
  params,
  overrides,
  onEdit,
  context,
  contextEdits,
  onContextEdit,
  everyTimeframe,
  onEveryTimeframeChange,
  assetText,
  mode,
}: {
  params: CheckParameter[];
  overrides: Record<string, string>;
  onEdit: (path: string, raw: string) => void;
  context: CheckContext;
  contextEdits: ContextEdits;
  onContextEdit: (key: keyof ContextEdits, value: string) => void;
  everyTimeframe: boolean;
  onEveryTimeframeChange: (value: boolean) => void;
  assetText: string;
  mode: CheckMode;
}) {
  const asset = normaliseAsset(assetText);
  const assetValue = asset ?? "";
  const side = contextEdits.side ?? (context.side.value === "short" ? "short" : "long");
  const timeframe = contextEdits.timeframe ?? context.timeframe.value ?? DEFAULT_TIMEFRAME;
  const notStated = timeframeNotStated(context);
  const showTimeframeSelect = showAssumedTimeframeSelect(context, everyTimeframe);

  const contextRows: {
    key: keyof ContextEdits;
    name: string;
    field: CheckContext[keyof CheckContext];
    overridden: boolean;
  }[] = [
    { key: "asset", name: "Asset", field: context.asset, overridden: contextEdits.asset !== undefined },
    {
      key: "timeframe",
      name: "Timeframe",
      field: context.timeframe,
      overridden: contextEdits.timeframe !== undefined,
    },
    { key: "side", name: "Side", field: context.side, overridden: contextEdits.side !== undefined },
  ];

  return (
    <div className="overflow-x-auto">
      <table className="w-full border-collapse text-sm">
        <thead>
          <tr>
            {["Parameter", "Source", "Value"].map((h) => (
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
          {contextRows.map((row) => (
            <tr key={`context-${row.key}`}>
              <td className="border border-[var(--border)] p-2 font-medium">{row.name}</td>
              <td className="border border-[var(--border)] p-2">
                <span className="rounded border border-[var(--border)] px-1.5 py-0.5 text-xs font-medium">
                  {row.key === "asset" && asset === null
                    ? ASSET_NOT_RECOGNISED
                    : contextLabel(row.field.source, row.overridden, mode)}
                </span>
                <br />
                {row.key === "timeframe" && notStated ? (
                  <span className="text-xs text-[var(--muted)]">
                    {everyTimeframe ? EVERY_TIMEFRAME_NOTE : row.field.reason}
                  </span>
                ) : (
                  <>
                    {row.field.source === "stated" && row.field.quote && (
                      <span className="text-xs text-[var(--muted)]">“{row.field.quote}”</span>
                    )}
                    {row.field.source === "assumed" && row.field.reason && (
                      <span className="text-xs text-[var(--muted)]">{row.field.reason}</span>
                    )}
                  </>
                )}
              </td>
              <td className="border border-[var(--border)] p-2">
                {row.key === "asset" && (
                  <select
                    className={wideInputClass}
                    aria-label="Asset"
                    value={assetValue}
                    onChange={(e) => onContextEdit("asset", e.target.value)}
                  >
                    <option value="" disabled>
                      Pick a coin
                    </option>
                    {ASSET_OPTIONS.map((o) => (
                      <option key={o.value} value={o.value}>
                        {o.label}
                      </option>
                    ))}
                  </select>
                )}
                {row.key === "side" && (
                  <select
                    className={inputClass}
                    aria-label="Side"
                    value={side}
                    onChange={(e) => onContextEdit("side", e.target.value)}
                  >
                    <option value="long">long</option>
                    <option value="short">short</option>
                  </select>
                )}
                {row.key === "timeframe" &&
                  (notStated ? (
                    <div className="flex flex-col gap-1">
                      <span className="text-[var(--muted)]">Not stated in the video</span>
                      {showTimeframeSelect && (
                        <>
                          <select
                            className={inputClass}
                            aria-label="Timeframe"
                            value={timeframe}
                            onChange={(e) => onContextEdit("timeframe", e.target.value)}
                          >
                            {TIMEFRAMES.map((t) => (
                              <option key={t} value={t}>
                                {t}
                              </option>
                            ))}
                          </select>
                          <span className="text-xs text-[var(--muted)]">
                            {ASSUMED_TIMEFRAME_NOTE}
                          </span>
                        </>
                      )}
                      <label className="flex items-center gap-2 text-sm">
                        <input
                          type="checkbox"
                          checked={everyTimeframe}
                          onChange={(e) => onEveryTimeframeChange(e.target.checked)}
                        />
                        Test on every timeframe
                      </label>
                    </div>
                  ) : (
                    <select
                      className={inputClass}
                      aria-label="Timeframe"
                      value={timeframe}
                      onChange={(e) => onContextEdit("timeframe", e.target.value)}
                    >
                      {optionsWith(timeframe, TIMEFRAMES).map((t) => (
                        <option key={t} value={t}>
                          {t}
                        </option>
                      ))}
                    </select>
                  ))}
              </td>
            </tr>
          ))}
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
                    {provenanceLabel(p, overridden, mode)}
                  </span>
                  <br />
                  {p.source === "stated" && p.quote && (
                    <span className="text-xs text-[var(--muted)]">“{p.quote}”</span>
                  )}
                  {p.source === "assumed" && p.reason && (
                    <span className="text-xs text-[var(--muted)]">{p.reason}</span>
                  )}
                  {p.source === "missing" && !overridden && (
                    <span className="text-xs text-[var(--muted)]">
                      {mode === "idea" ? "Not in your idea." : "Not in the video."}{" "}
                      Type a number to test.
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
