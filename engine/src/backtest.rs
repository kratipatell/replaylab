use crate::instrument::Instrument;
use crate::orders;
use crate::orders::Trail;
use crate::risk::{HaltLimit, RiskError, RiskLimits, RiskState};
use crate::session::SessionFilter;
use crate::sizing::{quantity, Sizing};
use crate::strategy::{BarStrategy, Side, Strategy};
use crate::types::Candle;
use serde::{Deserialize, Serialize};

pub use crate::orders::ExitReason;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BacktestConfig {
    pub fee_bps: f64,
    pub slippage_bps: f64,
}

impl Default for BacktestConfig {
    fn default() -> Self {
        Self {
            fee_bps: 5.0,
            slippage_bps: 2.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Trade {
    pub entry_idx: usize,
    pub exit_idx: usize,
    pub entry_ts: i64,
    pub exit_ts: i64,
    pub side: Side,
    pub entry_price: f64,
    pub exit_price: f64,
    pub return_pct: f64,
    pub reason: ExitReason,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionEvent {
    pub bar_ts: i64,
    pub kind: String,
    pub seq: i64,
    pub reason: Option<String>,
    pub price: Option<f64>,
    pub qty: Option<f64>,
    pub requested_qty: Option<f64>,
    pub configured: Option<f64>,
    pub observed: Option<f64>,
    pub equity: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BacktestResult {
    pub trades: Vec<Trade>,
    pub equity_curve: Vec<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bracket {
    pub stop_pct: Option<f64>,
    pub target_pct: Option<f64>,
    pub trailing_pct: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BacktestError {
    #[error("{0} must be > 0, got {1}")]
    BadPct(String, f64),
    #[error("initial_equity must be > 0, got {0}")]
    BadEquity(f64),
    #[error(transparent)]
    Strategy(#[from] crate::strategy::StrategyError),
    #[error(transparent)]
    Instrument(#[from] crate::instrument::InstrumentError),
    #[error(transparent)]
    Sizing(#[from] crate::sizing::SizingError),
    #[error(transparent)]
    Risk(#[from] RiskError),
    #[error("internal error: {0}")]
    Internal(String),
}

impl Bracket {
    pub fn validate(&self) -> Result<(), BacktestError> {
        for (name, v) in [
            ("stop_pct", self.stop_pct),
            ("target_pct", self.target_pct),
            ("trailing_pct", self.trailing_pct),
        ] {
            if let Some(x) = v {
                if !x.is_finite() || x <= 0.0 {
                    return Err(BacktestError::BadPct(name.to_string(), x));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunConfig {
    pub initial_equity: f64,
    pub exit_at_session_end: bool,
}

impl RunConfig {
    pub fn new(initial_equity: f64, exit_at_session_end: bool) -> Result<RunConfig, BacktestError> {
        if !initial_equity.is_finite() || initial_equity <= 0.0 {
            return Err(BacktestError::BadEquity(initial_equity));
        }
        Ok(RunConfig {
            initial_equity,
            exit_at_session_end,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenPos {
    entry_idx: usize,
    entry_ts: i64,
    entry_raw: f64,
    side: Side,
    qty: f64,
    equity_at_entry: f64,
    entry_cost: f64,
    peak: f64,
    trail: Trail,
}

fn levels(side: Side, entry: f64, b: &Bracket) -> (Option<f64>, Option<f64>, Option<f64>) {
    let pct = |p: f64| p / 100.0;
    match side {
        Side::Long => (
            b.stop_pct.map(|s| entry * (1.0 - pct(s))),
            b.target_pct.map(|t| entry * (1.0 + pct(t))),
            b.trailing_pct.map(|t| entry * (1.0 - pct(t))),
        ),
        Side::Short => (
            b.stop_pct.map(|s| entry * (1.0 + pct(s))),
            b.target_pct.map(|t| entry * (1.0 - pct(t))),
            b.trailing_pct.map(|t| entry * (1.0 + pct(t))),
        ),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingEntry {
    side: Side,
    sig_ts: i64,
}

/// A scheduled exit. New snapshots store the reason; older ones stored a bare
/// bool, which still loads.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
enum PendingExit {
    Reason(ExitReason),
    Legacy(bool),
}

impl PendingExit {
    fn is_set(self) -> bool {
        !matches!(self, PendingExit::Legacy(false))
    }

    fn reason(self) -> ExitReason {
        match self {
            PendingExit::Reason(reason) => reason,
            PendingExit::Legacy(_) => ExitReason::Signal,
        }
    }
}

struct SimCtx<'a> {
    instrument: &'a Instrument,
    cost_rate: f64,
    pv: f64,
    sizing: Sizing,
    bracket: &'a Bracket,
    session: &'a SessionFilter,
    exit_at_session_end: bool,
    risk: Option<&'a RiskLimits>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SimState {
    equity: f64,
    position: Option<OpenPos>,
    pending: Option<PendingEntry>,
    pending_exit: PendingExit,
    trades: Vec<Trade>,
    #[serde(default)]
    equity_curve: Vec<f64>,
    #[serde(default)]
    risk: RiskState,
    #[serde(default)]
    last_bar_equity: Option<f64>,
    #[serde(default)]
    pending_halt_event: Option<HaltLimit>,
    #[serde(default)]
    pending_entry_blocked: bool,
}

#[derive(Serialize)]
struct AgentSimulatorSnapshot<'a> {
    instrument: &'a Instrument,
    cost_rate: f64,
    pv: f64,
    sizing: Sizing,
    bracket: Bracket,
    session: &'a SessionFilter,
    exit_at_session_end: bool,
    risk: Option<RiskLimits>,
    state: AgentSimStateSnapshot<'a>,
}

#[derive(Serialize)]
struct AgentSimStateSnapshot<'a> {
    equity: f64,
    position: &'a Option<OpenPos>,
    pending: &'a Option<PendingEntry>,
    pending_exit: PendingExit,
    trades: &'a [Trade],
    risk: RiskState,
    last_bar_equity: Option<f64>,
    pending_halt_event: Option<HaltLimit>,
    pending_entry_blocked: bool,
}

fn close_position(
    ctx: &SimCtx,
    pos: &OpenPos,
    exit_raw: f64,
    exit_ts: i64,
    exit_idx: usize,
    reason: ExitReason,
    equity_before: f64,
) -> (f64, Trade) {
    let sign = match pos.side {
        Side::Long => 1.0,
        Side::Short => -1.0,
    };
    let exit_cost = ctx.cost_rate * pos.qty * ctx.pv * exit_raw;
    let pnl = sign * pos.qty * ctx.pv * (exit_raw - pos.entry_raw);
    let after = equity_before + pnl - pos.entry_cost - exit_cost;
    let ret = 100.0 * (after / equity_before - 1.0);
    let trade = Trade {
        entry_idx: pos.entry_idx,
        exit_idx,
        entry_ts: pos.entry_ts,
        exit_ts,
        side: pos.side,
        entry_price: pos.entry_raw,
        exit_price: exit_raw,
        return_pct: ret,
        reason,
    };
    (after, trade)
}

/// Size of the entry about to fill, or `None` when the position cap leaves
/// less than the venue's minimum order.
fn entry_qty_detail(
    ctx: &SimCtx,
    equity: f64,
    price: f64,
) -> Result<(f64, Option<f64>), BacktestError> {
    let requested = quantity(ctx.sizing, equity, price)?;
    let Some(limits) = ctx.risk else {
        return Ok((requested, Some(requested)));
    };
    let cap = equity * (limits.max_position_pct / 100.0) / (price * ctx.pv);
    let clamped = requested.min(cap);
    Ok((
        requested,
        (clamped >= ctx.instrument.min_qty).then_some(clamped),
    ))
}

fn emit(events: &mut Option<&mut Vec<DecisionEvent>>, create: impl FnOnce() -> DecisionEvent) {
    if let Some(events) = events.as_deref_mut() {
        let mut event = create();
        event.seq = events.len() as i64;
        events.push(event);
    }
}

fn event(bar_ts: i64, kind: &str, reason: Option<String>) -> DecisionEvent {
    DecisionEvent {
        bar_ts,
        kind: kind.to_string(),
        seq: 0,
        reason,
        price: None,
        qty: None,
        requested_qty: None,
        configured: None,
        observed: None,
        equity: 0.0,
    }
}

fn exit_reason_code(reason: ExitReason) -> &'static str {
    match reason {
        ExitReason::Signal => "signal",
        ExitReason::StopLoss => "stop_loss",
        ExitReason::TakeProfit => "take_profit",
        ExitReason::Trailing => "trailing",
        ExitReason::SessionEnd => "session_end",
        ExitReason::EndOfData => "end_of_data",
        ExitReason::RiskHalt => "risk_halt",
    }
}

fn halt_reason_code(limit: HaltLimit) -> &'static str {
    match limit {
        HaltLimit::DailyLoss => "daily_loss",
        HaltLimit::Drawdown => "drawdown",
        HaltLimit::KillSwitch => "kill_switch",
    }
}

fn configured_limit(limits: &RiskLimits, limit: HaltLimit) -> Option<f64> {
    match limit {
        HaltLimit::DailyLoss => Some(limits.max_daily_loss_pct),
        HaltLimit::Drawdown => Some(limits.max_drawdown_pct),
        HaltLimit::KillSwitch => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn sim_ctx<'a>(
    instrument: &'a Instrument,
    cost_rate: f64,
    pv: f64,
    sizing: Sizing,
    bracket: &'a Bracket,
    session: &'a SessionFilter,
    exit_at_session_end: bool,
    risk: Option<&'a RiskLimits>,
) -> SimCtx<'a> {
    SimCtx {
        instrument,
        cost_rate,
        pv,
        sizing,
        bracket,
        session,
        exit_at_session_end,
        risk,
    }
}

impl SimState {
    fn new(initial_equity: f64, n: usize) -> SimState {
        SimState {
            equity: initial_equity,
            position: None,
            pending: None,
            pending_exit: PendingExit::Legacy(false),
            trades: Vec::new(),
            equity_curve: Vec::with_capacity(n),
            risk: RiskState::starting(initial_equity),
            last_bar_equity: None,
            pending_halt_event: None,
            pending_entry_blocked: false,
        }
    }

    /// Cancel a queued entry and flatten the open position at the next open.
    fn flatten_for_halt(&mut self) -> bool {
        let blocked_entry = self.pending.take().is_some();
        if self.position.is_some() {
            self.pending_exit = PendingExit::Reason(ExitReason::RiskHalt);
        }
        blocked_entry
    }

    fn halt_now(&mut self, limit: HaltLimit) {
        self.risk.force_halt(limit);
        self.pending_entry_blocked |= self.flatten_for_halt();
        self.pending_halt_event = Some(limit);
    }

    /// `live` means another bar is expected but not known yet, so a decision
    /// taken on this bar's close is kept for the next call instead of being
    /// dropped at the end of a finished series.
    fn step(
        &mut self,
        i: usize,
        candles: &[Candle],
        strategy: &mut dyn BarStrategy,
        ctx: &SimCtx,
        live: bool,
        mut events: Option<&mut Vec<DecisionEvent>>,
    ) -> Result<(), BacktestError> {
        let bar = &candles[i];
        let next = candles.get(i + 1);
        if let Some(limit) = self.pending_halt_event.take() {
            emit(&mut events, || {
                let mut logged = event(
                    bar.ts,
                    "halt_triggered",
                    Some(halt_reason_code(limit).to_string()),
                );
                if let Some(limits) = ctx.risk {
                    logged.configured = configured_limit(limits, limit);
                    logged.observed = Some(
                        self.risk.observed_value(
                            limit,
                            self.last_bar_equity
                                .or_else(|| self.equity_curve.last().copied())
                                .unwrap_or(self.equity),
                        ),
                    );
                }
                logged
            });
        }
        if std::mem::take(&mut self.pending_entry_blocked) {
            emit(&mut events, || {
                event(bar.ts, "entry_dropped", Some("blocked_by_halt".to_string()))
            });
        }
        if self.pending_exit.is_set() {
            if let Some(pos) = self.position.take() {
                let raw_exit = bar.open;
                let reason = self.pending_exit.reason();
                let (after, trade) =
                    close_position(ctx, &pos, raw_exit, bar.ts, i, reason, self.equity);
                self.equity = after;
                self.trades.push(trade);
                emit(&mut events, || {
                    let mut logged = event(
                        bar.ts,
                        "exit_filled",
                        Some(exit_reason_code(reason).to_string()),
                    );
                    logged.price = Some(raw_exit);
                    logged.qty = Some(pos.qty);
                    logged
                });
            }
            self.pending_exit = PendingExit::Legacy(false);
        }
        if self.position.is_none() {
            if let Some(entry) = self.pending.take() {
                if ctx.session.contains(entry.sig_ts) && ctx.session.contains(bar.ts) {
                    let entry_raw = ctx.instrument.round_to_tick(bar.open);
                    let (requested_qty, qty) = entry_qty_detail(ctx, self.equity, entry_raw)?;
                    if let Some(qty) = qty {
                        if qty < requested_qty {
                            emit(&mut events, || {
                                let mut logged = event(
                                    bar.ts,
                                    "entry_clamped",
                                    Some("position_cap".to_string()),
                                );
                                logged.price = Some(entry_raw);
                                logged.qty = Some(qty);
                                logged.requested_qty = Some(requested_qty);
                                logged
                            });
                        }
                        let entry_cost = ctx.cost_rate * qty * ctx.pv * entry_raw;
                        self.position = Some(OpenPos {
                            entry_idx: i,
                            entry_ts: bar.ts,
                            entry_raw,
                            side: entry.side,
                            qty,
                            equity_at_entry: self.equity,
                            entry_cost,
                            peak: entry_raw,
                            trail: Trail::new(entry.side, entry_raw, ctx.bracket.trailing_pct),
                        });
                        strategy.on_fill(i);
                        emit(&mut events, || {
                            let mut logged = event(
                                bar.ts,
                                "entry_filled",
                                Some(
                                    match entry.side {
                                        Side::Long => "long",
                                        Side::Short => "short",
                                    }
                                    .to_string(),
                                ),
                            );
                            logged.price = Some(entry_raw);
                            logged.qty = Some(qty);
                            logged
                        });
                    } else {
                        emit(&mut events, || {
                            let mut logged =
                                event(bar.ts, "entry_dropped", Some("below_min_qty".to_string()));
                            logged.price = Some(entry_raw);
                            logged.qty =
                                Some(requested_qty.min(ctx.risk.map_or(requested_qty, |risk| {
                                    self.equity * (risk.max_position_pct / 100.0)
                                        / (entry_raw * ctx.pv)
                                })));
                            logged.requested_qty = Some(requested_qty);
                            logged
                        });
                    }
                }
            }
        }

        if let Some(mut pos) = self.position.take() {
            let (fixed_stop, target, _base_trail) = levels(pos.side, pos.entry_raw, ctx.bracket);
            let mut peak = pos.peak;
            if ctx.bracket.trailing_pct.is_some() {
                match pos.side {
                    Side::Long => {
                        if bar.high > peak {
                            peak = bar.high;
                        }
                    }
                    Side::Short => {
                        if bar.low < peak {
                            peak = bar.low;
                        }
                    }
                }
            }
            pos.peak = peak;
            let _ = pos.trail.advance(pos.side, bar);
            let exit = orders::resolve_exit(bar, pos.side, fixed_stop, target, &pos.trail);
            if let Some((raw_exit, reason)) = exit {
                let (after, trade) =
                    close_position(ctx, &pos, raw_exit, bar.ts, i, reason, self.equity);
                self.equity = after;
                self.trades.push(trade);
                emit(&mut events, || {
                    let mut logged = event(
                        bar.ts,
                        "exit_filled",
                        Some(exit_reason_code(reason).to_string()),
                    );
                    logged.price = Some(raw_exit);
                    logged.qty = Some(pos.qty);
                    logged
                });
            } else {
                self.position = Some(pos);
                if ctx.exit_at_session_end
                    && !live
                    && ctx.session.contains(bar.ts)
                    && next.is_none_or(|b| !ctx.session.contains(b.ts))
                {
                    let Some(pos) = self.position.take() else {
                        return Err(BacktestError::Internal("missing position".to_string()));
                    };
                    let raw_exit = bar.close;
                    let (after, trade) = close_position(
                        ctx,
                        &pos,
                        raw_exit,
                        bar.ts,
                        i,
                        ExitReason::SessionEnd,
                        self.equity,
                    );
                    self.equity = after;
                    self.trades.push(trade);
                }
            }
        }

        if live || next.is_some() {
            if self.position.is_some() {
                if strategy.exit_on_bar(i, candles) {
                    self.pending_exit = PendingExit::Reason(ExitReason::Signal);
                }
            } else if self.pending.is_none()
                && !self.risk.is_halted()
                && ctx.session.contains(bar.ts)
            {
                if let Some(side) = strategy.on_bar(i, candles) {
                    self.pending = Some(PendingEntry {
                        side,
                        sig_ts: bar.ts,
                    });
                    emit(&mut events, || {
                        event(
                            bar.ts,
                            "signal_received",
                            Some(
                                match side {
                                    Side::Long => "long",
                                    Side::Short => "short",
                                }
                                .to_string(),
                            ),
                        )
                    });
                }
            }
        }

        let bar_equity = match &self.position {
            Some(pos) => {
                let sign = match pos.side {
                    Side::Long => 1.0,
                    Side::Short => -1.0,
                };
                let mtm_cost = ctx.cost_rate * pos.qty * ctx.pv * bar.close;
                pos.equity_at_entry + sign * pos.qty * ctx.pv * (bar.close - pos.entry_raw)
                    - pos.entry_cost
                    - mtm_cost
            }
            None => self.equity,
        };
        let prev_close = self
            .last_bar_equity
            .or_else(|| self.equity_curve.last().copied())
            .unwrap_or(bar_equity);
        self.equity_curve.push(bar_equity);
        self.last_bar_equity = Some(bar_equity);
        if let Some(limits) = ctx.risk {
            if let Some(limit) = self.risk.observe(limits, bar.ts, bar_equity, prev_close) {
                emit(&mut events, || {
                    let mut logged = event(
                        bar.ts,
                        "halt_triggered",
                        Some(halt_reason_code(limit).to_string()),
                    );
                    logged.configured = configured_limit(limits, limit);
                    logged.observed = Some(self.risk.observed_value(limit, bar_equity));
                    logged
                });
                if self.flatten_for_halt() {
                    emit(&mut events, || {
                        event(bar.ts, "entry_dropped", Some("blocked_by_halt".to_string()))
                    });
                }
            }
        }
        if let Some(events) = events {
            for logged in events {
                logged.equity = bar_equity;
            }
        }
        Ok(())
    }

    fn finish(&mut self, i: usize, bar: &Candle, ctx: &SimCtx) {
        if let Some(pos) = self.position.take() {
            let raw_exit = bar.close;
            let (after, trade) = close_position(
                ctx,
                &pos,
                raw_exit,
                bar.ts,
                i,
                ExitReason::EndOfData,
                self.equity,
            );
            self.equity = after;
            self.trades.push(trade);
            if let Some(last) = self.equity_curve.last_mut() {
                *last = self.equity;
            }
            self.last_bar_equity = Some(self.equity);
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PositionView {
    pub side: Side,
    pub entry_ts: i64,
    pub entry_price: f64,
    pub qty: f64,
    pub stop: Option<f64>,
    pub target: Option<f64>,
    pub trailing: Option<f64>,
}

/// A resumable bar-by-bar run. Backtests drive it with `step`, which ends the
/// series at the last bar; paper agents drive it with `step_live`, which keeps
/// the last close's decision pending for the next bar. Both share this state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Simulator {
    instrument: Instrument,
    cost_rate: f64,
    pv: f64,
    sizing: Sizing,
    bracket: Bracket,
    session: SessionFilter,
    exit_at_session_end: bool,
    #[serde(default)]
    risk: Option<RiskLimits>,
    state: SimState,
}

impl Simulator {
    pub fn serialize_agent_snapshot<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        AgentSimulatorSnapshot {
            instrument: &self.instrument,
            cost_rate: self.cost_rate,
            pv: self.pv,
            sizing: self.sizing,
            bracket: self.bracket,
            session: &self.session,
            exit_at_session_end: self.exit_at_session_end,
            risk: self.risk,
            state: AgentSimStateSnapshot {
                equity: self.state.equity,
                position: &self.state.position,
                pending: &self.state.pending,
                pending_exit: self.state.pending_exit,
                trades: &self.state.trades,
                risk: self.state.risk,
                last_bar_equity: self.state.last_bar_equity,
                pending_halt_event: self.state.pending_halt_event,
                pending_entry_blocked: self.state.pending_entry_blocked,
            },
        }
        .serialize(serializer)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        instrument: Instrument,
        sizing: Sizing,
        bracket: Bracket,
        session: SessionFilter,
        initial_equity: f64,
        exit_at_session_end: bool,
        risk: Option<RiskLimits>,
    ) -> Result<Simulator, BacktestError> {
        bracket.validate()?;
        if !initial_equity.is_finite() || initial_equity <= 0.0 {
            return Err(BacktestError::BadEquity(initial_equity));
        }
        if let Some(limits) = &risk {
            limits.validate()?;
        }
        Ok(Simulator {
            cost_rate: instrument.cost_rate(),
            pv: instrument.point_value,
            instrument,
            sizing,
            bracket,
            session,
            exit_at_session_end,
            risk,
            state: SimState::new(initial_equity, 0),
        })
    }

    pub fn from_strategy(
        spec: &Strategy,
        cfg: &BacktestConfig,
        initial_equity: f64,
        risk: Option<RiskLimits>,
    ) -> Result<Simulator, BacktestError> {
        spec.validate()?;
        let (instrument, sizing, bracket, session, run_cfg) =
            crate::strategy::rules_v2_setup(spec, cfg)?;
        Simulator::new(
            instrument,
            sizing,
            bracket,
            session,
            initial_equity,
            run_cfg.exit_at_session_end,
            risk,
        )
    }

    pub fn step(
        &mut self,
        i: usize,
        candles: &[Candle],
        strategy: &mut dyn BarStrategy,
    ) -> Result<(), BacktestError> {
        let ctx = sim_ctx(
            &self.instrument,
            self.cost_rate,
            self.pv,
            self.sizing,
            &self.bracket,
            &self.session,
            self.exit_at_session_end,
            self.risk.as_ref(),
        );
        self.state.step(i, candles, strategy, &ctx, false, None)
    }

    pub fn step_live(
        &mut self,
        i: usize,
        candles: &[Candle],
        strategy: &mut dyn BarStrategy,
    ) -> Result<(), BacktestError> {
        let ctx = sim_ctx(
            &self.instrument,
            self.cost_rate,
            self.pv,
            self.sizing,
            &self.bracket,
            &self.session,
            self.exit_at_session_end,
            self.risk.as_ref(),
        );
        self.state.step(i, candles, strategy, &ctx, true, None)
    }

    pub fn step_live_with_events(
        &mut self,
        i: usize,
        candles: &[Candle],
        strategy: &mut dyn BarStrategy,
    ) -> Result<Vec<DecisionEvent>, BacktestError> {
        let ctx = sim_ctx(
            &self.instrument,
            self.cost_rate,
            self.pv,
            self.sizing,
            &self.bracket,
            &self.session,
            self.exit_at_session_end,
            self.risk.as_ref(),
        );
        let mut events = Vec::new();
        self.state
            .step(i, candles, strategy, &ctx, true, Some(&mut events))?;
        Ok(events)
    }

    pub fn finish(&mut self, i: usize, bar: &Candle) {
        let ctx = sim_ctx(
            &self.instrument,
            self.cost_rate,
            self.pv,
            self.sizing,
            &self.bracket,
            &self.session,
            self.exit_at_session_end,
            self.risk.as_ref(),
        );
        self.state.finish(i, bar, &ctx);
    }

    pub fn current_equity(&self) -> f64 {
        self.state
            .last_bar_equity
            .or_else(|| self.state.equity_curve.last().copied())
            .unwrap_or(self.state.equity)
    }

    pub fn restore_last_bar_equity(&mut self) {
        if self.state.last_bar_equity.is_none() {
            self.state.last_bar_equity = self.state.equity_curve.last().copied();
        }
    }

    pub fn trades(&self) -> &[Trade] {
        &self.state.trades
    }

    pub fn equity_curve(&self) -> &[f64] {
        &self.state.equity_curve
    }

    pub fn position(&self) -> Option<PositionView> {
        let pos = self.state.position.as_ref()?;
        let (stop, target, _) = levels(pos.side, pos.entry_raw, &self.bracket);
        Some(PositionView {
            side: pos.side,
            entry_ts: pos.entry_ts,
            entry_price: pos.entry_raw,
            qty: pos.qty,
            stop,
            target,
            trailing: pos.trail.level(),
        })
    }

    pub fn into_result(self) -> BacktestResult {
        BacktestResult {
            trades: self.state.trades,
            equity_curve: self.state.equity_curve,
        }
    }

    pub fn risk_limits(&self) -> Option<RiskLimits> {
        self.risk
    }

    /// Rehydrate caps on a snapshot written before agents enforced risk.
    pub fn set_risk(&mut self, limits: RiskLimits) {
        self.risk = Some(limits);
    }

    #[cfg(test)]
    pub fn risk_state(&self) -> RiskState {
        self.state.risk
    }

    pub fn halt(&self) -> Option<HaltLimit> {
        self.state.risk.halt()
    }

    /// Operator stop: flatten now, flat until the halt is cleared.
    pub fn kill_switch(&mut self) {
        self.state.halt_now(HaltLimit::KillSwitch);
    }

    pub fn clear_kill_switch(&mut self) {
        self.state.risk.clear_kill_switch();
    }

    pub fn reset_risk(&mut self, equity: f64, ts: i64) {
        self.state.risk.reset(equity, ts);
    }
}

/// A finished run plus the halt that stopped it, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct RiskOutcome {
    pub result: BacktestResult,
    pub halt: Option<HaltLimit>,
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    strategy: &mut dyn BarStrategy,
    candles: &[Candle],
    instrument: &Instrument,
    sizing: Sizing,
    bracket: &Bracket,
    session: &SessionFilter,
    cfg: &RunConfig,
) -> Result<BacktestResult, BacktestError> {
    run_with_risk(
        strategy, candles, instrument, sizing, bracket, session, cfg, None,
    )
    .map(|outcome| outcome.result)
}

#[allow(clippy::too_many_arguments)]
pub fn run_with_risk(
    strategy: &mut dyn BarStrategy,
    candles: &[Candle],
    instrument: &Instrument,
    sizing: Sizing,
    bracket: &Bracket,
    session: &SessionFilter,
    cfg: &RunConfig,
    risk: Option<RiskLimits>,
) -> Result<RiskOutcome, BacktestError> {
    bracket.validate()?;
    if !cfg.initial_equity.is_finite() || cfg.initial_equity <= 0.0 {
        return Err(BacktestError::BadEquity(cfg.initial_equity));
    }
    let n = candles.len();
    if n == 0 {
        return Ok(RiskOutcome {
            result: BacktestResult {
                trades: Vec::new(),
                equity_curve: Vec::new(),
            },
            halt: None,
        });
    }
    let mut sim = Simulator::new(
        instrument.clone(),
        sizing,
        *bracket,
        *session,
        cfg.initial_equity,
        cfg.exit_at_session_end,
        risk,
    )?;
    for i in 0..n {
        sim.step(i, candles, strategy)?;
    }
    sim.finish(n - 1, &candles[n - 1]);
    Ok(RiskOutcome {
        halt: sim.halt(),
        result: sim.into_result(),
    })
}

pub fn backtest_strategy(
    spec: &Strategy,
    candles: &[Candle],
    cfg: &BacktestConfig,
) -> Result<BacktestResult, BacktestError> {
    spec.validate()?;
    let (instrument, sizing, bracket, session, run_cfg) =
        crate::strategy::rules_v2_setup(spec, cfg)?;
    let mut strategy = crate::strategy::RulesStrategy::new(spec);
    run(
        &mut strategy,
        candles,
        &instrument,
        sizing,
        &bracket,
        &session,
        &run_cfg,
    )
}

/// `risk` is opt-in: `None` runs the same simulator with no caps and no risk
/// state effects.
pub fn backtest_strategy_with_risk(
    spec: &Strategy,
    candles: &[Candle],
    cfg: &BacktestConfig,
    risk: Option<RiskLimits>,
) -> Result<RiskOutcome, BacktestError> {
    spec.validate()?;
    let (instrument, sizing, bracket, session, run_cfg) =
        crate::strategy::rules_v2_setup(spec, cfg)?;
    let mut strategy = crate::strategy::RulesStrategy::new(spec);
    run_with_risk(
        &mut strategy,
        candles,
        &instrument,
        sizing,
        &bracket,
        &session,
        &run_cfg,
        risk,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::MS_PER_MIN;

    fn c(ts: i64, o: f64, h: f64, l: f64, cl: f64) -> Candle {
        Candle {
            ts,
            open: o,
            high: h,
            low: l,
            close: cl,
        }
    }

    fn inst() -> Instrument {
        Instrument::new("TEST", 0.01, 1.0, 0.0, 0.0).expect("valid")
    }

    fn session_all() -> SessionFilter {
        SessionFilter::all_day()
    }

    fn run_cfg() -> RunConfig {
        RunConfig::new(10_000.0, false).expect("valid")
    }

    fn no_bracket() -> Bracket {
        Bracket {
            stop_pct: None,
            target_pct: None,
            trailing_pct: None,
        }
    }

    fn limits(max_position_pct: f64, max_daily_loss_pct: f64, max_drawdown_pct: f64) -> RiskLimits {
        RiskLimits {
            max_position_pct,
            max_daily_loss_pct,
            max_drawdown_pct,
        }
    }

    /// Enter once and report the filled quantity, if the entry survived the cap.
    fn open_qty(max_position_pct: f64, instrument: Instrument) -> Option<f64> {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
            c(2 * MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
        ];
        let mut s = Sig(vec![Some(Side::Long), None, None]);
        let mut sim = Simulator::new(
            instrument,
            Sizing::FixedQty(100.0),
            no_bracket(),
            session_all(),
            10_000.0,
            false,
            Some(limits(max_position_pct, 100.0, 100.0)),
        )
        .expect("sim");
        for i in 0..candles.len() {
            sim.step(i, &candles, &mut s).expect("step");
        }
        sim.position().map(|p| p.qty)
    }

    #[test]
    fn shared_simulator_emits_fill_clamp_drop_risk_halt_and_exit_events() {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
            c(2 * MS_PER_MIN, 100.0, 100.0, 69.0, 70.0),
            c(3 * MS_PER_MIN, 70.0, 70.0, 70.0, 70.0),
        ];
        let mut strategy = Sig(vec![Some(Side::Long), None, None, None]);
        let mut sim = Simulator::new(
            inst(),
            Sizing::FixedQty(100.0),
            no_bracket(),
            session_all(),
            10_000.0,
            false,
            Some(limits(50.0, 100.0, 10.0)),
        )
        .expect("sim");
        let mut events = Vec::new();
        for i in 0..candles.len() {
            events.extend(
                sim.step_live_with_events(i, &candles, &mut strategy)
                    .expect("live step"),
            );
        }
        assert!(events.iter().any(|e| e.kind == "signal_received"));
        let clamp = events
            .iter()
            .find(|e| e.kind == "entry_clamped")
            .expect("cap clamp");
        assert_eq!(clamp.reason.as_deref(), Some("position_cap"));
        assert_eq!(clamp.requested_qty, Some(100.0));
        assert_eq!(clamp.qty, Some(50.0));
        let fill = events
            .iter()
            .find(|e| e.kind == "entry_filled")
            .expect("entry fill");
        assert_eq!(fill.price, Some(100.0));
        assert_eq!(fill.qty, Some(50.0));
        let halt = events
            .iter()
            .find(|e| e.kind == "halt_triggered")
            .expect("risk halt");
        assert_eq!(halt.reason.as_deref(), Some("drawdown"));
        assert_eq!(halt.configured, Some(10.0));
        assert!(halt.observed.is_some_and(|value| value >= 10.0));
        let exit = events
            .iter()
            .find(|e| e.kind == "exit_filled")
            .expect("risk exit");
        assert_eq!(exit.reason.as_deref(), Some("risk_halt"));
        assert_eq!(exit.price, Some(70.0));

        let min_size_instrument = inst().with_min_qty(11.0).expect("min qty");
        let mut dropped_strategy = Sig(vec![Some(Side::Long), None]);
        let mut dropped_sim = Simulator::new(
            min_size_instrument,
            Sizing::FixedQty(100.0),
            no_bracket(),
            session_all(),
            10_000.0,
            false,
            Some(limits(10.0, 100.0, 100.0)),
        )
        .expect("sim");
        dropped_sim
            .step_live_with_events(0, &candles[..2], &mut dropped_strategy)
            .expect("signal step");
        let dropped = dropped_sim
            .step_live_with_events(1, &candles[..2], &mut dropped_strategy)
            .expect("drop step");
        let drop = dropped
            .iter()
            .find(|e| e.kind == "entry_dropped")
            .expect("minimum size drop");
        assert_eq!(drop.reason.as_deref(), Some("below_min_qty"));
        assert_eq!(drop.requested_qty, Some(100.0));
        assert_eq!(drop.qty, Some(10.0));

        let stopped = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
            c(2 * MS_PER_MIN, 100.0, 100.0, 94.0, 94.0),
        ];
        let mut blocked_strategy = Sig(vec![Some(Side::Long), None, Some(Side::Long)]);
        let mut blocked_sim = Simulator::new(
            inst(),
            Sizing::FixedQty(100.0),
            Bracket {
                stop_pct: Some(5.0),
                target_pct: None,
                trailing_pct: None,
            },
            session_all(),
            10_000.0,
            false,
            Some(limits(100.0, 1.0, 100.0)),
        )
        .expect("sim");
        blocked_sim
            .step_live_with_events(0, &stopped, &mut blocked_strategy)
            .expect("signal");
        blocked_sim
            .step_live_with_events(1, &stopped, &mut blocked_strategy)
            .expect("fill");
        let blocked = blocked_sim
            .step_live_with_events(2, &stopped, &mut blocked_strategy)
            .expect("stop and halt");
        assert!(blocked.iter().any(|e| {
            e.kind == "entry_dropped" && e.reason.as_deref() == Some("blocked_by_halt")
        }));
    }

    #[test]
    fn position_cap_clamps_the_entry_qty() {
        // 100 units is the whole 10k book, so a 10% cap leaves 10 units.
        let capped = open_qty(10.0, inst()).expect("capped entry");
        assert!((capped - 10.0).abs() < 1e-9, "capped qty {capped}");
        // A cap the size of the book clamps nothing.
        let uncapped = open_qty(100.0, inst()).expect("uncapped entry");
        assert!((uncapped - 100.0).abs() < 1e-9, "uncapped qty {uncapped}");
    }

    #[test]
    fn a_cap_below_the_min_size_drops_the_entry() {
        let instrument = inst().with_min_qty(11.0).expect("min qty");
        assert_eq!(
            open_qty(10.0, instrument),
            None,
            "10 units is below the 11 unit minimum"
        );
    }

    #[test]
    fn a_daily_loss_halt_flattens_at_the_next_open() {
        use crate::session::MS_PER_DAY;
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_DAY - MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
            // First bar of the new UTC day closes exactly 10% down.
            c(MS_PER_DAY, 100.0, 100.0, 89.0, 90.0),
            c(MS_PER_DAY + MS_PER_MIN, 90.0, 90.0, 90.0, 90.0),
        ];
        let mut s = Sig(vec![Some(Side::Long), None, None, None]);
        let out = run_with_risk(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(100.0),
            &no_bracket(),
            &session_all(),
            &run_cfg(),
            Some(limits(100.0, 10.0, 100.0)),
        )
        .expect("run");
        assert_eq!(out.halt, Some(HaltLimit::DailyLoss));
        assert_eq!(out.result.trades.len(), 1);
        let trade = &out.result.trades[0];
        assert_eq!(trade.reason, ExitReason::RiskHalt);
        assert_eq!(trade.exit_idx, 3);
        assert!((trade.exit_price - 90.0).abs() < 1e-9);
    }

    #[test]
    fn a_drawdown_halt_flattens_at_the_next_open() {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
            c(2 * MS_PER_MIN, 100.0, 100.0, 69.0, 70.0),
            c(3 * MS_PER_MIN, 70.0, 70.0, 70.0, 70.0),
        ];
        let mut s = Sig(vec![Some(Side::Long), None, None, None]);
        let out = run_with_risk(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(100.0),
            &no_bracket(),
            &session_all(),
            &run_cfg(),
            Some(limits(100.0, 100.0, 20.0)),
        )
        .expect("run");
        assert_eq!(out.halt, Some(HaltLimit::Drawdown));
        assert_eq!(out.result.trades.len(), 1);
        let trade = &out.result.trades[0];
        assert_eq!(trade.reason, ExitReason::RiskHalt);
        assert_eq!(trade.exit_idx, 3);
        assert!((trade.exit_price - 70.0).abs() < 1e-9);
    }

    #[test]
    fn a_run_without_risk_never_halts_or_flattens_early() {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
            c(2 * MS_PER_MIN, 100.0, 100.0, 69.0, 70.0),
            c(3 * MS_PER_MIN, 70.0, 70.0, 70.0, 70.0),
        ];
        let mut s = Sig(vec![Some(Side::Long), None, None, None]);
        let out = run_with_risk(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(100.0),
            &no_bracket(),
            &session_all(),
            &run_cfg(),
            None,
        )
        .expect("run");
        assert_eq!(out.halt, None);
        assert_eq!(out.result.trades.len(), 1);
        assert_eq!(out.result.trades[0].reason, ExitReason::EndOfData);

        let mut s = Sig(vec![Some(Side::Long), None, None, None]);
        let plain = run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(100.0),
            &no_bracket(),
            &session_all(),
            &run_cfg(),
        )
        .expect("run");
        assert_eq!(plain, out.result, "risk=None must equal the plain run");
    }

    struct Sig(Vec<Option<Side>>);

    impl BarStrategy for Sig {
        fn on_bar(&mut self, idx: usize, _candles: &[Candle]) -> Option<Side> {
            self.0.get(idx).copied().flatten()
        }
    }

    #[test]
    fn stop_loss_fires_at_level() {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 101.0, 97.0, 100.0),
            c(2 * MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
        ];
        let mut s = Sig(vec![Some(Side::Long), None, None]);
        let b = Bracket {
            stop_pct: Some(2.0),
            target_pct: None,
            trailing_pct: None,
        };
        let r = run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &b,
            &session_all(),
            &run_cfg(),
        )
        .unwrap();
        assert_eq!(r.trades.len(), 1);
        assert_eq!(r.trades[0].entry_idx, 1);
        assert_eq!(r.trades[0].exit_idx, 1);
        assert!((r.trades[0].exit_price - 98.0).abs() < 1e-9);
        assert_eq!(r.trades[0].reason, ExitReason::StopLoss);
        assert!((r.equity_curve[1] - 9_998.0).abs() < 1e-9);
    }

    #[test]
    fn take_profit_fires_at_level() {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 103.0, 100.0, 102.0),
            c(2 * MS_PER_MIN, 102.0, 102.0, 102.0, 102.0),
        ];
        let mut s = Sig(vec![Some(Side::Long), None, None]);
        let b = Bracket {
            stop_pct: None,
            target_pct: Some(2.0),
            trailing_pct: None,
        };
        let r = run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &b,
            &session_all(),
            &run_cfg(),
        )
        .unwrap();
        assert_eq!(r.trades.len(), 1);
        assert!((r.trades[0].exit_price - 102.0).abs() < 1e-9);
        assert_eq!(r.trades[0].reason, ExitReason::TakeProfit);
        assert!((r.equity_curve[1] - 10_002.0).abs() < 1e-9);
    }

    #[test]
    fn trailing_ratchets_and_fires() {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 110.0, 109.0, 109.0),
            c(2 * MS_PER_MIN, 109.0, 109.0, 108.0, 108.5),
            c(3 * MS_PER_MIN, 108.5, 108.5, 108.5, 108.5),
        ];
        let mut s = Sig(vec![Some(Side::Long), None, None, None]);
        let b = Bracket {
            stop_pct: None,
            target_pct: None,
            trailing_pct: Some(1.0),
        };
        let r = run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &b,
            &session_all(),
            &run_cfg(),
        )
        .unwrap();
        assert_eq!(r.trades.len(), 1);
        assert_eq!(r.trades[0].reason, ExitReason::Trailing);
        assert!((r.trades[0].exit_price - 108.9).abs() < 1e-9);
        assert_eq!(r.trades[0].exit_idx, 2);
    }

    #[test]
    fn out_of_session_signal_is_dropped() {
        let eight = 8 * 60 * MS_PER_MIN;
        let ten = 10 * 60 * MS_PER_MIN;
        let candles = vec![
            c(eight, 100.0, 100.0, 100.0, 100.0),
            c(eight + MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
            c(ten, 100.0, 100.0, 100.0, 100.0),
        ];
        let mut s = Sig(vec![Some(Side::Long), None, None]);
        let b = Bracket {
            stop_pct: None,
            target_pct: None,
            trailing_pct: None,
        };
        let session = SessionFilter::new(9 * 60 + 30, 16 * 60).expect("valid");
        let r = run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &b,
            &session,
            &run_cfg(),
        )
        .unwrap();
        assert!(r.trades.is_empty());
    }

    struct SigExit {
        entries: Vec<Option<Side>>,
        exits: Vec<bool>,
    }

    impl BarStrategy for SigExit {
        fn on_bar(&mut self, idx: usize, _candles: &[Candle]) -> Option<Side> {
            self.entries.get(idx).copied().flatten()
        }
        fn exit_on_bar(&mut self, idx: usize, _candles: &[Candle]) -> bool {
            self.exits.get(idx).copied().unwrap_or(false)
        }
    }

    fn flat_bracket() -> Bracket {
        Bracket {
            stop_pct: None,
            target_pct: None,
            trailing_pct: None,
        }
    }

    #[test]
    fn signal_exit_fills_next_open() {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
            c(2 * MS_PER_MIN, 105.0, 105.0, 105.0, 105.0),
            c(3 * MS_PER_MIN, 105.0, 105.0, 105.0, 105.0),
        ];
        let mut s = SigExit {
            entries: vec![Some(Side::Long), None, None, None],
            exits: vec![false, true, false, false],
        };
        let r = run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &flat_bracket(),
            &session_all(),
            &run_cfg(),
        )
        .unwrap();
        assert_eq!(r.trades.len(), 1);
        assert_eq!(r.trades[0].entry_idx, 1);
        assert_eq!(r.trades[0].exit_idx, 2);
        assert!((r.trades[0].exit_price - 105.0).abs() < 1e-9);
        assert_eq!(r.trades[0].reason, ExitReason::Signal);
    }

    #[test]
    fn signal_on_last_bar_is_end_of_data() {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
            c(2 * MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
        ];
        let mut s = SigExit {
            entries: vec![Some(Side::Long), None, None],
            exits: vec![false, false, true],
        };
        let r = run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &flat_bracket(),
            &session_all(),
            &run_cfg(),
        )
        .unwrap();
        assert_eq!(r.trades.len(), 1);
        assert_eq!(r.trades[0].exit_idx, 2);
        assert_eq!(r.trades[0].reason, ExitReason::EndOfData);
    }

    #[test]
    fn pending_signal_exit_beats_gap_through_stop() {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
            c(2 * MS_PER_MIN, 90.0, 91.0, 89.0, 90.0),
        ];
        let mut s = SigExit {
            entries: vec![Some(Side::Long), None, None],
            exits: vec![false, true, false],
        };
        let b = Bracket {
            stop_pct: Some(2.0),
            target_pct: None,
            trailing_pct: None,
        };
        let r = run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &b,
            &session_all(),
            &run_cfg(),
        )
        .unwrap();
        assert_eq!(r.trades.len(), 1);
        assert_eq!(r.trades[0].exit_idx, 2);
        assert!((r.trades[0].exit_price - 90.0).abs() < 1e-9);
        assert_eq!(r.trades[0].reason, ExitReason::Signal);
    }

    #[test]
    fn default_strategy_never_exits() {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
            c(2 * MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
        ];
        let mut s = Sig(vec![Some(Side::Long), None, None]);
        assert!(!s.exit_on_bar(0, &candles));
        assert!(!s.exit_on_bar(1, &candles));
        let r = run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &flat_bracket(),
            &session_all(),
            &run_cfg(),
        )
        .unwrap();
        assert_eq!(r.trades.len(), 1);
        assert_eq!(r.trades[0].reason, ExitReason::EndOfData);
    }

    #[test]
    fn bracket_rejects_bad_pct() {
        let b = Bracket {
            stop_pct: Some(0.0),
            target_pct: None,
            trailing_pct: None,
        };
        assert!(b.validate().is_err());
    }

    #[test]
    fn ratcheted_trail_opening_below_the_level_fills_at_the_open() {
        // Entry 100 with a 10% trail: entry-relative level 90, ratcheted to
        // 108 by the 120 high. The last bar opens at 105, under that 108.
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 111.0, 100.0, 110.0),
            c(2 * MS_PER_MIN, 110.0, 120.0, 110.0, 119.0),
            c(3 * MS_PER_MIN, 105.0, 106.0, 104.0, 104.5),
        ];
        let mut s = Sig(vec![Some(Side::Long), None, None, None]);
        let b = Bracket {
            stop_pct: None,
            target_pct: None,
            trailing_pct: Some(10.0),
        };
        let r = run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &b,
            &session_all(),
            &run_cfg(),
        )
        .unwrap();
        assert_eq!(r.trades.len(), 1);
        assert_eq!(r.trades[0].entry_idx, 1);
        assert_eq!(r.trades[0].exit_idx, 3);
        assert_eq!(r.trades[0].reason, ExitReason::Trailing);
        // The 108 stop was already breached at the open, so the gap pays the
        // 105 open rather than a price the position could not have got.
        assert!((r.trades[0].exit_price - 105.0).abs() < 1e-9);
        assert!((r.equity_curve[3] - 10_005.0).abs() < 1e-9);
    }
}

