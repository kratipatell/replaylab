use axum::{
    extract::{
        rejection::JsonRejection, rejection::QueryRejection, ConnectInfo, DefaultBodyLimit,
        Extension, Path, Query, State,
    },
    http::{request::Parts, HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
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

const DEFAULT_BACKTEST_TIMEOUT_SECS: u64 = 60;
const BODY_LIMIT_BYTES: usize = 1024 * 1024;
const ADMIN_HEADER: &str = "x-admin-token";
const DEFAULT_DECISIONS_LIMIT: i64 = 100;
const MAX_DECISIONS_LIMIT: i64 = 500;
const DEFAULT_EQUITY_LIMIT: i64 = 1_000;
const MAX_EQUITY_LIMIT: i64 = 5_000;

#[derive(Clone)]
pub struct Limits {
    pub backtest: Arc<RateLimiter>,
    pub optimize: Arc<RateLimiter>,
    pub max_optimizer_jobs: usize,
    pub backtest_timeout: Duration,
}

impl Limits {
    pub fn new(backtest_per_min: u32, optimize_per_min: u32, max_optimizer_jobs: usize) -> Self {
        Self {
            backtest: Arc::new(RateLimiter::per_minute(backtest_per_min)),
            optimize: Arc::new(RateLimiter::per_minute(optimize_per_min)),
            max_optimizer_jobs,
            backtest_timeout: Duration::from_secs(DEFAULT_BACKTEST_TIMEOUT_SECS),
        }
    }

    pub fn with_backtest_timeout(mut self, secs: u64) -> Self {
        self.backtest_timeout = Duration::from_secs(secs);
        self
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
    /// When unset, admin endpoints refuse every request rather than allow all.
    pub admin_token: Option<String>,
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
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            HeaderName::from_static(ADMIN_HEADER),
        ]);
    Router::new()
        .route("/health", get(health))
        .route("/candles", get(get_candles))
        .route("/backtest", post(post_backtest))
        .route("/validate", post(post_validate))
        .route("/walk-forward", post(post_walk_forward))
        .route("/optimize", post(post_optimize).get(list_optimize))
        .route("/optimize/{job_id}", get(get_optimize))
        .route("/runs", get(list_runs))
        .route("/runs/{id}", get(get_run))
        .route("/agents", get(list_agents).post(post_agent))
        .route("/agents/{id}", get(get_agent))
        .route("/agents/{id}/decisions", get(get_agent_decisions))
        .route("/agents/{id}/equity", get(get_agent_equity))
        .route("/agents/{id}/start", post(start_agent))
        .route("/agents/{id}/stop", post(stop_agent))
        .route("/agents/{id}/advance", post(advance_agent))
        .route("/agents/{id}/reset", post(reset_agent))
        .route("/kill-switch", post(kill_switch))
        .route("/kill-switch/clear", post(clear_kill_switch))
        .with_state(state)
        .layer(DefaultBodyLimit::max(BODY_LIMIT_BYTES))
        .layer(Extension(Arc::new(limits)))
        .layer(cors)
}

fn err_json(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(serde_json::json!({"error": msg.into()})))
}

/// Failure carried by helpers that do not own the response. Keeping this small
/// avoids `Result<_, Response>` and the large error type that comes with it.
struct ApiFailure {
    status: StatusCode,
    message: String,
}

