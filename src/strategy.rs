use crate::config::StrategyCfg;
use crate::types::Side;
use std::collections::{HashMap, VecDeque};

/// Strategigrensesnittet: får en kurs, svarer eventuelt med et signal.
/// Bytt strategi ved å implementere denne traiten — engine og UI er uendret.
pub trait Strategy: Send {
    fn name(&self) -> &'static str;
    /// Så strategien med historiske sluttkurser (eldst først) ved oppstart.
    fn seed(&mut self, symbol: &str, closes: &[f64]);
    fn on_price(&mut self, symbol: &str, price: f64) -> Option<Side>;
}

/// Klassisk SMA-krysning: kjøp når raskt glidende snitt krysser over tregt,
/// selg når det krysser under. Enkel, gjennomsiktig og fin som første
/// strategi å bygge videre fra — ikke et løfte om avkastning.
pub struct SmaCross {
    fast: usize,
    slow: usize,
    prices: HashMap<String, VecDeque<f64>>,
    /// Forrige tilstand per symbol: true hvis fast > slow.
    above: HashMap<String, bool>,
}

impl SmaCross {
    pub fn new(cfg: &StrategyCfg) -> Self {
        Self {
            fast: cfg.fast,
            slow: cfg.slow,
            prices: HashMap::new(),
            above: HashMap::new(),
        }
    }

    fn sma(window: &VecDeque<f64>, n: usize) -> Option<f64> {
        if window.len() < n {
            return None;
        }
        Some(window.iter().rev().take(n).sum::<f64>() / n as f64)
    }
}

impl Strategy for SmaCross {
    fn name(&self) -> &'static str {
        "sma_cross"
    }

    fn seed(&mut self, symbol: &str, closes: &[f64]) {
        let window = self.prices.entry(symbol.to_string()).or_default();
        for &c in closes {
            window.push_back(c);
            if window.len() > self.slow + 1 {
                window.pop_front();
            }
        }
        if let (Some(f), Some(s)) = (Self::sma(window, self.fast), Self::sma(window, self.slow)) {
            self.above.insert(symbol.to_string(), f > s);
        }
    }

    fn on_price(&mut self, symbol: &str, price: f64) -> Option<Side> {
        let window = self.prices.entry(symbol.to_string()).or_default();
        window.push_back(price);
        if window.len() > self.slow + 1 {
            window.pop_front();
        }
        let fast = Self::sma(window, self.fast)?;
        let slow = Self::sma(window, self.slow)?;
        let now_above = fast > slow;
        let was_above = self.above.insert(symbol.to_string(), now_above);

        match was_above {
            Some(false) if now_above => Some(Side::Buy),
            Some(true) if !now_above => Some(Side::Sell),
            _ => None,
        }
    }
}

/// RSI (Relative Strength Index): kjøp når RSI faller under kjøpsterskelen
/// (oversolgt), selg når den stiger over salgsterskelen (overkjøpt).
pub struct Rsi {
    period: usize,
    buy_below: f64,
    sell_above: f64,
    closes: HashMap<String, VecDeque<f64>>,
    prev: HashMap<String, f64>,
}

impl Rsi {
    pub fn new(cfg: &StrategyCfg) -> Self {
        Self {
            period: cfg.rsi_period,
            buy_below: cfg.rsi_buy_below,
            sell_above: cfg.rsi_sell_above,
            closes: HashMap::new(),
            prev: HashMap::new(),
        }
    }

    fn rsi(window: &VecDeque<f64>, period: usize) -> Option<f64> {
        if window.len() < period + 1 {
            return None;
        }
        let v: Vec<f64> = window.iter().copied().collect();
        let (mut gains, mut losses) = (0.0, 0.0);
        for pair in v.windows(2) {
            let d = pair[1] - pair[0];
            if d >= 0.0 {
                gains += d;
            } else {
                losses -= d;
            }
        }
        if losses == 0.0 {
            return Some(100.0);
        }
        let rs = gains / losses;
        Some(100.0 - 100.0 / (1.0 + rs))
    }

