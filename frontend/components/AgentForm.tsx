"use client";

import { useState, type FormEvent } from "react";
import type { CreateAgentRequest } from "../lib/api";
import {
  ASSETS,
  TIMEFRAMES,
  defaultForm,
  formToStrategy,
  hasValidEntryRule,
  type Asset,
  type TestLabForm,
  type Timeframe,
} from "../lib/testlab";
import {
  RISK_FIELDS,
  defaultRiskDraft,
  riskLimitsFromDraft,
  validateAgentRisk,
  type RiskDraft,
  type RiskErrors,
} from "../lib/agents";
import RuleBuilder from "./RuleBuilder";

const inputClass =
  "min-h-10 rounded border border-[var(--border)] bg-[var(--background)] px-3 py-2 text-sm focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent)]";
const labelClass = "flex flex-col gap-1.5 text-sm font-medium";

export default function AgentForm({
  creating,
  error,
  disabled = false,
  disabledReason = "Agent creation is disabled until the engine confirms demo mode is off.",
  onCreate,
}: {
  creating: boolean;
  error: string | null;
  disabled?: boolean;
  disabledReason?: string;
  onCreate: (body: CreateAgentRequest) => void;
}) {
  const [name, setName] = useState("");
  const [form, setForm] = useState<TestLabForm>(defaultForm);
  const [riskEnabled, setRiskEnabled] = useState(true);
  const [risk, setRiskDraft] = useState<RiskDraft>(defaultRiskDraft);
  const [attempted, setAttempted] = useState(false);
  const [nameError, setNameError] = useState<string | null>(null);
  const [strategyError, setStrategyError] = useState<string | null>(null);

  const riskErrors: RiskErrors = attempted
    ? validateAgentRisk(riskEnabled, risk)
    : {};

  function submit(event: FormEvent<HTMLFormElement>): void {
    event.preventDefault();
    if (disabled) return;
    setAttempted(true);
    const trimmedName = name.trim();
    const nextNameError = trimmedName === "" ? "Agent name must not be empty." : null;
    const nextRiskErrors = validateAgentRisk(riskEnabled, risk);
    const nextStrategyError = hasValidEntryRule(form)
      ? null
      : "Add at least one valid entry rule.";
    setNameError(nextNameError);
    setStrategyError(nextStrategyError);
    if (nextNameError !== null) {
      document.getElementById("agent-name")?.focus();
      return;
    }
    const firstRiskError = RISK_FIELDS.find(({ key }) => nextRiskErrors[key] !== undefined);
    if (firstRiskError !== undefined) {
      document.getElementById(`risk-${firstRiskError.key}`)?.focus();
      return;
    }
    if (nextStrategyError !== null) return;

    const body: CreateAgentRequest = {
      name: trimmedName,
      strategy: formToStrategy(form),
    };
    if (riskEnabled) body.risk = riskLimitsFromDraft(risk);
    onCreate(body);
  }

  const updateRisk = (key: keyof RiskDraft, value: string): void => {
    setRiskDraft((current) => ({ ...current, [key]: value }));
  };

  return (
    <form
      id="new-agent"
      className="flex flex-col gap-5 rounded border border-[var(--border)] bg-[var(--surface)] p-4"
      onSubmit={submit}
      aria-busy={creating}
    >
      <div>
        <h2 className="text-base font-semibold">New paper agent</h2>
        <p className="mt-1 text-sm text-[var(--muted)]">
          Paper trading uses simulated money only. No live orders are sent.
        </p>
      </div>

      {disabled && (
        <p role="status" className="text-sm text-[var(--muted)]">
          {disabledReason}
        </p>
      )}

      <label className={labelClass} htmlFor="agent-name">
        Name <span className="text-[var(--muted)]">(required)</span>
        <input
          id="agent-name"
          className={inputClass}
          type="text"
          autoComplete="name"
          spellCheck={false}
          value={name}
          disabled={disabled}
          aria-invalid={nameError !== null}
          aria-describedby={nameError === null ? undefined : "agent-name-error"}
          onChange={(event) => {
            setName(event.target.value);
            if (attempted) setNameError(event.target.value.trim() === "" ? "Agent name must not be empty." : null);
          }}
        />
        {nameError !== null && (
          <span id="agent-name-error" className="text-xs text-[var(--verdict-fails)]">
            {nameError}
          </span>
        )}
      </label>

      <fieldset
        disabled={disabled}
        className="flex flex-col gap-4"
        aria-invalid={strategyError !== null}
        aria-describedby={strategyError === null ? undefined : "agent-strategy-error"}
      >
        <legend className="mb-2 text-sm font-semibold">Strategy</legend>
        <div className="flex flex-wrap gap-3">
          <label className={labelClass}>
            Symbol
            <select
              className={inputClass}
              value={form.asset}
              onChange={(event) =>
                setForm({ ...form, asset: event.target.value as Asset })
              }
            >
              {ASSETS.map((asset) => (
                <option key={asset} value={asset}>{asset}</option>
              ))}
            </select>
          </label>
          <label className={labelClass}>
            Timeframe
            <select
              className={inputClass}
              value={form.timeframe}
              onChange={(event) =>
                setForm({ ...form, timeframe: event.target.value as Timeframe })
              }
            >
              {TIMEFRAMES.map((timeframe) => (
                <option key={timeframe} value={timeframe}>{timeframe}</option>
              ))}
            </select>
          </label>
          <label className={labelClass}>
            Direction
            <select
              className={inputClass}
              value={form.side}
              onChange={(event) =>
                setForm({ ...form, side: event.target.value as TestLabForm["side"] })
              }
            >
              <option value="long">Long</option>
              <option value="short">Short</option>
            </select>
          </label>
        </div>
        <RuleBuilder
          title="Entry rules"
          hint="Checked on each closed bar; entries fill at the next bar's open."
          group={form.entry}
          onChange={(entry) => {
            const next = { ...form, entry };
            setForm(next);
            if (attempted) {
              setStrategyError(
                hasValidEntryRule(next) ? null : "Add at least one valid entry rule.",
              );
            }
          }}
        />
        {strategyError !== null && (
          <p id="agent-strategy-error" role="alert" className="text-xs text-[var(--verdict-fails)]">
            {strategyError}
          </p>
        )}
        <RuleBuilder
          title="Exit rules"
          hint="Optional. Leave empty to hold until a stop, target, or risk halt."
          group={form.exit}
          onChange={(exit) => setForm({ ...form, exit })}
        />
        <div className="flex flex-wrap gap-3">
          {([
            ["stopLoss", "Stop loss (%)"],
            ["takeProfit", "Take profit (%)"],
          ] as const).map(([key, label]) => (
            <label className={labelClass} key={key}>
              {label} (optional)
              <input
                className={`${inputClass} w-40`}
                type="text"
                inputMode="decimal"
                value={form[key]}
                onChange={(event) => setForm({ ...form, [key]: event.target.value })}
              />
            </label>
          ))}
        </div>
      </fieldset>

      <fieldset disabled={disabled} className="flex flex-col gap-3 rounded border border-[var(--border)] p-3">
        <legend className="px-1 text-sm font-semibold">Risk limits</legend>
        <label className="flex min-h-10 items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={riskEnabled}
            onChange={(event) => setRiskEnabled(event.target.checked)}
          />
          Enforce risk limits
        </label>
        {riskEnabled ? (
          <div className="grid gap-3 sm:grid-cols-3">
            {RISK_FIELDS.map(({ key, label }) => {
              const fieldError = riskErrors[key];
              const errorId = `risk-${key}-error`;
              return (
                <label className={labelClass} key={key}>
                  {label}
                  <input
                    id={`risk-${key}`}
                    className={inputClass}
                    type="text"
                    inputMode="decimal"
                    autoComplete="off"
                    value={risk[key]}
                    aria-invalid={fieldError !== undefined}
                    aria-describedby={fieldError === undefined ? undefined : errorId}
                    onChange={(event) => updateRisk(key, event.target.value)}
                  />
                  {fieldError !== undefined && (
                    <span id={errorId} className="text-xs text-[var(--verdict-fails)]">
                      {fieldError}
                    </span>
                  )}
                </label>
              );
            })}
          </div>
        ) : (
          <p className="text-xs text-[var(--muted)]">
            The engine will apply its default caps: 100% position, 10% daily loss, 20% drawdown.
          </p>
        )}
      </fieldset>

      {error !== null && (
        <p role="alert" className="rounded border border-[var(--verdict-fails)] p-3 text-sm">
          {error}
        </p>
      )}

      <button
        type="submit"
        disabled={disabled || creating}
        className="min-h-10 w-fit rounded bg-[var(--accent)] px-4 py-2 text-sm font-semibold text-[var(--accent-foreground)] disabled:cursor-not-allowed disabled:opacity-60 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent)]"
      >
        {creating ? "Creating agent…" : "Create agent"}
      </button>
    </form>
  );
}
