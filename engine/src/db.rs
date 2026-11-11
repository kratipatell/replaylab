use rusqlite::{params, Connection, Transaction};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

use crate::backtest::DecisionEvent;
use crate::risk::RiskLimits;

pub const MAX_AGENT_DECISIONS: i64 = 5_000;
pub const MAX_AGENT_EQUITY_POINTS: i64 = 20_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedRun {
    pub id: i64,
    pub created_at: i64,
    pub symbol: String,
    pub interval: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub strategy: serde_json::Value,
    pub stats: serde_json::Value,
    pub metrics: serde_json::Value,
    pub equity_curve: serde_json::Value,
    pub trades: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSummary {
    pub id: String,
    pub name: String,
    pub status: String,
    pub symbol: String,
    pub interval: String,
    pub run_id: Option<i64>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub stopped_at: Option<i64>,
    pub equity: f64,
    pub last_price: Option<f64>,
    pub last_ts: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AgentRow {
    pub summary: AgentSummary,
    pub strategy: serde_json::Value,
    pub initial_equity: f64,
    pub snapshot: Option<String>,
    pub risk: RiskLimits,
    pub kill_switch: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEquityPoint {
    pub bar_ts: i64,
    pub equity: f64,
}

pub struct AgentAdvanceWrite<'a> {
    pub id: &'a str,
    pub status: &'a str,
    pub equity: f64,
    pub last_price: f64,
    pub last_ts: i64,
    pub snapshot: &'a str,
    pub decisions: &'a [DecisionEvent],
    pub equity_points: &'a [crate::agent::EquityPoint],
}

#[derive(Debug, Clone, Serialize)]
pub struct RunSummary {
    pub id: i64,
    pub created_at: i64,
    pub symbol: String,
    pub interval: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub trades: usize,
    pub total_return_pct: f64,
    pub sharpe: f64,
    pub max_drawdown_pct: f64,
}

pub struct Db {
    conn: Mutex<Connection>,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at INTEGER NOT NULL,
    symbol TEXT NOT NULL,
    interval TEXT NOT NULL,
    start_ms INTEGER NOT NULL,
    end_ms INTEGER NOT NULL,
    strategy TEXT NOT NULL,
    stats TEXT NOT NULL,
    metrics TEXT NOT NULL,
    equity_curve TEXT NOT NULL,
    trades TEXT NOT NULL,
    owner_id TEXT NOT NULL DEFAULT 'local'
);
CREATE TABLE IF NOT EXISTS agents (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    status TEXT NOT NULL,
    strategy TEXT NOT NULL,
    run_id INTEGER,
    created_at INTEGER NOT NULL,
    started_at INTEGER,
    stopped_at INTEGER,
    equity REAL NOT NULL DEFAULT 10000.0,
    initial_equity REAL NOT NULL DEFAULT 10000.0,
    last_price REAL,
    last_ts INTEGER,
    snapshot TEXT,
    last_error TEXT,
    owner_id TEXT NOT NULL DEFAULT 'local',
    risk TEXT NOT NULL DEFAULT '',
    kill_switch INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS agent_decisions (
    id INTEGER PRIMARY KEY,
    agent_id TEXT NOT NULL,
    bar_ts INTEGER NOT NULL,
    kind TEXT NOT NULL,
    seq INTEGER NOT NULL,
    reason TEXT,
    price REAL,
    qty REAL,
    requested_qty REAL,
    configured REAL,
    observed REAL,
    equity REAL NOT NULL,
    UNIQUE (agent_id, bar_ts, kind, seq)
);
CREATE INDEX IF NOT EXISTS agent_decisions_cursor
    ON agent_decisions (agent_id, bar_ts DESC, seq DESC);
CREATE TABLE IF NOT EXISTS agent_equity (
    id INTEGER PRIMARY KEY,
    agent_id TEXT NOT NULL,
    bar_ts INTEGER NOT NULL,
    equity REAL NOT NULL,
    UNIQUE (agent_id, bar_ts)
)";

const ADD_OWNER_ID: &str = "ALTER TABLE runs ADD COLUMN owner_id TEXT NOT NULL DEFAULT 'local'";
const ADD_AGENTS_OWNER: &str =
    "ALTER TABLE agents ADD COLUMN owner_id TEXT NOT NULL DEFAULT 'local'";
const ADD_AGENTS_EQUITY: &str =
    "ALTER TABLE agents ADD COLUMN initial_equity REAL NOT NULL DEFAULT 10000.0";
const ADD_AGENTS_SNAPSHOT: &str = "ALTER TABLE agents ADD COLUMN snapshot TEXT";
const ADD_AGENTS_ERROR: &str = "ALTER TABLE agents ADD COLUMN last_error TEXT";
const ADD_AGENTS_RISK: &str = "ALTER TABLE agents ADD COLUMN risk TEXT NOT NULL DEFAULT ''";
const ADD_AGENTS_KILL: &str =
    "ALTER TABLE agents ADD COLUMN kill_switch INTEGER NOT NULL DEFAULT 0";

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("init runs db: {0}")]
    Init(String),
    #[error("create db dir: {0}")]
    MkDir(String),
    #[error("open runs db: {0}")]
    Open(String),
    #[cfg(test)]
    #[error("open memory db: {0}")]
    OpenMemory(String),
    #[error("db lock: {0}")]
    Lock(String),
    #[error("save run: {0}")]
    Save(String),
    #[error("list runs: {0}")]
    List(String),
    #[error("get run: {0}")]
    Get(String),
    #[error("create agent: {0}")]
    CreateAgent(String),
    #[error("list agents: {0}")]
    ListAgents(String),
    #[error("get agent: {0}")]
    GetAgent(String),
    #[error("update agent: {0}")]
    UpdateAgent(String),
    #[error("kill switch: {0}")]
    KillSwitch(String),
    #[error("agent history: {0}")]
    AgentHistory(String),
}

fn add_column(conn: &Connection, table: &str, column: &str, ddl: &str) -> Result<(), DbError> {
    let count: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"),
            params![column],
            |row| row.get(0),
        )
        .map_err(|e| DbError::Init(e.to_string()))?;
    if count == 0 {
        conn.execute_batch(ddl)
            .map_err(|e| DbError::Init(e.to_string()))?;
    }
    Ok(())
}

