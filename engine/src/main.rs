// Temporary: allow dead code while workspace is restructured into one binary crate.
#![allow(dead_code)]

mod api;
mod backtest;
mod backtest_v2;
mod config;
mod feeds;
mod indicators;
mod instrument;
mod orb_recon;
mod session;
mod sizing;
mod stats;
mod strategies;
mod strategy;
mod types;

use std::sync::Arc;

use crate::api::{app_with_origin, AppState};
use crate::config::Config;

#[tokio::main]
async fn main() {
    let cfg = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let source: Arc<dyn feeds::KlineSource + Send + Sync> = match cfg.base_url {
        Some(url) => Arc::new(feeds::BinanceSource { base_url: url }),
        None => Arc::new(feeds::BinanceSource::new()),
    };
    let state = AppState {
        source,
        cache_dir: cfg.cache_dir,
    };
    let router = match app_with_origin(state, &cfg.origin) {
        Ok(router) => router,
        Err(e) => {
            eprintln!("invalid ALLOWED_ORIGIN: {e}");
            std::process::exit(1);
        }
    };
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], cfg.port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind failed");
    axum::serve(listener, router).await.expect("serve failed");
}
