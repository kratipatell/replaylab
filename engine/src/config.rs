use std::path::PathBuf;

pub struct Config {
    pub port: u16,
    pub cache_dir: PathBuf,
    pub base_url: Option<String>,
    pub origin: String,
}

impl Config {
    pub fn from_env() -> Result<Config, String> {
        let port: u16 = std::env::var("PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8080);
        let cache_dir =
            PathBuf::from(std::env::var("CACHE_DIR").unwrap_or_else(|_| ".cache".to_string()));
        let base_url = std::env::var("BINANCE_BASE_URL").ok();
        let origin =
            std::env::var("ALLOWED_ORIGIN").unwrap_or_else(|_| "http://localhost:3000".to_string());
        Ok(Config {
            port,
            cache_dir,
            base_url,
            origin,
        })
    }
}
