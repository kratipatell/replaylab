use crate::session::day_id;
use crate::strategy::Side;
use crate::types::Candle;

/// Bar-by-bar signal interface for Engine v2.
///
/// The legacy JSON rule engine (`crate::strategy::Strategy`) stays for the
/// HTTP API; new strategies implement this trait and run in
/// `crate::backtest_v2`. Signals fire at the **close** of bar `idx` and are
/// filled at the open of `idx + 1` (same convention as v1).
pub trait Strategy {
    /// Return `Some(side)` to enter at the next open, `None` to stay flat.
    ///
    /// `candles[..=idx]` is visible; future bars must not be read.
    fn on_bar(&mut self, idx: usize, candles: &[Candle]) -> Option<Side>;

    /// Called by the runner after a fill so one-trade-per-day strategies
    /// can suppress further signals. Default does nothing.
    fn on_fill(&mut self, _idx: usize) {}
}

/// Opening-range breakout configuration.
///
/// The range is the high/low of the first `range_bars` bars of each UTC day.
/// After the range forms, a close above `high * (1 + buffer)` signals long,
/// a close below `low * (1 - buffer)` signals short.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbConfig {
    pub range_bars: usize,
    pub buffer_pct: f64,
    pub max_trades_per_day: usize,
}

impl OrbConfig {
    pub fn new(
        range_bars: usize,
        buffer_pct: f64,
        max_trades_per_day: usize,
    ) -> Result<OrbConfig, String> {
        if range_bars == 0 {
            return Err("range_bars must be > 0".to_string());
        }
        if !buffer_pct.is_finite() || buffer_pct < 0.0 {
            return Err(format!("buffer_pct must be >= 0, got {buffer_pct}"));
        }
        if max_trades_per_day == 0 {
            return Err("max_trades_per_day must be > 0".to_string());
        }
        Ok(OrbConfig {
            range_bars,
            buffer_pct,
            max_trades_per_day,
        })
    }
}

/// Opening-range breakout strategy.
#[derive(Debug, Clone)]
pub struct Orb {
    cfg: OrbConfig,
    day: Option<i64>,
    day_count: usize,
    range_high: f64,
    range_low: f64,
    range_ready: bool,
    traded: usize,
}

impl Orb {
    pub fn new(cfg: OrbConfig) -> Orb {
        Orb {
            cfg,
            day: None,
            day_count: 0,
            range_high: f64::NEG_INFINITY,
            range_low: f64::INFINITY,
            range_ready: false,
            traded: 0,
        }
    }

    /// Current day's range if formed: `(high, low)`.
    pub fn range(&self) -> Option<(f64, f64)> {
        if self.range_ready {
            Some((self.range_high, self.range_low))
        } else {
            None
        }
    }
}

impl Strategy for Orb {
    fn on_bar(&mut self, idx: usize, candles: &[Candle]) -> Option<Side> {
        if idx >= candles.len() {
            return None;
        }
        let day = day_id(candles[idx].ts);
        if self.day != Some(day) {
            self.day = Some(day);
            self.day_count = 0;
            self.range_high = f64::NEG_INFINITY;
            self.range_low = f64::INFINITY;
            self.range_ready = false;
            self.traded = 0;
        }
        self.day_count += 1;
        if !self.range_ready {
            let c = &candles[idx];
            if c.high > self.range_high {
                self.range_high = c.high;
            }
            if c.low < self.range_low {
                self.range_low = c.low;
            }
            if self.day_count >= self.cfg.range_bars {
                self.range_ready = true;
            }
            return None;
        }
        if self.traded >= self.cfg.max_trades_per_day {
            return None;
        }
        let c = &candles[idx];
        let up = self.range_high * (1.0 + self.cfg.buffer_pct / 100.0);
        let dn = self.range_low * (1.0 - self.cfg.buffer_pct / 100.0);
        if c.close > up {
            Some(Side::Long)
        } else if c.close < dn {
            Some(Side::Short)
        } else {
            None
        }
    }

    fn on_fill(&mut self, _idx: usize) {
        self.traded += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::MS_PER_MIN;

    fn c(ts: i64, o: f64, h: f64, l: f64, cl: f64) -> Candle {
        Candle {
            ts,
            open: o,
            high: h,
            low: l,
            close: cl,
        }
    }

    fn day_bars() -> Vec<Candle> {
        // One UTC day, 1-minute bars from 00:00.
        vec![
            c(0, 100.0, 101.0, 99.0, 100.0),
            c(MS_PER_MIN, 100.0, 102.0, 99.5, 101.0),
            c(2 * MS_PER_MIN, 101.0, 103.0, 100.0, 102.5),
            c(3 * MS_PER_MIN, 102.5, 103.0, 101.0, 98.0),
        ]
    }

    #[test]
    fn orb_range_and_long_breakout() {
        let cfg = OrbConfig::new(2, 0.0, 1).expect("valid");
        let mut orb = Orb::new(cfg);
        let bars = day_bars();
        assert_eq!(orb.on_bar(0, &bars), None);
        assert_eq!(orb.on_bar(1, &bars), None);
        // Range = high 102, low 99.
        assert_eq!(orb.range(), Some((102.0, 99.0)));
        // Bar 2 close 102.5 > 102 -> long.
        assert_eq!(orb.on_bar(2, &bars), Some(Side::Long));
        orb.on_fill(3);
        // One per day: suppressed afterwards even though bar 3 breaks down.
        assert_eq!(orb.on_bar(3, &bars), None);
    }

    #[test]
    fn orb_short_breakout_and_new_day_resets() {
        let cfg = OrbConfig::new(1, 0.0, 1).expect("valid");
        let mut orb = Orb::new(cfg);
        let mut bars = day_bars();
        assert_eq!(orb.on_bar(0, &bars), None);
        assert_eq!(orb.range(), Some((101.0, 99.0)));
        // Bar 1 close 101 is inside -> none.
        assert_eq!(orb.on_bar(1, &bars), None);
        // Next UTC day resets the range.
        bars.push(c(crate::session::MS_PER_DAY, 50.0, 51.0, 49.0, 50.0));
        let idx = bars.len() - 1;
        assert_eq!(orb.on_bar(idx, &bars), None);
        assert_eq!(orb.range(), Some((51.0, 49.0)));
    }

    #[test]
    fn orb_rejects_bad_config() {
        assert!(OrbConfig::new(0, 0.0, 1).is_err());
        assert!(OrbConfig::new(2, -1.0, 1).is_err());
        assert!(OrbConfig::new(2, 0.0, 0).is_err());
    }
}
