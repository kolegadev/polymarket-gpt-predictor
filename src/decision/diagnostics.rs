/// Diagnostic pass: run features on all bars and log distribution stats
/// to understand where the filter chain is too aggressive.
use crate::decision::candle::Candle;
use crate::decision::features;
use crate::decision::state::State;
use crate::decision::decision::{self, Thresholds, bull_candidate, bear_candidate, bull_score, bear_score, bull_yes_filter, bear_no_filter, bull_warnings};

pub fn run_diagnostics(candles: &[Candle]) {
    if candles.len() < 110 {
        eprintln!("Need at least 110 candles, got {}", candles.len());
        return;
    }

    let th = Thresholds::default();
    let eval_start = 100;
    let total = candles.len() - eval_start - 1;

    // Distribution collectors
    let mut spread_7_25_vals: Vec<f64> = Vec::new();
    let mut slope_25_vals: Vec<f64> = Vec::new();
    let mut close_pos_vals: Vec<f64> = Vec::new();
    let mut green_8_vals: Vec<u32> = Vec::new();

    // Pipeline counters
    let mut ma_stack_bull = 0;
    let mut spread_bull = 0;
    let mut slope_bull = 0;
    let mut close_above_25 = 0;
    let mut full_bull_candidate = 0;
    let mut bull_score_pass = 0;
    let mut consecutive_bull = 0;
    let mut bull_yes_filter_pass = 0;

    let mut ma_stack_bear = 0;
    let mut full_bear_candidate = 0;
    let mut bear_score_pass = 0;
    let mut consecutive_bear = 0;
    let mut bear_no_filter_pass = 0;

    let mut prev_was_bull_candidate = false;
    let mut prev_was_bear_candidate = false;

    for t in eval_start..candles.len() - 1 {
        let f = features::compute(candles, t);

        // Collect distributions
        spread_7_25_vals.push(f.spread_7_25);
        slope_25_vals.push(f.slope_25);
        close_pos_vals.push(f.close_pos);
        green_8_vals.push(f.green_8);

        // === BULL pipeline ===
        let stack_ok = f.sma7 > f.sma25 && f.sma25 > f.sma99;
        if stack_ok { ma_stack_bull += 1; }

        if stack_ok && f.spread_7_25 >= th.bull_spread { spread_bull += 1; }
        if stack_ok && f.spread_7_25 >= th.bull_spread && f.slope_25 >= th.bull_slope { slope_bull += 1; }
        if stack_ok && f.spread_7_25 >= th.bull_spread && f.slope_25 >= th.bull_slope && candles[t].close > f.sma25 { close_above_25 += 1; }

        let is_bc = bull_candidate(&candles[t], &f, &th);
        if is_bc { full_bull_candidate += 1; }

        let bs = bull_score(candles, t, &f);
        if is_bc && bs >= th.bull_score_min { bull_score_pass += 1; }

        let consec = is_bc && prev_was_bull_candidate && bs >= th.bull_score_min;
        if consec { consecutive_bull += 1; }

        let yf = bull_yes_filter(&f, &th);
        if consec && yf { bull_yes_filter_pass += 1; }

        prev_was_bull_candidate = is_bc && bs >= th.bull_score_min;

        // === BEAR pipeline ===
        let bstack_ok = f.sma7 < f.sma25 && f.sma25 < f.sma99;
        if bstack_ok { ma_stack_bear += 1; }

        let is_brc = bear_candidate(&candles[t], &f, &th);
        if is_brc { full_bear_candidate += 1; }

        let brs = bear_score(candles, t, &f);
        if is_brc && brs >= th.bear_score_min { bear_score_pass += 1; }

        let bconsec = is_brc && prev_was_bear_candidate && brs >= th.bear_score_min;
        if bconsec { consecutive_bear += 1; }

        let nf = bear_no_filter(candles, t, &f, &th);
        if bconsec && nf { bear_no_filter_pass += 1; }

        prev_was_bear_candidate = is_brc && brs >= th.bear_score_min;
    }

    // Compute percentiles for spread and slope
    let mut s_sorted = spread_7_25_vals.clone();
    s_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut sl_sorted = slope_25_vals.clone();
    sl_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let pct = |v: &Vec<f64>, p: f64| -> f64 {
        let idx = ((p / 100.0) * (v.len() - 1) as f64) as usize;
        v[idx.min(v.len() - 1)]
    };

    println!("╔══════════════════════════════════════════════════════════════════╗");
    println!("║                    THRESHOLD DIAGNOSTICS                        ║");
    println!("╠══════════════════════════════════════════════════════════════════╣");
    println!("║ Bars evaluated: {}                                              ", total);
    println!("╠══════════════════════════════════════════════════════════════════╣");
    println!("║ FEATURE DISTRIBUTIONS                                            ║");
    println!("╠══════════════════════════════════════════════════════════════════╣");
    println!("║ spread_7_25: min={:.3} p10={:.3} p25={:.3} p50={:.3} p75={:.3} p90={:.3} max={:.3}",
        s_sorted.first().unwrap_or(&0.0),
        pct(&s_sorted, 10.0), pct(&s_sorted, 25.0), pct(&s_sorted, 50.0),
        pct(&s_sorted, 75.0), pct(&s_sorted, 90.0), s_sorted.last().unwrap_or(&0.0));
    println!("║ slope_25:    min={:.3} p10={:.3} p25={:.3} p50={:.3} p75={:.3} p90={:.3} max={:.3}",
        sl_sorted.first().unwrap_or(&0.0),
        pct(&sl_sorted, 10.0), pct(&sl_sorted, 25.0), pct(&sl_sorted, 50.0),
        pct(&sl_sorted, 75.0), pct(&sl_sorted, 90.0), sl_sorted.last().unwrap_or(&0.0));

    let mut g8_sorted = green_8_vals.clone();
    g8_sorted.sort();
    println!("║ green_8:     min={} p10={} p25={} p50={} p75={} p90={} max={}",
        g8_sorted.first().unwrap_or(&0), g8_sorted[total/10], g8_sorted[total/4],
        g8_sorted[total/2], g8_sorted[3*total/4], g8_sorted[9*total/10], g8_sorted.last().unwrap_or(&0));
    println!("║                                                                  ");

    let spread_gt_010 = s_sorted.iter().filter(|&&x| x >= 0.10).count();
    let spread_gt_005 = s_sorted.iter().filter(|&&x| x >= 0.05).count();
    let spread_gt_000 = s_sorted.iter().filter(|&&x| x > 0.0).count();
    let spread_lt_000 = s_sorted.iter().filter(|&&x| x < 0.0).count();
    let spread_lt_m010 = s_sorted.iter().filter(|&&x| x <= -0.10).count();
    let spread_lt_m005 = s_sorted.iter().filter(|&&x| x <= -0.05).count();

    println!("║ spread > 0.00: {}/{} ({:.1}%)   spread < 0.00: {}/{} ({:.1}%)",
        spread_gt_000, total, spread_gt_000 as f64/total as f64*100.0,
        spread_lt_000, total, spread_lt_000 as f64/total as f64*100.0);
    println!("║ spread > 0.05: {}/{} ({:.1}%)   spread < -0.05: {}/{} ({:.1}%)",
        spread_gt_005, total, spread_gt_005 as f64/total as f64*100.0,
        spread_lt_m005, total, spread_lt_m005 as f64/total as f64*100.0);
    println!("║ spread > 0.10: {}/{} ({:.1}%)   spread < -0.10: {}/{} ({:.1}%)",
        spread_gt_010, total, spread_gt_010 as f64/total as f64*100.0,
        spread_lt_m010, total, spread_lt_m010 as f64/total as f64*100.0);
    println!("║                                                                  ");

    let slope_gt_008 = sl_sorted.iter().filter(|&&x| x >= 0.08).count();
    let slope_gt_004 = sl_sorted.iter().filter(|&&x| x >= 0.04).count();
    let slope_gt_000 = sl_sorted.iter().filter(|&&x| x > 0.0).count();
    let slope_lt_000 = sl_sorted.iter().filter(|&&x| x < 0.0).count();
    let slope_lt_m008 = sl_sorted.iter().filter(|&&x| x <= -0.08).count();
    let slope_lt_m004 = sl_sorted.iter().filter(|&&x| x <= -0.04).count();

    println!("║ slope > 0.00: {}/{} ({:.1}%)   slope < 0.00: {}/{} ({:.1}%)",
        slope_gt_000, total, slope_gt_000 as f64/total as f64*100.0,
        slope_lt_000, total, slope_lt_000 as f64/total as f64*100.0);
    println!("║ slope > 0.04: {}/{} ({:.1}%)   slope < -0.04: {}/{} ({:.1}%)",
        slope_gt_004, total, slope_gt_004 as f64/total as f64*100.0,
        slope_lt_m004, total, slope_lt_m004 as f64/total as f64*100.0);
    println!("║ slope > 0.08: {}/{} ({:.1}%)   slope < -0.08: {}/{} ({:.1}%)",
        slope_gt_008, total, slope_gt_008 as f64/total as f64*100.0,
        slope_lt_m008, total, slope_lt_m008 as f64/total as f64*100.0);
    println!("╠══════════════════════════════════════════════════════════════════╣");
    println!("║ BULL PIPELINE (current thresholds)                               ");
    println!("╠══════════════════════════════════════════════════════════════════╣");
    println!("║ MA stack bullish (7>25>99):     {}/{} ({:.1}%)", ma_stack_bull, total, ma_stack_bull as f64/total as f64*100.0);
    println!("║ + spread >= 0.10:               {}/{} ({:.1}%)", spread_bull, total, spread_bull as f64/total as f64*100.0);
    println!("║ + slope >= 0.08:                {}/{} ({:.1}%)", slope_bull, total, slope_bull as f64/total as f64*100.0);
    println!("║ + close > SMA25:                {}/{} ({:.1}%)", close_above_25, total, close_above_25 as f64/total as f64*100.0);
    println!("║ Full bull candidate:            {}/{} ({:.1}%)", full_bull_candidate, total, full_bull_candidate as f64/total as f64*100.0);
    println!("║ + bull_score >= 4:              {}/{} ({:.1}%)", bull_score_pass, total, bull_score_pass as f64/total as f64*100.0);
    println!("║ + 2-bar consecutive:            {}/{} ({:.1}%)", consecutive_bull, total, consecutive_bull as f64/total as f64*100.0);
    println!("║ + YES filter pass:              {}/{} ({:.1}%)", bull_yes_filter_pass, total, bull_yes_filter_pass as f64/total as f64*100.0);
    println!("╠══════════════════════════════════════════════════════════════════╣");
    println!("║ BEAR PIPELINE (current thresholds)                               ");
    println!("╠══════════════════════════════════════════════════════════════════╣");
    println!("║ MA stack bearish (7<25<99):     {}/{} ({:.1}%)", ma_stack_bear, total, ma_stack_bear as f64/total as f64*100.0);
    println!("║ Full bear candidate:            {}/{} ({:.1}%)", full_bear_candidate, total, full_bear_candidate as f64/total as f64*100.0);
    println!("║ + bear_score >= 4:              {}/{} ({:.1}%)", bear_score_pass, total, bear_score_pass as f64/total as f64*100.0);
    println!("║ + 2-bar consecutive:            {}/{} ({:.1}%)", consecutive_bear, total, consecutive_bear as f64/total as f64*100.0);
    println!("║ + NO filter pass:               {}/{} ({:.1}%)", bear_no_filter_pass, total, bear_no_filter_pass as f64/total as f64*100.0);
    println!("╚══════════════════════════════════════════════════════════════════╝");
}
