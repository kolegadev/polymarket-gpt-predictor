mod binance;
mod clob;
mod config;
mod db;
mod decision;
mod paper;
mod types;

use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use tracing::{info, warn};
use db::Db;
use paper::tracker::{PaperTracker, KellySizer};
use binance::feed::FeedEvent;
use crate::config::config::Args;
use crate::decision::decision::Thresholds;

const STARTING_BALANCE: f64 = 10_000.00;
const MAX_BET_PCT: f64 = 0.05;       // 5% of current balance
const DEFAULT_ODDS: f64 = 0.505;    // average opening odds

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

    let db = Arc::new(Db::new(&args.db_path)?);
    let tracker = PaperTracker::new(db.clone(), STARTING_BALANCE);

    if args.summary {
        tracker.print_summary()?;
        return Ok(());
    }

    if args.bootstrap {
        info!("Bootstrapping historical candles from Binance.US REST...");
        bootstrap_large(&db).await?;
        return Ok(());
    }

    if args.simulate {
        return run_simulation(&db, &tracker);
    }

    // ═══════════════════════════════════════════════════════════════
    // LIVE PAPER TRADING MODE
    // ═══════════════════════════════════════════════════════════════
    let th = Thresholds::default();
    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║     🐻 BEAR-ONLY BTC 15M PAPER TRADER                          ║");
    info!("║     Taker ratio < {:.2} for {}+ bars → bet NO               ║", th.bear_ratio, th.confirm_bars);
    info!("║     Kelly sizing | Max 5% per bet | Bal: ${:.2}              ║", tracker.kelly.balance);
    info!("╚══════════════════════════════════════════════════════════════╝");

    let mut rx = binance::feed::spawn_feed(db.clone()).await?;

    let mut in_bear = false;
    let mut consecutive_bear = 0u32;
    let mut pending_stake: f64 = 0.0;
    let mut pending_block: i64 = 0;

    // Print summary every 96 bars (~24h)
    let mut bar_count = 0u32;

    loop {
        if rx.changed().await.is_err() {
            warn!("Feed channel closed, exiting");
            break;
        }

        let event = rx.borrow().clone();
        let Some(event) = event else { continue };

        match event {
            FeedEvent::Closed(candle) => {
                bar_count += 1;
                let candles = db.get_latest_candles(20)?;
                if candles.len() < 10 { continue; }

                let t = candles.len() - 1;
                let (still_bear, ratio, consec) = decision::decision::evaluate(
                    &candles, t, in_bear, consecutive_bear, &th,
                );

                // Detect BEAR entry — prepare bet for next block
                if !in_bear && still_bear && consec >= th.confirm_bars {
                    // Calculate Kelly stake
                    let win_prob = 0.57; // from backtest
                    let stake = tracker.kelly.size(win_prob, DEFAULT_ODDS);
                    if stake > 0.01 {
                        info!("🐻 BEAR ACTIVATED | ratio={:.3} consec={} kelly_stake=${:.2}", ratio, consec, stake);
                        pending_stake = stake;
                        pending_block = candle.open_time + 900_000; // next block
                    }
                }

                // Detect BEAR exit
                if in_bear && !still_bear && consec == 0 {
                    info!("📈 BEAR EXITED | ratio={:.3}", ratio);
                }

                in_bear = still_bear;
                consecutive_bear = consec;

                // Resolve previous block's trade
                if pending_block > 0 && pending_stake > 0.0 {
                    // The block that just closed IS the pending block
                    // Its open_time was candle.open_time
                    // But pending_block was set to the NEXT block's open_time
                    // We need to resolve when that block closes
                    // Actually, let's resolve the block that matches pending_block
                    // which is the block we placed the bet for

                    // The pending_block is the open_time of the bar we bet on
                    // candle.open_time is the bar that just closed
                    // If candle.open_time == pending_block, this is our bet
                    if candle.open_time == pending_block {
                        match tracker.resolve_trade_by_block(pending_block, candle.close, pending_stake, DEFAULT_ODDS) {
                            Ok(Some((outcome, pnl))) => {
                                info!("  ✅ {} stake=${:.2} pnl={:+.2} bal={:.2} close={:.2}",
                                    outcome, pending_stake, pnl, tracker.kelly.balance, candle.close);
                            }
                            Ok(None) => {
                                warn!("  ⚠️ No trade found to resolve for block {}", pending_block);
                            }
                            Err(e) => {
                                warn!("  ⚠️ Resolve error: {}", e);
                            }
                        }
                        pending_stake = 0.0;
                        pending_block = 0;
                    }
                }

                // Place bet for next block if in BEAR
                if in_bear {
                    let win_prob = 0.57;
                    let stake = tracker.kelly.size(win_prob, DEFAULT_ODDS);
                    if stake > 0.01 {
                        let next_block = candle.open_time + 900_000;
                        pending_stake = stake;
                        pending_block = next_block;

                        let _ = tracker.record_bet(
                            next_block,
                            next_block + 899_999,
                            candle.close,
                            "NO",
                            Some(DEFAULT_ODDS),
                            ratio,
                            stake,
                        );
                    }
                }

                // Periodic summary
                if bar_count % 96 == 0 {
                    tracker.print_summary()?;
                }
            }
            FeedEvent::Update(_) => {}
        }
    }

    tracker.print_summary()?;
    Ok(())
}

