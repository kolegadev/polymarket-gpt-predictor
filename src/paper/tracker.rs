use anyhow::Result;
use rusqlite::params;
use std::sync::Arc;
use tracing::info;
use crate::db::Db;
use crate::types::models::PaperTrade;

/// Kelly criterion position sizing for binary bets.
///
/// `stake` always represents **dollars at risk** (the amount you lose if wrong).
/// On PolyMarket, paying $stake at `odds` buys you `stake / odds` shares.
/// Win resolves at $1/share: payout = stake / odds, profit = stake × (1/odds - 1).
/// Loss: you lose your stake.
pub struct KellySizer {
    pub balance: f64,
    pub starting_balance: f64,
    pub max_bet_pct: f64,
    pub avg_odds: f64,
}

impl KellySizer {
    pub fn new(starting_balance: f64, max_bet_pct: f64, avg_odds: f64) -> Self {
        Self { balance: starting_balance, starting_balance, max_bet_pct, avg_odds }
    }

    /// Returns the dollar amount to risk on this bet.
    /// Uses half-Kelly for safety, capped at max_bet_pct of current balance.
    pub fn size(&self, win_prob: f64, odds: f64) -> f64 {
        if win_prob <= 0.0 || win_prob >= 1.0 || odds <= 1e-6 { return 0.0; }
        let b = (1.0 - odds) / odds;  // payout ratio: profit per dollar risked
        let q = 1.0 - win_prob;
        let kelly_frac = (win_prob * b - q) / b;
        let half_kelly_frac = kelly_frac * 0.5;
        let kelly_stake = self.balance * half_kelly_frac;
        let max_stake = self.balance * self.max_bet_pct;
        kelly_stake.min(max_stake).max(0.0)
    }

    /// Update balance after trade resolution.
    /// stake = dollars risked (lost if wrong).
    pub fn settle(&mut self, stake: f64, won: bool, odds: f64) {
        if won {
            // You risked `stake`, bought stake/odds shares, each resolves to $1
            let payout = stake / odds;
            self.balance += payout - stake;  // net profit
        } else {
            self.balance -= stake;  // you lose what you risked
        }
    }

    pub fn roi(&self) -> f64 {
        if self.starting_balance < 1e-6 { 0.0 }
        else { (self.balance - self.starting_balance) / self.starting_balance * 100.0 }
    }
}

pub struct PaperTracker {
    db: Arc<Db>,
    pub kelly: KellySizer,
}

impl PaperTracker {
    pub fn new(db: Arc<Db>, starting_balance: f64) -> Self {
        let balance = db.get_balance(starting_balance).unwrap_or(starting_balance);
        Self {
            db,
            kelly: KellySizer::new(balance, 0.05, 0.505),
        }
    }

    pub fn record_bet(
        &self,
        block_open_time: i64,
        block_close_time: i64,
        reference_price: f64,
        decision: &str,
        odds: Option<f64>,
        taker_ratio: f64,
        stake: f64,
    ) -> Result<()> {
        let odds_str = match odds {
            Some(o) => format!("{:.3}", o),
            None => "none".to_string(),
        };
        let conn = self.db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO paper_trades (block_open_time, block_close_time, reference_price,
             regime, decision, open_odds_no, outcome, pnl, stake, taker_ratio)
             VALUES (?1, ?2, ?3, 'BULL', ?4, ?5, NULL, NULL, ?6, ?7)",
            params![block_open_time, block_close_time, reference_price, decision, odds, stake, taker_ratio],
        )?;
        info!(
            "BET {} block={} risk=${:.2} ratio={:.3} odds={} bal=${:.2}",
            decision, block_open_time, stake, taker_ratio, odds_str,
            self.kelly.balance,
        );
        Ok(())
    }

    pub fn resolve_trade_by_block(
        &mut self,
        block_open_time: i64,
        open_price: f64,
        close_price: f64,
        stake: f64,
        odds: f64,
    ) -> Result<Option<(String, f64)>> {
        let conn = self.db.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, decision FROM paper_trades
             WHERE block_open_time = ?1 AND outcome IS NULL AND pnl IS NULL"
        )?;
        let rows: Vec<_> = stmt.query_map(params![block_open_time], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?.flatten().collect();

        if rows.is_empty() { return Ok(None); }

        for (id, decision) in rows {
            // UP if close >= open, DOWN if close < open (matches PolyMarket resolution)
            let went_down = close_price < open_price;
            let won = match decision.as_str() {
                "NO" | "No" => went_down,
                _ => !went_down,  // YES: win if price went up
            };

            let outcome = if won { "WIN".to_string() } else { "LOSS".to_string() };
            let pnl_units = if won {
                stake / odds - stake  // profit = stake × (1/odds - 1)
            } else {
                -stake  // lose full risk amount
            };

            conn.execute(
                "UPDATE paper_trades SET outcome = ?1, pnl = ?2, resolution_price = ?3, reference_price = ?4 WHERE id = ?5",
                params![outcome, pnl_units, close_price, open_price, id],
            )?;

            // Update Kelly balance
            self.kelly.settle(stake, won, odds);

            // Persist balance to DB
            let _ = self.db.set_balance(self.kelly.balance);

            info!(
                "  {} open={:.2} close={:.2} risk=${:.2} pnl={:+.2} bal=${:.2}",
                outcome, open_price, close_price, stake, pnl_units, self.kelly.balance,
            );
            return Ok(Some((outcome, pnl_units)));
        }
        Ok(None)
    }

    pub fn print_summary(&self) -> Result<()> {
        let conn = self.db.conn.lock().unwrap();
        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM paper_trades WHERE outcome IS NOT NULL", [], |r| r.get(0))?;
        let wins: i64 = conn.query_row(
            "SELECT COUNT(*) FROM paper_trades WHERE outcome = 'WIN'", [], |r| r.get(0))?;
        let losses: i64 = conn.query_row(
            "SELECT COUNT(*) FROM paper_trades WHERE outcome = 'LOSS'", [], |r| r.get(0))?;
        let total_pnl: f64 = conn.query_row(
            "SELECT COALESCE(SUM(pnl), 0) FROM paper_trades WHERE outcome IS NOT NULL", [], |r| r.get(0))?;
        let wr = if total > 0 { wins as f64 / total as f64 * 100.0 } else { 0.0 };

        info!("==================================================");
        info!("  BULL PAPER TRADE SUMMARY");
        info!("  Balance: ${:.2}  |  ROI: {:+.2}%", self.kelly.balance, self.kelly.roi());
        info!("  PnL: ${:.2}", total_pnl);
        info!("  Bets: {}  Wins: {}  Losses: {}", total, wins, losses);
        info!("  Win rate: {:.1}%", wr);
        info!("  PnL/trade: ${:.4}", if total > 0 { total_pnl / total as f64 } else { 0.0 });
        info!("==================================================");

        Ok(())
    }

    pub fn reset(&mut self, starting_balance: f64) -> Result<()> {
        let new_balance = self.db.reset_balance(starting_balance)?;
        self.kelly.balance = new_balance;
        self.kelly.starting_balance = starting_balance;
        info!("Balance reset to ${:.2}", new_balance);
        Ok(())
    }
}
