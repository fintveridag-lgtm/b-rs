use crate::broker::Broker;
use crate::config::Config;
use crate::marketdata::Yahoo;
use crate::risk::{RiskManager, RiskVerdict};
use crate::state::{Flags, SharedState};
use crate::store::Store;
use crate::strategy::Strategy;
use crate::types::{OrderRequest, OrderStatus, Side};
use chrono::Utc;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// Hovedløkken: hent kurser → oppdater strategi → risikosjekk → legg ordrer
/// → oppdater posisjoner og UI-tilstand. Kjører til quit-flagget settes.
pub struct Engine {
    cfg: Config,
    broker: Arc<dyn Broker>,
    market: Yahoo,
    strategy: Box<dyn Strategy>,
    risk: RiskManager,
    store: Arc<Store>,
    state: SharedState,
    flags: Arc<Flags>,
    was_killed: bool,
}

impl Engine {
    pub fn new(
        cfg: Config,
        broker: Arc<dyn Broker>,
        market: Yahoo,
        strategy: Box<dyn Strategy>,
        store: Arc<Store>,
        state: SharedState,
        flags: Arc<Flags>,
    ) -> Self {
        let risk = RiskManager::new(cfg.risk.clone());
        Self {
            cfg,
            broker,
            market,
            strategy,
            risk,
            store,
            state,
            flags,
            was_killed: false,
        }
    }

    fn log(&self, msg: impl Into<String>) {
        let msg = msg.into();
        let _ = self.store.record_event(&msg);
        self.state.lock().unwrap().log(msg);
    }

    /// Så strategien med daglig historikk så den har SMA-vinduer fra start.
    pub async fn seed_history(&mut self) {
        for symbol in self.cfg.watchlist.clone() {
            match self.market.history_daily(&symbol, "3mo").await {
                Ok(points) if !points.is_empty() => {
                    let closes: Vec<f64> = points.iter().map(|&(_, c)| c).collect();
                    self.strategy.seed(&symbol, &closes);
                    {
                        let mut st = self.state.lock().unwrap();
                        for &(t, c) in &points {
                            st.push_price(&symbol, t as f64, c);
                        }
                    }
                    self.log(format!("{symbol}: sådd med {} dagers historikk", points.len()));
                }
                Ok(_) => self.log(format!("{symbol}: tom historikk fra Yahoo")),
                Err(e) => self.log(format!("{symbol}: klarte ikke hente historikk: {e:#}")),
            }
        }
    }

