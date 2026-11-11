use serde::{Deserialize, Serialize};

use crate::session::day_id;

/// Server-side caps for one agent, or an opt-in backtest.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskLimits {
    pub max_position_pct: f64,
    pub max_daily_loss_pct: f64,
    pub max_drawdown_pct: f64,
}

impl Default for RiskLimits {
    /// Caps an agent gets when it is created without a risk block. A backtest
    /// with no risk block enforces nothing, so it never reaches this default.
    fn default() -> Self {
        Self {
            max_position_pct: 100.0,
            max_daily_loss_pct: 10.0,
            max_drawdown_pct: 20.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum RiskError {
    #[error("{0} must be > 0 and <= 100, got {1}")]
    Pct(&'static str, f64),
}

impl RiskLimits {
    pub fn validate(&self) -> Result<(), RiskError> {
        for (name, v) in [
            ("max_position_pct", self.max_position_pct),
            ("max_daily_loss_pct", self.max_daily_loss_pct),
            ("max_drawdown_pct", self.max_drawdown_pct),
        ] {
            if !v.is_finite() || v <= 0.0 || v > 100.0 {
                return Err(RiskError::Pct(name, v));
            }
        }
        Ok(())
    }
}

/// The limit that stopped an agent. Persisted, so a restart keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HaltLimit {
    DailyLoss,
    Drawdown,
    KillSwitch,
}

/// Running risk state. Peaks and the day baseline are taken at bar close,
/// which is the equity the limits are measured against.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RiskState {
    peak: f64,
    day_start: f64,
    day: i64,
    halt: Option<HaltLimit>,
}

impl Default for RiskState {
    fn default() -> Self {
        // Only reached by a snapshot written before risk state existed; the
        // first observed bar re-baselines it.
        Self::starting(0.0)
    }
}

impl RiskState {
    pub fn starting(equity: f64) -> Self {
        Self {
            peak: equity,
            day_start: equity,
            day: i64::MIN,
            halt: None,
        }
    }

    pub fn new(equity: f64, ts: i64) -> Self {
        Self {
            day: day_id(ts),
            ..Self::starting(equity)
        }
    }

    #[cfg(test)]
    pub fn peak(&self) -> f64 {
        self.peak
    }

    #[cfg(test)]
    pub fn day_start(&self) -> f64 {
        self.day_start
    }

    #[cfg(test)]
    pub fn day(&self) -> i64 {
        self.day
    }

    pub fn halt(&self) -> Option<HaltLimit> {
        self.halt
    }

    pub fn is_halted(&self) -> bool {
        self.halt.is_some()
    }

    /// Forget every past breach: the next day and the next drawdown are both
    /// measured from `equity`.
    pub fn reset(&mut self, equity: f64, ts: i64) {
        *self = Self::new(equity, ts);
    }

    pub fn force_halt(&mut self, limit: HaltLimit) {
        self.halt = Some(limit);
    }

    pub fn clear_kill_switch(&mut self) {
        if self.halt == Some(HaltLimit::KillSwitch) {
            self.halt = None;
        }
    }

    /// Fold one bar close in and return the limit that halted this bar.
    ///
    /// `prev_close` is the equity at the close of the previous bar, which is
    /// the baseline a new UTC day starts from.
    pub fn observe(
        &mut self,
        limits: &RiskLimits,
        ts: i64,
        equity: f64,
        prev_close: f64,
    ) -> Option<HaltLimit> {
        self.peak = self.peak.max(equity);
        let day = day_id(ts);
        if day != self.day {
            self.day = day;
            self.day_start = prev_close;
            if self.halt == Some(HaltLimit::DailyLoss) {
                self.halt = None;
            }
        }
        if self.halt.is_some() {
            return None;
        }
        let tripped = if pct_below(self.peak, equity) >= limits.max_drawdown_pct {
            Some(HaltLimit::Drawdown)
        } else if pct_below(self.day_start, equity) >= limits.max_daily_loss_pct {
            Some(HaltLimit::DailyLoss)
        } else {
            None
        };
        self.halt = tripped;
        tripped
    }
}

fn pct_below(start: f64, equity: f64) -> f64 {
    if start <= 0.0 {
        return 0.0;
    }
    100.0 * (start - equity) / start
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{MS_PER_DAY, MS_PER_MIN};

    fn limits() -> RiskLimits {
        RiskLimits {
            max_position_pct: 50.0,
            max_daily_loss_pct: 10.0,
            max_drawdown_pct: 20.0,
        }
    }

    #[test]
    fn drawdown_breach_halts_exactly_at_the_limit() {
        let mut state = RiskState::new(10_000.0, 0);
        assert_eq!(state.observe(&limits(), 0, 10_000.0, 10_000.0), None);
        // 9.99% below the peak: inside both the daily and the drawdown cap.
        assert_eq!(
            state.observe(&limits(), MS_PER_MIN, 9_001.0, 10_000.0),
            None
        );
        assert!(!state.is_halted());
        // Exactly 20% is a breach: the limit is inclusive. The drawdown is
        // checked first, so it names the halt even though the day is also down.
        assert_eq!(
            state.observe(&limits(), 2 * MS_PER_MIN, 8_000.0, 9_001.0),
            Some(HaltLimit::Drawdown)
        );
        assert_eq!(state.halt(), Some(HaltLimit::Drawdown));
    }

    #[test]
    fn drawdown_uses_the_highest_close_not_the_first() {
        let mut state = RiskState::new(10_000.0, 0);
        state.observe(&limits(), 0, 10_000.0, 10_000.0);
        state.observe(&limits(), MS_PER_MIN, 12_000.0, 10_000.0);
        assert_eq!(state.peak(), 12_000.0);
        // 8% below the first close but 23% below the 12k peak.
        assert_eq!(
            state.observe(&limits(), 2 * MS_PER_MIN, 9_240.0, 12_000.0),
            Some(HaltLimit::Drawdown)
        );
    }

    #[test]
    fn daily_loss_breach_halts_exactly_at_the_limit() {
        let mut state = RiskState::new(10_000.0, 0);
        assert_eq!(state.observe(&limits(), 0, 10_000.0, 10_000.0), None);
        assert_eq!(
            state.observe(&limits(), MS_PER_MIN, 9_001.0, 10_000.0),
            None
        );
        assert_eq!(
            state.observe(&limits(), 2 * MS_PER_MIN, 9_000.0, 9_001.0),
            Some(HaltLimit::DailyLoss)
        );
    }

    #[test]
    fn daily_loss_measures_from_the_previous_days_last_close() {
        let mut state = RiskState::new(10_000.0, 0);
        let before_midnight = MS_PER_DAY - MS_PER_MIN;
        state.observe(&limits(), 0, 10_000.0, 10_000.0);
        state.observe(&limits(), before_midnight, 10_000.0, 10_000.0);
        // The new day opens 10% lower. Measured from yesterday's close that is
        // exactly the daily limit, so it halts on the first bar of the day.
        let halt = state.observe(&limits(), MS_PER_DAY, 9_000.0, 10_000.0);
        assert_eq!(halt, Some(HaltLimit::DailyLoss));
        assert_eq!(state.day_start(), 10_000.0);
        assert_eq!(state.day(), day_id(MS_PER_DAY));
    }

    #[test]
    fn daily_halt_clears_when_the_next_day_starts() {
        let mut state = RiskState::new(10_000.0, 0);
        state.observe(&limits(), 0, 10_000.0, 10_000.0);
        assert_eq!(
            state.observe(&limits(), MS_PER_MIN, 9_000.0, 10_000.0),
            Some(HaltLimit::DailyLoss)
        );
        // Flat across midnight: the new baseline is yesterday's 9k close, so
        // the same equity is 0% down and the halt lifts.
        assert_eq!(state.observe(&limits(), MS_PER_DAY, 9_000.0, 9_000.0), None);
        assert!(!state.is_halted());
        assert_eq!(state.day_start(), 9_000.0);
    }

    #[test]
    fn a_drawdown_halt_survives_the_day_rollover() {
        let mut state = RiskState::new(10_000.0, 0);
        state.observe(&limits(), 0, 10_000.0, 10_000.0);
        assert_eq!(
            state.observe(&limits(), MS_PER_MIN, 7_900.0, 10_000.0),
            Some(HaltLimit::Drawdown)
        );
        assert_eq!(state.observe(&limits(), MS_PER_DAY, 7_900.0, 7_900.0), None);
        assert_eq!(state.halt(), Some(HaltLimit::Drawdown));
    }

    #[test]
    fn reset_re_baselines_and_does_not_immediately_retrip() {
        let mut state = RiskState::new(10_000.0, 0);
        state.observe(&limits(), 0, 10_000.0, 10_000.0);
        state.observe(&limits(), MS_PER_MIN, 7_900.0, 10_000.0);
        assert_eq!(state.halt(), Some(HaltLimit::Drawdown));

        state.reset(7_900.0, MS_PER_MIN);
        assert!(!state.is_halted());
        assert_eq!(state.peak(), 7_900.0);
        assert_eq!(state.day_start(), 7_900.0);
        // The next bar at the same equity breaches nothing.
        assert_eq!(
            state.observe(&limits(), 2 * MS_PER_MIN, 7_900.0, 7_900.0),
            None
        );
        assert!(!state.is_halted());
    }

    #[test]
    fn clearing_the_kill_switch_leaves_a_drawdown_halt_alone() {
        let mut state = RiskState::new(10_000.0, 0);
        state.force_halt(HaltLimit::KillSwitch);
        state.clear_kill_switch();
        assert!(!state.is_halted());

        state.force_halt(HaltLimit::Drawdown);
        state.clear_kill_switch();
        assert_eq!(state.halt(), Some(HaltLimit::Drawdown));
    }

    #[test]
    fn limits_validate_bounds() {
        assert!(limits().validate().is_ok());
        for bad in [0.0, -1.0, 100.1, f64::NAN, f64::INFINITY] {
            let mut l = limits();
            l.max_position_pct = bad;
            assert!(l.validate().is_err(), "max_position_pct {bad}");
            let mut l = limits();
            l.max_daily_loss_pct = bad;
            assert!(l.validate().is_err(), "max_daily_loss_pct {bad}");
            let mut l = limits();
            l.max_drawdown_pct = bad;
            assert!(l.validate().is_err(), "max_drawdown_pct {bad}");
        }
        let exact = RiskLimits {
            max_position_pct: 100.0,
            max_daily_loss_pct: 100.0,
            max_drawdown_pct: 100.0,
        };
        assert!(exact.validate().is_ok());
    }

    #[test]
    fn defaults_are_the_agent_defaults() {
        let d = RiskLimits::default();
        assert_eq!(d.max_position_pct, 100.0);
        assert_eq!(d.max_daily_loss_pct, 10.0);
        assert_eq!(d.max_drawdown_pct, 20.0);
    }
}
