use crate::broker::Broker;
use crate::config::Config;
use crate::marketdata::Yahoo;
use crate::notify::Notifier;
use crate::risk::{protective_exit, RiskManager, RiskVerdict};
use crate::state::{Flags, SharedState};
use crate::store::Store;
use crate::strategy::Strategy;
use crate::types::{OrderRequest, OrderStatus, Side};
use chrono::Utc;
use std::collections::HashSet;
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
    notifier: Option<Arc<Notifier>>,
    was_killed: bool,
    /// Symboler som allerede er sådd med historikk.
    seeded: HashSet<String>,
    /// Symboler med utestående beskyttende salg (stop-loss/take-profit),
    /// så vi ikke sender samme salg flere ganger mens ordren fylles.
    protected: HashSet<String>,
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
        notifier: Option<Arc<Notifier>>,
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
            notifier,
            was_killed: false,
            seeded: HashSet::new(),
            protected: HashSet::new(),
        }
    }

    /// Send push-varsel til mobil i bakgrunnen (blokkerer aldri tikken).
    fn notify(&self, message: String) {
        let Some(notifier) = self.notifier.clone() else { return };
        let state = self.state.clone();
        tokio::spawn(async move {
            if let Err(e) = notifier.send(&message).await {
                state.lock().unwrap().log(format!("Varsel feilet: {e:#}"));
            }
        });
    }

    fn log(&self, msg: impl Into<String>) {
        let msg = msg.into();
        let _ = self.store.record_event(&msg);
        self.state.lock().unwrap().log(msg);
    }

    /// Så strategien med daglig historikk for alle usådde symboler i
    /// watchlisten — både ved oppstart og når GUI-et legger til nye.
    pub async fn ensure_seeded(&mut self) {
        let symbols: Vec<String> = self.state.lock().unwrap().watchlist.clone();
        for symbol in symbols {
            if self.seeded.contains(&symbol) {
                continue;
            }
            self.seeded.insert(symbol.clone());
            // 2 år: nok til grafens "Alt"-visning og ærlig backtesting.
            match self.market.history_daily(&symbol, "2y").await {
                Ok(bars) if !bars.is_empty() => {
                    let closes: Vec<f64> = bars.iter().map(|b| b.close).collect();
                    self.strategy.seed(&symbol, &closes);
                    {
                        let mut st = self.state.lock().unwrap();
                        for b in &bars {
                            st.push_price(&symbol, b.ts, b.close);
                        }
                        st.candles.insert(symbol.clone(), bars.clone());
                    }
                    self.log(format!("{symbol}: sådd med {} dagers historikk", bars.len()));
                    // Utbytte siste 12 mnd — vises i porteføljeanalysen.
                    if let Ok(div) = self.market.dividends_12m(&symbol).await {
                        if div > 0.0 {
                            self.state.lock().unwrap().dividends.insert(symbol.clone(), div);
                        }
                    }
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
        // Oppstartsvarsel — bekrefter samtidig at varselkanalen fungerer.
        self.notify(format!(
            "Startet i {}-modus (megler: {}, strategi: {}).",
            self.cfg.mode,
            self.broker.name(),
            self.strategy.name()
        ));
        self.ensure_seeded().await;

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
            self.notify("⛔ Kill switch aktivert — all handel stoppet.".to_string());
            if let Err(e) = self.broker.cancel_all().await {
                self.log(format!("Kansellering feilet: {e:#}"));
            }
        } else if !killed && self.was_killed {
            self.log("Kill switch deaktivert — handel gjenopptatt.");
            self.notify("Kill switch deaktivert — handel gjenopptatt.".to_string());
        }
        self.was_killed = killed;
    }

    /// Bytt strategi på forespørsel fra GUI-et: bygg den nye og så den med
    /// historikken vi allerede har, så den er varm fra første tikk.
    fn handle_strategy_switch(&mut self) {
        let request = self.state.lock().unwrap().strategy_request.take();
        let Some(name) = request else { return };
        let mut cfg = self.cfg.strategy.clone();
        cfg.name = name.clone();
        match crate::strategy::build(&cfg) {
            Ok(mut fresh) => {
                let history = self.state.lock().unwrap().history.clone();
                for (symbol, points) in history {
                    let closes: Vec<f64> = points.iter().map(|&(_, p)| p).collect();
                    fresh.seed(&symbol, &closes);
                }
                self.strategy = fresh;
                self.state.lock().unwrap().strategy_name = name.clone();
                self.log(format!("Strategi byttet til {name}."));
            }
            Err(e) => self.log(format!("Kunne ikke bytte strategi: {e:#}")),
        }
    }

    async fn tick(&mut self) {
        self.handle_strategy_switch();
        self.ensure_seeded().await;

        // 1) Hent kurser.
        let symbols: Vec<String> = self.state.lock().unwrap().watchlist.clone();
        let mut fresh = Vec::new();
        for symbol in &symbols {
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

        // 3) Beskyttende exits: stop-loss / take-profit per posisjon.
        //    Kjører også under strategipause (beskytter beholdningen),
        //    men aldri når kill switch er på.
        self.protected
            .retain(|s| positions.iter().any(|p| p.symbol == *s && p.qty > 1e-9));
        if !self.flags.killed() {
            let sl = self.cfg.risk.stop_loss_pct;
            let tp = self.cfg.risk.take_profit_pct;
            let mut exits = Vec::new();
            for p in &positions {
                if p.qty <= 0.0 || self.protected.contains(&p.symbol) {
                    continue;
                }
                let Some(q) = fresh.iter().find(|q| q.symbol == p.symbol) else { continue };
                if let Some(reason) = protective_exit(p.avg_price, q.last, sl, tp) {
                    exits.push((p.symbol.clone(), p.qty, q.last, reason));
                }
            }
            for (symbol, qty, price, reason) in exits {
                self.protected.insert(symbol.clone());
                self.log(format!("{reason} utløst for {symbol} — selger hele posisjonen."));
                self.notify(format!("🛡️ {reason}: selger {symbol}"));
                let result = self.place(&symbol, Side::Sell, qty, price, &reason).await;
                if matches!(result, None | Some(OrderStatus::Rejected)) {
                    // Ordren nådde ikke frem — prøv igjen neste tikk.
                    self.protected.remove(&symbol);
                }
            }
        }

        //    Manuelle ordrer går gjennom risikoreglene og stoppes av
        //    kill switch, men ikke av strategipausen.
        let manual: Vec<(String, Side, f64)> = {
            let mut st = self.state.lock().unwrap();
            st.manual_orders.drain(..).collect()
        };
        for (symbol, side, qty) in manual {
            if self.flags.killed() {
                self.log(format!("Manuell ordre {side} {symbol} forkastet — kill switch er på."));
                continue;
            }
            let price = fresh
                .iter()
                .find(|q| q.symbol == symbol)
                .map(|q| q.last)
                .or_else(|| self.state.lock().unwrap().quotes.get(&symbol).map(|q| q.last));
            let Some(price) = price else {
                self.log(format!("Manuell ordre: ingen kurs for {symbol} ennå."));
                continue;
            };
            let held = positions.iter().find(|p| p.symbol == symbol).map_or(0.0, |p| p.qty);
            let pos_value_after = match side {
                Side::Buy => (held + qty) * price,
                Side::Sell => (held - qty).max(0.0) * price,
            };
            match self.risk.check(qty * price, pos_value_after, equity) {
                RiskVerdict::Blocked(reason) => {
                    self.log(format!("Manuell {side} {symbol} blokkert av risikoregel: {reason}"));
                }
                RiskVerdict::Ok => {
                    let _ = self.place(&symbol, side, qty, price, "manuell ordre").await;
                }
            }
        }

        // 5) Strategisignaler → risikosjekk → ordre.
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
                        // Tapsgrensen er alvorlig nok til å vekke mobilen.
                        if reason.contains("tapsgrense") {
                            self.notify(format!("🛑 {reason}"));
                        }
                    }
                    RiskVerdict::Ok => {
                        let note = format!("signal fra {}", self.strategy.name());
                        let _ = self.place(&q.symbol, side, qty, q.last, &note).await;
                    }
                }
            }
        }

        // 6) Oppdater UI-tilstand.
        let positions = self.broker.positions().await.unwrap_or(positions);
        let cash = self.broker.cash().await.unwrap_or(cash);
        let equity = cash + positions.iter().map(|p| p.market_value()).sum::<f64>();
        let drawdown = self.risk.drawdown(equity);
        {
            let now = Utc::now();
            let mut st = self.state.lock().unwrap();
            for q in fresh {
                st.push_price(&q.symbol, q.ts.timestamp() as f64, q.last);
                st.quotes.insert(q.symbol.clone(), q);
            }
            st.positions = positions;
            st.cash = cash;
            st.equity = equity;
            st.drawdown = drawdown;
            st.push_equity(now.timestamp() as f64, equity);
            st.last_tick = Some(now);
        }
    }

    async fn place(
        &mut self,
        symbol: &str,
        side: Side,
        qty: f64,
        price: f64,
        note: &str,
    ) -> Option<OrderStatus> {
        let req = OrderRequest {
            symbol: symbol.to_string(),
            side,
            qty,
            ref_price: price,
            note: note.to_string(),
        };
        match self.broker.place_order(req).await {
            Ok(order) => {
                let status = order.status;
                let level = if status == OrderStatus::Rejected { "AVVIST" } else { "ORDRE" };
                self.log(format!(
                    "{level}: {} {} x{:.0} @ {:.2} [{}]",
                    order.side, order.symbol, order.qty, order.avg_price, order.status
                ));
                if status != OrderStatus::Rejected {
                    self.notify(format!(
                        "{} {} x{:.0} @ {:.2} [{}] — {}",
                        order.side, order.symbol, order.qty, order.avg_price, order.status, order.note
                    ));
                }
                let _ = self.store.record_order(&order, self.broker.name());
                let tx = crate::state::TxRow {
                    ts: order.created.format("%d.%m.%Y %H:%M").to_string(),
                    symbol: order.symbol.clone(),
                    side: order.side.to_string(),
                    qty: order.qty,
                    price: order.avg_price,
                    status: order.status.to_string(),
                    broker: self.broker.name().to_string(),
                    note: order.note.clone(),
                };
                let mut st = self.state.lock().unwrap();
                st.push_transaction(tx);
                st.push_order(order);
                Some(status)
            }
            Err(e) => {
                self.log(format!("Ordre feilet for {symbol}: {e:#}"));
                None
            }
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
