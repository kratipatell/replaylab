#!/usr/bin/env bash
set -uo pipefail

if [[ $# -ne 2 ]]; then
  printf 'Usage: %s ENGINE_URL FRONTEND_URL\n' "$0" >&2
  exit 2
fi

engine_url="${1%/}"
frontend_url="${2%/}"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

failures=0
http_status=""

request() {
  local output="$1"
  local timeout="$2"
  shift 2
  http_status="$(
    curl --silent --connect-timeout 5 --max-time "$timeout" \
      --output "$output" --write-out '%{http_code}' "$@" 2>/dev/null
  )"
  local curl_status=$?
  if [[ $curl_status -ne 0 ]]; then
    http_status="000"
  fi
}

report() {
  local result="$1"
  local message="$2"
  printf '%s %s\n' "$result" "$message"
  if [[ "$result" == "FAIL" ]]; then
    failures=$((failures + 1))
  fi
}

request "$tmp_dir/health.json" 15 "$engine_url/health"
if [[ "$http_status" == "200" ]] && python3 - "$tmp_dir/health.json" >/dev/null 2>&1 <<'PY'
import json
import sys
from pathlib import Path

health = json.loads(Path(sys.argv[1]).read_text())
assert health.get("status") == "ok"
assert health.get("demo_mode") is True
PY
then
  report PASS "engine health is ok and demo mode is enabled"
else
  report FAIL "engine health is unavailable or demo mode is not enabled"
fi

request "$tmp_dir/agent-create.json" 15 \
  -X POST -H 'Content-Type: application/json' --data '{}' \
  "$engine_url/agents"
if [[ "$http_status" == "403" ]]; then
  report PASS "demo mode rejects agent creation with 403"
else
  report FAIL "agent creation returned HTTP ${http_status:-000}, expected 403"
fi

python3 - "$tmp_dir/backtest-request.json" >/dev/null 2>&1 <<'PY'
import json
import sys
import time
from pathlib import Path

step = 60 * 60 * 1000
end_ms = int(time.time() * 1000) // step * step
request = {
    "strategy": {
        "asset": "BTCUSDT",
        "timeframe": "1h",
        "side": "long",
        "entry": {
            "all": [{
                "left": {"ind": "close"},
                "op": ">",
                "right": {"ind": "sma", "period": 5},
            }],
            "any": [],
        },
        "exit": {
            "all": [{
                "left": {"ind": "close"},
                "op": "crosses_below",
                "right": {"ind": "sma", "period": 5},
            }],
            "any": [],
        },
    },
    "start_ms": end_ms - 72 * step,
    "end_ms": end_ms,
    "fee_bps": 5,
    "slippage_bps": 2,
    "split": 0.7,
}
Path(sys.argv[1]).write_text(json.dumps(request))
PY
request "$tmp_dir/backtest.json" 75 \
  -H 'Content-Type: application/json' \
  --data-binary "@$tmp_dir/backtest-request.json" \
  "$engine_url/backtest"
if [[ "$http_status" == "200" ]] && python3 - "$tmp_dir/backtest.json" >/dev/null 2>&1 <<'PY'
import json
import sys
from pathlib import Path

result = json.loads(Path(sys.argv[1]).read_text())
assert result.get("candle_count", 0) > 0
assert isinstance(result.get("stats"), dict)
assert isinstance(result.get("out_of_sample"), dict)
PY
then
  report PASS "real BTCUSDT 1h backtest completed"
elif grep -Eqi 'market data host|BINANCE_BASE_URL|request failed' "$tmp_dir/backtest.json" 2>/dev/null; then
  report FAIL "BTC backtest: Binance is unreachable from the engine host; check BINANCE_BASE_URL"
elif [[ "$http_status" == "000" || "$http_status" == "408" ]]; then
  report FAIL "BTC backtest timed out; check Binance connectivity and BINANCE_BASE_URL"
else
  report FAIL "BTC backtest returned HTTP ${http_status:-000}"
fi

request "$tmp_dir/templates.json" 15 "$engine_url/templates"
if [[ "$http_status" == "200" ]] && python3 - "$tmp_dir/templates.json" >/dev/null 2>&1 <<'PY'
import json
import sys
from pathlib import Path

templates = json.loads(Path(sys.argv[1]).read_text()).get("templates")
assert isinstance(templates, list) and len(templates) == 6
PY
then
  report PASS "templates endpoint returns six templates"
else
  report FAIL "templates endpoint did not return six templates (HTTP ${http_status:-000})"
fi

unknown_origin="https://replaylab-origin-check.invalid"
request "$tmp_dir/cors-allowed.body" 15 \
  -D "$tmp_dir/cors-allowed.headers" \
  -X OPTIONS \
  -H "Origin: $frontend_url" \
  -H 'Access-Control-Request-Method: GET' \
  "$engine_url/health"
if [[ "$http_status" =~ ^2[0-9][0-9]$ ]] && python3 - "$tmp_dir/cors-allowed.headers" "$frontend_url" >/dev/null 2>&1 <<'PY'
import sys
from pathlib import Path

headers = Path(sys.argv[1]).read_text().splitlines()
allowed = [
    line.split(":", 1)[1].strip()
    for line in headers
    if line.lower().startswith("access-control-allow-origin:")
]
assert sys.argv[2] in allowed
PY
then
  report PASS "CORS allows the configured frontend origin"
else
  report FAIL "CORS did not allow the configured frontend origin"
fi

request "$tmp_dir/cors-unknown.body" 15 \
  -D "$tmp_dir/cors-unknown.headers" \
  -X OPTIONS \
  -H "Origin: $unknown_origin" \
  -H 'Access-Control-Request-Method: GET' \
  "$engine_url/health"
if [[ "$http_status" =~ ^2[0-9][0-9]$ ]] && python3 - "$tmp_dir/cors-unknown.headers" "$unknown_origin" >/dev/null 2>&1 <<'PY'
import sys
from pathlib import Path

headers = Path(sys.argv[1]).read_text().splitlines()
allowed = [
    line.split(":", 1)[1].strip()
    for line in headers
    if line.lower().startswith("access-control-allow-origin:")
]
assert sys.argv[2] not in allowed
PY
then
  report PASS "CORS rejects an unknown origin"
else
  report FAIL "CORS unexpectedly allowed an unknown origin"
fi

request "$tmp_dir/frontend.html" 25 "$frontend_url/test-lab"
frontend_ok=0
if [[ "$http_status" == "200" ]] && grep -q 'Test Lab' "$tmp_dir/frontend.html"; then
  if ! python3 - "$tmp_dir/frontend.html" "$frontend_url" "$tmp_dir/frontend-scripts.txt" >/dev/null 2>&1 <<'PY'
from html.parser import HTMLParser
from pathlib import Path
import sys
from urllib.parse import urljoin

class ScriptSources(HTMLParser):
    def __init__(self):
        super().__init__()
        self.sources = []

    def handle_starttag(self, tag, attrs):
        if tag == "script":
            source = dict(attrs).get("src")
            if source:
                self.sources.append(urljoin(sys.argv[2] + "/", source))

parser = ScriptSources()
parser.feed(Path(sys.argv[1]).read_text())
Path(sys.argv[3]).write_text("\n".join(parser.sources))
PY
  then
    : > "$tmp_dir/frontend-scripts.txt"
  fi
  index=0
  while IFS= read -r script_url; do
    [[ -n "$script_url" ]] || continue
    index=$((index + 1))
    request "$tmp_dir/frontend-script-$index.js" 15 "$script_url"
    if [[ "$http_status" == "200" ]] && grep -Fq -- "$engine_url" "$tmp_dir/frontend-script-$index.js"; then
      frontend_ok=1
      break
    fi
  done < "$tmp_dir/frontend-scripts.txt"
fi
if [[ $frontend_ok -eq 1 ]]; then
  report PASS "Test Lab loads with the reachable engine URL in its client bundle"
else
  report FAIL "Test Lab failed to load or its client bundle does not target this engine URL"
fi

if [[ $failures -gt 0 ]]; then
  exit 1
fi
exit 0