#[cfg(test)]
mod legacy_tests {
    use super::*;
    use crate::strategy::{
        Condition, IndicatorKind, IndicatorRef, Op, Operand, Rules, Side, Strategy,
    };

    fn c(ts: i64, open: f64, high: f64, low: f64, close: f64) -> Candle {
        Candle {
            ts,
            open,
            high,
            low,
            close,
        }
    }

    fn ind(ind: IndicatorKind, period: Option<usize>) -> Operand {
        Operand::Indicator(IndicatorRef { ind, period })
    }

    fn num(v: f64) -> Operand {
        Operand::Number(v)
    }

    fn long_strategy(
        entry: Rules,
        exit: Option<Rules>,
        sl: Option<f64>,
        tp: Option<f64>,
    ) -> Strategy {
        Strategy {
            asset: "TEST".to_string(),
            timeframe: "1h".to_string(),
            side: Side::Long,
            entry,
            exit,
            stop_loss_pct: sl,
            take_profit_pct: tp,
            claim: None,
        }
    }

    fn short_strategy(
        entry: Rules,
        exit: Option<Rules>,
        sl: Option<f64>,
        tp: Option<f64>,
    ) -> Strategy {
        Strategy {
            asset: "TEST".to_string(),
            timeframe: "1h".to_string(),
            side: Side::Short,
            entry,
            exit,
            stop_loss_pct: sl,
            take_profit_pct: tp,
            claim: None,
        }
    }

