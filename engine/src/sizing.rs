/// Position sizing model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sizing {
    /// Always trade `qty` units.
    FixedQty(f64),
    /// Risk `risk_pct` % of equity given a `stop_pct` % adverse move:
    /// `qty = equity * risk_pct/100 / (price * stop_pct/100)`.
    RiskPct { risk_pct: f64, stop_pct: f64 },
}

/// Compute quantity for one order.
///
/// `equity` and `price` must be finite and positive.
pub fn quantity(sizing: Sizing, equity: f64, price: f64) -> Result<f64, String> {
    if !equity.is_finite() || equity <= 0.0 {
        return Err(format!("equity must be > 0, got {equity}"));
    }
    if !price.is_finite() || price <= 0.0 {
        return Err(format!("price must be > 0, got {price}"));
    }
    match sizing {
        Sizing::FixedQty(q) => {
            if !q.is_finite() || q <= 0.0 {
                return Err(format!("fixed qty must be > 0, got {q}"));
            }
            Ok(q)
        }
        Sizing::RiskPct { risk_pct, stop_pct } => {
            if !risk_pct.is_finite() || risk_pct <= 0.0 {
                return Err(format!("risk_pct must be > 0, got {risk_pct}"));
            }
            if !stop_pct.is_finite() || stop_pct <= 0.0 {
                return Err(format!("stop_pct must be > 0, got {stop_pct}"));
            }
            Ok(equity * (risk_pct / 100.0) / (price * (stop_pct / 100.0)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_qty() {
        assert_eq!(
            quantity(Sizing::FixedQty(2.5), 10_000.0, 100.0).unwrap(),
            2.5
        );
        assert!(quantity(Sizing::FixedQty(0.0), 10_000.0, 100.0).is_err());
    }

    #[test]
    fn risk_pct_hand_computed() {
        // Risk 1% of 10_000 = 100 with a 2% stop on a 100 price:
        // qty = 100 / 2 = 50.
        let q = quantity(
            Sizing::RiskPct {
                risk_pct: 1.0,
                stop_pct: 2.0,
            },
            10_000.0,
            100.0,
        )
        .unwrap();
        assert!((q - 50.0).abs() < 1e-9);
    }

    #[test]
    fn rejects_bad_inputs() {
        assert!(quantity(
            Sizing::RiskPct {
                risk_pct: 1.0,
                stop_pct: 0.0,
            },
            10_000.0,
            100.0
        )
        .is_err());
        assert!(quantity(Sizing::FixedQty(1.0), -5.0, 100.0).is_err());
        assert!(quantity(Sizing::FixedQty(1.0), 10_000.0, 0.0).is_err());
    }
}