fn insert_agent_equity(
    tx: &Transaction<'_>,
    id: &str,
    points: &[crate::agent::EquityPoint],
) -> rusqlite::Result<()> {
    let start = points
        .len()
        .saturating_sub(MAX_AGENT_EQUITY_POINTS as usize);
    let mut insert = tx.prepare(
        "INSERT OR IGNORE INTO agent_equity (agent_id, bar_ts, equity)
         VALUES (?1, ?2, ?3)",
    )?;
    for point in &points[start..] {
        insert.execute(params![id, point.bar_ts, point.equity])?;
    }
    Ok(())
}

fn trim_agent_equity(tx: &Transaction<'_>, id: &str) -> rusqlite::Result<()> {
    tx.execute(
        "DELETE FROM agent_equity WHERE agent_id = ?1 AND id NOT IN (
             SELECT id FROM agent_equity
             WHERE agent_id = ?1 ORDER BY bar_ts DESC LIMIT ?2
         )",
        params![id, MAX_AGENT_EQUITY_POINTS],
    )?;
    Ok(())
}

fn trim_agent_decisions(tx: &Transaction<'_>, id: &str) -> rusqlite::Result<()> {
    tx.execute(
        "DELETE FROM agent_decisions WHERE agent_id = ?1 AND id NOT IN (
             SELECT id FROM agent_decisions
             WHERE agent_id = ?1 ORDER BY bar_ts DESC, seq DESC, id DESC LIMIT ?2
         )",
        params![id, MAX_AGENT_DECISIONS],
    )?;
    Ok(())
}

impl Db {
    fn init(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch(SCHEMA)
            .map_err(|e| DbError::Init(e.to_string()))?;
        Self::migrate(conn)
    }

    fn migrate(conn: &Connection) -> Result<(), DbError> {
        add_column(conn, "runs", "owner_id", ADD_OWNER_ID)?;
        add_column(conn, "agents", "owner_id", ADD_AGENTS_OWNER)?;
        add_column(conn, "agents", "initial_equity", ADD_AGENTS_EQUITY)?;
        add_column(conn, "agents", "snapshot", ADD_AGENTS_SNAPSHOT)?;
        add_column(conn, "agents", "last_error", ADD_AGENTS_ERROR)?;
        add_column(conn, "agents", "risk", ADD_AGENTS_RISK)?;
        add_column(conn, "agents", "kill_switch", ADD_AGENTS_KILL)?;
        // Every existing agent trades under the default caps.
        let default_risk = serde_json::to_string(&RiskLimits::default())
            .map_err(|e| DbError::Init(e.to_string()))?;
        conn.execute(
            "UPDATE agents SET risk = ?1 WHERE risk = ''",
            params![default_risk],
        )
        .map_err(|e| DbError::Init(e.to_string()))?;
        // The platform-wide kill switch is a single persisted row. IGNORE
        // keeps its value across restarts instead of resetting it on open.
        conn.execute(
            "INSERT OR IGNORE INTO settings (key, value) VALUES ('kill_switch', 0)",
            [],
        )
        .map_err(|e| DbError::Init(e.to_string()))?;
        Ok(())
    }