    pub async fn run(mut self) {
        self.log(format!(
            "Engine startet — modus={}, megler={}, strategi={}",
            self.cfg.mode,
            self.broker.name(),
            self.strategy.name()
        ));
        self.seed_history().await;

        let mut interval = tokio::time::interval(Duration::from_secs(self.cfg.poll_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        while !self.flags.quit() {
            interval.tick().await;
            if self.flags.quit() {
                break;
            }
            self.handle_kill_switch().await;
            self.tick().await;
        }
        self.log("Engine stoppet.");
    }

    /// Kanseller åpne ordrer én gang idet kill switch slås på.
    async fn handle_kill_switch(&mut self) {
        let killed = self.flags.killed();
        if killed && !self.was_killed {
            self.log("KILL SWITCH aktivert — kansellerer åpne ordrer, handel stoppet.");
            if let Err(e) = self.broker.cancel_all().await {
                self.log(format!("Kansellering feilet: {e:#}"));
            }
        } else if !killed && self.was_killed {
            self.log("Kill switch deaktivert — handel gjenopptatt.");
        }
        self.was_killed = killed;
    }

    async fn tick(&mut self) {
        // 1) Hent kurser.
        let mut fresh = Vec::new();
        for symbol in &self.cfg.watchlist {
            match self.market.quote(symbol).await {
                Ok(q) => {
                    self.broker.on_quote(symbol, q.last).await;
                    fresh.push(q);
                }
                Err(e) => self.log(format!("{symbol}: kursfeil: {e:#}")),
            }
        }

        // 2) Posisjoner og egenkapital fra megleren.
        let positions = match self.broker.positions().await {
            Ok(p) => p,
            Err(e) => {
                self.log(format!("Klarte ikke hente posisjoner: {e:#}"));
                Vec::new()
            }
        };
        let cash = self.broker.cash().await.unwrap_or(0.0);
        let equity = cash + positions.iter().map(|p| p.market_value()).sum::<f64>();
        self.risk.observe_equity(equity);

        // 3) Strategisignaler → risikosjekk → ordre.
        if !self.flags.killed() && !self.flags.paused() {
            for q in &fresh {
                let Some(side) = self.strategy.on_price(&q.symbol, q.last) else {
                    continue;
                };
                let held = positions
                    .iter()
                    .find(|p| p.symbol == q.symbol)
                    .map_or(0.0, |p| p.qty);

                // Kjøp bare når vi er flate, selg hele posisjonen — enkel
                // og forutsigbar posisjonsstyring.
                let qty = match side {
                    Side::Buy if held <= 0.0 => self.cfg.strategy.order_qty,
                    Side::Sell if held > 0.0 => held,
                    _ => continue,
                };

                let order_value = qty * q.last;
                let pos_value_after = match side {
                    Side::Buy => (held + qty) * q.last,
                    Side::Sell => 0.0,
                };

                match self.risk.check(order_value, pos_value_after, equity) {
                    RiskVerdict::Blocked(reason) => {
                        self.log(format!("{} {} blokkert av risikoregel: {reason}", side, q.symbol));
                    }
                    RiskVerdict::Ok => {
                        self.place(&q.symbol, side, qty, q.last).await;
                    }
                }
            }
        }

        // 4) Oppdater UI-tilstand.
        let positions = self.broker.positions().await.unwrap_or(positions);
        let cash = self.broker.cash().await.unwrap_or(cash);
        let equity = cash + positions.iter().map(|p| p.market_value()).sum::<f64>();
        let drawdown = self.risk.drawdown(equity);
        {
            let mut st = self.state.lock().unwrap();
            for q in fresh {
                st.push_price(&q.symbol, q.ts.timestamp() as f64, q.last);
                st.quotes.insert(q.symbol.clone(), q);
            }
            st.positions = positions;
            st.cash = cash;
            st.equity = equity;
            st.drawdown = drawdown;
            st.last_tick = Some(Utc::now());
        }
    }

    async fn place(&mut self, symbol: &str, side: Side, qty: f64, price: f64) {
        let note = format!("signal fra {}", self.strategy.name());
        let req = OrderRequest {
            symbol: symbol.to_string(),
            side,
            qty,
            ref_price: price,
            note,
        };
        match self.broker.place_order(req).await {
            Ok(order) => {
                let level = if order.status == OrderStatus::Rejected { "AVVIST" } else { "ORDRE" };
                self.log(format!(
                    "{level}: {} {} x{:.0} @ {:.2} [{}]",
                    order.side, order.symbol, order.qty, order.avg_price, order.status
                ));
                let _ = self.store.record_order(&order, self.broker.name());
                self.state.lock().unwrap().push_order(order);
            }
            Err(e) => self.log(format!("Ordre feilet for {symbol}: {e:#}")),
        }
    }
}

/// Egen bakgrunnsoppgave for Nordnet-lesemodus.
pub async fn nordnet_task(cfg: Config, state: SharedState, flags: Arc<Flags>) {
    let mut reader = match crate::nordnet::NordnetReader::new(&cfg.nordnet) {
        Ok(r) => r,
        Err(e) => {
            state.lock().unwrap().log(format!("Nordnet-lesemodus feilet ved oppstart: {e:#}"));
            return;
        }
    };
    let mut interval = tokio::time::interval(Duration::from_secs(cfg.nordnet.poll_secs));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_error: Option<String> = None;

    while !flags.quit.load(Ordering::Relaxed) {
        interval.tick().await;
        match reader.positions().await {
            Ok(positions) => {
                let mut st = state.lock().unwrap();
                if last_error.take().is_some() {
                    st.log("Nordnet-lesemodus: tilkobling gjenopprettet.");
                }
                st.log(format!("Nordnet: hentet {} posisjoner", positions.len()));
                st.nordnet_positions = positions;
            }
            Err(e) => {
                let msg = format!("{e:#}");
                // Ikke spam loggen med samme feil hver runde.
                if last_error.as_deref() != Some(&msg) {
                    state.lock().unwrap().log(format!("Nordnet-lesemodus: {msg}"));
                    last_error = Some(msg);
                }
            }
        }
    }
}