    fn push(&mut self, symbol: &str, price: f64) -> Option<f64> {
        let window = self.closes.entry(symbol.to_string()).or_default();
        window.push_back(price);
        let cap = self.period + 1;
        if window.len() > cap {
            window.pop_front();
        }
        Self::rsi(window, self.period)
    }
}

impl Strategy for Rsi {
    fn name(&self) -> &'static str {
        "rsi"
    }

    fn seed(&mut self, symbol: &str, closes: &[f64]) {
        for &c in closes {
            if let Some(rsi) = self.push(symbol, c) {
                self.prev.insert(symbol.to_string(), rsi);
            }
        }
    }

    fn on_price(&mut self, symbol: &str, price: f64) -> Option<Side> {
        let rsi = self.push(symbol, price)?;
        let prev = self.prev.insert(symbol.to_string(), rsi);
        let prev = prev?;
        if prev >= self.buy_below && rsi < self.buy_below {
            Some(Side::Buy)
        } else if prev <= self.sell_above && rsi > self.sell_above {
            Some(Side::Sell)
        } else {
            None
        }
    }
}

/// Momentum/brudd: kjøp når kursen bryter over høyeste i vinduet,
/// selg når den bryter under laveste.
pub struct Momentum {
    window: usize,
    closes: HashMap<String, VecDeque<f64>>,
}

impl Momentum {
    pub fn new(cfg: &StrategyCfg) -> Self {
        Self {
            window: cfg.momentum_window,
            closes: HashMap::new(),
        }
    }

    fn push(&mut self, symbol: &str, price: f64) {
        let w = self.closes.entry(symbol.to_string()).or_default();
        w.push_back(price);
        if w.len() > self.window {
            w.pop_front();
        }
    }
}

impl Strategy for Momentum {
    fn name(&self) -> &'static str {
        "momentum"
    }

    fn seed(&mut self, symbol: &str, closes: &[f64]) {
        for &c in closes {
            self.push(symbol, c);
        }
    }

    fn on_price(&mut self, symbol: &str, price: f64) -> Option<Side> {
        let signal = {
            let w = self.closes.entry(symbol.to_string()).or_default();
            if w.len() >= self.window {
                let max = w.iter().cloned().fold(f64::MIN, f64::max);
                let min = w.iter().cloned().fold(f64::MAX, f64::min);
                if price > max {
                    Some(Side::Buy)
                } else if price < min {
                    Some(Side::Sell)
                } else {
                    None
                }
            } else {
                None
            }
        };
        self.push(symbol, price);
        signal
    }
}

/// MACD (12/26/9): kjøp når MACD-linjen krysser over signallinjen,
/// selg ved kryss under — klassikeren for å fange trendskifter.
pub struct Macd {
    /// Per symbol: (EMA 12, EMA 26, signal-EMA 9, antall kurser sett).
    state: HashMap<String, (f64, f64, f64, usize)>,
    /// Forrige (MACD − signal) per symbol — fortegnsskiftet er signalet.
    prev_hist: HashMap<String, f64>,
}

const MACD_FAST: usize = 12;
const MACD_SLOW: usize = 26;
const MACD_SIGNAL: usize = 9;

impl Macd {
    pub fn new(_cfg: &StrategyCfg) -> Self {
        Self { state: HashMap::new(), prev_hist: HashMap::new() }
    }

    fn ema(prev: f64, price: f64, n: usize) -> f64 {
        let k = 2.0 / (n as f64 + 1.0);
        prev + k * (price - prev)
    }

    /// Mat inn en kurs; gi (MACD − signal) når nok data er sett.
    fn push(&mut self, symbol: &str, price: f64) -> Option<f64> {
        let e = self
            .state
            .entry(symbol.to_string())
            .or_insert((price, price, 0.0, 0));
        e.0 = Self::ema(e.0, price, MACD_FAST);
        e.1 = Self::ema(e.1, price, MACD_SLOW);
        let macd = e.0 - e.1;
        if e.3 == 0 {
            e.2 = macd;
        } else {
            e.2 = Self::ema(e.2, macd, MACD_SIGNAL);
        }
        e.3 += 1;
        // Vent til begge EMA-ene har stabilisert seg.
        (e.3 >= MACD_SLOW + MACD_SIGNAL).then_some(macd - e.2)
    }
}

