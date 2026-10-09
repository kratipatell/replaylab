"use strict";
const $ = (id) => document.getElementById(id);
const statusEl = $("status"), errEl = $("error"), runBtn = $("run-btn");

function setupCanvas(cv) {
  const dpr = window.devicePixelRatio || 1;
  const w = cv.clientWidth || cv.width, h = cv.clientHeight || cv.height;
  cv.width = w * dpr; cv.height = (cv.getAttribute("height") || 180) * dpr;
  const ctx = cv.getContext("2d");
  ctx.scale(dpr, dpr);
  return { ctx, w, h: cv.height / dpr };
}

function lineChart(id, data) {
  const cv = $(id), { ctx, w, h } = setupCanvas(cv);
  ctx.clearRect(0, 0, w, h);
  if (!data || data.length === 0) return;
  const min = Math.min(...data), max = Math.max(...data);
  const pad = (max - min) || 1;
  const X = (i) => (i / (data.length - 1)) * (w - 8) + 4;
  const Y = (v) => h - 6 - ((v - min) / pad) * (h - 12);
  ctx.strokeStyle = "#2e3543"; ctx.beginPath();
  const y1 = Y(1.0);
  if (y1 > 0 && y1 < h) { ctx.moveTo(0, y1); ctx.lineTo(w, y1); }
  ctx.stroke();
  ctx.strokeStyle = "#4da3ff"; ctx.lineWidth = 1.5; ctx.beginPath();
  data.forEach((v, i) => (i ? ctx.lineTo(X(i), Y(v)) : ctx.moveTo(X(0), Y(v))));
  ctx.stroke();
}

function drawdownChart(id, equity) {
  const cv = $(id), { ctx, w, h } = setupCanvas(cv);
  ctx.clearRect(0, 0, w, h);
  if (!equity || equity.length === 0) return;
  let peak = -Infinity;
  const dd = equity.map((e) => {
    peak = Math.max(peak, e);
    return peak ? ((peak - e) / peak) * 100 : 0;
  });
  const max = Math.max(...dd, 1);
  const X = (i) => (i / (dd.length - 1)) * (w - 8) + 4;
  const Y = (v) => h - 4 - (v / max) * (h - 10);
  ctx.fillStyle = "rgba(255,90,90,0.35)"; ctx.beginPath();
  ctx.moveTo(X(0), Y(0));
  dd.forEach((v, i) => ctx.lineTo(X(i), Y(v)));
  ctx.lineTo(X(dd.length - 1), Y(0)); ctx.closePath(); ctx.fill();
  ctx.strokeStyle = "#ff5a5a"; ctx.beginPath();
  dd.forEach((v, i) => (i ? ctx.lineTo(X(i), Y(v)) : ctx.moveTo(X(0), Y(v))));
  ctx.stroke();
}

function monthlyChart(id, monthly) {
  const cv = $(id), { ctx, w, h } = setupCanvas(cv);
  ctx.clearRect(0, 0, w, h);
  const keys = Object.keys(monthly || {}).sort();
  if (!keys.length) {
    ctx.fillStyle = "#9aa3b2"; ctx.font = "12px system-ui";
    ctx.fillText("no monthly data", 10, 20);
    return;
  }
  const vals = keys.map((k) => monthly[k]);
  const max = Math.max(...vals.map(Math.abs), 1);
  const bw = (w - 8) / keys.length;
  const mid = h / 2;
  ctx.fillStyle = "#2e3543"; ctx.fillRect(0, mid, w, 1);
  ctx.font = "9px system-ui";
  keys.forEach((k, i) => {
    const v = vals[i], bh = (Math.abs(v) / max) * (h / 2 - 14);
    ctx.fillStyle = v >= 0 ? "#3fb950" : "#f85149";
    const x = 4 + i * bw + 1, ww = Math.max(bw - 2, 1);
    v >= 0 ? ctx.fillRect(x, mid - bh, ww, bh) : ctx.fillRect(x, mid, ww, bh);
    if (bw > 34) { ctx.fillStyle = "#9aa3b2"; ctx.fillText(k.slice(2), x, h - 2); }
  });
}

const fmt = (v, d = 2) =>
  v === null || v === undefined || Number.isNaN(v) ? "—" : Number(v).toFixed(d);

function renderStats(stats) {
  const tiles = [
    ["Trades", String(stats.trades ?? 0)],
    ["Win rate %", fmt(stats.win_rate_pct)],
    ["Total ret %", fmt(stats.total_return_pct)],
    ["B&H %", fmt(stats.buy_and_hold_pct)],
    ["MaxDD %", fmt(stats.max_drawdown_pct)],
    ["Sharpe", fmt(stats.sharpe)],
    ["Profit factor", stats.profit_factor == null ? "—" : fmt(stats.profit_factor)],
    ["Expectancy %", fmt(stats.expectancy_pct)],
  ];
  $("stats").innerHTML = tiles
    .map(([k, v]) => `<div class="tile"><b>${v}</b><span>${k}</span></div>`)
    .join("");
}

