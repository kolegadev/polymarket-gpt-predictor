use anyhow::Result;
use rusqlite::{Connection, params};
use std::sync::Mutex;
use tracing::{info, warn, error};

pub struct BearDb {
    pub conn: Mutex<Connection>,
}

#[derive(Debug, Clone)]
pub struct BearTrade {
    pub id: Option<i64>,
    pub candle_open_time: i64,
    pub decision: String,
    pub obi5: f64,
    pub obi10: f64,
    pub obi20: f64,
    pub obi5_late: Option<f64>,
    pub obi5_early: Option<f64>,
    pub obi5_momentum: Option<f64>,
    pub bid_ask_ratio_5: Option<f64>,
    pub spread: Option<f64>,
    pub stake: f64,
    pub odds: f64,
    pub entry_price: Option<f64>,
    pub exit_price: Option<f64>,
    pub outcome: Option<String>,
    pub pnl: Option<f64>,
    pub balance_at_entry: f64,
}

impl BearDb {
    pub fn new(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Self { conn: Mutex::new(conn) };
        db.init()?;
        Ok(db)
    }

    fn init(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS bear_trades (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                candle_open_time INTEGER NOT NULL,
                decision TEXT NOT NULL,
                obi5 REAL NOT NULL,
                obi10 REAL NOT NULL,
                obi20 REAL NOT NULL,
                obi5_late REAL,
                obi5_early REAL,
                obi5_momentum REAL,
                bid_ask_ratio_5 REAL,
                spread REAL,
                stake REAL DEFAULT 0,
                odds REAL DEFAULT 0.505,
                entry_price REAL,
                exit_price REAL,
                outcome TEXT,
                pnl REAL,
                balance_at_entry REAL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_bear_candle ON bear_trades(candle_open_time);
        ")?;
        Ok(())
    }

    pub fn get_balance(&self, default: f64) -> Result<f64> {
        let conn = self.conn.lock().unwrap();
        let balance: f64 = conn.query_row(
            "SELECT value FROM account_state WHERE key = 'bear_balance'", [], |r| r.get(0)
        ).unwrap_or(default);
        Ok(balance)
    }

    pub fn set_balance(&self, balance: f64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO account_state (key, value, updated_at) VALUES ('bear_balance', ?1, datetime('now'))",
            params![balance],
        )?;
        Ok(())
    }

