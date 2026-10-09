use engine::Candle;
use std::path::Path;

/// Bar length in milliseconds (Binance open time, `ts` is in ms).
pub fn interval_ms(interval: &str) -> Option<i64> {
    match interval {
        "1m" => Some(60_000),
        "5m" => Some(5 * 60_000),
        "15m" => Some(15 * 60_000),
        "1h" => Some(3_600_000),
        "4h" => Some(4 * 3_600_000),
        "1d" => Some(86_400_000),
        _ => None,
    }
}

/// Validate a trading symbol: 2 to 20 characters, ASCII uppercase letters
/// and digits only.
pub fn validate_symbol(symbol: &str) -> Result<(), String> {
    let len = symbol.len();
    if !(2..=20).contains(&len) {
        return Err(format!(
            "invalid symbol: expected 2 to 20 characters, got {len}"
        ));
    }
    if !symbol
        .bytes()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return Err(format!("invalid symbol: {symbol:?}"));
    }
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn latest_closed_open_ts(now_ms: i64, step_ms: i64) -> i64 {
    (now_ms - now_ms.rem_euclid(step_ms)) - step_ms
}

/// Keep only candles that have closed: `ts + step_ms <= now_ms`.
pub fn drop_unclosed(candles: Vec<Candle>, step_ms: i64, now_ms: i64) -> Vec<Candle> {
    candles
        .into_iter()
        .filter(|c| c.ts.saturating_add(step_ms) <= now_ms)
        .collect()
}

/// Return `(previous_ts, next_ts)` for every pair of consecutive candles
/// whose `ts` difference is greater than `step_ms`.
pub fn find_gaps(candles: &[Candle], step_ms: i64) -> Vec<(i64, i64)> {
    let mut gaps = Vec::new();
    for w in candles.windows(2) {
        if w[1].ts.saturating_sub(w[0].ts) > step_ms {
            gaps.push((w[0].ts, w[1].ts));
        }
    }
    gaps
}

fn parse_f64(value: &serde_json::Value, row: usize, field: &str) -> Result<f64, String> {
    match value {
        serde_json::Value::String(s) => s
            .parse::<f64>()
            .map_err(|_| format!("row {row}: invalid {field} number: {s:?}")),
        serde_json::Value::Number(n) => n
            .as_f64()
            .ok_or_else(|| format!("row {row}: invalid {field} number: {n}")),
        other => Err(format!(
            "row {row}: invalid {field}, expected string or number, got {other}"
        )),
    }
}

fn parse_ts(value: &serde_json::Value, row: usize) -> Result<i64, String> {
    match value {
        serde_json::Value::Number(n) => {
            if let Some(v) = n.as_i64() {
                Ok(v)
            } else if let Some(v) = n.as_u64() {
                i64::try_from(v).map_err(|_| format!("row {row}: open time out of range: {v}"))
            } else if let Some(v) = n.as_f64() {
                Ok(v as i64)
            } else {
                Err(format!("row {row}: invalid open time: {n}"))
            }
        }
        serde_json::Value::String(s) => s
            .parse::<i64>()
            .map_err(|_| format!("row {row}: invalid open time: {s:?}")),
        other => Err(format!(
            "row {other_row}: invalid open time, expected number, got {other}",
            other_row = row
        )),
    }
}

/// Parse Binance `GET /api/v3/klines` JSON.
///
/// Expects a JSON array of arrays:
/// `[openTimeMs (number), open, high, low, close, volume, ...]`
/// where `open`/`high`/`low`/`close` are strings.
pub fn parse_klines(json: &str) -> Result<Vec<Candle>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("invalid JSON: {e}"))?;
    let rows = value
        .as_array()
        .ok_or_else(|| "invalid klines: expected top-level JSON array".to_string())?;
    let mut out = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        let cols = row
            .as_array()
            .ok_or_else(|| format!("row {i}: expected JSON array"))?;
        if cols.len() < 5 {
            return Err(format!(
                "row {i}: expected at least 5 fields, got {}",
                cols.len()
            ));
        }
        let ts = parse_ts(&cols[0], i)?;
        let open = parse_f64(&cols[1], i, "open")?;
        let high = parse_f64(&cols[2], i, "high")?;
        let low = parse_f64(&cols[3], i, "low")?;
        let close = parse_f64(&cols[4], i, "close")?;
        out.push(Candle {
            ts,
            open,
            high,
            low,
            close,
        });
    }
    Ok(out)
}

