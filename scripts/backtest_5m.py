#!/usr/bin/env python3
"""
5m BTC backtest — open vs close (PolyMarket resolution).
DB: data/predictor_5m.db (Binance.US 5m candles).
Entry odds: 0.505 (WIN pays 0.495, LOSS costs 0.505).
"""
import sqlite3

DB = "data/predictor_5m.db"
ODDS = 0.505
PAYOUT = 1.0 - ODDS  # 0.495

def main():
    c = sqlite3.connect(DB)
    raw = c.execute(
        "SELECT open, close, volume, taker_buy_vol FROM candles ORDER BY open_time"
    ).fetchall()

    # Filter to valid volume candles, extract fields
    candles = []
    for i, r in enumerate(raw):
        if r[2] >= 0.001:
            candles.append((i, r))  # (original_idx, (open, close, vol, buy_vol))
    N = len(candles)
    days = N / 288.0
    print(f"Candles: {len(raw)} total, {N} valid ({days:.1f} days)")
    print(f"Resolution: close >= open = Up (YES), close < open = Down (NO), odds={ODDS}")
    print()

    def ratio(t, lb=4):
        start = max(0, t - lb + 1)
        buy = sum(candles[j][1][3] for j in range(start, t + 1))
        vol = sum(candles[j][1][2] for j in range(start, t + 1))
        return buy / vol if vol > 1e-6 else 0.50

    def backtest(bear_thresh):
        in_bear = in_bull = False
        cb = cub = eb = xb = 0  # consec bear/bull, exit bear/bull
        bg = br = bb_ = 0       # bear green/red/bars
        ug = ur = ub_ = 0       # bull green/red/bars
        neu = 0
        runs = []  # (state, green, red)
        rs = rg = rr = 0  # current run

        for t in range(6, N):
            _, r = candles[t]
            op, cl = r[0], r[1]  # open, close
            is_green = cl >= op
            tr = ratio(t)

            if tr < bear_thresh: cb += 1
            else: cb = 0
            if tr > (1.0 - bear_thresh): cub += 1
            else: cub = 0

            # Entry
            if cb >= 2 and not in_bear:
                in_bear = True; in_bull = False; eb = 0
                rs = rg = rr = 0
            elif cub >= 2 and not in_bull:
                in_bull = True; in_bear = False; xb = 0
                rs = rg = rr = 0

            # Exit (2 bars confirming reversal)
            if in_bear:
                eb = eb + 1 if tr >= 0.50 else 0
                if eb >= 2:
                    if rs + rr >= 2: runs.append(('B', rs, rr))
                    in_bear = False
            if in_bull:
                xb = xb + 1 if tr <= 0.50 else 0
                if xb >= 2:
                    if rs + rr >= 2: runs.append(('L', rs, rr))
                    in_bull = False

            if in_bear:
                bb_ += 1
                if is_green: bg += 1; rg += 1
                else: br += 1; rr += 1
            elif in_bull:
                ub_ += 1
                if is_green: ug += 1; rg += 1
                else: ur += 1; rr += 1
            else:
                neu += 1

        bp = runs
        bpnl = (br * PAYOUT) - (bg * ODDS)
        lpnl = (ug * PAYOUT) - (ur * ODDS)
        return {
            'b_runs': sum(1 for s,_,_ in runs if s=='B'),
            'b_bars': bb_, 'b_green': bg, 'b_red': br,
            'b_wr': br/bb_*100 if bb_ else 0,
            'b_profit': sum(1 for s,g,r in runs if s=='B' and r>g),
            'b_pnl': bpnl,
            'l_runs': sum(1 for s,_,_ in runs if s=='L'),
            'l_bars': ub_, 'l_green': ug, 'l_red': ur,
            'l_wr': ug/ub_*100 if ub_ else 0,
            'l_profit': sum(1 for s,g,r in runs if s=='L' and g>r),
            'l_pnl': lpnl,
            'neu': neu, 'days': days,
            'runs': len(runs),
        }

    print(f"{'BB':>5} | {'BRuns':>5} {'BBars':>5} {'BGrn':>4} {'BRed':>4} {'BWR%':>4} {'BPr%':>4} {'BPnL':>6} {'BPPT':>6} | {'LRuns':>5} {'LBars':>5} {'LGrn':>4} {'LRed':>4} {'LWR%':>4} {'LPnL':>6} {'LPPT':>6} | {'Tot':>6}")
    print("-" * 120)

    best = None
    for bt in [0.30, 0.35, 0.40, 0.42, 0.44, 0.45, 0.46, 0.48, 0.50]:
        r = backtest(bt)
        comb = r['b_pnl'] + r['l_pnl']
        m = ""
        if comb > 0: m = " <<<"
        if r['b_pnl'] > 0: m += " BEAR"
        if r['l_pnl'] > 0: m += " BULL"

        bppt = r['b_pnl']/r['b_bars'] if r['b_bars'] else 0
        lppt = r['l_pnl']/r['l_bars'] if r['l_bars'] else 0
        bprop = r['b_profit']*100/r['b_runs'] if r['b_runs'] else 0
        lprop = r['l_profit']*100/r['l_runs'] if r['l_runs'] else 0

        print(f"{bt:>5.2f} | {r['b_runs']:>5} {r['b_bars']:>5} {r['b_green']:>4} {r['b_red']:>4} {r['b_wr']:>4.1f} {bprop:>3.0f}% {r['b_pnl']:>+6.1f} {bppt:>+6.3f} | {r['l_runs']:>5} {r['l_bars']:>5} {r['l_green']:>4} {r['l_red']:>4} {r['l_wr']:>4.1f} {r['l_pnl']:>+6.1f} {lppt:>+6.3f} | {comb:>+6.1f}{m}")
        if best is None or comb > best[0]:
            best = (comb, r)

    if best:
        r = best[1]
        print("-" * 120)
        print(f"Best: bb={best[0]:.2f}")
        print(f"  BEAR: {r['b_runs']} runs, {r['b_bars']} bars, {r['b_wr']:.1f}% WR, PnL={r['b_pnl']:+.1f}")
        print(f"  BULL: {r['l_runs']} runs, {r['l_bars']} bars, {r['l_wr']:.1f}% WR, PnL={r['l_pnl']:+.1f}")
        print(f"  Combined: {best[0]:+.1f} ({best[0]/r['days']:+.2f}/day)")
        print(f"  Runs: {r['runs']}, Neutral: {r['neu']} bars ({r['neu']/N*100:.0f}%)")

    print(f"\nBreak-even at {ODDS} odds: need >{ODDS*100:.1f}% win rate")

if __name__ == "__main__":
    main()
