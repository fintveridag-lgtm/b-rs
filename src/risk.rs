use crate::config::RiskCfg;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Resultatet av en risikosjekk: enten klarert, eller blokkert med begrunnelse.
pub enum RiskVerdict {
    Ok,
    Blocked(String),
}

/// Beskyttende exit per posisjon: stop-loss / take-profit i prosent fra
/// kjøpskurs. 0 betyr avslått. Returnerer begrunnelse hvis posisjonen skal
/// selges. Disse salgene går UTENOM ordre-risikosjekken — de reduserer
/// risiko og skal aldri blokkeres av f.eks. tapsgrensen.
pub fn protective_exit(avg_price: f64, last: f64, stop_loss_pct: f64, take_profit_pct: f64) -> Option<String> {
    if avg_price <= 0.0 || last <= 0.0 {
        return None;
    }
    let pnl_pct = (last / avg_price - 1.0) * 100.0;
    if stop_loss_pct > 0.0 && pnl_pct <= -stop_loss_pct {
        return Some(format!("Stop-loss ({pnl_pct:+.1} %)"));
    }
    if take_profit_pct > 0.0 && pnl_pct >= take_profit_pct {
        return Some(format!("Take-profit ({pnl_pct:+.1} %)"));
    }
    None
}

/// Trailing stop: selg hvis kursen har falt `trail_pct` fra toppen (peak)
/// siden kjøpet. 0 = avslått. Beskytter opparbeidet gevinst — en aksje
/// kjøpt på 100 som steg til 140 selges rundt 128 ved 8 % trailing.
pub fn trailing_exit(peak: f64, last: f64, trail_pct: f64) -> Option<String> {
    if trail_pct <= 0.0 || peak <= 0.0 || last <= 0.0 {
        return None;
    }
    let drop_pct = (1.0 - last / peak) * 100.0;
    if drop_pct >= trail_pct {
        return Some(format!("Trailing stop (−{drop_pct:.1} % fra topp {peak:.2})"));
    }
    None
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
            stop_loss_pct: 8.0,
            take_profit_pct: 0.0,
            trailing_stop_pct: 0.0,
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
    fn protective_exit_triggers_correctly() {
        // Stop-loss ved −8 %: 100 → 91 utløser, 100 → 93 gjør ikke.
        assert!(protective_exit(100.0, 91.0, 8.0, 0.0).unwrap().contains("Stop-loss"));
        assert!(protective_exit(100.0, 93.0, 8.0, 0.0).is_none());
        // Take-profit ved +10 %.
        assert!(protective_exit(100.0, 111.0, 8.0, 10.0).unwrap().contains("Take-profit"));
        assert!(protective_exit(100.0, 105.0, 8.0, 10.0).is_none());
        // Avslått (0) utløser aldri; ukjent kostpris ignoreres trygt.
        assert!(protective_exit(100.0, 50.0, 0.0, 0.0).is_none());
        assert!(protective_exit(0.0, 50.0, 8.0, 10.0).is_none());
    }

    #[test]
    fn trailing_stop_triggers_from_peak() {
        // Topp 140, 8 % trailing → utløses ved 128.8 eller lavere.
        assert!(trailing_exit(140.0, 128.0, 8.0).unwrap().contains("Trailing"));
        assert!(trailing_exit(140.0, 130.0, 8.0).is_none());
        // Avslått eller ugyldige tall utløser aldri.
        assert!(trailing_exit(140.0, 100.0, 0.0).is_none());
        assert!(trailing_exit(0.0, 100.0, 8.0).is_none());
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
