use axum::{
    extract::{rejection::JsonRejection, rejection::QueryRejection, Path, Query, State},
    http::{HeaderValue, Method, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
};
use tower_http::cors::{AllowOrigin, CorsLayer};

#[derive(Clone)]
pub struct AppState {
    pub source: Arc<dyn crate::feeds::KlineSource + Send + Sync>,
    pub cache_dir: PathBuf,
    pub jobs: Arc<JobStore>,
    pub db: Arc<crate::db::Db>,
    pub web_dir: PathBuf,
}

pub struct JobStore {
    next_id: AtomicU64,
    inner: RwLock<HashMap<String, Job>>,
}

impl Default for JobStore {
    fn default() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            inner: RwLock::new(HashMap::new()),
        }
    }
}

impl JobStore {
    fn insert(&self, job: Job) {
        if let Ok(mut map) = self.inner.write() {
            map.insert(job.job_id.clone(), job);
        }
    }

    fn update(&self, job_id: &str, f: impl FnOnce(&mut Job)) {
        if let Ok(mut map) = self.inner.write() {
            if let Some(job) = map.get_mut(job_id) {
                f(job);
            }
        }
    }

    fn get(&self, job_id: &str) -> Option<Job> {
        self.inner.read().ok()?.get(job_id).cloned()
    }

    fn summaries(&self) -> Vec<JobSummary> {
        self.inner
            .read()
            .map(|map| map.values().map(JobSummary::from).collect())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Completed,
    Failed,
}

/// An async optimizer sweep. `results` is the ranked trial table
/// (in-sample vs. out-of-sample per trial); it is `None` until the job
/// completes.
#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub job_id: String,
    pub status: JobStatus,
    pub total_trials: usize,
    pub completed_trials: usize,
    pub objective: String,
    pub validation: String,
    pub results: Option<Vec<crate::optimizer::RankedTrial>>,
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct JobSummary {
    job_id: String,
    status: JobStatus,
    total_trials: usize,
    completed_trials: usize,
}

impl From<&Job> for JobSummary {
    fn from(j: &Job) -> Self {
        Self {
            job_id: j.job_id.clone(),
            status: j.status,
            total_trials: j.total_trials,
            completed_trials: j.completed_trials,
        }
    }
}

const DEFAULT_ORIGIN: &str = "http://localhost:3000";

pub fn app(state: AppState) -> Router {
    app_with_origin(state, DEFAULT_ORIGIN).expect("default origin parses")
}

pub fn app_with_origin(state: AppState, origin: &str) -> Result<Router, String> {
    let origin_header: HeaderValue = origin
        .parse()
        .map_err(|e| format!("invalid origin {origin:?}: {e}"))?;
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list([origin_header]))
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([axum::http::header::CONTENT_TYPE]);
    Ok(Router::new()
        .route("/health", get(health))
        .route("/candles", get(get_candles))
        .route("/backtest", post(post_backtest))
        .route("/optimize", post(post_optimize).get(list_optimize))
        .route("/optimize/{job_id}", get(get_optimize))
        .route("/runs", get(list_runs))
        .route("/runs/{id}", get(get_run))
        .route("/", get(serve_index))
        .route("/app.js", get(serve_app_js))
        .route("/styles.css", get(serve_styles))
        .with_state(state)
        .layer(cors))
}

fn err_json(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(serde_json::json!({"error": msg.into()})))
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

#[derive(Debug, Deserialize)]
struct CandlesQuery {
    symbol: String,
    interval: String,
    start: i64,
    end: i64,
}

fn check_range_limit(start: i64, end: i64, step: i64) -> Result<(), String> {
    let count = (end as i128 - start as i128) / step as i128 + 1;
    if count > 100_000 {
        return Err(format!("range would hold {count} candles, limit is 100000"));
    }
    Ok(())
}

