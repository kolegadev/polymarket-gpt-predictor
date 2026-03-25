use anyhow::Result;
use tracing::info;
use rusqlite::params;
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

    /// Resolve a specific trade by its block open time.
    pub fn resolve_trade_by_block(&self, block_open_time: i64, close_price: f64) -> Result<Option<(String, f64)>> {
        let conn = self.db.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, decision, open_odds_yes, open_odds_no, reference_price
             FROM paper_trades WHERE block_open_time = ?1 AND outcome IS NULL"
        )?;
        let rows: Vec<_> = stmt.query_map(params![block_open_time], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<f64>>(2)?,
                row.get::<_, Option<f64>>(3)?,
                row.get::<_, f64>(4)?,
            ))
        })?.flatten().collect();

        if rows.is_empty() {
            info!("No unresolved trade for block {}", block_open_time);
            return Ok(None);
        }

        for (id, decision, odds_yes, odds_no, ref_price) in rows {
            let went_up = close_price > ref_price;
            let went_down = close_price < ref_price;
            info!("Resolving trade {}: dec={} ref={:.2} close={:.2} up={} down={}", id, decision, ref_price, close_price, went_up, went_down);

            let (outcome, pnl) = match decision.as_str() {
                "YES" | "Yes" => {
                    if went_up {
                        let payout = odds_yes.unwrap_or(0.50);
                        ("WIN".to_string(), payout)
                    } else if went_down {
                        ("LOSS".to_string(), -1.0)
                    } else {
                        ("PUSH".to_string(), 0.0)
                    }
                }
                "NO" | "No" => {
                    if went_down {
                        let payout = odds_no.unwrap_or(0.50);
                        ("WIN".to_string(), payout)
                    } else if went_up {
                        ("LOSS".to_string(), -1.0)
                    } else {
                        ("PUSH".to_string(), 0.0)
                    }
                }
                _ => continue,
            };

            let rows_updated = conn.execute(
                "UPDATE paper_trades SET outcome = ?1, pnl = ?2, resolution_price = ?3 WHERE id = ?4",
                params![outcome, pnl, close_price, id],
            )?;
            info!("✅ Resolved trade {}: {} pnl={:.4} rows_updated={}", id, outcome, pnl, rows_updated);
            return Ok(Some((outcome, pnl)));
        }
        Ok(None)
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
