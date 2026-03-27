use anyhow::Result;
use rusqlite::params;
use serde::Deserialize;
use tracing::{info, warn};

#[derive(Debug, Deserialize)]
struct DepthResponse {
    bids: Vec<(String, String)>,
    asks: Vec<(String, String)>,
}

#[derive(Debug, Deserialize)]
struct KlineRow {
    #[serde(rename = "0")]
    open_time: i64,
    #[serde(rename = "1")]
    open: String,
    #[serde(rename = "2")]
    high: String,
    #[serde(rename = "3")]
    low: String,
    #[serde(rename = "4")]
    close: String,
    #[serde(rename = "5")]
    volume: String,
    #[serde(rename = "6")]
    close_time: i64,
    #[serde(rename = "8")]
    trades: String,
    #[serde(rename = "9")]
    taker_buy_vol: String,
}

pub struct ObiSnapshot {
    pub candle_open_time: i64,
    pub timestamp_ms: i64,
    pub bid_vol_top5: f64,
    pub ask_vol_top5: f64,
    pub bid_vol_top10: f64,
    pub ask_vol_top10: f64,
    pub bid_vol_top20: f64,
    pub ask_vol_top20: f64,
    pub obi_5: f64,
    pub obi_10: f64,
    pub obi_20: f64,
    pub spread: f64,
    pub mid_price: f64,
    pub bid_levels: i64,
    pub ask_levels: i64,
}

pub async fn fetch_depth_snapshot() -> Result<ObiSnapshot> {
    let url = "https://data-api.binance.vision/api/v3/depth?symbol=BTCUSDT&limit=20";
    let resp: DepthResponse = reqwest::get(url).await?.json().await?;
    let ts = chrono::Utc::now().timestamp_millis();

    let bids: Vec<(f64, f64)> = resp.bids.iter()
        .filter_map(|(p, v)| Some((p.parse().ok()?, v.parse().ok()?)))
        .collect();
    let asks: Vec<(f64, f64)> = resp.asks.iter()
        .filter_map(|(p, v)| Some((p.parse().ok()?, v.parse().ok()?)))
        .collect();

    let bid_vol_top5: f64 = bids.iter().take(5).map(|(_, v)| v).sum();
    let ask_vol_top5: f64 = asks.iter().take(5).map(|(_, v)| v).sum();
    let bid_vol_top10: f64 = bids.iter().take(10).map(|(_, v)| v).sum();
    let ask_vol_top10: f64 = asks.iter().take(10).map(|(_, v)| v).sum();
    let bid_vol_top20: f64 = bids.iter().take(20).map(|(_, v)| v).sum();
    let ask_vol_top20: f64 = asks.iter().take(20).map(|(_, v)| v).sum();

    let obi_5 = obi(bid_vol_top5, ask_vol_top5);
    let obi_10 = obi(bid_vol_top10, ask_vol_top10);
    let obi_20 = obi(bid_vol_top20, ask_vol_top20);

    let best_bid = bids.first().map(|(p, _)| *p).unwrap_or(0.0);
    let best_ask = asks.first().map(|(p, _)| *p).unwrap_or(0.0);
    let spread = best_ask - best_bid;
    let mid_price = (best_bid + best_ask) / 2.0;

    Ok(ObiSnapshot {
        candle_open_time: 0, // caller sets this
        timestamp_ms: ts,
        bid_vol_top5, ask_vol_top5, bid_vol_top10, ask_vol_top10,
        bid_vol_top20, ask_vol_top20,
        obi_5, obi_10, obi_20,
        spread, mid_price,
        bid_levels: bids.len() as i64,
        ask_levels: asks.len() as i64,
    })
}

fn obi(bid: f64, ask: f64) -> f64 {
    let total = bid + ask;
    if total < 1e-12 { 0.0 } else { (bid - ask) / total }
}

/// Fetch latest 5m candles from Binance REST and upsert into DB.
async fn sync_candles(db: &std::sync::Arc<crate::db::Db>) -> Result<()> {
    let url = "https://data-api.binance.vision/api/v3/klines?symbol=BTCUSDT&interval=5m&limit=10";
    let resp: Vec<Vec<serde_json::Value>> = reqwest::get(url).await?.json().await?;

    let conn = db.conn.lock().unwrap();
    for row in &resp {
        if row.len() < 10 { continue; }
        let vol: f64 = row[5].as_str().unwrap_or("0").parse().unwrap_or(0.0);
        if vol == 0.0 { continue; } // Skip zero-volume candles

        let open_time = row[0].as_i64().unwrap_or(0);
        let close_time = row[6].as_i64().unwrap_or(0);
        conn.execute(
            "INSERT OR REPLACE INTO candles (open_time, close_time, open, high, low, close, volume, taker_buy_vol, trades)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                open_time, close_time,
                row[1].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0),
                row[2].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0),
                row[3].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0),
                row[4].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0),
                row[5].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0),
                row[9].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0),
                row[8].as_u64().unwrap_or(0) as i64,
            ],
        )?;
    }
    Ok(())
}

