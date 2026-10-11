#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sizing {
    #[cfg(test)]
    FixedQty(f64),
    RiskPct {
        risk_pct: f64,
        stop_pct: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum SizingError {
    #[error("equity must be > 0, got {0}")]
    Equity(f64),
    #[error("price must be > 0, got {0}")]
    Price(f64),
    #[cfg(test)]
    #[error("fixed qty must be > 0, got {0}")]
    Qty(f64),
    #[error("risk_pct must be > 0, got {0}")]
    Risk(f64),
    #[error("stop_pct must be > 0, got {0}")]
    Stop(f64),
}

pub fn quantity(sizing: Sizing, equity: f64, price: f64) -> Result<f64, SizingError> {
    if !equity.is_finite() || equity <= 0.0 {
        return Err(SizingError::Equity(equity));
    }
    if !price.is_finite() || price <= 0.0 {
        return Err(SizingError::Price(price));
    }
    match sizing {
        #[cfg(test)]
        Sizing::FixedQty(q) => {
            if !q.is_finite() || q <= 0.0 {
                return Err(SizingError::Qty(q));
            }
            Ok(q)
        }
        Sizing::RiskPct { risk_pct, stop_pct } => {
            if !risk_pct.is_finite() || risk_pct <= 0.0 {
                return Err(SizingError::Risk(risk_pct));
            }
            if !stop_pct.is_finite() || stop_pct <= 0.0 {
                return Err(SizingError::Stop(stop_pct));
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
