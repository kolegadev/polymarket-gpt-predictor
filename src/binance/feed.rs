use anyhow::{Result, Context};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{info, warn, debug};
use std::sync::Arc;
use tokio::sync::watch;
use crate::decision::candle::Candle;
use crate::db::Db;

const BINANCE_WS: &str = "wss://stream.binance.us:9443/ws/btcusdt@kline_5m";
const BINANCE_REST_KLINES: &str = "https://data-api.binance.vision/api/v3/klines?symbol=BTCUSDT&interval=5m&limit=2";
const WS_IDLE_TIMEOUT_SECS: u64 = 30;
const REST_POLL_INTERVAL_SECS: u64 = 5;
const MAX_WS_CONSECUTIVE_FAILURES: u32 = 3;

/// Building candle from the websocket stream.
#[derive(Debug, Clone, Default)]
pub struct BuildingCandle {
    pub open_time: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub taker_buy_vol: f64,
    pub trades: u32,
}

/// Events emitted by the feed.
#[derive(Debug, Clone)]
pub enum FeedEvent {
    /// Tick update within current candle
    Update(BuildingCandle),
    /// Candle closed — finalized
    Closed(Candle),
}

#[derive(Debug, Deserialize)]
struct KlineEvent {
    #[serde(rename = "E")]
    event_time: i64,
    k: Kline,
}

#[derive(Debug, Deserialize)]
struct Kline {
    #[serde(rename = "t")]
    open_time: i64,
    #[serde(rename = "T")]
    close_time: i64,
    #[serde(rename = "o")]
    open: String,
    #[serde(rename = "h")]
    high: String,
    #[serde(rename = "l")]
    low: String,
    #[serde(rename = "c")]
    close: String,
    #[serde(rename = "v")]
    volume: String,
    #[serde(rename = "V")]
    taker_buy_vol: String,
    #[serde(rename = "n")]
    trades: u32,
    #[serde(rename = "x")]
    is_closed: bool,
}

/// Spawn the Binance websocket feed. Returns a watch channel receiver
/// that delivers FeedEvents. Also writes closed candles to the DB.
pub async fn spawn_feed(
    db: Arc<Db>,
) -> Result<watch::Receiver<Option<FeedEvent>>> {
    let (tx, rx) = watch::channel(None);

    // Bootstrap historical candles from REST before WS connects
    bootstrap_history(&db).await?;

    tokio::spawn(async move {
        let mut building: Option<BuildingCandle> = None;
        let mut current_open_time: i64 = 0;
        let mut ws_failures: u32 = 0;

        loop {
            if ws_failures < MAX_WS_CONSECUTIVE_FAILURES {
                match connect_and_listen(&db, &mut building, &mut current_open_time, &tx).await {
                    Ok(()) => {
                        ws_failures += 1;
                        warn!("Binance WS closed (failure #{}/{}), reconnecting in 3s...",
                            ws_failures, MAX_WS_CONSECUTIVE_FAILURES);
                        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                    }
                    Err(e) => {
                        ws_failures += 1;
                        warn!("Binance WS error (failure #{}/{}): {}, reconnecting in 3s...",
                            ws_failures, MAX_WS_CONSECUTIVE_FAILURES, e);
                        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                    }
                }
            } else {
                warn!("WS failed {} times, switching to REST polling fallback...", ws_failures);
                match rest_poll_loop(&db, &mut building, &mut current_open_time, &tx).await {
                    Ok(()) => {
                        // REST loop exited (e.g. recovered), reset failures and try WS again
                        ws_failures = 0;
                        info!("REST polling ended, attempting WS reconnect...");
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    }
                    Err(e) => {
                        warn!("REST polling error: {}, retrying in 5s...", e);
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    }
                }
            }
        }
    });

    Ok(rx)
}

