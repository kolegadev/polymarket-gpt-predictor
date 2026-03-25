use anyhow::Result;
use rusqlite::{Connection, params};
use crate::types::models::{PaperTrade, RegimeSnapshot};

pub struct Db {
    pub conn: std::sync::Mutex<Connection>,
}

impl Db {
    pub fn new(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Self { conn: std::sync::Mutex::new(conn) };
        db.init()?;
        Ok(db)
    }

    fn init(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS candles (
                open_time INTEGER PRIMARY KEY,
                close_time INTEGER NOT NULL,
                open REAL NOT NULL,
                high REAL NOT NULL,
                low REAL NOT NULL,
                close REAL NOT NULL,
                volume REAL NOT NULL,
                taker_buy_vol REAL NOT NULL,
                trades INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS paper_trades (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                block_open_time INTEGER NOT NULL,
                block_close_time INTEGER NOT NULL,
                reference_price REAL NOT NULL,
                resolution_price REAL,
                regime TEXT NOT NULL,
                decision TEXT NOT NULL,
                open_odds_yes REAL,
                open_odds_no REAL,
                outcome TEXT,
                pnl REAL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS regime_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                block_open_time INTEGER NOT NULL,
                state TEXT NOT NULL,
                bull_score INTEGER,
                bear_score INTEGER,
                spread_7_25 REAL,
                slope_25 REAL,
                close_pos REAL,
                green_8 INTEGER,
                red_8 INTEGER
            );

            CREATE INDEX IF NOT EXISTS idx_candles_close_time ON candles(close_time);
            CREATE INDEX IF NOT EXISTS idx_paper_trades_block ON paper_trades(block_open_time);
            CREATE INDEX IF NOT EXISTS idx_regime_block ON regime_snapshots(block_open_time);
        ")?;
        Ok(())
    }

    pub fn upsert_candle(&self, c: &crate::decision::candle::Candle) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO candles (open_time, close_time, open, high, low, close, volume, taker_buy_vol, trades)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![c.open_time, c.close_time, c.open, c.high, c.low, c.close, c.volume, c.taker_buy_vol, c.trades],
        )?;
        Ok(())
    }

    pub fn upsert_candles_batch(&self, candles: &[crate::decision::candle::Candle]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        for c in candles {
            tx.execute(
                "INSERT OR REPLACE INTO candles (open_time, close_time, open, high, low, close, volume, taker_buy_vol, trades)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![c.open_time, c.close_time, c.open, c.high, c.low, c.close, c.volume, c.taker_buy_vol, c.trades],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_latest_candles(&self, limit: usize) -> Result<Vec<crate::decision::candle::Candle>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT open_time, close_time, open, high, low, close, volume, taker_buy_vol, trades
             FROM candles ORDER BY open_time DESC LIMIT ?1"
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(crate::decision::candle::Candle {
                open_time: row.get(0)?,
                close_time: row.get(1)?,
                open: row.get(2)?,
                high: row.get(3)?,
                low: row.get(4)?,
                close: row.get(5)?,
                volume: row.get(6)?,
                taker_buy_vol: row.get(7)?,
                trades: row.get(8)?,
            })
        })?;
        let mut candles: Vec<_> = rows.filter_map(|r| r.ok()).collect();
        candles.reverse();
        Ok(candles)
    }

    pub fn get_candles_since(&self, since_open_time: i64) -> Result<Vec<crate::decision::candle::Candle>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT open_time, close_time, open, high, low, close, volume, taker_buy_vol, trades
             FROM candles WHERE open_time >= ?1 ORDER BY open_time"
        )?;
        let rows = stmt.query_map(params![since_open_time], |row| {
            Ok(crate::decision::candle::Candle {
                open_time: row.get(0)?,
                close_time: row.get(1)?,
                open: row.get(2)?,
                high: row.get(3)?,
                low: row.get(4)?,
                close: row.get(5)?,
                volume: row.get(6)?,
                taker_buy_vol: row.get(7)?,
                trades: row.get(8)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn insert_paper_trade(&self, trade: &PaperTrade) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO paper_trades (block_open_time, block_close_time, reference_price, resolution_price,
             regime, decision, open_odds_yes, open_odds_no, outcome, pnl, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![trade.block_open_time, trade.block_close_time, trade.reference_price,
                    trade.resolution_price, trade.regime, trade.decision,
                    trade.open_odds_yes, trade.open_odds_no, trade.outcome, trade.pnl, trade.created_at],
        )?;
        Ok(())
    }

    pub fn update_trade_outcome(&self, id: i64, outcome: &str, pnl: f64, resolution_price: f64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE paper_trades SET outcome = ?1, pnl = ?2, resolution_price = ?3 WHERE id = ?4",
            params![outcome, pnl, resolution_price, id],
        )?;
        Ok(())
    }

    pub fn get_unresolved_trades(&self) -> Result<Vec<PaperTrade>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, block_open_time, block_close_time, reference_price, COALESCE(resolution_price,0),
                    regime, decision, open_odds_yes, open_odds_no, outcome, COALESCE(pnl,0), created_at
             FROM paper_trades WHERE outcome IS NULL ORDER BY block_open_time"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(PaperTrade {
                id: row.get(0)?,
                block_open_time: row.get(1)?,
                block_close_time: row.get(2)?,
                reference_price: row.get(3)?,
                resolution_price: row.get(4)?,
                regime: row.get(5)?,
                decision: row.get(6)?,
                open_odds_yes: row.get(7)?,
                open_odds_no: row.get(8)?,
                outcome: row.get(9)?,
                pnl: row.get(10)?,
                created_at: row.get(11)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn insert_regime_snapshot(&self, s: &RegimeSnapshot) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO regime_snapshots (block_open_time, state, bull_score, bear_score,
             spread_7_25, slope_25, close_pos, green_8, red_8)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![s.block_open_time, s.state, s.bull_score, s.bear_score,
                    s.spread_7_25, s.slope_25, s.close_pos, s.green_8, s.red_8],
        )?;
        Ok(())
    }

    pub fn get_summary(&self) -> Result<(i64, i64, i64, i64, f64)> {
        let conn = self.conn.lock().unwrap();
        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM paper_trades WHERE decision NOT IN ('SKIP', 'Skip')", [], |r| r.get(0))?;
        let wins: i64 = conn.query_row(
            "SELECT COUNT(*) FROM paper_trades WHERE outcome = 'WIN'", [], |r| r.get(0))?;
        let losses: i64 = conn.query_row(
            "SELECT COUNT(*) FROM paper_trades WHERE outcome = 'LOSS'", [], |r| r.get(0))?;
        let skips: i64 = conn.query_row(
            "SELECT COUNT(*) FROM paper_trades WHERE decision IN ('SKIP', 'Skip')", [], |r| r.get(0))?;
        let total_pnl: f64 = conn.query_row(
            "SELECT COALESCE(SUM(pnl), 0) FROM paper_trades WHERE outcome IS NOT NULL", [], |r| r.get(0))?;
        Ok((total, wins, losses, skips, total_pnl))
    }
}