/// Validate candles: strictly ascending `ts`, finite OHLC, and
/// `high >= low`, `high >= max(open, close)`, `low <= min(open, close)`.
pub fn validate_candles(c: &[Candle]) -> Result<(), String> {
    for (i, candle) in c.iter().enumerate() {
        if i > 0 && candle.ts <= c[i - 1].ts {
            return Err(format!(
                "candle {i}: timestamps not strictly ascending ({} <= {})",
                candle.ts,
                c[i - 1].ts
            ));
        }
        for (name, v) in [
            ("open", candle.open),
            ("high", candle.high),
            ("low", candle.low),
            ("close", candle.close),
        ] {
            if !v.is_finite() {
                return Err(format!("candle {i}: {name} is not finite: {v}"));
            }
        }
        if candle.high < candle.low {
            return Err(format!(
                "candle {i}: high ({}) < low ({})",
                candle.high, candle.low
            ));
        }
        let max_oc = candle.open.max(candle.close);
        let min_oc = candle.open.min(candle.close);
        if candle.high < max_oc {
            return Err(format!(
                "candle {i}: high ({}) < max(open, close) ({max_oc})",
                candle.high
            ));
        }
        if candle.low > min_oc {
            return Err(format!(
                "candle {i}: low ({}) > min(open, close) ({min_oc})",
                candle.low
            ));
        }
    }
    Ok(())
}

/// A paged source of klines.
pub trait KlineSource {
    fn fetch(
        &self,
        symbol: &str,
        interval: &str,
        start_ms: i64,
        end_ms: i64,
        limit: usize,
    ) -> Result<Vec<Candle>, String>;
}

/// Binance REST klines source.
pub struct BinanceSource {
    pub base_url: String,
}

impl BinanceSource {
    pub fn new() -> Self {
        Self {
            base_url: "https://api.binance.com".to_string(),
        }
    }
}

impl Default for BinanceSource {
    fn default() -> Self {
        Self::new()
    }
}

