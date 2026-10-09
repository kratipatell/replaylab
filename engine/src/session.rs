/// UTC trading-session helpers (no timezone database; all in UTC).
///
/// `ts` values are milliseconds since the Unix epoch like [`crate::types::Candle::ts`].
pub const MS_PER_DAY: i64 = 86_400_000;
pub const MS_PER_MIN: i64 = 60_000;

/// Whole-day id for `ts_ms` (floored, works for negative timestamps).
pub fn day_id(ts_ms: i64) -> i64 {
    ts_ms.div_euclid(MS_PER_DAY)
}

/// Minutes since UTC midnight in `[0, 1440)`.
pub fn mins_since_midnight_utc(ts_ms: i64) -> u16 {
    (ts_ms.rem_euclid(MS_PER_DAY) / MS_PER_MIN) as u16
}

/// True when `ts` and `prev_ts` fall on different UTC days.
pub fn is_new_day(prev_ts: i64, ts: i64) -> bool {
    day_id(prev_ts) != day_id(ts)
}

/// Intraday session window in UTC minutes: `[open_min_utc, close_min_utc)`.
///
/// When `open_min_utc > close_min_utc` the window spans midnight
/// (e.g. 22:00 -> 06:00).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionFilter {
    pub open_min_utc: u16,
    pub close_min_utc: u16,
}

impl SessionFilter {
    pub fn new(open_min_utc: u16, close_min_utc: u16) -> Result<SessionFilter, String> {
        if open_min_utc >= 24 * 60 || close_min_utc > 24 * 60 {
            return Err(format!(
                "session minutes must be within a day, got {open_min_utc}..{close_min_utc}"
            ));
        }
        if open_min_utc == close_min_utc {
            return Err("session open == close would always be empty".to_string());
        }
        Ok(SessionFilter {
            open_min_utc,
            close_min_utc,
        })
    }

    /// Accept every bar (no filtering).
    pub fn all_day() -> SessionFilter {
        SessionFilter {
            open_min_utc: 0,
            close_min_utc: 24 * 60,
        }
    }

    pub fn contains(&self, ts_ms: i64) -> bool {
        let m = mins_since_midnight_utc(ts_ms);
        if self.open_min_utc < self.close_min_utc {
            m >= self.open_min_utc && m < self.close_min_utc
        } else {
            m >= self.open_min_utc || m < self.close_min_utc
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_split_and_minutes() {
        // 1970-01-01T00:00Z = 0, 1970-01-01T01:30Z = 90 min.
        assert_eq!(day_id(0), 0);
        assert_eq!(day_id(MS_PER_DAY), 1);
        assert_eq!(mins_since_midnight_utc(90 * MS_PER_MIN), 90);
        assert!(is_new_day(MS_PER_DAY - 1, MS_PER_DAY));
        assert!(!is_new_day(0, 1000));
    }

    #[test]
    fn session_contains_and_overnight() {
        let day = SessionFilter::new(9 * 60 + 30, 16 * 60).expect("valid");
        assert!(day.contains((9 * 60 + 30) * MS_PER_MIN));
        assert!(!day.contains(9 * 60 * MS_PER_MIN));
        assert!(!day.contains(16 * 60 * MS_PER_MIN));

        let night = SessionFilter::new(22 * 60, 6 * 60).expect("valid");
        assert!(night.contains(23 * 60 * MS_PER_MIN));
        assert!(night.contains(5 * 60 * MS_PER_MIN));
        assert!(!night.contains(12 * 60 * MS_PER_MIN));
    }

    #[test]
    fn rejects_empty_session() {
        assert!(SessionFilter::new(60, 60).is_err());
        assert!(SessionFilter::new(24 * 60, 100).is_err());
    }
}
