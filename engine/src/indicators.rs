pub fn sma(x: &[f64], n: usize) -> Vec<f64> {
    let len = x.len();
    if len == 0 {
        return Vec::new();
    }
    if n == 0 || n > len {
        return vec![f64::NAN; len];
    }

    let mut out = vec![f64::NAN; len];
    let mut sum = 0.0;
    for i in 0..len {
        sum += x[i];
        if i >= n {
            sum -= x[i - n];
        }
        if i + 1 >= n {
            out[i] = sum / n as f64;
        }
    }
    out
}

pub fn ema(x: &[f64], n: usize) -> Vec<f64> {
    let len = x.len();
    if len == 0 {
        return Vec::new();
    }
    if n == 0 || n > len {
        return vec![f64::NAN; len];
    }

    let mut out = vec![f64::NAN; len];
    let k = 2.0 / (n as f64 + 1.0);

    let seed: f64 = x[..n].iter().sum::<f64>() / n as f64;
    out[n - 1] = seed;

    for i in n..len {
        out[i] = x[i] * k + out[i - 1] * (1.0 - k);
    }
    out
}

pub fn rsi(x: &[f64], n: usize) -> Vec<f64> {
    let len = x.len();
    if len == 0 {
        return Vec::new();
    }
    if n == 0 || len <= n {
        return vec![f64::NAN; len];
    }

    fn rsi_from_averages(avg_gain: f64, avg_loss: f64) -> f64 {
        if avg_loss == 0.0 {
            if avg_gain == 0.0 {
                50.0
            } else {
                100.0
            }
        } else {
            let rs = avg_gain / avg_loss;
            100.0 - 100.0 / (1.0 + rs)
        }
    }

    let mut out = vec![f64::NAN; len];

    let mut avg_gain = 0.0;
    let mut avg_loss = 0.0;
    for i in 1..=n {
        let delta = x[i] - x[i - 1];
        if delta > 0.0 {
            avg_gain += delta;
        } else {
            avg_loss += -delta;
        }
    }
    avg_gain /= n as f64;
    avg_loss /= n as f64;
    out[n] = rsi_from_averages(avg_gain, avg_loss);

    for i in (n + 1)..len {
        let delta = x[i] - x[i - 1];
        let gain = if delta > 0.0 { delta } else { 0.0 };
        let loss = if delta < 0.0 { -delta } else { 0.0 };
        avg_gain = (avg_gain * (n as f64 - 1.0) + gain) / n as f64;
        avg_loss = (avg_loss * (n as f64 - 1.0) + loss) / n as f64;
        out[i] = rsi_from_averages(avg_gain, avg_loss);
    }

    out
}

#[cfg(test)]
pub fn true_range(high: f64, low: f64, prev_close: f64) -> f64 {
    let hl = high - low;
    let hc = (high - prev_close).abs();
    let lc = (low - prev_close).abs();
    hl.max(hc).max(lc)
}