async fn bootstrap_history(db: &Db) -> Result<()> {
    // Fetch last 150 candles to warm indicators (need 99 for SMA99 + some buffer)
    info!("Bootstrapping historical candles from Binance REST...");
    let url = "https://data-api.binance.vision/api/v3/klines?symbol=BTCUSDT&interval=5m&limit=60";
    let resp = reqwest::get(url).await?.json::<Vec<Vec<serde_json::Value>>>().await?;

    let mut candles = Vec::new();
    for row in &resp {
        if row.len() < 10 { continue; }
        let candle = Candle {
            open_time: row[0].as_i64().unwrap_or(0),
            close_time: row[6].as_i64().unwrap_or(0),
            open: row[1].as_str().unwrap_or("0").parse().unwrap_or(0.0),
            high: row[2].as_str().unwrap_or("0").parse().unwrap_or(0.0),
            low: row[3].as_str().unwrap_or("0").parse().unwrap_or(0.0),
            close: row[4].as_str().unwrap_or("0").parse().unwrap_or(0.0),
            volume: row[5].as_str().unwrap_or("0").parse().unwrap_or(0.0),
            taker_buy_vol: row[9].as_str().unwrap_or("0").parse().unwrap_or(0.0),
            trades: row[8].as_u64().unwrap_or(0) as u32,
        };
        candles.push(candle);
    }

    info!("Fetched {} historical candles", candles.len());
    db.upsert_candles_batch(&candles)?;
    Ok(())
}

