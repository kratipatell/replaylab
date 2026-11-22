use crate::{
    agent::{AgentSnapshot, EquityPoint, DEFAULT_INITIAL_EQUITY},
    backtest::{backtest_strategy, BacktestConfig, DecisionEvent},
    db::{Db, DbError, DemoAgentSeed},
    risk::RiskLimits,
    templates::{TemplateCatalog, TemplateCatalogError},
    types::Candle,
};

const DEMO_AGENTS: [(&str, &str, &str); 3] = [
    ("demo-ema-trend", "Demo · EMA trend", "ema-crossover-trend"),
    (
        "demo-rsi-reversion",
        "Demo · RSI reversion",
        "rsi-mean-reversion",
    ),
    (
        "demo-trend-pullback",
        "Demo · Trend pullback",
        "trend-filtered-rsi-pullback",
    ),
];
const DEMO_CREATED_AT: i64 = 1_700_000_000;
const DEMO_START_TS: i64 = DEMO_CREATED_AT * 1_000;
const DEMO_BAR_MS: i64 = 3_600_000;
const DEMO_WARMUP_BARS: usize = 300;
const DEMO_TOTAL_BARS: usize = 360;

#[derive(Debug, thiserror::Error)]
pub enum DemoSeedError {
    #[error(transparent)]
    Catalog(#[from] TemplateCatalogError),
    #[error(transparent)]
    Database(#[from] DbError),
    #[error(transparent)]
    Agent(#[from] crate::agent::AgentError),
    #[error(transparent)]
    Backtest(#[from] crate::backtest::BacktestError),
    #[error("demo template {0:?} is missing")]
    MissingTemplate(String),
    #[error("could not serialize demo strategy: {0}")]
    Serialize(#[from] serde_json::Error),
}

pub fn seed_demo_agents(db: &Db) -> Result<usize, DemoSeedError> {
    let catalog = TemplateCatalog::from_json(include_str!("../templates.json"))?;
    let candles = demo_candles();
    let warmup = &candles[..DEMO_WARMUP_BARS];
    let continuation = &candles[DEMO_WARMUP_BARS..];
    let cfg = BacktestConfig::default();
    let risk = RiskLimits::default();
    let mut inserted = 0;

    for (id, name, template_id) in DEMO_AGENTS {
        if db.get_agent(id)?.is_some() {
            continue;
        }
        let template = catalog
            .get(template_id)
            .ok_or_else(|| DemoSeedError::MissingTemplate(template_id.to_string()))?;
        let strategy = &template.strategy;
        let strategy_json = serde_json::to_value(strategy)?;
        let mut snapshot =
            AgentSnapshot::warm_up(strategy, warmup, &cfg, DEFAULT_INITIAL_EQUITY, risk)?;
        let backtest = backtest_strategy(strategy, warmup, &cfg)?;
        let mut equity_points: Vec<EquityPoint> = warmup
            .iter()
            .zip(backtest.equity_curve)
            .map(|(candle, equity)| EquityPoint {
                bar_ts: candle.ts,
                equity,
            })
            .collect();

        let advance = snapshot.advance(continuation)?;
        equity_points.extend(advance.equity_points);
        let mut decisions = advance.events;
        let stopped = snapshot.stop();
        equity_points.extend(stopped.equity_points);
        decisions.extend(stopped.events);
        renumber_decisions(&mut decisions);
        equity_points.sort_by_key(|point| point.bar_ts);
        let snapshot_json = snapshot.to_json()?;

        if db.seed_demo_agent(DemoAgentSeed {
            id,
            name,
            created_at: DEMO_CREATED_AT,
            started_at: DEMO_START_TS / 1_000,
            stopped_at: snapshot.last_ts() / 1_000,
            strategy: &strategy_json,
            initial_equity: DEFAULT_INITIAL_EQUITY,
            equity: snapshot.equity(),
            last_price: snapshot.last_price(),
            last_ts: snapshot.last_ts(),
            snapshot: &snapshot_json,
            risk: &risk,
            decisions: &decisions,
            equity_points: &equity_points,
        })? {
            inserted += 1;
        }
    }
    Ok(inserted)
}

fn renumber_decisions(decisions: &mut [DecisionEvent]) {
    let mut current_bar = None;
    let mut sequence = 0;
    for decision in decisions {
        if current_bar != Some(decision.bar_ts) {
            current_bar = Some(decision.bar_ts);
            sequence = 0;
        }
        decision.seq = sequence;
        sequence += 1;
    }
}

fn demo_candles() -> Vec<Candle> {
    let mut previous = 100.0;
    (0..DEMO_TOTAL_BARS)
        .map(|index| {
            let i = index as f64;
            let close = 100.0 + i * 0.025 + (i * 0.23).sin() * 7.0 + (i * 0.071).cos() * 2.5;
            let open = previous;
            previous = close;
            Candle {
                ts: DEMO_START_TS + index as i64 * DEMO_BAR_MS,
                open,
                high: open.max(close) + 0.8,
                low: open.min(close) - 0.8,
                close,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::MAX_AGENT_EQUITY_POINTS;

    #[test]
    fn deterministic_demo_seed_creates_agents_with_history_once() {
        let db = Db::open_in_memory().expect("database");

        assert_eq!(seed_demo_agents(&db).expect("seed"), DEMO_AGENTS.len());
        assert_eq!(seed_demo_agents(&db).expect("idempotent seed"), 0);

        let agents = db.list_agents().expect("agent list");
        assert_eq!(agents.len(), DEMO_AGENTS.len());
        let mut decision_count = 0;
        for agent in agents {
            assert_eq!(agent.status, "stopped");
            assert_eq!(agent.created_at, DEMO_CREATED_AT);
            assert!(agent.is_sample);
            let row = db
                .get_agent(&agent.id)
                .expect("agent detail query")
                .expect("seeded agent");
            assert!(row.summary.is_sample);
            assert!(row.snapshot.is_some());
            let equity = db
                .list_agent_equity(&agent.id, MAX_AGENT_EQUITY_POINTS, None)
                .expect("equity history");
            assert_eq!(equity.len(), DEMO_TOTAL_BARS);
            assert!(equity.iter().all(|point| point.equity.is_finite()));
            let decisions = db
                .list_agent_decisions(&agent.id, 100, None)
                .expect("decision history");
            decision_count += decisions.len();
        }
        assert!(decision_count > 0);
    }
}
