use anyhow::Result;
use reqwest::Client;
use serde::Deserialize;
use tracing::info;

/// PolyMarket Gamma API CLOB client (read-only for paper trading).
pub struct ClobClient {
    client: Client,
    base_url: String,
}

/// Best bid/ask from the CLOB order book.
#[derive(Debug, Clone)]
pub struct OrderBookPrices {
    pub best_bid: f64,
    pub best_ask: f64,
    pub mid: f64,
}

#[derive(Debug, Deserialize)]
struct BookResponse {
    bids: Vec<Vec<String>>,
    asks: Vec<Vec<String>>,
}

impl ClobClient {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            base_url: "https://clob.polymarket.com".to_string(),
        }
    }

    /// Fetch the best bid/ask for a given token ID.
    /// Returns odds (0.0 to 1.0) where YES odds = 1 - NO odds.
    pub async fn get_best_prices(&self, token_id: &str) -> Result<OrderBookPrices> {
        let url = format!("{}/book?token_id={}", self.base_url, token_id);
        let resp: BookResponse = self.client.get(&url).send().await?.json().await?;

        let best_bid = resp.bids
            .first()
            .and_then(|b| b.first())
            .and_then(|p| p.parse::<f64>().ok())
            .unwrap_or(0.0);

        let best_ask = resp.asks
            .first()
            .and_then(|a| a.first())
            .and_then(|p| p.parse::<f64>().ok())
            .unwrap_or(0.0);

        let mid = (best_bid + best_ask) / 2.0;

        Ok(OrderBookPrices { best_bid, best_ask, mid })
    }

    /// Fetch opening odds for a BTC 15m UpDown market.
    /// condition_id is the PolyMarket condition slug (e.g. "btc-15m-updown").
    /// We need to look up the token_id from the condition.
    pub async fn get_btc_15m_opening_odds(&self, block_open_time_secs: i64) -> Result<(f64, f64)> {
        // PolyMarket BTC 15m UpDown slug format: btc-updown-15m-{block_timestamp}
        // where block_timestamp is the Unix epoch in seconds
        let condition_slug = format!("btc-updown-15m-{}", block_open_time_secs);

        let gamma_url = format!(
            "https://gamma-api.polymarket.com/markets?slug={}&closed=false&limit=1",
            condition_slug
        );

        #[derive(Deserialize)]
        struct MarketResponse {
            id: String,
            tokens: Vec<TokenInfo>,
        }

        #[derive(Deserialize)]
        struct TokenInfo {
            token_id: String,
            outcome: String,
        }

        let resp: Vec<MarketResponse> = self.client.get(&gamma_url).send().await?.json().await?;

        let market = resp.first()
            .ok_or_else(|| anyhow::anyhow!("No active market found for {}", condition_slug))?;

        let mut yes_odds = 0.5;
        let mut no_odds = 0.5;

        for token in &market.tokens {
            let prices = self.get_best_prices(&token.token_id).await?;
            match token.outcome.as_str() {
                "Up" => yes_odds = prices.mid,
                "Down" => no_odds = prices.mid,
                _ => {}
            }
        }

        info!("CLOB odds — YES(Up): {:.4}, NO(Down): {:.4}", yes_odds, no_odds);
        Ok((yes_odds, no_odds))
    }

    /// Get odds for a specific 15m block using its open time.
    pub async fn get_odds_for_block(&self, block_open_time_ms: i64) -> Result<(f64, f64)> {
        let block_open_time_secs = block_open_time_ms / 1000;
        self.get_btc_15m_opening_odds(block_open_time_secs).await
    }
}
