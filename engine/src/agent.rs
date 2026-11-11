use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::backtest::{BacktestConfig, PositionView, Simulator, Trade};
#[cfg(test)]
use crate::risk::RiskState;
use crate::risk::{HaltLimit, RiskLimits};
use crate::strategy::{RulesStrategy, Strategy};
use crate::types::Candle;

pub const DEFAULT_INITIAL_EQUITY: f64 = 10_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Draft,
    Running,
    Stopped,
}

impl AgentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentStatus::Draft => "draft",
            AgentStatus::Running => "running",
            AgentStatus::Stopped => "stopped",
        }
    }

    pub fn parse(value: &str) -> Option<AgentStatus> {
        match value {
            "draft" => Some(AgentStatus::Draft),
            "running" => Some(AgentStatus::Running),
            "stopped" => Some(AgentStatus::Stopped),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("agent name must not be empty")]
    EmptyName,
    #[error("initial_equity must be > 0, got {0}")]
    BadEquity(f64),
    #[error("no closed candles to start the agent")]
    NoCandles,
    #[error("candles must be strictly ascending by timestamp")]
    UnorderedCandles,
    #[error("agent snapshot is corrupt: {0}")]
    Snapshot(String),
    #[error("symbol {0} is not supported: {1}")]
    BadSymbol(String, String),
    #[error("unknown timeframe: {0}")]
    BadTimeframe(String),
    #[error(transparent)]
    Strategy(#[from] crate::strategy::StrategyError),
    #[error(transparent)]
    Backtest(#[from] crate::backtest::BacktestError),
    #[error(transparent)]
    Db(#[from] crate::db::DbError),
}

/// Everything needed to continue a paper agent exactly where it stopped:
/// the candle history (indicators and trade indices), the rules state
/// (armed/was_open), and the simulator state (cash, position, trades).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSnapshot {
    candles: Vec<Candle>,
    strategy: RulesStrategy,
    #[serde(serialize_with = "serialize_agent_simulator")]
    simulator: Simulator,
}

fn serialize_agent_simulator<S>(simulator: &Simulator, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    simulator.serialize_agent_snapshot(serializer)
}

#[derive(Debug, Clone, Serialize)]
pub struct Advance {
    pub new_candles: usize,
    pub equity: f64,
    pub last_price: f64,
    pub last_ts: i64,
    pub trades: Vec<Trade>,
    pub position: Option<PositionView>,
    #[serde(skip)]
    pub events: Vec<crate::backtest::DecisionEvent>,
    #[serde(skip)]
    pub equity_points: Vec<EquityPoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EquityPoint {
    pub bar_ts: i64,
    pub equity: f64,
}

pub fn validate_new(name: &str, spec: &Strategy, initial_equity: f64) -> Result<(), AgentError> {
    if name.trim().is_empty() {
        return Err(AgentError::EmptyName);
    }
    if !initial_equity.is_finite() || initial_equity <= 0.0 {
        return Err(AgentError::BadEquity(initial_equity));
    }
    spec.validate()?;
    crate::feeds::validate_symbol(&spec.asset)
        .map_err(|e| AgentError::BadSymbol(spec.asset.clone(), e.to_string()))?;
    if crate::feeds::interval_ms(&spec.timeframe).is_none() {
        return Err(AgentError::BadTimeframe(spec.timeframe.clone()));
    }
    Ok(())
}

pub fn new_agent_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("ag-{ms:x}-{n:x}")
}

impl AgentSnapshot {
    /// Warm the agent up on closed history. The last bar's decision is kept
    /// pending, so the first live bar fills it at that bar's open.
    pub fn warm_up(
        spec: &Strategy,
        candles: &[Candle],
        cfg: &BacktestConfig,
        initial_equity: f64,
        risk: RiskLimits,
    ) -> Result<AgentSnapshot, AgentError> {
        if candles.is_empty() {
            return Err(AgentError::NoCandles);
        }
        ensure_ordered(candles)?;
        let mut simulator = Simulator::from_strategy(spec, cfg, initial_equity, Some(risk))?;
        let mut strategy = RulesStrategy::new(spec);
        for i in 0..candles.len() {
            simulator.step_live(i, candles, &mut strategy)?;
        }
        Ok(AgentSnapshot {
            candles: candles.to_vec(),
            strategy,
            simulator,
        })
    }

    /// Append only candles the agent has not seen, then step each once. A
    /// candle at or before the last stored timestamp is ignored, which makes
    /// a retried advance idempotent.
    pub fn advance(&mut self, new_candles: &[Candle]) -> Result<Advance, AgentError> {
        let last_ts = self.last_ts();
        let fresh: Vec<Candle> = new_candles
            .iter()
            .copied()
            .filter(|c| c.ts > last_ts)
            .collect();
        if fresh.is_empty() {
            return Ok(self.no_op());
        }
        ensure_ordered(&fresh)?;
        let start = self.candles.len();
        let before = self.simulator.trades().len();
        self.candles.extend(fresh.iter().copied());
        let mut events = Vec::new();
        let mut equity_points = Vec::with_capacity(fresh.len());
        for i in start..self.candles.len() {
            events.extend(self.simulator.step_live_with_events(
                i,
                &self.candles,
                &mut self.strategy,
            )?);
            equity_points.push(EquityPoint {
                bar_ts: self.candles[i].ts,
                equity: self.simulator.current_equity(),
            });
        }
        Ok(Advance {
            new_candles: fresh.len(),
            equity: self.equity(),
            last_price: self.last_price(),
            last_ts: self.last_ts(),
            trades: self.simulator.trades()[before..].to_vec(),
            position: self.simulator.position(),
            events,
            equity_points,
        })
    }

    fn no_op(&self) -> Advance {
        Advance {
            new_candles: 0,
            equity: self.equity(),
            last_price: self.last_price(),
            last_ts: self.last_ts(),
            trades: Vec::new(),
            position: self.simulator.position(),
            events: Vec::new(),
            equity_points: Vec::new(),
        }
    }

    pub fn to_json(&self) -> Result<String, AgentError> {
        serde_json::to_string(self).map_err(|e| AgentError::Snapshot(e.to_string()))
    }

    pub fn from_json(text: &str) -> Result<AgentSnapshot, AgentError> {
        let mut snapshot: AgentSnapshot =
            serde_json::from_str(text).map_err(|e| AgentError::Snapshot(e.to_string()))?;
        snapshot.simulator.restore_last_bar_equity();
        Ok(snapshot)
    }

    pub fn equity(&self) -> f64 {
        self.simulator.current_equity()
    }

    pub fn last_ts(&self) -> i64 {
        self.candles.last().map(|c| c.ts).unwrap_or(0)
    }

    pub fn last_price(&self) -> f64 {
        self.candles.last().map(|c| c.close).unwrap_or(0.0)
    }

    pub fn candle_count(&self) -> usize {
        self.candles.len()
    }

    pub fn trades(&self) -> &[Trade] {
        self.simulator.trades()
    }

    pub fn equity_curve(&self) -> &[f64] {
        self.simulator.equity_curve()
    }

    pub fn equity_points(&self) -> Vec<EquityPoint> {
        self.candles
            .iter()
            .zip(self.simulator.equity_curve())
            .map(|(candle, &equity)| EquityPoint {
                bar_ts: candle.ts,
                equity,
            })
            .collect()
    }

    pub fn position(&self) -> Option<PositionView> {
        self.simulator.position()
    }

    pub fn risk_limits(&self) -> Option<RiskLimits> {
        self.simulator.risk_limits()
    }

    /// Rehydrate caps on a snapshot written before agents enforced risk.
    pub fn set_risk_limits(&mut self, limits: RiskLimits) {
        self.simulator.set_risk(limits);
    }

    #[cfg(test)]
    pub fn risk_state(&self) -> RiskState {
        self.simulator.risk_state()
    }

    pub fn halt(&self) -> Option<HaltLimit> {
        self.simulator.halt()
    }

    /// Re-baseline the peak and the day at the current equity, and clear the
    /// halt, so the agent can trade again from here.
    pub fn reset_risk(&mut self) {
        let equity = self.equity();
        let ts = self.last_ts();
        self.simulator.reset_risk(equity, ts);
    }

    pub fn kill_switch(&mut self) {
        self.simulator.kill_switch();
    }

    pub fn clear_kill_switch(&mut self) {
        self.simulator.clear_kill_switch();
    }
}

fn ensure_ordered(candles: &[Candle]) -> Result<(), AgentError> {
    if candles.windows(2).any(|w| w[0].ts >= w[1].ts) {
        return Err(AgentError::UnorderedCandles);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::MS_PER_MIN;
    use crate::strategy::{Condition, IndicatorKind, IndicatorRef, Op, Operand, Rules, Side};

    fn c(ts: i64, o: f64, h: f64, l: f64, cl: f64) -> Candle {
        Candle {
            ts,
            open: o,
            high: h,
            low: l,
            close: cl,
        }
    }

    fn spec(entry_gt: f64) -> Strategy {
        let close = Operand::Indicator(IndicatorRef {
            ind: IndicatorKind::Close,
            period: None,
        });
        Strategy {
            asset: "BTCUSDT".to_string(),
            timeframe: "1m".to_string(),
            side: Side::Long,
            entry: Rules {
                all: vec![Condition {
                    left: close,
                    op: Op::Gt,
                    right: Operand::Number(entry_gt),
                }],
                any: vec![],
            },
            exit: None,
            stop_loss_pct: None,
            take_profit_pct: None,
            claim: None,
        }
    }

    fn bars(closes: &[f64]) -> Vec<Candle> {
        closes
            .iter()
            .enumerate()
            .map(|(i, &cl)| {
                let open = if i == 0 { cl } else { closes[i - 1] };
                c(
                    i as i64 * MS_PER_MIN,
                    open,
                    open.max(cl) + 0.5,
                    open.min(cl) - 0.5,
                    cl,
                )
            })
            .collect()
    }

    fn limits() -> RiskLimits {
        RiskLimits {
            max_position_pct: 100.0,
            max_daily_loss_pct: 10.0,
            max_drawdown_pct: 20.0,
        }
    }

    /// Enters long on the first bar that closes above 101.5 and keeps the
    /// position, so a later crash is a mark-to-market drawdown.
    fn agent_up_then_crash() -> (AgentSnapshot, Vec<Candle>) {
        let all = bars(&[100.0, 102.0, 104.0, 130.0, 52.0, 52.0]);
        let agent = AgentSnapshot::warm_up(
            &spec(101.5),
            &all[..4],
            &BacktestConfig::default(),
            10_000.0,
            limits(),
        )
        .expect("warm up");
        (agent, all)
    }

    #[test]
    fn warm_up_then_advance_matches_one_full_run() {
        let closes = [100.0, 102.0, 101.0, 104.0, 103.0, 106.0, 105.0, 108.0];
        let all = bars(&closes);
        let spec = spec(101.5);
        let cfg = BacktestConfig::default();

        let mut agent =
            AgentSnapshot::warm_up(&spec, &all[..4], &cfg, 10_000.0, limits()).expect("warm up");
        let first = agent.advance(&all[4..]).expect("advance");
        assert!(first.new_candles > 0);

        let full = crate::backtest::backtest_strategy(&spec, &all, &cfg).expect("full run");
        // A backtest force-closes at the last bar; a paper agent keeps the
        // position open, so only closed trades are comparable.
        let closed: Vec<crate::backtest::Trade> = full
            .trades
            .iter()
            .filter(|t| t.reason != crate::backtest::ExitReason::EndOfData)
            .cloned()
            .collect();
        assert_eq!(agent.trades(), closed.as_slice());
        assert!(agent.position().is_some());
        assert_eq!(agent.equity_curve().len(), all.len());
    }

    #[test]
    fn a_signal_on_the_newest_bar_fills_at_the_next_open() {
        let all = bars(&[100.0, 102.0, 105.0, 105.0]);
        let spec = spec(101.5);
        let mut agent = AgentSnapshot::warm_up(
            &spec,
            &all[..2],
            &BacktestConfig::default(),
            10_000.0,
            limits(),
        )
        .expect("warm up");
        // The second bar closed above the threshold, so the entry is pending
        // and must fill at the third bar's open, never at the signal close.
        assert_eq!(agent.trades().len(), 0);
        let out = agent.advance(&all[2..3]).expect("advance");
        assert_eq!(out.new_candles, 1);
        assert_eq!(out.trades.len(), 0);
        assert!(out.position.is_some());
        // The signal came from bar 1's close; the fill is bar 2's open.
        assert_eq!(out.position.as_ref().expect("position").entry_price, 102.0);
    }

    #[test]
    fn advance_ignores_seen_and_out_of_order_candles() {
        let all = bars(&[100.0, 101.0, 102.0]);
        let mut agent = AgentSnapshot::warm_up(
            &spec(101.5),
            &all[..2],
            &BacktestConfig::default(),
            10_000.0,
            limits(),
        )
        .expect("warm up");
        let before = agent.candle_count();
        let out = agent
            .advance(&[all[0], all[1]])
            .expect("duplicates are ignored");
        assert_eq!(out.new_candles, 0);
        assert_eq!(agent.candle_count(), before);
        assert_eq!(agent.last_ts(), all[1].ts);
    }

    #[test]
    fn advance_rejects_new_candles_that_are_not_ascending() {
        let all = bars(&[100.0, 101.0, 102.0]);
        let mut agent = AgentSnapshot::warm_up(
            &spec(101.5),
            &all[..2],
            &BacktestConfig::default(),
            10_000.0,
            limits(),
        )
        .expect("warm up");
        let err = agent
            .advance(&[all[2], all[2]])
            .expect_err("duplicate timestamps must fail");
        assert!(matches!(err, AgentError::UnorderedCandles));
    }

    #[test]
    fn snapshot_round_trips_and_continues_identically() {
        let all = bars(&[100.0, 102.0, 101.0, 104.0, 103.0, 106.0, 105.0, 108.0]);
        let spec = spec(101.5);
        let cfg = BacktestConfig::default();
        let mut agent =
            AgentSnapshot::warm_up(&spec, &all[..5], &cfg, 10_000.0, limits()).expect("warm up");

        let text = agent.to_json().expect("serialize");
        let value: serde_json::Value = serde_json::from_str(&text).expect("snapshot json");
        assert!(value["simulator"]["state"].get("equity_curve").is_none());
        let mut resumed = AgentSnapshot::from_json(&text).expect("deserialize");
        agent.advance(&all[5..]).expect("advance original");
        resumed.advance(&all[5..]).expect("advance resumed");

        assert_eq!(agent.trades(), resumed.trades());
        assert_eq!(agent.equity(), resumed.equity());
        assert_eq!(resumed.equity_curve().len(), all.len() - 5);
    }

    #[test]
    fn validate_new_rejects_bad_input() {
        let good = spec(100.0);
        assert!(matches!(
            validate_new("  ", &good, 10_000.0),
            Err(AgentError::EmptyName)
        ));
        assert!(matches!(
            validate_new("agent", &good, 0.0),
            Err(AgentError::BadEquity(0.0))
        ));
        let mut bad_symbol = good.clone();
        bad_symbol.asset = "btc/usd".to_string();
        assert!(matches!(
            validate_new("agent", &bad_symbol, 10_000.0),
            Err(AgentError::BadSymbol(_, _))
        ));
        let mut bad_timeframe = good.clone();
        bad_timeframe.timeframe = "7m".to_string();
        assert!(matches!(
            validate_new("agent", &bad_timeframe, 10_000.0),
            Err(AgentError::BadTimeframe(_))
        ));
        assert!(validate_new("agent", &good, 10_000.0).is_ok());
    }

    #[test]
    fn resumed_snapshot_keeps_its_peak_and_still_trips_drawdown() {
        let (agent, all) = agent_up_then_crash();
        let peak = agent.risk_state().peak();
        assert!(peak > 12_000.0, "the warm-up must run up to a peak: {peak}");
        assert_eq!(agent.halt(), None);

        let text = agent.to_json().expect("serialize");
        let mut resumed = AgentSnapshot::from_json(&text).expect("deserialize");
        assert_eq!(
            resumed.risk_state().peak(),
            peak,
            "peak must survive the snapshot"
        );

        // The crash bar closes 60% below the peak, well past the 20% drawdown.
        // The halt is decided on that close, so the position flattens at the
        // next bar's open, never on the bar that decided it.
        let out = resumed.advance(&all[4..]).expect("advance");
        assert_eq!(out.trades.len(), 1, "the halt must flatten the position");
        assert_eq!(out.trades[0].reason, crate::backtest::ExitReason::RiskHalt);
        assert_eq!(out.trades[0].exit_price, all[5].open);
        assert_eq!(resumed.halt(), Some(HaltLimit::Drawdown));
    }

    #[test]
    fn a_halt_with_an_open_position_flattens_on_the_next_advance_then_stays_halted() {
        // Warm up on history whose last bar crashes through the drawdown cap
        // while a long is open: the halt is decided on that close, so the
        // flatten is left pending across the snapshot round-trip.
        let all = bars(&[100.0, 102.0, 104.0, 130.0, 52.0, 52.0, 52.0]);
        let agent = AgentSnapshot::warm_up(
            &spec(101.5),
            &all[..5],
            &BacktestConfig::default(),
            10_000.0,
            limits(),
        )
        .expect("warm up");
        assert!(
            agent.position().is_some(),
            "the crash leaves a position open"
        );
        assert_eq!(agent.halt(), Some(HaltLimit::Drawdown));

        let text = agent.to_json().expect("serialize");
        let mut resumed = AgentSnapshot::from_json(&text).expect("deserialize");
        assert!(
            resumed.position().is_some(),
            "the scheduled flatten survives the snapshot"
        );
        assert_eq!(resumed.halt(), Some(HaltLimit::Drawdown));

        // The next advance still executes the scheduled flatten at the next
        // open, even though the agent is already halted.
        let out = resumed
            .advance(&all[5..6])
            .expect("advance must drain the scheduled flatten");
        assert_eq!(out.trades.len(), 1, "the halt flattens the open position");
        assert_eq!(out.trades[0].reason, crate::backtest::ExitReason::RiskHalt);
        assert_eq!(out.trades[0].exit_price, all[5].open);
        assert!(resumed.position().is_none());

        // Only after the flatten is drained do further advances leave the
        // agent halted: no new entry, no new trade, halt stays in force.
        let out = resumed
            .advance(&all[6..])
            .expect("an advance on a halted agent is a no-op");
        assert_eq!(out.new_candles, 1);
        assert!(out.trades.is_empty(), "no trades while halted");
        assert!(resumed.position().is_none());
        assert_eq!(resumed.halt(), Some(HaltLimit::Drawdown));
        assert_eq!(resumed.trades().len(), 1, "only the RiskHalt trade stands");
    }

    #[test]
    fn reset_re_baselines_and_does_not_immediately_re_trip() {
        let (mut agent, all) = agent_up_then_crash();
        agent.advance(&all[4..5]).expect("crash");
        assert_eq!(agent.halt(), Some(HaltLimit::Drawdown));

        agent.reset_risk();
        assert_eq!(agent.halt(), None);
        let state = agent.risk_state();
        assert_eq!(state.peak(), agent.equity());
        assert_eq!(state.day_start(), agent.equity());

        // The next bar closes at the same crushed price and must not re-halt.
        let out = agent.advance(&all[5..]).expect("advance");
        assert_eq!(out.new_candles, 1);
        assert_eq!(agent.halt(), None);
        assert!(!agent.trades().is_empty());
    }

    #[test]
    fn kill_switch_cancels_a_pending_entry_and_schedules_the_flatten() {
        // The last warm-up bar closes above the entry threshold, so an entry
        // is queued for the next open.
        let all = bars(&[100.0, 102.0, 104.0]);
        let mut agent = AgentSnapshot::warm_up(
            &spec(101.5),
            &all[..2],
            &BacktestConfig::default(),
            10_000.0,
            limits(),
        )
        .expect("warm up");
        assert!(agent.position().is_none());

        agent.kill_switch();
        assert_eq!(agent.halt(), Some(HaltLimit::KillSwitch));
        let out = agent.advance(&all[2..]).expect("advance");
        assert!(out.position.is_none(), "the queued entry must be cancelled");
        assert!(
            out.trades.is_empty(),
            "nothing was open, nothing to flatten"
        );

        // A signal while halted must not re-arm an entry.
        let more = bars(&[100.0, 102.0, 104.0, 120.0]);
        let out = agent.advance(&more[3..]).expect("advance");
        assert!(out.position.is_none());
        assert_eq!(agent.halt(), Some(HaltLimit::KillSwitch));
    }

    #[test]
    fn kill_switch_flattens_an_open_position_at_the_next_open() {
        let (mut agent, all) = agent_up_then_crash();
        assert!(agent.position().is_some());
        agent.kill_switch();
        let out = agent.advance(&all[4..5]).expect("advance");
        assert_eq!(out.trades.len(), 1);
        assert_eq!(out.trades[0].reason, crate::backtest::ExitReason::RiskHalt);
        // Flat at the next open, so the fill price is that bar's open.
        assert_eq!(out.trades[0].exit_price, all[4].open);
        assert!(out.position.is_none());
    }

    #[test]
    fn a_snapshot_from_before_risk_state_still_loads() {
        let (agent, _) = agent_up_then_crash();
        let mut value: serde_json::Value =
            serde_json::from_str(&agent.to_json().expect("serialize")).expect("json");
        let state = value
            .get_mut("simulator")
            .and_then(|s| s.get_mut("state"))
            .expect("simulator state");
        let state = state.as_object_mut().expect("state object");
        state.remove("risk");
        // Older snapshots stored the pending exit as a bare bool.
        state.insert("pending_exit".to_string(), serde_json::Value::Bool(true));

        let loaded = AgentSnapshot::from_json(&value.to_string()).expect("old snapshot loads");
        assert_eq!(loaded.risk_state(), RiskState::default());
        assert_eq!(loaded.halt(), None);
        assert_eq!(loaded.simulator.risk_limits(), Some(limits()));
    }
}