#[cfg(test)]
pub fn atr(candles: &[crate::types::Candle], n: usize) -> Vec<f64> {
    let len = candles.len();
    if len == 0 {
        return Vec::new();
    }
    if n == 0 || n > len {
        return vec![f64::NAN; len];
    }
    let mut tr = Vec::with_capacity(len);
    for (i, c) in candles.iter().enumerate() {
        if i == 0 {
            tr.push(c.high - c.low);
        } else {
            tr.push(true_range(c.high, c.low, candles[i - 1].close));
        }
    }
    let mut out = vec![f64::NAN; len];
    let seed: f64 = tr[..n].iter().sum::<f64>() / n as f64;
    out[n - 1] = seed;
    for i in n..len {
        out[i] = (out[i - 1] * (n as f64 - 1.0) + tr[i]) / n as f64;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_approx_eq(a: f64, b: f64, eps: f64) {
        if a.is_nan() && b.is_nan() {
            return;
        }
        assert!(
            !a.is_nan() && !b.is_nan(),
            "mismatch: one is NaN (a={a}, b={b})"
        );
        assert!(
            (a - b).abs() <= eps,
            "assert_approx_eq failed: {a} vs {b} (eps={eps})"
        );
    }

    fn assert_vec_approx_eq(actual: &[f64], expected: &[f64], eps: f64) {
        assert_eq!(
            actual.len(),
            expected.len(),
            "length mismatch: {} vs {}",
            actual.len(),
            expected.len()
        );
        for (i, (&a, &b)) in actual.iter().zip(expected.iter()).enumerate() {
            if b.is_nan() {
                assert!(a.is_nan(), "index {i}: expected NaN, got {a}");
            } else {
                assert!(!a.is_nan(), "index {i}: expected {b}, got NaN");
                assert!((a - b).abs() <= eps, "index {i}: {a} vs {b} (eps={eps})");
            }
        }
    }

    #[test]
    fn sma_basic() {
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let got = sma(&x, 3);
        let expected = vec![f64::NAN, f64::NAN, 2.0, 3.0, 4.0];
        assert_vec_approx_eq(&got, &expected, 1e-12);
    }

    #[test]
    fn sma_window_one_is_identity() {
        let x = vec![5.0, -1.5, 3.25];
        let got = sma(&x, 1);
        assert_vec_approx_eq(&got, &x, 1e-12);
    }

    #[test]
    fn sma_warmup_and_edges() {
        let got = sma(&[1.0, 2.0], 5);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|v| v.is_nan()));

        let got = sma(&[1.0, 2.0, 3.0], 0);
        assert!(got.iter().all(|v| v.is_nan()));

        assert!(sma(&[], 3).is_empty());
    }

    #[test]
    fn ema_basic() {
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let got = ema(&x, 3);
        let expected = vec![f64::NAN, f64::NAN, 2.0, 3.0, 4.0];
        assert_vec_approx_eq(&got, &expected, 1e-12);
    }

    #[test]
    fn ema_seed_and_smoothing() {
        let x = vec![10.0, 11.0, 12.0];
        let got = ema(&x, 2);
        let expected = vec![f64::NAN, 10.5, 11.5];
        assert_vec_approx_eq(&got, &expected, 1e-12);
    }

    #[test]
    fn ema_warmup_and_edges() {
        let got = ema(&[1.0, 2.0], 5);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|v| v.is_nan()));

        let got = ema(&[1.0, 2.0, 3.0], 0);
        assert!(got.iter().all(|v| v.is_nan()));

        assert!(ema(&[], 3).is_empty());

        let x = vec![2.0, 4.0, 6.0];
        assert_vec_approx_eq(&ema(&x, 1), &x, 1e-12);
    }

    #[test]
    fn rsi_all_gains_is_100() {
        let x = vec![10.0, 11.0, 12.0, 13.0, 14.0];
        let got = rsi(&x, 2);
        let expected = vec![f64::NAN, f64::NAN, 100.0, 100.0, 100.0];
        assert_vec_approx_eq(&got, &expected, 1e-12);
    }

    #[test]
    fn rsi_all_losses_is_0() {
        let x = vec![14.0, 13.0, 12.0, 11.0];
        let got = rsi(&x, 2);
        let expected = vec![f64::NAN, f64::NAN, 0.0, 0.0];
        assert_vec_approx_eq(&got, &expected, 1e-12);
    }

    #[test]
    fn rsi_mixed_wilder_smoothing() {
        let x = vec![10.0, 11.0, 10.0, 11.0];
        let got = rsi(&x, 2);
        let expected = vec![f64::NAN, f64::NAN, 50.0, 75.0];
        assert_vec_approx_eq(&got, &expected, 1e-12);
    }

    #[test]
    fn rsi_warmup_and_edges() {
        let got = rsi(&[10.0, 11.0], 2);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|v| v.is_nan()));

        let got = rsi(&[10.0, 11.0, 12.0], 0);
        assert!(got.iter().all(|v| v.is_nan()));

        assert!(rsi(&[], 14).is_empty());

        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let got = rsi(&x, 3);
        assert_eq!(got.len(), x.len());
        assert!(got[0].is_nan() && got[1].is_nan() && got[2].is_nan());
        assert!(!got[3].is_nan());
        assert_approx_eq(got[3], 100.0, 1e-12);
    }

    fn mk_candle_ts(h: f64, l: f64, c: f64) -> crate::types::Candle {
        crate::types::Candle {
            ts: 0,
            open: c,
            high: h,
            low: l,
            close: c,
        }
    }

    #[test]
    fn atr_hand_computed_wilder() {
        let candles = vec![
            mk_candle_ts(10.0, 9.0, 9.5),
            mk_candle_ts(11.0, 9.5, 10.5),
            mk_candle_ts(12.0, 10.0, 11.0),
        ];
        let got = atr(&candles, 2);
        let expected = vec![f64::NAN, 1.25, 1.625];
        assert_vec_approx_eq(&got, &expected, 1e-12);
    }

    #[test]
    fn atr_edges() {
        let candles = vec![mk_candle_ts(10.0, 9.0, 9.5)];
        assert!(atr(&candles, 5).iter().all(|v| v.is_nan()));
        assert!(atr(&candles, 0).iter().all(|v| v.is_nan()));
        let empty: Vec<crate::types::Candle> = Vec::new();
        assert!(atr(&empty, 14).is_empty());
    }
}
