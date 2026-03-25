# PolyMarket BTC 15M UpDown Prediction Engine — Strategy Design

*Source: ChatGPT development thread, March 2025*

## Core Thesis

- **BULL Regime**: ~2:1 green:red candle ratio → bet YES on most 15m blocks
- **BEAR Regime**: ~1:1 green:red → NO only on specific continuation setups
- **NEUTRAL**: Skip — preserves edge

Key principle: candle color ratio is a *symptom* of regime, not the regime definition itself.

## 3-State Engine

| State | Action |
|-------|--------|
| BULL  | YES on most bars (with health filter) |
| BEAR  | NO only on selected continuation setups |
| NEUTRAL | SKIP |

## Version 1 — Inputs (Close-of-Bar Only)

- BTC/USDT 15m OHLCV candles
- SMA7, SMA25, SMA99
- ATR14
- Candle geometry (body, wicks, close position)
- Rolling green/red count (8-bar window)

No order book, delta, or lower timeframe data for V1.

## Normalized Features

```
spread_7_25 = (SMA7 - SMA25) / ATR14
spread_25_99 = (SMA25 - SMA99) / ATR14
slope_25 = (SMA25[t] - SMA25[t-4]) / ATR14
slope_99 = (SMA99[t] - SMA99[t-8]) / ATR14
dist_close_25 = (close - SMA25) / ATR14
```

## BULL Regime

### Entry (2-bar confirmation — 1 bar late)

All mandatory conditions on bar t AND t-1:
- SMA7 > SMA25 > SMA99
- spread_7_25 >= 0.10
- slope_25 >= 0.08
- slope_99 >= -0.03
- close > SMA25

Plus bull_score >= 4 (max 6):
1. close >= SMA7
2. close_pos >= 0.55
3. green_8 >= 5
4. low > low[t-2]
5. high > high[t-2]
6. dist_close_25 <= 1.40

### Exit (1 bar early — sensitive)

Hard exit (any one):
- close < SMA25
- SMA7 < SMA25
- slope_25 < 0

Soft exit (2+ of 5 warnings):
1. close < SMA7
2. close_pos < 0.45
3. green_8 <= 4
4. high <= high[t-1]
5. spread_7_25 shrinking for 2 bars

### YES Bet Filter

Only YES if:
- close >= SMA7
- close_pos >= 0.50
- dist_close_25 <= 1.40
- range / ATR14 <= 1.80
- upper_wick / range <= 0.35

## BEAR Regime

### Entry (2-bar confirmation)

All mandatory on bar t AND t-1:
- SMA7 < SMA25 < SMA99
- spread_7_25 <= -0.10
- slope_25 <= -0.08
- close < SMA25
- close < SMA99

Plus bear_score >= 4 (max 6):
1. close <= SMA7
2. close_pos <= 0.45
3. red_8 >= 4
4. low < low[t-2]
5. high < high[t-2]
6. (SMA25 - close) / ATR14 <= 1.20

### Exit

Hard exit (any one):
- close > SMA25
- SMA7 > SMA25
- slope_25 > 0

### NO Bet Filter (continuation setup)

All must be true:
1. high >= SMA7 - 0.10 * ATR14 (push toward resistance)
2. close < open (red candle)
3. close_pos <= 0.35 (closed in lower third)
4. upper_wick / range >= 0.20 (rejection)
5. low < low[t-1] (downside continuation)
6. (SMA25 - close) / ATR14 <= 1.20 (not overextended)
7. lower_wick / range <= 0.30 (not a hammer)

## Execution Timing

- Decision computed at **close of bar t** (no intrabar peeking)
- Order submitted at **t+1 open + 10-50ms**
- Fail-safe: skip if decision not ready by +75ms
- Skip if entry price worse than ~0.56 (tunable)

## Architecture

1. **Binance data engine** — rolling candles, incremental indicator updates
2. **Decision engine** — state machine, YES/NO/SKIP per bar
3. **PolyMarket execution** — pre-connected, pre-staged orders

## Risk Controls

- Skip on incomplete data / clock drift > 100ms / late events
- Max 1 bet per 15m block
- Daily loss stop
- Consecutive-loss stop
- Kill switch on API errors
- Price drift filter

## Critical Alignment Check

Before implementation: confirm exact 15m block boundaries and price resolution convention between Binance source and PolyMarket market.

## Version Roadmap

- **V1**: MA stack + ATR + candle geometry + volume
- **V2**: + taker buy/sell, cumulative delta, order book imbalance, OI changes
- **V3**: + 1m/5m entry confirmation inside 15m block
