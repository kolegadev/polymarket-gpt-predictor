def compute_features(candles, t):
    c = candles[t]
    sma7 = SMA(close, 7, t)
    sma25 = SMA(close, 25, t)
    sma99 = SMA(close, 99, t)
    atr = ATR14(candles, t)

    spread_7_25 = (sma7 - sma25) / atr
    slope_25 = (sma25 - SMA(close, 25, t - 4)) / atr
    slope_99 = (sma99 - SMA(close, 99, t - 8)) / atr
    dist_close_25 = (c.close - sma25) / atr

    rng = max(c.high - c.low, 1e-9)
    body = abs(c.close - c.open)
    upper_wick = c.high - max(c.open, c.close)
    lower_wick = min(c.open, c.close) - c.low
    close_pos = (c.close - c.low) / rng

    green_8 = sum(1 for i in range(t - 7, t + 1) if candles[i].close > candles[i].open)
    red_8 = sum(1 for i in range(t - 7, t + 1) if candles[i].close < candles[i].open)

    return {
        "sma7": sma7,
        "sma25": sma25,
        "sma99": sma99,
        "atr": atr,
        "spread_7_25": spread_7_25,
        "slope_25": slope_25,
        "slope_99": slope_99,
        "dist_close_25": dist_close_25,
        "range": rng,
        "body": body,
        "upper_wick": upper_wick,
        "lower_wick": lower_wick,
        "close_pos": close_pos,
        "green_8": green_8,
        "red_8": red_8,
    }


def bull_candidate(c, f):
    return (
        f["sma7"] > f["sma25"] > f["sma99"]
        and f["spread_7_25"] >= 0.10
        and f["slope_25"] >= 0.08
        and f["slope_99"] >= -0.03
        and c.close > f["sma25"]
    )


def bull_score(candles, t, f):
    c = candles[t]
    score = 0
    score += int(c.close >= f["sma7"])
    score += int(f["close_pos"] >= 0.55)
    score += int(f["green_8"] >= 5)
    score += int(c.low > candles[t - 2].low)
    score += int(c.high > candles[t - 2].high)
    score += int(f["dist_close_25"] <= 1.40)
    return score


def bear_candidate(c, f):
    return (
        f["sma7"] < f["sma25"] < f["sma99"]
        and f["spread_7_25"] <= -0.10
        and f["slope_25"] <= -0.08
        and c.close < f["sma25"]
        and c.close < f["sma99"]
    )


def bear_score(candles, t, f):
    c = candles[t]
    score = 0
    score += int(c.close <= f["sma7"])
    score += int(f["close_pos"] <= 0.45)
    score += int(f["red_8"] >= 4)
    score += int(c.low < candles[t - 2].low)
    score += int(c.high < candles[t - 2].high)
    score += int((f["sma25"] - c.close) / f["atr"] <= 1.20)
    return score


def bull_warnings(candles, t, f, prev_f, prev2_f):
    c = candles[t]
    warnings = 0
    warnings += int(c.close < f["sma7"])
    warnings += int(f["close_pos"] < 0.45)
    warnings += int(f["green_8"] <= 4)
    warnings += int(c.high <= candles[t - 1].high)
    warnings += int(f["spread_7_25"] < prev_f["spread_7_25"] < prev2_f["spread_7_25"])
    return warnings


def bull_yes_filter(f):
    return (
        f["close_pos"] >= 0.50
        and f["dist_close_25"] <= 1.40
        and (f["range"] / f["atr"]) <= 1.80
        and (f["upper_wick"] / f["range"]) <= 0.35
    )


def bear_no_filter(candles, t, f):
    c = candles[t]
    return (
        c.high >= f["sma7"] - 0.10 * f["atr"]
        and c.close < c.open
        and f["close_pos"] <= 0.35
        and (f["upper_wick"] / f["range"]) >= 0.20
        and c.low < candles[t - 1].low
        and ((f["sma25"] - c.close) / f["atr"]) <= 1.20
        and (f["lower_wick"] / f["range"]) <= 0.30
    )


def next_state(prev_state, candles, t, f_t, f_t1, f_t2):
    c = candles[t]

    # Hard bull exit
    if prev_state == "BULL":
        if c.close < f_t["sma25"] or f_t["sma7"] < f_t["sma25"] or f_t["slope_25"] < 0:
            return "NEUTRAL"
        if bull_warnings(candles, t, f_t, f_t1, f_t2) >= 2:
            return "NEUTRAL"

    # Hard bear exit
    if prev_state == "BEAR":
        if c.close > f_t["sma25"] or f_t["sma7"] > f_t["sma25"] or f_t["slope_25"] > 0:
            return "NEUTRAL"

    bull_ok = (
        bull_candidate(c, f_t)
        and bull_candidate(candles[t - 1], f_t1)
        and bull_score(candles, t, f_t) >= 4
        and bull_score(candles, t - 1, f_t1) >= 4
    )
    bear_ok = (
        bear_candidate(c, f_t)
        and bear_candidate(candles[t - 1], f_t1)
        and bear_score(candles, t, f_t) >= 4
        and bear_score(candles, t - 1, f_t1) >= 4
    )

    if bull_ok:
        return "BULL"
    if bear_ok:
        return "BEAR"
    return "NEUTRAL"


def decide_next_bar(prev_state, candles, t, f_t, f_t1, f_t2):
    state = next_state(prev_state, candles, t, f_t, f_t1, f_t2)

    if state == "BULL" and bull_yes_filter(f_t):
        return state, "YES"

    if state == "BEAR" and bear_no_filter(candles, t, f_t):
        return state, "NO"

    return state, "SKIP"
