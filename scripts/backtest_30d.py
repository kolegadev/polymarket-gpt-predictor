#!/usr/bin/env python3
"""30-day BULL YES paper trade backtest with 3% PolyMarket fee.
Compares Kelly compounding vs flat $500 stake (5% of $10K starting)."""

import json, urllib.request, math
from datetime import datetime, timezone

def fetch_candles():
    all = []
    end = None
    for _ in range(15):
        url = 'https://api.binance.us/api/v3/klines?symbol=BTCUSDT&interval=5m&limit=1000'
        if end:
            url += f'&endTime={end}'
        data = json.loads(urllib.request.urlopen(url).read())
        if not data: break
        for r in data:
            if len(r) < 10: continue
            all.append({
                'open_time': r[0], 'open': float(r[1]), 'close': float(r[4]),
                'volume': float(r[5]), 'taker_buy_vol': float(r[9]),
            })
        end = data[0][0] - 1
    all.sort(key=lambda c: c['open_time'])
    latest = all[-1]['open_time']
    return [c for c in all if c['open_time'] >= latest - (30 * 86400 * 1000)]

BULL_RATIO, EXIT_RATIO, LOOKBACK, CONFIRM = 0.54, 0.50, 4, 2
POLY_FEE = 0.03
ODDS = 0.505
MAX_BET_PCT = 0.05
WIN_RATE = 0.555

def kelly_stake(balance):
    b = (1.0 - ODDS) / ODDS
    q = 1.0 - WIN_RATE
    frac = (WIN_RATE * b - q) / b * 0.5
    return min(balance * frac, balance * MAX_BET_PCT)

def run_backtest(candles, mode='kelly', flat_stake=500.0):
    balance = 10_000.0
    peak = balance
    max_dd = 0.0
    in_bull = consec_bull = consec_exit = 0
    bets = wins = losses = 0
    total_pnl = total_fees = total_gross = active = 0.0
    streak_w = streak_l = max_streak_w = max_streak_l = 0
    daily = {}

    for i in range(LOOKBACK + 2, len(candles) - 1):
        bv = sum(candles[j]['taker_buy_vol'] for j in range(i - LOOKBACK + 1, i + 1))
        tv = sum(candles[j]['volume'] for j in range(i - LOOKBACK + 1, i + 1))
        ratio = bv / tv if tv > 1e-6 else 0.50

        # Resolve
        if active > 0:
            up = candles[i]['close'] >= candles[i]['open']
            gross = active / ODDS - active
            fee = active * POLY_FEE
            if up:
                net = gross - fee
                balance += net
                wins += 1
                total_gross += gross
                streak_w += 1
                streak_l = 0
                if streak_w > max_streak_w: max_streak_w = streak_w
            else:
                net_loss = active + fee
                balance -= net_loss
                losses += 1
                total_gross -= active
                streak_l += 1
                streak_w = 0
                if streak_l > max_streak_l: max_streak_l = streak_l
                net = -net_loss
            total_pnl += net
            total_fees += fee
            if balance > peak: peak = balance
            dd = (peak - balance) / peak * 100
            if dd > max_dd: max_dd = dd

        # Regime
        if in_bull:
            consec_exit = consec_exit + 1 if ratio < EXIT_RATIO else 0
            consec_bull = consec_bull + 1 if ratio > BULL_RATIO else 0
            in_bull = consec_exit < CONFIRM
        else:
            consec_bull = consec_bull + 1 if ratio > BULL_RATIO else 0
            consec_exit = consec_exit + 1 if ratio < EXIT_RATIO else 0
            in_bull = consec_bull >= CONFIRM

        if in_bull:
            stake = kelly_stake(balance) if mode == 'kelly' else flat_stake
            if stake > 0.01 and balance >= stake + stake * POLY_FEE:
                active = stake
                bets += 1
                day = datetime.fromtimestamp(candles[i+1]['open_time']/1000, tz=timezone.utc).strftime('%Y-%m-%d')
                if day not in daily:
                    daily[day] = {'bets':0,'wins':0,'pnl':0.0,'fees':0.0,'bal_start':balance}
                daily[day]['bets'] += 1
                daily[day]['fees'] += stake * POLY_FEE
            else:
                active = 0.0
        else:
            active = 0.0

    return {
        'balance': balance, 'bets': bets, 'wins': wins, 'losses': losses,
        'pnl': total_pnl, 'fees': total_fees, 'gross_pnl': total_gross,
        'max_dd': max_dd, 'daily': daily,
        'max_streak_w': max_streak_w, 'max_streak_l': max_streak_l,
    }

