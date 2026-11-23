# Replaylab

Replaylab is a strategy research workbench. Describe a rule-based trading idea,
backtest it on historical market candles, and inspect its out-of-sample results.
The same simulator powers paper agents, so strategy decisions and simulated
fills use the same rules across testing and paper runs.

> Historical simulations are not promises of future performance or financial
> advice.

## Screenshot

<!-- Screenshot placeholder: add a current Test Lab or Paper Agents screenshot here. -->

## Run locally

Prerequisites: Rust stable, Node.js 20+, and npm.

1. Configure the engine:

   ```sh
   cp .env.example .env
   set -a
   . ./.env
   set +a
   ```

2. Start the engine from the repository root:

   ```sh
   cargo run -p engine
   ```

   It listens on `http://127.0.0.1:8080` by default. Check
   `http://127.0.0.1:8080/health`.

3. In another terminal, configure and start the frontend:

   ```sh
   cd frontend
   cp .env.example .env.local
   npm ci
   npm run dev
   ```

   Open `http://localhost:3000`. `NEXT_PUBLIC_API_URL` is the browser-visible
   engine URL; set it to a URL the user's browser can reach.

## Deploy

### Engine container

Build and run the engine image:

```sh
docker build -f Dockerfile -t replaylab-engine .
docker volume create replaylab-data
docker run -d --name replaylab-engine --restart unless-stopped \
  -p 8080:8080 \
  -v replaylab-data:/data \
  --env-file .env \
  -e HOST=0.0.0.0 \
  -e CACHE_DIR=/data/cache \
  -e DB_PATH=/data/runs.db \
  replaylab-engine
```

Set `ALLOWED_ORIGINS` to the exact HTTPS origin serving the frontend. Put the
engine behind HTTPS using your hosting provider or reverse proxy, and set
`NEXT_PUBLIC_API_URL` to that public engine URL. The engine exposes `/health`
for container health checks; it reports the version, demo mode, and whether
configured cache prewarming has completed.

The container defaults to `BINANCE_BASE_URL=https://api.binance.com`. If that
host is blocked from your deployment network, set
`BINANCE_BASE_URL=https://data-api.binance.vision`, Binance's market-data-only
endpoint. `PREWARM` can start optional cache fills in the background during
engine startup.

Run **one engine instance only** for a given SQLite database and candle cache.
SQLite is local-file storage and this deployment is not configured for
multi-instance coordination. Keep `/data` on persistent storage.

### Frontend

Deploy `frontend/` to a Node-capable host. Provide the environment values below
to the frontend build/runtime. `NEXT_PUBLIC_API_URL` is inlined into browser
JavaScript at build time and is only a URL, never a credential. Keep
`GEMINI_API_KEY` server-side; it is used by the Check API route and must not
use a `NEXT_PUBLIC_` prefix.

## Environment variables

| Variable | Component | Default | Purpose |
|---|---|---:|---|
| `HOST` | Engine | `127.0.0.1` | Listen IP; use `0.0.0.0` in a container. |
| `PORT` | Engine | `8080` | Listen port. |
| `ALLOWED_ORIGINS` | Engine | `http://localhost:3000` | Comma-separated, explicit browser origins. Wildcards are rejected. |
| `CACHE_DIR` | Engine | `.cache` | Candle cache directory. |
| `DB_PATH` | Engine | `$CACHE_DIR/runs.db` | SQLite database path. |
| `BINANCE_BASE_URL` | Engine | `https://api.binance.com` | Binance market-data endpoint. Use `https://data-api.binance.vision` when the default host is blocked. |
| `BACKTEST_RATE_PER_MIN` | Engine | `30` | Backtest requests per minute per IP. |
| `OPTIMIZE_RATE_PER_MIN` | Engine | `5` | Optimizer requests per minute per IP. |
| `MAX_OPTIMIZER_JOBS` | Engine | `2` | Concurrent optimizer jobs. |
| `BACKTEST_TIMEOUT_SECS` | Engine | `60` | Backtest/agent candle-load timeout; accepted range is 5–600. |
| `ADMIN_TOKEN` | Engine | unset | Enables admin routes when set; send it in `x-admin-token`. Unset means fail closed. |
| `DEMO_MODE` | Engine | `false` | Seeds three deterministic sample paper agents and rejects agent mutations. |
| `PREWARM` | Engine | unset | Optional comma-separated `SYMBOL:interval:years` cache requests, e.g. `BTCUSDT:1h:5`. Runs in the background; each entry is capped at 100,000 candles. |
| `NEXT_PUBLIC_API_URL` | Frontend | `http://127.0.0.1:8080` | Public engine base URL used by browser requests; baked in at build time. |
| `LLM_PROVIDER` | Frontend server | `gemini` | Check-page provider; only Gemini is supported. |
| `GEMINI_API_KEY` | Frontend server | unset | Google AI Studio credential for Check. Keep server-side only. |
| `GEMINI_MODEL` | Frontend server | unset | Gemini model identifier. |
| `CHECK_API_ENABLED` | Frontend server | `false` | Enables the public Check route's paid Gemini calls. Set `true` explicitly, including in demo deployments. |
| `CHECK_DAILY_LIMIT` | Frontend server | `20` | Check-page extractions per IP per day. |

Copy `.env.example` and `frontend/.env.example` for documented local settings.
Do not commit real credentials.

## Demo mode

Set `DEMO_MODE=true` on the engine to seed three example agents from a fixed
synthetic candle fixture. Seeding is idempotent and stores simulated equity
and decision history in SQLite. Agent data is readable, but create, start,
stop, advance, reset, and kill-switch mutations return `403 Demo mode: agents
are read-only`. Backtests, walk-forward runs, templates, and reads remain
available under their normal limits.

The frontend Check route's third-party Gemini call is disabled unless
`CHECK_API_ENABLED=true` is explicitly set on the frontend server.

## Honest limitations

- Replaylab is paper/simulation only; it does not place live orders or hold funds.
- Market candles currently come from Binance only (or a configured compatible
  mirror).
- There are no user accounts or multi-tenant data isolation; SQLite data is
  local to the engine instance.
- Demo agents are deterministic examples on synthetic candles, not live
  strategies or performance claims.
- Backtests can be affected by data quality, fees, slippage, and assumptions.
  A backtest result is not a promise of future profit.

## Results walkthrough

_Reserved for the project owner to complete._
