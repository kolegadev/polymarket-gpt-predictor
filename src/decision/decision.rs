/// 5m BULL YES strategy using taker buy/sell volume ratio.
pub struct Thresholds {
    /// Taker buy ratio threshold for BULL entry (> this = buyers dominate)
    pub bull_ratio: f64,
    /// Lookback bars for taker ratio calculation
    pub lookback: usize,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            bull_ratio: 0.54,  // best threshold from backtest
            lookback: 4,
        }
    }
}

/// Compute taker buy ratio: taker_buy_vol / total_volume over last N bars.
pub fn taker_ratio(candles: &[crate::decision::candle::Candle], t: usize, lookback: usize) -> f64 {
    let start = t.saturating_sub(lookback - 1);
    let mut buy_vol = 0.0_f64;
    let mut total_vol = 0.0_f64;
    for i in start..=t {
        buy_vol += candles[i].taker_buy_vol;
        total_vol += candles[i].volume;
    }
    if total_vol < 1e-6 { 0.50 } else { buy_vol / total_vol }
}

/// Evaluate BULL regime on bar t.
/// Returns (is_bull, taker_ratio, consecutive_count).
pub fn evaluate(
    candles: &[crate::decision::candle::Candle],
    t: usize,
    in_bull: bool,
    _consec: u32,
    th: &Thresholds,
) -> (bool, f64, u32) {
    let ratio = taker_ratio(candles, t, th.lookback);

    let consec = if ratio > th.bull_ratio { _consec + 1 } else { 0 };

    // BULL entry: ratio > threshold (handled by caller checking consec >= 2)
    // BULL exit: ratio drops below 0.50 for 2+ bars (handled in main)

    (in_bull || consec >= 2, ratio, consec)
}