    fn entry_close_gt(threshold: f64) -> Rules {
        Rules {
            all: vec![Condition {
                left: ind(IndicatorKind::Close, None),
                op: Op::Gt,
                right: num(threshold),
            }],
            any: vec![],
        }
    }

    fn entry_close_lt(threshold: f64) -> Rules {
        Rules {
            all: vec![Condition {
                left: ind(IndicatorKind::Close, None),
                op: Op::Lt,
                right: num(threshold),
            }],
            any: vec![],
        }
    }

    fn zero_cost() -> BacktestConfig {
        BacktestConfig {
            fee_bps: 0.0,
            slippage_bps: 0.0,
        }
    }

    #[test]
    fn entry_fill_is_next_bar_open_not_signal_close() {
        let candles = vec![
            c(0, 90.0, 95.0, 85.0, 90.0),
            c(1, 90.0, 115.0, 90.0, 110.0),
            c(2, 200.0, 210.0, 190.0, 205.0),
            c(3, 205.0, 210.0, 200.0, 206.0),
        ];
        let strat = long_strategy(entry_close_gt(100.0), None, None, None);
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
        assert_eq!(res.trades.len(), 1);
        let t = &res.trades[0];
        assert_eq!(t.entry_idx, 2, "entry must fill at bar 2 open, not bar 1");
        assert!(
            (t.entry_price - 200.0).abs() < 1e-12,
            "entry_price {}, expected 200",
            t.entry_price
        );
        assert_ne!(t.entry_price, 110.0);
    }

