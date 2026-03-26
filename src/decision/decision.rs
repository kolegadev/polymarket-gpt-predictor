/// 5m BULL YES strategy using taker buy/sell volume ratio.
pub struct Thresholds {
    /// Taker buy ratio threshold for BULL entry (> this = buyers dominate)
    pub bull_ratio: f64,
    /// Taker buy ratio threshold for BULL exit (< this = neutral/bearish)
    pub exit_ratio: f64,
    /// Lookback bars for taker ratio calculation
    pub lookback: usize,
    /// Consecutive bars to confirm entry or exit
    pub confirm_bars: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            bull_ratio: 0.54,
            exit_ratio: 0.50,
            lookback: 4,
            confirm_bars: 2,
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
/// Returns (in_bull, taker_ratio, consecutive_bull, consecutive_exit).
///
/// Entry:  ratio > bull_ratio for confirm_bars consecutive bars
/// Exit:   ratio < exit_ratio  for confirm_bars consecutive bars
pub fn evaluate(
    candles: &[crate::decision::candle::Candle],
    t: usize,
    in_bull: bool,
    consec_bull: u32,
    consec_exit: u32,
    th: &Thresholds,
) -> (bool, f64, u32, u32) {
    let ratio = taker_ratio(candles, t, th.lookback);

    let (cb, ce) = if in_bull {
        // Track exit countdown; reset bull consec if ratio is still high
        let new_ce = if ratio < th.exit_ratio { consec_exit + 1 } else { 0 };
        let new_cb = if ratio > th.bull_ratio { consec_bull + 1 } else { 0 };
        (new_cb, new_ce)
    } else {
        // Track entry countdown; reset exit consec if ratio is still low
        let new_cb = if ratio > th.bull_ratio { consec_bull + 1 } else { 0 };
        let new_ce = if ratio < th.exit_ratio { consec_exit + 1 } else { 0 };
        (new_cb, new_ce)
    };

    let now_bull = if in_bull {
        // Stay in bull unless exit confirmed
        ce < th.confirm_bars
    } else {
        // Enter bull if entry confirmed
        cb >= th.confirm_bars
    };

    (now_bull, ratio, cb, ce)
}
