use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

/// One persisted backtest run.
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

/// Lightweight row for the runs table (no big blobs).
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
    trades TEXT NOT NULL
)";

impl Db {
    fn init(conn: &Connection) -> Result<(), String> {
        conn.execute_batch(SCHEMA)
            .map_err(|e| format!("init runs db: {e}"))?;
        Ok(())
    }

    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| format!("create db dir: {e}"))?;
            }
        }
        let conn = Connection::open(path).map_err(|e| format!("open runs db: {e}"))?;
        Self::init(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn open_in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory().map_err(|e| format!("open memory db: {e}"))?;
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
    ) -> Result<i64, String> {
        let conn = self.conn.lock().map_err(|e| format!("db lock: {e}"))?;
        conn.execute(
            "INSERT INTO runs (created_at, symbol, interval, start_ms, end_ms, strategy, stats, metrics, equity_curve, trades)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
        .map_err(|e| format!("save run: {e}"))?;
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

    pub fn list_runs(&self, limit: usize) -> Result<Vec<RunSummary>, String> {
        let conn = self.conn.lock().map_err(|e| format!("db lock: {e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, created_at, symbol, interval, start_ms, end_ms, stats
                 FROM runs ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|e| format!("list runs: {e}"))?;
        let rows = stmt
            .query_map(params![limit as i64], Self::summary_from_row)
            .map_err(|e| format!("list runs: {e}"))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| format!("list runs: {e}"))?);
        }
        Ok(out)
    }

    pub fn get_run(&self, id: i64) -> Result<Option<SavedRun>, String> {
        let conn = self.conn.lock().map_err(|e| format!("db lock: {e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, created_at, symbol, interval, start_ms, end_ms,
                        strategy, stats, metrics, equity_curve, trades
                 FROM runs WHERE id = ?1",
            )
            .map_err(|e| format!("get run: {e}"))?;
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
            .map_err(|e| format!("get run: {e}"))?;
        match rows.next() {
            None => Ok(None),
            Some(r) => Ok(Some(r.map_err(|e| format!("get run: {e}"))?)),
        }
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
}
