mod bear;

use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use tracing::{info, warn};
use bear::trader::{BearDb, BearTrade};

const STARTING_BALANCE: f64 = 10_000.00;
const MAX_BET_PCT: f64 = 0.05;
const DEFAULT_ODDS: f64 = 0.505;
const POLY_FEE: f64 = 0.03;
const POLL_INTERVAL_SECS: u64 = 10;
const SUMMARY_BARS: u32 = 288; // 1 day

// BEAR v3: 1h Momentum Mean Reversion (no taker filter)
// Best backtested config: 55.8% WR, +$11,290 on 1,513 bets over 45 days
// When price moved > 0.8% over the last 1h (12 × 5m candles),
// bet OPPOSITE direction (mean reversion).
const MOM_LOOKBACK: usize = 12;  // 12 × 5min = 1 hour
const MOM_THRESHOLD: f64 = 0.8;  // 0.8% move triggers a signal

// Win rate for Kelly sizing (backtested 55.8%)
const WIN_RATE: f64 = 0.558;

#[derive(Parser, Debug)]
#[command(name = "bear_trader", about = "BEAR v3: 1h momentum mean-reversion paper trader")]
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

/// BEAR v3 strategy: 1h momentum mean reversion
/// Returns (decision, reason)
fn decide(momentum_pct: f64, _taker_ratio: f64) -> (&'static str, String) {
    if momentum_pct.abs() < MOM_THRESHOLD {
        return ("SKIP", format!("no edge (1h mom {:+.2}% < ±{:.1}%)", momentum_pct, MOM_THRESHOLD));
    }

    if momentum_pct > MOM_THRESHOLD {
        // 1h bullish move → bet DOWN (mean reversion)
        ("NO", format!("1h mom +{:.2}% → mean reversion DOWN", momentum_pct))
    } else {
        // 1h bearish move → bet UP (mean reversion)
        ("YES", format!("1h mom {:+.2}% → mean reversion UP", momentum_pct))
    }
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
    info!("🐻 BEAR v3 started | balance=${:.2} | 1h mom ±{:.1}% mean-reversion (no taker filter)", balance, MOM_THRESHOLD);

    // Resolve any pending trades first
    resolve_pending(&db, &mut balance)?;

    let mut last_candle_open: i64 = 0;
    let mut bar_count = 0u32;

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS)).await;

        let now_ms = chrono::Utc::now().timestamp_millis();

        // Resolve pending trades
        resolve_pending(&db, &mut balance)?;

        // Get latest closed candle
        let candle = match db.get_latest_closed_candle(now_ms)? {
            Some(c) => c,
            None => continue,
        };

        if candle.open_time == last_candle_open {
            continue;
        }

        // Skip if we already traded this candle
        if db.has_trade_for_candle(candle.open_time)? {
            last_candle_open = candle.open_time;
            continue;
        }

        last_candle_open = candle.open_time;

        // Compute 1h momentum (price change over last 12 candles)
        let momentum = match db.momentum_pct(candle.open_time, MOM_LOOKBACK)? {
            Some(m) => m,
            None => {
                info!("Candle {} — insufficient history for momentum, SKIP", candle.open_time);
                skip_candle(&db, candle.open_time, 0.0, 0.0, balance)?;
                continue;
            }
        };

        let taker = db.taker_ratio(candle.open_time, 4)?; // logged for analysis, not used in decision

        let (decision, reason) = decide(momentum, taker);

        let trade = BearTrade {
            id: None,
            candle_open_time: candle.open_time,
            decision: decision.into(),
            obi5: momentum,      // repurpose obi5 to store momentum %
            obi10: 0.0,
            obi20: 0.0,
            obi5_late: Some(taker), // repurpose to store taker ratio
            obi5_early: None,
            obi5_momentum: None,
            bid_ask_ratio_5: None,
            spread: None,
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
            info!("Candle {} — 1h_mom={:+.3}% taker={:.3} → SKIP ({})", 
                candle.open_time, momentum, taker, reason);
        } else {
            info!("Candle {} — 1h_mom={:+.3}% taker={:.3} → {} | stake=${:.2} @ {:.3}x | {}",
                candle.open_time, momentum, taker, decision, trade.stake, DEFAULT_ODDS, reason);
        }

        bar_count += 1;
        if bar_count % SUMMARY_BARS == 0 {
            db.print_summary(STARTING_BALANCE)?;
        }
    }
}

