# PolyMarket BTC 15m GPT Predictor

A Rust-based paper trading engine for PolyMarket's BTC 15-minute Up/Down prediction markets.

## Architecture

- **Binance WS** → real-time 15m BTC/USDT kline data (with REST bootstrap)
- **Decision Engine** → 3-state regime detector (BULL/BEAR/NEUTRAL) with ATR-normalized features
- **PolyMarket CLOB** → read-only odds fetching via Gamma API
- **Paper Tracker** → simulated trades with PnL tracking in SQLite

## Quick Start

```bash
# Bootstrap historical candles (5000 bars ≈ 52 days)
cargo run --release -- --bootstrap

# Run simulation on historical data
cargo run --release -- --simulate

# Print paper trade summary
cargo run --release -- --summary

# Live paper trading mode
cargo run --release
```

## CLI Options

| Flag | Description |
|------|-------------|
| `--bootstrap` | Fetch historical candles from Binance.US REST and exit |
| `--simulate` | Run decision engine on historical data in DB |
| `--summary` | Print paper trade performance summary |
| `--db-path` | SQLite DB path (default: `data/predictor.db`) |
| `--log-level` | trace/debug/info/warn (default: `info`) |
| `--market-slug` | PolyMarket market slug |

## Strategy (V1)

- **BULL regime**: MA7 > MA25 > MA99, 2-bar confirmation → YES bets with health filter
- **BEAR regime**: MA7 < MA25 < MA99, 2-bar confirmation → NO bets only on failed bounce setups
- **NEUTRAL**: SKIP all bets
- All thresholds ATR-normalized, no raw BTC dollar values

See [docs/01-strategy-design.md](docs/01-strategy-design.md) for full spec.