    #[test]
    fn truncated_run_trades_appear_in_full_run_no_lookahead() {
        let closes = [
            95.0, 105.0, 95.0, 105.0, 95.0, 105.0, 95.0, 105.0, 95.0, 105.0,
        ];
        let candles: Vec<Candle> = closes
            .iter()
            .enumerate()
            .map(|(i, cl)| c(i as i64, *cl, *cl + 1.0, *cl - 1.0, *cl))
            .collect();
        let entry = entry_close_gt(100.0);
        let exit_rules = Rules {
            all: vec![],
            any: vec![Condition {
                left: ind(IndicatorKind::Close, None),
                op: Op::Lt,
                right: num(100.0),
            }],
        };
        let strat = long_strategy(entry, Some(exit_rules), None, None);
        let cfg = zero_cost();
        let full = backtest_strategy(&strat, &candles, &cfg).unwrap();
        let k = 6;
        let trunc_candles = &candles[..k];
        let trunc = backtest_strategy(&strat, trunc_candles, &cfg).unwrap();
        for tt in trunc
            .trades
            .iter()
            .filter(|t| t.reason != ExitReason::EndOfData)
        {
            let found = full.trades.iter().any(|ft| {
                ft.entry_idx == tt.entry_idx
                    && ft.exit_idx == tt.exit_idx
                    && ft.entry_ts == tt.entry_ts
                    && ft.exit_ts == tt.exit_ts
                    && ft.side == tt.side
                    && (ft.entry_price - tt.entry_price).abs() < 1e-12
                    && (ft.exit_price - tt.exit_price).abs() < 1e-12
                    && (ft.return_pct - tt.return_pct).abs() < 1e-12
                    && ft.reason == tt.reason
            });
            assert!(
                found,
                "truncated trade {:?} not found identically in full {:?}",
                tt, full.trades
            );
        }
        assert!(
            trunc.trades.iter().any(|t| t.reason == ExitReason::Signal),
            "expected at least one signal trade in truncated run, got {:?}",
            trunc.trades
        );
    }

