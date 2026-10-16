use axum::{
    extract::{
        rejection::JsonRejection, rejection::QueryRejection, ConnectInfo, DefaultBodyLimit,
        Extension, Path, Query, State,
    },
    http::{request::Parts, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
    time::Duration,
};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::ratelimit::RateLimiter;

const BACKTEST_TIMEOUT_SECS: u64 = 60;
const BACKTEST_TIMEOUT: Duration = Duration::from_secs(BACKTEST_TIMEOUT_SECS);
const BODY_LIMIT_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub struct Limits {
    pub backtest: Arc<RateLimiter>,
    pub optimize: Arc<RateLimiter>,
    pub max_optimizer_jobs: usize,
}

impl Limits {
    pub fn new(backtest_per_min: u32, optimize_per_min: u32, max_optimizer_jobs: usize) -> Self {
        Self {
            backtest: Arc::new(RateLimiter::per_minute(backtest_per_min)),
            optimize: Arc::new(RateLimiter::per_minute(optimize_per_min)),
            max_optimizer_jobs,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new(30, 5, 2)
    }
}

// A reverse proxy makes the peer address the proxy, so the forwarded address
// comes first; the peer address covers direct connections.
struct ClientIp(String);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let forwarded = parts
            .headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0.ip().to_string());
        Ok(ClientIp(
            forwarded
                .map(str::to_string)
                .or(peer)
                .unwrap_or_else(|| "unknown".to_string()),
        ))
    }
}

fn rate_limited(retry_after_secs: u64, msg: &str) -> Response {
    let mut res = err_json(StatusCode::TOO_MANY_REQUESTS, msg).into_response();
    if let Ok(value) = HeaderValue::from_str(&retry_after_secs.to_string()) {
        res.headers_mut()
            .insert(axum::http::header::RETRY_AFTER, value);
    }
    res
}

#[derive(Clone)]
pub struct AppState {
    pub source: Arc<dyn crate::feeds::KlineSource + Send + Sync>,
    pub cache_dir: PathBuf,
    pub jobs: Arc<JobStore>,
    pub db: Arc<crate::db::Db>,
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
    // Admits at most `max_running` jobs that are still running, so the check
    // and the insert cannot race.
    fn try_insert_running(&self, max_running: usize, job: Job) -> bool {
        let mut map = match self.inner.write() {
            Ok(map) => map,
            Err(_) => return false,
        };
        let running = map
            .values()
            .filter(|j| j.status == JobStatus::Running)
            .count();
        if running >= max_running {
            return false;
        }
        map.insert(job.job_id.clone(), job);
        true
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

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("invalid origin {0:?}: {1}")]
    BadOrigin(String, String),
    #[error("range would hold {0} candles, limit is 100000")]
    TooMany(i128),
}

#[cfg(test)]
const DEFAULT_ORIGIN: &str = "http://localhost:3000";

#[cfg(test)]
pub fn app(state: AppState) -> Router {
    let fallback = state.clone();
    app_with_origin(state, DEFAULT_ORIGIN).unwrap_or_else(|_| {
        Router::new()
            .route("/health", get(health))
            .with_state(fallback)
    })
}

#[cfg(test)]
pub fn app_with_origin(state: AppState, origin: &str) -> Result<Router, ApiError> {
    app_with_limits(state, origin, Limits::default())
}

pub fn app_with_limits(state: AppState, origins: &str, limits: Limits) -> Result<Router, ApiError> {
    let headers = crate::config::parse_origins(origins)
        .map_err(|msg| ApiError::BadOrigin(origins.to_string(), msg))?;
    Ok(router(state, headers, limits))
}