    pub fn insert_trade(&self, t: &BearTrade) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO bear_trades (candle_open_time, decision, obi5, obi10, obi20,
             obi5_late, obi5_early, obi5_momentum, bid_ask_ratio_5, spread,
             stake, odds, entry_price, balance_at_entry)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![t.candle_open_time, t.decision, t.obi5, t.obi10, t.obi20,
                    t.obi5_late, t.obi5_early, t.obi5_momentum, t.bid_ask_ratio_5, t.spread,
                    t.stake, t.odds, t.entry_price, t.balance_at_entry],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_trade_outcome(&self, id: i64, outcome: &str, pnl: f64, exit_price: f64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE bear_trades SET outcome = ?1, pnl = ?2, exit_price = ?3 WHERE id = ?4",
            params![outcome, pnl, exit_price, id],
        )?;
        Ok(())
    }

    pub fn get_unresolved_trades(&self) -> Result<Vec<BearTrade>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, candle_open_time, decision, obi5, obi10, obi20,
                    obi5_late, obi5_early, obi5_momentum, bid_ask_ratio_5, spread,
                    stake, odds, entry_price, exit_price, outcome, COALESCE(pnl,0), balance_at_entry
             FROM bear_trades WHERE outcome IS NULL ORDER BY candle_open_time"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(BearTrade {
                id: Some(row.get(0)?),
                candle_open_time: row.get(1)?,
                decision: row.get(2)?,
                obi5: row.get(3)?,
                obi10: row.get(4)?,
                obi20: row.get(5)?,
                obi5_late: row.get(6)?,
                obi5_early: row.get(7)?,
                obi5_momentum: row.get(8)?,
                bid_ask_ratio_5: row.get(9)?,
                spread: row.get(10)?,
                stake: row.get(11)?,
                odds: row.get(12)?,
                entry_price: row.get(13)?,
                exit_price: row.get(14)?,
                outcome: row.get(15)?,
                pnl: row.get(16)?,
                balance_at_entry: row.get(17)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Get aggregated OBI for a candle
    pub fn get_obi_for_candle(&self, candle_open_time: i64) -> Result<Option<ObiAgg>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT COUNT(*), AVG(obi_5), AVG(obi_10), AVG(obi_20),
                    AVG(spread), AVG(bid_vol_top5), AVG(ask_vol_top5),
                    MIN(obi_5) as obi5_min, MAX(obi_5) as obi5_max,
                    AVG(CASE WHEN timestamp_ms > candle_open_time + 240000 THEN obi_5 END),
                    AVG(CASE WHEN timestamp_ms < candle_open_time + 60000 THEN obi_5 END)
             FROM obi_snapshots WHERE candle_open_time = ?1"
        )?;
        let mut rows = stmt.query_map(params![candle_open_time], |row| {
            Ok(ObiAgg {
                count: row.get(0)?,
                obi5_avg: row.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                obi10_avg: row.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                obi20_avg: row.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                spread_avg: row.get::<_, Option<f64>>(4)?.unwrap_or(0.0),
                bid_vol5_avg: row.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
                ask_vol5_avg: row.get::<_, Option<f64>>(6)?.unwrap_or(0.0),
                obi5_min: row.get::<_, Option<f64>>(7)?.unwrap_or(0.0),
                obi5_max: row.get::<_, Option<f64>>(8)?.unwrap_or(0.0),
                obi5_late: row.get::<_, Option<f64>>(9)?,
                obi5_early: row.get::<_, Option<f64>>(10)?,
            })
        })?;
        match rows.next() {
            Some(r) => {
                let agg = r?;
                if agg.count > 0 { Ok(Some(agg)) } else { Ok(None) }
            }
            None => Ok(None),
        }
    }

    /// Get the latest closed candle (close_time < now_ms)
    pub fn get_latest_closed_candle(&self, now_ms: i64) -> Result<Option<CandleInfo>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT open_time, close_time, open, close FROM candles WHERE close_time < ?1 ORDER BY open_time DESC LIMIT 1"
        )?;
        let mut rows = stmt.query_map(params![now_ms], |row| {
            Ok(CandleInfo {
                open_time: row.get(0)?,
                close_time: row.get(1)?,
                open: row.get(2)?,
                close: row.get(3)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// Get candle by open_time
    pub fn get_candle(&self, open_time: i64) -> Result<Option<CandleInfo>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT open_time, close_time, open, close FROM candles WHERE open_time = ?1"
        )?;
        let mut rows = stmt.query_map(params![open_time], |row| {
            Ok(CandleInfo {
                open_time: row.get(0)?,
                close_time: row.get(1)?,
                open: row.get(2)?,
                close: row.get(3)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// Compute taker buy ratio (taker_buy_vol / volume) over last N bars ending at candle_open_time.
    pub fn taker_ratio(&self, candle_open_time: i64, lookback: usize) -> Result<f64> {
        let conn = self.conn.lock().unwrap();
        // Get the N most recent candles up to and including candle_open_time
        let mut stmt = conn.prepare(
            "SELECT taker_buy_vol, volume FROM candles
             WHERE open_time <= ?1
             ORDER BY open_time DESC
             LIMIT ?2"
        )?;
        let rows: Vec<(f64, f64)> = stmt.query_map(params![candle_open_time, lookback as i64], |row| {
            Ok((row.get::<_, f64>(0)?, row.get::<_, f64>(1)?))
        })?.filter_map(|r| r.ok()).collect();

        if rows.is_empty() { return Ok(0.5); }
        let buy_vol: f64 = rows.iter().map(|(b, _)| b).sum();
        let total_vol: f64 = rows.iter().map(|(_, t)| t).sum();
        if total_vol < 1e-10 { return Ok(0.5); }
        Ok(buy_vol / total_vol)
    }

    pub fn has_trade_for_candle(&self, candle_open_time: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM bear_trades WHERE candle_open_time = ?1",
            params![candle_open_time], |r| r.get(0)
        )?;
        Ok(count > 0)
    }

    pub fn print_summary(&self, start_balance: f64) -> Result<()> {
        let conn = self.conn.lock().unwrap();

        let balance = self.get_balance(start_balance)?;
        let pnl = balance - start_balance;
        let roi = (pnl / start_balance) * 100.0;

        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM bear_trades WHERE decision NOT IN ('SKIP','Skip')", [], |r| r.get(0))?;
        let wins: i64 = conn.query_row(
            "SELECT COUNT(*) FROM bear_trades WHERE outcome = 'WIN'", [], |r| r.get(0))?;
        let losses: i64 = conn.query_row(
            "SELECT COUNT(*) FROM bear_trades WHERE outcome = 'LOSS'", [], |r| r.get(0))?;
        let skips: i64 = conn.query_row(
            "SELECT COUNT(*) FROM bear_trades WHERE decision IN ('SKIP','Skip')", [], |r| r.get(0))?;
        let total_pnl: f64 = conn.query_row(
            "SELECT COALESCE(SUM(pnl),0) FROM bear_trades WHERE outcome IS NOT NULL", [], |r| r.get(0))?;

        let win_rate = if total > 0 { wins as f64 / total as f64 * 100.0 } else { 0.0 };
        let pnl_per = if total > 0 { total_pnl / total as f64 } else { 0.0 };

        // YES breakdown
        let yes_total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM bear_trades WHERE decision = 'YES' AND outcome IS NOT NULL", [], |r| r.get(0))?;
        let yes_wins: i64 = conn.query_row(
            "SELECT COUNT(*) FROM bear_trades WHERE decision = 'YES' AND outcome = 'WIN'", [], |r| r.get(0))?;
        let yes_pnl: f64 = conn.query_row(
            "SELECT COALESCE(SUM(pnl),0) FROM bear_trades WHERE decision = 'YES' AND outcome IS NOT NULL", [], |r| r.get(0))?;

        // NO breakdown
        let no_total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM bear_trades WHERE decision = 'NO' AND outcome IS NOT NULL", [], |r| r.get(0))?;
        let no_wins: i64 = conn.query_row(
            "SELECT COUNT(*) FROM bear_trades WHERE decision = 'NO' AND outcome = 'WIN'", [], |r| r.get(0))?;
        let no_pnl: f64 = conn.query_row(
            "SELECT COALESCE(SUM(pnl),0) FROM bear_trades WHERE decision = 'NO' AND outcome IS NOT NULL", [], |r| r.get(0))?;

        // Streak
        let last_outcome: Option<String> = conn.query_row(
            "SELECT outcome FROM bear_trades WHERE outcome IS NOT NULL ORDER BY id DESC LIMIT 1", [], |r| r.get(0)).ok();
        let (streak, streak_type) = if let Some(o) = &last_outcome {
            let s: i64 = conn.query_row(
                &format!("SELECT COUNT(*) FROM (SELECT outcome FROM bear_trades WHERE outcome IS NOT NULL ORDER BY id DESC LIMIT (SELECT COUNT(*) FROM bear_trades WHERE outcome = '{}'))", o),
                [], |r| r.get(0)).unwrap_or(0);
            (s, o.clone())
        } else { (0, "-".into()) };

        println!("\n╔══════════════════════════════════════════════════════╗");
        println!("║          🐻 BEAR TRADER SUMMARY                      ║");
        println!("╠══════════════════════════════════════════════════════╣");
        println!("║ Balance:    ${:>12.2}                             ║", balance);
        println!("║ PnL:        ${:>12.2}                             ║", pnl);
        println!("║ ROI:        {:>12.2}%                             ║", roi);
        println!("║ Streak:     {:>6} {:>8}                            ║", streak, streak_type);
        println!("╠══════════════════════════════════════════════════════╣");
        println!("║ Total bets: {:>6}  Wins: {:>4}  Losses: {:>4}  Skips: {:>4}  ║", total, wins, losses, skips);
        println!("║ Win rate:   {:>11.1}%                             ║", win_rate);
        println!("║ PnL/trade:  ${:>11.2}                             ║", pnl_per);
        println!("╠══════════════════════════════════════════════════════╣");
        println!("║ YES bets:   {:>4} ({:>2}W)  PnL: ${:>10.2}              ║", yes_total, yes_wins, yes_pnl);
        println!("║ NO bets:    {:>4} ({:>2}W)  PnL: ${:>10.2}              ║", no_total, no_wins, no_pnl);
        println!("╚══════════════════════════════════════════════════════╝\n");

        Ok(())
    }
}

#[derive(Debug)]
pub struct ObiAgg {
    pub count: i64,
    pub obi5_avg: f64,
    pub obi10_avg: f64,
    pub obi20_avg: f64,
    pub spread_avg: f64,
    pub bid_vol5_avg: f64,
    pub ask_vol5_avg: f64,
    pub obi5_min: f64,
    pub obi5_max: f64,
    pub obi5_late: Option<f64>,
    pub obi5_early: Option<f64>,
}

#[derive(Debug)]
pub struct CandleInfo {
    pub open_time: i64,
    pub close_time: i64,
    pub open: f64,
    pub close: f64,
}