async fn connect_and_listen(
    db: &Db,
    building: &mut Option<BuildingCandle>,
    current_open_time: &mut i64,
    tx: &watch::Sender<Option<FeedEvent>>,
) -> Result<()> {
    info!("Connecting to Binance WS: {}", BINANCE_WS);
    let (ws_stream, _) = connect_async(BINANCE_WS).await.context("WS connect failed")?;
    let (mut write, mut read) = ws_stream.split();

    loop {
        // Watchdog: if no message for 30s, force reconnect
        let msg = tokio::select! {
            m = read.next() => {
                match m {
                    Some(m) => m,
                    None => {
                        warn!("Binance WS stream ended, reconnecting...");
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(30)) => {
                warn!("Binance WS idle for 30s, forcing reconnect...");
                break;
            }
        };

        let msg = msg?;
        if msg.is_ping() {
            write.send(Message::Pong(vec![].into())).await?;
            continue;
        }
        if !msg.is_text() { continue; }

        let text = msg.to_text()?;
        let event: KlineEvent = match serde_json::from_str(text) {
            Ok(e) => e,
            Err(e) => {
                debug!("Non-kline message or parse error: {}", e);
                continue;
            }
        };

        let k = event.k;
        let open_time = k.open_time;

        // New candle started?
        if open_time != *current_open_time {
            // Finalize previous if it exists
            if let Some(b) = building.take() {
                let candle = Candle {
                    open_time: b.open_time,
                    close_time: b.open_time + 299999, // 5min - 1ms
                    open: b.open,
                    high: b.high,
                    low: b.low,
                    close: b.close,
                    volume: b.volume,
                    taker_buy_vol: b.taker_buy_vol,
                    trades: b.trades,
                };

                if let Err(e) = db.upsert_candle(&candle) {
                    warn!("Failed to save candle: {}", e);
                }

                let _ = tx.send(Some(FeedEvent::Closed(candle.clone())));
                info!("Candle closed: open_time={} close={:.2}", candle.open_time, candle.close);
            }

            *current_open_time = open_time;
            let o: f64 = k.open.parse().unwrap_or(0.0);
            let h: f64 = k.high.parse().unwrap_or(o);
            let l: f64 = k.low.parse().unwrap_or(o);
            let c: f64 = k.close.parse().unwrap_or(o);
            let v: f64 = k.volume.parse().unwrap_or(0.0);
            let tbv: f64 = k.taker_buy_vol.parse().unwrap_or(0.0);

            *building = Some(BuildingCandle {
                open_time, open: o, high: h, low: l, close: c,
                volume: v, taker_buy_vol: tbv, trades: k.trades,
            });
        } else {
            // Update building candle
            let b = building.get_or_insert_with(|| BuildingCandle {
                open_time, ..Default::default()
            });
            b.trades = k.trades;
            if let Ok(v) = k.high.parse::<f64>() { b.high = b.high.max(v); }
            if let Ok(v) = k.low.parse::<f64>() { b.low = if b.low == 0.0 { v } else { b.low.min(v) }; }
            if let Ok(v) = k.close.parse::<f64>() { b.close = v; }
            if let Ok(v) = k.volume.parse::<f64>() { b.volume = v; }
            if let Ok(v) = k.taker_buy_vol.parse::<f64>() { b.taker_buy_vol = v; }
        }

        // If kline says closed, force close
        if k.is_closed {
            if let Some(b) = building.take() {
                let candle = Candle {
                    open_time: b.open_time,
                    close_time: k.close_time,
                    open: b.open, high: b.high, low: b.low, close: b.close,
                    volume: b.volume, taker_buy_vol: b.taker_buy_vol, trades: b.trades,
                };
                if let Err(e) = db.upsert_candle(&candle) {
                    warn!("Failed to save closed candle: {}", e);
                }
                let _ = tx.send(Some(FeedEvent::Closed(candle.clone())));
                info!("Candle closed (kline flag): open_time={} close={:.2}", candle.open_time, candle.close);
            }
        }
    }

    Ok(())
}

/// REST polling fallback when websocket is unreliable.
/// Polls Binance REST every REST_POLL_INTERVAL_SECS for new closed candles.
async fn rest_poll_loop(
    db: &Db,
    building: &mut Option<BuildingCandle>,
    current_open_time: &mut i64,
    tx: &watch::Sender<Option<FeedEvent>>,
) -> Result<()> {
    info!("Starting REST polling fallback (every {}s)...", REST_POLL_INTERVAL_SECS);

    loop {
        let resp: Vec<Vec<serde_json::Value>> = match reqwest::get(BINANCE_REST_KLINES).await {
            Ok(r) => r.json().await?,
            Err(e) => {
                warn!("REST poll error: {}", e);
                tokio::time::sleep(tokio::time::Duration::from_secs(REST_POLL_INTERVAL_SECS)).await;
                continue;
            }
        };

        for row in &resp {
            if row.len() < 10 { continue; }
            let vol: f64 = row[5].as_str().unwrap_or("0").parse().unwrap_or(0.0);
            if vol == 0.0 { continue; } // Skip zero-volume candles (Binance.US data gaps)

            let candle = Candle {
                open_time: row[0].as_i64().unwrap_or(0),
                close_time: row[6].as_i64().unwrap_or(0),
                open: row[1].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                high: row[2].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                low: row[3].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                close: row[4].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                volume: vol,
                taker_buy_vol: row[9].as_str().unwrap_or("0").parse().unwrap_or(0.0),
                trades: row[8].as_u64().unwrap_or(0) as u32,
            };

            // New candle period detected — finalize the previous one
            if candle.open_time > *current_open_time {
                if let Some(b) = building.take() {
                    let prev = Candle {
                        open_time: b.open_time,
                        close_time: b.open_time + 299999,
                        open: b.open, high: b.high, low: b.low, close: b.close,
                        volume: b.volume, taker_buy_vol: b.taker_buy_vol, trades: b.trades,
                    };
                    if let Err(e) = db.upsert_candle(&prev) {
                        warn!("Failed to save candle: {}", e);
                    }
                    let _ = tx.send(Some(FeedEvent::Closed(prev.clone())));
                    info!("Candle closed (REST): open_time={} close={:.2}", prev.open_time, prev.close);
                }

                *current_open_time = candle.open_time;
                *building = Some(BuildingCandle {
                    open_time: candle.open_time,
                    open: candle.open, high: candle.high, low: candle.low,
                    close: candle.close, volume: candle.volume,
                    taker_buy_vol: candle.taker_buy_vol, trades: candle.trades,
                });
            } else if candle.open_time == *current_open_time {
                // Update building candle with latest data
                if let Some(b) = building.as_mut() {
                    b.high = b.high.max(candle.high);
                    b.low = if b.low == 0.0 { candle.low } else { b.low.min(candle.low) };
                    b.close = candle.close;
                    b.volume = candle.volume;
                    b.taker_buy_vol = candle.taker_buy_vol;
                    b.trades = candle.trades;
                }
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(REST_POLL_INTERVAL_SECS)).await;
    }
}


