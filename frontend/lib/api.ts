// Typed client for the engine HTTP API. Shapes mirror engine/src/api.rs,
// engine/src/strategy.rs, engine/src/stats.rs, engine/src/verdict.rs,
// engine/src/backtest.rs and engine/src/db.rs.

const BASE_URL =
  process.env.NEXT_PUBLIC_API_URL ?? "http://127.0.0.1:8080";

export type Side = "long" | "short";

export type IndicatorKind =
  | "close"
  | "open"
  | "high"
  | "low"
  | "sma"
  | "ema"
  | "rsi";

export type Op = "<" | ">" | "crosses_above" | "crosses_below";

export interface IndicatorRef {
  ind: IndicatorKind;
  period?: number;
}

export type Operand = number | IndicatorRef;

export interface Condition {
  left: Operand;
  op: Op;
  right: Operand;
}

export interface Rules {
  all: Condition[];
  any: Condition[];
}

export interface Strategy {
  asset: string;
  timeframe: string;
  side: Side;
  entry: Rules;
  exit?: Rules | null;
  stop_loss_pct?: number;
  take_profit_pct?: number;
}

export interface BacktestRequest {
  strategy: Strategy;
  start_ms: number;
  end_ms: number;
  fee_bps?: number;
  slippage_bps?: number;
  split?: number;
}

export interface SuggestedTemplateParameter {
  target: string;
  kind: "int" | "float";
  values: number[];
}

export interface StrategyTemplate {
  id: string;
  name: string;
  description: string;
  market_idea: string;
  typical_failure_mode: string;
  recommended_timeframe: string;
  suggested_walk_forward_parameter_grid: SuggestedTemplateParameter[];
  strategy: Strategy;
}

export interface Stats {
  trades: number;
  wins: number;
  losses: number;
  win_rate_pct: number;
  avg_win_pct: number;
  avg_loss_pct: number;
  profit_factor: number | null;
  expectancy_pct: number;
  expectancy_stdev_pct: number;
  total_return_pct: number;
  buy_and_hold_pct: number;
  max_drawdown_pct: number;
  sharpe: number;
}

export type Grade = "holds" | "fragile" | "fails" | "not_enough_data";

export interface Verdict {
  grade: Grade;
  headline: string;
  reasons: string[];
  oos_trades: number;
}

export type ExitReason =
  | "Signal"
  | "StopLoss"
  | "TakeProfit"
  | "Trailing"
  | "SessionEnd"
  | "EndOfData"
  | "RiskHalt"
  | "Stopped";

export interface Trade {
  entry_idx: number;
  exit_idx: number;
  entry_ts: number;
  exit_ts: number;
  side: Side;
  entry_price: number;
  exit_price: number;
  return_pct: number;
  reason: ExitReason;
}

export interface DrawdownPeriod {
  start_idx: number;
  trough_idx: number;
  end_idx: number | null;
  depth_pct: number;
  length_bars: number;
}

export interface MetricsV2 {
  sortino: number;
  calmar: number;
  max_win_streak: number;
  max_loss_streak: number;
  drawdown_periods: DrawdownPeriod[];
  monthly: Record<string, number>;
  by_weekday: Record<string, number>;
  /** Missing on saved runs written before performance analytics were added. */
  analytics?: PerformanceAnalytics;
}

export interface UnderwaterPoint {
  bar_ts: number;
  drawdown_pct: number | null;
}

export interface MonthlyReturn {
  year: number;
  month: number;
  return_pct: number | null;
  bars: number;
  partial: boolean;
}

export interface MonthSummary {
  year: number;
  month: number;
  return_pct: number;
}

export interface PerformanceAnalytics {
  underwater_curve: UnderwaterPoint[];
  longest_drawdown_duration_bars: number | null;
  time_to_recover_max_drawdown_bars: number | null;
  monthly_returns: MonthlyReturn[];
  best_month: MonthSummary | null;
  worst_month: MonthSummary | null;
  positive_month_share_pct: number | null;
}

export interface BacktestResponse {
  run_id: number | null;
  candle_count: number;
  gaps: [number, number][];
  stats: Stats;
  metrics: MetricsV2;
  in_sample: Stats | null;
  out_of_sample: Stats | null;
  verdict: Verdict;
  trades: Trade[];
  equity_curve: number[];
}

export interface MarketCheck {
  symbol: string;
  candle_count: number;
  stats: Stats;
  in_sample: Stats | null;
  out_of_sample: Stats | null;
  verdict: Verdict;
}

export interface UncheckedMarket {
  symbol: string;
  reason: string;
}

export interface CrossMarket {
  grade: Grade;
  headline: string;
  reasons: string[];
  holds: number;
  tested: number;
  markets: MarketCheck[];
  unchecked: UncheckedMarket[];
}