# ── Run ──
print("Fetching candles...")
candles = fetch_candles()
dt0 = datetime.fromtimestamp(candles[0]['open_time']/1000, tz=timezone.utc)
dt1 = datetime.fromtimestamp(candles[-1]['open_time']/1000, tz=timezone.utc)
print(f"{len(candles)} candles: {dt0.strftime('%Y-%m-%d')} to {dt1.strftime('%Y-%m-%d')}")

for label, mode, stake in [("KELLY COMPOUND", "kelly", 0), ("FLAT $500 (5%)", "flat", 500)]:
    r = run_backtest(candles, mode, stake)
    wr = r['wins']/r['bets']*100 if r['bets'] else 0

    print()
    print("=" * 72)
    print(f"  30-DAY BULL YES BACKTEST — {label} (3% PolyMarket fee)")
    print("=" * 72)
    print(f"  Starting balance:   $10,000.00")
    print(f"  Final balance:      ${r['balance']:>14,.2f}")
    print(f"  Net PnL:            ${r['pnl']:>14,.2f}")
    print(f"  ROI:                {(r['balance']-10000)/10000*100:>+14.2f}%")
    print(f"  Gross PnL (no fee): ${r['gross_pnl']:>14,.2f}")
    print(f"  Total fees paid:    ${r['fees']:>14,.2f}")
    print(f"  Fee as % of gross:  {abs(r['fees']/r['gross_pnl']*100) if r['gross_pnl'] else 0:.1f}%")
    print()
    print(f"  Bets:               {r['bets']}")
    print(f"  Wins:               {r['wins']} ({wr:.1f}%)")
    print(f"  Losses:             {r['losses']}")
    print(f"  PnL per bet:        ${r['pnl']/r['bets']:,.2f}" if r['bets'] else "")
    print(f"  Fees per bet:       ${r['fees']/r['bets']:,.2f}" if r['bets'] else "")
    print(f"  Max drawdown:       {r['max_dd']:.1f}%")
    print(f"  Max win streak:     {r['max_streak_w']}")
    print(f"  Max loss streak:    {r['max_streak_l']}")
    print(f"  Avg bets/day:       {r['bets']/30:.0f}")
    print()

    # Weekly summary
    weeks = {}
    for day, s in sorted(r['daily'].items()):
        wn = (datetime.strptime(day,'%Y-%m-%d').timetuple().tm_yday - 1) // 7
        wk = f"Wk{(datetime.strptime(day,'%Y-%m-%d') - datetime(2026,1,1)).days // 7 + 1}"
        if wk not in weeks: weeks[wk] = {'bets':0,'wins':0,'pnl':0.0,'fees':0.0,'days':0}
        weeks[wk]['bets'] += s['bets']; weeks[wk]['wins'] += s['wins']
        weeks[wk]['pnl'] += s['pnl']; weeks[wk]['fees'] += s['fees']; weeks[wk]['days'] += 1

    print(f"  {'Week':<5} {'Days':>4} {'Bets':>5} {'WR':>7} {'Net PnL':>14} {'Fees':>14}")
    print(f"  {'-'*72}")
    for wk in sorted(weeks):
        s = weeks[wk]
        w = s['wins']/s['bets']*100 if s['bets'] else 0
        print(f"  {wk:<5} {s['days']:>4} {s['bets']:>5} {w:>6.1f}% ${s['pnl']:>13,.2f} ${s['fees']:>13,.2f}")
    print()

    # Break-even analysis
    print("  ── BREAK-EVEN ANALYSIS ──")
    print(f"  Break-even WR at {ODDS:.3f} odds + 3% fee:  {ODDS / (1 + 0.03):.1%}")
    print(f"  Actual WR:                              {wr:.1f}%")
    print(f"  Edge:                                   {wr - ODDS/(1+0.03)*100:.1f} percentage points")
    print()
