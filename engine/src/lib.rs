pub mod backtest;
pub mod stats;
pub mod strategy;

/// A single OHLC candle.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Candle {
    pub ts: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

/// Simple moving average.
///
/// Returns a `Vec` the same length as the input. The first `n - 1` values
/// (the warmup period) are `f64::NAN`. Element `i >= n - 1` is the mean of
/// `x[i + 1 - n ..= i]`.
///
/// Edge cases: if `n == 0`, `x` is empty, or `n > x.len()`, there is no valid
/// window, so the result is all `NAN` (or empty for empty input).
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

/// Exponential moving average.
///
/// Uses `k = 2 / (n + 1)` and seeds the first valid value at index `n - 1`
/// with the simple moving average of the first `n` inputs. All earlier
/// entries are `f64::NAN`.
///
/// Edge cases mirror [`sma`]: `n == 0`, empty input, or `n > x.len()`
/// yields all `NAN` (or empty).
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

    // Seed with SMA of first `n` values.
    let seed: f64 = x[..n].iter().sum::<f64>() / n as f64;
    out[n - 1] = seed;

    for i in n..len {
        out[i] = x[i] * k + out[i - 1] * (1.0 - k);
    }
    out
}

/// Relative strength index with Wilder smoothing.
///
/// Returns a `Vec` the same length as the input. The first `n` values are
/// `f64::NAN` because `n` price changes (i.e. `n + 1` prices) are needed for
/// the first reading at index `n`.
///
/// Method:
/// * `gain[i] = max(close[i] - close[i-1], 0)`,
///   `loss[i] = max(close[i-1] - close[i], 0)`.
/// * First `avg_gain` / `avg_loss` are simple means of the first `n` gains/losses.
/// * Afterwards: `avg = (prev * (n - 1) + current) / n` (Wilder).
/// * `RSI = 100 - 100 / (1 + RS)` with `RS = avg_gain / avg_loss`.
/// * If `avg_loss == 0`: RSI is `100.0` when `avg_gain > 0`, else `50.0`
///   (flat series, no gain and no loss).
///
/// Edge cases: `n == 0` or `x.len() <= n` yields all `NAN` (or empty input
/// yields empty output).
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

    // Initial averages over deltas x[1]-x[0] .. x[n]-x[n-1].
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
        // Hand-computed: means of sliding windows of 3.
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
        // n > len -> all NaN, same length.
        let got = sma(&[1.0, 2.0], 5);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|v| v.is_nan()));

        // n == 0 -> all NaN.
        let got = sma(&[1.0, 2.0, 3.0], 0);
        assert!(got.iter().all(|v| v.is_nan()));

        // Empty input -> empty output.
        assert!(sma(&[], 3).is_empty());
    }

    #[test]
    fn ema_basic() {
        // n=3 => k=0.5, seed SMA(1,2,3)=2.0 at idx 2,
        // idx3 = 4*0.5 + 2*0.5 = 3.0, idx4 = 5*0.5 + 3*0.5 = 4.0.
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let got = ema(&x, 3);
        let expected = vec![f64::NAN, f64::NAN, 2.0, 3.0, 4.0];
        assert_vec_approx_eq(&got, &expected, 1e-12);
    }

    #[test]
    fn ema_seed_and_smoothing() {
        // n=2 => k=2/3, seed (10+11)/2=10.5 at idx1,
        // idx2 = 12*(2/3) + 10.5*(1/3) = 8 + 3.5 = 11.5.
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

        // Window of 1: k=1, EMA equals the price itself.
        let x = vec![2.0, 4.0, 6.0];
        assert_vec_approx_eq(&ema(&x, 1), &x, 1e-12);
    }

    #[test]
    fn rsi_all_gains_is_100() {
        // Deltas are all +1, so avg_loss=0 -> RSI 100.
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
        // x=[10,11,10,11], n=2, hand-computed:
        // deltas: +1,-1,+1
        // idx2: avg_gain=(1+0)/2=0.5, avg_loss=(0+1)/2=0.5 -> RS=1 -> RSI=50
        // idx3: avg_gain=(0.5*1+1)/2=0.75, avg_loss=(0.5*1+0)/2=0.25
        //       RS=3 -> RSI=100-100/4=75
        let x = vec![10.0, 11.0, 10.0, 11.0];
        let got = rsi(&x, 2);
        let expected = vec![f64::NAN, f64::NAN, 50.0, 75.0];
        assert_vec_approx_eq(&got, &expected, 1e-12);
    }

    #[test]
    fn rsi_warmup_and_edges() {
        // Need n+1 prices for the first value, so len <= n -> all NaN.
        let got = rsi(&[10.0, 11.0], 2);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|v| v.is_nan()));

        let got = rsi(&[10.0, 11.0, 12.0], 0);
        assert!(got.iter().all(|v| v.is_nan()));

        assert!(rsi(&[], 14).is_empty());

        // Same length as input, NaN warmup of exactly n.
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let got = rsi(&x, 3);
        assert_eq!(got.len(), x.len());
        assert!(got[0].is_nan() && got[1].is_nan() && got[2].is_nan());
        assert!(!got[3].is_nan());
        assert_approx_eq(got[3], 100.0, 1e-12);
    }
}
