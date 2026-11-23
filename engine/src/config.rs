use std::collections::HashSet;
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
    pub backtest_timeout_secs: u64,
    pub admin_token: Option<String>,
    pub demo_mode: bool,
    pub prewarm: Vec<crate::feeds::PrewarmSpec>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid ALLOWED_ORIGINS: {0}")]
    BadOrigins(String),
    #[error(
        "invalid PREWARM entry {0:?}; expected SYMBOL:interval:years with at most 100000 candles"
    )]
    BadPrewarm(String),
}

pub fn parse_origins(raw: &str) -> Result<Vec<HeaderValue>, String> {
    raw.split(',')
        .map(str::trim)
        .map(|part| {
            if part.is_empty() {
                return Err(format!("empty origin in {raw:?}"));
            }
            if part.contains('*') {
                return Err("wildcard origins are not allowed; list explicit origins".to_string());
            }
            part.parse()
                .map_err(|e: axum::http::header::InvalidHeaderValue| e.to_string())
        })
        .collect()
}

fn parse_or<T: std::str::FromStr>(raw: Option<String>, default: T) -> T {
    raw.and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

fn parse_u64_in(raw: Option<String>, default: u64, min: u64, max: u64) -> u64 {
    raw.and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| (min..=max).contains(v))
        .unwrap_or(default)
}

fn parse_bool(raw: Option<String>, default: bool) -> bool {
    match raw
        .as_deref()
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("true" | "1" | "yes" | "on") => true,
        Some("false" | "0" | "no" | "off") => false,
        _ => default,
    }
}

fn parse_prewarm(raw: Option<String>) -> Result<Vec<crate::feeds::PrewarmSpec>, ConfigError> {
    let Some(raw) = raw.filter(|value| !value.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    let mut seen = HashSet::new();
    let mut specs = Vec::new();
    for entry in raw.split(',').map(str::trim) {
        let mut fields = entry.split(':').map(str::trim);
        let symbol = fields.next().unwrap_or_default();
        let interval = fields.next().unwrap_or_default();
        let years = fields.next().and_then(|value| value.parse::<u32>().ok());
        if fields.next().is_some()
            || crate::feeds::validate_symbol(symbol).is_err()
            || crate::feeds::interval_ms(interval).is_none()
        {
            return Err(ConfigError::BadPrewarm(entry.to_string()));
        }
        let Some(years) = years.filter(|years| *years > 0) else {
            return Err(ConfigError::BadPrewarm(entry.to_string()));
        };
        let step = crate::feeds::interval_ms(interval).unwrap_or_default() as u128;
        let candles = years as u128 * 365 * 24 * 60 * 60 * 1_000 / step;
        if candles > 100_000 {
            return Err(ConfigError::BadPrewarm(entry.to_string()));
        }
        let spec = crate::feeds::PrewarmSpec {
            symbol: symbol.to_string(),
            interval: interval.to_string(),
            years,
        };
        if seen.insert(spec.clone()) {
            specs.push(spec);
        }
    }
    Ok(specs)
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
        let base_url = get("BINANCE_BASE_URL").filter(|value| !value.trim().is_empty());
        let prewarm = parse_prewarm(get("PREWARM"))?;
        let origins_raw =
            get("ALLOWED_ORIGINS").unwrap_or_else(|| "http://localhost:3000".to_string());
        parse_origins(&origins_raw).map_err(ConfigError::BadOrigins)?;
        let admin_token = get("ADMIN_TOKEN")
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
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
            backtest_timeout_secs: parse_u64_in(get("BACKTEST_TIMEOUT_SECS"), 60, 5, 600),
            admin_token,
            demo_mode: parse_bool(get("DEMO_MODE"), false),
            prewarm,
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
        assert_eq!(cfg.admin_token, None);
        assert!(!cfg.demo_mode);
        assert!(cfg.prewarm.is_empty());
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
            ("ADMIN_TOKEN", "s3cret"),
            ("DEMO_MODE", "true"),
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
        assert_eq!(cfg.admin_token.as_deref(), Some("s3cret"));
        assert!(cfg.demo_mode);
    }

    #[test]
    fn a_blank_admin_token_means_no_admin_access() {
        for raw in ["", "   "] {
            let cfg = Config::from_lookup(lookup(&[("ADMIN_TOKEN", raw)])).expect("blank");
            assert_eq!(cfg.admin_token, None, "raw = {raw:?}");
        }
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
    fn wildcard_origin_is_rejected() {
        for origin in ["*", "https://*.example.com"] {
            let cfg = Config::from_lookup(lookup(&[("ALLOWED_ORIGINS", origin)]));
            assert!(matches!(cfg, Err(ConfigError::BadOrigins(_))), "{origin}");
        }
    }

    #[test]
    fn invalid_demo_mode_fails_closed() {
        for raw in ["", "maybe", "enabled"] {
            let cfg = Config::from_lookup(lookup(&[("DEMO_MODE", raw)])).expect("defaults");
            assert!(!cfg.demo_mode, "raw = {raw:?}");
        }
    }

    #[test]
    fn parses_and_deduplicates_prewarm_entries() {
        let cfg = Config::from_lookup(lookup(&[(
            "PREWARM",
            "BTCUSDT:1h:5, ETHUSDT:4h:2, BTCUSDT:1h:5",
        )]))
        .expect("valid prewarm list");
        assert_eq!(
            cfg.prewarm,
            vec![
                crate::feeds::PrewarmSpec {
                    symbol: "BTCUSDT".to_string(),
                    interval: "1h".to_string(),
                    years: 5,
                },
                crate::feeds::PrewarmSpec {
                    symbol: "ETHUSDT".to_string(),
                    interval: "4h".to_string(),
                    years: 2,
                },
            ]
        );
    }

    #[test]
    fn rejects_invalid_or_oversized_prewarm_entries() {
        for raw in [
            "BTCUSDT",
            "btc:1h:1",
            "BTCUSDT:2h:1",
            "BTCUSDT:1m:1",
            "BTCUSDT:1h:0",
            "BTCUSDT:1h:many",
        ] {
            let cfg = Config::from_lookup(lookup(&[("PREWARM", raw)]));
            assert!(matches!(cfg, Err(ConfigError::BadPrewarm(_))), "{raw}");
        }
    }

    #[test]
    fn backtest_timeout_defaults_valid_and_out_of_range() {
        let default = Config::from_lookup(|_| None).expect("defaults");
        assert_eq!(default.backtest_timeout_secs, 60);

        for (raw, expected) in [("5", 5), ("120", 120), ("600", 600)] {
            let cfg = Config::from_lookup(lookup(&[("BACKTEST_TIMEOUT_SECS", raw)])).expect("set");
            assert_eq!(cfg.backtest_timeout_secs, expected, "raw = {raw:?}");
        }

        for raw in ["4", "601", "abc", ""] {
            let cfg =
                Config::from_lookup(lookup(&[("BACKTEST_TIMEOUT_SECS", raw)])).expect("fallback");
            assert_eq!(cfg.backtest_timeout_secs, 60, "raw = {raw:?}");
        }
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
