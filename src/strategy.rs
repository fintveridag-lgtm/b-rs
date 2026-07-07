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

pub fn build(cfg: &StrategyCfg) -> anyhow::Result<Box<dyn Strategy>> {
    match cfg.name.as_str() {
        "sma_cross" => Ok(Box::new(SmaCross::new(cfg))),
        other => anyhow::bail!("ukjent strategi: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(fast: usize, slow: usize) -> StrategyCfg {
        StrategyCfg { name: "sma_cross".into(), fast, slow, order_qty: 1.0 }
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
}