    #[test]
    fn stop_loss_wins_when_both_stop_and_target_hit() {
        let candles = vec![
            c(0, 90.0, 115.0, 90.0, 110.0),
            c(1, 100.0, 101.0, 99.0, 100.0),
            c(2, 100.0, 110.0, 90.0, 100.0),
            c(3, 100.0, 101.0, 99.0, 90.0),
        ];
        let strat = long_strategy(entry_close_gt(100.0), None, Some(2.0), Some(2.0));
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
        assert!(!res.trades.is_empty());
        let t = &res.trades[0];
        assert_eq!(t.entry_idx, 1);
        assert_eq!(t.exit_idx, 2);
        assert_eq!(t.reason, ExitReason::StopLoss);
        assert!(
            (t.exit_price - 98.0).abs() < 1e-9,
            "exit {} expected 98 (stop)",
            t.exit_price
        );
    }

    #[test]
    fn gap_through_stop_fills_at_open_not_stop_level() {
        let candles = vec![
            c(0, 90.0, 115.0, 90.0, 110.0),
            c(1, 100.0, 101.0, 99.0, 100.0),
            c(2, 90.0, 95.0, 85.0, 92.0),
            c(3, 92.0, 93.0, 91.0, 90.0),
        ];
        let strat = long_strategy(entry_close_gt(100.0), None, Some(2.0), None);
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
        assert!(!res.trades.is_empty());
        let t = &res.trades[0];
        assert_eq!(t.reason, ExitReason::StopLoss);
        assert_eq!(t.exit_idx, 2);
        assert!(
            (t.exit_price - 90.0).abs() < 1e-9,
            "gap fill: exit {} expected open 90, not stop 98",
            t.exit_price
        );
    }

    #[test]
    fn flat_prices_with_fees_lose_about_point_two_percent() {
        let candles: Vec<Candle> = (0..5).map(|i| c(i, 100.0, 100.0, 100.0, 100.0)).collect();
        let strat = long_strategy(entry_close_gt(99.0), None, None, None);
        let cfg = BacktestConfig {
            fee_bps: 10.0,
            slippage_bps: 0.0,
        };
        let res = backtest_strategy(&strat, &candles, &cfg).unwrap();
        assert_eq!(res.trades.len(), 1);
        let t = &res.trades[0];
        assert!(
            (t.return_pct - (-0.2)).abs() < 1e-9,
            "return {} expected -0.2",
            t.return_pct
        );
    }

    #[test]
    fn reentry_after_stopout_requires_entry_false_bar() {
        let candles = vec![
            c(0, 100.0, 110.0, 100.0, 110.0),
            c(1, 100.0, 101.0, 90.0, 110.0), // entry fill 100 + SL hit same bar,
            c(2, 100.0, 101.0, 99.0, 110.0), // entry true but must NOT enter (disarmed),
            c(3, 100.0, 101.0, 99.0, 90.0),  // entry false -> re-arm,
            c(4, 100.0, 101.0, 99.0, 110.0), // entry true -> schedule entry,
            c(5, 100.0, 101.0, 99.0, 110.0), // entry fill,
            c(6, 100.0, 101.0, 99.0, 100.0), // hold to end,
        ];
        let strat = long_strategy(entry_close_gt(100.0), None, Some(2.0), None);
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
        assert_eq!(
            res.trades.len(),
            2,
            "expected exactly 2 trades, got {:?}",
            res.trades
        );
        assert_eq!(res.trades[0].entry_idx, 1);
        assert_eq!(res.trades[0].reason, ExitReason::StopLoss);
        assert!(
            !res.trades
                .iter()
                .any(|t| t.entry_idx == 2 || t.entry_idx == 3),
            "must not re-enter while entry still true: {:?}",
            res.trades
        );
        assert_eq!(
            res.trades[1].entry_idx, 5,
            "second entry after false bar, got {:?}",
            res.trades
        );
    }

    #[test]
    fn short_gains_when_price_falls_and_stop_is_above_entry() {
        let fall = vec![
            c(0, 100.0, 100.0, 90.0, 90.0),
            c(1, 100.0, 101.0, 99.0, 95.0),
            c(2, 95.0, 96.0, 90.0, 90.0),
            c(3, 90.0, 91.0, 85.0, 85.0),
        ];
        let strat = short_strategy(entry_close_lt(95.0), None, None, None);
        let res = backtest_strategy(&strat, &fall, &zero_cost()).unwrap();
        assert_eq!(res.trades.len(), 1);
        let t = &res.trades[0];
        assert_eq!(t.side, Side::Short);
        assert!(
            t.return_pct > 0.0,
            "short falling must gain, got {}",
            t.return_pct
        );

        let rise = vec![
            c(0, 100.0, 100.0, 90.0, 90.0),
            c(1, 100.0, 101.0, 99.0, 95.0),
            c(2, 100.0, 110.0, 99.0, 108.0),
            c(3, 108.0, 109.0, 107.0, 108.0),
        ];
        let strat_sl = short_strategy(entry_close_lt(95.0), None, Some(2.0), None);
        let res2 = backtest_strategy(&strat_sl, &rise, &zero_cost()).unwrap();
        assert_eq!(res2.trades.len(), 1);
        let t2 = &res2.trades[0];
        assert_eq!(t2.reason, ExitReason::StopLoss);
        assert!(
            (t2.exit_price - 102.0).abs() < 1e-9,
            "short stop must be above entry: exit {} expected 102",
            t2.exit_price
        );
        assert!(
            t2.return_pct < 0.0,
            "short rising must lose, got {}",
            t2.return_pct
        );
    }

    #[test]
    fn invalid_strategy_returns_err() {
        let bad = long_strategy(
            Rules {
                all: vec![],
                any: vec![],
            },
            None,
            None,
            None,
        );
        let candles = vec![c(0, 100.0, 101.0, 99.0, 100.0)];
        let res = backtest_strategy(&bad, &candles, &zero_cost());
        assert!(res.is_err(), "invalid strategy must return Err");
    }

    #[test]
    fn equity_curve_length_equals_candles_and_starts_at_one() {
        let candles: Vec<Candle> = (0..7)
            .map(|i| {
                c(
                    i,
                    100.0 + i as f64,
                    102.0 + i as f64,
                    99.0 + i as f64,
                    101.0 + i as f64,
                )
            })
            .collect();
        let strat = long_strategy(entry_close_gt(100.0), None, None, None);
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
        assert_eq!(res.equity_curve.len(), candles.len());
        assert!(
            (res.equity_curve[0] - 1.0).abs() < 1e-12,
            "equity must start at 1.0, got {}",
            res.equity_curve[0]
        );
    }

    #[test]
    fn crosses_above_number_triggers_once() {
        let closes = [95.0, 99.0, 101.0, 102.0, 103.0];
        let candles: Vec<Candle> = closes
            .iter()
            .enumerate()
            .map(|(i, cl)| c(i as i64, *cl, *cl, *cl, *cl))
            .collect();
        let entry = Rules {
            all: vec![Condition {
                left: ind(IndicatorKind::Close, None),
                op: Op::CrossesAbove,
                right: num(100.0),
            }],
            any: vec![],
        };
        let strat = long_strategy(entry, None, None, None);
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
        assert_eq!(
            res.trades.len(),
            1,
            "expected exactly one trade, got {:?}",
            res.trades
        );
        assert_eq!(res.trades[0].entry_idx, 3);
    }

    #[test]
    fn crosses_below_number_triggers_once_short() {
        let closes = [105.0, 101.0, 99.0, 98.0, 97.0];
        let candles: Vec<Candle> = closes
            .iter()
            .enumerate()
            .map(|(i, cl)| c(i as i64, *cl, *cl, *cl, *cl))
            .collect();
        let entry = Rules {
            all: vec![Condition {
                left: ind(IndicatorKind::Close, None),
                op: Op::CrossesBelow,
                right: num(100.0),
            }],
            any: vec![],
        };
        let strat = short_strategy(entry, None, None, None);
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
        assert_eq!(
            res.trades.len(),
            1,
            "expected exactly one trade, got {:?}",
            res.trades
        );
        assert_eq!(res.trades[0].entry_idx, 3);
    }

