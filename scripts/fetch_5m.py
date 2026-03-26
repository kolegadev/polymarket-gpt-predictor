import json, urllib.request, sqlite3, os

# Fetch 5m Binance.US data
all_candles = []
for i in range(3):
    url = 'https://api.binance.us/api/v3/klines?symbol=BTCUSDT&interval=5m&limit=1000'
    if all_candles:
        url += '&endTime=' + str(all_candles[-1][0] - 1)
    resp = urllib.request.urlopen(url).read()
    data = json.loads(resp)
    if not data:
        break
    for r in data:
        all_candles.append((
            int(r[0]), int(r[6]),
            float(r[1]), float(r[2]), float(r[3]), float(r[4]),
            float(r[5]), float(r[9]), int(r[8])
        ))
    print(f'Batch {i+1}: {len(data)} candles')

print(f'Total: {len(all_candles)} candles ({len(all_candles)/288:.1f} days)')

os.makedirs('data', exist_ok=True)
c = sqlite3.connect('data/predictor_5m.db')
c.execute('''CREATE TABLE IF NOT EXISTS candles (
    open_time INTEGER PRIMARY KEY, close_time INTEGER NOT NULL,
    open REAL NOT NULL, high REAL NOT NULL, low REAL NOT NULL,
    close REAL NOT NULL, volume REAL NOT NULL,
    taker_buy_vol REAL NOT NULL, trades INTEGER NOT NULL
)''')
c.executemany('INSERT OR REPLACE INTO candles VALUES (?,?,?,?,?,?,?,?,?)', all_candles)
c.commit()

total = c.execute('SELECT count(*) FROM candles').fetchone()[0]
zero = c.execute('SELECT count(*) FROM candles WHERE volume < 0.001').fetchone()[0]
print(f'DB: {total} candles, {zero} zero-vol ({zero/total*100:.1f}%)')

# Ratio distribution
rows = c.execute('SELECT volume, taker_buy_vol FROM candles ORDER BY open_time').fetchall()
ratios = []
for i in range(4, len(rows)):
    buy = sum(rows[j][1] for j in range(i-3, i+1))
    total_v = sum(rows[j][0] for j in range(i-3, i+1))
    if total_v > 0.001:
        ratios.append(buy/total_v)

ratios.sort()
n = len(ratios)
print(f'\nRatio distribution (lb=4):')
print(f'  P5:  {ratios[int(n*0.05)]:.4f}')
print(f'  P25: {ratios[int(n*0.25)]:.4f}')
print(f'  P50: {ratios[int(n*0.50)]:.4f}')
print(f'  P75: {ratios[int(n*0.75)]:.4f}')
print(f'  P95: {ratios[int(n*0.95)]:.4f}')
print(f'  Below 0.45: {sum(1 for r in ratios if r < 0.45)} ({sum(1 for r in ratios if r < 0.45)/n*100:.1f}%)')
print(f'  Above 0.55: {sum(1 for r in ratios if r > 0.55)} ({sum(1 for r in ratios if r > 0.55)/n*100:.1f}%)')

# Recent
print('\nRecent:')
for v, bv in reversed(rows[-10:]):
    r = bv/v if v > 0.001 else 0
    print(f'  vol={v:.4f} buy={bv:.4f} ratio={r:.4f}')
