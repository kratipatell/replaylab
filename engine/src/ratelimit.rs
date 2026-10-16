use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

// Unique keys over this many are all expired; purge them instead of growing forever.
const MAX_TRACKED_KEYS: usize = 10_000;

pub struct RateLimiter {
    limit: u32,
    window_ms: u64,
    buckets: Mutex<HashMap<String, Bucket>>,
}

struct Bucket {
    count: u32,
    reset_at_ms: u64,
}

impl RateLimiter {
    pub fn per_minute(limit: u32) -> Self {
        Self::new(limit, 60_000)
    }

    fn new(limit: u32, window_ms: u64) -> Self {
        Self {
            limit,
            window_ms: window_ms.max(1),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    // Err carries Retry-After seconds.
    pub fn check(&self, key: &str) -> Result<(), u64> {
        self.check_at(key, now_ms())
    }

    fn check_at(&self, key: &str, now_ms: u64) -> Result<(), u64> {
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        if buckets.len() >= MAX_TRACKED_KEYS {
            buckets.retain(|_, b| b.reset_at_ms > now_ms);
        }
        let bucket = buckets.entry(key.to_string()).or_insert(Bucket {
            count: 0,
            reset_at_ms: now_ms.saturating_add(self.window_ms),
        });
        if now_ms >= bucket.reset_at_ms {
            bucket.count = 0;
            bucket.reset_at_ms = now_ms.saturating_add(self.window_ms);
        }
        if bucket.count >= self.limit {
            let wait_ms = bucket.reset_at_ms.saturating_sub(now_ms);
            return Err(wait_ms.div_ceil(1000).max(1));
        }
        bucket.count += 1;
        Ok(())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_then_blocks_then_recovers() {
        let limiter = RateLimiter::new(2, 60_000);
        assert_eq!(limiter.check_at("ip", 0), Ok(()));
        assert_eq!(limiter.check_at("ip", 1), Ok(()));
        assert_eq!(limiter.check_at("ip", 2), Err(60));
        assert_eq!(limiter.check_at("ip", 59_999), Err(1));
        assert_eq!(limiter.check_at("ip", 60_000), Ok(()));
    }

    #[test]
    fn separate_keys_do_not_share_buckets() {
        let limiter = RateLimiter::new(1, 60_000);
        assert_eq!(limiter.check_at("a", 0), Ok(()));
        assert_eq!(limiter.check_at("b", 0), Ok(()));
        assert!(limiter.check_at("a", 1).is_err());
        assert!(limiter.check_at("b", 1).is_err());
    }
}
