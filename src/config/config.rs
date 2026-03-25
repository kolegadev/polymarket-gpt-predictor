use clap::Parser;
use serde::Deserialize;

#[derive(Parser, Debug)]
#[command(name = "polymarket-gpt-predictor", about = "BTC 15m UpDown prediction engine")]
pub struct Args {
    /// DB path for SQLite
    #[arg(long, default_value = "data/predictor.db")]
    pub db_path: String,

    /// Log level: trace, debug, info, warn
    #[arg(long, default_value = "info")]
    pub log_level: String,

    /// PolyMarket BTC 15m market slug
    #[arg(long, default_value = "will-bitcoin-price-go-up-or-down-in-the-next-15-minutes")]
    pub market_slug: String,

    /// Bootstrap: fetch historical candles from Binance and exit
    #[arg(long)]
    pub bootstrap: bool,

    /// Run diagnostics on historical data
    #[arg(long)]
    pub diagnose: bool,

    /// Simulate mode: run on historical data from DB
    #[arg(long)]
    pub simulate: bool,

    /// Print summary and exit
    #[arg(long)]
    pub summary: bool,

    /// Bet size in units (for PnL tracking)
    #[arg(long, default_value = "1.0")]
    pub bet_size: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub db_path: String,
    pub market_slug: String,
    pub bet_size: f64,
}

impl From<&Args> for Config {
    fn from(args: &Args) -> Self {
        Self {
            db_path: args.db_path.clone(),
            market_slug: args.market_slug.clone(),
            bet_size: args.bet_size,
        }
    }
}