fn router(state: AppState, origins: Vec<HeaderValue>, limits: Limits) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([axum::http::header::CONTENT_TYPE]);
    Router::new()
        .route("/health", get(health))
        .route("/candles", get(get_candles))
        .route("/backtest", post(post_backtest))
        .route("/optimize", post(post_optimize).get(list_optimize))
        .route("/optimize/{job_id}", get(get_optimize))
        .route("/runs", get(list_runs))
        .route("/runs/{id}", get(get_run))
        .with_state(state)
        .layer(DefaultBodyLimit::max(BODY_LIMIT_BYTES))
        .layer(Extension(Arc::new(limits)))
        .layer(cors)
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

fn check_range_limit(start: i64, end: i64, step: i64) -> Result<(), ApiError> {
    let count = (end as i128 - start as i128) / step as i128 + 1;
    if count > 100_000 {
        return Err(ApiError::TooMany(count));
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
        return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response();
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
        return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response();
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
        Ok(Err(e)) => return err_json(StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
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
    verdict: crate::verdict::Verdict,
    trades: Vec<crate::backtest::Trade>,
    equity_curve: Vec<f64>,
}

struct SimRequest {
    symbol: String,
    interval: String,
    step: i64,
    start: i64,
    end: i64,
    split: Option<f64>,
    fee_bps: f64,
    slippage_bps: f64,
    bars_per_year: f64,
}

struct BacktestParts {
    candle_count: usize,
    gaps: Vec<(i64, i64)>,
    stats: crate::stats::Stats,
    metrics: crate::stats::MetricsV2,
    in_sample: Option<crate::stats::Stats>,
    out_of_sample: Option<crate::stats::Stats>,
    verdict: crate::verdict::Verdict,
    trades: Vec<crate::backtest::Trade>,
    equity_curve: Vec<f64>,
}

enum BacktestFailure {
    Upstream(String),
    BadRequest(String),
}

impl BacktestFailure {
    fn into_error_response(self) -> Response {
        match self {
            BacktestFailure::Upstream(msg) => {
                err_json(StatusCode::BAD_GATEWAY, msg).into_response()
            }
            BacktestFailure::BadRequest(msg) => {
                err_json(StatusCode::BAD_REQUEST, msg).into_response()
            }
        }
    }
}

fn run_backtest(
    cache_dir: &std::path::Path,
    source: &dyn crate::feeds::KlineSource,
    strategy: &crate::strategy::Strategy,
    req: &SimRequest,
) -> Result<BacktestParts, BacktestFailure> {
    let candles = crate::feeds::load_or_fetch(
        cache_dir,
        source,
        &req.symbol,
        &req.interval,
        req.start,
        req.end,
    )
    .map_err(|e| BacktestFailure::Upstream(e.to_string()))?;
    if candles.is_empty() {
        return Err(BacktestFailure::BadRequest(
            "no candles in range".to_string(),
        ));
    }
    let cfg = crate::backtest::BacktestConfig {
        fee_bps: req.fee_bps,
        slippage_bps: req.slippage_bps,
    };
    let result = crate::backtest::backtest_strategy(strategy, &candles, &cfg)
        .map_err(|e| BacktestFailure::BadRequest(e.to_string()))?;
    let stats = crate::stats::compute_stats(&result, &candles, req.bars_per_year);
    let verdict_split = req.split.unwrap_or(crate::verdict::DEFAULT_VERDICT_SPLIT);
    let split_idx = (candles.len() as f64 * verdict_split).floor() as usize;
    let split_pieces = crate::stats::split_stats(&result, &candles, split_idx, req.bars_per_year);
    let (in_sample, out_of_sample, verdict) = match (req.split, split_pieces) {
        (Some(_), Ok((is, oos))) => {
            let v = crate::verdict::assess(&is, &oos);
            (Some(is), Some(oos), v)
        }
        (None, Ok((is, oos))) => {
            let v = crate::verdict::assess(&is, &oos);
            (None, None, v)
        }
        (_, Err(_)) => (
            None,
            None,
            crate::verdict::insufficient_split(candles.len(), verdict_split),
        ),
    };
    Ok(BacktestParts {
        candle_count: candles.len(),
        gaps: crate::feeds::find_gaps(&candles, req.step),
        stats,
        metrics: crate::stats::compute_v2(&result, &candles, req.bars_per_year),
        in_sample,
        out_of_sample,
        verdict,
        trades: result.trades,
        equity_curve: result.equity_curve,
    })
}

async fn post_backtest(
    State(state): State<AppState>,
    Extension(limits): Extension<Arc<Limits>>,
    ClientIp(ip): ClientIp,
    body: Result<Json<BacktestBody>, JsonRejection>,
) -> Response {
    if let Err(retry_after) = limits.backtest.check(&ip) {
        return rate_limited(retry_after, "too many backtest requests; try again shortly");
    }
    let req = match body {
        Ok(Json(b)) => b,
        Err(e) => return err_json(e.status(), e.to_string()).into_response(),
    };
    if let Err(e) = req.strategy.validate() {
        return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response();
    }
    let symbol = req.strategy.asset.clone();
    let interval = req.strategy.timeframe.clone();
    if let Err(e) = crate::feeds::validate_symbol(&symbol) {
        return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response();
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
        return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response();
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
    let sim = SimRequest {
        symbol,
        interval,
        step,
        start: req.start_ms,
        end: req.end_ms,
        split: req.split,
        fee_bps,
        slippage_bps,
        bars_per_year,
    };
    let source = state.source.clone();
    let cache_dir = state.cache_dir.clone();
    let strategy = req.strategy.clone();
    let computed = tokio::time::timeout(
        BACKTEST_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            run_backtest(&cache_dir, source.as_ref(), &strategy, &sim)
        }),
    )
    .await;
    let parts = match computed {
        Err(_) => {
            return err_json(
                StatusCode::REQUEST_TIMEOUT,
                format!("backtest timed out after {BACKTEST_TIMEOUT_SECS}s"),
            )
            .into_response();
        }
        Ok(Err(join)) => {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, join.to_string()).into_response();
        }
        Ok(Ok(Err(failure))) => return failure.into_error_response(),
        Ok(Ok(Ok(parts))) => parts,
    };
    let stats_v = serde_json::to_value(&parts.stats).unwrap_or_default();
    let metrics_v = serde_json::to_value(&parts.metrics).unwrap_or_default();
    let equity_v = serde_json::to_value(&parts.equity_curve).unwrap_or_default();
    let trades_v = serde_json::to_value(&parts.trades).unwrap_or_default();
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
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, format!("save run: {e}"))
                .into_response()
        }
    };
    let resp = BacktestResponse {
        run_id,
        candle_count: parts.candle_count,
        gaps: parts.gaps,
        stats: parts.stats,
        metrics: parts.metrics,
        in_sample: parts.in_sample,
        out_of_sample: parts.out_of_sample,
        verdict: parts.verdict,
        trades: parts.trades,
        equity_curve: parts.equity_curve,
    };
    (
        StatusCode::OK,
        Json(serde_json::to_value(resp).unwrap_or_default()),
    )
        .into_response()
}

