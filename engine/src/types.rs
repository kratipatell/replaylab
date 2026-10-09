/// A single OHLC candle.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Candle {
    pub ts: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}
