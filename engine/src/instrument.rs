/// Tradable instrument definition.
///
/// `tick_size` is the minimum price increment, `point_value` converts one
/// price point into account currency per unit of quantity. `fee_bps` and
/// `slippage_bps` are in basis points per side (like [`crate::backtest`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Instrument {
    pub symbol: String,
    pub tick_size: f64,
    pub point_value: f64,
    pub fee_bps: f64,
    pub slippage_bps: f64,
}

impl Instrument {
    pub fn new(
        symbol: &str,
        tick_size: f64,
        point_value: f64,
        fee_bps: f64,
        slippage_bps: f64,
    ) -> Result<Instrument, String> {
        if symbol.is_empty() {
            return Err("symbol must not be empty".to_string());
        }
        if !tick_size.is_finite() || tick_size <= 0.0 {
            return Err(format!("tick_size must be > 0, got {tick_size}"));
        }
        if !point_value.is_finite() || point_value <= 0.0 {
            return Err(format!("point_value must be > 0, got {point_value}"));
        }
        for (name, v) in [("fee_bps", fee_bps), ("slippage_bps", slippage_bps)] {
            if !v.is_finite() || !(0.0..=1000.0).contains(&v) {
                return Err(format!("{name} must be between 0 and 1000, got {v}"));
            }
        }
        Ok(Instrument {
            symbol: symbol.to_string(),
            tick_size,
            point_value,
            fee_bps,
            slippage_bps,
        })
    }

    /// Combined per-side cost rate, e.g. 7 bps -> 0.0007.
    pub fn cost_rate(&self) -> f64 {
        (self.fee_bps + self.slippage_bps) / 10_000.0
    }

    /// Round a price to the nearest tick.
    pub fn round_to_tick(&self, price: f64) -> f64 {
        (price / self.tick_size).round() * self.tick_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_rate_and_tick_rounding() {
        let inst = Instrument::new("BTCUSDT", 0.5, 1.0, 5.0, 2.0).expect("valid");
        assert!((inst.cost_rate() - 0.000_7).abs() < 1e-12);
        assert!((inst.round_to_tick(100.3) - 100.5).abs() < 1e-9);
        assert!((inst.round_to_tick(100.1) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn rejects_bad_instrument() {
        assert!(Instrument::new("", 0.5, 1.0, 5.0, 2.0).is_err());
        assert!(Instrument::new("X", 0.0, 1.0, 5.0, 2.0).is_err());
        assert!(Instrument::new("X", 0.5, -1.0, 5.0, 2.0).is_err());
        assert!(Instrument::new("X", 0.5, 1.0, 5000.0, 2.0).is_err());
    }
}
