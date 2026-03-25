use crate::decision::candle::Candle;
use crate::decision::features::Features;
use crate::decision::state::{State, Decision};

/// Thresholds — can be made configurable later.
pub struct Thresholds {
    pub bull_spread: f64,    // 0.10
    pub bull_slope: f64,     // 0.08
    pub bull_slope99: f64,   // -0.03
    pub bull_score_min: u32, // 4
    pub bear_spread: f64,    // -0.10
    pub bear_slope: f64,     // -0.08
    pub bear_score_min: u32, // 4
    pub bull_warnings_max: u32, // 2 triggers exit
    pub yes_close_pos: f64,  // 0.50
    pub yes_dist_max: f64,   // 1.40
    pub yes_range_max: f64,  // 1.80
    pub yes_wick_max: f64,   // 0.35
    pub no_close_pos: f64,   // 0.35
    pub no_upper_wick_min: f64, // 0.20
    pub no_lower_wick_max: f64, // 0.30
    pub no_dist_max: f64,    // 1.20
    pub no_ma_tolerance: f64, // 0.10
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            bull_spread: 0.10,
            bull_slope: 0.08,
            bull_slope99: -0.03,
            bull_score_min: 4,
            bear_spread: -0.10,
            bear_slope: -0.08,
            bear_score_min: 4,
            bull_warnings_max: 2,
            yes_close_pos: 0.50,
            yes_dist_max: 1.40,
            yes_range_max: 1.80,
            yes_wick_max: 0.35,
            no_close_pos: 0.35,
            no_upper_wick_min: 0.20,
            no_lower_wick_max: 0.30,
            no_dist_max: 1.20,
            no_ma_tolerance: 0.10,
        }
    }
}

fn bull_candidate(c: &Candle, f: &Features, th: &Thresholds) -> bool {
    f.sma7 > f.sma25
        && f.sma25 > f.sma99
        && f.spread_7_25 >= th.bull_spread
        && f.slope_25 >= th.bull_slope
        && f.slope_99 >= th.bull_slope99
        && c.close > f.sma25
}

pub fn bull_score(candles: &[Candle], t: usize, f: &Features) -> u32 {
    let c = &candles[t];
    let mut s = 0u32;
    if c.close >= f.sma7 { s += 1; }
    if f.close_pos >= 0.55 { s += 1; }
    if f.green_8 >= 5 { s += 1; }
    if t >= 2 && c.low > candles[t - 2].low { s += 1; }
    if t >= 2 && c.high > candles[t - 2].high { s += 1; }
    if f.dist_close_25 <= 1.40 { s += 1; }
    s
}

fn bear_candidate(c: &Candle, f: &Features, th: &Thresholds) -> bool {
    f.sma7 < f.sma25
        && f.sma25 < f.sma99
        && f.spread_7_25 <= th.bear_spread
        && f.slope_25 <= th.bear_slope
        && c.close < f.sma25
        && c.close < f.sma99
}

pub fn bear_score(candles: &[Candle], t: usize, f: &Features) -> u32 {
    let c = &candles[t];
    let mut s = 0u32;
    if c.close <= f.sma7 { s += 1; }
    if f.close_pos <= 0.45 { s += 1; }
    if f.red_8 >= 4 { s += 1; }
    if t >= 2 && c.low < candles[t - 2].low { s += 1; }
    if t >= 2 && c.high < candles[t - 2].high { s += 1; }
    if (f.sma25 - c.close) / f.atr <= 1.20 { s += 1; }
    s
}

fn bull_warnings(candles: &[Candle], t: usize, f: &Features, prev_f: &Features, prev2_f: &Features) -> u32 {
    let c = &candles[t];
    let mut w = 0u32;
    if c.close < f.sma7 { w += 1; }
    if f.close_pos < 0.45 { w += 1; }
    if f.green_8 <= 4 { w += 1; }
    if t >= 1 && c.high <= candles[t - 1].high { w += 1; }
    if f.spread_7_25 < prev_f.spread_7_25 && prev_f.spread_7_25 < prev2_f.spread_7_25 { w += 1; }
    w
}

fn bull_yes_filter(f: &Features, th: &Thresholds) -> bool {
    let rng_safe = if f.range < 1e-12 { 1e-12 } else { f.range };
    f.close_pos >= th.yes_close_pos
        && f.dist_close_25 <= th.yes_dist_max
        && (f.range / f.atr) <= th.yes_range_max
        && (f.upper_wick / rng_safe) <= th.yes_wick_max
}

fn bear_no_filter(candles: &[Candle], t: usize, f: &Features, th: &Thresholds) -> bool {
    let c = &candles[t];
    let rng_safe = if f.range < 1e-12 { 1e-12 } else { f.range };
    c.high >= f.sma7 - th.no_ma_tolerance * f.atr
        && c.close < c.open                           // red
        && f.close_pos <= th.no_close_pos             // lower third
        && (f.upper_wick / rng_safe) >= th.no_upper_wick_min  // rejection
        && t >= 1 && c.low < candles[t - 1].low       // continuation
        && (f.sma25 - c.close) / f.atr <= th.no_dist_max      // not overextended
        && (f.lower_wick / rng_safe) <= th.no_lower_wick_max   // not hammer
}

/// Compute next regime state given previous state and features.
pub fn next_state(
    prev: State,
    candles: &[Candle],
    t: usize,
    f: &Features,
    f_prev: &Features,
    f_prev2: &Features,
    th: &Thresholds,
) -> State {
    let c = &candles[t];

    // Hard bull exit
    if prev == State::Bull {
        if c.close < f.sma25 || f.sma7 < f.sma25 || f.slope_25 < 0.0 {
            return State::Neutral;
        }
        if bull_warnings(candles, t, f, f_prev, f_prev2) >= th.bull_warnings_max {
            return State::Neutral;
        }
    }

    // Hard bear exit
    if prev == State::Bear {
        if c.close > f.sma25 || f.sma7 > f.sma25 || f.slope_25 > 0.0 {
            return State::Neutral;
        }
    }

    // Entry checks (need t-1 valid)
    if t < 1 { return State::Neutral; }
    let f_prev_ok = f_prev; // already computed by caller
    let _f_prev2_ok = if t >= 2 { f_prev2 } else { f_prev };

    let bull_ok = bull_candidate(c, f, th)
        && bull_candidate(&candles[t - 1], f_prev_ok, th)
        && bull_score(candles, t, f) >= th.bull_score_min
        && bull_score(candles, t - 1, f_prev_ok) >= th.bull_score_min;

    let bear_ok = bear_candidate(c, f, th)
        && bear_candidate(&candles[t - 1], f_prev_ok, th)
        && bear_score(candles, t, f) >= th.bear_score_min
        && bear_score(candles, t - 1, f_prev_ok) >= th.bear_score_min;

    if bull_ok { return State::Bull; }
    if bear_ok { return State::Bear; }
    State::Neutral
}

/// Main decision function: returns (new_state, decision) for bar t+1.
pub fn decide(
    prev_state: State,
    candles: &[Candle],
    t: usize,
    f_t: &Features,
    f_t1: &Features,
    f_t2: &Features,
    th: &Thresholds,
) -> (State, Decision) {
    let state = next_state(prev_state, candles, t, f_t, f_t1, f_t2, th);

    let decision = match state {
        State::Bull if bull_yes_filter(f_t, th) => Decision::Yes,
        State::Bear if bear_no_filter(candles, t, f_t, th) => Decision::No,
        _ => Decision::Skip,
    };

    (state, decision)
}
