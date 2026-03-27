# BEAR Trader Research Notes

## Date: 2026-03-26

## Data
- 34 candles with OBI + taker ratio + known outcomes
- Base DOWN rate: 44.1%
- Data collected from Binance.US REST (candles) + depth API (OBI, 1s snapshots)

## Key Insights

### 1. OBI and Taker Ratio are Anti-Correlated
- obi5 vs taker_ratio_1: r = -0.54
- bid_ask_5 vs taker_ratio_1: r = -0.60
- They measure different things (order book positioning vs actual trade flow)
- This makes them complementary, not redundant

### 2. Taker Ratio Validates OBI Signals
When OBI and taker disagree, taker tends to be correct:
- Extreme bear OBI (< -0.41) + bearish taker (< 0.48) → only 25% DOWN (taker contradicts, no edge)
- Extreme bear OBI (< -0.41) + bullish taker (>= 0.48) → 50% DOWN (mixed signal, no edge)
- Moderate OBI (> -0.35) + bearish taker (< 0.45) → **67% DOWN** (taker confirms selling)
- Bounce zone OBI (-0.41..-0.35) + bullish taker (>= 0.48) → **20% DOWN** (80% UP, confirmed bounce)

### 3. Original BEAR Thresholds Were Wrong
The original strategy bet NO when OBI < -0.41 regardless of taker ratio. This lost 3 of 4 trades because:
- Extreme bear OBI often represents absorption (big bids absorbing selling, price doesn't follow)
- Without taker confirmation, the signal is noise

### 4. The Winning Combos
| Signal | DOWN% | n | Edge | Interpretation |
|--------|-------|---|------|----------------|
| OBI > -0.35 + Taker < 0.45 | 67% | 6 | +22.5pp | Neutral OBI + real selling = down |
| OBI_mom > 0.1 + Taker < 0.45 | 67% | 6 | +22.5pp | Fake OBI bounce + real selling = down |
| OBI -0.41..-0.35 + Taker >= 0.48 | 20% | 5 | -24.1pp | Exhaustion + buying = up |

### 5. Collinear Features (avoid using together)
- bid_ask_5 and bid5: r = 0.975 (same signal)
- volume and bid_ask_5: r = 0.894 (proxy for same thing)
- obi5_momentum and obi_late_minus_avg: r = 0.855 (same calculation)
- obi5 and bid_ask_5: r = 0.695 (related but distinct enough)

### 6. Slope/MA Features are Noise
From earlier 73-day analysis (without OBI):
- slope_25, slope_99 had no significant correlation with outcomes
- Mean reversion in green_red_ratio_8 (+0.076 corr) was the best non-OBI signal
- Suggestion: HMA or EMA could improve timing vs SMA lag

## Updated BEAR Strategy (v2)

### NO Bets (expect price to go DOWN)
```
OBI5 < -0.41 AND taker_ratio < 0.45
  → Extreme bear OBI + bearish taker = genuine selling pressure
```
- Wait... this combo had only 25% DOWN in our data. Skip this for now.

```
OBI5 > -0.35 AND taker_ratio < 0.45
  → Neutral OBI but real selling flow happening
```
- 67% DOWN, n=6. This is the best combo.

```
OBI5_momentum > 0.1 AND taker_ratio < 0.45
  → OBI looks like it's shifting bullish but taker says selling
```
- 67% DOWN, n=6. Fake bounce detection.

### YES Bets (expect price to go UP)
```
OBI5 between -0.41 and -0.35 AND taker_ratio >= 0.48
  → Exhaustion zone + confirmed buying = bounce
```
- 80% UP, n=5. Strongest signal.

### ELSE → SKIP

## Live Trading Results (BEAR v1)
- 7 trades: 1W - 4L, 2 SKIPs
- All 4 NO losses were in the OBI < -0.41 zone without taker filter
- The 1 YES win (bounce zone) was correctly predicted
- v2 with taker filter would have avoided 3 of 4 losses

## TODO
- [ ] Implement v2 thresholds in bear trader
- [ ] Collect more data (need 100+ candles per segment for significance)
- [ ] Test HMA/EMA as potential additional validators
- [ ] Investigate the U-shape with larger dataset (extreme OBI zones with more samples)
- [ ] Add OBI_20 as secondary signal (deeper book, less noise)
