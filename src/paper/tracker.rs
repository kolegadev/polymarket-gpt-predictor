use anyhow::Result;
use tracing::info;
use std::sync::Arc;
use crate::decision::state::{State, Decision};
use crate::db::Db;
use crate::types::models::{PaperTrade, RegimeSnapshot};
use crate::decision::features::Features;

/// Paper trade manager — tracks decisions and resolves outcomes.
pub struct PaperTracker {
    db: Arc<Db>,
}

impl PaperTracker {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }

    /// Record a paper trade decision for the next 15m block.
    pub fn record_decision(
        &self,
        block_open_time: i64,
        block_close_time: i64,
        reference_price: f64,
        regime: State,
        decision: Decision,
        open_odds_yes: Option<f64>,
        open_odds_no: Option<f64>,
        features: &Features,
        bull_score: Option<u32>,
        bear_score: Option<u32>,
    ) -> Result<()> {
        let trade = PaperTrade {
            id: 0,
            block_open_time,
            block_close_time,
            reference_price,
            resolution_price: 0.0,
            regime: format!("{:?}", regime),
            decision: format!("{:?}", decision),
            open_odds_yes,
            open_odds_no,
            outcome: None,
            pnl: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        self.db.insert_paper_trade(&trade)?;

        // Also record regime snapshot
        let snapshot = RegimeSnapshot {
            id: 0,
            block_open_time,
            state: format!("{:?}", regime),
            bull_score,
            bear_score,
            spread_7_25: features.spread_7_25,
            slope_25: features.slope_25,
            close_pos: features.close_pos,
            green_8: features.green_8,
            red_8: features.red_8,
        };
        self.db.insert_regime_snapshot(&snapshot)?;

        info!(
            "📝 Paper trade: block={} regime={:?} decision={:?} ref_price={:.2} odds_yes={:?} odds_no={:?}",
            block_open_time, regime, decision, reference_price, open_odds_yes, open_odds_no
        );

        Ok(())
    }

    /// Resolve any unresolved trades using the actual close price.
    pub fn resolve_trades(&self, close_price: f64) -> Result<usize> {
        let trades = self.db.get_unresolved_trades()?;
        let mut resolved = 0;

        for trade in &trades {
            // Determine outcome: YES wins if close > open (Up), NO wins if close < open (Down)
            let went_up = close_price > trade.reference_price;
            let went_down = close_price < trade.reference_price;

            let (outcome, pnl) = match trade.decision.as_str() {
                "YES" => {
                    if went_up {
                        let profit = trade.open_odds_yes.unwrap_or(0.50); // payout at odds
                        ("WIN".to_string(), profit - 1.0) // net: win odds - 1
                    } else if went_down {
                        ("LOSS".to_string(), -1.0) // lose stake
                    } else {
                        // Doji — push, return stake
                        ("PUSH".to_string(), 0.0)
                    }
                }
                "NO" => {
                    if went_down {
                        let profit = trade.open_odds_no.unwrap_or(0.50);
                        ("WIN".to_string(), profit - 1.0)
                    } else if went_up {
                        ("LOSS".to_string(), -1.0)
                    } else {
                        ("PUSH".to_string(), 0.0)
                    }
                }
                _ => continue, // SKIP trades don't need resolution
            };

            self.db.update_trade_outcome(trade.id, &outcome, pnl, close_price)?;
            resolved += 1;
        }

        if resolved > 0 {
            info!("Resolved {} paper trades with close_price={:.2}", resolved, close_price);
        }
        Ok(resolved)
    }

    /// Print a summary of paper trade performance.
    pub fn print_summary(&self) -> Result<()> {
        let (total, wins, losses, skips, total_pnl) = self.db.get_summary()?;

        let win_rate = if total > 0 { wins as f64 / total as f64 * 100.0 } else { 0.0 };

        info!("═══════════════════════════════════════════");
        info!("📊 PAPER TRADE SUMMARY");
        info!("═══════════════════════════════════════════");
        info!("Total bets: {}", total);
        info!("Wins: {} | Losses: {}", wins, losses);
        info!("Win rate: {:.1}%", win_rate);
        info!("Skipped blocks: {}", skips);
        info!("Total PnL: {:.4} units", total_pnl);
        info!("═══════════════════════════════════════════");

        Ok(())
    }
}