function renderResult(d) {
  $("run-id").textContent = d.run_id != null ? `#${d.run_id}` : (d.id != null ? `#${d.id}` : "");
  renderStats(d.stats);
  const equity = d.equity_curve;
  lineChart("ch-equity", equity);
  drawdownChart("ch-dd", equity);
  monthlyChart("ch-monthly", (d.metrics || {}).monthly || {});
}

function operand(indSel, perSel, isPrice) {
  const ind = $(indSel).value;
  if (["sma", "ema", "rsi"].includes(ind)) {
    const p = Math.max(1, parseInt($(perSel).value || "14", 10));
    return { ind, period: p };
  }
  return { ind };
}

function showError(msg) {
  errEl.hidden = !msg;
  errEl.textContent = msg || "";
}

async function runBacktest(ev) {
  ev.preventDefault();
  showError(""); runBtn.disabled = true; statusEl.textContent = "running…";
  try {
    const entry = { left: operand("f-entry-ind", "f-entry-period"), op: $("f-entry-op").value, right: parseFloat($("f-entry-val").value) };
    let exit = null;
    if ($("f-use-exit").checked) {
      exit = { any: [{ left: operand("f-exit-ind", "f-exit-period"), op: $("f-exit-op").value, right: parseFloat($("f-exit-val").value) }] };
    }
    const strategy = {
      asset: $("f-symbol").value.trim().toUpperCase(),
      timeframe: $("f-interval").value,
      side: $("f-side").value,
      entry: { all: [entry] },
      ...(exit ? { exit } : {}),
    };
    const sl = parseFloat($("f-sl").value), tp = parseFloat($("f-tp").value);
    if (!Number.isNaN(sl) && sl > 0) strategy.stop_loss_pct = sl;
    if (!Number.isNaN(tp) && tp > 0) strategy.take_profit_pct = tp;
    const body = {
      strategy,
      start_ms: Date.parse($("f-start").value),
      end_ms: Date.parse($("f-end").value),
      fee_bps: parseFloat($("f-fee").value),
      slippage_bps: parseFloat($("f-slip").value),
    };
    if (!Number.isFinite(body.start_ms) || !Number.isFinite(body.end_ms)) throw new Error("start/end date invalid");
    const res = await fetch("/backtest", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
    const data = await res.json();
    if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
    renderResult(data);
    statusEl.textContent = `run #${data.run_id} ok (${data.candle_count} candles)`;
    await refreshRuns();
  } catch (e) {
    showError(String(e.message || e));
    statusEl.textContent = "error";
  } finally {
    runBtn.disabled = false;
  }
}

async function refreshRuns() {
  try {
    const res = await fetch("/runs");
    const data = await res.json();
    const tb = $("runs-body");
    if (!data.runs || !data.runs.length) {
      tb.innerHTML = `<tr><td colspan="9" class="muted">no runs yet</td></tr>`;
      return;
    }
    tb.innerHTML = "";
    for (const r of data.runs) {
      const tr = document.createElement("tr");
      const dt = new Date(r.created_at * 1000).toLocaleString();
      tr.innerHTML = `<td>${r.id}</td><td>${dt}</td><td>${r.symbol}</td><td>${r.interval}</td>` +
        `<td>${r.trades}</td><td>${fmt(r.total_return_pct)}</td><td>${fmt(r.sharpe)}</td><td>${fmt(r.max_drawdown_pct)}</td>`;
      const btn = document.createElement("button");
      btn.textContent = "Load";
      btn.onclick = async () => {
        const rr = await fetch(`/runs/${r.id}`);
        const d = await rr.json();
        if (rr.ok) { renderResult(d); statusEl.textContent = `loaded run #${r.id}`; window.scrollTo(0, 0); }
      };
      const td = document.createElement("td");
      td.appendChild(btn); tr.appendChild(td);
      tb.appendChild(tr);
    }
  } catch (e) {
    $("runs-body").innerHTML = `<tr><td colspan="9" class="error">runs failed: ${e.message}</td></tr>`;
  }
}

function defaultDates() {
  const end = Date.now() - (Date.now() % 3600000) - 3600000;
  const start = end - 90 * 86400000;
  const iso = (ms) => new Date(ms).toISOString().slice(0, 16);
  $("f-start").value = iso(start);
  $("f-end").value = iso(end);
}

$("run-form").addEventListener("submit", runBacktest);
defaultDates();
refreshRuns();