    pub fn open(path: &Path) -> Result<Self, DbError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| DbError::MkDir(e.to_string()))?;
            }
        }
        let conn = Connection::open(path).map_err(|e| DbError::Open(e.to_string()))?;
        Self::init(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, DbError> {
        let conn = Connection::open_in_memory().map_err(|e| DbError::OpenMemory(e.to_string()))?;
        Self::init(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn save_run(
        &self,
        symbol: &str,
        interval: &str,
        start_ms: i64,
        end_ms: i64,
        strategy: &serde_json::Value,
        stats: &serde_json::Value,
        metrics: &serde_json::Value,
        equity_curve: &serde_json::Value,
        trades: &serde_json::Value,
    ) -> Result<i64, DbError> {
        let conn = self.conn.lock().map_err(|e| DbError::Lock(e.to_string()))?;
        conn.execute(
            // owner_id stays 'local' until accounts exist.
            "INSERT INTO runs (created_at, symbol, interval, start_ms, end_ms, strategy, stats, metrics, equity_curve, trades, owner_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'local')",
            params![
                now_secs(),
                symbol,
                interval,
                start_ms,
                end_ms,
                strategy.to_string(),
                stats.to_string(),
                metrics.to_string(),
                equity_curve.to_string(),
                trades.to_string(),
            ],
        )
        .map_err(|e| DbError::Save(e.to_string()))?;
        Ok(conn.last_insert_rowid())
    }

    fn summary_from_row(row: &rusqlite::Row) -> rusqlite::Result<RunSummary> {
        let stats_txt: String = row.get(6)?;
        let stats: serde_json::Value =
            serde_json::from_str(&stats_txt).unwrap_or(serde_json::Value::Null);
        Ok(RunSummary {
            id: row.get(0)?,
            created_at: row.get(1)?,
            symbol: row.get(2)?,
            interval: row.get(3)?,
            start_ms: row.get(4)?,
            end_ms: row.get(5)?,
            trades: stats.get("trades").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            total_return_pct: stats
                .get("total_return_pct")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            sharpe: stats.get("sharpe").and_then(|v| v.as_f64()).unwrap_or(0.0),
            max_drawdown_pct: stats
                .get("max_drawdown_pct")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
        })
    }

    pub fn list_runs(&self, limit: usize) -> Result<Vec<RunSummary>, DbError> {
        let conn = self.conn.lock().map_err(|e| DbError::Lock(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, created_at, symbol, interval, start_ms, end_ms, stats
                 FROM runs ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|e| DbError::List(e.to_string()))?;
        let rows = stmt
            .query_map(params![limit as i64], Self::summary_from_row)
            .map_err(|e| DbError::List(e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| DbError::List(e.to_string()))?);
        }
        Ok(out)
    }

    pub fn get_run(&self, id: i64) -> Result<Option<SavedRun>, DbError> {
        let conn = self.conn.lock().map_err(|e| DbError::Lock(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, created_at, symbol, interval, start_ms, end_ms,
                        strategy, stats, metrics, equity_curve, trades
                 FROM runs WHERE id = ?1",
            )
            .map_err(|e| DbError::Get(e.to_string()))?;
        let mut rows = stmt
            .query_map(params![id], |row| {
                let parse = |i: usize| -> rusqlite::Result<serde_json::Value> {
                    let txt: String = row.get(i)?;
                    Ok(serde_json::from_str(&txt).unwrap_or(serde_json::Value::Null))
                };
                Ok(SavedRun {
                    id: row.get(0)?,
                    created_at: row.get(1)?,
                    symbol: row.get(2)?,
                    interval: row.get(3)?,
                    start_ms: row.get(4)?,
                    end_ms: row.get(5)?,
                    strategy: parse(6)?,
                    stats: parse(7)?,
                    metrics: parse(8)?,
                    equity_curve: parse(9)?,
                    trades: parse(10)?,
                })
            })
            .map_err(|e| DbError::Get(e.to_string()))?;
        match rows.next() {
            None => Ok(None),
            Some(r) => Ok(Some(r.map_err(|e| DbError::Get(e.to_string()))?)),
        }
    }

    pub fn create_agent(
        &self,
        id: &str,
        name: &str,
        strategy: &serde_json::Value,
        run_id: Option<i64>,
        initial_equity: f64,
        risk: &RiskLimits,
    ) -> Result<(), DbError> {
        let risk_json =
            serde_json::to_string(risk).map_err(|e| DbError::CreateAgent(e.to_string()))?;
        let conn = self.conn.lock().map_err(|e| DbError::Lock(e.to_string()))?;
        conn.execute(
            // owner_id stays 'local' until accounts exist.
            "INSERT INTO agents
                (id, name, status, strategy, run_id, created_at, equity, initial_equity, owner_id, risk)
             VALUES (?1, ?2, 'draft', ?3, ?4, ?5, ?6, ?6, 'local', ?7)",
            params![
                id,
                name,
                strategy.to_string(),
                run_id,
                now_secs(),
                initial_equity,
                risk_json
            ],
        )
        .map_err(|e| DbError::CreateAgent(e.to_string()))?;
        Ok(())
    }

    fn agent_summary_from_row(row: &rusqlite::Row) -> rusqlite::Result<AgentSummary> {
        let strategy_txt: String = row.get(3)?;
        let strategy: serde_json::Value =
            serde_json::from_str(&strategy_txt).unwrap_or(serde_json::Value::Null);
        Ok(AgentSummary {
            id: row.get(0)?,
            name: row.get(1)?,
            status: row.get(2)?,
            symbol: strategy
                .get("asset")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            interval: strategy
                .get("timeframe")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            run_id: row.get(4)?,
            created_at: row.get(5)?,
            started_at: row.get(6)?,
            stopped_at: row.get(7)?,
            equity: row.get(8)?,
            last_price: row.get(9)?,
            last_ts: row.get(10)?,
            last_error: row.get(11)?,
        })
    }

    pub fn list_agents(&self) -> Result<Vec<AgentSummary>, DbError> {
        let conn = self.conn.lock().map_err(|e| DbError::Lock(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, status, strategy, run_id, created_at, started_at,
                        stopped_at, equity, last_price, last_ts, last_error
                 FROM agents ORDER BY created_at DESC, id DESC",
            )
            .map_err(|e| DbError::ListAgents(e.to_string()))?;
        let rows = stmt
            .query_map([], Self::agent_summary_from_row)
            .map_err(|e| DbError::ListAgents(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| DbError::ListAgents(e.to_string()))?);
        }
        Ok(out)
    }

    pub fn get_agent(&self, id: &str) -> Result<Option<AgentRow>, DbError> {
        let conn = self.conn.lock().map_err(|e| DbError::Lock(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, status, strategy, run_id, created_at, started_at,
                        stopped_at, equity, last_price, last_ts, last_error,
                        initial_equity, snapshot, risk, kill_switch
                 FROM agents WHERE id = ?1",
            )
            .map_err(|e| DbError::GetAgent(e.to_string()))?;
        let mut rows = stmt
            .query_map(params![id], |row| {
                let summary = Self::agent_summary_from_row(row)?;
                let strategy_txt: String = row.get(3)?;
                let risk_txt: String = row.get(14)?;
                Ok(AgentRow {
                    summary,
                    strategy: serde_json::from_str(&strategy_txt)
                        .unwrap_or(serde_json::Value::Null),
                    initial_equity: row.get(12)?,
                    snapshot: row.get(13)?,
                    risk: serde_json::from_str(&risk_txt).unwrap_or_default(),
                    kill_switch: row.get::<_, i64>(15)? != 0,
                })
            })
            .map_err(|e| DbError::GetAgent(e.to_string()))?;
        match rows.next() {
            None => Ok(None),
            Some(row) => Ok(Some(row.map_err(|e| DbError::GetAgent(e.to_string()))?)),
        }
    }

    #[cfg(test)]
    pub fn start_agent(&self, id: &str, started_at: i64) -> Result<bool, DbError> {
        let conn = self.conn.lock().map_err(|e| DbError::Lock(e.to_string()))?;
        conn.execute(
            "UPDATE agents
             SET status = 'running', started_at = ?2, stopped_at = NULL, last_error = NULL
             WHERE id = ?1",
            params![id, started_at],
        )
        .map(|n| n > 0)
        .map_err(|e| DbError::UpdateAgent(e.to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start_agent_with_snapshot(
        &self,
        id: &str,
        started_at: i64,
        equity: f64,
        last_price: f64,
        last_ts: i64,
        snapshot: &str,
        equity_points: &[crate::agent::EquityPoint],
    ) -> Result<bool, DbError> {
        let mut conn = self.conn.lock().map_err(|e| DbError::Lock(e.to_string()))?;
        let tx = conn
            .transaction()
            .map_err(|e| DbError::UpdateAgent(e.to_string()))?;
        let updated = tx
            .execute(
                "UPDATE agents
             SET status = 'running', started_at = ?2, stopped_at = NULL,
                 equity = ?3, last_price = ?4, last_ts = ?5,
                 snapshot = ?6, last_error = NULL
             WHERE id = ?1",
                params![id, started_at, equity, last_price, last_ts, snapshot],
            )
            .map_err(|e| DbError::UpdateAgent(e.to_string()))?;
        if updated == 0 {
            return Ok(false);
        }
        insert_agent_equity(&tx, id, equity_points)
            .map_err(|e| DbError::AgentHistory(e.to_string()))?;
        trim_agent_equity(&tx, id).map_err(|e| DbError::AgentHistory(e.to_string()))?;
        tx.commit()
            .map_err(|e| DbError::AgentHistory(e.to_string()))?;
        Ok(true)
    }

    pub fn stop_agent(&self, id: &str, stopped_at: i64) -> Result<bool, DbError> {
        let conn = self.conn.lock().map_err(|e| DbError::Lock(e.to_string()))?;
        conn.execute(
            "UPDATE agents SET status = 'stopped', stopped_at = ?2 WHERE id = ?1",
            params![id, stopped_at],
        )
        .map(|n| n > 0)
        .map_err(|e| DbError::UpdateAgent(e.to_string()))
    }

    #[cfg(test)]
    pub fn save_agent_snapshot(
        &self,
        id: &str,
        status: &str,
        equity: f64,
        last_price: f64,
        last_ts: i64,
        snapshot: &str,
    ) -> Result<bool, DbError> {
        let conn = self.conn.lock().map_err(|e| DbError::Lock(e.to_string()))?;
        conn.execute(
            "UPDATE agents
             SET status = ?2, equity = ?3, last_price = ?4, last_ts = ?5,
                 snapshot = ?6, last_error = NULL
             WHERE id = ?1",
            params![id, status, equity, last_price, last_ts, snapshot],
        )
        .map(|n| n > 0)
        .map_err(|e| DbError::UpdateAgent(e.to_string()))
    }

    pub fn save_agent_advance(&self, advance: AgentAdvanceWrite<'_>) -> Result<bool, DbError> {
        let mut conn = self.conn.lock().map_err(|e| DbError::Lock(e.to_string()))?;
        let tx = conn
            .transaction()
            .map_err(|e| DbError::AgentHistory(e.to_string()))?;
        let updated = tx
            .execute(
                "UPDATE agents
                 SET status = ?2, equity = ?3, last_price = ?4, last_ts = ?5,
                     snapshot = ?6, last_error = NULL
                 WHERE id = ?1",
                params![
                    advance.id,
                    advance.status,
                    advance.equity,
                    advance.last_price,
                    advance.last_ts,
                    advance.snapshot
                ],
            )
            .map_err(|e| DbError::AgentHistory(e.to_string()))?;
        if updated == 0 {
            return Ok(false);
        }
        {
            let mut insert = tx
                .prepare(
                    "INSERT OR IGNORE INTO agent_decisions
                     (agent_id, bar_ts, kind, seq, reason, price, qty,
                      requested_qty, configured, observed, equity)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                )
                .map_err(|e| DbError::AgentHistory(e.to_string()))?;
            for decision in advance.decisions {
                insert
                    .execute(params![
                        advance.id,
                        decision.bar_ts,
                        decision.kind,
                        decision.seq,
                        decision.reason,
                        decision.price,
                        decision.qty,
                        decision.requested_qty,
                        decision.configured,
                        decision.observed,
                        decision.equity,
                    ])
                    .map_err(|e| DbError::AgentHistory(e.to_string()))?;
            }
        }
        insert_agent_equity(&tx, advance.id, advance.equity_points)
            .map_err(|e| DbError::AgentHistory(e.to_string()))?;
        trim_agent_decisions(&tx, advance.id).map_err(|e| DbError::AgentHistory(e.to_string()))?;
        trim_agent_equity(&tx, advance.id).map_err(|e| DbError::AgentHistory(e.to_string()))?;
        tx.commit()
            .map_err(|e| DbError::AgentHistory(e.to_string()))?;
        Ok(true)
    }

    pub fn list_agent_decisions(
        &self,
        id: &str,
        limit: i64,
        before: Option<(i64, i64)>,
    ) -> Result<Vec<DecisionEvent>, DbError> {
        let conn = self.conn.lock().map_err(|e| DbError::Lock(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT bar_ts, kind, seq, reason, price, qty, requested_qty,
                        configured, observed, equity
                 FROM agent_decisions
                 WHERE agent_id = ?1
                   AND (?2 IS NULL OR bar_ts < ?2 OR (bar_ts = ?2 AND seq < ?3))
                 ORDER BY bar_ts DESC, seq DESC LIMIT ?4",
            )
            .map_err(|e| DbError::AgentHistory(e.to_string()))?;
        let rows = stmt
            .query_map(
                params![id, before.map(|v| v.0), before.map(|v| v.1), limit],
                |row| {
                    Ok(DecisionEvent {
                        bar_ts: row.get(0)?,
                        kind: row.get(1)?,
                        seq: row.get(2)?,
                        reason: row.get(3)?,
                        price: row.get(4)?,
                        qty: row.get(5)?,
                        requested_qty: row.get(6)?,
                        configured: row.get(7)?,
                        observed: row.get(8)?,
                        equity: row.get(9)?,
                    })
                },
            )
            .map_err(|e| DbError::AgentHistory(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| DbError::AgentHistory(e.to_string()))
    }

    pub fn list_agent_equity(
        &self,
        id: &str,
        limit: i64,
        since: Option<i64>,
    ) -> Result<Vec<AgentEquityPoint>, DbError> {
        let conn = self.conn.lock().map_err(|e| DbError::Lock(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT bar_ts, equity FROM agent_equity
                 WHERE agent_id = ?1 AND (?2 IS NULL OR bar_ts >= ?2)
                 ORDER BY bar_ts ASC LIMIT ?3",
            )
            .map_err(|e| DbError::AgentHistory(e.to_string()))?;
        let rows = stmt
            .query_map(params![id, since, limit], |row| {
                Ok(AgentEquityPoint {
                    bar_ts: row.get(0)?,
                    equity: row.get(1)?,
                })
            })
            .map_err(|e| DbError::AgentHistory(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| DbError::AgentHistory(e.to_string()))
    }

    pub fn record_agent_error(&self, id: &str, error: &str) -> Result<bool, DbError> {
        let conn = self.conn.lock().map_err(|e| DbError::Lock(e.to_string()))?;
        conn.execute(
            "UPDATE agents SET last_error = ?2 WHERE id = ?1",
            params![id, error],
        )
        .map(|n| n > 0)
        .map_err(|e| DbError::UpdateAgent(e.to_string()))
    }

    /// Replace only the snapshot, leaving status, equity and last_error alone.
    #[cfg(test)]
    pub fn set_agent_snapshot(&self, id: &str, snapshot: &str) -> Result<bool, DbError> {
        self.set_agent_snapshot_with_equity(id, snapshot, &[])
    }

    pub fn set_agent_snapshot_with_equity(
        &self,
        id: &str,
        snapshot: &str,
        equity_points: &[crate::agent::EquityPoint],
    ) -> Result<bool, DbError> {
        let mut conn = self.conn.lock().map_err(|e| DbError::Lock(e.to_string()))?;
        let tx = conn
            .transaction()
            .map_err(|e| DbError::AgentHistory(e.to_string()))?;
        let updated = tx
            .execute(
                "UPDATE agents SET snapshot = ?2 WHERE id = ?1",
                params![id, snapshot],
            )
            .map_err(|e| DbError::UpdateAgent(e.to_string()))?;
        if updated == 0 {
            return Ok(false);
        }
        insert_agent_equity(&tx, id, equity_points)
            .map_err(|e| DbError::AgentHistory(e.to_string()))?;
        trim_agent_equity(&tx, id).map_err(|e| DbError::AgentHistory(e.to_string()))?;
        tx.commit()
            .map_err(|e| DbError::AgentHistory(e.to_string()))?;
        Ok(true)
    }

    pub fn set_kill_switch(&self, id: &str, on: bool) -> Result<bool, DbError> {
        let conn = self.conn.lock().map_err(|e| DbError::Lock(e.to_string()))?;
        conn.execute(
            "UPDATE agents SET kill_switch = ?2 WHERE id = ?1",
            params![id, on as i64],
        )
        .map(|n| n > 0)
        .map_err(|e| DbError::UpdateAgent(e.to_string()))
    }

    /// Platform-wide kill switch. New agents inherit it even when they are
    /// created after the switch was flipped, so `start` must read it too.
    pub fn set_global_kill_switch(&self, on: bool) -> Result<(), DbError> {
        let conn = self.conn.lock().map_err(|e| DbError::Lock(e.to_string()))?;
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('kill_switch', ?1)",
            params![on as i64],
        )
        .map_err(|e| DbError::KillSwitch(e.to_string()))?;
        Ok(())
    }

    pub fn global_kill_switch_on(&self) -> Result<bool, DbError> {
        let conn = self.conn.lock().map_err(|e| DbError::Lock(e.to_string()))?;
        let value: i64 = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'kill_switch'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| DbError::KillSwitch(e.to_string()))?;
        Ok(value != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_list_get_roundtrip() {
        let db = Db::open_in_memory().expect("mem db");
        let strat = serde_json::json!({"asset": "BTCUSDT"});
        let stats = serde_json::json!({"trades": 3, "total_return_pct": 5.0, "sharpe": 1.2, "max_drawdown_pct": 2.0});
        let id = db
            .save_run(
                "BTCUSDT",
                "1h",
                1,
                2,
                &strat,
                &stats,
                &serde_json::json!({}),
                &serde_json::json!([1.0, 1.05]),
                &serde_json::json!([]),
            )
            .expect("save");
        assert_eq!(id, 1);
        let runs = db.list_runs(10).expect("list");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].symbol, "BTCUSDT");
        assert_eq!(runs[0].trades, 3);
        let got = db.get_run(1).expect("get").expect("exists");
        assert_eq!(got.strategy, strat);
        assert!(db.get_run(999).expect("get").is_none());
    }

    fn strategy_json() -> serde_json::Value {
        serde_json::json!({"asset": "BTCUSDT", "timeframe": "1h", "side": "long"})
    }

    #[test]
    fn agent_lifecycle_roundtrip() {
        let db = Db::open_in_memory().expect("mem db");
        db.create_agent(
            "ag-1",
            "Paper BTC",
            &strategy_json(),
            None,
            10_000.0,
            &RiskLimits::default(),
        )
        .expect("create");

        let agents = db.list_agents().expect("list");
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, "ag-1");
        assert_eq!(agents[0].status, "draft");
        assert_eq!(agents[0].symbol, "BTCUSDT");
        assert_eq!(agents[0].interval, "1h");
        assert_eq!(agents[0].equity, 10_000.0);

        let row = db.get_agent("ag-1").expect("get").expect("exists");
        assert_eq!(row.strategy, strategy_json());
        assert_eq!(row.initial_equity, 10_000.0);
        assert!(row.snapshot.is_none());
        assert_eq!(row.risk, RiskLimits::default());
        assert!(!row.kill_switch);

        assert!(db.start_agent("ag-1", 10).expect("start"));
        assert!(db
            .save_agent_snapshot("ag-1", "running", 10_050.0, 101.0, 1_234, "{}")
            .expect("snapshot"));
        let row = db.get_agent("ag-1").expect("get").expect("exists");
        assert_eq!(row.summary.status, "running");
        assert_eq!(row.summary.started_at, Some(10));
        assert_eq!(row.summary.equity, 10_050.0);
        assert_eq!(row.summary.last_price, Some(101.0));
        assert_eq!(row.summary.last_ts, Some(1_234));
        assert_eq!(row.snapshot.as_deref(), Some("{}"));

        assert!(db.record_agent_error("ag-1", "boom").expect("error"));
        assert_eq!(
            db.get_agent("ag-1")
                .expect("get")
                .expect("exists")
                .summary
                .last_error
                .as_deref(),
            Some("boom")
        );

        assert!(db.stop_agent("ag-1", 20).expect("stop"));
        assert_eq!(
            db.get_agent("ag-1")
                .expect("get")
                .expect("exists")
                .summary
                .status,
            "stopped"
        );
        assert!(!db.start_agent("missing", 1).expect("missing"));
        assert!(!db.stop_agent("missing", 1).expect("missing"));
        assert!(!db
            .save_agent_snapshot("missing", "running", 1.0, 1.0, 1, "{}")
            .expect("missing"));
    }

    #[test]
    fn custom_risk_and_kill_switch_persist() {
        let db = Db::open_in_memory().expect("mem db");
        let risk = RiskLimits {
            max_position_pct: 25.0,
            max_daily_loss_pct: 5.0,
            max_drawdown_pct: 15.0,
        };
        db.create_agent("ag", "Tight", &strategy_json(), None, 1_000.0, &risk)
            .expect("create");
        assert_eq!(db.get_agent("ag").expect("get").expect("exists").risk, risk);

        assert!(db.set_kill_switch("ag", true).expect("kill"));
        assert!(
            db.get_agent("ag")
                .expect("get")
                .expect("exists")
                .kill_switch
        );
        assert!(db.set_kill_switch("ag", false).expect("clear"));
        assert!(
            !db.get_agent("ag")
                .expect("get")
                .expect("exists")
                .kill_switch
        );
    }

    #[test]
    fn agent_advance_transaction_rolls_back_snapshot_and_logs_together() {
        let db = Db::open_in_memory().expect("mem db");
        db.create_agent(
            "ag",
            "Paper",
            &strategy_json(),
            None,
            10_000.0,
            &RiskLimits::default(),
        )
        .expect("create");
        db.set_agent_snapshot("ag", "before")
            .expect("initial snapshot");
        {
            let conn = db.conn.lock().expect("lock");
            conn.execute_batch(
                "CREATE TRIGGER reject_equity BEFORE INSERT ON agent_equity
                 BEGIN SELECT RAISE(ABORT, 'forced history failure'); END",
            )
            .expect("trigger");
        }
        let decisions = [DecisionEvent {
            bar_ts: 10,
            kind: "signal_received".to_string(),
            seq: 0,
            reason: Some("long".to_string()),
            price: None,
            qty: None,
            requested_qty: None,
            configured: None,
            observed: None,
            equity: 10_000.0,
        }];
        let points = [crate::agent::EquityPoint {
            bar_ts: 10,
            equity: 10_000.0,
        }];
        assert!(db
            .save_agent_advance(AgentAdvanceWrite {
                id: "ag",
                status: "running",
                equity: 10_000.0,
                last_price: 100.0,
                last_ts: 10,
                snapshot: "after",
                decisions: &decisions,
                equity_points: &points,
            })
            .is_err());

        let row = db.get_agent("ag").expect("get").expect("exists");
        assert_eq!(row.snapshot.as_deref(), Some("before"));
        assert_eq!(row.summary.status, "draft");
        assert_eq!(row.summary.equity, 10_000.0);
        assert_eq!(row.summary.last_ts, None);
        let conn = db.conn.lock().expect("lock");
        let decision_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_decisions", [], |r| r.get(0))
            .expect("decision count");
        let equity_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_equity", [], |r| r.get(0))
            .expect("equity count");
        assert_eq!(decision_count, 0);
        assert_eq!(equity_count, 0);
    }

    #[test]
    fn agent_history_retention_keeps_only_the_newest_rows() {
        let db = Db::open_in_memory().expect("mem db");
        db.create_agent(
            "ag",
            "Paper",
            &strategy_json(),
            None,
            10_000.0,
            &RiskLimits::default(),
        )
        .expect("create");
        {
            let conn = db.conn.lock().expect("lock");
            let tx = conn.unchecked_transaction().expect("transaction");
            {
                let mut insert = tx
                    .prepare(
                        "INSERT INTO agent_decisions
                         (agent_id, bar_ts, kind, seq, equity)
                         VALUES ('ag', ?1, 'quiet_test', 0, 10000.0)",
                    )
                    .expect("decision insert");
                for ts in 0..(MAX_AGENT_DECISIONS + 3) {
                    insert.execute([ts]).expect("seed decision");
                }
            }
            {
                let mut insert = tx
                    .prepare(
                        "INSERT INTO agent_equity (agent_id, bar_ts, equity)
                         VALUES ('ag', ?1, 10000.0)",
                    )
                    .expect("equity insert");
                for ts in 0..(MAX_AGENT_EQUITY_POINTS + 3) {
                    insert.execute([ts]).expect("seed equity");
                }
            }
            tx.commit().expect("seed commit");
        }
        let newest_decision = DecisionEvent {
            bar_ts: 30_003,
            kind: "signal_received".to_string(),
            seq: 0,
            reason: Some("long".to_string()),
            price: None,
            qty: None,
            requested_qty: None,
            configured: None,
            observed: None,
            equity: 10_000.0,
        };
        let newest_equity = crate::agent::EquityPoint {
            bar_ts: 30_003,
            equity: 10_001.0,
        };
        assert!(db
            .save_agent_advance(AgentAdvanceWrite {
                id: "ag",
                status: "running",
                equity: 10_001.0,
                last_price: 100.0,
                last_ts: 30_003,
                snapshot: "{}",
                decisions: &[newest_decision],
                equity_points: &[newest_equity],
            })
            .expect("trim write"));
        let conn = db.conn.lock().expect("lock");
        let (decision_count, oldest_decision): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), MIN(bar_ts) FROM agent_decisions WHERE agent_id = 'ag'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("decision retention");
        let (equity_count, oldest_equity): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), MIN(bar_ts) FROM agent_equity WHERE agent_id = 'ag'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("equity retention");
        assert_eq!(decision_count, MAX_AGENT_DECISIONS);
        assert_eq!(oldest_decision, 4);
        assert_eq!(equity_count, MAX_AGENT_EQUITY_POINTS);
        assert_eq!(oldest_equity, 4);
        let newest: i64 = conn
            .query_row(
                "SELECT MAX(bar_ts) FROM agent_decisions WHERE agent_id = 'ag'",
                [],
                |r| r.get(0),
            )
            .expect("newest decision");
        assert_eq!(newest, 30_003);
        let newest_equity: i64 = conn
            .query_row(
                "SELECT MAX(bar_ts) FROM agent_equity WHERE agent_id = 'ag'",
                [],
                |r| r.get(0),
            )
            .expect("newest equity");
        assert_eq!(newest_equity, 30_003);
    }

    const OLD_AGENTS_SCHEMA: &str = "CREATE TABLE agents (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        status TEXT NOT NULL,
        strategy TEXT NOT NULL,
        run_id INTEGER,
        created_at INTEGER NOT NULL,
        started_at INTEGER,
        stopped_at INTEGER,
        equity REAL NOT NULL DEFAULT 10000.0,
        last_price REAL,
        last_ts INTEGER
    )";

    #[test]
    fn migration_adds_agent_columns_and_keeps_rows() {
        let path = temp_db_path("agent_migrate");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        {
            let conn = Connection::open(&path).expect("open old");
            conn.execute_batch(OLD_AGENTS_SCHEMA).expect("old agents");
            conn.execute(
                "INSERT INTO agents (id, name, status, strategy, created_at)
                 VALUES ('old', 'Old agent', 'draft', '{}', 1)",
                [],
            )
            .expect("old row");
        }
        let db = Db::open(&path).expect("migrate");
        let row = db.get_agent("old").expect("get").expect("exists");
        assert_eq!(row.summary.name, "Old agent");
        assert_eq!(row.initial_equity, 10_000.0);
        assert!(row.snapshot.is_none());
        assert!(row.summary.last_error.is_none());
        // An agent that predates risk caps gets the defaults and can trade.
        assert_eq!(row.risk, RiskLimits::default());
        assert!(!row.kill_switch);
        drop(db);
        let _ = Db::open(&path).expect("reopen");
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn migration_creates_agent_history_tables_for_an_old_db() {
        let path = temp_db_path("settings_migrate");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        {
            let conn = Connection::open(&path).expect("open old");
            conn.execute_batch(OLD_AGENTS_SCHEMA).expect("old agents");
            conn.execute(
                "INSERT INTO agents (id, name, status, strategy, created_at)
                 VALUES ('old', 'Old agent', 'draft', '{}', 1)",
                [],
            )
            .expect("old row");
            for table in ["settings", "agent_decisions", "agent_equity"] {
                let tables: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                        [table],
                        |r| r.get(0),
                    )
                    .expect("missing history table");
                assert_eq!(tables, 0, "the old DB predates {table}");
            }
        }

        // CREATE TABLE IF NOT EXISTS creates it on an existing database.
        let db = Db::open(&path).expect("migrate");
        assert!(
            !db.global_kill_switch_on().expect("global defaults off"),
            "the settings table is created and seeded"
        );
        assert!(db
            .list_agent_decisions("old", 1, None)
            .expect("decision table created")
            .is_empty());
        assert!(db
            .list_agent_equity("old", 1, None)
            .expect("equity table created")
            .is_empty());
        assert!(
            db.get_agent("old").expect("get").is_some(),
            "the old row survives migration"
        );
        db.set_global_kill_switch(true).expect("flip global");
        drop(db);

        // Reopening is idempotent: the flag is not reset by a second open.
        let db = Db::open(&path).expect("reopen");
        assert!(
            db.global_kill_switch_on().expect("global stays on"),
            "CREATE TABLE IF NOT EXISTS must not reset the flag"
        );
        drop(db);
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    const OLD_SCHEMA: &str = "CREATE TABLE runs (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        created_at INTEGER NOT NULL,
        symbol TEXT NOT NULL,
        interval TEXT NOT NULL,
        start_ms INTEGER NOT NULL,
        end_ms INTEGER NOT NULL,
        strategy TEXT NOT NULL,
        stats TEXT NOT NULL,
        metrics TEXT NOT NULL,
        equity_curve TEXT NOT NULL,
        trades TEXT NOT NULL
    )";

    fn temp_db_path(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir()
            .join(format!("db_test_{}_{}_{}", name, std::process::id(), nanos))
            .join("runs.db")
    }

    #[test]
    fn migration_adds_owner_id_and_keeps_existing_rows() {
        let path = temp_db_path("migrate");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        {
            let conn = Connection::open(&path).expect("open old");
            conn.execute_batch(OLD_SCHEMA).expect("old schema");
            conn.execute(
                "INSERT INTO runs (created_at, symbol, interval, start_ms, end_ms, strategy, stats, metrics, equity_curve, trades)
                 VALUES (1, 'BTCUSDT', '1h', 0, 1, '{}', '{}', '{}', '[]', '[]')",
                [],
            )
            .expect("old row");
        }
        let db = Db::open(&path).expect("migrate");
        {
            let conn = db.conn.lock().expect("lock");
            let owner: String = conn
                .query_row("SELECT owner_id FROM runs WHERE id = 1", [], |r| r.get(0))
                .expect("owner of old row");
            assert_eq!(owner, "local");
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))
                .expect("count");
            assert_eq!(count, 1);
        }
        let id = db
            .save_run(
                "BTCUSDT",
                "1h",
                0,
                1,
                &serde_json::json!({}),
                &serde_json::json!({}),
                &serde_json::json!({}),
                &serde_json::json!([]),
                &serde_json::json!([]),
            )
            .expect("save");
        {
            let conn = db.conn.lock().expect("lock");
            let owner: String = conn
                .query_row("SELECT owner_id FROM runs WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .expect("owner of new row");
            assert_eq!(owner, "local");
        }
        drop(db);
        // Re-opening a migrated database must be a no-op.
        let _ = Db::open(&path).expect("reopen");
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }
}
