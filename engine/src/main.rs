// Temporary: allow dead code while workspace is restructured into one binary crate.
#![allow(dead_code)]

mod api;
mod backtest;
mod backtest_v2;
mod config;
mod db;
mod feeds;
mod indicators;
mod instrument;
mod optimizer;
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
    let db_path = std::env::var("RUNS_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| cfg.cache_dir.join("runs.db"));
    let db = match crate::db::Db::open(&db_path) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let web_dir = std::env::var("WEB_DIR").unwrap_or_else(|_| "web".to_string());
    let state = AppState {
        source,
        cache_dir: cfg.cache_dir,
        jobs: std::sync::Arc::new(crate::api::JobStore::default()),
        db: std::sync::Arc::new(db),
        web_dir: std::path::PathBuf::from(web_dir),
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
