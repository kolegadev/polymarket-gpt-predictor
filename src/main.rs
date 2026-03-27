mod analysis;
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
use paper::tracker::PaperTracker;
use binance::feed::FeedEvent;
use crate::clob::clob::GammaClient;
use crate::decision::decision::Thresholds;
use crate::config::config::Args;

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
    let mut tracker = PaperTracker::new(db.clone(), STARTING_BALANCE);

    if args.summary {
        tracker.print_summary()?;
        return Ok(());
    }

    if args.reset {
        tracker.reset(STARTING_BALANCE)?;
        tracker.print_summary()?;
        return Ok(());
    }

    if args.research {
        let candles = db.get_latest_candles(50000)?;
        return analysis::bear_research::run_analysis(&candles);
    }

    if args.expand_data {
        expand_historical_data(&db).await?;
        return Ok(());
    }

    if args.collect_obi {
        let db_clone = db.clone();
        info!("Starting OBI collector (1s intervals)...");
        let handle = binance::obi::spawn_collector(db_clone, 1);
        // Run forever — the collector is a background task
        tokio::signal::ctrl_c().await.ok();
        info!("Shutting down OBI collector...");
        handle.abort();
        return Ok(());
    }

    if args.bootstrap {
        bootstrap_5m(&db).await?;
        return Ok(());
    }

    if args.simulate {
        return simulate_5m(&db, &mut tracker);
    }

    // ═════════════════════════════════════════════════════════════════
    // LIVE: 5m BULL YES paper trader
    // ═══════════════════════════════════════════════════════════════
    let th = Thresholds::default();
    let gamma = GammaClient::new();
    let mut rx = binance::feed::spawn_feed(db.clone()).await?;

    let mut in_bull = false;
    let mut consec_bull: u32 = 0;
    let mut consec_exit: u32 = 0;
    let mut current_stake: f64 = 0.0;
    let mut current_odds: f64 = DEFAULT_ODDS;
    let mut bar_count = 0u32;

    info!("BULL-ONLY 5M PAPER TRADER | entry> {:.2} | exit< {:.2} | lb={} | confirm={} | bal=${:.2}",
        th.bull_ratio, th.exit_ratio, th.lookback, th.confirm_bars, tracker.kelly.balance);

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
                let was_bull = in_bull;
                let (still_bull, ratio, cb, ce) = decision::decision::evaluate(
                    &candles, t, in_bull, consec_bull, consec_exit, &th,
                );

                if !was_bull && still_bull {
                    info!("BULL ENTERED | ratio={:.3} consec={}", ratio, cb);
                }
                if was_bull && !still_bull {
                    info!("BULL EXITED | ratio={:.3} exit_consec={}", ratio, ce);
                }

                in_bull = still_bull;
                consec_bull = cb;
                consec_exit = ce;

                // 3. Place bet if in BULL
                if in_bull {
                    let next_block_open = candle.open_time + 300_000; // 5min
                    let odds = match tokio::time::timeout(
                        std::time::Duration::from_secs(3),
                        gamma.get_odds(next_block_open)
                    ).await {
                        Ok(Ok(m)) => m.yes_best_ask,  // pay ask for YES
                        Ok(Err(e)) => { warn!("CLOB err: {}, using default", e); DEFAULT_ODDS }
                        Err(_) => { warn!("CLOB timeout, using default"); DEFAULT_ODDS }
                    };

                    let stake = tracker.kelly.size(WIN_RATE, odds);
                    if stake > 0.01 {
                        tracker.record_bet(
                            next_block_open, next_block_open + 299_999,
                            candle.close, "YES", Some(odds), ratio, stake,
                        )?;
                        info!("BET YES | block={} stake={:.2} odds={:.3} ratio={:.3}",
                            next_block_open, stake, odds, ratio);
                        current_stake = stake;
                        current_odds = odds;
                    } else {
                        info!("SKIP | stake too small ({:.4})", stake);
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

fn simulate_5m(db: &Arc<Db>, tracker: &mut PaperTracker) -> Result<()> {
    info!("5m BULL YES simulation (7-day window)...");
    let candles = db.get_latest_candles(5000)?;
    if candles.len() < 20 {
        anyhow::bail!("Not enough candles");
    }

    let sim_window = 7 * 288; // 7 days of 5m bars
    let start = candles.len().saturating_sub(sim_window);
    let eval_start = start.max(10);
    let total = candles.len() - eval_start - 1;

    info!("Processing {} bars ({:.1} days)", total, total as f64 / 288.0);

    let th = Thresholds::default();
    let mut in_bull = false;
    let mut consec_bull: u32 = 0;
    let mut consec_exit: u32 = 0;
    let mut bets = 0u32;
    let mut wins = 0u32;

    for t in eval_start..candles.len() - 1 {
        let (still_bull, ratio, cb, ce) = decision::decision::evaluate(
            &candles, t, in_bull, consec_bull, consec_exit, &th,
        );
        in_bull = still_bull;
        consec_bull = cb;
        consec_exit = ce;

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

    info!("SIMULATION: {} bets, {} wins ({:.1}%)",
        bets, wins, if bets > 0 { wins as f64 / bets as f64 * 100.0 } else { 0.0 });
    tracker.print_summary()?;
    Ok(())
}

async fn bootstrap_5m(db: &Arc<Db>) -> Result<()> {
    let mut all = Vec::new();
    let mut end: Option<i64> = None;

    for _ in 0..5 {
        let mut url = "https://data-api.binance.vision/api/v3/klines?symbol=BTCUSDT&interval=5m&limit=1000".to_string();
        if let Some(et) = end {
            url.push_str(&format!("&endTime={}", et));
        }
        let resp: Vec<Vec<serde_json::Value>> =
            reqwest::get(&url).await?.json().await?;
        if resp.is_empty() { break; }

        let first_open = resp[0][0].as_i64().unwrap_or(0);
        for r in &resp {
            if r.len() < 10 { continue; }
            let vol: f64 = r[5].as_str().unwrap_or("0").parse().unwrap_or(0.0);
            if vol == 0.0 { continue; } // Skip zero-volume candles (data gaps)
            all.push(crate::decision::candle::Candle {
                open_time: r[0].as_i64().unwrap_or(0),
                close_time: r[6].as_i64().unwrap_or(0),
                open: r[1].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                high: r[2].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                low: r[3].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                close: r[4].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                volume: vol,
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

/// Expand historical data — paginate backwards as far as Binance.US allows.
async fn expand_historical_data(db: &Arc<Db>) -> Result<()> {
    // Get current earliest candle
    let existing = db.get_latest_candles(50000)?;
    let earliest = existing.first().map(|c| c.open_time).unwrap_or(0);

    info!("Current DB: {} candles, earliest open_time={}", existing.len(), earliest);
    if earliest == 0 {
        anyhow::bail!("No existing candles");
    }

    let target_days = 40;
    let target_earliest = earliest - (target_days * 86400000i64);

    let mut end: i64 = earliest - 1;
    let mut total_new = 0usize;
    let mut page = 0;

    loop {
        let url = format!(
            "https://data-api.binance.vision/api/v3/klines?symbol=BTCUSDT&interval=5m&limit=1000&endTime={}",
            end
        );

        page += 1;
        info!("Page {}: fetching before {} ...", page, end);

        let resp: Vec<Vec<serde_json::Value>> = match reqwest::get(&url).await {
            Ok(r) => r.json().await?,
            Err(e) => {
                warn!("Page {} error: {}, stopping", page, e);
                break;
            }
        };

        if resp.is_empty() {
            info!("Empty response, reached beginning of available data");
            break;
        }

        let first_open = resp[0][0].as_i64().unwrap_or(0);
        let last_open = resp.last().unwrap()[0].as_i64().unwrap_or(0);

        let mut candles = Vec::new();
        for r in &resp {
            if r.len() < 10 { continue; }
            let vol: f64 = r[5].as_str().unwrap_or("0").parse().unwrap_or(0.0);
            if vol == 0.0 { continue; } // Skip zero-volume candles (data gaps)
            candles.push(crate::decision::candle::Candle {
                open_time: r[0].as_i64().unwrap_or(0),
                close_time: r[6].as_i64().unwrap_or(0),
                open: r[1].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                high: r[2].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                low: r[3].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                close: r[4].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                volume: vol,
                taker_buy_vol: r[9].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                trades: r[8].as_u64().unwrap_or(0) as u32,
            });
        }

        total_new += candles.len();
        db.upsert_candles_batch(&candles)?;
        info!("  Got {} candles ({} to {})", candles.len(), first_open, last_open);

        if first_open <= target_earliest {
            info!("Reached target of {} days, stopping", target_days);
            break;
        }

        // Avoid hitting rate limits
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        end = first_open - 1;

        // Safety: don't paginate more than 50 pages
        if page >= 50 {
            warn!("Reached 50 pages, stopping");
            break;
        }
    }

    // Final count
    let final_candles = db.get_latest_candles(100000)?;
    let first_dt = chrono::DateTime::from_timestamp_millis(final_candles.first().unwrap().open_time)
        .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string()).unwrap_or_default();
    let last_dt = chrono::DateTime::from_timestamp_millis(final_candles.last().unwrap().open_time)
        .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string()).unwrap_or_default();
    let days = (final_candles.last().unwrap().open_time - final_candles.first().unwrap().open_time) as f64 / 86400000.0;

    info!("DONE: {} new candles fetched across {} pages", total_new, page);
    info!("DB now has {} candles ({:.1} days): {} to {}",
        final_candles.len(), days, first_dt, last_dt);

    Ok(())
}

