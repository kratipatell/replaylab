# Replaylab

Replaylab is a paid platform where traders test strategies on historical data
(crypto first, later forex, metals, and stocks), get an honest verdict, and
check strategies claimed in videos and reels. Later milestones add paper
agents and live trading on the user's own exchange account. The platform
never holds user funds.

## Run locally

1. Install prerequisites: Rust (stable) and Node.js 20+.
2. Configure the engine: `cp .env.example .env`, adjust if needed, then
   `set -a; source .env; set +a`.
3. Start the engine: `cargo run` (listens on `http://127.0.0.1:8080`).
4. Configure the frontend: `cd frontend && cp .env.example .env.local` and
   set `GEMINI_API_KEY` and `GEMINI_MODEL` for the Check page.
5. Install frontend dependencies: `npm ci`.
6. Start the app: `npm run dev`, then open `http://localhost:3000`.

## Architecture

- The Rust engine (axum) fetches Binance candles, runs backtests and sweeps,
  computes stats and verdicts, and stores saved runs in SQLite.
- The Next.js app has Home, Test Lab, and Check pages that call the engine
  over HTTP.
- Candle data is cached on disk under `CACHE_DIR`; runs live in the SQLite
  database at `DB_PATH`.
- The LLM reader behind the Check page lives only in the Next.js server; the
  Rust engine never calls an LLM and treats LLM output as untrusted input.
- Backtests, paper agents, and live trading all share one simulator.

## Rules

- No lookahead: decisions use a bar's close; fills happen at the next open.
- Fees and slippage are always on.
- Every result reports an out-of-sample figure and its sample size.
- Exchange keys must be trade-only; the platform never holds funds.
- Results are never presented as promises of profit.

## Honest note

Results are simulated on historical data and are not a promise of future
performance. Nothing in Replaylab is financial advice.
