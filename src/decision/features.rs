use crate::decision::candle::Candle;

/// Precomputed features for a single bar, all ATR-normalized where applicable.
#[derive(Debug, Clone, Default)]
pub struct Features {
    pub sma7: f64,
    pub sma25: f64,
    pub sma99: f64,
    pub atr: f64,

    pub spread_7_25: f64,   // (sma7 - sma25) / atr
    pub slope_25: f64,      // (sma25[t] - sma25[t-4]) / atr
    pub slope_99: f64,      // (sma99[t] - sma99[t-8]) / atr
    pub dist_close_25: f64, // (close - sma25) / atr

    pub range: f64,
    pub upper_wick: f64,
    pub lower_wick: f64,
    pub close_pos: f64,

    pub green_8: u32,  // green candles in last 8
    pub red_8: u32,    // red candles in last 8
}

pub fn compute(candles: &[Candle], t: usize) -> Features {
    let c = &candles[t];
    let sma7 = sma(candles, t, 7);
    let sma25 = sma(candles, t, 25);
    let sma99 = sma(candles, t, 99);
    let atr = atr14(candles, t);
    let atr_safe = if atr < 1e-12 { 1.0 } else { atr };

    let sma25_4 = if t >= 4 { sma(candles, t - 4, 25) } else { sma25 };
    let sma99_8 = if t >= 8 { sma(candles, t - 8, 99) } else { sma99 };

    let rng = c.range();
    let _rng_safe = if rng < 1e-12 { 1e-12 } else { rng };

    let mut green_8 = 0u32;
    let mut red_8 = 0u32;
    for i in t.saturating_sub(7)..=t {
        if candles[i].is_green() { green_8 += 1; }
        if candles[i].is_red() { red_8 += 1; }
    }

    Features {
        sma7, sma25, sma99, atr,
        spread_7_25: (sma7 - sma25) / atr_safe,
        slope_25: (sma25 - sma25_4) / atr_safe,
        slope_99: (sma99 - sma99_8) / atr_safe,
        dist_close_25: (c.close - sma25) / atr_safe,
        range: rng,
        upper_wick: c.upper_wick(),
        lower_wick: c.lower_wick(),
        close_pos: c.close_pos(),
        green_8, red_8,
    }
}

fn sma(candles: &[Candle], t: usize, period: usize) -> f64 {
    let start = t.saturating_sub(period - 1);
    if start >= t + 1 || candles.len() <= start {
        return candles[t].close;
    }
    let sum: f64 = (start..=t).map(|i| candles[i].close).sum();
    sum / (t - start + 1) as f64
}

fn atr14(candles: &[Candle], t: usize) -> f64 {
    if t == 0 { return candles[0].range(); }
    let period = 14.min(t + 1);
    let start = t + 1 - period;
    // Wilder's smoothing approximation: SMA of true range for V1 simplicity
    let sum: f64 = (start..=t).map(|i| {
        let tr = candles[i].high - candles[i].low
            .max((candles[i].high - candles[i - 1].close).abs())
            .max((candles[i].low - candles[i - 1].close).abs());
        tr
    }).sum();
    sum / period as f64
}
