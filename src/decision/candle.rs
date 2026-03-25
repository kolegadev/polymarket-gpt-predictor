use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candle {
    pub open_time: i64,   // ms timestamp
    pub close_time: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub taker_buy_vol: f64,
    pub trades: u32,
}

impl Candle {
    pub fn is_green(&self) -> bool {
        self.close > self.open
    }
    pub fn is_red(&self) -> bool {
        self.close < self.open
    }
    pub fn range(&self) -> f64 {
        self.high - self.low
    }
    pub fn body(&self) -> f64 {
        (self.close - self.open).abs()
    }
    pub fn upper_wick(&self) -> f64 {
        self.high - self.open.max(self.close)
    }
    pub fn lower_wick(&self) -> f64 {
        self.open.min(self.close) - self.low
    }
    pub fn close_pos(&self) -> f64 {
        let rng = self.range();
        if rng < 1e-12 { 0.5 } else { (self.close - self.low) / rng }
    }
    pub fn dt_open(&self) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(self.open_time).unwrap_or_default()
    }
}
