use serde::{Deserialize, Serialize};

/// A paper trade record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperTrade {
    pub id: i64,
    pub block_open_time: i64,    // Binance open_time of the 15m block
    pub block_close_time: i64,   // Binance close_time
    pub reference_price: f64,    // Binance open price at block start
    pub resolution_price: f64,   // Binance close price at block end
    pub regime: String,          // "BULL", "BEAR", "NEUTRAL"
    pub decision: String,        // "YES", "NO", "SKIP"
    pub open_odds_yes: Option<f64>,  // PolyMarket CLOB odds for YES at t+10ms
    pub open_odds_no: Option<f64>,   // PolyMarket CLOB odds for NO at t+10ms
    pub outcome: Option<String>,     // "WIN", "LOSS", null if SKIP or unresolved
    pub pnl: Option<f64>,            // simulated PnL
    pub created_at: String,
}

/// A regime state snapshot for analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegimeSnapshot {
    pub id: i64,
    pub block_open_time: i64,
    pub state: String,
    pub bull_score: Option<u32>,
    pub bear_score: Option<u32>,
    pub spread_7_25: f64,
    pub slope_25: f64,
    pub close_pos: f64,
    pub green_8: u32,
    pub red_8: u32,
}