impl KlineSource for BinanceSource {
    fn fetch(
        &self,
        symbol: &str,
        interval: &str,
        start_ms: i64,
        end_ms: i64,
        limit: usize,
    ) -> Result<Vec<Candle>, String> {
        let url = format!("{}/api/v3/klines", self.base_url.trim_end_matches('/'));
        let client = reqwest::blocking::Client::new();
        let resp = client
            .get(&url)
            .query(&[
                ("symbol", symbol.to_string()),
                ("interval", interval.to_string()),
                ("startTime", start_ms.to_string()),
                ("endTime", end_ms.to_string()),
                ("limit", limit.to_string()),
            ])
            .send()
            .map_err(|e| format!("request failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            return Err(format!("HTTP {status}: {body}"));
        }
        let text = resp
            .text()
            .map_err(|e| format!("failed to read response body: {e}"))?;
        parse_klines(&text)
    }
}

/// Page through `[start_ms, end_ms]`, dedupe, sort ascending and validate.
pub fn fetch_range(
    source: &dyn KlineSource,
    symbol: &str,
    interval: &str,
    start_ms: i64,
    end_ms: i64,
    page_limit: usize,
) -> Result<Vec<Candle>, String> {
    validate_symbol(symbol)?;
    let step = interval_ms(interval).ok_or_else(|| format!("unknown interval: {interval}"))?;
    let start_ms = start_ms - start_ms.rem_euclid(step);
    if page_limit == 0 {
        return Err("page_limit must be greater than 0".to_string());
    }
    let mut all: Vec<Candle> = Vec::new();
    let mut next_start = start_ms;
    loop {
        let page = source.fetch(symbol, interval, next_start, end_ms, page_limit)?;
        if page.is_empty() {
            break;
        }
        let n = page.len();
        let last_ts = page.last().map(|c| c.ts).unwrap_or(next_start);
        all.extend(page);
        if n < page_limit {
            break;
        }
        if last_ts >= end_ms {
            break;
        }
        let next = last_ts
            .checked_add(step)
            .ok_or_else(|| "timestamp overflow".to_string())?;
        if next <= next_start {
            return Err("source did not advance".to_string());
        }
        next_start = next;
        if all.len() > 1_000_000 {
            return Err("too many candles".to_string());
        }
    }
    let mut filtered: Vec<Candle> = all
        .into_iter()
        .filter(|c| c.ts >= start_ms && c.ts <= end_ms)
        .collect();
    filtered.sort_by_key(|c| c.ts);
    filtered.dedup_by_key(|c| c.ts);
    let filtered = drop_unclosed(filtered, step, now_ms());
    validate_candles(&filtered)?;
    Ok(filtered)
}

fn cache_path(cache_dir: &Path, symbol: &str, interval: &str) -> std::path::PathBuf {
    cache_dir.join(format!("{symbol}_{interval}.csv"))
}

fn read_cache_file(path: &Path) -> Result<Vec<Candle>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("failed to read cache: {e}"))?;
    let mut out = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if idx == 0 {
            continue; // header ts,open,high,low,close
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() != 5 {
            return Err(format!(
                "invalid cache row {}: expected 5 fields, got {}",
                idx,
                parts.len()
            ));
        }
        let ts: i64 = parts[0]
            .parse()
            .map_err(|_| format!("invalid cache ts on row {idx}: {:?}", parts[0]))?;
        let open: f64 = parts[1]
            .parse()
            .map_err(|_| format!("invalid cache open on row {idx}: {:?}", parts[1]))?;
        let high: f64 = parts[2]
            .parse()
            .map_err(|_| format!("invalid cache high on row {idx}: {:?}", parts[2]))?;
        let low: f64 = parts[3]
            .parse()
            .map_err(|_| format!("invalid cache low on row {idx}: {:?}", parts[3]))?;
        let close: f64 = parts[4]
            .parse()
            .map_err(|_| format!("invalid cache close on row {idx}: {:?}", parts[4]))?;
        out.push(Candle {
            ts,
            open,
            high,
            low,
            close,
        });
    }
    out.sort_by_key(|c| c.ts);
    out.dedup_by_key(|c| c.ts);
    Ok(out)
}

fn write_cache_file(path: &Path, candles: &[Candle]) -> Result<(), String> {
    use std::fmt::Write as _;
    let mut text = String::from("ts,open,high,low,close\n");
    for c in candles {
        writeln!(text, "{},{},{},{},{}", c.ts, c.open, c.high, c.low, c.close)
            .map_err(|e| format!("failed to format cache: {e}"))?;
    }
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "cache.csv".to_string());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_name = format!("{file_name}.tmp-{}-{nanos}", std::process::id());
    let tmp_path = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(tmp_name),
        _ => std::path::PathBuf::from(tmp_name),
    };
    if let Err(e) = std::fs::write(&tmp_path, text) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("failed to write cache: {e}"));
    }
    if let Err(e) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("failed to write cache: {e}"));
    }
    Ok(())
}

