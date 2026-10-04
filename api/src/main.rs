use api::{app_with_origin, AppState};
use std::{path::PathBuf, sync::Arc};

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8080);
    let cache_dir =
        PathBuf::from(std::env::var("CACHE_DIR").unwrap_or_else(|_| ".cache".to_string()));
    let base_url = std::env::var("BINANCE_BASE_URL").ok();
    let origin =
        std::env::var("ALLOWED_ORIGIN").unwrap_or_else(|_| "http://localhost:3000".to_string());

    let source: Arc<dyn marketdata::KlineSource + Send + Sync> = match base_url {
        Some(url) => Arc::new(marketdata::BinanceSource { base_url: url }),
        None => Arc::new(marketdata::BinanceSource::new()),
    };
    let state = AppState { source, cache_dir };
    let router = app_with_origin(state, &origin);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind failed");
    axum::serve(listener, router).await.expect("serve failed");
}
