mod agent;
mod api;
mod backtest;
mod config;
mod db;
mod demo;
mod feeds;
mod indicators;
mod instrument;
mod optimizer;
mod orders;
mod ratelimit;
mod risk;
mod session;
mod sizing;
mod stats;
mod strategy;
mod templates;
mod types;
mod validation;
mod verdict;
mod walk_forward;

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
    let base_url = cfg
        .base_url
        .as_deref()
        .unwrap_or(feeds::DEFAULT_BINANCE_BASE_URL);
    println!(
        "{}",
        serde_json::json!({
            "event": "market_data_source",
            "host": feeds::base_host(base_url),
        })
    );
    let db = match crate::db::Db::open(&cfg.db_path) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    if cfg.demo_mode {
        match crate::demo::seed_demo_agents(&db) {
            Ok(seeded) => println!(
                "{}",
                serde_json::json!({"event": "demo_seed_complete", "agents_seeded": seeded})
            ),
            Err(e) => {
                eprintln!("could not seed demo agents: {e}");
                std::process::exit(1);
            }
        }
    }
    let prewarm_done =
        feeds::spawn_prewarm(cfg.cache_dir.clone(), source.clone(), cfg.prewarm.clone());
    let state = AppState {
        source,
        cache_dir: cfg.cache_dir.clone(),
        jobs: Arc::new(JobStore::default()),
        db: Arc::new(db),
        admin_token: cfg.admin_token.clone(),
    };
    let router = match app_with_limits(
        state,
        &cfg.origins,
        Limits::new(
            cfg.backtest_per_min,
            cfg.optimize_per_min,
            cfg.max_optimizer_jobs,
        )
        .with_backtest_timeout(cfg.backtest_timeout_secs)
        .with_demo_mode(cfg.demo_mode)
        .with_prewarm_done(prewarm_done),
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
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("serve failed");
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("register SIGTERM handler");
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    eprintln!("could not listen for Ctrl-C: {error}");
                }
            }
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    if let Err(error) = tokio::signal::ctrl_c().await {
        eprintln!("could not listen for Ctrl-C: {error}");
    }
    println!(
        "{}",
        serde_json::json!({"event": "shutdown_signal_received"})
    );
}
