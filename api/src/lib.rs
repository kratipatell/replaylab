use axum::{
    extract::{rejection::JsonRejection, rejection::QueryRejection, Query, State},
    http::{HeaderValue, Method, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use tower_http::cors::{AllowOrigin, CorsLayer};

#[derive(Clone)]
pub struct AppState {
    pub source: Arc<dyn marketdata::KlineSource + Send + Sync>,
    pub cache_dir: PathBuf,
}

const DEFAULT_ORIGIN: &str = "http://localhost:3000";

pub fn app(state: AppState) -> Router {
    app_with_origin(state, DEFAULT_ORIGIN)
}

pub fn app_with_origin(state: AppState, origin: &str) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::exact(
            origin.parse::<HeaderValue>().unwrap_or_else(|_| {
                DEFAULT_ORIGIN
                    .parse::<HeaderValue>()
                    .expect("default origin parses")
            }),
        ))
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([axum::http::header::CONTENT_TYPE]);
    Router::new()
        .route("/health", get(health))
        .route("/candles", get(get_candles))
        .route("/backtest", post(post_backtest))
        .with_state(state)
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
    if let Err(e) = marketdata::validate_symbol(&q.symbol) {
        return err_json(StatusCode::BAD_REQUEST, e).into_response();
    }
    let step = match marketdata::interval_ms(&q.interval) {
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
        marketdata::load_or_fetch(&cache_dir, source.as_ref(), &symbol, &interval, start, end)
    })
    .await;
    let candles = match loaded {
        Err(e) => {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
        Ok(Err(e)) => return err_json(StatusCode::BAD_GATEWAY, e).into_response(),
        Ok(Ok(c)) => c,
    };
    let gaps = marketdata::find_gaps(&candles, step);
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
    strategy: engine::strategy::Strategy,
    start_ms: i64,
    end_ms: i64,
    fee_bps: Option<f64>,
    slippage_bps: Option<f64>,
    split: Option<f64>,
}

#[derive(Debug, Serialize)]
struct BacktestResponse {
    candle_count: usize,
    gaps: Vec<(i64, i64)>,
    stats: engine::stats::Stats,
    in_sample: Option<engine::stats::Stats>,
    out_of_sample: Option<engine::stats::Stats>,
    trades: Vec<engine::backtest::Trade>,
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
    if let Err(e) = marketdata::validate_symbol(&symbol) {
        return err_json(StatusCode::BAD_REQUEST, e).into_response();
    }
    let step = match marketdata::interval_ms(&interval) {
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
    let defaults = engine::backtest::BacktestConfig::default();
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
    let bars_per_year = match engine::stats::bars_per_year(&interval) {
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
        marketdata::load_or_fetch(&cache_dir, source.as_ref(), &symbol, &interval, start, end)
    })
    .await;
    let candles = match loaded {
        Err(e) => {
            return err_json(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
        Ok(Err(e)) => return err_json(StatusCode::BAD_GATEWAY, e).into_response(),
        Ok(Ok(c)) => c,
    };
    let cfg = engine::backtest::BacktestConfig {
        fee_bps,
        slippage_bps,
    };
    let result = match engine::backtest::backtest(&req.strategy, &candles, &cfg) {
        Ok(r) => r,
        Err(e) => return err_json(StatusCode::BAD_REQUEST, e).into_response(),
    };
    let stats = engine::stats::compute_stats(&result, &candles, bars_per_year);
    let (in_sample, out_of_sample) = match req.split {
        None => (None, None),
        Some(split) => {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let split_idx = (candles.len() as f64 * split).floor() as usize;
            match engine::stats::split_stats(&result, &candles, split_idx, bars_per_year) {
                Ok((is, oos)) => (Some(is), Some(oos)),
                Err(e) => return err_json(StatusCode::BAD_REQUEST, e).into_response(),
            }
        }
    };
    let gaps = marketdata::find_gaps(&candles, step);
    let resp = BacktestResponse {
        candle_count: candles.len(),
        gaps,
        stats,
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
        data: Vec<engine::Candle>,
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
                data.push(engine::Candle {
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

    impl marketdata::KlineSource for FakeSource {
        fn fetch(
            &self,
            _symbol: &str,
            _interval: &str,
            start_ms: i64,
            end_ms: i64,
            limit: usize,
        ) -> Result<Vec<engine::Candle>, String> {
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
}