    #[test]
    fn take_profit_fills_at_target_level() {
        let candles = vec![
            c(0, 90.0, 115.0, 90.0, 110.0),
            c(1, 100.0, 101.0, 99.0, 100.0),
            c(2, 100.0, 106.0, 99.0, 102.0),
        ];
        let strat = long_strategy(entry_close_gt(100.0), None, None, Some(5.0));
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
        assert_eq!(
            res.trades.len(),
            1,
            "expected one trade, got {:?}",
            res.trades
        );
        let t = &res.trades[0];
        assert_eq!(t.reason, ExitReason::TakeProfit);
        assert!(
            (t.exit_price - 105.0).abs() < 1e-12,
            "exit {} expected exactly 105.0",
            t.exit_price
        );
    }

    #[test]
    fn any_semantics_entry() {
        let entry = Rules {
            all: vec![],
            any: vec![
                Condition {
                    left: ind(IndicatorKind::Close, None),
                    op: Op::Lt,
                    right: num(90.0),
                },
                Condition {
                    left: ind(IndicatorKind::Close, None),
                    op: Op::Gt,
                    right: num(110.0),
                },
            ],
        };
        let closes = [100.0, 85.0, 100.0, 100.0];
        let candles: Vec<Candle> = closes
            .iter()
            .enumerate()
            .map(|(i, cl)| c(i as i64, *cl, *cl, *cl, *cl))
            .collect();
        let strat = long_strategy(entry.clone(), None, None, None);
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
        assert_eq!(
            res.trades.len(),
            1,
            "expected one trade, got {:?}",
            res.trades
        );
        assert_eq!(res.trades[0].entry_idx, 2);
        let flat_closes = [100.0, 100.0, 100.0, 100.0];
        let flat: Vec<Candle> = flat_closes
            .iter()
            .enumerate()
            .map(|(i, cl)| c(i as i64, *cl, *cl, *cl, *cl))
            .collect();
        let strat2 = long_strategy(entry, None, None, None);
        let res2 = backtest_strategy(&strat2, &flat, &zero_cost()).unwrap();
        assert!(
            res2.trades.is_empty(),
            "expected no trades, got {:?}",
            res2.trades
        );
    }

    #[test]
    fn equity_curve_equals_compounded_trade_returns() {
        let closes = [
            95.0, 105.0, 95.0, 105.0, 95.0, 105.0, 95.0, 105.0, 95.0, 105.0,
        ];
        let candles: Vec<Candle> = closes
            .iter()
            .enumerate()
            .map(|(i, cl)| c(i as i64, *cl, *cl + 1.0, *cl - 1.0, *cl))
            .collect();
        let entry = entry_close_gt(100.0);
        let exit_rules = Rules {
            all: vec![],
            any: vec![Condition {
                left: ind(IndicatorKind::Close, None),
                op: Op::Lt,
                right: num(100.0),
            }],
        };
        let strat = long_strategy(entry, Some(exit_rules), None, None);
        let cfg = BacktestConfig::default();
        let res = backtest_strategy(&strat, &candles, &cfg).unwrap();
        let mut expected = 1.0;
        for t in &res.trades {
            expected *= 1.0 + t.return_pct / 100.0;
        }
        let last = *res.equity_curve.last().expect("equity curve non-empty");
        assert!(
            (last - expected).abs() < 1e-9,
            "last equity {last} != compounded {expected}"
        );
    }

    #[test]
    fn no_lookahead_with_indicator_truncation() {
        let n = 40usize;
        let closes: Vec<f64> = (0..n)
            .map(|i| 100.0 + 10.0 * (i as f64 * 0.7).sin())
            .collect();
        let mut candles: Vec<Candle> = Vec::with_capacity(n);
        let mut prev_close: f64 = 100.0;
        for (i, cl) in closes.iter().enumerate() {
            let open: f64 = if i == 0 { 100.0 } else { prev_close };
            let high = open.max(*cl) + 1.0;
            let low = open.min(*cl) - 1.0;
            candles.push(c(i as i64, open, high, low, *cl));
            prev_close = *cl;
        }
        let entry = Rules {
            all: vec![Condition {
                left: ind(IndicatorKind::Close, None),
                op: Op::CrossesAbove,
                right: ind(IndicatorKind::Sma, Some(3)),
            }],
            any: vec![],
        };
        let exit_rules = Rules {
            all: vec![Condition {
                left: ind(IndicatorKind::Close, None),
                op: Op::CrossesBelow,
                right: ind(IndicatorKind::Sma, Some(3)),
            }],
            any: vec![],
        };
        let strat = long_strategy(entry, Some(exit_rules), None, None);
        let cfg = zero_cost();
        let full = backtest_strategy(&strat, &candles, &cfg).unwrap();
        let trunc = backtest_strategy(&strat, &candles[..25], &cfg).unwrap();
        for tt in trunc
            .trades
            .iter()
            .filter(|t| t.reason != ExitReason::EndOfData)
        {
            let found = full.trades.iter().any(|ft| {
                ft.entry_idx == tt.entry_idx
                    && ft.exit_idx == tt.exit_idx
                    && ft.entry_ts == tt.entry_ts
                    && ft.exit_ts == tt.exit_ts
                    && ft.side == tt.side
                    && (ft.entry_price - tt.entry_price).abs() < 1e-12
                    && (ft.exit_price - tt.exit_price).abs() < 1e-12
                    && (ft.return_pct - tt.return_pct).abs() < 1e-12
                    && ft.reason == tt.reason
            });
            assert!(
                found,
                "truncated trade {:?} not found identically in full {:?}",
                tt, full.trades
            );
        }
        assert!(
            trunc
                .trades
                .iter()
                .any(|t| t.reason != ExitReason::EndOfData),
            "expected at least one non-EndOfData trade in truncated run, got {:?}",
            trunc.trades
        );
    }

    #[test]
    fn gap_up_open_beyond_target_fills_take_profit_at_open() {
        let candles = vec![
            c(0, 90.0, 115.0, 90.0, 110.0),
            c(1, 100.0, 101.0, 99.0, 100.0),
            c(2, 105.0, 106.0, 90.0, 95.0),
        ];
        let strat = long_strategy(entry_close_gt(100.0), None, Some(2.0), Some(2.0));
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
        assert!(!res.trades.is_empty());
        let t = &res.trades[0];
        assert_eq!(t.reason, ExitReason::TakeProfit);
        assert!(
            (t.exit_price - 105.0).abs() < 1e-12,
            "exit {} expected open 105.0",
            t.exit_price
        );
    }

    #[test]
    fn same_bar_stop_and_target_exits_at_the_stop() {
        let candles = vec![
            c(0, 90.0, 115.0, 90.0, 110.0),
            c(1, 100.0, 101.0, 99.0, 100.0),
            c(2, 100.0, 106.0, 97.0, 100.0),
        ];
        let strat = long_strategy(entry_close_gt(100.0), None, Some(2.0), Some(2.0));
        let res = backtest_strategy(&strat, &candles, &zero_cost()).unwrap();
        assert_eq!(res.trades.len(), 1);
        let t = &res.trades[0];
        assert_eq!(t.entry_idx, 1);
        assert_eq!(t.exit_idx, 2);
        assert_eq!(t.reason, ExitReason::StopLoss);
        assert!(
            (t.exit_price - 98.0).abs() < 1e-12,
            "exit {} expected stop 98.0",
            t.exit_price
        );
        assert!((t.return_pct - (-2.0)).abs() < 1e-9);
    }
}

#[cfg(test)]
mod orb_recon_tests {
    use crate::backtest::ExitReason;
    use crate::backtest::{Bracket, RunConfig};
    use crate::instrument::Instrument;
    use crate::session::{SessionFilter, MS_PER_MIN};
    use crate::sizing::Sizing;
    use crate::stats::{bars_per_year, compute_stats, compute_v2};
    use crate::strategy::{Orb, OrbConfig};
    use crate::types::Candle;

    fn c(ts: i64, o: f64, h: f64, l: f64, cl: f64) -> Candle {
        Candle {
            ts,
            open: o,
            high: h,
            low: l,
            close: cl,
        }
    }

    #[test]
    fn orb_run_reconciles_by_hand() {
        let candles = vec![
            c(0, 100.0, 101.0, 99.0, 100.0),
            c(MS_PER_MIN, 100.0, 102.0, 99.5, 101.0),
            c(2 * MS_PER_MIN, 101.0, 103.0, 100.0, 102.5),
            c(3 * MS_PER_MIN, 102.5, 104.0, 102.0, 103.5),
            c(4 * MS_PER_MIN, 103.5, 107.0, 103.0, 106.0),
        ];
        let mut orb = Orb::new(OrbConfig::new(2, 0.0, 1).expect("valid"));
        let inst = Instrument::new("TEST", 0.01, 1.0, 0.0, 0.0).expect("valid");
        let bracket = Bracket {
            stop_pct: Some(2.0),
            target_pct: Some(4.0),
            trailing_pct: None,
        };
        let session = SessionFilter::all_day();
        let cfg = RunConfig::new(10_000.0, false).expect("valid");

        let result = crate::backtest::run(
            &mut orb,
            &candles,
            &inst,
            Sizing::FixedQty(1.0),
            &bracket,
            &session,
            &cfg,
        )
        .expect("run");

        let rng = orb.range().expect("range");
        assert!((rng.0 - 102.0).abs() < 1e-9);
        assert!((rng.1 - 99.0).abs() < 1e-9);

        assert_eq!(result.trades.len(), 1);
        let t = &result.trades[0];
        assert_eq!(t.entry_idx, 3);
        assert_eq!(t.exit_idx, 4);
        assert!((t.entry_price - 102.5).abs() < 1e-9);
        assert!((t.exit_price - 106.6).abs() < 1e-9);
        assert_eq!(t.reason, ExitReason::TakeProfit);
        assert!((t.return_pct - 0.041).abs() < 1e-9);

        let expected = [10_000.0, 10_000.0, 10_000.0, 10_001.0, 10_004.1];
        assert_eq!(result.equity_curve.len(), expected.len());
        for (i, (got, want)) in result.equity_curve.iter().zip(expected.iter()).enumerate() {
            assert!(
                (got - want).abs() < 1e-9,
                "equity[{i}]: got {got}, want {want}"
            );
        }

        let bpy = bars_per_year("1m").expect("1m");
        let s = compute_stats(&result, &candles, bpy);
        assert_eq!(s.trades, 1);
        assert_eq!(s.wins, 1);
        assert!((s.total_return_pct - 0.041).abs() < 1e-9);
        assert!((s.max_drawdown_pct - 0.0).abs() < 1e-12);

        let m = compute_v2(&result, &candles, bpy);
        assert_eq!(m.max_win_streak, 1);
        assert_eq!(m.max_loss_streak, 0);
        assert!(m.drawdown_periods.is_empty());
        assert!((m.calmar - 0.0).abs() < 1e-12);
        assert!((m.sortino - 0.0).abs() < 1e-12);
        assert_eq!(m.monthly.len(), 1);
        assert!((m.monthly["1970-01"] - 0.041).abs() < 1e-9);
        assert!((m.by_weekday["Thu"] - 0.041).abs() < 1e-9);
    }

    #[test]
    fn golden_bracket_orders_via_resolve_exit() {
        use crate::types::Candle;
        let candles = vec![
            Candle {
                ts: 0,
                open: 100.0,
                high: 100.0,
                low: 100.0,
                close: 100.0,
            },
            Candle {
                ts: crate::session::MS_PER_MIN,
                open: 100.0,
                high: 101.0,
                low: 97.0,
                close: 100.0,
            },
            Candle {
                ts: 2 * crate::session::MS_PER_MIN,
                open: 100.0,
                high: 100.0,
                low: 100.0,
                close: 100.0,
            },
        ];
        struct Sig2(Vec<Option<crate::strategy::Side>>);
        impl crate::strategy::BarStrategy for Sig2 {
            fn on_bar(&mut self, idx: usize, _candles: &[Candle]) -> Option<crate::strategy::Side> {
                self.0.get(idx).copied().flatten()
            }
        }
        let mut s = Sig2(vec![Some(crate::strategy::Side::Long), None, None]);
        let b = Bracket {
            stop_pct: Some(2.0),
            target_pct: None,
            trailing_pct: None,
        };
        let inst2 = Instrument::new("TEST", 0.01, 1.0, 0.0, 0.0).expect("valid");
        let r = crate::backtest::run(
            &mut s,
            &candles,
            &inst2,
            Sizing::FixedQty(1.0),
            &b,
            &SessionFilter::all_day(),
            &RunConfig::new(10000.0, false).unwrap(),
        )
        .unwrap();
        assert_eq!(r.trades.len(), 1);
        assert_eq!(r.trades[0].entry_idx, 1);
        assert_eq!(r.trades[0].exit_idx, 1);
        assert!((r.trades[0].exit_price - 98.0).abs() < 1e-9);
        assert_eq!(r.trades[0].reason, ExitReason::StopLoss);
        assert!((r.equity_curve[1] - 9_998.0).abs() < 1e-9);
    }
}

