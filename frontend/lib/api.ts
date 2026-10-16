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
  | "EndOfData";

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
): Promise<BacktestResponse> {
  return request<BacktestResponse>("/backtest", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

export async function listRuns(): Promise<RunSummary[]> {
  const body = await request<{ runs: RunSummary[] }>("/runs");
  return body.runs;
}

export function getRun(id: number): Promise<SavedRun> {
  return request<SavedRun>(`/runs/${id}`);
}
