use anyhow::Result;
use crate::decision::candle::Candle;
use tracing::info;

#[derive(Debug, Clone)]
struct FeatureRow {
    taker_ratio: f64,
    taker_ratio_momentum: f64,
    volume_vs_avg_20: f64,
    consecutive_red_bars: f64,
    green_red_ratio_8: f64,
    price_vs_sma7: f64,
    price_vs_sma25: f64,
    price_vs_sma99: f64,
    spread_7_25: f64,
    slope_25: f64,
    body_ratio: f64,
    upper_wick_ratio: f64,
    lower_wick_ratio: f64,
    next_candle_down: f64,
}

fn sma(candles: &[Candle], t: usize, period: usize) -> f64 {
    if t + 1 < period { return candles[t].close; }
    let start = t + 1 - period;
    let sum: f64 = (start..=t).map(|i| candles[i].close).sum();
    sum / period as f64
}

fn atr14(candles: &[Candle], t: usize) -> f64 {
    if t == 0 { return candles[0].range(); }
    let period = 14.min(t + 1);
    let start = t + 1 - period;
    let sum: f64 = (start..=t).map(|i| {
        candles[i].high - candles[i].low
            .max((candles[i].high - candles[i-1].close).abs())
            .max((candles[i].low - candles[i-1].close).abs())
    }).sum();
    sum / period as f64
}

fn compute_features(candles: &[Candle], t: usize) -> Option<FeatureRow> {
    if t < 100 || t + 1 >= candles.len() { return None; }

    let c = &candles[t];
    let vol = c.volume;
    let taker_buy = c.taker_buy_vol;
    let taker_total = vol; // approx: taker buy + taker sell ≈ volume
    let taker_ratio = if taker_total < 1e-12 { 0.5 } else { taker_buy / taker_total };

    let taker_ratio_prev = if t > 0 {
        let p = &candles[t - 1];
        let tt = p.volume;
        if tt < 1e-12 { 0.5 } else { p.taker_buy_vol / tt }
    } else { 0.5 };

    let vol_avg_20: f64 = (t.saturating_sub(19)..=t).map(|i| candles[i].volume).sum::<f64>() / 20.0;
    let volume_vs_avg_20 = if vol_avg_20 < 1e-12 { 1.0 } else { vol / vol_avg_20 };

    let mut consec_red = 0f64;
    for i in (0..t).rev() {
        if candles[i].is_red() { consec_red += 1.0; } else { break; }
    }

    let mut green_8 = 0u32;
    for i in t.saturating_sub(7)..=t {
        if candles[i].is_green() { green_8 += 1; }
    }
    let green_red_ratio_8 = green_8 as f64 / 8.0;

    let sma7 = sma(candles, t, 7);
    let sma25 = sma(candles, t, 25);
    let sma99 = sma(candles, t, 99);
    let price_vs_sma7 = (c.close - sma7) / sma7;
    let price_vs_sma25 = (c.close - sma25) / sma25;
    let price_vs_sma99 = (c.close - sma99) / sma99;

    let atr = atr14(candles, t);
    let atr_safe = if atr < 1e-12 { 1.0 } else { atr };

    let sma25_4 = if t >= 4 { sma(candles, t - 4, 25) } else { sma25 };
    let spread_7_25 = (sma7 - sma25) / atr_safe;
    let slope_25 = (sma25 - sma25_4) / atr_safe;

    let rng = c.range();
    let rng_safe = if rng < 1e-12 { 1e-12 } else { rng };
    let body_ratio = c.body() / rng_safe;
    let upper_wick_ratio = c.upper_wick() / rng_safe;
    let lower_wick_ratio = c.lower_wick() / rng_safe;

    // Target: did next candle go down?
    let next = &candles[t + 1];
    let next_candle_down = if next.close < next.open { 1.0 } else { 0.0 };

    Some(FeatureRow {
        taker_ratio, taker_ratio_momentum: taker_ratio - taker_ratio_prev,
        volume_vs_avg_20, consecutive_red_bars: consec_red, green_red_ratio_8,
        price_vs_sma7, price_vs_sma25, price_vs_sma99,
        spread_7_25, slope_25, body_ratio, upper_wick_ratio, lower_wick_ratio,
        next_candle_down,
    })
}

fn pearson(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len() as f64;
    if n < 2.0 { return 0.0; }
    let mx: f64 = x.iter().sum::<f64>() / n;
    let my: f64 = y.iter().sum::<f64>() / n;
    let num: f64 = x.iter().zip(y.iter()).map(|(a, b)| (a - mx) * (b - my)).sum();
    let dx: f64 = x.iter().map(|a| (a - mx).powi(2)).sum::<f64>().sqrt();
    let dy: f64 = y.iter().map(|a| (a - my).powi(2)).sum::<f64>().sqrt();
    if dx < 1e-12 || dy < 1e-12 { return 0.0; }
    num / (dx * dy)
}