#[cfg(test)]
mod golden_sim_tests {
    use crate::backtest::{run, BacktestResult, Bracket, ExitReason, RunConfig, Trade};
    use crate::instrument::Instrument;
    use crate::session::{SessionFilter, MS_PER_MIN};
    use crate::sizing::Sizing;
    use crate::strategy::{BarStrategy, Orb, OrbConfig, Side};
    use crate::types::Candle;

    fn c(ts: i64, o: f64, h: f64, l: f64, cl: f64) -> Candle {
        Candle {
            ts,
            open: o,
            high: h,
            low: l,
            close: cl,
        }
    }

    fn inst() -> Instrument {
        Instrument::new("TEST", 0.01, 1.0, 0.0, 0.0).expect("valid")
    }

    fn cfg(exit_at_session_end: bool) -> RunConfig {
        RunConfig::new(10_000.0, exit_at_session_end).expect("valid")
    }

    struct Scripted {
        entries: Vec<Option<Side>>,
        exits: Vec<bool>,
    }

    impl BarStrategy for Scripted {
        fn on_bar(&mut self, idx: usize, _candles: &[Candle]) -> Option<Side> {
            self.entries.get(idx).copied().flatten()
        }

        fn exit_on_bar(&mut self, idx: usize, _candles: &[Candle]) -> bool {
            self.exits.get(idx).copied().unwrap_or(false)
        }
    }

    fn scripted(entries: &[Option<Side>], exits: &[bool]) -> Scripted {
        Scripted {
            entries: entries.to_vec(),
            exits: exits.to_vec(),
        }
    }

    fn flat() -> Bracket {
        Bracket {
            stop_pct: None,
            target_pct: None,
            trailing_pct: None,
        }
    }

    fn case_take_profit_exit() -> BacktestResult {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 103.0, 100.0, 102.0),
            c(2 * MS_PER_MIN, 102.0, 102.0, 102.0, 102.0),
        ];
        let mut s = scripted(&[Some(Side::Long), None, None], &[]);
        let b = Bracket {
            stop_pct: None,
            target_pct: Some(2.0),
            trailing_pct: None,
        };
        run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &b,
            &SessionFilter::all_day(),
            &cfg(false),
        )
        .unwrap()
    }

    fn case_stop_loss_exit() -> BacktestResult {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 101.0, 97.0, 100.0),
            c(2 * MS_PER_MIN, 100.0, 100.0, 100.0, 100.0),
        ];
        let mut s = scripted(&[Some(Side::Long), None, None], &[]);
        let b = Bracket {
            stop_pct: Some(2.0),
            target_pct: None,
            trailing_pct: None,
        };
        run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &b,
            &SessionFilter::all_day(),
            &cfg(false),
        )
        .unwrap()
    }

    fn case_trailing_exit_after_ratchet() -> BacktestResult {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 110.0, 109.0, 109.0),
            c(2 * MS_PER_MIN, 109.0, 109.0, 108.0, 108.5),
            c(3 * MS_PER_MIN, 108.5, 108.5, 108.5, 108.5),
        ];
        let mut s = scripted(&[Some(Side::Long), None, None, None], &[]);
        let b = Bracket {
            stop_pct: None,
            target_pct: None,
            trailing_pct: Some(1.0),
        };
        run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &b,
            &SessionFilter::all_day(),
            &cfg(false),
        )
        .unwrap()
    }

    fn case_gap_through_trail_fills_at_open() -> BacktestResult {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 111.0, 100.0, 110.0),
            c(2 * MS_PER_MIN, 110.0, 120.0, 110.0, 119.0),
            c(3 * MS_PER_MIN, 105.0, 106.0, 104.0, 104.5),
        ];
        let mut s = scripted(&[Some(Side::Long), None, None, None], &[]);
        let b = Bracket {
            stop_pct: None,
            target_pct: None,
            trailing_pct: Some(10.0),
        };
        run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &b,
            &SessionFilter::all_day(),
            &cfg(false),
        )
        .unwrap()
    }

    fn case_signal_exit() -> BacktestResult {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 100.5, 99.5, 100.0),
            c(2 * MS_PER_MIN, 105.0, 106.0, 104.0, 105.5),
            c(3 * MS_PER_MIN, 105.5, 106.0, 104.0, 105.0),
        ];
        let mut s = scripted(
            &[Some(Side::Long), None, None, None],
            &[false, true, false, false],
        );
        run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &flat(),
            &SessionFilter::all_day(),
            &cfg(false),
        )
        .unwrap()
    }

    fn case_session_end_exit() -> BacktestResult {
        let candles = vec![
            c(570 * MS_PER_MIN, 100.0, 100.4, 99.8, 100.2),
            c(571 * MS_PER_MIN, 100.2, 101.0, 100.0, 100.8),
            c(572 * MS_PER_MIN, 100.8, 102.0, 100.7, 101.5),
            c(600 * MS_PER_MIN, 101.5, 101.6, 101.4, 101.5),
        ];
        let mut s = scripted(&[Some(Side::Long), None, None, None], &[]);
        let session = SessionFilter::new(570, 600).expect("valid");
        run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &flat(),
            &session,
            &cfg(true),
        )
        .unwrap()
    }

    fn case_end_of_data_flatten() -> BacktestResult {
        let candles = vec![
            c(0, 100.0, 100.0, 100.0, 100.0),
            c(MS_PER_MIN, 100.0, 101.0, 99.0, 100.5),
            c(2 * MS_PER_MIN, 100.5, 102.0, 100.0, 101.5),
        ];
        let mut s = scripted(&[Some(Side::Long), None, None], &[]);
        run(
            &mut s,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &flat(),
            &SessionFilter::all_day(),
            &cfg(false),
        )
        .unwrap()
    }

    fn case_orb_run() -> BacktestResult {
        let candles = vec![
            c(0, 100.0, 101.0, 99.0, 100.0),
            c(MS_PER_MIN, 100.0, 102.0, 99.5, 101.0),
            c(2 * MS_PER_MIN, 101.0, 103.0, 100.0, 102.5),
            c(3 * MS_PER_MIN, 102.5, 104.0, 102.0, 103.5),
            c(4 * MS_PER_MIN, 103.5, 107.0, 103.0, 106.0),
        ];
        let mut orb = Orb::new(OrbConfig::new(2, 0.0, 1).expect("valid"));
        let b = Bracket {
            stop_pct: Some(2.0),
            target_pct: Some(4.0),
            trailing_pct: None,
        };
        run(
            &mut orb,
            &candles,
            &inst(),
            Sizing::FixedQty(1.0),
            &b,
            &SessionFilter::all_day(),
            &cfg(false),
        )
        .unwrap()
    }

    fn golden(label: &str, got: &BacktestResult, trades: &[Trade], equity: &[f64]) {
        assert_eq!(got.trades, trades, "{label}: trades");
        assert_eq!(got.equity_curve, equity, "{label}: equity_curve");
    }

    #[test]
    fn golden_take_profit_exit() {
        let expected = [Trade {
            entry_idx: 1,
            exit_idx: 1,
            entry_ts: 60_000,
            exit_ts: 60_000,
            side: Side::Long,
            entry_price: 100.0,
            exit_price: 102.0,
            return_pct: 0.019999999999997797,
            reason: ExitReason::TakeProfit,
        }];
        golden(
            "take_profit",
            &case_take_profit_exit(),
            &expected,
            &[10000.0, 10002.0, 10002.0],
        );
    }

    #[test]
    fn golden_stop_loss_exit() {
        let expected = [Trade {
            entry_idx: 1,
            exit_idx: 1,
            entry_ts: 60_000,
            exit_ts: 60_000,
            side: Side::Long,
            entry_price: 100.0,
            exit_price: 98.0,
            return_pct: -0.019999999999997797,
            reason: ExitReason::StopLoss,
        }];
        golden(
            "stop_loss",
            &case_stop_loss_exit(),
            &expected,
            &[10000.0, 9998.0, 9998.0],
        );
    }

    #[test]
    fn golden_trailing_exit_after_ratchet() {
        let expected = [Trade {
            entry_idx: 1,
            exit_idx: 2,
            entry_ts: 60_000,
            exit_ts: 120_000,
            side: Side::Long,
            entry_price: 100.0,
            exit_price: 108.9,
            return_pct: 0.08900000000000574,
            reason: ExitReason::Trailing,
        }];
        golden(
            "trailing_ratchet",
            &case_trailing_exit_after_ratchet(),
            &expected,
            &[10000.0, 10009.0, 10008.9, 10008.9],
        );
    }

    #[test]
    fn golden_gap_through_trail_fills_at_open() {
        let expected = [Trade {
            entry_idx: 1,
            exit_idx: 3,
            entry_ts: 60_000,
            exit_ts: 180_000,
            side: Side::Long,
            entry_price: 100.0,
            exit_price: 105.0,
            return_pct: 0.04999999999999449,
            reason: ExitReason::Trailing,
        }];
        golden(
            "gap_trail",
            &case_gap_through_trail_fills_at_open(),
            &expected,
            &[10000.0, 10010.0, 10019.0, 10005.0],
        );
    }

    #[test]
    fn golden_signal_exit() {
        let expected = [Trade {
            entry_idx: 1,
            exit_idx: 2,
            entry_ts: 60_000,
            exit_ts: 120_000,
            side: Side::Long,
            entry_price: 100.0,
            exit_price: 105.0,
            return_pct: 0.04999999999999449,
            reason: ExitReason::Signal,
        }];
        golden(
            "signal_exit",
            &case_signal_exit(),
            &expected,
            &[10000.0, 10000.0, 10005.0, 10005.0],
        );
    }

    #[test]
    fn golden_session_end_exit() {
        let expected = [Trade {
            entry_idx: 1,
            exit_idx: 2,
            entry_ts: 34_260_000,
            exit_ts: 34_320_000,
            side: Side::Long,
            entry_price: 100.2,
            exit_price: 101.5,
            return_pct: 0.012999999999996348,
            reason: ExitReason::SessionEnd,
        }];
        golden(
            "session_end",
            &case_session_end_exit(),
            &expected,
            &[10000.0, 10000.6, 10001.3, 10001.3],
        );
    }

    #[test]
    fn golden_end_of_data_flatten() {
        let expected = [Trade {
            entry_idx: 1,
            exit_idx: 2,
            entry_ts: 60_000,
            exit_ts: 120_000,
            side: Side::Long,
            entry_price: 100.0,
            exit_price: 101.5,
            return_pct: 0.01500000000000945,
            reason: ExitReason::EndOfData,
        }];
        golden(
            "end_of_data",
            &case_end_of_data_flatten(),
            &expected,
            &[10000.0, 10000.5, 10001.5],
        );
    }

    #[test]
    fn golden_orb_run() {
        let expected = [Trade {
            entry_idx: 3,
            exit_idx: 4,
            entry_ts: 180_000,
            exit_ts: 240_000,
            side: Side::Long,
            entry_price: 102.5,
            exit_price: 106.60000000000001,
            return_pct: 0.041000000000002146,
            reason: ExitReason::TakeProfit,
        }];
        golden(
            "orb",
            &case_orb_run(),
            &expected,
            &[10000.0, 10000.0, 10000.0, 10001.0, 10004.1],
        );
    }
}