pub fn ensure_table(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch("
        CREATE TABLE IF NOT EXISTS obi_snapshots (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            candle_open_time INTEGER NOT NULL,
            timestamp_ms INTEGER NOT NULL,
            bid_vol_top5 REAL NOT NULL,
            ask_vol_top5 REAL NOT NULL,
            bid_vol_top10 REAL NOT NULL,
            ask_vol_top10 REAL NOT NULL,
            bid_vol_top20 REAL NOT NULL,
            ask_vol_top20 REAL NOT NULL,
            obi_5 REAL NOT NULL,
            obi_10 REAL NOT NULL,
            obi_20 REAL NOT NULL,
            spread REAL NOT NULL,
            mid_price REAL NOT NULL,
            bid_levels INTEGER NOT NULL,
            ask_levels INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_obi_candle ON obi_snapshots(candle_open_time);
        CREATE INDEX IF NOT EXISTS idx_obi_timestamp ON obi_snapshots(timestamp_ms);
    ")?;
    Ok(())
}

pub fn insert_snapshot(conn: &rusqlite::Connection, snap: &ObiSnapshot) -> Result<()> {
    conn.execute(
        "INSERT INTO obi_snapshots (candle_open_time, timestamp_ms, bid_vol_top5, ask_vol_top5,
         bid_vol_top10, ask_vol_top10, bid_vol_top20, ask_vol_top20,
         obi_5, obi_10, obi_20, spread, mid_price, bid_levels, ask_levels)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
        params![snap.candle_open_time, snap.timestamp_ms,
               snap.bid_vol_top5, snap.ask_vol_top5,
               snap.bid_vol_top10, snap.ask_vol_top10,
               snap.bid_vol_top20, snap.ask_vol_top20,
               snap.obi_5, snap.obi_10, snap.obi_20,
               snap.spread, snap.mid_price, snap.bid_levels, snap.ask_levels],
    )?;
    Ok(())
}

/// Spawn a background task that collects OBI snapshots every interval_secs.
/// Each snapshot is tagged with the current candle's open_time (5m aligned).
pub fn spawn_collector(
    db: std::sync::Arc<crate::db::Db>,
    interval_secs: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Ensure table exists
        {
            let conn = db.conn.lock().unwrap();
            if let Err(e) = ensure_table(&conn) {
                tracing::error!("Failed to create OBI table: {}", e);
                return;
            }
        }

        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(interval_secs));
        let mut last_candle_sync: i64 = 0;
        info!("OBI collector started (every {}s)", interval_secs);

        loop {
            interval.tick().await;

            let now_ms = chrono::Utc::now().timestamp_millis();

            // Every ~60s, fetch latest candles from REST and upsert into DB
            // This ensures we always have OHLCV data matching OBI snapshots
            if now_ms - last_candle_sync > 60_000 {
                last_candle_sync = now_ms;
                if let Err(e) = sync_candles(&db).await {
                    warn!("Candle sync error: {}", e);
                }
            }

            match fetch_depth_snapshot().await {
                Ok(mut snap) => {
                    let candle_open = (now_ms / 300_000) * 300_000;
                    snap.candle_open_time = candle_open;

                    let conn = db.conn.lock().unwrap();
                    if let Err(e) = insert_snapshot(&conn, &snap) {
                        warn!("OBI insert error: {}", e);
                    }
                }
                Err(e) => {
                    warn!("OBI fetch error: {}", e);
                }
            }

            // Log progress every ~5 min
            if now_ms % 300_000 < (interval_secs * 1000) as i64 {
                let conn = db.conn.lock().unwrap();
                let count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM obi_snapshots", [], |r| r.get(0)
                ).unwrap_or(0);
                let candles: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM obi_snapshots WHERE candle_open_time = ?1",
                    params![((now_ms / 300_000) * 300_000)], |r| r.get(0)
                ).unwrap_or(0);
                info!("OBI progress: total={} this_candle={} candle_time={}", count, candles, now_ms / 300_000);
            }
        }
    })
}