fn percentile_idx(data: &[f64], p: f64) -> usize {
    let idx = ((p / 100.0) * (data.len() as f64 - 1.0)).round() as usize;
    idx.min(data.len() - 1)
}

fn threshold_analysis(values: &[f64], targets: &[f64], feature_name: &str, ascending: bool) {
    let mut paired: Vec<(f64, f64)> = values.iter().zip(targets.iter()).map(|(a,b)|(*a,*b)).collect();
    paired.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let sorted_vals: Vec<f64> = paired.iter().map(|(v, _)| *v).collect();
    let sorted_targets: Vec<f64> = paired.iter().map(|(_, t)| *t).collect();
    let n = sorted_vals.len();
    let baseline_win: f64 = targets.iter().sum::<f64>() / targets.len() as f64 * 100.0;

    println!("\n  ┌─ {} ─", feature_name);
    println!("  │ Base rate (all bars): {:.1}% down", baseline_win);

    let percentiles: &[f64] = &[5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 40.0];
    for &p in percentiles {
        let cutoff_idx = percentile_idx(&sorted_vals, p);
        let cutoff = sorted_vals[cutoff_idx];

        // For ascending features, "high value" means bottom percentile (low values predict down)
        // For descending features, "high value" means top percentile
        let (below, below_t): (Vec<_>, Vec<_>) = if ascending {
            // Low values of this feature → more likely down: take bottom p%
            sorted_vals[..=cutoff_idx].iter().zip(sorted_targets[..=cutoff_idx].iter())
                .map(|(v, t)| (*v, *t)).unzip()
        } else {
            // High values → more likely down: take top (100-p)%
            let start = n.saturating_sub(cutoff_idx + 1);
            sorted_vals[start..].iter().zip(sorted_targets[start..].iter())
                .map(|(v, t)| (*v, *t)).unzip()
        };

        if below.is_empty() { continue; }
        let win_rate: f64 = below_t.iter().sum::<f64>() / below_t.len() as f64 * 100.0;
        let edge = win_rate - baseline_win;

        let direction = if ascending { "≤" } else { "≥" };
        println!("  │ {:>4.0}th pct ({} {:.4}): {:>5.1}% WR, n={:>5}, edge={:+.1}pp",
            p, direction, cutoff, win_rate, below.len(), edge);
    }
    println!("  └");
}