impl Strategy for Macd {
    fn name(&self) -> &'static str {
        "macd"
    }

    fn seed(&mut self, symbol: &str, closes: &[f64]) {
        for &c in closes {
            if let Some(h) = self.push(symbol, c) {
                self.prev_hist.insert(symbol.to_string(), h);
            }
        }
    }

    fn on_price(&mut self, symbol: &str, price: f64) -> Option<Side> {
        let hist = self.push(symbol, price)?;
        let prev = self.prev_hist.insert(symbol.to_string(), hist)?;
        if prev <= 0.0 && hist > 0.0 {
            Some(Side::Buy)
        } else if prev >= 0.0 && hist < 0.0 {
            Some(Side::Sell)
        } else {
            None
        }
    }
}

/// Bollinger-bånd (20 dager, ±2 standardavvik): kjøp når kursen faller
/// under nedre bånd (uvanlig billig mot eget snitt), selg over øvre bånd.
pub struct Bollinger {
    window: usize,
    closes: HashMap<String, VecDeque<f64>>,
}

impl Bollinger {
    pub fn new(_cfg: &StrategyCfg) -> Self {
        Self { window: 20, closes: HashMap::new() }
    }

    fn push(&mut self, symbol: &str, price: f64) {
        let w = self.closes.entry(symbol.to_string()).or_default();
        w.push_back(price);
        if w.len() > self.window {
            w.pop_front();
        }
    }

    /// (nedre, øvre) bånd for symbolet, når vinduet er fullt.
    fn bands(&self, symbol: &str) -> Option<(f64, f64)> {
        let w = self.closes.get(symbol)?;
        if w.len() < self.window {
            return None;
        }
        let n = w.len() as f64;
        let mean = w.iter().sum::<f64>() / n;
        let var = w.iter().map(|p| (p - mean).powi(2)).sum::<f64>() / n;
        let std = var.sqrt();
        Some((mean - 2.0 * std, mean + 2.0 * std))
    }
}

impl Strategy for Bollinger {
    fn name(&self) -> &'static str {
        "bollinger"
    }

    fn seed(&mut self, symbol: &str, closes: &[f64]) {
        for &c in closes {
            self.push(symbol, c);
        }
    }

    fn on_price(&mut self, symbol: &str, price: f64) -> Option<Side> {
        // Båndene regnes på vinduet FØR dagens kurs — dagens kurs er
        // det som eventuelt bryter dem.
        let signal = self.bands(symbol).and_then(|(lower, upper)| {
            if price < lower {
                Some(Side::Buy)
            } else if price > upper {
                Some(Side::Sell)
            } else {
                None
            }
        });
        self.push(symbol, price);
        signal
    }
}

/// Strategiene brukeren kan velge mellom i appen.
pub const AVAILABLE: [&str; 5] = ["sma_cross", "rsi", "momentum", "macd", "bollinger"];