/// Load `[start_ms, end_ms]` from disk cache or fetch and populate it.
///
/// Cache file is `{cache_dir}/{symbol}_{interval}.csv` with header
/// `ts,open,high,low,close`. If cached data covers the range
/// (`first ts <= start_ms` and `last ts + interval_ms >= end_ms`),
/// the slice within `[start_ms, end_ms]` is returned without calling
/// `source`. Otherwise the full range is fetched with `page_limit` 1000,
/// the cache is overwritten, and the fetched candles are returned.
pub fn load_or_fetch(
    cache_dir: &Path,
    source: &dyn KlineSource,
    symbol: &str,
    interval: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<Vec<Candle>, String> {
    validate_symbol(symbol)?;
    let step = interval_ms(interval).ok_or_else(|| format!("unknown interval: {interval}"))?;
    let start_ms = start_ms - start_ms.rem_euclid(step);
    let effective_end = end_ms.min(
        latest_closed_open_ts(now_ms(), step)
            .saturating_add(step)
            .saturating_sub(1),
    );
    if start_ms > effective_end {
        return Ok(Vec::new());
    }
    std::fs::create_dir_all(cache_dir).map_err(|e| format!("failed to create cache dir: {e}"))?;
    let path = cache_path(cache_dir, symbol, interval);
    if path.exists() {
        if let Ok(cached) = read_cache_file(&path) {
            if !cached.is_empty() {
                let first = cached.first().map(|c| c.ts).unwrap_or(i64::MAX);
                let last = cached.last().map(|c| c.ts).unwrap_or(i64::MIN);
                if first <= start_ms && last.saturating_add(step) >= effective_end {
                    let sliced: Vec<Candle> = cached
                        .into_iter()
                        .filter(|c| c.ts >= start_ms && c.ts <= effective_end)
                        .collect();
                    return Ok(sliced);
                }
            }
        }
    }
    let fetched = fetch_range(source, symbol, interval, start_ms, effective_end, 1000)?;
    if fetched.is_empty() {
        return Ok(fetched);
    }
    write_cache_file(&path, &fetched)?;
    Ok(fetched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct FakeSource {
        data: Vec<Candle>,
        calls: Cell<usize>,
    }

    impl FakeSource {
        fn new(data: Vec<Candle>) -> Self {
            Self {
                data,
                calls: Cell::new(0),
            }
        }
    }

    impl KlineSource for FakeSource {
        fn fetch(
            &self,
            _symbol: &str,
            _interval: &str,
            start_ms: i64,
            end_ms: i64,
            limit: usize,
        ) -> Result<Vec<Candle>, String> {
            self.calls.set(self.calls.get() + 1);
            Ok(self
                .data
                .iter()
                .filter(|c| c.ts >= start_ms && c.ts <= end_ms)
                .take(limit)
                .cloned()
                .collect())
        }
    }

    fn mk_candle(ts: i64, open: f64) -> Candle {
        Candle {
            ts,
            open,
            high: open + 1.0,
            low: open - 1.0,
            close: open + 0.5,
        }
    }

    fn ten_1m_candles() -> Vec<Candle> {
        (0..10)
            .map(|i| mk_candle(i * 60_000, 100.0 + i as f64))
            .collect()
    }

    #[test]
    fn parse_klines_two_rows() {
        let json = r#"[
            [1704067200000,"42000.50","42100.00","41900.25","42050.75","12.345",1704070799999,"1",2,3,4,5],
            [1704070800000,"42050.75","42200.00","42000.00","42150.00","8.765",1704074399999,"1",2,3,4,5]
        ]"#;
        let candles = parse_klines(json).expect("parse should succeed");
        assert_eq!(candles.len(), 2);
        assert_eq!(candles[0].ts, 1_704_067_200_000);
        assert!((candles[0].open - 42000.50).abs() < 1e-9);
        assert!((candles[0].high - 42100.00).abs() < 1e-9);
        assert!((candles[0].low - 41900.25).abs() < 1e-9);
        assert!((candles[0].close - 42050.75).abs() < 1e-9);
        assert_eq!(candles[1].ts, 1_704_070_800_000);
        assert!((candles[1].close - 42150.00).abs() < 1e-9);
    }

    #[test]
    fn parse_klines_rejects_bad_rows() {
        let too_few = r#"[[1704067200000,"42000.50"]]"#;
        assert!(parse_klines(too_few).is_err());

        let bad_price = r#"[[1704067200000,"abc","42100.00","41900.00","42050.00","1.0"]]"#;
        assert!(parse_klines(bad_price).is_err());
    }

    #[test]
    fn fetch_range_pages_ten_candles() {
        let data = ten_1m_candles();
        let src = FakeSource::new(data);
        let out = fetch_range(&src, "BTCUSDT", "1m", 0, 9 * 60_000, 3).expect("fetch");
        assert_eq!(out.len(), 10);
        for w in out.windows(2) {
            assert!(w[0].ts < w[1].ts, "not ascending: {:?} >= {:?}", w[0], w[1]);
        }
        let mut ts: Vec<i64> = out.iter().map(|c| c.ts).collect();
        let before = ts.len();
        ts.sort_unstable();
        ts.dedup();
        assert_eq!(before, ts.len(), "duplicates found");
        assert_eq!(src.calls.get(), 4);
    }

    #[test]
    fn fetch_range_drops_out_of_range_and_rejects_unknown_interval() {
        let data = ten_1m_candles();
        let src = FakeSource::new(data);
        let out = fetch_range(&src, "BTCUSDT", "1m", 2 * 60_000, 5 * 60_000, 10).expect("fetch");
        assert_eq!(out.len(), 4);
        for c in &out {
            assert!(
                c.ts >= 2 * 60_000 && c.ts <= 5 * 60_000,
                "out of range: {}",
                c.ts
            );
        }

        let err = fetch_range(&src, "BTCUSDT", "7m", 0, 60_000, 10).unwrap_err();
        assert!(
            err.contains("7m") || err.to_lowercase().contains("interval"),
            "{err}"
        );
    }

    #[test]
    fn validate_candles_rejects_bad_data() {
        // descending timestamps
        let bad_ts = vec![mk_candle(2000, 100.0), mk_candle(1000, 100.0)];
        assert!(validate_candles(&bad_ts).is_err());

        // NaN
        let bad_nan = vec![Candle {
            ts: 0,
            open: f64::NAN,
            high: 101.0,
            low: 99.0,
            close: 100.5,
        }];
        assert!(validate_candles(&bad_nan).is_err());

        // high < low
        let bad_hl = vec![Candle {
            ts: 0,
            open: 100.0,
            high: 99.0,
            low: 100.0,
            close: 99.5,
        }];
        assert!(validate_candles(&bad_hl).is_err());
    }

    #[test]
    fn load_or_fetch_caches() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "marketdata_test_{}_{}_{}",
            std::process::id(),
            nanos,
            1
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let data = ten_1m_candles();
        let src = FakeSource::new(data);
        let start = 0;
        let end = 9 * 60_000;
        let first = load_or_fetch(&dir, &src, "BTCUSDT", "1m", start, end).expect("first fetch");
        assert_eq!(first.len(), 10);
        assert!(
            dir.join("BTCUSDT_1m.csv").exists(),
            "cache file not written"
        );
        let calls_after_first = src.calls.get();
        assert!(calls_after_first > 0);

        let second = load_or_fetch(&dir, &src, "BTCUSDT", "1m", start, end).expect("second fetch");
        assert_eq!(second, first);
        assert_eq!(
            src.calls.get(),
            calls_after_first,
            "second call should not hit the source"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_symbol_accepts_and_rejects() {
        assert!(validate_symbol("BTCUSDT").is_ok());
        assert!(validate_symbol("SOL2USDT").is_ok());
        assert!(validate_symbol("../../x").is_err());
        assert!(validate_symbol("btcusdt").is_err());
        assert!(validate_symbol("BTC/USDT").is_err());
        assert!(validate_symbol("A").is_err());
        let long = "A".repeat(21);
        assert!(validate_symbol(&long).is_err());
    }

    #[test]
    fn invalid_symbol_does_not_call_source() {
        let data = ten_1m_candles();
        let src = FakeSource::new(data);
        let err = fetch_range(&src, "../x", "1m", 0, 9 * 60_000, 10).unwrap_err();
        assert!(!err.is_empty());
        assert_eq!(src.calls.get(), 0);

        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "marketdata_test_invalid_{}_{}",
            std::process::id(),
            nanos
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let src2 = FakeSource::new(ten_1m_candles());
        let err2 = load_or_fetch(&dir, &src2, "../x", "1m", 0, 9 * 60_000).unwrap_err();
        assert!(!err2.is_empty());
        assert_eq!(src2.calls.get(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fetch_range_aligns_unaligned_start() {
        let data = ten_1m_candles();
        let src_aligned = FakeSource::new(data.clone());
        let src_unaligned = FakeSource::new(data);
        let aligned =
            fetch_range(&src_aligned, "BTCUSDT", "1m", 2 * 60_000, 5 * 60_000, 10).expect("fetch");
        let unaligned = fetch_range(
            &src_unaligned,
            "BTCUSDT",
            "1m",
            2 * 60_000 + 17_000,
            5 * 60_000,
            10,
        )
        .expect("fetch");
        assert_eq!(aligned, unaligned);
    }

    #[test]
    fn load_or_fetch_cache_hit_unaligned_start() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "marketdata_test_unaligned_{}_{}",
            std::process::id(),
            nanos
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let src = FakeSource::new(ten_1m_candles());
        let start = 2 * 60_000 + 17_000;
        let end = 5 * 60_000;
        let first = load_or_fetch(&dir, &src, "BTCUSDT", "1m", start, end).expect("first");
        assert!(!first.is_empty());
        let calls_after_first = src.calls.get();
        assert!(calls_after_first > 0);
        let second = load_or_fetch(&dir, &src, "BTCUSDT", "1m", start, end).expect("second");
        assert_eq!(second, first);
        assert_eq!(
            src.calls.get(),
            calls_after_first,
            "second call should not hit the source"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn drop_unclosed_keeps_closed_and_drops_open() {
        let candles = vec![
            mk_candle(0, 100.0),
            mk_candle(60_000, 101.0),
            mk_candle(120_000, 102.0),
            mk_candle(180_000, 103.0),
        ];
        let kept = drop_unclosed(candles, 60_000, 180_000);
        let ts: Vec<i64> = kept.iter().map(|c| c.ts).collect();
        assert_eq!(ts, vec![0, 60_000, 120_000]);
    }

    #[test]
    fn fetch_range_drops_unclosed_candle() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let step = 60_000;
        let current_ts = now_ms - now_ms.rem_euclid(step);
        let data = vec![mk_candle(0, 100.0), mk_candle(current_ts, 200.0)];
        let src = FakeSource::new(data);
        let out = fetch_range(&src, "BTCUSDT", "1m", 0, current_ts, 10).expect("fetch");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].ts, 0);
    }

    #[test]
    fn find_gaps_detects_missing_candle() {
        let candles: Vec<Candle> = (0..5).map(|i| mk_candle(i * 60_000, 100.0)).collect();
        assert!(find_gaps(&candles, 60_000).is_empty());
        let mut with_hole = candles.clone();
        with_hole.remove(2);
        let gaps = find_gaps(&with_hole, 60_000);
        assert_eq!(gaps, vec![(60_000, 180_000)]);
    }

    #[test]
    fn load_or_fetch_end_now_hits_cache_on_second_call() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let step = 86_400_000_i64;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let latest_closed = (now_ms - now_ms.rem_euclid(step)) - step;
        let data: Vec<Candle> = (0..5)
            .map(|i| mk_candle(latest_closed - (4 - i) * step, 100.0 + i as f64))
            .collect();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "marketdata_test_end_now_{}_{}",
            std::process::id(),
            nanos
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let src = FakeSource::new(data);
        let start = latest_closed - 4 * step;
        let end = now_ms;
        let first = load_or_fetch(&dir, &src, "BTCUSDT", "1d", start, end).expect("first");
        assert_eq!(first.len(), 5);
        let calls_after_first = src.calls.get();
        assert!(calls_after_first > 0);
        let second = load_or_fetch(&dir, &src, "BTCUSDT", "1d", start, end).expect("second");
        assert_eq!(second, first);
        assert_eq!(
            src.calls.get(),
            calls_after_first,
            "second call should not hit the source"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_or_fetch_future_start_returns_empty() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let step = 86_400_000_i64;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let latest_closed = (now_ms - now_ms.rem_euclid(step)) - step;
        let data: Vec<Candle> = (0..5)
            .map(|i| mk_candle(latest_closed - (4 - i) * step, 100.0 + i as f64))
            .collect();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "marketdata_test_future_start_{}_{}",
            std::process::id(),
            nanos
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let src = FakeSource::new(data);
        let start = now_ms + 10 * step;
        let end = start + step;
        let out = load_or_fetch(&dir, &src, "BTCUSDT", "1d", start, end).expect("fetch");
        assert!(out.is_empty());
        assert_eq!(src.calls.get(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_or_fetch_future_end_returns_closed_candles() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let step = 86_400_000_i64;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let latest_closed = (now_ms - now_ms.rem_euclid(step)) - step;
        let data: Vec<Candle> = (0..5)
            .map(|i| mk_candle(latest_closed - (4 - i) * step, 100.0 + i as f64))
            .collect();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "marketdata_test_future_end_{}_{}",
            std::process::id(),
            nanos
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let src = FakeSource::new(data);
        let start = latest_closed - 4 * step;
        let end = now_ms + 10 * step;
        let out = load_or_fetch(&dir, &src, "BTCUSDT", "1d", start, end).expect("fetch");
        assert_eq!(out.len(), 5);
        assert_eq!(out.last().map(|c| c.ts), Some(latest_closed));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_or_fetch_leaves_no_tmp_files() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "marketdata_test_no_tmp_{}_{}",
            std::process::id(),
            nanos
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let src = FakeSource::new(ten_1m_candles());
        let out = load_or_fetch(&dir, &src, "BTCUSDT", "1m", 0, 9 * 60_000).expect("fetch");
        assert_eq!(out.len(), 10);
        assert!(
            dir.join("BTCUSDT_1m.csv").exists(),
            "cache file not written"
        );
        let entries = std::fs::read_dir(&dir).expect("read cache dir");
        for entry in entries {
            let entry = entry.expect("dir entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            assert!(!name.contains("tmp"), "tmp file left behind: {name}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_or_fetch_empty_fetch_keeps_cache_unchanged() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "marketdata_test_empty_keeps_{}_{}",
            std::process::id(),
            nanos
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let src = FakeSource::new(ten_1m_candles());
        let first = load_or_fetch(&dir, &src, "BTCUSDT", "1m", 0, 9 * 60_000).expect("first fetch");
        assert_eq!(first.len(), 10);
        let cache_path = dir.join("BTCUSDT_1m.csv");
        let before = std::fs::read(&cache_path).expect("read cache file");
        let out =
            load_or_fetch(&dir, &src, "BTCUSDT", "1m", -10 * 60_000, -60_000).expect("empty fetch");
        assert!(out.is_empty(), "expected empty vec, got {}", out.len());
        let after = std::fs::read(&cache_path).expect("read cache file again");
        assert_eq!(before, after, "cache file should be unchanged");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore]
    fn live_fetch_btcusdt_1h() {
        let start = 1_704_067_200_000_i64;
        let step = 3_600_000_i64;
        let end = start + 20 * step - 1;
        let primary = BinanceSource::new();
        let candles = match fetch_range(&primary, "BTCUSDT", "1h", start, end, 1000) {
            Ok(c) => c,
            Err(first_err) => {
                let alt = BinanceSource {
                    base_url: "https://data-api.binance.vision".to_string(),
                };
                fetch_range(&alt, "BTCUSDT", "1h", start, end, 1000).unwrap_or_else(|e| {
                    panic!("live fetch failed via both hosts: {first_err} / {e}")
                })
            }
        };
        assert_eq!(
            candles.len(),
            20,
            "expected 20 candles, got {}",
            candles.len()
        );
        validate_candles(&candles).expect("live candles should validate");
    }
}
