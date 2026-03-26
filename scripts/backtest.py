#!/usr/bin/env python3
"""
Clean BEAR/BULL backtest against Binance 15m data.
- Reference price: candle.open (t=0)
- Resolution price: candle.close (t=900)
- Entry odds: 0.505 (WIN pays 0.495, LOSS costs 0.505)
- Taker ratio: taker_buy_vol / volume, lookback=4
- BEAR entry: ratio < threshold for 2+ consecutive bars
- BULL entry: ratio > (1-threshold) for 2+ consecutive bars
"""

import sqlite3, sys
from datetime import datetime

DB = "data/predictor.db"
ENTRY_ODDS = 0.505
WIN_PAYOUT = 1.0 - ENTRY_ODDS  # 0.495
LOOKBACK = 4
CONFIRM_BARS = 2

def taker_ratio(rows, t, lb):
    start = max(0, t - lb + 1)
    buy = sum(r[3] for r in rows[start:t+1])
    total = sum(r[2] for r in rows[start:t+1])
    return buy / total if total > 1e-12 else 0.50

def run_backtest(rows, bear_thresh, label):
    """rows = list of (open_time, open, close, volume, taker_buy_vol)"""
    eval_start = LOOKBACK + 2

    # State
    in_bear = False
    in_bull = False
    consec_bear = 0
    consec_bull = 0
    exit_bear = 0  # count bars above exit threshold
    exit_bull = 0  # count bars below exit threshold
    BEAR_EXIT_THRESH = 0.50
    BULL_EXIT_THRESH = 0.50

    # Track per-regime outcomes
    bear_green = 0; bear_red = 0; bear_bars = 0; bear_runs = 0; bear_profitable = 0
    bull_green = 0; bull_red = 0; bull_bars = 0; bull_runs = 0; bull_profitable = 0
    neutral_bars = 0

    # Per-run tracking
    current_run_state = None  # 'bear' or 'bull'
    current_run_green = 0
    current_run_red = 0
    run_results = []  # (state, green, red)

    for t in range(eval_start, len(rows)):
        open_time, open_p, close_p, vol, buy_vol = rows[t]
        ratio = taker_ratio(rows, t, LOOKBACK)
        bull_thresh = 1.0 - bear_thresh

        # Candle outcome: green if close >= open (PolyMarket: UP), red if close < open (DOWN)
        is_green = close_p >= open_p

        # Count consecutives
        if ratio < bear_thresh:
            consec_bear += 1
        else:
            consec_bear = 0

        if ratio > bull_thresh:
            consec_bull += 1
        else:
            consec_bull = 0

        # State transitions
        prev_state = in_bear and 'bear' or (in_bull and 'bull' or None)

        if consec_bear >= CONFIRM_BARS and not in_bear:
            in_bear = True
            in_bull = False
            exit_bear = 0
            current_run_state = 'bear'
            current_run_green = 0
            current_run_red = 0
            bear_runs += 1
        elif consec_bull >= CONFIRM_BARS and not in_bull:
            in_bull = True
            in_bear = False
            exit_bull = 0
            current_run_state = 'bull'
            current_run_green = 0
            current_run_red = 0
            bull_runs += 1

        # Exit conditions (require 2+ bars to confirm regime change)
        if in_bear:
            if ratio >= BEAR_EXIT_THRESH:
                exit_bear += 1
            else:
                exit_bear = 0
            if exit_bear >= 2:
                if current_run_green + current_run_red >= 2:
                    run_results.append(('bear', current_run_green, current_run_red))
                in_bear = False
                current_run_state = None
        if in_bull:
            if ratio <= BULL_EXIT_THRESH:
                exit_bull += 1
            else:
                exit_bull = 0
            if exit_bull >= 2:
                if current_run_green + current_run_red >= 2:
                    run_results.append(('bull', current_run_green, current_run_red))
                in_bull = False
                current_run_state = None

        # Count outcomes in active regime
        if in_bear:
            bear_bars += 1
            if is_green:
                bear_green += 1
                current_run_green += 1
            else:
                bear_red += 1
                current_run_red += 1
        elif in_bull:
            bull_bars += 1
            if is_green:
                bull_green += 1
                current_run_green += 1
            else:
                bull_red += 1
                current_run_red += 1
        else:
            neutral_bars += 1

    # Close final run
    if current_run_state and current_run_green + current_run_red >= 2:
        run_results.append((current_run_state, current_run_green, current_run_red))

    # Count profitable runs
    for state, g, r in run_results:
        total = g + r
        if state == 'bear' and r > g:
            bear_profitable += 1
        if state == 'bull' and g > r:
            bull_profitable += 1

    # Calculate results
    total = bear_bars + bull_bars + neutral_bars
    days = total / 96.0

    results = {}
    results['label'] = label
    results['days'] = days
    results['total_bars'] = total

    # BEAR stats
    results['bear_runs'] = bear_runs
    results['bear_bars'] = bear_bars
    results['bear_green'] = bear_green
    results['bear_red'] = bear_red
    results['bear_wr'] = bear_red / bear_bars * 100 if bear_bars > 0 else 0
    results['bear_profitable_pct'] = bear_profitable / bear_runs * 100 if bear_runs > 0 else 0
    # PnL: NO bet wins when close < open (red candle)
    # Win: +WIN_PAYOUT, Loss: -ENTRY_ODDS
    results['bear_pnl'] = (bear_red * WIN_PAYOUT) - (bear_green * ENTRY_ODDS)
    results['bear_pnl_per_trade'] = results['bear_pnl'] / bear_bars if bear_bars > 0 else 0

    # BULL stats
    results['bull_runs'] = bull_runs
    results['bull_bars'] = bull_bars
    results['bull_green'] = bull_green
    results['bull_red'] = bull_red
    results['bull_wr'] = bull_green / bull_bars * 100 if bull_bars > 0 else 0
    results['bull_profitable_pct'] = bull_profitable / bull_runs * 100 if bull_runs > 0 else 0
    # PnL: YES bet wins when close >= open (green candle)
    results['bull_pnl'] = (bull_green * WIN_PAYOUT) - (bull_red * ENTRY_ODDS)
    results['bull_pnl_per_trade'] = results['bull_pnl'] / bull_bars if bull_bars > 0 else 0

    results['neutral_bars'] = neutral_bars
    results['combined_pnl'] = results['bear_pnl'] + results['bull_pnl']
    results['combined_pnl_day'] = results['combined_pnl'] / days if days > 0 else 0

    return results