pub fn build(cfg: &StrategyCfg) -> anyhow::Result<Box<dyn Strategy>> {
    match cfg.name.as_str() {
        "sma_cross" => Ok(Box::new(SmaCross::new(cfg))),
        "rsi" => Ok(Box::new(Rsi::new(cfg))),
        "momentum" => Ok(Box::new(Momentum::new(cfg))),
        "macd" => Ok(Box::new(Macd::new(cfg))),
        "bollinger" => Ok(Box::new(Bollinger::new(cfg))),
        other => anyhow::bail!("ukjent strategi: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(fast: usize, slow: usize) -> StrategyCfg {
        StrategyCfg { fast, slow, ..StrategyCfg::default() }
    }

    #[test]
    fn crossover_generates_buy_then_sell() {
        let mut s = SmaCross::new(&cfg(2, 4));
        // Fallende serie: fast under slow.
        s.seed("X", &[110.0, 108.0, 106.0, 104.0, 102.0]);
        // Kraftig oppgang → fast krysser over slow → kjøp.
        assert_eq!(s.on_price("X", 115.0), Some(Side::Buy));
        // Videre oppgang: ingen nye signaler.
        assert_eq!(s.on_price("X", 125.0), None);
        assert_eq!(s.on_price("X", 130.0), None);
        // Kraftig fall → fast krysser under → selg.
        assert_eq!(s.on_price("X", 90.0), Some(Side::Sell));
        assert_eq!(s.on_price("X", 70.0), None);
    }

    #[test]
    fn no_signal_before_enough_data() {
        let mut s = SmaCross::new(&cfg(2, 4));
        assert_eq!(s.on_price("Y", 100.0), None);
        assert_eq!(s.on_price("Y", 101.0), None);
        assert_eq!(s.on_price("Y", 102.0), None);
    }

    #[test]
    fn rsi_buys_oversold_and_sells_overbought() {
        let mut s = Rsi::new(&StrategyCfg { rsi_period: 3, ..StrategyCfg::default() });
        // Jevn oppgang: RSI = 100.
        s.seed("X", &[100.0, 101.0, 102.0, 103.0]);
        // Kraftig fall → RSI under 30 → kjøp.
        assert_eq!(s.on_price("X", 90.0), Some(Side::Buy));
        // Kraftig oppgang → RSI over 70 → selg.
        assert_eq!(s.on_price("X", 120.0), Some(Side::Sell));
    }

    #[test]
    fn macd_signals_on_trend_shift() {
        let mut s = Macd::new(&StrategyCfg::default());
        // Lang, jevn nedgang: MACD godt under signal.
        let seed: Vec<f64> = (0..60).map(|i| 200.0 - i as f64).collect();
        s.seed("X", &seed);
        // Kraftig snuoperasjon oppover → MACD krysser over signal → kjøp.
        let mut got_buy = false;
        for i in 0..15 {
            if s.on_price("X", 145.0 + i as f64 * 8.0) == Some(Side::Buy) {
                got_buy = true;
                break;
            }
        }
        assert!(got_buy, "forventet kjøpssignal når trenden snur opp");
        // Og tilsvarende ned igjen → selg.
        let mut got_sell = false;
        for i in 0..20 {
            if s.on_price("X", 250.0 - i as f64 * 12.0) == Some(Side::Sell) {
                got_sell = true;
                break;
            }
        }
        assert!(got_sell, "forventet salgssignal når trenden snur ned");
    }

    #[test]
    fn bollinger_buys_below_band_sells_above() {
        let mut s = Bollinger::new(&StrategyCfg::default());
        // 20 dager med små svingninger rundt 100.
        let seed: Vec<f64> = (0..20).map(|i| 100.0 + if i % 2 == 0 { 1.0 } else { -1.0 }).collect();
        s.seed("X", &seed);
        // Kraftig fall langt under nedre bånd → kjøp.
        assert_eq!(s.on_price("X", 90.0), Some(Side::Buy));
        // Kraftig hopp langt over øvre bånd → selg.
        assert_eq!(s.on_price("X", 115.0), Some(Side::Sell));
        // Normal kurs innenfor båndene → ingenting.
        assert_eq!(s.on_price("X", 100.0), None);
    }

    #[test]
    fn momentum_buys_breakout_and_sells_breakdown() {
        let mut s = Momentum::new(&StrategyCfg { momentum_window: 3, ..StrategyCfg::default() });
        s.seed("X", &[1.0, 2.0, 3.0]);
        assert_eq!(s.on_price("X", 4.0), Some(Side::Buy)); // brudd over høyeste
        assert_eq!(s.on_price("X", 1.0), Some(Side::Sell)); // brudd under laveste
        assert_eq!(s.on_price("X", 2.0), None); // innenfor vinduet
    }
}
