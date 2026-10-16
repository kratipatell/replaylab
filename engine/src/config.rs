use std::path::PathBuf;

use axum::http::HeaderValue;

pub struct Config {
    pub host: String,
    pub port: u16,
    pub cache_dir: PathBuf,
    pub db_path: PathBuf,
    pub base_url: Option<String>,
    pub origins: String,
    pub backtest_per_min: u32,
    pub optimize_per_min: u32,
    pub max_optimizer_jobs: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid ALLOWED_ORIGINS: {0}")]
    BadOrigins(String),
}

pub fn parse_origins(raw: &str) -> Result<Vec<HeaderValue>, String> {
    raw.split(',')
        .map(str::trim)
        .map(|part| {
            if part.is_empty() {
                return Err(format!("empty origin in {raw:?}"));
            }
            part.parse()
                .map_err(|e: axum::http::header::InvalidHeaderValue| e.to_string())
        })
        .collect()
}

fn parse_or<T: std::str::FromStr>(raw: Option<String>, default: T) -> T {
    raw.and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

impl Config {
    pub fn from_env() -> Result<Config, ConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Config, ConfigError> {
        let host = get("HOST").unwrap_or_else(|| "127.0.0.1".to_string());
        let port = parse_or(get("PORT"), 8080);
        let cache_dir = PathBuf::from(get("CACHE_DIR").unwrap_or_else(|| ".cache".to_string()));
        let db_path = get("DB_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| cache_dir.join("runs.db"));
        let base_url = get("BINANCE_BASE_URL");
        let origins_raw =
            get("ALLOWED_ORIGINS").unwrap_or_else(|| "http://localhost:3000".to_string());
        parse_origins(&origins_raw).map_err(ConfigError::BadOrigins)?;
        Ok(Config {
            host,
            port,
            cache_dir,
            db_path,
            base_url,
            origins: origins_raw,
            backtest_per_min: parse_or(get("BACKTEST_RATE_PER_MIN"), 30),
            optimize_per_min: parse_or(get("OPTIMIZE_RATE_PER_MIN"), 5),
            max_optimizer_jobs: parse_or(get("MAX_OPTIMIZER_JOBS"), 2),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn defaults_when_env_is_empty() {
        let cfg = Config::from_lookup(|_| None).expect("defaults");
        assert_eq!(cfg.host, "127.0.0.1");
        assert_eq!(cfg.port, 8080);
        assert_eq!(cfg.cache_dir, PathBuf::from(".cache"));
        assert_eq!(cfg.db_path, PathBuf::from(".cache/runs.db"));
        assert_eq!(cfg.base_url, None);
        assert_eq!(cfg.origins, "http://localhost:3000");
        assert_eq!(cfg.backtest_per_min, 30);
        assert_eq!(cfg.optimize_per_min, 5);
        assert_eq!(cfg.max_optimizer_jobs, 2);
    }

    #[test]
    fn reads_overrides() {
        let cfg = Config::from_lookup(lookup(&[
            ("HOST", "0.0.0.0"),
            ("PORT", "9090"),
            ("CACHE_DIR", "/tmp/cache"),
            ("DB_PATH", "/tmp/runs.sqlite"),
            ("BINANCE_BASE_URL", "https://mirror.example"),
            ("ALLOWED_ORIGINS", "http://a.example, https://b.example"),
            ("BACKTEST_RATE_PER_MIN", "7"),
            ("OPTIMIZE_RATE_PER_MIN", "3"),
            ("MAX_OPTIMIZER_JOBS", "4"),
        ]))
        .expect("overrides");
        assert_eq!(cfg.host, "0.0.0.0");
        assert_eq!(cfg.port, 9090);
        assert_eq!(cfg.cache_dir, PathBuf::from("/tmp/cache"));
        assert_eq!(cfg.db_path, PathBuf::from("/tmp/runs.sqlite"));
        assert_eq!(cfg.base_url.as_deref(), Some("https://mirror.example"));
        assert_eq!(cfg.origins, "http://a.example, https://b.example");
        assert_eq!(cfg.backtest_per_min, 7);
        assert_eq!(cfg.optimize_per_min, 3);
        assert_eq!(cfg.max_optimizer_jobs, 4);
    }

    #[test]
    fn parse_origins_splits_and_trims() {
        let origins = parse_origins("http://a.example, https://b.example").expect("origins");
        assert_eq!(
            origins,
            vec![
                HeaderValue::from_static("http://a.example"),
                HeaderValue::from_static("https://b.example"),
            ]
        );
    }

    #[test]
    fn bad_origin_is_rejected() {
        let cfg = Config::from_lookup(lookup(&[(
            "ALLOWED_ORIGINS",
            "http://ok.example,bad\norigin",
        )]));
        assert!(matches!(cfg, Err(ConfigError::BadOrigins(_))));
    }

    #[test]
    fn empty_origin_is_rejected() {
        let cfg = Config::from_lookup(lookup(&[("ALLOWED_ORIGINS", "http://ok.example,")]));
        assert!(cfg.is_err());
    }

    #[test]
    fn bad_numbers_fall_back_to_defaults() {
        let cfg = Config::from_lookup(lookup(&[
            ("PORT", "abc"),
            ("BACKTEST_RATE_PER_MIN", "-3"),
            ("MAX_OPTIMIZER_JOBS", "many"),
        ]))
        .expect("defaults");
        assert_eq!(cfg.port, 8080);
        assert_eq!(cfg.backtest_per_min, 30);
        assert_eq!(cfg.max_optimizer_jobs, 2);
    }
}