fn skip_candle(db: &BearDb, candle_open_time: i64, mom: f64, taker: f64, balance: f64) -> Result<()> {
    db.insert_trade(&BearTrade {
        id: None,
        candle_open_time,
        decision: "SKIP".into(),
        obi5: mom,
        obi10: 0.0, obi20: 0.0,
        obi5_late: Some(taker),
        obi5_early: None, obi5_momentum: None,
        bid_ask_ratio_5: None, spread: None,
        stake: 0.0, odds: DEFAULT_ODDS,
        entry_price: None,
        exit_price: None, outcome: None, pnl: None,
        balance_at_entry: balance,
    })?;
    Ok(())
}

fn resolve_pending(db: &BearDb, balance: &mut f64) -> Result<()> {
    let trades = db.get_unresolved_trades()?;
    let now_ms = chrono::Utc::now().timestamp_millis();

    for mut trade in trades {
        let next_open_time = trade.candle_open_time + 300_000;

        let next_candle = match db.get_candle(next_open_time)? {
            Some(c) => c,
            None => {
                if now_ms > next_open_time + 600_000 {
                    warn!("Trade {} — next candle {} not found after 10min, marking SKIP", trade.id.unwrap(), next_open_time);
                    db.update_trade_outcome(trade.id.unwrap(), "SKIP", 0.0, 0.0)?;
                    continue;
                }
                continue;
            }
        };

        // Skip if next candle has zero volume (data gap)
        let next_has_vol = {
            let conn = db.conn.lock().unwrap();
            let vol: f64 = conn.query_row(
                "SELECT COALESCE(volume, 0) FROM candles WHERE open_time = ?1",
                rusqlite::params![next_open_time], |r| r.get(0)
            ).unwrap_or(0.0);
            vol > 0.0
        };
        if !next_has_vol {
            warn!("Trade {} — next candle {} has zero volume (data gap), marking SKIP", trade.id.unwrap(), next_open_time);
            db.update_trade_outcome(trade.id.unwrap(), "SKIP", 0.0, 0.0)?;
            continue;
        }

        let next_open = next_candle.open;
        let next_close = next_candle.close;

        // FLAT candle (open == close) → SKIP
        if (next_close - next_open).abs() < 0.01 {
            warn!("Trade {} — next candle is FLAT, marking SKIP", trade.id.unwrap());
            db.update_trade_outcome(trade.id.unwrap(), "SKIP", 0.0, next_close)?;
            continue;
        }

        let went_up = next_close > next_open;
        let won = (trade.decision == "YES" && went_up) || (trade.decision == "NO" && !went_up);

        // PnL includes 3% PolyMarket fee
        let pnl = if won {
            let gross = trade.stake / DEFAULT_ODDS - trade.stake;
            let fee = trade.stake * POLY_FEE;
            gross - fee
        } else {
            -trade.stake - trade.stake * POLY_FEE
        };

        *balance += pnl;
        db.set_balance(*balance)?;
        db.update_trade_outcome(trade.id.unwrap(), if won { "WIN" } else { "LOSS" }, pnl, next_close)?;

        let emoji = if won { "✅" } else { "❌" };
        info!("{} Trade {} resolved: {} {} | PnL=${:.2} | bal=${:.2} | mom={:+.3}%",
            emoji, trade.id.unwrap(), trade.decision, if won { "WIN" } else { "LOSS" }, 
            pnl, *balance, trade.obi5);
    }

    Ok(())
}
