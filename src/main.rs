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
use decision::state::State;
use db::Db;
use paper::tracker::PaperTracker;
use clob::clob::ClobClient;
use binance::feed::FeedEvent;
use config::config::Args;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| args.log_level.clone().into()),
        )
        .init();

    // Ensure data directory exists
    std::fs::create_dir_all("data")?;

    let db = Arc::new(Db::new(&args.db_path)?);
    let clob = ClobClient::new();
    let tracker = PaperTracker::new(db.clone());

    if args.summary {
        tracker.print_summary()?;
        return Ok(());
    }

    if args.diagnose {
        let candles = db.get_latest_candles(5000)?;
        if candles.len() < 110 {
            anyhow::bail!("Need at least 110 candles, got {}", candles.len());
        }
        decision::diagnostics::run_diagnostics(&candles);
        return Ok(());
    }

    if args.bootstrap {
        // Fetch historical candles then exit
        let count = db.get_latest_candles(1)?.len();
        info!("Existing candles in DB: {}", count);
        info!("Bootstrapping 1500 historical candles from Binance REST...");
        bootstrap_large(&db).await?;
        let new_count = db.get_latest_candles(1)?.len();
        info!("Done. Total candles now: {}", new_count);
        return Ok(());
    }

    if args.simulate {
        return run_simulation(&db, &tracker).await;
    }

    // Live mode
    info!("🚀 Starting PolyMarket BTC 15m Prediction Engine (paper trading mode)");
    info!("📊 DB: {}", args.db_path);
    info!("🎯 Market: {}", args.market_slug);

    let mut rx = binance::feed::spawn_feed(db.clone()).await?;

    let mut current_state = State::Neutral;

    loop {
        // Wait for candle close events
        if rx.changed().await.is_err() {
            warn!("Feed channel closed, exiting");
            break;
        }

        let event = rx.borrow().clone();
        let Some(event) = event else { continue };

        match event {
            FeedEvent::Closed(candle) => {
                info!("🔄 Processing closed candle: open_time={} close={:.2}", candle.open_time, candle.close);

                // Load enough candles for indicator computation
                let candles = db.get_latest_candles(110)?;
                if candles.len() < 100 {
                    warn!("Not enough candles for computation ({}), need 100+", candles.len());
                    continue;
                }

                let t = candles.len() - 1;
                let f_t = decision::features::compute(&candles, t);
                let f_t1 = decision::features::compute(&candles, t - 1);
                let f_t2 = decision::features::compute(&candles, t.saturating_sub(2));

                let th = decision::decision::Thresholds::default();
                let (new_state, decision) = decision::decision::decide(
                    current_state, &candles, t, &f_t, &f_t1, &f_t2, &th,
                );

                info!("  State: {:?} → {:?} | Decision: {:?}", current_state, new_state, decision);

                // Skip if SKIP
                if decision == decision::state::Decision::Skip {
                    current_state = new_state;
                    continue;
                }

                // Fetch PolyMarket opening odds
                let next_block_open = candle.open_time + 900_000; // 15min in ms
                let next_block_close = next_block_open + 899_999;
                let reference_price = candle.close; // use current close as reference

                let odds = match clob.get_odds_for_block(next_block_open).await {
                    Ok((yes, no)) => {
                        info!("  CLOB odds: YES={:.4} NO={:.4}", yes, no);
                        Some((yes, no))
                    }
                    Err(e) => {
                        warn!("  Failed to fetch CLOB odds: {}, recording without odds", e);
                        None
                    }
                };

                // Compute scores for logging
                let bull_sc = if f_t.sma7 > f_t.sma25 {
                    Some(decision::decision::bull_score(&candles, t, &f_t))
                } else { None };
                let bear_sc = if f_t.sma7 < f_t.sma25 {
                    Some(decision::decision::bear_score(&candles, t, &f_t))
                } else { None };

                let _ = tracker.record_decision(
                    next_block_open,
                    next_block_close,
                    reference_price,
                    new_state,
                    decision,
                    odds.as_ref().map(|(y, _)| *y),
                    odds.as_ref().map(|(_, n)| *n),
                    &f_t,
                    bull_sc,
                    bear_sc,
                );

                // Resolve any previous unresolved trades
                // Note: in live mode, resolution happens when the next candle closes.
                // We'll resolve the trade for the block that just ended.

                current_state = new_state;
            }
            FeedEvent::Update(_) => {
                // Intra-candle updates — could be used for pre-computation
            }
        }
    }

    // Print final summary
    tracker.print_summary()?;

    Ok(())
}

