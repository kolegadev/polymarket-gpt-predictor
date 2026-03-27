/// BULL v3 regime: 1h momentum mean reversion (same as BEAR v3).
/// When BTC has moved DOWN > MOM_THRESHOLD over the last 1h, we're in a
/// mean-reversion BULL zone — expect price to bounce UP.
/// Exit when momentum recovers above the threshold.

/// Momentum lookback: 12 x 5min = 1 hour
const MOM_LOOKBACK: usize = 12;
/// Momentum threshold: 0.8% (must have moved this much in 1h)
const MOM_THRESHOLD: f64 = 0.8;

pub struct Thresholds {
    /// Lookback bars for momentum calculation
    pub lookback: usize,
    /// Momentum threshold (% move) to trigger BULL zone
    pub mom_threshold: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            lookback: MOM_LOOKBACK,
            mom_threshold: MOM_THRESHOLD,
        }
    }
}

/// Compute 1h momentum as % change.
pub fn momentum_pct(candles: &[crate::decision::candle::Candle], t: usize, lookback: usize) -> f64 {
    if t < lookback { return 0.0; }
    let start_price = candles[t - lookback].open;
    let end_price = candles[t].close;
    if start_price < 1e-10 { return 0.0; }
    (end_price - start_price) / start_price * 100.0
}

/// Evaluate BULL regime on bar t.
/// BULL zone: 1h momentum < -MOM_THRESHOLD (price dropped, expect bounce)
/// Returns (in_bull, momentum_pct, consecutive_bull, consecutive_exit).
pub fn evaluate(
    candles: &[crate::decision::candle::Candle],
    t: usize,
    in_bull: bool,
    consec_bull: u32,
    consec_exit: u32,
    th: &Thresholds,
) -> (bool, f64, u32, u32) {
    let mom = momentum_pct(candles, t, th.lookback);

    let (cb, ce) = if in_bull {
        // Exit when momentum recovers above threshold (bounce happened)
        let new_ce = if mom > -th.mom_threshold { consec_exit + 1 } else { 0 };
        let new_cb = if mom < -th.mom_threshold { consec_bull + 1 } else { 0 };
        (new_cb, new_ce)
    } else {
        // Enter when momentum drops below negative threshold
        let new_cb = if mom < -th.mom_threshold { consec_bull + 1 } else { 0 };
        let new_ce = if mom > -th.mom_threshold { consec_exit + 1 } else { 0 };
        (new_cb, new_ce)
    };

    let now_bull = if in_bull {
        // Stay in bull unless exit confirmed (1 bar is enough since momentum is smooth)
        ce < 1
    } else {
        // Enter bull if momentum confirmed (1 bar below threshold)
        cb >= 1
    };

    (now_bull, mom, cb, ce)
}
