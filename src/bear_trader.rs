mod bear;

use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use tracing::{info, warn, error};
use bear::trader::{BearDb, BearTrade};

const STARTING_BALANCE: f64 = 10_000.00;
const MAX_BET_PCT: f64 = 0.05;
const DEFAULT_ODDS: f64 = 0.505;
const WIN_RATE: f64 = 0.555;
const POLL_INTERVAL_SECS: u64 = 10;
const SUMMARY_BARS: u32 = 288; // 1 day

#[derive(Parser, Debug)]
#[command(name = "bear_trader", about = "OBI-based BEAR paper trader")]
struct Args {
    #[arg(long, default_value = "data/predictor.db")]
    db_path: String,

    #[arg(long, default_value = "10000.0")]
    balance: f64,

    #[arg(long, default_value = "info")]
    log_level: String,

    #[arg(long)]
    summary: bool,

    #[arg(long)]
    reset: bool,
}

fn kelly_stake(balance: f64, win_rate: f64, odds: f64) -> f64 {
    let q = 1.0 - win_rate;
    let kelly = (win_rate * (odds - 1.0) - q) / (odds - 1.0);
    let half_kelly = kelly.max(0.0) * 0.5;
    let raw = balance * half_kelly;
    let max = balance * MAX_BET_PCT;
    raw.min(max)
}

/// BEAR v2 strategy: OBI zones + taker ratio validation
/// Returns (decision, reason)
fn decide(obi5_avg: f64, obi5_momentum: f64, taker_ratio: f64) -> (&'static str, &'static str) {
    let bear_taker = 0.45;  // taker below this = selling pressure
    let bull_taker = 0.65;  // taker above this = strong buying pressure

    // YES bets: exhaustion bounce zone + confirmed buying
    if obi5_avg >= -0.41 && obi5_avg < -0.35 && taker_ratio >= bull_taker {
        return ("YES", "bounce zone + taker confirms buying");
    }

    // NO bets: neutral OBI + bearish taker (real selling flow)
    if obi5_avg >= -0.35 && taker_ratio < bear_taker {
        return ("NO", "neutral OBI + bearish taker flow");
    }

    // NO bets: fake OBI bounce + bearish taker
    if obi5_momentum > 0.1 && taker_ratio < bear_taker {
        return ("NO", "fake OBI bounce + bearish taker flow");
    }

    ("SKIP", "no edge")
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| args.log_level.clone().into()),
        )
        .init();

    std::fs::create_dir_all("data")?;

    let db = Arc::new(BearDb::new(&args.db_path)?);

    if args.summary {
        db.print_summary(STARTING_BALANCE)?;
        return Ok(());
    }

    if args.reset {
        db.set_balance(args.balance)?;
        info!("Balance reset to ${:.2}", args.balance);
        db.print_summary(args.balance)?;
        return Ok(());
    }

    let mut balance = db.get_balance(args.balance)?;
    info!("🐻 BEAR TRADER v2 started | balance=${:.2} | OBI + Taker strategy", balance);

    // Resolve any pending trades first
    resolve_pending(&db, &mut balance)?;

    let mut last_candle_open: i64 = 0;
    let mut bar_count = 0u32;

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS)).await;

        let now_ms = chrono::Utc::now().timestamp_millis();

        // Resolve pending trades first
        resolve_pending(&db, &mut balance)?;

        // Check for new candle
        let candle = match db.get_latest_closed_candle(now_ms)? {
            Some(c) => c,
            None => continue,
        };

        if candle.open_time == last_candle_open {
            continue;
        }

        // Check if we already traded this candle
        if db.has_trade_for_candle(candle.open_time)? {
            last_candle_open = candle.open_time;
            continue;
        }

        last_candle_open = candle.open_time;

        // Get OBI data
        let obi = match db.get_obi_for_candle(candle.open_time)? {
            Some(o) if o.count >= 10 => o,
            _ => {
                info!("Candle {} — insufficient OBI data, SKIP", candle.open_time);
                db.insert_trade(&BearTrade {
                    id: None,
                    candle_open_time: candle.open_time,
                    decision: "SKIP".into(),
                    obi5: 0.0, obi10: 0.0, obi20: 0.0,
                    obi5_late: None, obi5_early: None, obi5_momentum: None,
                    bid_ask_ratio_5: None, spread: None,
                    stake: 0.0, odds: DEFAULT_ODDS,
                    entry_price: Some(candle.close),
                    exit_price: None, outcome: None, pnl: None,
                    balance_at_entry: balance,
                })?;
                continue;
            }
        };

        // Compute OBI momentum (late - early)
        let obi_momentum = match (obi.obi5_late, obi.obi5_early) {
            (Some(late), Some(early)) => late - early,
            _ => 0.0,
        };

        // Get taker ratio (4-bar lookback)
        let taker = db.taker_ratio(candle.open_time, 4)?;

        let (decision, reason) = decide(obi.obi5_avg, obi_momentum, taker);

        let trade = BearTrade {
            id: None,
            candle_open_time: candle.open_time,
            decision: decision.into(),
            obi5: obi.obi5_avg,
            obi10: obi.obi10_avg,
            obi20: obi.obi20_avg,
            obi5_late: obi.obi5_late,
            obi5_early: obi.obi5_early,
            obi5_momentum: Some(obi_momentum),
            bid_ask_ratio_5: if obi.ask_vol5_avg > 0.0 { Some(obi.bid_vol5_avg / obi.ask_vol5_avg) } else { None },
            spread: Some(obi.spread_avg),
            stake: if decision == "SKIP" { 0.0 } else { kelly_stake(balance, WIN_RATE, DEFAULT_ODDS) },
            odds: DEFAULT_ODDS,
            entry_price: Some(candle.close),
            exit_price: None,
            outcome: None,
            pnl: None,
            balance_at_entry: balance,
        };

        let trade_id = db.insert_trade(&trade)?;

        if decision == "SKIP" {
            info!("Candle {} — OBI5={:.4} Taker={:.3} → SKIP ({})", candle.open_time, obi.obi5_avg, taker, reason);
        } else {
            info!("Candle {} — OBI5={:.4} Taker={:.3} Mom={:+.4} → {} | stake=${:.2} @ {:.3}x | {}",
                candle.open_time, obi.obi5_avg, taker, obi_momentum, decision, trade.stake, DEFAULT_ODDS, reason);
        }

        bar_count += 1;
        if bar_count % SUMMARY_BARS == 0 {
            db.print_summary(STARTING_BALANCE)?;
        }
    }
}

