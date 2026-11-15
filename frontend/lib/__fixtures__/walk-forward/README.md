# Walk-forward contract captures

Real responses from the engine's `POST /walk-forward`, used by
`lib/walkforward.contract.test.ts`. They are not edited by hand.

Captured on 2026-11-15 against the engine built from this repo (no engine
changes), with `CACHE_DIR` holding the BTCUSDT 1h candles from `.cache/`, so
every request used an explicit date range inside the cache.

| File | Source |
|---|---|
| `request-rolling.json` | Body produced by `buildWalkForwardRequest` for the form in the contract test (SMA-20 crossover, period grid 14/20/26, 3 windows, 300 IS and 150 OOS bars, sharpe). Accepted by the engine with 200. |
| `response-rolling.json` | Engine response to `request-rolling.json`. Efficiency is `null` (in-sample return per bar is negative). |
| `response-anchored.json` | Same request with `"mode": "anchored"`. |
| `response-unstable.json` | Same as rolling but `objective: total_return`. Parameter stability is `unstable: true`. |
| `error-400.json` | `is_bars: 1400` on a 1500-candle range. Status 400, body `{"error": ...}`. |
| `error-429.json` | Sixth request within one minute at the default `OPTIMIZE_RATE_PER_MIN=5`. Status 429 with a `retry-after` header. |

To regenerate: start the engine with `CACHE_DIR` pointing at a copy of the
cached candles and `DB_PATH` outside the repo, then POST the request bodies
above. The engine adds `efficiency_ratio: null` when the in-sample return per
bar is zero or negative, so a numeric efficiency needs a case where the
optimizer finds a profitable in-sample period.