export interface ValidateRequest extends BacktestRequest {
  symbols?: string[];
}

// Mirrors engine/src/walk_forward.rs (WalkForwardRequest, WalkForwardReport)
// and engine/src/optimizer.rs (ParamSpec, SearchSpec, Objective).
export type WalkForwardMode = "rolling" | "anchored";

export type Objective = "sharpe" | "total_return" | "expectancy" | "profit_factor";

export interface ParamSpec {
  target: string;
  kind: "int" | "float";
  values: number[];
}

export interface WalkForwardRequest {
  strategy: Strategy;
  start_ms: number;
  end_ms: number;
  fee_bps?: number;
  slippage_bps?: number;
  windows: number;
  is_bars: number;
  oos_bars: number;
  mode: WalkForwardMode;
  params: ParamSpec[];
  search: { method: "grid" };
  objective?: Objective;
}

export interface SegmentRange {
  start_ms: number;
  end_ms: number;
  bars: number;
}

export interface SegmentResult {
  range: SegmentRange;
  return_pct: number;
  drawdown_pct: number;
  trade_count: number;
  too_few_trades_to_judge: boolean;
}

export interface WalkForwardWindow {
  window: number;
  in_sample: SegmentResult;
  out_of_sample: SegmentResult;
  chosen_parameters: Record<string, number>;
  optimizer_score: number;
  oos_starting_equity: number;
  oos_ending_equity: number;
}

export interface WalkForwardEquityPoint {
  bar_ts: number;
  equity: number;
}

export interface ParameterStability {
  parameter: string;
  modal_value: number;
  modal_windows: number;
  windows: number;
  modal_share_pct: number;
  unstable: boolean;
}

export interface WalkForwardReport {
  mode: WalkForwardMode;
  objective: string;
  window_count: number;
  total_optimizer_evaluations: number;
  windows: WalkForwardWindow[];
  stitched_oos_equity_curve: WalkForwardEquityPoint[];
  analytics: PerformanceAnalytics;
  out_of_sample_stats: Stats;
  total_oos_return_pct: number;
  max_oos_drawdown_pct: number;
  profitable_oos_windows: number;
  profitable_oos_share_pct: number;
  mean_is_return_per_bar_pct: number;
  mean_oos_return_per_bar_pct: number;
  efficiency_ratio: number | null;
  efficiency_ratio_definition: string;
  parameter_stability: ParameterStability[];
  stability_measure: string;
  verdict: Verdict;
}

export interface RunSummary {
  id: number;
  created_at: number;
  symbol: string;
  interval: string;
  start_ms: number;
  end_ms: number;
  trades: number;
  total_return_pct: number;
  sharpe: number;
  max_drawdown_pct: number;
}

export interface SavedRun {
  id: number;
  created_at: number;
  symbol: string;
  interval: string;
  start_ms: number;
  end_ms: number;
  strategy: Strategy;
  stats: Stats;
  metrics: MetricsV2;
  equity_curve: number[];
  trades: Trade[];
}

export type AgentStatus = "draft" | "running" | "stopped";
export type HaltLimit = "DailyLoss" | "Drawdown" | "KillSwitch";

export interface RiskLimits {
  max_position_pct: number;
  max_daily_loss_pct: number;
  max_drawdown_pct: number;
}

export interface AgentSummary {
  id: string;
  name: string;
  status: AgentStatus;
  symbol: string;
  interval: string;
  run_id: number | null;
  created_at: number;
  started_at: number | null;
  stopped_at: number | null;
  equity: number;
  last_price: number | null;
  last_ts: number | null;
  last_error: string | null;
  is_sample: boolean;
}

export interface AgentPosition {
  side: Side;
  entry_ts: number;
  entry_price: number;
  qty: number;
  stop: number | null;
  target: number | null;
  trailing: number | null;
}

export interface AgentDetail {
  agent: AgentSummary;
  strategy: Strategy;
  initial_equity: number;
  risk: RiskLimits;
  halt: HaltLimit | null;
  candle_count: number;
  position: AgentPosition | null;
  trades: Trade[];
  equity_curve: number[];
}

export interface CreateAgentRequest {
  name: string;
  strategy: Strategy;
  initial_equity?: number;
  risk?: RiskLimits;
}

export interface StartAgentRequest {
  start_ms?: number;
  end_ms?: number;
}

export interface AdvanceAgentRequest {
  end_ms?: number;
}

export interface AgentDecision {
  bar_ts: number;
  kind: string;
  seq: number;
  reason: string | null;
  price: number | null;
  qty: number | null;
  requested_qty: number | null;
  configured: number | null;
  observed: number | null;
  equity: number;
}

export interface AgentEquityPoint {
  bar_ts: number;
  equity: number;
}

export interface AgentDecisionPage {
  decisions: AgentDecision[];
  next_before: string | null;
}