#[cfg(test)]
mod resume_tests {
    use super::*;
    use crate::session::{MS_PER_DAY, MS_PER_MIN};
    use crate::strategy::{
        Condition, IndicatorKind, IndicatorRef, Op, Operand, Orb, OrbConfig, Rules, RulesStrategy,
        Strategy,
    };

    fn json<T: serde::Serialize>(v: &T) -> String {
        serde_json::to_string(v).expect("serialize")
    }

    fn unjson<T: serde::de::DeserializeOwned>(text: &str) -> T {
        serde_json::from_str(text).expect("deserialize")
    }

    fn num(v: f64) -> Operand {
        Operand::Number(v)
    }

    fn close() -> Operand {
        Operand::Indicator(IndicatorRef {
            ind: IndicatorKind::Close,
            period: None,
        })
    }

    fn rule(op: Op, rhs: f64) -> Rules {
        Rules {
            all: vec![Condition {
                left: close(),
                op,
                right: num(rhs),
            }],
            any: vec![],
        }
    }

    /// O/H/L derived from the previous close so the bars carry the kind of
    /// non-round binary fractions a real feed produces.
    fn candles_from(closes: &[f64]) -> Vec<Candle> {
        closes
            .iter()
            .enumerate()
            .map(|(i, &cl)| {
                let open = if i == 0 { cl } else { closes[i - 1] };
                Candle {
                    ts: i as i64 * MS_PER_MIN,
                    open,
                    high: open.max(cl) + 0.4,
                    low: open.min(cl) - 0.6,
                    close: cl,
                }
            })
            .collect()
    }

    /// Same wiring as `backtest_strategy`, except the bracket also carries a
    /// trailing stop so the resume test can hit a mid-ratchet split.
    fn rules_setup(spec: &Strategy) -> (Instrument, Sizing, Bracket, SessionFilter, RunConfig) {
        let cfg = BacktestConfig {
            fee_bps: 5.0,
            slippage_bps: 2.0,
        };
        let (instrument, sizing, mut bracket, session, run_cfg) =
            crate::strategy::rules_v2_setup(spec, &cfg).expect("setup");
        bracket.trailing_pct = Some(6.0);
        (instrument, sizing, bracket, session, run_cfg)
    }

    fn ctx_of<'a>(
        instrument: &'a Instrument,
        sizing: Sizing,
        bracket: &'a Bracket,
        session: &'a SessionFilter,
        run_cfg: &'a RunConfig,
    ) -> SimCtx<'a> {
        SimCtx {
            instrument,
            cost_rate: instrument.cost_rate(),
            pv: instrument.point_value,
            sizing,
            bracket,
            session,
            exit_at_session_end: run_cfg.exit_at_session_end,
            risk: None,
        }
    }

    /// Drives `step` over every bar and then `finish`, exactly like `run`.
    fn drive(
        state: &mut SimState,
        strategy: &mut dyn BarStrategy,
        candles: &[Candle],
        from: usize,
        ctx: &SimCtx,
    ) -> Result<(), BacktestError> {
        for i in from..candles.len() {
            state.step(i, candles, strategy, ctx, false, None)?;
        }
        let last = candles.len() - 1;
        state.finish(last, &candles[last], ctx);
        Ok(())
    }

    #[derive(Default)]
    struct Coverage {
        splits: usize,
        mid_position: usize,
        ratcheted_trail: usize,
        pending_entry: usize,
        pending_exit: usize,
    }

    impl Coverage {
        fn observe(&mut self, state: &SimState) {
            self.splits += 1;
            if let Some(pos) = &state.position {
                self.mid_position += 1;
                if pos.trail.peak() > pos.entry_raw && pos.trail.level() != pos.trail.base() {
                    self.ratcheted_trail += 1;
                }
            }
            if state.pending.is_some() {
                self.pending_entry += 1;
            }
            if state.pending_exit.is_set() {
                self.pending_exit += 1;
            }
        }

        fn assert_exercised(&self) {
            assert!(self.mid_position > 0, "no split landed mid-position");
            assert!(
                self.ratcheted_trail > 0,
                "no split landed after a trail ratchet"
            );
            assert!(
                self.pending_entry > 0,
                "no split landed with a pending entry armed"
            );
            assert!(
                self.pending_exit > 0,
                "no split landed with a signal exit armed"
            );
        }
    }

    fn assert_same(got: &BacktestResult, want: &BacktestResult, k: usize) {
        assert_eq!(
            got.trades, want.trades,
            "trades diverged after resuming at bar {k}"
        );
        assert_eq!(
            got.equity_curve.len(),
            want.equity_curve.len(),
            "equity length diverged after resuming at bar {k}"
        );
        assert_eq!(
            got.equity_curve, want.equity_curve,
            "equity curve diverged after resuming at bar {k}"
        );
    }

    #[test]
    fn rules_strategy_resume_matches_full_run_at_every_split() {
        let spec = Strategy {
            asset: "TEST".to_string(),
            timeframe: "1h".to_string(),
            side: Side::Long,
            entry: rule(Op::Gt, 101.5),
            exit: Some(rule(Op::Lt, 102.5)),
            stop_loss_pct: None,
            take_profit_pct: None,
            claim: None,
        };
        spec.validate().expect("valid");
        let closes = [
            100.0, 101.6, 103.2, 104.4, 102.0, 102.0, 100.4, 101.8, 103.0, 103.9, 102.1, 102.1,
            100.9, 102.3, 103.6, 104.2, 102.4, 102.4, 100.2, 99.0, 99.6, 100.0,
        ];
        let candles = candles_from(&closes);
        let (instrument, sizing, bracket, session, run_cfg) = rules_setup(&spec);
        let ctx = ctx_of(&instrument, sizing, &bracket, &session, &run_cfg);

        let mut full_strategy = RulesStrategy::new(&spec);
        let full = run(
            &mut full_strategy,
            &candles,
            &instrument,
            sizing,
            &bracket,
            &session,
            &run_cfg,
        )
        .expect("full run");

        let mut by_step = SimState::new(run_cfg.initial_equity, candles.len());
        let mut by_step_strategy = RulesStrategy::new(&spec);
        drive(&mut by_step, &mut by_step_strategy, &candles, 0, &ctx).expect("step run");
        assert_eq!(
            BacktestResult {
                trades: by_step.trades.clone(),
                equity_curve: by_step.equity_curve.clone(),
            },
            full,
            "the step harness must reproduce run() before it can prove anything"
        );
        assert!(!full.trades.is_empty(), "corpus must trade");

        let mut coverage = Coverage::default();
        for k in 0..=candles.len() {
            let mut head_state = SimState::new(run_cfg.initial_equity, candles.len());
            let mut head_strategy = RulesStrategy::new(&spec);
            for i in 0..k {
                head_state
                    .step(i, &candles, &mut head_strategy, &ctx, false, None)
                    .expect("head step");
            }
            coverage.observe(&head_state);

            let state_text = json(&head_state);
            let strategy_text = json(&head_strategy);
            assert!(
                state_text.len() > 2 && strategy_text.len() > 2,
                "snapshot at bar {k} must carry something"
            );

            let mut tail_state: SimState = unjson(&state_text);
            let mut tail_strategy: RulesStrategy = unjson(&strategy_text);
            drive(&mut tail_state, &mut tail_strategy, &candles, k, &ctx).expect("tail step");

            let resumed = BacktestResult {
                trades: tail_state.trades,
                equity_curve: tail_state.equity_curve,
            };
            assert_same(&resumed, &full, k);
            assert_eq!(tail_state.equity, by_step.equity, "final equity at bar {k}");
        }
        assert_eq!(coverage.splits, candles.len() + 1);
        coverage.assert_exercised();
    }

    #[test]
    fn orb_resume_matches_full_run_after_the_range_is_ready() {
        let cfg = OrbConfig::new(2, 0.1, 1).expect("valid");
        let day = MS_PER_DAY;
        let candles = vec![
            Candle {
                ts: day,
                open: 100.0,
                high: 101.0,
                low: 99.0,
                close: 100.0,
            },
            Candle {
                ts: day + MS_PER_MIN,
                open: 100.0,
                high: 102.0,
                low: 99.5,
                close: 101.0,
            },
            Candle {
                ts: day + 2 * MS_PER_MIN,
                open: 101.0,
                high: 103.0,
                low: 100.0,
                close: 102.5,
            },
            Candle {
                ts: day + 3 * MS_PER_MIN,
                open: 102.5,
                high: 104.0,
                low: 102.0,
                close: 103.5,
            },
            Candle {
                ts: day + 4 * MS_PER_MIN,
                open: 103.5,
                high: 107.0,
                low: 103.0,
                close: 106.0,
            },
            Candle {
                ts: 2 * day,
                open: 106.0,
                high: 106.5,
                low: 104.0,
                close: 105.0,
            },
            Candle {
                ts: 2 * day + MS_PER_MIN,
                open: 105.0,
                high: 108.0,
                low: 104.5,
                close: 107.5,
            },
            Candle {
                ts: 2 * day + 2 * MS_PER_MIN,
                open: 107.5,
                high: 109.5,
                low: 107.0,
                close: 109.0,
            },
            Candle {
                ts: 2 * day + 3 * MS_PER_MIN,
                open: 109.0,
                high: 110.0,
                low: 108.0,
                close: 108.5,
            },
        ];
        let instrument = Instrument::new("TEST", 0.01, 1.0, 5.0, 2.0).expect("valid");
        let sizing = Sizing::FixedQty(1.0);
        let bracket = Bracket {
            stop_pct: Some(2.0),
            target_pct: Some(4.0),
            trailing_pct: None,
        };
        let session = SessionFilter::all_day();
        let run_cfg = RunConfig::new(10_000.0, false).expect("valid");
        let ctx = ctx_of(&instrument, sizing, &bracket, &session, &run_cfg);

        let full = run(
            &mut Orb::new(cfg),
            &candles,
            &instrument,
            sizing,
            &bracket,
            &session,
            &run_cfg,
        )
        .expect("full run");
        assert!(!full.trades.is_empty(), "corpus must trade");

        let mut resumed_splits = 0;
        let mut skipped = 0;
        for k in 0..=candles.len() {
            let mut head = Orb::new(cfg);
            let mut head_state = SimState::new(run_cfg.initial_equity, candles.len());
            for i in 0..k {
                head_state
                    .step(i, &candles, &mut head, &ctx, false, None)
                    .expect("head step");
            }
            // Before the opening range is complete the ORB keeps its bounds as
            // +/-infinity, which JSON cannot carry (see the module report).
            if head.range().is_none() {
                skipped += 1;
                continue;
            }
            resumed_splits += 1;

            let state_text = json(&head_state);
            let strategy_text = json(&head);
            let mut tail_state: SimState = unjson(&state_text);
            let mut tail_strategy: Orb = unjson(&strategy_text);
            drive(&mut tail_state, &mut tail_strategy, &candles, k, &ctx).expect("tail step");

            let resumed = BacktestResult {
                trades: tail_state.trades,
                equity_curve: tail_state.equity_curve,
            };
            assert_same(&resumed, &full, k);
        }
        assert!(
            resumed_splits >= 5,
            "only {resumed_splits} splits were resumable ({skipped} skipped)"
        );
    }

    #[test]
    fn f64_bits_survive_json_round_trip() {
        let awkward = [
            106.60000000000001,
            0.1 + 0.2,
            -0.0,
            1.0 / 3.0,
            f64::MIN_POSITIVE,
            1e-300,
            4.1e-2,
            1.0 + f64::EPSILON,
        ];
        assert_ne!(
            106.60000000000001f64.to_bits(),
            106.6f64.to_bits(),
            "the awkward value must differ from the round one, else the test proves nothing"
        );
        for v in awkward {
            let text = serde_json::to_string(&v).expect("serialize");
            let back: f64 = serde_json::from_str(&text).expect("deserialize");
            assert_eq!(
                back.to_bits(),
                v.to_bits(),
                "{v:?} round-tripped through {text} as {back:?}"
            );
        }
    }
}