async fn get_candles(
    State(state): State<AppState>,
    query: Result<Query<CandlesQuery>, QueryRejection>,
) -> Response {
    let q = match query {
        Ok(q) => q.0,
        Err(e) => return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    if let Err(e) = crate::feeds::validate_symbol(&q.symbol) {
        return err_json(StatusCode::BAD_REQUEST, e).into_response();
    }
    let step = match crate::feeds::interval_ms(&q.interval) {
        Some(s) => s,
        None => {
            return err_json(
                StatusCode::BAD_REQUEST,
                format!("unknown interval: {}", q.interval),
            )
            .into_response();
        }
    };
    if q.start >= q.end {
        return err_json(
            StatusCode::BAD_REQUEST,
            "start must be less than end".to_string(),
        )
        .into_response();
    }
    if let Err(e) = check_range_limit(q.start, q.end, step) {
        return err_json(StatusCode::BAD_REQUEST, e).into_response();
    }
    let source = state.source.clone();
    let cache_dir = state.cache_dir.clone();
    let symbol = q.symbol.clone();
    let interval = q.interval.clone();
    let start = q.start;
    let end = q.end;
    let loaded = tokio::task::spawn_blocking(move || {
        crate::feeds::load_or_fetch(&cache_dir, source.as_ref(), &symbol, &interval, start, end)
    })
    .await;
    let candles = match loaded {
        Err(e) => {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
        Ok(Err(e)) => return err_json(StatusCode::BAD_GATEWAY, e).into_response(),
        Ok(Ok(c)) => c,
    };
    let gaps = crate::feeds::find_gaps(&candles, step);
    let body = serde_json::json!({
        "symbol": q.symbol,
        "interval": q.interval,
        "count": candles.len(),
        "candles": candles,
        "gaps": gaps,
    });
    (StatusCode::OK, Json(body)).into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BacktestBody {
    strategy: crate::strategy::Strategy,
    start_ms: i64,
    end_ms: i64,
    fee_bps: Option<f64>,
    slippage_bps: Option<f64>,
    split: Option<f64>,
}

#[derive(Debug, Serialize)]
struct BacktestResponse {
    run_id: Option<i64>,
    candle_count: usize,
    gaps: Vec<(i64, i64)>,
    stats: crate::stats::Stats,
    metrics: crate::stats::MetricsV2,
    in_sample: Option<crate::stats::Stats>,
    out_of_sample: Option<crate::stats::Stats>,
    trades: Vec<crate::backtest::Trade>,
    equity_curve: Vec<f64>,
}

async fn post_backtest(
    State(state): State<AppState>,
    body: Result<Json<BacktestBody>, JsonRejection>,
) -> Response {
    let req = match body {
        Ok(Json(b)) => b,
        Err(e) => return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    if let Err(e) = req.strategy.validate() {
        return err_json(StatusCode::BAD_REQUEST, e).into_response();
    }
    let symbol = req.strategy.asset.clone();
    let interval = req.strategy.timeframe.clone();
    if let Err(e) = crate::feeds::validate_symbol(&symbol) {
        return err_json(StatusCode::BAD_REQUEST, e).into_response();
    }
    let step = match crate::feeds::interval_ms(&interval) {
        Some(s) => s,
        None => {
            return err_json(
                StatusCode::BAD_REQUEST,
                format!("unknown interval: {interval}"),
            )
            .into_response();
        }
    };
    if req.start_ms >= req.end_ms {
        return err_json(
            StatusCode::BAD_REQUEST,
            "start_ms must be less than end_ms".to_string(),
        )
        .into_response();
    }
    if let Err(e) = check_range_limit(req.start_ms, req.end_ms, step) {
        return err_json(StatusCode::BAD_REQUEST, e).into_response();
    }
    let defaults = crate::backtest::BacktestConfig::default();
    let fee_bps = req.fee_bps.unwrap_or(defaults.fee_bps);
    let slippage_bps = req.slippage_bps.unwrap_or(defaults.slippage_bps);
    if !(0.0..=1000.0).contains(&fee_bps) || !fee_bps.is_finite() {
        return err_json(
            StatusCode::BAD_REQUEST,
            format!("fee_bps must be between 0 and 1000, got {fee_bps}"),
        )
        .into_response();
    }
    if !(0.0..=1000.0).contains(&slippage_bps) || !slippage_bps.is_finite() {
        return err_json(
            StatusCode::BAD_REQUEST,
            format!("slippage_bps must be between 0 and 1000, got {slippage_bps}"),
        )
        .into_response();
    }
    let bars_per_year = match crate::stats::bars_per_year(&interval) {
        Some(b) => b,
        None => {
            return err_json(
                StatusCode::BAD_REQUEST,
                format!("unknown timeframe: {interval}"),
            )
            .into_response();
        }
    };
    if let Some(split) = req.split {
        if !(split > 0.0 && split < 1.0) || !split.is_finite() {
            return err_json(
                StatusCode::BAD_REQUEST,
                format!("split must be strictly between 0 and 1, got {split}"),
            )
            .into_response();
        }
    }
    let source = state.source.clone();
    let cache_dir = state.cache_dir.clone();
    let start = req.start_ms;
    let end = req.end_ms;
    let loaded = tokio::task::spawn_blocking(move || {
        crate::feeds::load_or_fetch(&cache_dir, source.as_ref(), &symbol, &interval, start, end)
    })
    .await;
    let candles = match loaded {
        Err(e) => {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
        Ok(Err(e)) => return err_json(StatusCode::BAD_GATEWAY, e).into_response(),
        Ok(Ok(c)) => c,
    };
    if candles.is_empty() {
        return err_json(StatusCode::BAD_REQUEST, "no candles in range".to_string())
            .into_response();
    }
    let cfg = crate::backtest::BacktestConfig {
        fee_bps,
        slippage_bps,
    };
    let result = match crate::backtest::backtest(&req.strategy, &candles, &cfg) {
        Ok(r) => r,
        Err(e) => return err_json(StatusCode::BAD_REQUEST, e).into_response(),
    };
    let stats = crate::stats::compute_stats(&result, &candles, bars_per_year);
    let (in_sample, out_of_sample) = match req.split {
        None => (None, None),
        Some(split) => {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let split_idx = (candles.len() as f64 * split).floor() as usize;
            match crate::stats::split_stats(&result, &candles, split_idx, bars_per_year) {
                Ok((is, oos)) => (Some(is), Some(oos)),
                Err(e) => return err_json(StatusCode::BAD_REQUEST, e).into_response(),
            }
        }
    };
    let gaps = crate::feeds::find_gaps(&candles, step);
    let metrics = crate::stats::compute_v2(&result, &candles, bars_per_year);
    let stats_v = serde_json::to_value(&stats).unwrap_or_default();
    let metrics_v = serde_json::to_value(&metrics).unwrap_or_default();
    let equity_v =
        serde_json::to_value(&result.equity_curve).unwrap_or_default();
    let trades_v = serde_json::to_value(&result.trades).unwrap_or_default();
    let strategy_v = serde_json::to_value(&req.strategy).unwrap_or_default();
    let run_id = match state.db.save_run(
        &req.strategy.asset,
        &req.strategy.timeframe,
        req.start_ms,
        req.end_ms,
        &strategy_v,
        &stats_v,
        &metrics_v,
        &equity_v,
        &trades_v,
    ) {
        Ok(id) => Some(id),
        Err(e) => {
            return err_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("save run: {e}"),
            )
            .into_response()
        }
    };
    let resp = BacktestResponse {
        run_id,
        candle_count: candles.len(),
        gaps,
        stats,
        metrics,
        in_sample,
        out_of_sample,
        trades: result.trades,
        equity_curve: result.equity_curve,
    };
    (
        StatusCode::OK,
        Json(serde_json::to_value(resp).unwrap_or_default()),
    )
        .into_response()
}

async fn post_optimize(
    State(state): State<AppState>,
    body: Result<Json<crate::optimizer::OptimizeSpec>, JsonRejection>,
) -> Response {
    let req = match body {
        Ok(Json(b)) => b,
        Err(e) => return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    if let Err(e) = req.strategy.validate() {
        return err_json(
            StatusCode::BAD_REQUEST,
            format!("invalid base strategy: {e}"),
        )
        .into_response();
    }
    // Fail fast on bad search specs before touching market data.
    let combos_len = match crate::optimizer::build_combos(&req) {
        Ok(c) => c.len(),
        Err(e) => return err_json(StatusCode::BAD_REQUEST, e).into_response(),
    };
    let symbol = req.strategy.asset.clone();
    let interval = req.strategy.timeframe.clone();
    if let Err(e) = crate::feeds::validate_symbol(&symbol) {
        return err_json(StatusCode::BAD_REQUEST, e).into_response();
    }
    let step = match crate::feeds::interval_ms(&interval) {
        Some(s) => s,
        None => {
            return err_json(
                StatusCode::BAD_REQUEST,
                format!("unknown interval: {interval}"),
            )
            .into_response();
        }
    };
    if req.start_ms >= req.end_ms {
        return err_json(
            StatusCode::BAD_REQUEST,
            "start_ms must be less than end_ms".to_string(),
        )
        .into_response();
    }
    if let Err(e) = check_range_limit(req.start_ms, req.end_ms, step) {
        return err_json(StatusCode::BAD_REQUEST, e).into_response();
    }
    let defaults = crate::backtest::BacktestConfig::default();
    let fee_bps = req.fee_bps.unwrap_or(defaults.fee_bps);
    let slippage_bps = req.slippage_bps.unwrap_or(defaults.slippage_bps);
    if !(0.0..=1000.0).contains(&fee_bps) || !fee_bps.is_finite() {
        return err_json(
            StatusCode::BAD_REQUEST,
            format!("fee_bps must be between 0 and 1000, got {fee_bps}"),
        )
        .into_response();
    }
    if !(0.0..=1000.0).contains(&slippage_bps) || !slippage_bps.is_finite() {
        return err_json(
            StatusCode::BAD_REQUEST,
            format!("slippage_bps must be between 0 and 1000, got {slippage_bps}"),
        )
        .into_response();
    }
    let bars_per_year = match crate::stats::bars_per_year(&interval) {
        Some(b) => b,
        None => {
            return err_json(
                StatusCode::BAD_REQUEST,
                format!("unknown timeframe: {interval}"),
            )
            .into_response();
        }
    };
    let source = state.source.clone();
    let cache_dir = state.cache_dir.clone();
    let start = req.start_ms;
    let end = req.end_ms;
    let loaded = tokio::task::spawn_blocking(move || {
        crate::feeds::load_or_fetch(&cache_dir, source.as_ref(), &symbol, &interval, start, end)
    })
    .await;
    let candles = match loaded {
        Err(e) => {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
        Ok(Err(e)) => return err_json(StatusCode::BAD_GATEWAY, e).into_response(),
        Ok(Ok(c)) => c,
    };
    if candles.is_empty() {
        return err_json(StatusCode::BAD_REQUEST, "no candles in range".to_string())
            .into_response();
    }

    let objective = req.objective.unwrap_or_default().as_str().to_string();
    let job_id = format!("opt-{}", state.jobs.next_id.fetch_add(1, Ordering::SeqCst));
    state.jobs.insert(Job {
        job_id: job_id.clone(),
        status: JobStatus::Running,
        total_trials: combos_len,
        completed_trials: 0,
        objective,
        validation: String::new(),
        results: None,
        warnings: Vec::new(),
        error: None,
    });

    let jobs = state.jobs.clone();
    let reply_id = job_id.clone();
    tokio::task::spawn_blocking(move || {
        let cfg = crate::backtest::BacktestConfig {
            fee_bps,
            slippage_bps,
        };
        match crate::optimizer::sweep(&req, &candles, &cfg, bars_per_year) {
            Ok(report) => jobs.update(&reply_id, |job| {
                job.status = JobStatus::Completed;
                job.completed_trials = report.total_trials;
                job.validation = report.validation.clone();
                job.results = Some(report.trials);
                job.warnings = report.warnings;
            }),
            Err(e) => jobs.update(&reply_id, |job| {
                job.status = JobStatus::Failed;
                job.error = Some(e);
            }),
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "job_id": job_id,
            "status": JobStatus::Running,
            "total_trials": combos_len,
        })),
    )
        .into_response()
}

async fn get_optimize(State(state): State<AppState>, Path(job_id): Path<String>) -> Response {
    match state.jobs.get(&job_id) {
        Some(job) => (
            StatusCode::OK,
            Json(serde_json::to_value(job).unwrap_or_default()),
        )
            .into_response(),
        None => err_json(StatusCode::NOT_FOUND, format!("unknown job: {job_id}")).into_response(),
    }
}

async fn list_optimize(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({ "jobs": state.jobs.summaries() }))
}

async fn list_runs(State(state): State<AppState>) -> Response {
    match state.db.list_runs(100) {
        Ok(runs) => (StatusCode::OK, Json(serde_json::json!({ "runs": runs }))).into_response(),
        Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

async fn get_run(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    match state.db.get_run(id) {
        Ok(Some(run)) => (
            StatusCode::OK,
            Json(serde_json::to_value(run).unwrap_or_default()),
        )
            .into_response(),
        Ok(None) => err_json(StatusCode::NOT_FOUND, format!("unknown run: {id}")).into_response(),
        Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

fn serve_file(dir: &PathBuf, name: &str, content_type: &str) -> Response {
    let path = dir.join(name);
    match std::fs::read(&path) {
        Ok(bytes) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, content_type)],
            bytes,
        )
            .into_response(),
        Err(_) => err_json(StatusCode::NOT_FOUND, format!("missing {name}")).into_response(),
    }
}

async fn serve_index(State(state): State<AppState>) -> Response {
    serve_file(&state.web_dir, "index.html", "text/html; charset=utf-8")
}

async fn serve_app_js(State(state): State<AppState>) -> Response {
    serve_file(
        &state.web_dir,
        "app.js",
        "application/javascript; charset=utf-8",
    )
}

async fn serve_styles(State(state): State<AppState>) -> Response {
    serve_file(&state.web_dir, "styles.css", "text/css; charset=utf-8")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    struct FakeSource {
        data: Vec<crate::types::Candle>,
    }

    impl FakeSource {
        fn new() -> Self {
            let start: i64 = 1_700_006_400_000;
            let step: i64 = 3_600_000;
            let mut prev_close: f64 = 100.0;
            let mut data = Vec::with_capacity(300);
            for i in 0..300_i64 {
                let close: f64 = 100.0 + 10.0 * (i as f64 * 0.3).sin();
                let open: f64 = if i == 0 { 100.0 } else { prev_close };
                let high = open.max(close) + 1.0;
                let low = open.min(close) - 1.0;
                data.push(crate::types::Candle {
                    ts: start + i * step,
                    open,
                    high,
                    low,
                    close,
                });
                prev_close = close;
            }
            Self { data }
        }

        fn start(&self) -> i64 {
            self.data.first().map(|c| c.ts).unwrap_or(0)
        }

        fn end(&self) -> i64 {
            self.data.last().map(|c| c.ts).unwrap_or(0)
        }
    }

    impl crate::feeds::KlineSource for FakeSource {
        fn fetch(
            &self,
            _symbol: &str,
            _interval: &str,
            start_ms: i64,
            end_ms: i64,
            limit: usize,
        ) -> Result<Vec<crate::types::Candle>, String> {
            Ok(self
                .data
                .iter()
                .filter(|c| c.ts >= start_ms && c.ts <= end_ms)
                .take(limit)
                .cloned()
                .collect())
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "api_test_{}_{}_{}",
            name,
            std::process::id(),
            nanos
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn test_state(dir: PathBuf) -> AppState {
        AppState {
            source: Arc::new(FakeSource::new()),
            cache_dir: dir,
            jobs: Arc::new(JobStore::default()),
            db: Arc::new(crate::db::Db::open_in_memory().expect("mem db")),
            web_dir: PathBuf::from("web"),
        }
    }

    async fn body_json(res: axum::response::Response) -> serde_json::Value {
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn valid_strategy() -> serde_json::Value {
        serde_json::json!({
            "asset": "BTCUSDT",
            "timeframe": "1h",
            "side": "long",
            "entry": {"all": [{"left": {"ind": "close"}, "op": ">", "right": 100.0}]},
            "exit": {"any": [{"left": {"ind": "close"}, "op": "<", "right": 100.0}]}
        })
    }

    #[tokio::test]
    async fn health_returns_200() {
        let dir = temp_dir("health");
        let res = app(test_state(dir.clone()))
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert_eq!(v["status"], "ok");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn candles_valid_range() {
        let dir = temp_dir("candles_valid");
        let fake = FakeSource::new();
        let start = fake.start();
        let end = fake.end();
        let state_router = app(test_state(dir.clone()));
        let uri = format!("/candles?symbol=BTCUSDT&interval=1h&start={start}&end={end}");
        let res = state_router
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        let candles = v["candles"].as_array().unwrap();
        assert_eq!(v["count"].as_u64().unwrap() as usize, candles.len());
        let mut prev: Option<i64> = None;
        for c in candles {
            let ts = c["ts"].as_i64().unwrap();
            if let Some(p) = prev {
                assert!(ts > p, "timestamps must ascend");
            }
            prev = Some(ts);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn candles_rejects_bad_input() {
        let dir = temp_dir("candles_bad");
        let fake = FakeSource::new();
        let start = fake.start();
        let end = fake.end();
        let step: i64 = 3_600_000;
        let too_far = start + step * 100_001;

        let cases = vec![
            format!("/candles?symbol=..%2Fx&interval=1h&start={start}&end={end}"),
            format!("/candles?symbol=BTCUSDT&interval=7m&start={start}&end={end}"),
            format!("/candles?symbol=BTCUSDT&interval=1h&start={end}&end={start}"),
            format!("/candles?symbol=BTCUSDT&interval=1h&start={end}&end={end}"),
            format!("/candles?symbol=BTCUSDT&interval=1h&start={start}&end={too_far}"),
        ];
        for uri in cases {
            let res = app(test_state(dir.clone()))
                .oneshot(Request::get(uri.clone()).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::BAD_REQUEST, "uri: {uri}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn backtest_valid_strategy() {
        let dir = temp_dir("bt_valid");
        let fake = FakeSource::new();
        let body = serde_json::json!({
            "strategy": valid_strategy(),
            "start_ms": fake.start(),
            "end_ms": fake.end(),
        });
        let res = app(test_state(dir.clone()))
            .oneshot(
                Request::post("/backtest")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert!(v["stats"]["trades"].as_u64().unwrap() > 0);
        let candle_count = v["candle_count"].as_u64().unwrap() as usize;
        assert_eq!(v["equity_curve"].as_array().unwrap().len(), candle_count);
        assert!(v["in_sample"].is_null());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn backtest_misspelled_field_is_4xx() {
        let dir = temp_dir("bt_misspell");
        let fake = FakeSource::new();
        let mut strat = valid_strategy();
        strat["stop_loss"] = serde_json::json!(2.0);
        let body = serde_json::json!({
            "strategy": strat,
            "start_ms": fake.start(),
            "end_ms": fake.end(),
        });
        let res = app(test_state(dir.clone()))
            .oneshot(
                Request::post("/backtest")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            res.status().is_client_error(),
            "expected 4xx, got {}",
            res.status()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn backtest_empty_entry_mentions_entry() {
        let dir = temp_dir("bt_empty");
        let fake = FakeSource::new();
        let mut strat = valid_strategy();
        strat["entry"] = serde_json::json!({"all": [], "any": []});
        let body = serde_json::json!({
            "strategy": strat,
            "start_ms": fake.start(),
            "end_ms": fake.end(),
        });
        let res = app(test_state(dir.clone()))
            .oneshot(
                Request::post("/backtest")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let v = body_json(res).await;
        let msg = v["error"].as_str().unwrap_or("").to_lowercase();
        assert!(msg.contains("entry"), "error should mention entry: {v}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn backtest_split() {
        let dir = temp_dir("bt_split");
        let fake = FakeSource::new();
        let body = serde_json::json!({
            "strategy": valid_strategy(),
            "start_ms": fake.start(),
            "end_ms": fake.end(),
            "split": 0.7,
        });
        let res = app(test_state(dir.clone()))
            .oneshot(
                Request::post("/backtest")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert!(v["in_sample"]["trades"].is_number());
        assert!(v["out_of_sample"]["trades"].is_number());

        for split in [1.5, 0.0] {
            let body = serde_json::json!({
                "strategy": valid_strategy(),
                "start_ms": fake.start(),
                "end_ms": fake.end(),
                "split": split,
            });
            let res = app(test_state(dir.clone()))
                .oneshot(
                    Request::post("/backtest")
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::BAD_REQUEST, "split {split}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn backtest_bad_timeframe() {
        let dir = temp_dir("bt_tf");
        let fake = FakeSource::new();
        let mut strat = valid_strategy();
        strat["timeframe"] = serde_json::json!("7m");
        let body = serde_json::json!({
            "strategy": strat,
            "start_ms": fake.start(),
            "end_ms": fake.end(),
        });
        let res = app(test_state(dir.clone()))
            .oneshot(
                Request::post("/backtest")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn backtest_bad_fee() {
        let dir = temp_dir("bt_fee");
        let fake = FakeSource::new();
        let body = serde_json::json!({
            "strategy": valid_strategy(),
            "start_ms": fake.start(),
            "end_ms": fake.end(),
            "fee_bps": 5000.0,
        });
        let res = app(test_state(dir.clone()))
            .oneshot(
                Request::post("/backtest")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct FailingSource;

    impl crate::feeds::KlineSource for FailingSource {
        fn fetch(
            &self,
            _symbol: &str,
            _interval: &str,
            _start_ms: i64,
            _end_ms: i64,
            _limit: usize,
        ) -> Result<Vec<crate::types::Candle>, String> {
            Err("boom".to_string())
        }
    }

    #[tokio::test]
    async fn cors_allows_configured_origin() {
        let dir = temp_dir("cors_allow");
        let res = app(test_state(dir.clone()))
            .oneshot(
                Request::get("/health")
                    .header("Origin", "http://localhost:3000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let allow = res
            .headers()
            .get("access-control-allow-origin")
            .expect("cors allow-origin header");
        assert_eq!(allow, "http://localhost:3000");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn cors_blocks_other_origin() {
        let dir = temp_dir("cors_block");
        let res = app(test_state(dir.clone()))
            .oneshot(
                Request::get("/health")
                    .header("Origin", "http://evil.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(
            res.headers().get("access-control-allow-origin").is_none(),
            "evil origin must not get allow-origin header"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn cors_preflight_allowed_and_blocked() {
        let dir = temp_dir("cors_preflight");
        let res = app(test_state(dir.clone()))
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/backtest")
                    .header("Origin", "http://localhost:3000")
                    .header("Access-Control-Request-Method", "POST")
                    .header("Access-Control-Request-Headers", "content-type")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            res.status().is_success(),
            "allowed preflight should succeed, got {}",
            res.status()
        );
        let allow = res
            .headers()
            .get("access-control-allow-origin")
            .expect("allowed preflight should have allow-origin");
        assert_eq!(allow, "http://localhost:3000");

        let res = app(test_state(dir.clone()))
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/backtest")
                    .header("Origin", "http://evil.example")
                    .header("Access-Control-Request-Method", "POST")
                    .header("Access-Control-Request-Headers", "content-type")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            res.headers().get("access-control-allow-origin").is_none(),
            "blocked preflight must not have allow-origin header"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn candles_upstream_failure_is_502() {
        let dir = temp_dir("candles_502");
        let fake = FakeSource::new();
        let state = AppState {
            source: Arc::new(FailingSource),
            cache_dir: dir.clone(),
            jobs: Arc::new(JobStore::default()),
            db: Arc::new(crate::db::Db::open_in_memory().expect("mem db")),
            web_dir: PathBuf::from("web"),
        };
        let uri = format!(
            "/candles?symbol=BTCUSDT&interval=1h&start={}&end={}",
            fake.start(),
            fake.end()
        );
        let res = app(state)
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
        let v = body_json(res).await;
        let msg = v["error"].as_str().unwrap_or("").to_lowercase();
        assert!(msg.contains("boom"), "error should mention boom: {v}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn backtest_empty_range_is_400() {
        let dir = temp_dir("bt_empty_range");
        let body = serde_json::json!({
            "strategy": valid_strategy(),
            "start_ms": 1_600_000_000_000i64,
            "end_ms": 1_600_100_000_000i64,
        });
        let res = app(test_state(dir.clone()))
            .oneshot(
                Request::post("/backtest")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let v = body_json(res).await;
        let msg = v["error"].as_str().unwrap_or("").to_lowercase();
        assert!(
            msg.contains("no candles"),
            "error should mention no candles: {v}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn candles_empty_range_is_200_count_zero() {
        let dir = temp_dir("candles_empty");
        let uri = "/candles?symbol=BTCUSDT&interval=1h&start=1600000000000&end=1600100000000";
        let res = app(test_state(dir.clone()))
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert_eq!(v["count"].as_u64().unwrap(), 0);
        assert!(v["candles"].as_array().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn app_with_origin_rejects_bad_origin() {
        let dir = temp_dir("bad_origin");
        let state = test_state(dir.clone());
        let res = app_with_origin(state, "bad\norigin");
        assert!(res.is_err(), "expected Err for bad origin");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn optimize_body(fake: &FakeSource, trials: usize) -> serde_json::Value {
        serde_json::json!({
            "strategy": valid_strategy(),
            "start_ms": fake.start(),
            "end_ms": fake.end(),
            "search": {"method": "random", "trials": trials, "seed": 7},
            "params": [
                {"target": "entry.all.0.right", "kind": "float", "min": 95.0, "max": 105.0},
                {"target": "stop_loss_pct", "kind": "float", "values": [1.0, 2.0]},
            ],
        })
    }

    async fn poll_job(router: &Router, job_id: &str) -> serde_json::Value {
        for _ in 0..200 {
            let res = router
                .clone()
                .oneshot(
                    Request::get(format!("/optimize/{job_id}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            let v = body_json(res).await;
            if v["status"] == "completed" {
                return v;
            }
            assert_ne!(v["status"], "failed", "job failed: {v}");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("job {job_id} did not complete in time");
    }

    #[tokio::test]
    async fn optimize_200_trial_sweep_returns_ranked_table() {
        let dir = temp_dir("opt_200");
        let fake = FakeSource::new();
        let router = app(test_state(dir.clone()));
        let res = router
            .clone()
            .oneshot(
                Request::post("/optimize")
                    .header("content-type", "application/json")
                    .body(Body::from(optimize_body(&fake, 200).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED, "expected 202");
        let created = body_json(res).await;
        assert_eq!(created["total_trials"].as_u64().unwrap(), 200);
        let job_id = created["job_id"].as_str().unwrap().to_string();

        let job = poll_job(&router, &job_id).await;
        let trials = job["results"].as_array().expect("ranked table");
        assert_eq!(trials.len(), 200);
        for (i, t) in trials.iter().enumerate() {
            assert_eq!(t["rank"].as_u64().unwrap() as usize, i + 1);
            // In-sample vs out-of-sample performance per trial.
            assert!(t["in_sample"]["trades"].is_number(), "trial {i}");
            assert!(t["out_of_sample"]["trades"].is_number(), "trial {i}");
            assert!(t["score"].is_number(), "trial {i}");
            assert!(t["params"]["entry.all.0.right"].is_number());
        }
        for w in trials.windows(2) {
            let a = w[0]["score"].as_f64().unwrap();
            let b = w[1]["score"].as_f64().unwrap();
            assert!(a >= b, "ranked table must be score-descending");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn optimize_bad_target_is_400() {
        let dir = temp_dir("opt_bad");
        let fake = FakeSource::new();
        let mut body = optimize_body(&fake, 4);
        body["params"] = serde_json::json!([
            {"target": "entry.all.9.right", "kind": "float", "values": [100.0]},
        ]);
        let res = app(test_state(dir.clone()))
            .oneshot(
                Request::post("/optimize")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn optimize_unknown_job_is_404() {
        let dir = temp_dir("opt_404");
        let res = app(test_state(dir.clone()))
            .oneshot(
                Request::get("/optimize/opt-999999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn backtest_saves_run_and_lists_it() {
        let dir = temp_dir("runs_save");
        let fake = FakeSource::new();
        let router = app(test_state(dir.clone()));
        let body = serde_json::json!({
            "strategy": valid_strategy(),
            "start_ms": fake.start(),
            "end_ms": fake.end(),
        });
        let res = router
            .clone()
            .oneshot(
                Request::post("/backtest")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        let run_id = v["run_id"].as_i64().expect("run_id");
        assert!(v["metrics"]["monthly"].is_object(), "metrics.monthly: {v}");
        assert!(v["equity_curve"].is_array());

        let res = router
            .clone()
            .oneshot(Request::get("/runs").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let listed = body_json(res).await;
        let runs = listed["runs"].as_array().expect("runs array");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0]["id"].as_i64().unwrap(), run_id);
        assert_eq!(runs[0]["symbol"].as_str().unwrap(), "BTCUSDT");

        let res = router
            .clone()
            .oneshot(
                Request::get(format!("/runs/{run_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let detail = body_json(res).await;
        assert_eq!(detail["id"].as_i64().unwrap(), run_id);
        assert!(detail["equity_curve"].is_array());
        assert!(detail["trades"].is_array());

        let res = router
            .oneshot(Request::get("/runs/999999").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