/// Run backtest simulation on historical data already in DB.
async fn run_simulation(db: &Arc<Db>, tracker: &PaperTracker) -> Result<()> {
    info!("📊 Running simulation on historical data...");

    let candles = db.get_latest_candles(10000)?;
    if candles.len() < 110 {
        anyhow::bail!("Need at least 110 candles in DB for simulation, got {}", candles.len());
    }

    info!("Processing {} candles", candles.len());

    let mut state = State::Neutral;
    let th = decision::decision::Thresholds::default();
    let mut yes_count = 0u32;
    let mut no_count = 0u32;
    let mut skip_count = 0u32;

    for t in 100..candles.len() - 1 {
        let f_t = decision::features::compute(&candles, t);
        let f_t1 = decision::features::compute(&candles, t - 1);
        let f_t2 = decision::features::compute(&candles, t.saturating_sub(2));

        let (new_state, decision) = decision::decision::decide(
            state, &candles, t, &f_t, &f_t1, &f_t2, &th,
        );

        let next = &candles[t + 1];
        let next_block_open = next.open_time;
        let next_block_close = next.close_time;
        let reference_price = candles[t].close;

        match decision {
            decision::state::Decision::Yes => {
                yes_count += 1;
                let bull_sc = Some(decision::decision::bull_score(&candles, t, &f_t));

                tracker.record_decision(
                    next_block_open, next_block_close, reference_price,
                    new_state, decision, Some(0.50), Some(0.50),
                    &f_t, bull_sc, None,
                )?;
            }
            decision::state::Decision::No => {
                no_count += 1;
                let bear_sc = Some(decision::decision::bear_score(&candles, t, &f_t));

                tracker.record_decision(
                    next_block_open, next_block_close, reference_price,
                    new_state, decision, Some(0.50), Some(0.50),
                    &f_t, None, bear_sc,
                )?;
            }
            decision::state::Decision::Skip => {
                skip_count += 1;
            }
        }

        // Resolve with actual next candle close
        if decision != decision::state::Decision::Skip {
            tracker.resolve_trade_by_block(next_block_open, next.close)?;
        }

        state = new_state;
    }

    info!("─────────────────────────────────────");
    info!("SIMULATION COMPLETE");
    info!("  YES bets: {} | NO bets: {} | SKIP: {}", yes_count, no_count, skip_count);
    info!("─────────────────────────────────────");

    tracker.print_summary()?;

    Ok(())
}

/// Fetch a large batch of historical candles from Binance REST.
async fn bootstrap_large(db: &Arc<Db>) -> Result<()> {
    let mut all_candles = Vec::new();
    let mut end_time: Option<i64> = None;

    // Fetch in batches of 1000 working backwards from now
    for _ in 0..10 {
        let mut url = format!(
            "https://api.binance.us/api/v3/klines?symbol=BTCUSDT&interval=15m&limit=1000"
        );
        if let Some(et) = end_time {
            url.push_str(&format!("&endTime={}", et));
        }

        let resp = reqwest::get(&url).await?.json::<Vec<Vec<serde_json::Value>>>().await?;
        if resp.is_empty() { break; }

        // Binance returns ascending order. First candle = oldest in this batch.
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

        // Next batch: endTime before the first candle of this batch
        end_time = Some(first_open - 1);
    }

    info!("Fetched {} total candles (may contain duplicates)", all_candles.len());
    db.upsert_candles_batch(&all_candles)?;

    let unique = db.get_latest_candles(10000)?.len();
    info!("Unique candles in DB: {}", unique);
    Ok(())
}