def main():
    c = sqlite3.connect(DB)
    rows = c.execute(
        "SELECT open_time, open, close, volume, taker_buy_vol FROM candles ORDER BY open_time"
    ).fetchall()

    print(f"Data: {len(rows)} candles ({len(rows)/96:.1f} days)")
    print(f"Resolution: candle.open vs candle.close (same block, PolyMarket style)")
    print(f"Entry odds: {ENTRY_ODDS} (WIN pays {WIN_PAYOUT}, LOSS costs {ENTRY_ODDS})")
    print(f"Taker lookback: {LOOKBACK}, confirm: {CONFIRM_BARS} bars")
    print()

    # Sweep bear thresholds
    thresholds = [0.42, 0.44, 0.45, 0.46, 0.47, 0.48, 0.49, 0.50]

    print("=" * 120)
    print(f"{'THRESH':>6} | {'BEAR':>4} {'BEAR':>5} {'BEAR':>5} {'BEAR':>5} {'BEAR':>5} {'BEAR':>6} {'BEAR':>8} | {'BULL':>4} {'BULL':>5} {'BULL':>5} {'BULL':>5} {'BULL':>6} {'BULL':>8} | {'COMB':>7}")
    print(f"{'':>6} | {'RUNS':>4} {'BARS':>5} {'GRN':>5} {'RED':>5} {'WR%':>5} {'PRO%':>6} {'PnL':>8} | {'RUNS':>4} {'BARS':>5} {'GRN':>5} {'RED':>5} {'WR%':>5} {'PnL':>8} | {'PnL':>7}")
    print("-" * 120)

    best_combined = None
    best_bear = None
    best_bull = None

    for thresh in thresholds:
        label = f"bb={thresh}"
        r = run_backtest(rows, thresh, label)

        marker = ""
        if r['combined_pnl'] > 0:
            marker = " <<<"
        if r['bear_pnl'] > 0:
            marker += " BEAR+"
        if r['bull_pnl'] > 0:
            marker += " BULL+"

        print(f"{thresh:>6.2f} | {r['bear_runs']:>4} {r['bear_bars']:>5} {r['bear_green']:>5} {r['bear_red']:>5} {r['bear_wr']:>5.1f} {r['bear_profitable_pct']:>5.0f}% {r['bear_pnl']:>+8.1f} | {r['bull_runs']:>4} {r['bull_bars']:>5} {r['bull_green']:>5} {r['bull_red']:>5} {r['bull_wr']:>5.1f} {r['bull_pnl']:>+8.1f} | {r['combined_pnl']:>+7.1f}{marker}")

        if best_combined is None or r['combined_pnl'] > best_combined['combined_pnl']:
            best_combined = r
        if best_bear is None or r['bear_pnl'] > best_bear['bear_pnl']:
            best_bear = r
        if best_bull is None or r['bull_pnl'] > best_bull['bull_pnl']:
            best_bull = r

    print("-" * 120)
    print()
    print("BEST COMBINED:")
    print(f"  {best_combined['label']}: combined PnL = {best_combined['combined_pnl']:+.1f} ({best_combined['combined_pnl_day']:+.2f}/day)")
    print(f"  BEAR: {best_combined['bear_bars']} bars, {best_combined['bear_wr']:.1f}% NO win rate, PnL={best_combined['bear_pnl']:+.1f}")
    print(f"  BULL: {best_combined['bull_bars']} bars, {best_combined['bull_wr']:.1f}% YES win rate, PnL={best_combined['bull_pnl']:+.1f}")
    print()
    print("BEST BEAR:")
    print(f"  {best_bear['label']}: {best_bear['bear_bars']} bars, {best_bear['bear_wr']:.1f}% NO WR, PnL={best_bear['bear_pnl']:+.1f}")
    print()
    print("BEST BULL:")
    print(f"  {best_bull['label']}: {best_bull['bull_bars']} bars, {best_bull['bull_wr']:.1f}% YES WR, PnL={best_bull['bull_pnl']:+.1f}")
    print()
    print(f"Break-even at {ENTRY_ODDS} odds: need >{ENTRY_ODDS*100:.1f}% win rate")
    print()

if __name__ == "__main__":
    main()
