use anyhow::Result;
use reqwest::Client;
use serde::Deserialize;
use tracing::{info, warn};

/// PolyMarket Gamma API client — fetches real odds for BTC 5m UpDown markets.
pub struct GammaClient {
    client: Client,
}

#[derive(Debug, Clone)]
pub struct MarketOdds {
    pub yes_price: f64,
    pub no_price: f64,
    pub yes_best_bid: f64,
    pub yes_best_ask: f64,
    pub no_best_bid: f64,
    pub no_best_ask: f64,
    pub spread: f64,
    pub slug: String,
    pub fetched_at_ms: i64,
}

#[derive(Debug, Deserialize)]
struct MarketResponse {
    slug: String,
    outcome_prices: Vec<String>,
    clob_token_ids: Vec<String>,
    best_bid: Option<f64>,
    best_ask: Option<f64>,
    spread: Option<f64>,
    outcomes: Option<Vec<String>>,
    accepting_orders: Option<bool>,
    volume: Option<String>,
}

impl GammaClient {
    pub fn new() -> Self {
        Self { client: Client::new() }
    }

    /// Fetch odds for a specific 5m block.
    pub async fn get_odds(&self, block_open_time_ms: i64) -> Result<MarketOdds> {
        let block_secs = block_open_time_ms / 1000;
        let slug = format!("btc-updown-5m-{}", block_secs);

        let url = format!(
            "https://gamma-api.polymarket.com/markets?slug={}&closed=false&limit=1",
            slug
        );

        let start = chrono::Utc::now().timestamp_millis();
        let resp: Vec<MarketResponse> = self.client.get(&url)
            .timeout(std::time::Duration::from_secs(3))
            .send().await?
            .json().await?;

        let market = resp.first()
            .filter(|m| m.accepting_orders.unwrap_or(false))
            .ok_or_else(|| anyhow::anyhow!("No active market for block {}", block_secs))?;

        let yes_price = market.outcome_prices.first()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.505);
        let no_price = market.outcome_prices.get(1)
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.495);

        let yes_bid = market.best_bid.unwrap_or(yes_price - 0.005);
        let yes_ask = market.best_ask.unwrap_or(yes_price + 0.005);
        let no_bid = 1.0 - yes_ask;
        let no_ask = 1.0 - yes_bid;
        let spread = market.spread.unwrap_or(yes_ask - yes_bid);

        let elapsed = chrono::Utc::now().timestamp_millis() - start;
        info!(
            "CLOB [{:.0}ms] {} YES={:.3f} NO_bid={:.3f} NO_ask={:.3f} spread={:.3f}",
            elapsed, slug, yes_price, no_bid, no_ask, spread,
        );

        Ok(MarketOdds {
            yes_price, no_price,
            yes_best_bid: yes_bid, yes_best_ask: yes_ask,
            no_best_bid: no_bid, no_best_ask: no_ask,
            spread,
            slug,
            fetched_at_ms: start,
        })
    }
}