fn run_simulation(db: &Arc<Db>, tracker: &PaperTracker) -> Result<()> {
    info!("📊 Running BEAR-only simulation (7-day window)...");

    let candles = db.get_latest_candles(10000)?;
    if candles.len() < 20 {
        anyhow::bail!("Not enough candles, got {}", candles.len());
    }

    let sim_window = 7 * 96;
    let sim_start = candles.len().saturating_sub(sim_window);
    let eval_start = sim_start.max(10);
    let total = candles.len() - eval_start - 1;

    info!("Processing {} bars ({:.1} days)", total, total as f64 / 96.0);

    let th = Thresholds::default();
    let mut in_bear = false;
    let mut consecutive_bear = 0u32;
    let mut bets = 0u32;
    let mut wins = 0u32;

    for t in eval_start..candles.len() - 1 {
        let (still_bear, ratio, consec) = decision::decision::evaluate(
            &candles, t, in_bear, consecutive_bear, &th,
        );

        in_bear = still_bear;
        consecutive_bear = consec;

        if in_bear {
            let win_prob = 0.57;
            let stake = tracker.kelly.size(win_prob, DEFAULT_ODDS);
            if stake > 0.01 {
                let next = &candles[t + 1];
                let _ = tracker.record_bet(
                    next.open_time, next.close_time, candles[t].close,
                    "NO", Some(DEFAULT_ODDS), ratio, stake,
                );
                match tracker.resolve_trade_by_block(next.open_time, next.close, stake, DEFAULT_ODDS) {
                    Ok(Some((outcome, _))) => {
                        if outcome == "WIN" { wins += 1; }
                    }
                    _ => {}
                }
                bets += 1;
            }
        }
    }

    info!("─────────────────────────────────────────");
    info!("SIMULATION: {} bets, {} wins ({:.1}%)", bets, wins, if bets > 0 { wins as f64 / bets as f64 * 100.0 } else { 0.0 });
    info!("─────────────────────────────────────────");

    tracker.print_summary()?;
    Ok(())
}

async fn bootstrap_large(db: &Arc<Db>) -> Result<()> {
    let mut all_candles = Vec::new();
    let mut end_time: Option<i64> = None;

    for _ in 0..10 {
        let mut url = format!(
            "https://api.binance.us/api/v3/klines?symbol=BTCUSDT&interval=15m&limit=1000"
        );
        if let Some(et) = end_time {
            url.push_str(&format!("&endTime={}", et));
        }

        let resp = reqwest::get(&url).await?.json::<Vec<Vec<serde_json::Value>>>().await?;
        if resp.is_empty() { break; }

        let first_open = resp[0][0].as_i64().unwrap_or(0);
        for row in &resp {
            if row.len() < 10 { continue; }
            all_candles.push(crate::decision::candle::Candle {
                open_time: row[0].as_i64().unwrap_or(0),
                close_time: row[6].as_i64().unwrap_or(0),
                open: row[1].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                high: row[2].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                low: row[3].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                close: row[4].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                volume: row[5].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                taker_buy_vol: row[9].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                trades: row[8].as_u64().unwrap_or(0) as u32,
            });
        }
        end_time = Some(first_open - 1);
    }

    info!("Fetched {} candles", all_candles.len());
    db.upsert_candles_batch(&all_candles)?;
    info!("DB now has {} candles", db.get_latest_candles(10000)?.len());
    Ok(())
}
