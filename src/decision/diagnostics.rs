/// BULL signal search: test various momentum/buy-pressure indicators for BULL edge.
use crate::decision::candle::Candle;

fn taker_ratio(candles: &[Candle], t: usize, lookback: usize) -> f64 {
    let start = t.saturating_sub(lookback - 1);
    let mut buy_vol = 0.0_f64;
    let mut total_vol = 0.0_f64;
    for i in start..=t {
        buy_vol += candles[i].taker_buy_vol;
        total_vol += candles[i].volume;
    }
    if total_vol < 1e-12 { 0.50 } else { buy_vol / total_vol }
}

/// Count consecutive green candles ending at bar t
fn consecutive_greens(candles: &[Candle], t: usize) -> u32 {
    let mut count = 0u32;
    for i in (0..=t).rev() {
        if candles[i].close > candles[i].open {
            count += 1;
        } else {
            break;
        }
    }
    count
}

/// Count consecutive red candles ending at bar t
fn consecutive_reds(candles: &[Candle], t: usize) -> u32 {
    let mut count = 0u32;
    for i in (0..=t).rev() {
        if candles[i].close < candles[i].open {
            count += 1;
        } else {
            break;
        }
    }
    count
}

struct TestResult {
    label: String,
    bars: u32,
    green: u32,
    red: u32,
    win_rate: f64,
    pnl: f64,
    pnl_per_trade: f64,
}

fn test_signal<F>(candles: &[Candle], label: &str, min_bars: usize, predicate: F) -> TestResult
where F: Fn(&[Candle], usize) -> bool,
{
    let eval_start = 20;
    let mut bars = 0u32;
    let mut green = 0u32;
    let mut red = 0u32;

    for t in eval_start..candles.len() - 1 {
        if predicate(candles, t) {
            bars += 1;
            if candles[t + 1].close > candles[t + 1].open {
                green += 1;
            } else {
                red += 1;
            }
        }
    }

    let wr = if bars > 0 { green as f64 / bars as f64 * 100.0 } else { 0.0 };
    let pnl = (green as f64 * 0.495) - (red as f64 * 0.505);
    let ppt = if bars > 0 { pnl / bars as f64 } else { 0.0 };

    TestResult { label: label.to_string(), bars, green, red, win_rate: wr, pnl, pnl_per_trade: ppt }
}

