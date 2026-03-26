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
use crate::clob::clob::GammaClient;
use crate::decision::decision::Thresholds;

const STARTING_BALANCE: f64 = 10_000.00;
const MAX_BET_PCT: f64 = 0.05;
const DEFAULT_ODDS: f64 = 0.505;
const WIN_RATE: f64 = 0.555;  // from 5m backtest

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
        bootstrap_5m(&db).await?;
        return Ok(());
    }

    if args.simulate {
        return simulate_5m(&db, &tracker);
    }

    // ═════════════════════════════════════════════════════════════════
    // LIVE: 5m BULL YES paper trader
    // ═══════════════════════════════════════════════════════════════
    let th = Thresholds::default();
    let gamma = GammaClient::new();
    let mut rx = binance::feed::spawn_feed(db.clone()).await?;

    let mut in_bull = false;
    let mut current_stake: f64 = 0.0;
    let mut current_odds: f64 = DEFAULT_ODDS;
    let mut bar_count = 0u32;

    info!("BULL-ONLY 5M PAPER TRADER | taker ratio > {:.2} | lb={} | bal=${:.2}",
        th.bull_ratio, th.lookback, tracker.kelly.balance);

    loop {
        if rx.changed().await.is_err() {
            warn!("Feed closed");
            break;
        }

        let event = rx.borrow().clone();
        let Some(event) = event else { continue };

        match event {
            FeedEvent::Closed(candle) => {
                bar_count += 1;

                // 1. Resolve previous block's trade
                if current_stake > 0.0 {
                    match tracker.resolve_trade_by_block(
                        candle.open_time, candle.open, candle.close,
                        current_stake, current_odds,
                    ) {
                        Ok(Some((outcome, _))) => {
                            info!("  RESOLVED block={}: {} bal={:.2}",
                                candle.open_time, outcome, tracker.kelly.balance);
                        }
                        _ => {}
                    }
                }

                // 2. Evaluate regime on closed bar, prepare bet for next block
                let candles = db.get_latest_candles(th.lookback + 5)?;
                if candles.len() < th.lookback + 2 { continue; }

                let t = candles.len() - 1;
                let (_, ratio, _) = decision::decision::evaluate(
                    &candles, t, in_bull, 0, &th,
                );

                // BULL entry: taker ratio exceeds threshold for 2+ bars
                if !in_bull && ratio > th.bull_ratio {
                    info!("BULL ACTIVATED | ratio={:.3}", ratio);
                }
                // BULL exit: ratio drops below 0.50 for 2+ bars
                if in_bull && ratio < 0.50 {
                    info!("BULL EXITED | ratio={:.3}", ratio);
                    in_bull = false;
                }

                // 3. Place bet if in BULL
                if in_bull {
                    let next_block_open = candle.open_time + 300_000; // 5min
                    let odds = match tokio::time::timeout(
                        std::time::Duration::from_secs(3),
                        gamma.get_odds(next_block_open)
                    ).await {
                        Ok(Ok(m)) => m.yes_best_ask,  // pay ask for YES
                        Ok(Err(e)) => { warn!("CLOB err: {}, using default", e); DEFAULT_ODDS }
                        Err(_) => { warn!("CLOB timeout"); DEFAULT_ODDS }
                    };

                    let stake = tracker.kelly.size(WIN_RATE, odds);
                    if stake > 0.01 {
                        tracker.record_bet(
                            next_block_open, next_block_open + 299_999,
                            candle.close, "YES", Some(odds), ratio, stake,
                        )?;
                        current_stake = stake;
                        current_odds = odds;
                    } else {
                        current_stake = 0.0;
                    }
                } else {
                    current_stake = 0.0;
                }

                // 4. Periodic summary (every 288 bars = 1 day on 5m)
                if bar_count % 288 == 0 {
                    tracker.print_summary()?;
                }
            }
            FeedEvent::Update(_) => {}
        }
    }

    tracker.print_summary()?;
    Ok(())
}

fn simulate_5m(db: &Arc<Db>, tracker: &PaperTracker) -> Result<()> {
    info!("5m BULL simulation...");
    let candles = db.get_latest_candles(5000)?;
    if candles.len() < 20 {
        anyhow::bail!("Not enough candles");
    }

    let sim_window = 7 * 288; // 7 days of 5m bars
    let start = candles.len().saturating_sub(sim_window);
    let eval_start = start.max(10);
    let total = candles.len() - eval_start - 1;

    info!("Processing {} bars ({:.1f} days)", total, total as f64 / 288.0);

    let th = Thresholds::default();
    let mut in_bull = false;
    let mut bets = 0u32;
    let mut wins = 0u32;

    for t in eval_start..candles.len() - 1 {
        let (_, ratio, _) = decision::decision::evaluate(&candles, t, in_bull, 0, &th);

        if !in_bull && ratio > th.bull_ratio {
            in_bull = true;
        }
        if in_bull && ratio < 0.50 {
            in_bull = false;
        }

        if in_bull {
            let next = &candles[t + 1];
            let stake = tracker.kelly.size(WIN_RATE, DEFAULT_ODDS);
            if stake > 0.01 {
                tracker.record_bet(
                    next.open_time, next.open_time + 299_999,
                    candles[t].close, "YES", Some(DEFAULT_ODDS), ratio, stake,
                )?;
                match tracker.resolve_trade_by_block(
                    next.open_time, next.open, next.close, stake, DEFAULT_ODDS,
                ) {
                    Ok(Some((outcome, _))) => { if outcome == "WIN" { wins += 1; } }
                    _ => {}
                }
                bets += 1;
            }
        }
    }

    info!("SIMULATION: {} bets, {} wins ({:.1}%)", bets, wins, if bets > 0 { wins as f64 / bets as f64 * 100.0 } else { 0.0 });
    tracker.print_summary()?;
    Ok(())
}

async fn bootstrap_5m(db: &Arc<Db>) -> Result<()> {
    let mut all = Vec::new();
    let mut end: Option<i64> = None;

    for _ in 0..5 {
        let mut url = "https://api.binance.us/api/v3/klines?symbol=BTCUSDT&interval=5m&limit=1000".to_string();
        if let Some(et) = end {
            url.push_str(&format!("&endTime={}", et));
        }
        let resp: Vec<Vec<serde_json::Value>> =
            reqwest::get(&url).await?.json().await?;
        if resp.is_empty() { break; }

        let first_open = resp[0][0].as_i64().unwrap_or(0);
        for r in &resp {
            if r.len() < 10 { continue; }
            all.push(crate::decision::candle::Candle {
                open_time: r[0].as_i64().unwrap_or(0),
                close_time: r[6].as_i64().unwrap_or(0),
                open: r[1].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                high: r[2].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                low: r[3].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                close: r[4].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                volume: r[5].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                taker_buy_vol: r[9].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                trades: r[8].as_u64().unwrap_or(0) as u32,
            });
        }
        end = Some(first_open - 1);
    }

    info!("Fetched {} candles", all.len());
    db.upsert_candles_batch(&all)?;
    info!("DB: {} candles", db.get_latest_candles(10000)?.len());
    Ok(())
}