async fn post_optimize(
    State(state): State<AppState>,
    Extension(limits): Extension<Arc<Limits>>,
    ClientIp(ip): ClientIp,
    body: Result<Json<crate::optimizer::OptimizeSpec>, JsonRejection>,
) -> Response {
    if let Err(retry_after) = limits.optimize.check(&ip) {
        return rate_limited(retry_after, "too many optimize requests; try again shortly");
    }
    let req = match body {
        Ok(Json(b)) => b,
        Err(e) => return err_json(e.status(), e.to_string()).into_response(),
    };
    if let Err(e) = req.strategy.validate() {
        return err_json(
            StatusCode::BAD_REQUEST,
            format!("invalid base strategy: {e}"),
        )
        .into_response();
    }
    let combos_len = match crate::optimizer::build_combos(&req) {
        Ok(c) => c.len(),
        Err(e) => return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    let symbol = req.strategy.asset.clone();
    let interval = req.strategy.timeframe.clone();
    if let Err(e) = crate::feeds::validate_symbol(&symbol) {
        return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response();
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
        return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response();
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
        Ok(Err(e)) => return err_json(StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
        Ok(Ok(c)) => c,
    };
    if candles.is_empty() {
        return err_json(StatusCode::BAD_REQUEST, "no candles in range".to_string())
            .into_response();
    }

    let objective = req.objective.unwrap_or_default().as_str().to_string();
    let job_id = format!("opt-{}", state.jobs.next_id.fetch_add(1, Ordering::SeqCst));
    let job = Job {
        job_id: job_id.clone(),
        status: JobStatus::Running,
        total_trials: combos_len,
        completed_trials: 0,
        objective,
        validation: String::new(),
        results: None,
        warnings: Vec::new(),
        error: None,
    };
    if !state
        .jobs
        .try_insert_running(limits.max_optimizer_jobs, job)
    {
        return err_json(
            StatusCode::TOO_MANY_REQUESTS,
            "too many optimizer jobs running",
        )
        .into_response();
    }

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
                job.error = Some(e.to_string());
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
        Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
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
        Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
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
        ) -> Result<Vec<crate::types::Candle>, crate::feeds::FeedError> {
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
        ) -> Result<Vec<crate::types::Candle>, crate::feeds::FeedError> {
            Err(crate::feeds::FeedError::Request("boom".to_string()))
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

    #[tokio::test]
    async fn backtest_rate_limit_blocks_over_limit_with_retry_after() {
        let dir = temp_dir("bt_rate_limit");
        let fake = FakeSource::new();
        let router = app_with_limits(
            test_state(dir.clone()),
            DEFAULT_ORIGIN,
            Limits::new(1, 5, 2),
        )
        .expect("router");
        let body = serde_json::json!({
            "strategy": valid_strategy(),
            "start_ms": fake.start(),
            "end_ms": fake.end(),
        });
        let request = || {
            Request::post("/backtest")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let res = router.clone().oneshot(request()).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let res = router.oneshot(request()).await.unwrap();
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry_after = res
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .expect("retry-after header");
        assert!(
            (1..=60).contains(&retry_after),
            "retry-after: {retry_after}"
        );
        let v = body_json(res).await;
        assert!(v["error"].as_str().is_some_and(|s| !s.is_empty()), "{v}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn backtest_body_over_1mb_is_413() {
        let dir = temp_dir("bt_body_limit");
        let body = vec![b'a'; BODY_LIMIT_BYTES + 1];
        let res = app(test_state(dir.clone()))
            .oneshot(
                Request::post("/backtest")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn job_store_admits_until_cap_then_frees_slots() {
        fn job(id: &str) -> Job {
            Job {
                job_id: id.to_string(),
                status: JobStatus::Running,
                total_trials: 1,
                completed_trials: 0,
                objective: "score".to_string(),
                validation: String::new(),
                results: None,
                warnings: Vec::new(),
                error: None,
            }
        }
        let store = JobStore::default();
        assert!(store.try_insert_running(2, job("a")));
        assert!(store.try_insert_running(2, job("b")));
        assert!(!store.try_insert_running(2, job("c")));
        store.update("a", |j| j.status = JobStatus::Failed);
        assert!(store.try_insert_running(2, job("c")));
        assert!(!store.try_insert_running(2, job("d")));
    }

    #[tokio::test]
    async fn optimize_cap_rejects_when_no_job_slots() {
        let dir = temp_dir("opt_no_slots");
        let fake = FakeSource::new();
        let router = app_with_limits(
            test_state(dir.clone()),
            DEFAULT_ORIGIN,
            Limits::new(30, 5, 0),
        )
        .expect("router");
        let res = router
            .clone()
            .oneshot(
                Request::post("/optimize")
                    .header("content-type", "application/json")
                    .body(Body::from(optimize_body(&fake, 4).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
        let v = body_json(res).await;
        assert_eq!(v["error"], "too many optimizer jobs running");
        let res = router
            .oneshot(Request::get("/optimize").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let v = body_json(res).await;
        assert_eq!(v["jobs"].as_array().expect("jobs").len(), 0);
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

    struct WalkSource {
        data: Vec<crate::types::Candle>,
    }

    impl crate::feeds::KlineSource for WalkSource {
        fn fetch(
            &self,
            _symbol: &str,
            _interval: &str,
            start_ms: i64,
            end_ms: i64,
            limit: usize,
        ) -> Result<Vec<crate::types::Candle>, crate::feeds::FeedError> {
            Ok(self
                .data
                .iter()
                .filter(|c| c.ts >= start_ms && c.ts <= end_ms)
                .take(limit)
                .cloned()
                .collect())
        }
    }

    fn seeded_walk(n: usize, seed: u64) -> Vec<crate::types::Candle> {
        let mut state = seed;
        let mut next01 = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) as f64) / ((1u64 << 31) as f64)
        };
        let start: i64 = 1_700_006_400_000;
        let step: i64 = 3_600_000;
        let mut out = Vec::with_capacity(n);
        let mut prev = 100.0;
        for i in 0..n {
            let drift = (next01() - 0.5) * 0.02;
            let close = prev * (1.0 + drift);
            let open = prev;
            let high = open.max(close) + next01() * 0.5;
            let low = open.min(close) - next01() * 0.5;
            out.push(crate::types::Candle {
                ts: start + i as i64 * step,
                open,
                high,
                low,
                close,
            });
            prev = close;
        }
        out
    }

    #[tokio::test]
    async fn backtest_v2_matches_v1_on_seeded_walk() {
        let dir = temp_dir("bt_v2_parity");
        let candles = seeded_walk(3000, 42);
        let start = candles.first().map(|c| c.ts).unwrap_or(0);
        let end = candles.last().map(|c| c.ts).unwrap_or(0);
        let state = AppState {
            source: Arc::new(WalkSource {
                data: candles.clone(),
            }),
            cache_dir: dir.clone(),
            jobs: Arc::new(JobStore::default()),
            db: Arc::new(crate::db::Db::open_in_memory().expect("mem db")),
        };
        let strategy = serde_json::json!({
            "asset": "BTCUSDT",
            "timeframe": "1h",
            "side": "long",
            "entry": {"all": [{"left": {"ind": "close"}, "op": "crosses_above", "right": {"ind": "sma", "period": 10}}]},
            "exit": {"all": [{"left": {"ind": "close"}, "op": "crosses_below", "right": {"ind": "sma", "period": 10}}]},
            "stop_loss_pct": 2.0,
            "take_profit_pct": 4.0,
        });
        let body = serde_json::json!({
            "strategy": strategy,
            "start_ms": start,
            "end_ms": end,
        });
        let res = app(state)
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
        let http_trades = v["trades"].as_array().expect("trades array").clone();
        let expected: &[(u64, u64, &str, &str, f64)] = &[
            (11, 19, "long", "Signal", -0.401493474054),
            (22, 25, "long", "Signal", -0.715596149886),
            (30, 43, "long", "Signal", 1.88843928444),
            (57, 58, "long", "Signal", -1.06232875268),
            (60, 62, "long", "Signal", -0.596422628339),
            (72, 81, "long", "TakeProfit", 3.85450184871),
            (100, 103, "long", "Signal", -1.10159430335),
            (111, 112, "long", "Signal", -0.624996388311),
            (115, 132, "long", "Signal", 2.05385235154),
            (136, 137, "long", "Signal", -0.894993537614),
            (157, 166, "long", "Signal", 0.193611397899),
            (169, 171, "long", "Signal", -0.278895497595),
            (200, 203, "long", "Signal", -1.28746706694),
            (210, 212, "long", "Signal", -0.922409687215),
            (233, 234, "long", "Signal", -0.420704686326),
            (237, 239, "long", "Signal", -1.44601270581),
            (250, 262, "long", "Signal", 0.159021699174),
            (280, 290, "long", "Signal", 1.14275005542),
            (294, 295, "long", "Signal", -0.849477570347),
            (298, 299, "long", "Signal", -0.937764035768),
            (305, 309, "long", "Signal", -0.435180520864),
            (316, 328, "long", "Signal", 1.11643781473),
            (330, 335, "long", "Signal", -0.758668240302),
            (344, 346, "long", "Signal", -0.555094138935),
            (350, 353, "long", "Signal", -0.719543300924),
            (365, 367, "long", "Signal", -0.988517974601),
            (381, 383, "long", "Signal", -0.630664625468),
            (385, 386, "long", "Signal", -1.00960544666),
            (389, 393, "long", "Signal", -0.395705332889),
            (407, 408, "long", "Signal", -0.398128443874),
        ];
        assert_eq!(http_trades.len(), 226, "frozen trade count");
        assert!(http_trades.len() >= 20, "need >=20 trades");
        for (h, e) in http_trades.iter().take(expected.len()).zip(expected.iter()) {
            assert_eq!(h["entry_idx"].as_u64().unwrap(), e.0);
            assert_eq!(h["exit_idx"].as_u64().unwrap(), e.1);
            assert_eq!(h["side"].as_str().unwrap(), e.2);
            assert_eq!(h["reason"].as_str().unwrap(), e.3);
            let hr = h["return_pct"].as_f64().unwrap();
            assert!((hr - e.4).abs() <= 0.05, "return_pct {hr} vs {}", e.4);
        }
        let eq = v["equity_curve"].as_array().expect("equity array");
        assert_eq!(eq.len(), 3000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn backtest_verdict_on_seeded_walk() {
        let dir = temp_dir("bt_verdict_walk");
        let candles = seeded_walk(3000, 42);
        let start = candles.first().map(|c| c.ts).unwrap_or(0);
        let end = candles.last().map(|c| c.ts).unwrap_or(0);
        let state = AppState {
            source: Arc::new(WalkSource {
                data: candles.clone(),
            }),
            cache_dir: dir.clone(),
            jobs: Arc::new(JobStore::default()),
            db: Arc::new(crate::db::Db::open_in_memory().expect("mem db")),
        };
        let strategy = serde_json::json!({
            "asset": "BTCUSDT",
            "timeframe": "1h",
            "side": "long",
            "entry": {"all": [{"left": {"ind": "close"}, "op": "crosses_above", "right": {"ind": "sma", "period": 10}}]},
            "exit": {"all": [{"left": {"ind": "close"}, "op": "crosses_below", "right": {"ind": "sma", "period": 10}}]},
            "stop_loss_pct": 2.0,
            "take_profit_pct": 4.0,
        });
        for split in [None, Some(0.7)] {
            let mut body = serde_json::json!({
                "strategy": strategy,
                "start_ms": start,
                "end_ms": end,
            });
            if let Some(s) = split {
                body["split"] = serde_json::json!(s);
            }
            let router = app(AppState {
                source: state.source.clone(),
                cache_dir: dir.clone(),
                jobs: Arc::new(JobStore::default()),
                db: Arc::new(crate::db::Db::open_in_memory().expect("mem db")),
            });
            let res = router
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
            let grade = v["verdict"]["grade"].as_str().expect("verdict.grade");
            assert!(
                matches!(grade, "holds" | "fragile" | "fails" | "not_enough_data"),
                "invalid grade: {grade}"
            );
            assert!(v["verdict"]["headline"].is_string());
            assert!(v["verdict"]["reasons"].is_array());
            assert!(v["verdict"]["oos_trades"].is_number());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
