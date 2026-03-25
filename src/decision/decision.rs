/// BEAR-only strategy using taker buy/sell volume ratio.
/// Backtested edge: 57% NO win rate, +0.065 per trade over 104 days.
use crate::decision::candle::Candle;

pub struct Thresholds {
    /// Taker buy ratio threshold for BEAR entry (< this = sellers dominate)
    pub bear_ratio: f64,
    /// Lookback bars for taker ratio calculation
    pub lookback: usize,
    /// Consecutive bars below threshold to enter BEAR
    pub confirm_bars: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            bear_ratio: 0.46,
            lookback: 4,
            confirm_bars: 2,
        }
    }
}

/// Compute taker buy ratio: taker_buy_vol / total_volume over last N bars.
pub fn taker_ratio(candles: &[Candle], t: usize, lookback: usize) -> f64 {
    let start = t.saturating_sub(lookback - 1);
    let mut buy_vol = 0.0_f64;
    let mut total_vol = 0.0_f64;
    for i in start..=t {
        buy_vol += candles[i].taker_buy_vol;
        total_vol += candles[i].volume;
    }
    if total_vol < 1e-12 { 0.50 } else { buy_vol / total_vol }
}

/// Run the BEAR strategy on bar t.
/// Returns (is_bear: bool, taker_ratio: f64, consecutive_bear: u32).
pub fn evaluate(candles: &[Candle], t: usize, in_bear: bool, consec: u32, th: &Thresholds)
    -> (bool, f64, u32)
{
    let ratio = taker_ratio(candles, t, th.lookback);

    let new_consec = if ratio < th.bear_ratio {
        consec + 1
    } else {
        0
    };

    let is_bear = in_bear || new_consec >= th.confirm_bars;

    (is_bear, ratio, new_consec)
}