export interface AgentEquityPage {
  equity: AgentEquityPoint[];
  next_since: number | null;
}

interface ErrorBody {
  error?: string;
}

// Carries the HTTP status so callers can tell a rate limit from a bad request.
export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

async function readError(res: Response): Promise<string> {
  try {
    const body = (await res.json()) as ErrorBody;
    if (typeof body.error === "string" && body.error.length > 0) {
      return body.error;
    }
  } catch {
    // Fall through to the status-based message below.
  }
  return `Request failed with status ${res.status}`;
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  let res: Response;
  try {
    res = await fetch(`${BASE_URL}${path}`, init);
  } catch (e) {
    // An aborted request is ours to explain, not a network failure.
    if (init?.signal?.aborted === true) {
      throw e;
    }
    const detail = e instanceof Error ? e.message : String(e);
    throw new Error(
      `Could not reach the test engine at ${BASE_URL}. Is it running? (${detail})`,
    );
  }
  if (!res.ok) {
    throw new ApiError(res.status, await readError(res));
  }
  return (await res.json()) as T;
}

export function postBacktest(
  body: BacktestRequest,
  signal?: AbortSignal,
): Promise<BacktestResponse> {
  return request<BacktestResponse>("/backtest", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
    signal,
  });
}

export async function listTemplates(): Promise<StrategyTemplate[]> {
  const body = await request<{ templates: StrategyTemplate[] }>("/templates");
  return body.templates;
}

export function getTemplate(id: string): Promise<StrategyTemplate> {
  return request<StrategyTemplate>(`/templates/${encodeURIComponent(id)}`);
}

export function postValidate(
  body: ValidateRequest,
  signal?: AbortSignal,
): Promise<CrossMarket> {
  return request<CrossMarket>("/validate", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
    signal,
  });
}

export function postWalkForward(
  body: WalkForwardRequest,
  signal?: AbortSignal,
): Promise<WalkForwardReport> {
  return request<WalkForwardReport>("/walk-forward", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
    signal,
  });
}

export async function listRuns(): Promise<RunSummary[]> {
  const body = await request<{ runs: RunSummary[] }>("/runs");
  return body.runs;
}

export function getRun(id: number): Promise<SavedRun> {
  return request<SavedRun>(`/runs/${id}`);
}

export async function listAgents(): Promise<AgentSummary[]> {
  const body = await request<{ agents: AgentSummary[] }>("/agents");
  return body.agents;
}

export function createAgent(body: CreateAgentRequest): Promise<AgentDetail> {
  return request<AgentDetail>("/agents", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

export function getAgent(id: string): Promise<AgentDetail> {
  return request<AgentDetail>(`/agents/${encodeURIComponent(id)}`);
}

export function startAgent(
  id: string,
  body: StartAgentRequest,
): Promise<AgentDetail> {
  return request<AgentDetail>(`/agents/${encodeURIComponent(id)}/start`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

export function stopAgent(id: string): Promise<AgentDetail> {
  return request<AgentDetail>(`/agents/${encodeURIComponent(id)}/stop`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: "{}",
  });
}

export function advanceAgent(
  id: string,
  body: AdvanceAgentRequest = {},
): Promise<AgentDetail> {
  return request<AgentDetail>(`/agents/${encodeURIComponent(id)}/advance`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

export function getAgentDecisions(
  id: string,
  options: { limit?: number; before?: string } = {},
): Promise<AgentDecisionPage> {
  const query = new URLSearchParams();
  if (options.limit !== undefined) query.set("limit", String(options.limit));
  if (options.before !== undefined) query.set("before", options.before);
  const suffix = query.size === 0 ? "" : `?${query.toString()}`;
  return request<AgentDecisionPage>(
    `/agents/${encodeURIComponent(id)}/decisions${suffix}`,
  );
}

export function getAgentEquity(
  id: string,
  options: { limit?: number; since?: number } = {},
): Promise<AgentEquityPage> {
  const query = new URLSearchParams();
  if (options.limit !== undefined) query.set("limit", String(options.limit));
  if (options.since !== undefined) query.set("since", String(options.since));
  const suffix = query.size === 0 ? "" : `?${query.toString()}`;
  return request<AgentEquityPage>(
    `/agents/${encodeURIComponent(id)}/equity${suffix}`,
  );
}

export async function getAllAgentEquity(id: string): Promise<AgentEquityPoint[]> {
  const points: AgentEquityPoint[] = [];
  let since: number | undefined;
  for (let pageCount = 0; pageCount < 5; pageCount += 1) {
    const page = await getAgentEquity(id, { limit: 5_000, since });
    points.push(...page.equity);
    if (page.next_since === null || page.equity.length === 0) break;
    since = page.next_since;
  }
  return points;
}
