# Deploy Replaylab

This deploy runs the engine as one Fly Machine with a persistent SQLite/cache
volume. It does not provision a database, Redis, or object storage service.
The frontend is hosted separately on Vercel.

## Ordered deployment

### 1. Authenticate with Fly

**You only — sign in with your Fly.io account.**

```sh
flyctl auth login
```

Choose a globally unique app name and a Fly region near your users:

```sh
export FLY_APP="replaylab-engine-your-unique-suffix"
export FLY_REGION="iad"
```

### 2. Launch without deploying

`fly.toml` has a default app name. `--name` sets your chosen app name in the
Fly configuration.

```sh
flyctl launch \
  --no-deploy \
  --copy-config \
  --name "$FLY_APP" \
  --region "$FLY_REGION" \
  --ha=false \
  --no-db \
  --no-redis \
  --no-object-storage
```

Confirm that the `app` value in `fly.toml` is now `$FLY_APP`.

### 3. Create the persistent volume

```sh
flyctl volumes create replaylab_data \
  --app "$FLY_APP" \
  --region "$FLY_REGION" \
  --size 1 \
  --yes
```

The volume mounts at `/data`; the database is `/data/runs.db` and the candle
cache is `/data/cache`.

### 4. Set the engine's deploy-time values

The engine reads **`ALLOWED_ORIGINS`** for CORS and **`ADMIN_TOKEN`** for admin
routes. Neither value is in `fly.toml`. CORS origins must be explicit; do not
use `*`. Pick the production Vercel origin you intend to use. If Vercel assigns
a different origin, update `ALLOWED_ORIGINS` and redeploy the engine.

**You only — set the origin and, optionally, an admin token.**

```sh
export FRONTEND_ORIGIN="https://your-project.vercel.app"
flyctl secrets set --app "$FLY_APP" "ALLOWED_ORIGINS=$FRONTEND_ORIGIN"
```

Admin routes fail closed when `ADMIN_TOKEN` is unset. If you need them, create
and store the token privately, then set it without putting it in the repo:

```sh
read -r -s -p "ADMIN_TOKEN: " ADMIN_TOKEN
printf '\n'
flyctl secrets set --app "$FLY_APP" "ADMIN_TOKEN=$ADMIN_TOKEN"
unset ADMIN_TOKEN
```

### 5. Deploy the engine

```sh
flyctl deploy --app "$FLY_APP"
```

### 6. Enforce one Machine

```sh
flyctl scale count 1 --app "$FLY_APP" --region "$FLY_REGION" --yes
```

Keep it at one instance: SQLite and the mounted volume are not shared or
replicated across Machines. Auto-stop is disabled so the Machine does not
sleep through cache prewarming or lose in-memory job state.

### 7. Configure Vercel

**You only — configure and redeploy the Vercel project.** In **Project →
Settings → Environment Variables**:

| Variable | Value |
|---|---|
| `NEXT_PUBLIC_API_URL` | `https://<your-FLY_APP>.fly.dev` |
| `CHECK_API_ENABLED` | Leave unset. |
| `GEMINI_API_KEY` | Leave unset unless you choose to enable Check. |
| `GEMINI_MODEL` | Leave unset unless you choose to enable Check. |

`NEXT_PUBLIC_API_URL` is a public URL embedded at build time. The Check route
is disabled unless you explicitly set the server-only `CHECK_API_ENABLED=true`;
enabling it also requires the server-only Gemini key and model. Redeploy Vercel
after changing the public URL.

### 8. Verify both deployments

Install `curl` and Python 3 on the machine running the script, then pass the
engine and frontend origins:

```sh
scripts/verify-deploy.sh \
  "https://<your-FLY_APP>.fly.dev" \
  "https://your-project.vercel.app"
```

The script reports one PASS/FAIL line per check, including a real recent
BTCUSDT 1h backtest. If Binance is unreachable from the Fly Machine, it names
that failure and points to `BINANCE_BASE_URL`.

## Rollback and logs

List release images, select the last known-good image reference, and redeploy
it as a rollback:

```sh
flyctl releases --app "$FLY_APP" --image
flyctl deploy --app "$FLY_APP" \
  --image "registry.fly.io/$FLY_APP:<previous-image-tag>" \
  --rollback
```

Stream logs or show the buffered logs:

```sh
flyctl logs --app "$FLY_APP"
flyctl logs --app "$FLY_APP" --no-tail
```