pub fn run_diagnostics(candles: &[Candle]) {
    if candles.len() < 50 {
        eprintln!("Need at least 50 candles, got {}", candles.len());
        return;
    }

    let days = candles.len() as f64 / 96.0;

    println!("╔═════════════════════════════════════════════════════════════════════════════════════════════════════╗");
    println!("║                        BULL SIGNAL SEARCH                                                       ║");
    println!("║ {} bars ({:.1} days)  |  YES bet: cost=0.505  win=0.495 per unit                     ", candles.len(), days);
    println!("║ All signals: when trigger fires on bar t, bet YES on bar t+1                                    ║");
    println!("╠═════════════════════════════════════════════════════════════════════════════════════════════════════╣");
    println!("║ {:40} │ {:>5} {:>6} {:>6} {:>5} {:>7} {:>8} {:>6} ║",
        "SIGNAL", "BARS", "GREEN", "RED", "WR%", "PnL", "PnL/d", "PPT");
    println!("╠══════════════════════════════════╪═══════════════════════════════════╪═══════════════════════════════════╣");

    let mut results: Vec<TestResult> = Vec::new();

    // 1. CONSECUTIVE GREEN CANDLES
    for min_green in [2, 3, 4, 5, 6, 7, 8] {
        let label = format!("{}+ consec greens", min_green);
        results.push(test_signal(candles, &label, 100,
            move |c: &[Candle], t: usize| consecutive_greens(c, t) >= min_green));
    }

    // 2. TAKER RATIO (BUYER DOMINANCE)
    for ratio in [0.55, 0.57, 0.58, 0.60, 0.62, 0.65] {
        let lb = 4u32;
        let label = format!("taker ratio > {:.2} (lb={})", ratio, lb);
        results.push(test_signal(candles, &label, 100,
            move |c: &[Candle], t: usize| taker_ratio(c, t, lb as usize) > ratio));
    }

    // 3. TAKER RATIO (BUYER DOMINANCE) - longer lookback
    for lb in [6usize, 8, 12] {
        let label = format!("taker ratio > 0.58 (lb={})", lb);
        results.push(test_signal(candles, &label, 100,
            move |c: &[Candle], t: usize| taker_ratio(c, t, lb) > 0.58));
    }

    // 4. CLOSE ABOVE SMA7 (short-term trend)
    for sma_period in [5u32, 7, 10] {
        let label = format!("close > SMA{}(t-1)", sma_period);
        results.push(test_signal(candles, &label, 100,
            move |c: &[Candle], t: usize| {
                if t < sma_period as usize + 1 { return false; }
                let sma: f64 = ((t + 1 - sma_period as usize)..=t)
                    .map(|i| c[i].close).sum::<f64>() / sma_period as f64;
                c[t].close > sma
            }));
    }

    // 5. CLOSE > SMA7 AND CLOSE > SMA25 (trend alignment)
    results.push(test_signal(candles, "close > SMA7 & SMA25", 100,
        |c: &[Candle], t: usize| {
            if t < 25 { return false; }
            let sma7: f64 = (t-6..=t).map(|i| c[i].close).sum::<f64>() / 7.0;
            let sma25: f64 = (t-24..=t).map(|i| c[i].close).sum::<f64>() / 25.0;
            c[t].close > sma7 && c[t].close > sma25
        }));

    // 6. GREEN CANDLE + CLOSE ABOVE SMA7 (momentum + trend)
    results.push(test_signal(candles, "green + close > SMA7", 100,
        |c: &[Candle], t: usize| {
            let sma7: f64 = (t.saturating_sub(6)..=t).map(|i| c[i].close).sum::<f64>() / 7.0;
            c[t].close > c[t].open && c[t].close > sma7
        }));

    // 7. VOLUME EXPANSION ON GREEN CANDLE
    results.push(test_signal(candles, "green + vol > avg_vol", 100,
        |c: &[Candle], t: usize| {
            if t < 20 { return false; }
            let avg_vol: f64 = (t-19..=t).map(|i| c[i].volume).sum::<f64>() / 20.0;
            c[t].close > c[t].open && c[t].volume > avg_vol
        }));

    // 8. TAKER BUY DOMINANCE ON GREEN CANDLE
    results.push(test_signal(candles, "green + taker ratio > 0.55", 100,
        |c: &[Candle], t: usize| {
            c[t].close > c[t].open && c[t].volume > 1e-12 && (c[t].taker_buy_vol / c[t].volume) > 0.55
        }));

    // 9. 3+ CONSEC GREEN + TAKER RATIO > 0.55
    for ratio in [0.52, 0.55, 0.58] {
        let label = format!("3+ greens + taker > {:.2}", ratio);
        results.push(test_signal(candles, &label, 100,
            move |c: &[Candle], t: usize| {
                consecutive_greens(c, t) >= 3 && c[t].volume > 1e-12 && (c[t].taker_buy_vol / c[t].volume) > ratio
            }));
    }

    // 10. 3+ CONSEC GREEN + close > SMA7
    results.push(test_signal(candles, "3+ greens + close > SMA7", 100,
        |c: &[Candle], t: usize| {
            let sma7: f64 = (t.saturating_sub(6)..=t).map(|i| c[i].close).sum::<f64>() / 7.0;
            consecutive_greens(c, t) >= 3 && c[t].close > sma7
        }));

    // 11. CLOSE POSITION (strong close in upper portion)
    for threshold in [0.65, 0.70, 0.75, 0.80] {
        let label = format!("close_pos > {:.2}", threshold);
        results.push(test_signal(candles, &label, 100,
            move |c: &[Candle], t: usize| {
                let rng = c[t].high - c[t].low;
                if rng < 1e-12 { false } else { (c[t].close - c[t].low) / rng > threshold }
            }));
    }

    // 12. STRONG GREEN (close_pos > 0.70 + range > avg)
    results.push(test_signal(candles, "strong green (pos>0.7 + big range)", 100,
        |c: &[Candle], t: usize| {
            if t < 20 { return false; }
            let rng = c[t].high - c[t].low;
            let avg_rng: f64 = (t-19..=t).map(|i| c[i].high - c[i].low).sum::<f64>() / 20.0;
            rng > avg_rng && rng > 1e-12 && (c[t].close - c[t].low) / rng > 0.70 && c[t].close > c[t].open
        }));

    // Sort by PnL descending
    results.sort_by(|a, b| b.pnl.partial_cmp(&a.pnl).unwrap());

    for r in &results {
        println!("║ {:40} │ {:>5} {:>6} {:>6} {:>5.1} {:>+7.1} {:>+7.1} {:>+6.4} ║",
            r.label, r.bars, r.green, r.red, r.win_rate, r.pnl,
            r.pnl / days, r.pnl_per_trade);
    }

    // Best above break-even (50.5%)
    let profitable: Vec<_> = results.iter().filter(|r| r.win_rate > 50.5 && r.bars >= 200).collect();

    println!("╠═════════════════════════════════════════════════════════════════════════════════════════════════════╣");
    println!("║ PROFITABLE SIGNALS (win rate > 50.5%, min 200 bars)                                         ║");
    println!("╠═════════════════════════════════════════════════════════════════════════════════════════════════════╣");
    if profitable.is_empty() {
        println!("║   NONE found — no single signal achieves >50.5% YES win rate with enough volume                    ║");
        println!("║   This means BULL as a standalone directional bet is not viable with simple indicators             ║");
    } else {
        for r in profitable.iter().take(10) {
            println!("║   {:40}  {} bars  {:.1}%  PnL={:+.1}  ppt={:+.4}         ║",
                r.label, r.bars, r.win_rate, r.pnl, r.pnl_per_trade);
        }
    }

    println!("╚═════════════════════════════════════════════════════════════════════════════════════════════════════╝");
}