fn resolve_pending(db: &BearDb, balance: &mut f64) -> Result<()> {
    let trades = db.get_unresolved_trades()?;
    let now_ms = chrono::Utc::now().timestamp_millis();

    for mut trade in trades {
        // The next candle after this trade's candle
        // Next candle open_time = trade.candle_open_time + 300_000
        let next_open_time = trade.candle_open_time + 300_000;

        let next_candle = match db.get_candle(next_open_time)? {
            Some(c) => c,
            None => {
                // Not enough time passed yet
                if now_ms > next_open_time + 600_000 {
                    // 10 min after expected close, still no candle — skip
                    warn!("Trade {} — next candle {} not found after 10min, marking SKIP", trade.id.unwrap(), next_open_time);
                    db.update_trade_outcome(trade.id.unwrap(), "SKIP", 0.0, 0.0)?;
                    continue;
                }
                continue;
            }
        };

        let next_open = next_candle.open;
        let next_close = next_candle.close;
        let went_up = next_close >= next_open;

        let won = (trade.decision == "YES" && went_up) || (trade.decision == "NO" && !went_up);
        let outcome = if won { "WIN" } else { "LOSS" };
        let pnl = if won { trade.stake / DEFAULT_ODDS - trade.stake } else { -trade.stake };

        *balance += pnl;
        db.set_balance(*balance)?;
        db.update_trade_outcome(trade.id.unwrap(), outcome, pnl, next_close)?;

        let emoji = if won { "✅" } else { "❌" };
        info!("{} Trade {} resolved: {} {} | PnL=${:.2} | bal=${:.2} | OBI5={:.4}",
            emoji, trade.id.unwrap(), trade.decision, outcome, pnl, *balance, trade.obi5);
    }

    Ok(())
}