impl ApiFailure {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    fn into_response(self) -> Response {
        err_json(self.status, self.message).into_response()
    }
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

/// Compare without returning early on the first differing byte.
fn tokens_match(expected: &str, given: &str) -> bool {
    let (a, b) = (expected.as_bytes(), given.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Admin-only routes. Fail closed: no configured token means no access, not
/// open access.
fn admin_gate(state: &AppState, headers: &HeaderMap) -> Result<(), ApiFailure> {
    let Some(expected) = state.admin_token.as_deref() else {
        return Err(ApiFailure::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "admin actions are disabled; set ADMIN_TOKEN",
        ));
    };
    let given = headers
        .get(ADMIN_HEADER)
        .and_then(|value| value.to_str().ok());
    match given {
        Some(given) if tokens_match(expected, given) => Ok(()),
        _ => Err(ApiFailure::new(
            StatusCode::FORBIDDEN,
            "admin token required",
        )),
    }
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
    risk: Option<crate::risk::RiskLimits>,
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
    risk_halt: Option<crate::risk::HaltLimit>,
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
    risk: Option<crate::risk::RiskLimits>,
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
    risk_halt: Option<crate::risk::HaltLimit>,
}

enum BacktestFailure {
    Upstream(String),
    BadRequest(String),
}

impl BacktestFailure {
    fn reason(&self) -> &str {
        match self {
            BacktestFailure::Upstream(msg) | BacktestFailure::BadRequest(msg) => msg,
        }
    }

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

struct SimParams {
    interval: String,
    step: i64,
    start: i64,
    end: i64,
    split: Option<f64>,
    fee_bps: f64,
    slippage_bps: f64,
    bars_per_year: f64,
}

impl SimParams {
    fn request_for(&self, symbol: &str) -> SimRequest {
        SimRequest {
            symbol: symbol.to_string(),
            interval: self.interval.clone(),
            step: self.step,
            start: self.start,
            end: self.end,
            split: self.split,
            fee_bps: self.fee_bps,
            slippage_bps: self.slippage_bps,
            bars_per_year: self.bars_per_year,
            risk: None,
        }
    }
}

fn parse_sim_params(
    interval: &str,
    start_ms: i64,
    end_ms: i64,
    fee_bps: Option<f64>,
    slippage_bps: Option<f64>,
    split: Option<f64>,
) -> Result<SimParams, String> {
    let step = crate::feeds::interval_ms(interval)
        .ok_or_else(|| format!("unknown interval: {interval}"))?;
    if start_ms >= end_ms {
        return Err("start_ms must be less than end_ms".to_string());
    }
    check_range_limit(start_ms, end_ms, step).map_err(|e| e.to_string())?;
    let defaults = crate::backtest::BacktestConfig::default();
    let fee_bps = fee_bps.unwrap_or(defaults.fee_bps);
    let slippage_bps = slippage_bps.unwrap_or(defaults.slippage_bps);
    if !(0.0..=1000.0).contains(&fee_bps) || !fee_bps.is_finite() {
        return Err(format!("fee_bps must be between 0 and 1000, got {fee_bps}"));
    }
    if !(0.0..=1000.0).contains(&slippage_bps) || !slippage_bps.is_finite() {
        return Err(format!(
            "slippage_bps must be between 0 and 1000, got {slippage_bps}"
        ));
    }
    let bars_per_year = crate::stats::bars_per_year(interval)
        .ok_or_else(|| format!("unknown timeframe: {interval}"))?;
    if let Some(split) = split {
        if !(split > 0.0 && split < 1.0) || !split.is_finite() {
            return Err(format!(
                "split must be strictly between 0 and 1, got {split}"
            ));
        }
    }
    Ok(SimParams {
        interval: interval.to_string(),
        step,
        start: start_ms,
        end: end_ms,
        split,
        fee_bps,
        slippage_bps,
        bars_per_year,
    })
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
    let outcome = crate::backtest::backtest_strategy_with_risk(strategy, &candles, &cfg, req.risk)
        .map_err(|e| BacktestFailure::BadRequest(e.to_string()))?;
    let result = outcome.result;
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
        risk_halt: outcome.halt,
    })
}

fn run_validation(
    cache_dir: &std::path::Path,
    source: &dyn crate::feeds::KlineSource,
    strategy: &crate::strategy::Strategy,
    params: &SimParams,
    symbols: &[String],
) -> Result<crate::validation::CrossMarket, BacktestFailure> {
    let mut markets = Vec::with_capacity(symbols.len());
    let mut unchecked = Vec::new();
    let mut first_failure: Option<BacktestFailure> = None;

    for symbol in symbols {
        let mut per_market = strategy.clone();
        per_market.asset = symbol.clone();
        let req = params.request_for(symbol);
        match run_backtest(cache_dir, source, &per_market, &req) {
            Ok(parts) => markets.push(crate::validation::MarketCheck {
                symbol: symbol.clone(),
                candle_count: parts.candle_count,
                stats: parts.stats,
                in_sample: parts.in_sample,
                out_of_sample: parts.out_of_sample,
                verdict: parts.verdict,
            }),
            Err(failure) => {
                unchecked.push(crate::validation::UncheckedMarket {
                    symbol: symbol.clone(),
                    reason: failure.reason().to_string(),
                });
                if first_failure.is_none() {
                    first_failure = Some(failure);
                }
            }
        }
    }

    if markets.is_empty() {
        return Err(first_failure.unwrap_or_else(|| {
            BacktestFailure::BadRequest("no markets were checked".to_string())
        }));
    }
    Ok(crate::validation::assess(markets, unchecked))
}

fn validation_symbols(
    primary: &str,
    requested: Option<Vec<String>>,
) -> Result<Vec<String>, String> {
    let requested = match requested {
        Some(list) if list.is_empty() => {
            return Err("symbols must name at least one other market".to_string())
        }
        Some(list) => list,
        None => crate::validation::DEFAULT_SYMBOLS
            .iter()
            .map(|s| s.to_string())
            .collect(),
    };
    let mut symbols = vec![primary.to_string()];
    for symbol in requested {
        crate::feeds::validate_symbol(&symbol).map_err(|e| e.to_string())?;
        if !symbols.contains(&symbol) {
            symbols.push(symbol);
        }
    }
    if symbols.len() < 2 {
        return Err("at least two markets are needed to validate elsewhere".to_string());
    }
    if symbols.len() > crate::validation::MAX_SYMBOLS {
        return Err(format!(
            "at most {} markets per validation, got {}",
            crate::validation::MAX_SYMBOLS,
            symbols.len()
        ));
    }
    Ok(symbols)
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
    if let Some(limits) = &req.risk {
        if let Err(e) = limits.validate() {
            return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response();
        }
    }
    let symbol = req.strategy.asset.clone();
    if let Err(e) = crate::feeds::validate_symbol(&symbol) {
        return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response();
    }
    let params = match parse_sim_params(
        &req.strategy.timeframe,
        req.start_ms,
        req.end_ms,
        req.fee_bps,
        req.slippage_bps,
        req.split,
    ) {
        Ok(p) => p,
        Err(msg) => return err_json(StatusCode::BAD_REQUEST, msg).into_response(),
    };
    let mut sim = params.request_for(&symbol);
    sim.risk = req.risk;
    let source = state.source.clone();
    let cache_dir = state.cache_dir.clone();
    let strategy = req.strategy.clone();
    let timeout = limits.backtest_timeout;
    let computed = tokio::time::timeout(
        timeout,
        tokio::task::spawn_blocking(move || {
            run_backtest(&cache_dir, source.as_ref(), &strategy, &sim)
        }),
    )
    .await;
    let parts = match computed {
        Err(_) => {
            return err_json(
                StatusCode::REQUEST_TIMEOUT,
                format!("backtest timed out after {}s", timeout.as_secs()),
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
        risk_halt: parts.risk_halt,
    };
    (
        StatusCode::OK,
        Json(serde_json::to_value(resp).unwrap_or_default()),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidateBody {
    strategy: crate::strategy::Strategy,
    start_ms: i64,
    end_ms: i64,
    fee_bps: Option<f64>,
    slippage_bps: Option<f64>,
    split: Option<f64>,
    symbols: Option<Vec<String>>,
}

// One backtest token is charged per market, since each market is a full run.
async fn post_validate(
    State(state): State<AppState>,
    Extension(limits): Extension<Arc<Limits>>,
    ClientIp(ip): ClientIp,
    body: Result<Json<ValidateBody>, JsonRejection>,
) -> Response {
    let req = match body {
        Ok(Json(b)) => b,
        Err(e) => return err_json(e.status(), e.to_string()).into_response(),
    };
    if let Err(e) = req.strategy.validate() {
        return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response();
    }
    let primary = req.strategy.asset.clone();
    if let Err(e) = crate::feeds::validate_symbol(&primary) {
        return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response();
    }
    let symbols = match validation_symbols(&primary, req.symbols) {
        Ok(s) => s,
        Err(msg) => return err_json(StatusCode::BAD_REQUEST, msg).into_response(),
    };
    if let Err(retry_after) = limits.backtest.check_n(&ip, symbols.len() as u32) {
        return rate_limited(
            retry_after,
            "too many validation requests; try again shortly",
        );
    }
    let params = match parse_sim_params(
        &req.strategy.timeframe,
        req.start_ms,
        req.end_ms,
        req.fee_bps,
        req.slippage_bps,
        req.split,
    ) {
        Ok(p) => p,
        Err(msg) => return err_json(StatusCode::BAD_REQUEST, msg).into_response(),
    };

    let source = state.source.clone();
    let cache_dir = state.cache_dir.clone();
    let strategy = req.strategy;
    let timeout = limits.backtest_timeout;
    let computed = tokio::time::timeout(
        timeout,
        tokio::task::spawn_blocking(move || {
            run_validation(&cache_dir, source.as_ref(), &strategy, &params, &symbols)
        }),
    )
    .await;
    let cross = match computed {
        Err(_) => {
            return err_json(
                StatusCode::REQUEST_TIMEOUT,
                format!("validation timed out after {}s", timeout.as_secs()),
            )
            .into_response();
        }
        Ok(Err(join)) => {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, join.to_string()).into_response();
        }
        Ok(Ok(Err(failure))) => return failure.into_error_response(),
        Ok(Ok(Ok(cross))) => cross,
    };
    (StatusCode::OK, Json(cross)).into_response()
}

async fn post_walk_forward(
    State(state): State<AppState>,
    Extension(limits): Extension<Arc<Limits>>,
    ClientIp(ip): ClientIp,
    body: Result<Json<crate::walk_forward::WalkForwardRequest>, JsonRejection>,
) -> Response {
    if let Err(retry_after) = limits.optimize.check(&ip) {
        return rate_limited(
            retry_after,
            "too many optimizer requests; try again shortly",
        );
    }
    let req = match body {
        Ok(Json(body)) => body,
        Err(error) => return err_json(error.status(), error.to_string()).into_response(),
    };
    if let Err(error) = req.strategy.validate() {
        return err_json(StatusCode::BAD_REQUEST, error.to_string()).into_response();
    }
    if let Err(error) = crate::feeds::validate_symbol(&req.strategy.asset) {
        return err_json(StatusCode::BAD_REQUEST, error.to_string()).into_response();
    }
    let sim = match parse_sim_params(
        &req.strategy.timeframe,
        req.start_ms,
        req.end_ms,
        req.fee_bps,
        req.slippage_bps,
        None,
    ) {
        Ok(params) => params,
        Err(message) => return err_json(StatusCode::BAD_REQUEST, message).into_response(),
    };
    if let Err(error) = crate::walk_forward::validate_request(&req) {
        return err_json(StatusCode::BAD_REQUEST, error.to_string()).into_response();
    }

    let source = state.source.clone();
    let cache_dir = state.cache_dir.clone();
    let symbol = req.strategy.asset.clone();
    let interval = req.strategy.timeframe.clone();
    let start = req.start_ms;
    let end = req.end_ms;
    let cfg = crate::backtest::BacktestConfig {
        fee_bps: sim.fee_bps,
        slippage_bps: sim.slippage_bps,
    };
    let bars_per_year = sim.bars_per_year;
    let timeout = limits.backtest_timeout;
    let computed = tokio::time::timeout(
        timeout,
        tokio::task::spawn_blocking(move || {
            let candles = crate::feeds::load_or_fetch(
                &cache_dir,
                source.as_ref(),
                &symbol,
                &interval,
                start,
                end,
            )
            .map_err(|error| BacktestFailure::Upstream(error.to_string()))?;
            if candles.is_empty() {
                return Err(BacktestFailure::BadRequest(
                    "no candles in range".to_string(),
                ));
            }
            crate::walk_forward::run(&req, &candles, &cfg, bars_per_year)
                .map_err(|error| BacktestFailure::BadRequest(error.to_string()))
        }),
    )
    .await;
    let report = match computed {
        Err(_) => {
            return err_json(
                StatusCode::REQUEST_TIMEOUT,
                format!("walk-forward timed out after {}s", timeout.as_secs()),
            )
            .into_response();
        }
        Ok(Err(error)) => {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response()
        }
        Ok(Ok(Err(failure))) => return failure.into_error_response(),
        Ok(Ok(Ok(report))) => report,
    };
    (StatusCode::OK, Json(report)).into_response()
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

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn now_secs() -> i64 {
    now_ms() / 1000
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateAgentBody {
    name: String,
    strategy: Option<crate::strategy::Strategy>,
    run_id: Option<i64>,
    initial_equity: Option<f64>,
    risk: Option<crate::risk::RiskLimits>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct StartAgentBody {
    start_ms: Option<i64>,
    end_ms: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AdvanceAgentBody {
    end_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
struct AgentDetail {
    agent: crate::db::AgentSummary,
    strategy: crate::strategy::Strategy,
    initial_equity: f64,
    risk: crate::risk::RiskLimits,
    halt: Option<crate::risk::HaltLimit>,
    candle_count: usize,
    position: Option<crate::backtest::PositionView>,
    trades: Vec<crate::backtest::Trade>,
    equity_curve: Vec<f64>,
}

fn agent_detail(state: &AppState, row: crate::db::AgentRow) -> Result<AgentDetail, ApiFailure> {
    let agent_id = row.summary.id.clone();
    let strategy: crate::strategy::Strategy = serde_json::from_value(row.strategy)
        .map_err(|e| ApiFailure::internal(format!("agent strategy is corrupt: {e}")))?;
    let (candle_count, position, trades, snapshot_equity_curve, halt) =
        match row.snapshot.as_deref() {
            Some(text) => {
                let snapshot = crate::agent::AgentSnapshot::from_json(text)
                    .map_err(|e| ApiFailure::internal(e.to_string()))?;
                (
                    snapshot.candle_count(),
                    snapshot.position(),
                    snapshot.trades().to_vec(),
                    snapshot.equity_curve().to_vec(),
                    snapshot.halt(),
                )
            }
            None => (0, None, Vec::new(), Vec::new(), None),
        };
    let history = state
        .db
        .list_agent_equity(&agent_id, crate::db::MAX_AGENT_EQUITY_POINTS, None)
        .map_err(|e| ApiFailure::internal(e.to_string()))?;
    let equity_curve = if history.is_empty() {
        snapshot_equity_curve
    } else {
        history.into_iter().map(|point| point.equity).collect()
    };
    Ok(AgentDetail {
        agent: row.summary,
        strategy,
        initial_equity: row.initial_equity,
        risk: row.risk,
        halt,
        candle_count,
        position,
        trades,
        equity_curve,
    })
}

fn agent_json(detail: AgentDetail) -> Response {
    (
        StatusCode::OK,
        Json(serde_json::to_value(detail).unwrap_or_default()),
    )
        .into_response()
}

fn agent_or_404(state: &AppState, id: &str) -> Result<crate::db::AgentRow, ApiFailure> {
    match state.db.get_agent(id) {
        Ok(Some(row)) => Ok(row),
        Ok(None) => Err(ApiFailure::not_found(format!("unknown agent: {id}"))),
        Err(e) => Err(ApiFailure::internal(e.to_string())),
    }
}

fn agent_status(row: &crate::db::AgentRow) -> Result<crate::agent::AgentStatus, ApiFailure> {
    crate::agent::AgentStatus::parse(&row.summary.status).ok_or_else(|| {
        ApiFailure::internal(format!("agent {} has an unknown status", row.summary.id))
    })
}

fn agent_spec(row: &crate::db::AgentRow) -> Result<crate::strategy::Strategy, ApiFailure> {
    serde_json::from_value(row.strategy.clone())
        .map_err(|e| ApiFailure::internal(format!("agent strategy is corrupt: {e}")))
}

async fn load_agent_candles(
    state: &AppState,
    symbol: &str,
    interval: &str,
    start: i64,
    end: i64,
    timeout: Duration,
) -> Result<Vec<crate::types::Candle>, ApiFailure> {
    let source = state.source.clone();
    let cache_dir = state.cache_dir.clone();
    let symbol = symbol.to_string();
    let interval = interval.to_string();
    let loaded = tokio::time::timeout(
        timeout,
        tokio::task::spawn_blocking(move || {
            crate::feeds::load_or_fetch(&cache_dir, source.as_ref(), &symbol, &interval, start, end)
        }),
    )
    .await;
    match loaded {
        Err(_) => Err(ApiFailure::new(
            StatusCode::REQUEST_TIMEOUT,
            format!("agent request timed out after {}s", timeout.as_secs()),
        )),
        Ok(Err(join)) => Err(ApiFailure::internal(join.to_string())),
        Ok(Ok(Err(e))) => Err(ApiFailure::new(StatusCode::BAD_GATEWAY, e.to_string())),
        Ok(Ok(Ok(candles))) => Ok(candles),
    }
}

async fn list_agents(State(state): State<AppState>) -> Response {
    match state.db.list_agents() {
        Ok(agents) => (
            StatusCode::OK,
            Json(serde_json::json!({ "agents": agents })),
        )
            .into_response(),
        Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_agent(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match agent_or_404(&state, &id) {
        Ok(row) => match agent_detail(&state, row) {
            Ok(detail) => agent_json(detail),
            Err(e) => e.into_response(),
        },
        Err(e) => e.into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct AgentDecisionsQuery {
    limit: Option<i64>,
    before: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AgentEquityQuery {
    limit: Option<i64>,
    since: Option<i64>,
}

fn parse_decision_cursor(value: &str) -> Result<(i64, i64), String> {
    let Some((bar_ts, seq)) = value.split_once(':') else {
        return Err("before must be formatted as bar_ts:seq".to_string());
    };
    let bar_ts = bar_ts
        .parse::<i64>()
        .map_err(|_| "before bar_ts must be a non-negative integer".to_string())?;
    let seq = seq
        .parse::<i64>()
        .map_err(|_| "before seq must be a non-negative integer".to_string())?;
    if bar_ts < 0 || seq < 0 {
        return Err("before values must be non-negative".to_string());
    }
    Ok((bar_ts, seq))
}

async fn get_agent_decisions(
    State(state): State<AppState>,
    Path(id): Path<String>,
    query: Result<Query<AgentDecisionsQuery>, QueryRejection>,
) -> Response {
    let query = match query {
        Ok(query) => query.0,
        Err(error) => return err_json(StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };
    let limit = query.limit.unwrap_or(DEFAULT_DECISIONS_LIMIT);
    if !(1..=MAX_DECISIONS_LIMIT).contains(&limit) {
        return err_json(
            StatusCode::BAD_REQUEST,
            format!("limit must be between 1 and {MAX_DECISIONS_LIMIT}"),
        )
        .into_response();
    }
    let before = match query
        .before
        .as_deref()
        .map(parse_decision_cursor)
        .transpose()
    {
        Ok(before) => before,
        Err(error) => return err_json(StatusCode::BAD_REQUEST, error).into_response(),
    };
    if let Err(error) = agent_or_404(&state, &id) {
        return error.into_response();
    }
    match state.db.list_agent_decisions(&id, limit, before) {
        Ok(decisions) => {
            let next_before = (decisions.len() == limit as usize)
                .then(|| decisions.last())
                .flatten()
                .map(|row| format!("{}:{}", row.bar_ts, row.seq));
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "decisions": decisions,
                    "next_before": next_before,
                })),
            )
                .into_response()
        }
        Err(error) => {
            err_json(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response()
        }
    }
}

async fn get_agent_equity(
    State(state): State<AppState>,
    Path(id): Path<String>,
    query: Result<Query<AgentEquityQuery>, QueryRejection>,
) -> Response {
    let query = match query {
        Ok(query) => query.0,
        Err(error) => return err_json(StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };
    let limit = query.limit.unwrap_or(DEFAULT_EQUITY_LIMIT);
    if !(1..=MAX_EQUITY_LIMIT).contains(&limit) {
        return err_json(
            StatusCode::BAD_REQUEST,
            format!("limit must be between 1 and {MAX_EQUITY_LIMIT}"),
        )
        .into_response();
    }
    if query.since.is_some_and(|since| since < 0) {
        return err_json(StatusCode::BAD_REQUEST, "since must be non-negative").into_response();
    }
    if let Err(error) = agent_or_404(&state, &id) {
        return error.into_response();
    }
    match state.db.list_agent_equity(&id, limit, query.since) {
        Ok(points) => {
            let next_since = (points.len() == limit as usize)
                .then(|| points.last())
                .flatten()
                .and_then(|point| point.bar_ts.checked_add(1));
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "equity": points,
                    "next_since": next_since,
                })),
            )
                .into_response()
        }
        Err(error) => {
            err_json(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response()
        }
    }
}

async fn post_agent(
    State(state): State<AppState>,
    body: Result<Json<CreateAgentBody>, JsonRejection>,
) -> Response {
    let body = match body {
        Ok(Json(body)) => body,
        Err(e) => return err_json(e.status(), e.to_string()).into_response(),
    };
    let (spec, run_id) = match (body.strategy, body.run_id) {
        (Some(spec), None) => (spec, None),
        (None, Some(run_id)) => match state.db.get_run(run_id) {
            Ok(Some(run)) => match serde_json::from_value(run.strategy) {
                Ok(spec) => (spec, Some(run_id)),
                Err(e) => {
                    return err_json(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("run {run_id} has a corrupt strategy: {e}"),
                    )
                    .into_response()
                }
            },
            Ok(None) => {
                return err_json(StatusCode::NOT_FOUND, format!("unknown run: {run_id}"))
                    .into_response()
            }
            Err(e) => {
                return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
            }
        },
        _ => {
            return err_json(
                StatusCode::BAD_REQUEST,
                "provide exactly one of strategy or run_id".to_string(),
            )
            .into_response()
        }
    };
    let equity = body
        .initial_equity
        .unwrap_or(crate::agent::DEFAULT_INITIAL_EQUITY);
    if let Err(e) = crate::agent::validate_new(&body.name, &spec, equity) {
        return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response();
    }
    // An agent always has caps. A backtest opts in; an agent never runs bare.
    let risk = body.risk.unwrap_or_default();
    if let Err(e) = risk.validate() {
        return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response();
    }
    let strategy = match serde_json::to_value(&spec) {
        Ok(v) => v,
        Err(e) => {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    };
    let id = crate::agent::new_agent_id();
    if let Err(e) = state
        .db
        .create_agent(&id, body.name.trim(), &strategy, run_id, equity, &risk)
    {
        return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    match agent_or_404(&state, &id).and_then(|row| agent_detail(&state, row)) {
        Ok(detail) => (
            StatusCode::CREATED,
            Json(serde_json::to_value(detail).unwrap_or_default()),
        )
            .into_response(),
        Err(e) => e.into_response(),
    }
}

async fn start_agent(
    State(state): State<AppState>,
    Extension(limits): Extension<Arc<Limits>>,
    Path(id): Path<String>,
    body: Result<Json<StartAgentBody>, JsonRejection>,
) -> Response {
    let body = match body {
        Ok(Json(body)) => body,
        Err(e) => return err_json(e.status(), e.to_string()).into_response(),
    };
    let row = match agent_or_404(&state, &id) {
        Ok(row) => row,
        Err(e) => return e.into_response(),
    };
    let status = match agent_status(&row) {
        Ok(status) => status,
        Err(e) => return e.into_response(),
    };
    if status == crate::agent::AgentStatus::Running {
        return err_json(
            StatusCode::CONFLICT,
            format!("agent {id} is already running"),
        )
        .into_response();
    }
    // The global flag covers agents created after the switch was flipped;
    // the column covers agents that already existed when it was flipped.
    let global_kill = match state.db.global_kill_switch_on() {
        Ok(on) => on,
        Err(e) => {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    };
    if global_kill || row.kill_switch {
        return err_json(
            StatusCode::CONFLICT,
            format!("agent {id} is halted by the kill switch; clear it first"),
        )
        .into_response();
    }
    // A stopped agent keeps its snapshot: start resumes it rather than
    // replaying history and rewriting decisions already taken.
    if let Some(text) = row.snapshot.as_deref() {
        let mut snapshot = match crate::agent::AgentSnapshot::from_json(text) {
            Ok(snapshot) => snapshot,
            Err(e) => {
                return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
            }
        };
        // Snapshots written before agents enforced risk get the caps now.
        if snapshot.risk_limits().is_none() {
            snapshot.set_risk_limits(row.risk);
        }
        let snapshot_json = match snapshot.to_json() {
            Ok(json) => json,
            Err(e) => {
                return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
            }
        };
        let equity_points = snapshot.equity_points();
        match state.db.start_agent_with_snapshot(
            &id,
            now_secs(),
            snapshot.equity(),
            snapshot.last_price(),
            snapshot.last_ts(),
            &snapshot_json,
            &equity_points,
        ) {
            Ok(true) => {}
            Ok(false) => {
                return err_json(StatusCode::NOT_FOUND, format!("unknown agent: {id}"))
                    .into_response()
            }
            Err(e) => {
                return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
            }
        }
        return match agent_or_404(&state, &id).and_then(|row| agent_detail(&state, row)) {
            Ok(detail) => agent_json(detail),
            Err(e) => e.into_response(),
        };
    }
    let spec = match agent_spec(&row) {
        Ok(spec) => spec,
        Err(e) => return e.into_response(),
    };
    let (start, end) = match (body.start_ms, body.end_ms) {
        (Some(start), Some(end)) => (start, end),
        _ => match row.summary.run_id {
            Some(run_id) => match state.db.get_run(run_id) {
                Ok(Some(run)) => (run.start_ms, run.end_ms),
                Ok(None) => {
                    return err_json(StatusCode::NOT_FOUND, format!("unknown run: {run_id}"))
                        .into_response()
                }
                Err(e) => {
                    return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                        .into_response()
                }
            },
            None => {
                return err_json(
                    StatusCode::BAD_REQUEST,
                    "start_ms and end_ms are required for an agent not created from a run"
                        .to_string(),
                )
                .into_response()
            }
        },
    };
    let step = match crate::feeds::interval_ms(&spec.timeframe) {
        Some(step) => step,
        None => {
            return err_json(
                StatusCode::BAD_REQUEST,
                format!("unknown interval: {}", spec.timeframe),
            )
            .into_response()
        }
    };
    if start >= end {
        return err_json(
            StatusCode::BAD_REQUEST,
            "start_ms must be less than end_ms".to_string(),
        )
        .into_response();
    }
    if let Err(e) = check_range_limit(start, end, step) {
        return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response();
    }
    let candles = match load_agent_candles(
        &state,
        &spec.asset,
        &spec.timeframe,
        start,
        end,
        limits.backtest_timeout,
    )
    .await
    {
        Ok(candles) => candles,
        Err(e) => {
            let _ = state
                .db
                .record_agent_error(&id, "could not load warm-up candles");
            return e.into_response();
        }
    };
    if candles.is_empty() {
        let _ = state
            .db
            .record_agent_error(&id, "no closed candles in the warm-up range");
        return err_json(
            StatusCode::BAD_REQUEST,
            "no closed candles in the warm-up range".to_string(),
        )
        .into_response();
    }
    let snapshot = match crate::agent::AgentSnapshot::warm_up(
        &spec,
        &candles,
        &crate::backtest::BacktestConfig::default(),
        row.initial_equity,
        row.risk,
    ) {
        Ok(snapshot) => snapshot,
        Err(e) => {
            let _ = state.db.record_agent_error(&id, &e.to_string());
            return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response();
        }
    };
    let snapshot_json = match snapshot.to_json() {
        Ok(text) => text,
        Err(e) => {
            let _ = state.db.record_agent_error(&id, &e.to_string());
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };
    let equity_points = snapshot.equity_points();
    if let Err(e) = state.db.start_agent_with_snapshot(
        &id,
        now_secs(),
        snapshot.equity(),
        snapshot.last_price(),
        snapshot.last_ts(),
        &snapshot_json,
        &equity_points,
    ) {
        return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    match agent_or_404(&state, &id).and_then(|row| agent_detail(&state, row)) {
        Ok(detail) => agent_json(detail),
        Err(e) => e.into_response(),
    }
}

async fn stop_agent(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let row = match agent_or_404(&state, &id) {
        Ok(row) => row,
        Err(e) => return e.into_response(),
    };
    let status = match agent_status(&row) {
        Ok(status) => status,
        Err(e) => return e.into_response(),
    };
    if status == crate::agent::AgentStatus::Draft {
        return err_json(StatusCode::CONFLICT, format!("agent {id} has not started"))
            .into_response();
    }
    if status == crate::agent::AgentStatus::Running {
        if let Err(e) = state.db.stop_agent(&id, now_secs()) {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }
    match agent_or_404(&state, &id).and_then(|row| agent_detail(&state, row)) {
        Ok(detail) => agent_json(detail),
        Err(e) => e.into_response(),
    }
}

async fn advance_agent(
    State(state): State<AppState>,
    Extension(limits): Extension<Arc<Limits>>,
    Path(id): Path<String>,
    body: Result<Json<AdvanceAgentBody>, JsonRejection>,
) -> Response {
    let body = match body {
        Ok(Json(body)) => body,
        Err(e) => return err_json(e.status(), e.to_string()).into_response(),
    };
    let row = match agent_or_404(&state, &id) {
        Ok(row) => row,
        Err(e) => return e.into_response(),
    };
    let status = match agent_status(&row) {
        Ok(status) => status,
        Err(e) => return e.into_response(),
    };
    if status != crate::agent::AgentStatus::Running {
        return err_json(
            StatusCode::CONFLICT,
            format!("agent {id} is {}; start it first", status.as_str()),
        )
        .into_response();
    }
    let spec = match agent_spec(&row) {
        Ok(spec) => spec,
        Err(e) => return e.into_response(),
    };
    let Some(text) = row.snapshot.as_deref() else {
        return err_json(
            StatusCode::CONFLICT,
            format!("agent {id} has no snapshot; start it first"),
        )
        .into_response();
    };
    let mut snapshot = match crate::agent::AgentSnapshot::from_json(text) {
        Ok(snapshot) => snapshot,
        Err(e) => {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    };
    // Keep the stored snapshot in step with the source of truth: caps added
    // after this snapshot was written, and a kill switch flipped while the
    // agent was stopped.
    let mut reconciled = false;
    if snapshot.risk_limits().is_none() {
        snapshot.set_risk_limits(row.risk);
        reconciled = true;
    }
    if row.kill_switch && snapshot.halt() != Some(crate::risk::HaltLimit::KillSwitch) {
        snapshot.kill_switch();
        reconciled = true;
    }
    let history_exists = match state.db.list_agent_equity(&id, 1, None) {
        Ok(points) => !points.is_empty(),
        Err(e) => {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    };
    let prior_equity_points = if history_exists {
        Vec::new()
    } else {
        snapshot.equity_points()
    };
    let step = match crate::feeds::interval_ms(&spec.timeframe) {
        Some(step) => step,
        None => {
            return err_json(
                StatusCode::BAD_REQUEST,
                format!("unknown interval: {}", spec.timeframe),
            )
            .into_response()
        }
    };
    let start = snapshot.last_ts() + step;
    let end = body.end_ms.unwrap_or_else(now_ms);
    if start >= end {
        if reconciled {
            let json = match snapshot.to_json() {
                Ok(json) => json,
                Err(e) => {
                    return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                        .into_response()
                }
            };
            if let Err(e) =
                state
                    .db
                    .set_agent_snapshot_with_equity(&id, &json, &prior_equity_points)
            {
                return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
            }
        }
        return match agent_or_404(&state, &id).and_then(|row| agent_detail(&state, row)) {
            Ok(detail) => agent_json(detail),
            Err(e) => e.into_response(),
        };
    }
    let candles = match load_agent_candles(
        &state,
        &spec.asset,
        &spec.timeframe,
        start,
        end,
        limits.backtest_timeout,
    )
    .await
    {
        Ok(candles) => candles,
        Err(e) => {
            let _ = state
                .db
                .record_agent_error(&id, "could not load new candles");
            return e.into_response();
        }
    };
    let outcome = match snapshot.advance(&candles) {
        Ok(outcome) => outcome,
        Err(e) => {
            let _ = state.db.record_agent_error(&id, &e.to_string());
            return err_json(StatusCode::BAD_REQUEST, e.to_string()).into_response();
        }
    };
    if outcome.new_candles > 0 {
        let snapshot_json = match snapshot.to_json() {
            Ok(text) => text,
            Err(e) => {
                let _ = state.db.record_agent_error(&id, &e.to_string());
                return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
            }
        };
        let mut equity_points = prior_equity_points;
        equity_points.extend(outcome.equity_points.iter().cloned());
        if let Err(e) = state.db.save_agent_advance(crate::db::AgentAdvanceWrite {
            id: &id,
            status: status.as_str(),
            equity: outcome.equity,
            last_price: outcome.last_price,
            last_ts: outcome.last_ts,
            snapshot: &snapshot_json,
            decisions: &outcome.events,
            equity_points: &equity_points,
        }) {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    } else if reconciled {
        let json = match snapshot.to_json() {
            Ok(json) => json,
            Err(e) => {
                return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
            }
        };
        if let Err(e) = state
            .db
            .set_agent_snapshot_with_equity(&id, &json, &prior_equity_points)
        {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }
    match agent_or_404(&state, &id).and_then(|row| agent_detail(&state, row)) {
        Ok(detail) => agent_json(detail),
        Err(e) => e.into_response(),
    }
}

/// Apply one change to an agent's stored snapshot. `Ok(false)` means the agent
/// has no snapshot yet, so only its database flag applies to it.
fn update_snapshot(
    state: &AppState,
    id: &str,
    change: impl FnOnce(&mut crate::agent::AgentSnapshot),
) -> Result<bool, ApiFailure> {
    let row = agent_or_404(state, id)?;
    let Some(text) = row.snapshot.as_deref() else {
        return Ok(false);
    };
    let mut snapshot = crate::agent::AgentSnapshot::from_json(text)
        .map_err(|e| ApiFailure::internal(e.to_string()))?;
    change(&mut snapshot);
    let equity_points = snapshot.equity_points();
    let json = snapshot
        .to_json()
        .map_err(|e| ApiFailure::internal(e.to_string()))?;
    state
        .db
        .set_agent_snapshot_with_equity(id, &json, &equity_points)
        .map_err(|e| ApiFailure::internal(e.to_string()))?;
    Ok(true)
}

/// Operator stop for every agent. Open positions flatten at the next open:
/// a frozen position is risk the operator cannot see or close.
async fn kill_switch(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(e) = admin_gate(&state, &headers) {
        return e.into_response();
    }
    if let Err(e) = state.db.set_global_kill_switch(true) {
        return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    let agents = match state.db.list_agents() {
        Ok(agents) => agents,
        Err(e) => {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    };
    let mut snapshots = 0usize;
    for agent in &agents {
        if let Err(e) = state.db.set_kill_switch(&agent.id, true) {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
        match update_snapshot(&state, &agent.id, |snapshot| snapshot.kill_switch()) {
            Ok(true) => snapshots += 1,
            Ok(false) => {}
            Err(e) => return e.into_response(),
        }
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({"agents": agents.len(), "snapshots": snapshots})),
    )
        .into_response()
}

/// Lift the kill switch. Other halts (daily loss, drawdown) keep their state.
async fn clear_kill_switch(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(e) = admin_gate(&state, &headers) {
        return e.into_response();
    }
    if let Err(e) = state.db.set_global_kill_switch(false) {
        return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    let agents = match state.db.list_agents() {
        Ok(agents) => agents,
        Err(e) => {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    };
    let mut snapshots = 0usize;
    for agent in &agents {
        if let Err(e) = state.db.set_kill_switch(&agent.id, false) {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
        match update_snapshot(&state, &agent.id, |snapshot| snapshot.clear_kill_switch()) {
            Ok(true) => snapshots += 1,
            Ok(false) => {}
            Err(e) => return e.into_response(),
        }
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({"agents": agents.len(), "snapshots": snapshots})),
    )
        .into_response()
}

/// Re-baseline one agent's peak and day at its current equity and clear the
/// halt. A scheduled flatten is left alone.
async fn reset_agent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(e) = admin_gate(&state, &headers) {
        return e.into_response();
    }
    let row = match agent_or_404(&state, &id) {
        Ok(row) => row,
        Err(e) => return e.into_response(),
    };
    // Same rule as start: the global flag also covers agents created after
    // the switch, whose per-agent column was never set.
    let global_kill = match state.db.global_kill_switch_on() {
        Ok(on) => on,
        Err(e) => {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    };
    if global_kill || row.kill_switch {
        return err_json(
            StatusCode::CONFLICT,
            format!("agent {id} is halted by the kill switch; clear it first"),
        )
        .into_response();
    }
    if row.snapshot.is_none() {
        return err_json(
            StatusCode::CONFLICT,
            format!("agent {id} has no snapshot; start it first"),
        )
        .into_response();
    }
    let risk = row.risk;
    match update_snapshot(&state, &id, |snapshot| {
        if snapshot.risk_limits().is_none() {
            snapshot.set_risk_limits(risk);
        }
        snapshot.reset_risk();
    }) {
        Ok(_) => {}
        Err(e) => return e.into_response(),
    }
    match agent_or_404(&state, &id).and_then(|row| agent_detail(&state, row)) {
        Ok(detail) => agent_json(detail),
        Err(e) => e.into_response(),
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
            admin_token: None,
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

    fn walk_forward_body(fake: &FakeSource) -> serde_json::Value {
        serde_json::json!({
            "strategy": valid_strategy(),
            "start_ms": fake.start(),
            "end_ms": fake.end(),
            "windows": 2,
            "is_bars": 60,
            "oos_bars": 30,
            "params": [{
                "target": "entry.all.0.right",
                "kind": "float",
                "values": [99.0, 100.0, 101.0]
            }],
            "search": {"method": "grid"}
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
    async fn walk_forward_rejects_invalid_windows_lengths_and_evaluation_caps() {
        let dir = temp_dir("walk_forward_bad");
        let fake = FakeSource::new();
        let router = app_with_limits(
            test_state(dir.clone()),
            DEFAULT_ORIGIN,
            Limits::new(30, 30, 2),
        )
        .expect("router");
        let mut cases = Vec::new();

        let mut body = walk_forward_body(&fake);
        body["windows"] = serde_json::json!(1);
        cases.push((body, "windows"));

        let mut body = walk_forward_body(&fake);
        body["windows"] = serde_json::json!(11);
        cases.push((body, "windows"));

        let mut body = walk_forward_body(&fake);
        body["is_bars"] = serde_json::json!(0);
        cases.push((body, "is_bars"));

        let mut body = walk_forward_body(&fake);
        body["oos_bars"] = serde_json::json!(0);
        cases.push((body, "oos_bars"));

        let mut body = walk_forward_body(&fake);
        body["windows"] = serde_json::json!(2);
        body["is_bars"] = serde_json::json!(200);
        body["oos_bars"] = serde_json::json!(60);
        cases.push((body, "not enough candles"));

        let mut body = walk_forward_body(&fake);
        body["windows"] = serde_json::json!(10);
        body["search"] = serde_json::json!({"method": "random", "trials": 201, "seed": 7});
        cases.push((body, "optimizer evaluations"));

        for (body, expected) in cases {
            let res = router
                .clone()
                .oneshot(json_req("/walk-forward", &body))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::BAD_REQUEST, "body: {body}");
            assert!(
                body_json(res).await["error"]
                    .as_str()
                    .unwrap_or("")
                    .contains(expected),
                "expected {expected:?} in error for {body}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn walk_forward_reuses_optimizer_rate_limit_and_returns_report() {
        let dir = temp_dir("walk_forward_rate");
        let fake = FakeSource::new();
        let router = app_with_limits(
            test_state(dir.clone()),
            DEFAULT_ORIGIN,
            Limits::new(30, 1, 2),
        )
        .expect("router");
        let body = walk_forward_body(&fake);
        let first = router
            .clone()
            .oneshot(json_req("/walk-forward", &body))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let report = body_json(first).await;
        assert_eq!(report["mode"], "rolling");
        assert_eq!(report["windows"].as_array().unwrap().len(), 2);
        assert_eq!(
            report["stitched_oos_equity_curve"]
                .as_array()
                .unwrap()
                .len(),
            61
        );
        let second = router
            .oneshot(json_req("/walk-forward", &body))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
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
            admin_token: None,
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
            admin_token: None,
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
        let expected = frozen_golden_trades();
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
    async fn backtest_without_a_risk_block_matches_the_frozen_golden() {
        let dir = temp_dir("bt_no_risk_golden");
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
            admin_token: None,
        };
        let body = serde_json::json!({
            "strategy": seeded_strategy(),
            "start_ms": start,
            "end_ms": end,
        });
        let res = app(state)
            .oneshot(json_req("/backtest", &body))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert!(v["risk_halt"].is_null(), "no risk block means no halt");

        // Bytes on the wire must equal the plain simulator run.
        let spec: crate::strategy::Strategy =
            serde_json::from_value(seeded_strategy()).expect("strategy");
        let direct = crate::backtest::backtest_strategy_with_risk(
            &spec,
            &candles,
            &crate::backtest::BacktestConfig::default(),
            None,
        )
        .expect("direct run")
        .result;
        assert_eq!(
            v["trades"],
            serde_json::to_value(&direct.trades).expect("trades")
        );
        assert_eq!(
            v["equity_curve"],
            serde_json::to_value(&direct.equity_curve).expect("equity")
        );

        // And both are the frozen golden, trade for trade.
        let http_trades = v["trades"].as_array().expect("trades");
        assert_eq!(http_trades.len(), 226, "frozen trade count");
        for (h, e) in http_trades.iter().zip(frozen_golden_trades().iter()) {
            assert_eq!(h["entry_idx"].as_u64().unwrap(), e.0);
            assert_eq!(h["exit_idx"].as_u64().unwrap(), e.1);
            assert_eq!(h["side"].as_str().unwrap(), e.2);
            assert_eq!(h["reason"].as_str().unwrap(), e.3);
            let hr = h["return_pct"].as_f64().unwrap();
            assert!((hr - e.4).abs() <= 0.05, "return_pct {hr} vs {}", e.4);
        }
        let equity = v["equity_curve"].as_array().expect("equity");
        assert_eq!(equity.len(), 3000);
        let last = equity
            .last()
            .expect("last equity")
            .as_f64()
            .expect("number");
        assert_eq!(last, 0.676222700666094, "frozen final equity");
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
            admin_token: None,
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
                admin_token: None,
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

    fn validate_body(strategy: serde_json::Value, start: i64, end: i64) -> serde_json::Value {
        serde_json::json!({
            "strategy": strategy,
            "start_ms": start,
            "end_ms": end,
        })
    }

    fn seeded_strategy() -> serde_json::Value {
        serde_json::json!({
            "asset": "BTCUSDT",
            "timeframe": "1h",
            "side": "long",
            "entry": {"all": [{"left": {"ind": "close"}, "op": "crosses_above", "right": {"ind": "sma", "period": 10}}]},
            "exit": {"all": [{"left": {"ind": "close"}, "op": "crosses_below", "right": {"ind": "sma", "period": 10}}]},
            "stop_loss_pct": 2.0,
            "take_profit_pct": 4.0,
        })
    }

    /// The first trades of the frozen 226-trade golden on `seeded_walk(3000, 42)`.
    /// Any change here means the shared simulator changed behaviour.
    fn frozen_golden_trades() -> &'static [(u64, u64, &'static str, &'static str, f64)] {
        &[
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
        ]
    }

    #[tokio::test]
    async fn validate_runs_the_same_strategy_on_three_markets() {
        let dir = temp_dir("validate_three");
        let fake = FakeSource::new();
        let body = validate_body(valid_strategy(), fake.start(), fake.end());
        let res = app(test_state(dir.clone()))
            .oneshot(
                Request::post("/validate")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert_eq!(v["tested"], 3);
        assert!(v["holds"].as_u64().unwrap() <= v["tested"].as_u64().unwrap());
        assert!(v["grade"].is_string());
        assert!(v["headline"].is_string());
        assert!(v["unchecked"].as_array().unwrap().is_empty());
        let markets = v["markets"].as_array().expect("markets");
        assert_eq!(markets.len(), 3);
        let symbols: Vec<&str> = markets
            .iter()
            .map(|m| m["symbol"].as_str().unwrap())
            .collect();
        assert_eq!(symbols, vec!["BTCUSDT", "ETHUSDT", "SOLUSDT"]);
        for market in markets {
            assert!(market["candle_count"].as_u64().unwrap() > 0);
            assert!(market["stats"]["trades"].as_u64().unwrap() > 0);
            assert!(market["out_of_sample"].is_null());
            assert!(market["verdict"]["grade"].is_string());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn validate_keeps_the_primary_market_first_and_dedupes() {
        let dir = temp_dir("validate_dedupe");
        let fake = FakeSource::new();
        let mut body = validate_body(valid_strategy(), fake.start(), fake.end());
        body["symbols"] = serde_json::json!(["ETHUSDT", "BTCUSDT", "ETHUSDT"]);
        let res = app(test_state(dir.clone()))
            .oneshot(
                Request::post("/validate")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        let markets = v["markets"].as_array().expect("markets");
        assert_eq!(markets.len(), 2);
        assert_eq!(markets[0]["symbol"], "BTCUSDT");
        assert_eq!(markets[1]["symbol"], "ETHUSDT");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn validate_rejects_bad_symbol_lists() {
        let dir = temp_dir("validate_bad_symbols");
        let fake = FakeSource::new();
        let cases = vec![
            serde_json::json!([]),
            serde_json::json!(["btcusdt"]),
            serde_json::json!(["BTCUSDT"]),
            serde_json::json!(["ETHUSDT", "SOLUSDT", "BNBUSDT", "XRPUSDT", "DOGEUSDT"]),
        ];
        for symbols in cases {
            let mut body = validate_body(valid_strategy(), fake.start(), fake.end());
            body["symbols"] = symbols.clone();
            let res = app(test_state(dir.clone()))
                .oneshot(
                    Request::post("/validate")
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::BAD_REQUEST, "symbols: {symbols}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct PickySource {
        inner: FakeSource,
        broken: &'static str,
    }

    impl crate::feeds::KlineSource for PickySource {
        fn fetch(
            &self,
            symbol: &str,
            interval: &str,
            start_ms: i64,
            end_ms: i64,
            limit: usize,
        ) -> Result<Vec<crate::types::Candle>, crate::feeds::FeedError> {
            if symbol == self.broken {
                return Err(crate::feeds::FeedError::Request("feed down".to_string()));
            }
            self.inner.fetch(symbol, interval, start_ms, end_ms, limit)
        }
    }

    #[tokio::test]
    async fn validate_reports_a_market_it_could_not_load() {
        let dir = temp_dir("validate_unchecked");
        let fake = FakeSource::new();
        let state = AppState {
            source: Arc::new(PickySource {
                inner: FakeSource::new(),
                broken: "ETHUSDT",
            }),
            cache_dir: dir.clone(),
            jobs: Arc::new(JobStore::default()),
            db: Arc::new(crate::db::Db::open_in_memory().expect("mem db")),
            admin_token: None,
        };
        let body = validate_body(valid_strategy(), fake.start(), fake.end());
        let res = app(state)
            .oneshot(
                Request::post("/validate")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert_eq!(v["tested"], 2);
        assert_eq!(v["grade"], "fragile");
        let unchecked = v["unchecked"].as_array().expect("unchecked");
        assert_eq!(unchecked.len(), 1);
        assert_eq!(unchecked[0]["symbol"], "ETHUSDT");
        assert!(
            unchecked[0]["reason"]
                .as_str()
                .unwrap()
                .contains("feed down"),
            "reason: {}",
            unchecked[0]["reason"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn validate_charges_the_backtest_budget_per_market() {
        let dir = temp_dir("validate_rate_limit");
        let fake = FakeSource::new();
        let router = app_with_limits(
            test_state(dir.clone()),
            DEFAULT_ORIGIN,
            Limits::new(3, 5, 2),
        )
        .expect("router");
        let body = validate_body(valid_strategy(), fake.start(), fake.end());
        let request = || {
            Request::post("/validate")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let res = router.clone().oneshot(request()).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let res = router.oneshot(request()).await.unwrap();
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(res.headers().get("retry-after").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn validate_aggregates_the_same_walk_on_every_market() {
        let dir = temp_dir("validate_walk");
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
            admin_token: None,
        };
        let body = validate_body(seeded_strategy(), start, end);
        let res = app(state)
            .oneshot(
                Request::post("/validate")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        let markets = v["markets"].as_array().expect("markets");
        assert_eq!(markets.len(), 3);
        for market in markets {
            assert_eq!(market["stats"], markets[0]["stats"]);
            assert_eq!(market["verdict"], markets[0]["verdict"]);
        }
        assert_eq!(v["grade"], markets[0]["verdict"]["grade"]);
        assert_eq!(v["tested"], 3);
        assert_eq!(
            v["holds"].as_u64().unwrap(),
            if v["grade"] == "holds" { 3 } else { 0 }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn json_req(uri: &str, body: &serde_json::Value) -> Request<Body> {
        Request::post(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn get_req(uri: &str) -> Request<Body> {
        Request::get(uri).body(Body::empty()).unwrap()
    }

    async fn create_agent(
        router: &Router,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let res = router
            .clone()
            .oneshot(json_req("/agents", &body))
            .await
            .unwrap();
        let status = res.status();
        (status, body_json(res).await)
    }

    fn admin_state(dir: PathBuf, token: Option<&str>) -> AppState {
        AppState {
            admin_token: token.map(str::to_string),
            ..test_state(dir)
        }
    }

    fn admin_req(uri: &str, token: Option<&str>) -> Request<Body> {
        let mut builder = Request::post(uri);
        if let Some(token) = token {
            builder = builder.header("x-admin-token", token);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn invalid_risk_limits_are_rejected_with_400() {
        let dir = temp_dir("bad_risk");
        let router = app(test_state(dir.clone()));
        let fake = FakeSource::new();
        let cases = [
            serde_json::json!({
                "max_position_pct": 0.0,
                "max_daily_loss_pct": 10.0,
                "max_drawdown_pct": 20.0,
            }),
            serde_json::json!({
                "max_position_pct": 100.0,
                "max_daily_loss_pct": 101.0,
                "max_drawdown_pct": 20.0,
            }),
            serde_json::json!({
                "max_position_pct": 100.0,
                "max_daily_loss_pct": 10.0,
                "max_drawdown_pct": -1.0,
            }),
        ];
        for risk in cases {
            let body = serde_json::json!({
                "strategy": valid_strategy(),
                "start_ms": fake.start(),
                "end_ms": fake.end(),
                "risk": risk.clone(),
            });
            let res = router
                .clone()
                .oneshot(json_req("/backtest", &body))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::BAD_REQUEST, "backtest {risk}");

            let (status, _) = create_agent(
                &router,
                serde_json::json!({
                    "name": "Bad risk",
                    "strategy": valid_strategy(),
                    "risk": risk,
                }),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "agent {risk}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn an_agent_without_a_risk_block_gets_the_default_caps() {
        let dir = temp_dir("default_risk");
        let router = app(test_state(dir.clone()));
        let (status, created) = create_agent(
            &router,
            serde_json::json!({ "name": "Default", "strategy": valid_strategy() }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create: {created}");
        assert_eq!(created["risk"]["max_position_pct"], 100.0);
        assert_eq!(created["risk"]["max_daily_loss_pct"], 10.0);
        assert_eq!(created["risk"]["max_drawdown_pct"], 20.0);
        assert!(created["halt"].is_null());

        let (status, tight) = create_agent(
            &router,
            serde_json::json!({
                "name": "Tight",
                "strategy": valid_strategy(),
                "risk": {
                    "max_position_pct": 25.0,
                    "max_daily_loss_pct": 5.0,
                    "max_drawdown_pct": 15.0,
                },
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create: {tight}");
        assert_eq!(tight["risk"]["max_position_pct"], 25.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn admin_endpoints_fail_closed() {
        let dir = temp_dir("admin_gate");
        let uris = ["/kill-switch", "/kill-switch/clear", "/agents/x/reset"];

        // No token configured: refuse even a correct-looking header.
        let disabled = app(admin_state(dir.clone(), None));
        for uri in uris {
            let res = disabled
                .clone()
                .oneshot(admin_req(uri, Some("secret")))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE, "{uri}");
        }

        let router = app(admin_state(dir.clone(), Some("secret")));
        for uri in uris {
            let res = router.clone().oneshot(admin_req(uri, None)).await.unwrap();
            assert_eq!(res.status(), StatusCode::FORBIDDEN, "{uri} without token");
            let res = router
                .clone()
                .oneshot(admin_req(uri, Some("wrong")))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::FORBIDDEN, "{uri} wrong token");
        }

        let res = router
            .clone()
            .oneshot(admin_req("/kill-switch", Some("secret")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let res = router
            .clone()
            .oneshot(admin_req("/kill-switch/clear", Some("secret")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let res = router
            .oneshot(admin_req("/agents/nope/reset", Some("secret")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn kill_switch_blocks_an_agent_created_after_it_was_flipped() {
        let dir = temp_dir("kill_new_agent");
        let router = app(admin_state(dir.clone(), Some("secret")));
        let fake = FakeSource::new();

        // Flip the global switch before any agent exists.
        let res = router
            .clone()
            .oneshot(admin_req("/kill-switch", Some("secret")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // A brand-new agent is created afterwards; its per-agent column was
        // never set by the (empty) sweep over existing agents.
        let (status, created) = create_agent(
            &router,
            serde_json::json!({
                "name": "Created after the switch",
                "strategy": valid_strategy(),
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create: {created}");
        let id = created["agent"]["id"].as_str().expect("id").to_string();

        let res = router
            .clone()
            .oneshot(json_req(
                &format!("/agents/{id}/start"),
                &serde_json::json!({ "start_ms": fake.start(), "end_ms": fake.end() }),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        let msg = body_json(res).await["error"]
            .as_str()
            .unwrap_or("")
            .to_lowercase();
        assert!(msg.contains("kill switch"), "message: {msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn kill_switch_survives_a_restart_and_clear_lifts_it() {
        let dir = temp_dir("kill_restart");
        let db_path = dir.join("runs.db");
        let fake = FakeSource::new();
        let open_state = || AppState {
            source: Arc::new(FakeSource::new()),
            cache_dir: dir.clone(),
            jobs: Arc::new(JobStore::default()),
            db: Arc::new(crate::db::Db::open(&db_path).expect("file db")),
            admin_token: Some("secret".to_string()),
        };

        let router = app(open_state());
        let (status, created) = create_agent(
            &router,
            serde_json::json!({
                "name": "Guarded",
                "strategy": valid_strategy(),
                "initial_equity": 10_000.0,
                // Wide enough that the warm-up itself never halts.
                "risk": {
                    "max_position_pct": 100.0,
                    "max_daily_loss_pct": 100.0,
                    "max_drawdown_pct": 100.0,
                },
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create: {created}");
        let id = created["agent"]["id"].as_str().expect("id").to_string();

        let res = router
            .clone()
            .oneshot(json_req(
                &format!("/agents/{id}/start"),
                &serde_json::json!({ "start_ms": fake.start(), "end_ms": fake.end() }),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(body_json(res).await["halt"].is_null(), "starts unhalted");

        let res = router
            .clone()
            .oneshot(admin_req("/kill-switch", Some("secret")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_json(res).await["agents"], 1);

        let res = router
            .clone()
            .oneshot(
                Request::get(format!("/agents/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(body_json(res).await["halt"], "KillSwitch");

        let res = router
            .clone()
            .oneshot(json_req(
                &format!("/agents/{id}/stop"),
                &serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        drop(router);

        // A fresh process opens the same database.
        let restarted = app(open_state());
        let res = restarted
            .clone()
            .oneshot(
                Request::get(format!("/agents/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            body_json(res).await["halt"],
            "KillSwitch",
            "the halt must survive a restart"
        );

        let res = restarted
            .clone()
            .oneshot(json_req(
                &format!("/agents/{id}/start"),
                &serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        let msg = body_json(res).await["error"]
            .as_str()
            .unwrap_or("")
            .to_lowercase();
        assert!(msg.contains("kill switch"), "message: {msg}");

        let res = restarted
            .clone()
            .oneshot(admin_req("/kill-switch/clear", Some("secret")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let res = restarted
            .clone()
            .oneshot(
                Request::get(format!("/agents/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(body_json(res).await["halt"].is_null());
        let res = restarted
            .oneshot(json_req(
                &format!("/agents/{id}/start"),
                &serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "clear lets it resume");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn reset_is_admin_only_and_clears_the_halt() {
        let dir = temp_dir("agent_reset");
        let router = app(admin_state(dir.clone(), Some("secret")));
        let fake = FakeSource::new();
        let (status, created) = create_agent(
            &router,
            serde_json::json!({
                "name": "Tight",
                "strategy": valid_strategy(),
                "risk": {
                    "max_position_pct": 100.0,
                    "max_daily_loss_pct": 100.0,
                    // A sine walk always draws down more than this.
                    "max_drawdown_pct": 0.5,
                },
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create: {created}");
        let id = created["agent"]["id"].as_str().expect("id").to_string();

        let res = router
            .clone()
            .oneshot(admin_req(&format!("/agents/{id}/reset"), Some("wrong")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // Nothing to re-baseline before the agent has a snapshot.
        let res = router
            .clone()
            .oneshot(admin_req(&format!("/agents/{id}/reset"), Some("secret")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);

        let res = router
            .clone()
            .oneshot(json_req(
                &format!("/agents/{id}/start"),
                &serde_json::json!({ "start_ms": fake.start(), "end_ms": fake.end() }),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let started = body_json(res).await;
        assert_eq!(
            started["halt"], "Drawdown",
            "the tight cap must trip during warm-up"
        );

        let res = router
            .clone()
            .oneshot(admin_req(&format!("/agents/{id}/reset"), Some("secret")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let reset = body_json(res).await;
        assert!(reset["halt"].is_null(), "reset clears the halt");
        assert_eq!(reset["risk"]["max_drawdown_pct"], 0.5);

        let res = router
            .oneshot(
                Request::get(format!("/agents/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(body_json(res).await["halt"].is_null());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn reset_is_blocked_while_the_global_kill_switch_is_on() {
        let dir = temp_dir("reset_global_kill");
        let state = admin_state(dir.clone(), Some("secret"));
        let db = state.db.clone();
        let router = app(state);
        let fake = FakeSource::new();

        let (status, created) = create_agent(
            &router,
            serde_json::json!({
                "name": "Wide caps",
                "strategy": valid_strategy(),
                // Wide enough that the warm-up itself never halts.
                "risk": {
                    "max_position_pct": 100.0,
                    "max_daily_loss_pct": 100.0,
                    "max_drawdown_pct": 100.0,
                },
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create: {created}");
        let id = created["agent"]["id"].as_str().expect("id").to_string();

        // Start gives the agent a snapshot, so without the global check the
        // reset would succeed instead of returning 409.
        let res = router
            .clone()
            .oneshot(json_req(
                &format!("/agents/{id}/start"),
                &serde_json::json!({ "start_ms": fake.start(), "end_ms": fake.end() }),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "start gives a snapshot");

        // Flip only the global flag: this agent's per-agent column stays off,
        // so the global state alone must block the reset.
        db.set_global_kill_switch(true).expect("flip global");
        assert!(db.global_kill_switch_on().expect("read global"));
        assert!(!db.get_agent(&id).expect("get").expect("exists").kill_switch);

        let res = router
            .clone()
            .oneshot(admin_req(&format!("/agents/{id}/reset"), Some("secret")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        let msg = body_json(res).await["error"]
            .as_str()
            .unwrap_or("")
            .to_lowercase();
        assert!(msg.contains("kill switch"), "message: {msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn agent_lifecycle_warm_up_advance_stop_and_resume() {
        let dir = temp_dir("agent_lifecycle");
        let router = app(test_state(dir.clone()));
        let fake = FakeSource::new();
        let step: i64 = 3_600_000;

        let (status, created) = create_agent(
            &router,
            serde_json::json!({
                "name": "Paper BTC",
                "strategy": valid_strategy(),
                "initial_equity": 5000.0,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create: {created}");
        let id = created["agent"]["id"].as_str().expect("id").to_string();
        assert_eq!(created["agent"]["status"], "draft");
        assert_eq!(created["agent"]["symbol"], "BTCUSDT");
        assert_eq!(created["initial_equity"], 5000.0);

        let res = router
            .clone()
            .oneshot(Request::get("/agents").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let listed = body_json(res).await;
        assert_eq!(listed["agents"].as_array().expect("agents").len(), 1);

        let warm_end = fake.start() + step * 100;
        let res = router
            .clone()
            .oneshot(json_req(
                &format!("/agents/{id}/start"),
                &serde_json::json!({ "start_ms": fake.start(), "end_ms": warm_end }),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let started = body_json(res).await;
        assert_eq!(started["agent"]["status"], "running");
        let warm_bars = started["candle_count"].as_u64().expect("candles");
        assert!(warm_bars >= 90, "warm-up bars: {warm_bars}");
        assert!(started["agent"]["equity"].as_f64().expect("equity") > 0.0);

        let res = router
            .clone()
            .oneshot(json_req(
                &format!("/agents/{id}/advance"),
                &serde_json::json!({ "end_ms": fake.end() }),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let advanced = body_json(res).await;
        assert!(
            advanced["candle_count"].as_u64().expect("candles") > warm_bars,
            "advance must append new closed candles"
        );

        let res = router
            .clone()
            .oneshot(json_req(
                &format!("/agents/{id}/stop"),
                &serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_json(res).await["agent"]["status"], "stopped");

        let res = router
            .clone()
            .oneshot(json_req(
                &format!("/agents/{id}/advance"),
                &serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);

        let res = router
            .clone()
            .oneshot(json_req(
                &format!("/agents/{id}/start"),
                &serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_json(res).await["agent"]["status"], "running");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn agent_history_endpoints_paginate_validate_and_return_not_found() {
        let dir = temp_dir("agent_history_routes");
        let state = test_state(dir.clone());
        let db = state.db.clone();
        let router = app(state);
        let (status, created) = create_agent(
            &router,
            serde_json::json!({
                "name": "History",
                "strategy": valid_strategy(),
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create: {created}");
        let id = created["agent"]["id"].as_str().expect("id").to_string();
        let decisions = [
            (100, 0, "signal_received"),
            (100, 1, "entry_filled"),
            (200, 0, "exit_filled"),
            (300, 0, "halt_triggered"),
            (300, 1, "exit_filled"),
        ]
        .map(|(bar_ts, seq, kind)| crate::backtest::DecisionEvent {
            bar_ts,
            kind: kind.to_string(),
            seq,
            reason: None,
            price: None,
            qty: None,
            requested_qty: None,
            configured: None,
            observed: None,
            equity: 10_000.0,
        });
        let points = [100, 200, 300, 400].map(|bar_ts| crate::agent::EquityPoint {
            bar_ts,
            equity: 10_000.0 + bar_ts as f64,
        });
        assert!(db
            .save_agent_advance(crate::db::AgentAdvanceWrite {
                id: &id,
                status: "running",
                equity: 10_400.0,
                last_price: 100.0,
                last_ts: 400,
                snapshot: "{}",
                decisions: &decisions,
                equity_points: &points,
            })
            .expect("seed history"));

        let res = router
            .clone()
            .oneshot(get_req(&format!("/agents/{id}/decisions")))
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "read-only route has no admin gate"
        );
        assert_eq!(
            body_json(res).await["decisions"].as_array().unwrap().len(),
            5
        );

        let res = router
            .clone()
            .oneshot(get_req(&format!("/agents/{id}/decisions?limit=2")))
            .await
            .unwrap();
        let page = body_json(res).await;
        assert_eq!(page["decisions"][0]["bar_ts"], 300);
        assert_eq!(page["decisions"][0]["seq"], 1);
        assert_eq!(page["decisions"][1]["seq"], 0);
        assert_eq!(page["next_before"], "300:0");

        let res = router
            .clone()
            .oneshot(get_req(&format!(
                "/agents/{id}/decisions?limit=2&before=300:0"
            )))
            .await
            .unwrap();
        let page = body_json(res).await;
        assert_eq!(page["decisions"][0]["bar_ts"], 200);
        assert_eq!(page["decisions"][1]["bar_ts"], 100);
        assert_eq!(page["decisions"][1]["seq"], 1);

        let res = router
            .clone()
            .oneshot(get_req(&format!("/agents/{id}/equity?limit=2")))
            .await
            .unwrap();
        let page = body_json(res).await;
        assert_eq!(page["equity"][0]["bar_ts"], 100);
        assert_eq!(page["equity"][1]["bar_ts"], 200);
        assert_eq!(page["next_since"], 201);

        let res = router
            .clone()
            .oneshot(get_req(&format!("/agents/{id}/equity?limit=2&since=201")))
            .await
            .unwrap();
        let page = body_json(res).await;
        assert_eq!(page["equity"][0]["bar_ts"], 300);
        assert_eq!(page["equity"][1]["bar_ts"], 400);

        for uri in [
            format!("/agents/{id}/decisions?limit=0"),
            format!("/agents/{id}/decisions?limit=501"),
            format!("/agents/{id}/decisions?before=-1:0"),
            format!("/agents/{id}/decisions?before=1:-1"),
            format!("/agents/{id}/equity?limit=0"),
            format!("/agents/{id}/equity?limit=5001"),
            format!("/agents/{id}/equity?since=-1"),
            format!("/agents/{id}/equity?limit=abc"),
        ] {
            let res = router.clone().oneshot(get_req(&uri)).await.unwrap();
            assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{uri}");
        }
        for uri in ["/agents/missing/decisions", "/agents/missing/equity"] {
            let res = router.clone().oneshot(get_req(uri)).await.unwrap();
            assert_eq!(res.status(), StatusCode::NOT_FOUND, "{uri}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn retrying_agent_advance_does_not_duplicate_history() {
        let dir = temp_dir("agent_history_retry");
        let state = test_state(dir.clone());
        let db = state.db.clone();
        let router = app(state);
        let fake = FakeSource::new();
        let step = 3_600_000;
        let (status, created) = create_agent(
            &router,
            serde_json::json!({
                "name": "Retry",
                "strategy": valid_strategy(),
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create: {created}");
        let id = created["agent"]["id"].as_str().expect("id").to_string();
        let warm_end = fake.start() + step * 100;
        let res = router
            .clone()
            .oneshot(json_req(
                &format!("/agents/{id}/start"),
                &serde_json::json!({ "start_ms": fake.start(), "end_ms": warm_end }),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let _started = body_json(res).await;

        let advance = || {
            json_req(
                &format!("/agents/{id}/advance"),
                &serde_json::json!({ "end_ms": fake.end() }),
            )
        };
        let res = router.clone().oneshot(advance()).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let advanced = body_json(res).await;
        let decisions_before = db
            .list_agent_decisions(&id, 5_000, None)
            .expect("decisions after first advance");
        let equity_before = db
            .list_agent_equity(&id, 20_000, None)
            .expect("equity after first advance");
        assert!(!equity_before.is_empty());
        assert_eq!(
            equity_before.len() as u64,
            advanced["candle_count"].as_u64().expect("candle count"),
            "one point is stored for every bar processed by the agent"
        );
        let snapshot_before = db.get_agent(&id).expect("get").expect("exists").snapshot;

        let res = router.clone().oneshot(advance()).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let decisions_after = db
            .list_agent_decisions(&id, 5_000, None)
            .expect("decisions after retry");
        let equity_after = db
            .list_agent_equity(&id, 20_000, None)
            .expect("equity after retry");
        let snapshot_after = db.get_agent(&id).expect("get").expect("exists").snapshot;
        assert_eq!(decisions_after.len(), decisions_before.len());
        assert_eq!(equity_after.len(), equity_before.len());
        assert_eq!(snapshot_after, snapshot_before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn halted_agent_logs_flatten_and_halt_then_noop_adds_no_rows() {
        let dir = temp_dir("halted_agent_history");
        let state = admin_state(dir.clone(), Some("secret"));
        let db = state.db.clone();
        let router = app(state);
        let fake = FakeSource::new();
        let step = 3_600_000;
        let (status, created) = create_agent(
            &router,
            serde_json::json!({
                "name": "Halted history",
                "strategy": {
                    "asset": "BTCUSDT",
                    "timeframe": "1h",
                    "side": "long",
                    "entry": {"all": [{"left": {"ind": "close"}, "op": ">", "right": 0.0}]}
                },
                "risk": {
                    "max_position_pct": 100.0,
                    "max_daily_loss_pct": 100.0,
                    "max_drawdown_pct": 100.0,
                },
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create: {created}");
        let id = created["agent"]["id"].as_str().expect("id").to_string();
        let warm_end = fake.start() + step * 100;
        let res = router
            .clone()
            .oneshot(json_req(
                &format!("/agents/{id}/start"),
                &serde_json::json!({ "start_ms": fake.start(), "end_ms": warm_end }),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let started = body_json(res).await;
        assert!(started["position"].is_object());

        let res = router
            .clone()
            .oneshot(admin_req("/kill-switch", Some("secret")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let advance = || {
            json_req(
                &format!("/agents/{id}/advance"),
                &serde_json::json!({ "end_ms": fake.end() }),
            )
        };
        let res = router.clone().oneshot(advance()).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let advanced = body_json(res).await;

        let decisions = db
            .list_agent_decisions(&id, 5_000, None)
            .expect("decisions");
        assert!(decisions.iter().any(|event| {
            event.kind == "halt_triggered" && event.reason.as_deref() == Some("kill_switch")
        }));
        assert!(decisions.iter().any(|event| {
            event.kind == "exit_filled" && event.reason.as_deref() == Some("risk_halt")
        }));
        let equity_count = db
            .list_agent_equity(&id, 20_000, None)
            .expect("equity")
            .len();
        assert_eq!(
            equity_count as u64,
            advanced["candle_count"].as_u64().expect("candle count")
        );

        let res = router.clone().oneshot(advance()).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            db.list_agent_decisions(&id, 5_000, None)
                .expect("decisions after noop")
                .len(),
            decisions.len()
        );
        assert_eq!(
            db.list_agent_equity(&id, 20_000, None)
                .expect("equity after noop")
                .len(),
            equity_count
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn agent_can_be_deployed_from_a_saved_run() {
        let dir = temp_dir("agent_from_run");
        let router = app(test_state(dir.clone()));
        let fake = FakeSource::new();
        let res = router
            .clone()
            .oneshot(json_req(
                "/backtest",
                &serde_json::json!({
                    "strategy": valid_strategy(),
                    "start_ms": fake.start(),
                    "end_ms": fake.end(),
                }),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let run_id = body_json(res).await["run_id"].as_i64().expect("run id");

        let (status, created) = create_agent(
            &router,
            serde_json::json!({ "name": "From run", "run_id": run_id }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create: {created}");
        assert_eq!(created["agent"]["run_id"], run_id);
        let id = created["agent"]["id"].as_str().expect("id").to_string();

        // The saved run supplies the warm-up range, so an empty body is enough.
        let res = router
            .clone()
            .oneshot(json_req(
                &format!("/agents/{id}/start"),
                &serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let started = body_json(res).await;
        assert_eq!(started["agent"]["status"], "running");
        assert_eq!(started["candle_count"].as_u64().expect("candles"), 300);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn agent_rejects_ambiguous_creation_and_missing_warm_up() {
        let dir = temp_dir("agent_bad");
        let router = app(test_state(dir.clone()));

        let (status, _) = create_agent(
            &router,
            serde_json::json!({
                "name": "Both",
                "strategy": valid_strategy(),
                "run_id": 1,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, _) = create_agent(&router, serde_json::json!({ "name": "Neither" })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, _) = create_agent(
            &router,
            serde_json::json!({ "name": "  ", "strategy": valid_strategy() }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (_, created) = create_agent(
            &router,
            serde_json::json!({ "name": "No range", "strategy": valid_strategy() }),
        )
        .await;
        let id = created["agent"]["id"].as_str().expect("id").to_string();
        let res = router
            .clone()
            .oneshot(json_req(
                &format!("/agents/{id}/start"),
                &serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        let res = router
            .clone()
            .oneshot(Request::get("/agents/nope").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
