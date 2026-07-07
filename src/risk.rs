use crate::config::RiskCfg;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Resultatet av en risikosjekk: enten klarert, eller blokkert med begrunnelse.
pub enum RiskVerdict {
    Ok,
    Blocked(String),
}

/// Alle ordrer må gjennom denne før de når megleren. Reglene er bevisst
/// enkle og harde — de skal stoppe løpske strategier, ikke optimalisere.
pub struct RiskManager {
    cfg: RiskCfg,
    order_times: VecDeque<Instant>,
    start_equity: Option<f64>,
}

impl RiskManager {
    pub fn new(cfg: RiskCfg) -> Self {
        Self {
            cfg,
            order_times: VecDeque::new(),
            start_equity: None,
        }
    }

    /// Kalles hver tikk med nåværende egenkapital (kontanter + posisjoner).
    /// Første kall setter referansen for tapsgrensen.
    pub fn observe_equity(&mut self, equity: f64) {
        self.start_equity.get_or_insert(equity);
    }

    pub fn drawdown(&self, equity: f64) -> f64 {
        self.start_equity.map_or(0.0, |s| equity - s)
    }

    pub fn check(
        &mut self,
        order_value: f64,
        position_value_after: f64,
        current_equity: f64,
    ) -> RiskVerdict {
        if let Some(start) = self.start_equity {
            let loss = start - current_equity;
            if loss >= self.cfg.max_daily_loss {
                return RiskVerdict::Blocked(format!(
                    "tapsgrense nådd ({loss:.0} >= {:.0}) — handel stoppet",
                    self.cfg.max_daily_loss
                ));
            }
        }

        if order_value > self.cfg.max_order_value {
            return RiskVerdict::Blocked(format!(
                "ordreverdi {order_value:.0} over maks {:.0}",
                self.cfg.max_order_value
            ));
        }

        if position_value_after > self.cfg.max_position_value {
            return RiskVerdict::Blocked(format!(
                "posisjonsverdi {position_value_after:.0} ville oversteget maks {:.0}",
                self.cfg.max_position_value
            ));
        }

        let now = Instant::now();
        while let Some(&t) = self.order_times.front() {
            if now.duration_since(t) > Duration::from_secs(60) {
                self.order_times.pop_front();
            } else {
                break;
            }
        }
        if self.order_times.len() as u32 >= self.cfg.max_orders_per_min {
            return RiskVerdict::Blocked(format!(
                "ratebegrensning: {} ordrer siste minutt",
                self.order_times.len()
            ));
        }

        self.order_times.push_back(now);
        RiskVerdict::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> RiskCfg {
        RiskCfg {
            max_order_value: 1000.0,
            max_position_value: 2000.0,
            max_orders_per_min: 2,
            max_daily_loss: 500.0,
        }
    }

    #[test]
    fn blocks_oversized_order() {
        let mut r = RiskManager::new(cfg());
        r.observe_equity(10_000.0);
        assert!(matches!(r.check(1500.0, 1500.0, 10_000.0), RiskVerdict::Blocked(_)));
        assert!(matches!(r.check(500.0, 500.0, 10_000.0), RiskVerdict::Ok));
    }

    #[test]
    fn blocks_after_daily_loss() {
        let mut r = RiskManager::new(cfg());
        r.observe_equity(10_000.0);
        assert!(matches!(r.check(100.0, 100.0, 9_400.0), RiskVerdict::Blocked(_)));
    }

    #[test]
    fn rate_limits_orders() {
        let mut r = RiskManager::new(cfg());
        r.observe_equity(10_000.0);
        assert!(matches!(r.check(100.0, 100.0, 10_000.0), RiskVerdict::Ok));
        assert!(matches!(r.check(100.0, 100.0, 10_000.0), RiskVerdict::Ok));
        assert!(matches!(r.check(100.0, 100.0, 10_000.0), RiskVerdict::Blocked(_)));
    }
}