pub fn run_analysis(candles: &[Candle]) -> Result<()> {
    if candles.len() < 200 {
        anyhow::bail!("Need at least 200 candles, got {}", candles.len());
    }

    println!("═══════════════════════════════════════════════════════════════");
    println!("  BEAR REGIME RESEARCH — Predicting next_candle_down");
    println!("═══════════════════════════════════════════════════════════════");

    let first_dt = chrono::DateTime::from_timestamp_millis(candles[0].open_time)
        .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string()).unwrap_or_default();
    let last_dt = chrono::DateTime::from_timestamp_millis(candles[candles.len()-1].open_time)
        .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string()).unwrap_or_default();
    let days = (candles[candles.len()-1].open_time - candles[0].open_time) as f64 / 86400000.0;
    println!("  Data: {} candles ({:.1} days) from {} to {}", candles.len(), days, first_dt, last_dt);
    println!();

    // Compute features for all valid bars
    let rows: Vec<FeatureRow> = (100..candles.len() - 1)
        .filter_map(|t| compute_features(candles, t))
        .collect();

    let n = rows.len();
    println!("  Usable sample: {} bars (need 100 lookback + 1 forward)", n);
    println!();

    if n < 100 {
        anyhow::bail!("Too few usable bars: {}", n);
    }

    // Base rate
    let down_count: f64 = rows.iter().map(|r| r.next_candle_down).sum();
    let base_rate = down_count / n as f64 * 100.0;
    println!("  Base rate: {:.1}% of bars are followed by a red (down) candle", base_rate);
    println!();

    // Extract feature vectors
    let feature_names = [
        ("taker_ratio", true),           // ascending = low ratio → down
        ("taker_ratio_momentum", true),  // negative momentum → down
        ("volume_vs_avg_20", false),     // descending = high volume can be either
        ("consecutive_red_bars", true),  // ascending = more consec red → down
        ("green_red_ratio_8", true),     // ascending = low green ratio → down
        ("price_vs_sma7", true),         // ascending = below sma → down (wait, this is (close-sma)/sma, so negative = below)
        ("price_vs_sma25", true),
        ("price_vs_sma99", true),
        ("spread_7_25", false),          // descending = sma7 below sma25 → bearish
        ("slope_25", false),             // descending = negative slope → bearish
        ("body_ratio", false),
        ("upper_wick_ratio", true),      // ascending = large upper wick → rejection → down
        ("lower_wick_ratio", false),     // descending = large lower wick → support → not down
    ];

    println!("─── Feature Correlations with next_candle_down ───");
    println!("{:>24}  corr     mean    std", "FEATURE");
    println!("{}", "─".repeat(62));

    let get_feature = |rows: &[FeatureRow], name: &str| -> Vec<f64> {
        match name {
            "taker_ratio" => rows.iter().map(|r| r.taker_ratio).collect(),
            "taker_ratio_momentum" => rows.iter().map(|r| r.taker_ratio_momentum).collect(),
            "volume_vs_avg_20" => rows.iter().map(|r| r.volume_vs_avg_20).collect(),
            "consecutive_red_bars" => rows.iter().map(|r| r.consecutive_red_bars).collect(),
            "green_red_ratio_8" => rows.iter().map(|r| r.green_red_ratio_8).collect(),
            "price_vs_sma7" => rows.iter().map(|r| r.price_vs_sma7).collect(),
            "price_vs_sma25" => rows.iter().map(|r| r.price_vs_sma25).collect(),
            "price_vs_sma99" => rows.iter().map(|r| r.price_vs_sma99).collect(),
            "spread_7_25" => rows.iter().map(|r| r.spread_7_25).collect(),
            "slope_25" => rows.iter().map(|r| r.slope_25).collect(),
            "body_ratio" => rows.iter().map(|r| r.body_ratio).collect(),
            "upper_wick_ratio" => rows.iter().map(|r| r.upper_wick_ratio).collect(),
            "lower_wick_ratio" => rows.iter().map(|r| r.lower_wick_ratio).collect(),
            _ => vec![],
        }
    };

    let targets: Vec<f64> = rows.iter().map(|r| r.next_candle_down).collect();

    let mut promising: Vec<(String, f64, bool)> = Vec::new();
    for (name, ascending) in &feature_names {
        let vals = get_feature(&rows, name);
        let corr = pearson(&vals, &targets);
        let mean = vals.iter().sum::<f64>() / vals.len() as f64;
        let variance: f64 = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / vals.len() as f64;
        let std = variance.sqrt();

        let marker = if corr.abs() > 0.03 { "★" } else { " " };
        println!("{} {:>24}  {:+.4}  {:+.6}  {:.6}", marker, name, corr, mean, std);

        if corr.abs() > 0.03 {
            promising.push((name.to_string(), corr, *ascending));
        }
    }

    println!();

    // Threshold analysis for promising features
    println!("─── Threshold Analysis for Promising Features (|corr| > 0.03) ───");
    for (name, _corr, ascending) in &promising {
        let vals = get_feature(&rows, name);
        // For negative correlations: low values predict down → ascending analysis
        // For positive correlations: high values predict down → descending analysis
        let is_asc = *_corr < 0.0;
        threshold_analysis(&vals, &targets, name, is_asc);
    }

    // Combined feature analysis
    println!();
    println!("─── Combined Feature Analysis ───");

    // Combo 1: Low taker ratio + low green/red ratio
    {
        let mut count = 0u32;
        let mut downs = 0u32;
        for r in &rows {
            if r.taker_ratio < 0.48 && r.green_red_ratio_8 < 0.375 {
                count += 1;
                downs += r.next_candle_down as u32;
            }
        }
        if count > 10 {
            let wr = downs as f64 / count as f64 * 100.0;
            println!("  taker_ratio < 0.48 AND green_red_ratio_8 < 0.375: {:.1}% WR, n={}", wr, count);
        } else {
            println!("  taker_ratio < 0.48 AND green_red_ratio_8 < 0.375: n={} (insufficient)", count);
        }
    }

    // Combo 2: Negative slope + below SMA25
    {
        let mut count = 0u32;
        let mut downs = 0u32;
        for r in &rows {
            if r.slope_25 < -0.3 && r.price_vs_sma25 < 0.0 {
                count += 1;
                downs += r.next_candle_down as u32;
            }
        }
        if count > 10 {
            let wr = downs as f64 / count as f64 * 100.0;
            println!("  slope_25 < -0.3 AND price < SMA25: {:.1}% WR, n={}", wr, count);
        }
    }

    // Combo 3: Low taker ratio + consecutive red
    {
        let mut count = 0u32;
        let mut downs = 0u32;
        for r in &rows {
            if r.taker_ratio < 0.47 && r.consecutive_red_bars >= 2.0 {
                count += 1;
                downs += r.next_candle_down as u32;
            }
        }
        if count > 10 {
            let wr = downs as f64 / count as f64 * 100.0;
            println!("  taker_ratio < 0.47 AND consec_red >= 2: {:.1}% WR, n={}", wr, count);
        }
    }

    // Combo 4: Low taker ratio + negative momentum
    {
        let mut count = 0u32;
        let mut downs = 0u32;
        for r in &rows {
            if r.taker_ratio < 0.47 && r.taker_ratio_momentum < -0.02 {
                count += 1;
                downs += r.next_candle_down as u32;
            }
        }
        if count > 10 {
            let wr = downs as f64 / count as f64 * 100.0;
            println!("  taker_ratio < 0.47 AND momentum < -0.02: {:.1}% WR, n={}", wr, count);
        }
    }

    // Combo 5: Bear triple — low taker, below sma25, negative spread
    {
        let mut count = 0u32;
        let mut downs = 0u32;
        for r in &rows {
            if r.taker_ratio < 0.48 && r.price_vs_sma25 < -0.001 && r.spread_7_25 < -0.5 {
                count += 1;
                downs += r.next_candle_down as u32;
            }
        }
        if count > 10 {
            let wr = downs as f64 / count as f64 * 100.0;
            println!("  taker<0.48 AND below_sma25 AND spread_7_25<-0.5: {:.1}% WR, n={}", wr, count);
        }
    }

    // Combo 6: Low green_red + upper wick rejection
    {
        let mut count = 0u32;
        let mut downs = 0u32;
        for r in &rows {
            if r.green_red_ratio_8 < 0.25 && r.upper_wick_ratio > 0.6 {
                count += 1;
                downs += r.next_candle_down as u32;
            }
        }
        if count > 10 {
            let wr = downs as f64 / count as f64 * 100.0;
            println!("  green_red_ratio_8 < 0.25 AND upper_wick > 0.6: {:.1}% WR, n={}", wr, count);
        }
    }

    // Statistical significance test for best combos
    println!();
    println!("─── Statistical Significance (binomial z-test vs base rate {:.1}%) ───", base_rate);

    fn z_test(wr_pct: f64, n: u32, p0: f64) -> f64 {
        let p = wr_pct / 100.0;
        let se = (p0 * (1.0 - p0) / n as f64).sqrt();
        if se < 1e-12 { return 0.0; }
        (p - p0) / se
    }

    let combos: Vec<(&str, f64, u32)> = vec![
        ("taker<0.48 & gr<0.375",
            rows.iter().filter(|r| r.taker_ratio < 0.48 && r.green_red_ratio_8 < 0.375)
                .map(|r| r.next_candle_down).sum::<f64>() /
            rows.iter().filter(|r| r.taker_ratio < 0.48 && r.green_red_ratio_8 < 0.375).count().max(1) as f64 * 100.0,
            rows.iter().filter(|r| r.taker_ratio < 0.48 && r.green_red_ratio_8 < 0.375).count() as u32),
        ("taker<0.47 & consec≥2",
            rows.iter().filter(|r| r.taker_ratio < 0.47 && r.consecutive_red_bars >= 2.0)
                .map(|r| r.next_candle_down).sum::<f64>() /
            rows.iter().filter(|r| r.taker_ratio < 0.47 && r.consecutive_red_bars >= 2.0).count().max(1) as f64 * 100.0,
            rows.iter().filter(|r| r.taker_ratio < 0.47 && r.consecutive_red_bars >= 2.0).count() as u32),
    ];

    let p0 = base_rate / 100.0;
    for (name, wr, count) in &combos {
        if *count < 10 { continue; }
        let z = z_test(*wr, *count, p0);
        let sig = if z.abs() > 2.576 { "*** (p<0.005)" }
            else if z.abs() > 1.96 { "** (p<0.05)" }
            else if z.abs() > 1.645 { "* (p<0.10)" }
            else { "n.s." };
        println!("  {:30}: {:.1}% WR n={:>5}  z={:+.2} {}", name, wr, count, z, sig);
    }

    println!();
    println!("═══════════════════════════════════════════════════════════════");
    println!("  OBI NOTE: Order book imbalance data requires live collection.");
    println!("  The obi_snapshots table is set up for forward data collection");
    println!("  via the OBI collector module. Historical OBI analysis will be");
    println!("  possible once sufficient snapshots accumulate.");
    println!("═══════════════════════════════════════════════════════════════");

    Ok(())
}
