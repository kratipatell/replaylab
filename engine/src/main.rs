mod api;
mod backtest;
mod config;
mod db;
mod feeds;
mod indicators;
mod instrument;
mod optimizer;
mod ratelimit;
mod session;
mod sizing;
mod stats;
mod strategy;
mod types;
mod verdict;

use std::sync::Arc;

use crate::api::{app_with_limits, AppState, JobStore, Limits};
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
    let source: Arc<dyn feeds::KlineSource + Send + Sync> = match &cfg.base_url {
        Some(url) => Arc::new(feeds::BinanceSource {
            base_url: url.clone(),
        }),
        None => Arc::new(feeds::BinanceSource::new()),
    };
    let db = match crate::db::Db::open(&cfg.db_path) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let state = AppState {
        source,
        cache_dir: cfg.cache_dir.clone(),
        jobs: Arc::new(JobStore::default()),
        db: Arc::new(db),
    };
    let router = match app_with_limits(
        state,
        &cfg.origins,
        Limits::new(
            cfg.backtest_per_min,
            cfg.optimize_per_min,
            cfg.max_optimizer_jobs,
        ),
    ) {
        Ok(router) => router,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let addr: std::net::SocketAddr = match cfg.host.parse::<std::net::IpAddr>() {
        Ok(ip) => std::net::SocketAddr::new(ip, cfg.port),
        Err(e) => {
            eprintln!("invalid HOST (use an IP address): {e}");
            std::process::exit(1);
        }
    };
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind failed");
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .expect("serve failed");
}
