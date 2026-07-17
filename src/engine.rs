use crate::broker::Broker;
use crate::config::Config;
use crate::marketdata::Yahoo;
use crate::notify::Notifier;
use crate::risk::{order_size, protective_exit, trailing_exit, RiskManager, RiskVerdict};
use crate::types::Quote;
use crate::state::{Flags, SharedState};
use crate::store::Store;
use crate::strategy::Strategy;
use crate::types::{OrderRequest, OrderStatus, Side};
use anyhow::Result;
use chrono::Utc;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// Hovedløkken: hent kurser → oppdater strategi → risikosjekk → legg ordrer
/// → oppdater posisjoner og UI-tilstand. Kjører til quit-flagget settes.
pub struct Engine {
    cfg: Config,
    broker: Arc<dyn Broker>,
    market: Arc<Yahoo>,
    /// Én strategiinstans per strateginavn — hver instans håndterer alle
    /// symbolene som er tilordnet den (standard eller per-aksje-valg).
    strategies: HashMap<String, Box<dyn Strategy>>,
    /// (strateginavn, symbol)-par som er sådd med historikk.
    strategy_seeded: HashSet<(String, String)>,
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
    /// Høyeste kurs per posisjon siden kjøp — grunnlag for trailing stop.
    peaks: HashMap<String, f64>,
    /// Valutakurser til kontovaluta: valuta → (kurs, hentet-tidspunkt).
    fx: HashMap<String, (f64, std::time::Instant)>,
    /// Valutaer med feilet oppslag — logget én gang til det lykkes.
    fx_failed: HashSet<String>,
    /// Antall tikk på rad uten en eneste kurs — vakthund for datastrømmen.
    fail_streak: u32,
    /// Sist daglig egenkapital-øyeblikksbilde ble skrevet (maks hvert 5. min).
    last_equity_write: Option<std::time::Instant>,
    /// (dato, symbol)-par som alt har fått dagsfall-varsel — én gang per dag.
    day_move_alerted: HashSet<(String, String)>,
    /// Kontantsaldo-feil er logget (nullstilles når kallet lykkes igjen).
    cash_error_logged: bool,
    /// Tidsramme-lys per symbol: (lys-id, siste kurs i lyset). Strategien
    /// ser bare sluttkursen når et lys lukkes (timeframe_min > 0).
    tf_buckets: HashMap<String, (i64, f64)>,
}

/// Rull tidsramme-lyset for et symbol: oppdater innholdet og gi eventuelt
/// sluttkursen for lyset som nettopp ble lukket.
fn roll_bucket(prev: Option<(i64, f64)>, bucket: i64, price: f64) -> ((i64, f64), Option<f64>) {
    match prev {
        // Første observasjon — start lyset, ingenting å lukke.
        None => ((bucket, price), None),
        // Samme lys — oppdater sluttkursen.
        Some((b, _)) if b == bucket => ((bucket, price), None),
        // Nytt lys — lukk det forrige og lever sluttkursen til strategien.
        Some((_, close)) => ((bucket, price), Some(close)),
    }
}

impl Engine {
    // Mange samarbeidspartnere er selve poenget med motoren — en
    // config-struct ville bare flyttet listen.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cfg: Config,
        broker: Arc<dyn Broker>,
        market: Arc<Yahoo>,
        store: Arc<Store>,
        state: SharedState,
        flags: Arc<Flags>,
        notifier: Option<Arc<Notifier>>,
    ) -> Result<Self> {
        // Bygg standardstrategien med én gang så et ukjent navn i konfigen
        // stopper appen ved oppstart, ikke midt i en handelsdag.
        let default = crate::strategy::build(&cfg.strategy)?;
        let mut strategies = HashMap::new();
        strategies.insert(cfg.strategy.name.clone(), default);
        let risk = RiskManager::new(cfg.risk.clone());
        Ok(Self {
            cfg,
            broker,
            market,
            strategies,
            strategy_seeded: HashSet::new(),
            risk,
            store,
            state,
            flags,
            notifier,
            was_killed: false,
            seeded: HashSet::new(),
            protected: HashSet::new(),
            peaks: HashMap::new(),
            fx: HashMap::new(),
            fx_failed: HashSet::new(),
            fail_streak: 0,
            last_equity_write: None,
            day_move_alerted: HashSet::new(),
            cash_error_logged: false,
            tf_buckets: HashMap::new(),
        })
    }

    /// Kursen i kontovaluta, eller None hvis valutakursen mangler.
    fn base_price(&self, q: &Quote) -> Option<f64> {
        if q.currency.is_empty() || q.currency == self.cfg.base_currency {
            return Some(q.last);
        }
        self.fx.get(&q.currency).map(|&(rate, _)| q.last * rate)
    }

    /// Hent/forny valutakurser for alle valutaer i dagens kurser.
    /// Yahoo har valutapar som egne symboler, f.eks. "USDNOK=X".
    async fn update_fx(&mut self, fresh: &[Quote]) {
        let base = self.cfg.base_currency.clone();
        let needed: HashSet<String> = fresh
            .iter()
            .map(|q| q.currency.clone())
            .filter(|c| !c.is_empty() && *c != base)
            .collect();
        for currency in needed {
            let is_fresh = self
                .fx
                .get(&currency)
                .is_some_and(|(_, t)| t.elapsed() < Duration::from_secs(900));
            if is_fresh {
                continue;
            }
            match self.market.quote(&format!("{currency}{base}=X")).await {
                Ok(q) if q.last > 0.0 => {
                    let is_new = !self.fx.contains_key(&currency);
                    self.fx.insert(currency.clone(), (q.last, std::time::Instant::now()));
                    self.fx_failed.remove(&currency);
                    self.state.lock().unwrap().fx_rates.insert(currency.clone(), q.last);
                    if is_new {
                        self.log(format!("Valutakurs {currency}/{base}: {:.3}", q.last));
                    }
                }
                _ => {
                    if self.fx_failed.insert(currency.clone()) {
                        self.log(format!(
                            "Fikk ikke valutakurs {currency}/{base} — hopper over handel i {currency}-instrumenter inntil videre."
                        ));
                    }
                }
            }
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
                    {
                        let mut st = self.state.lock().unwrap();
                        for b in &bars {
                            st.push_price(&symbol, b.ts, b.close);
                        }
                        st.candles.insert(symbol.clone(), bars.clone());
                    }
                    self.log(format!("{symbol}: sådd med {} dagers historikk", bars.len()));
                    // Tidsramme-strategi trenger intradag-lys til seeding
                    // og backtest — hent dem samtidig.
                    if self.cfg.strategy.timeframe_min > 0 {
                        match self.market.history_intraday(&symbol).await {
                            Ok(intra) if !intra.is_empty() => {
                                self.log(format!(
                                    "{symbol}: {} intradag-lys (5 min) til tidsramme-strategien",
                                    intra.len()
                                ));
                                self.state.lock().unwrap().candles_intraday.insert(symbol.clone(), intra);
                            }
                            _ => self.log(format!(
                                "{symbol}: fikk ikke intradag-historikk — strategien varmes opp av live-lys i stedet"
                            )),
                        }
                    }
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
            self.cfg.strategy.name
        ));
        // Oppstartsvarsel — bekrefter samtidig at varselkanalen fungerer.
        self.notify(format!(
            "Startet i {}-modus (megler: {}, strategi: {}).",
            self.cfg.mode,
            self.broker.name(),
            self.cfg.strategy.name
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

    /// Bytt standardstrategi på forespørsel fra GUI-et. Instanser bygges
    /// og sås lat per symbol i signal-løkken.
    fn handle_strategy_switch(&mut self) {
        let request = self.state.lock().unwrap().strategy_request.take();
        let Some(name) = request else { return };
        if self.ensure_strategy(&name) {
            self.state.lock().unwrap().strategy_name = name.clone();
            self.log(format!("Standardstrategi byttet til {name}."));
        }
    }

    /// Sørg for at en strategiinstans finnes; false hvis navnet er ukjent.
    fn ensure_strategy(&mut self, name: &str) -> bool {
        if self.strategies.contains_key(name) {
            return true;
        }
        let mut cfg = self.cfg.strategy.clone();
        cfg.name = name.to_string();
        match crate::strategy::build(&cfg) {
            Ok(s) => {
                self.strategies.insert(name.to_string(), s);
                true
            }
            Err(e) => {
                self.log(format!("Ukjent strategi {name}: {e:#}"));
                false
            }
        }
    }

    /// Hent signal for et symbol fra riktig strategi (per-aksje-valg eller
    /// standard), med lat såing av historikk første gang paret brukes.
    /// Returnerer signal + strateginavnet det kom fra.
    fn signal_for(&mut self, symbol: &str, price: f64) -> Option<(Side, String)> {
        let name = {
            let st = self.state.lock().unwrap();
            st.symbol_strategy
                .get(symbol)
                .cloned()
                .unwrap_or_else(|| st.strategy_name.clone())
        };
        if !self.ensure_strategy(&name) {
            return None;
        }
        self.seed_if_needed(&name, symbol);
        let side = self.strategies.get_mut(&name)?.on_price(symbol, price)?;
        Some((side, name))
    }

    /// Så strategien for symbolet hvis det ikke er gjort — med samme
    /// oppløsning som den handler på: intradag-lys når tidsramme er valgt,
    /// ellers dagshistorikken.
    fn seed_if_needed(&mut self, name: &str, symbol: &str) {
        let key = (name.to_string(), symbol.to_string());
        if self.strategy_seeded.contains(&key) {
            return;
        }
        let st = self.state.lock().unwrap();
        let closes: Vec<f64> = if self.cfg.strategy.timeframe_min > 0 {
            st.candles_intraday
                .get(symbol)
                .map(|c| c.iter().map(|b| b.close).collect())
                .unwrap_or_default()
        } else {
            st.history
                .get(symbol)
                .map(|h| h.iter().map(|&(_, p)| p).collect())
                .unwrap_or_default()
        };
        drop(st);
        if closes.is_empty() {
            return; // historikken er ikke lastet ennå — prøv igjen neste tikk
        }
        if let Some(s) = self.strategies.get_mut(name) {
            s.seed(symbol, &closes);
        }
        self.strategy_seeded.insert(key);
    }

    /// «Hva ser boten?» — strategiens eget ståsted for symbolet, til UI-et.
    fn strategy_view(&mut self, symbol: &str) -> Option<String> {
        let name = {
            let st = self.state.lock().unwrap();
            st.symbol_strategy
                .get(symbol)
                .cloned()
                .unwrap_or_else(|| st.strategy_name.clone())
        };
        if !self.ensure_strategy(&name) {
            return None;
        }
        self.seed_if_needed(&name, symbol);
        let status = self.strategies.get(&name)?.status(symbol)?;
        let tf = self.cfg.strategy.timeframe_min;
        let ramme = if tf > 0 { format!("{tf} min-lys") } else { "hvert tikk".to_string() };
        Some(format!("[{name} · {ramme}] {status}"))
    }

    /// Signal per tikk — men med tidsramme (timeframe_min > 0) samles
    /// tikkene i jevne lys, og strategien ser bare sluttkursen per lys.
    /// Da betyr vinduene (fast/slow) det samme live som i backtest.
    fn signal_on_tick(&mut self, symbol: &str, price: f64) -> Option<(Side, String)> {
        let n = self.cfg.strategy.timeframe_min;
        if n == 0 {
            return self.signal_for(symbol, price);
        }
        let bucket = Utc::now().timestamp() / (n as i64 * 60);
        let prev = self.tf_buckets.get(symbol).copied();
        let (entry, closed) = roll_bucket(prev, bucket, price);
        self.tf_buckets.insert(symbol.to_string(), entry);
        let close = closed?;
        self.signal_for(symbol, close)
    }

    async fn tick(&mut self) {
        self.handle_strategy_switch();
        self.ensure_seeded().await;

        // 1) Hent kurser.
        let symbols: Vec<String> = self.state.lock().unwrap().watchlist.clone();
        let mut fresh = Vec::new();
        for symbol in &symbols {
            match self.market.quote(symbol).await {
                Ok(q) => fresh.push(q),
                Err(e) => self.log(format!("{symbol}: kursfeil: {e:#}")),
            }
        }

        // Vakthund: rop hvis datastrømmen dør helt.
        if fresh.is_empty() && !symbols.is_empty() {
            self.fail_streak += 1;
            if self.fail_streak == 5 {
                let msg = "⚠ Mistet kursdata — fem runder på rad uten svar fra Yahoo.".to_string();
                self.log(msg.clone());
                self.notify(msg);
            }
        } else {
            self.fail_streak = 0;
        }

        // Valutakurser, deretter marker posisjoner i kontovaluta.
        self.update_fx(&fresh).await;
        for q in &fresh {
            if let Some(px) = self.base_price(q) {
                self.broker.on_quote(&q.symbol, px).await;
            }
        }

        // 1b) Kursalarmer: varsle når brukerens nivåer brytes.
        {
            let mut fired: Vec<String> = Vec::new();
            let mut changed = false;
            let alarms_snapshot = {
                let mut st = self.state.lock().unwrap();
                for a in st.alarms.iter_mut() {
                    if a.triggered {
                        continue;
                    }
                    let Some(q) = fresh.iter().find(|q| q.symbol == a.symbol) else { continue };
                    let hit = if a.above { q.last >= a.level } else { q.last <= a.level };
                    if hit {
                        a.triggered = true;
                        changed = true;
                        fired.push(format!(
                            "🔔 Alarm: {} {} {:.2} (kurs nå {:.2})",
                            a.symbol,
                            if a.above { "over" } else { "under" },
                            a.level,
                            q.last
                        ));
                    }
                }
                if changed { Some(st.alarms.clone()) } else { None }
            };
            if let Some(alarms) = alarms_snapshot {
                let _ = self.store.save_alarms(&alarms);
                if !fired.is_empty() && self.cfg.notify.sound {
                    crate::notify::beep();
                }
                for msg in fired {
                    self.log(msg.clone());
                    self.notify(msg.clone());
                    self.state.lock().unwrap().toast(msg);
                }
            }
        }

        // 1c) «Hva ser boten?» — strategienes ståsted per symbol til UI-et.
        //     Oppdateres også under pause/kill, så panelet alltid er ærlig.
        for symbol in fresh.iter().map(|q| q.symbol.clone()).collect::<Vec<_>>() {
            if let Some(view) = self.strategy_view(&symbol) {
                self.state.lock().unwrap().strategy_status.insert(symbol, view);
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
        let cash = match self.broker.cash().await {
            Ok(c) => {
                self.cash_error_logged = false;
                c
            }
            Err(e) => {
                // Si fra ÉN gang — en stille 0 i kontanter er umulig å feilsøke.
                if !self.cash_error_logged {
                    self.cash_error_logged = true;
                    self.log(format!("Klarte ikke hente kontantsaldo fra megleren: {e:#}"));
                }
                0.0
            }
        };
        let equity = cash + positions.iter().map(|p| p.market_value()).sum::<f64>();
        self.risk.observe_equity(equity);

        // 2b) Limit-ordrer: handle når nivået brytes. Utløste ordrer legges
        //     i den manuelle køen og går gjennom risikoreglene lenger ned.
        self.check_limit_orders(&fresh, &positions);

        // 2c) Spareavtaler: månedlig kjøp på fast dag.
        self.check_savings_plans(&fresh).await;

        // 2d) Ukesrapport til mobilen fredag ettermiddag.
        self.maybe_weekly_report(equity);

        // 2e) Dagsfall-alarm: noe du eier faller mer enn grensen på én dag.
        self.check_day_moves(&fresh, &positions);

        // 2f) Dagsoppsummering ved børsslutt.
        self.maybe_daily_summary(&fresh, &positions, equity);

        // 3) Beskyttende exits: stop-loss / take-profit / trailing stop.
        //    Kjører også under strategipause (beskytter beholdningen),
        //    men aldri når kill switch er på.
        self.protected
            .retain(|s| positions.iter().any(|p| p.symbol == *s && p.qty > 1e-9));
        // Oppdater toppnivåer for trailing stop.
        self.peaks
            .retain(|s, _| positions.iter().any(|p| p.symbol == *s && p.qty > 1e-9));
        for p in &positions {
            if p.qty <= 0.0 {
                continue;
            }
            let last = fresh
                .iter()
                .find(|q| q.symbol == p.symbol)
                .and_then(|q| self.base_price(q))
                .unwrap_or(p.last);
            let peak = self.peaks.entry(p.symbol.clone()).or_insert(p.avg_price.max(last));
            *peak = peak.max(last);
        }
        if !self.flags.killed() {
            let sl = self.cfg.risk.stop_loss_pct;
            let tp = self.cfg.risk.take_profit_pct;
            let trail = self.cfg.risk.trailing_stop_pct;
            let mut exits = Vec::new();
            for p in &positions {
                if p.qty <= 0.0 || self.protected.contains(&p.symbol) {
                    continue;
                }
                let Some(q) = fresh.iter().find(|q| q.symbol == p.symbol) else { continue };
                let Some(px) = self.base_price(q) else { continue };
                let reason = protective_exit(p.avg_price, px, sl, tp).or_else(|| {
                    self.peaks
                        .get(&p.symbol)
                        .and_then(|&peak| trailing_exit(peak, px, trail))
                });
                if let Some(reason) = reason {
                    exits.push((p.symbol.clone(), p.qty, px, reason));
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
            let quote = fresh
                .iter()
                .find(|q| q.symbol == symbol)
                .cloned()
                .or_else(|| self.state.lock().unwrap().quotes.get(&symbol).cloned());
            let Some(price) = quote.as_ref().and_then(|q| self.base_price(q)) else {
                self.log(format!("Manuell ordre: mangler kurs eller valutakurs for {symbol}."));
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
                // Handle bare når børsen er åpen (krypto er alltid åpen).
                if self.cfg.market_hours_only
                    && !crate::marketdata::is_trading_open(&q.symbol, Utc::now())
                {
                    continue;
                }
                // Strategien regner i instrumentets valuta; ordrer og
                // risiko i kontovaluta.
                let Some((side, strat_name)) = self.signal_on_tick(&q.symbol, q.last) else {
                    continue;
                };
                let Some(px) = self.base_price(q) else {
                    continue; // valutakurs mangler — allerede logget
                };
                let held = positions
                    .iter()
                    .find(|p| p.symbol == q.symbol)
                    .map_or(0.0, |p| p.qty);

                // Kjøp bare når vi er flate, selg hele posisjonen — enkel
                // og forutsigbar posisjonsstyring.
                let qty = match side {
                    Side::Buy if held <= 0.0 => order_size(
                        self.cfg.strategy.order_value,
                        self.cfg.strategy.order_qty,
                        px,
                        crate::types::is_crypto(&q.symbol),
                    ),
                    Side::Sell if held > 0.0 => held,
                    _ => continue,
                };
                if qty <= 0.0 {
                    self.log(format!(
                        "{}: order_value {:.0} rekker ikke til én enhet (kurs {px:.2}) — hopper over.",
                        q.symbol, self.cfg.strategy.order_value
                    ));
                    continue;
                }

                let order_value = qty * px;
                let pos_value_after = match side {
                    Side::Buy => (held + qty) * px,
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
                        let note = format!("signal fra {strat_name}");
                        let _ = self.place(&q.symbol, side, qty, px, &note).await;
                    }
                }
            }
        }

        // 6) Oppdater UI-tilstand.
        let positions = self.broker.positions().await.unwrap_or(positions);
        let cash = self.broker.cash().await.unwrap_or(cash);
        let accounts = self.broker.accounts().await;
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
            st.accounts = accounts;
            st.equity = equity;
            st.drawdown = drawdown;
            st.push_equity(now.timestamp() as f64, equity);
            st.last_tick = Some(now);
        }
        self.record_equity_snapshot(equity);
    }

    /// Daglig egenkapital-øyeblikksbilde: én rad per dag i databasen
    /// (siste verdi vinner), maks én skriving hvert 5. minutt.
    /// Grunnlaget for den langsiktige utviklingsgrafen i Portefølje.
    fn record_equity_snapshot(&mut self, equity: f64) {
        if equity <= 0.0 {
            return;
        }
        let due = self
            .last_equity_write
            .is_none_or(|t| t.elapsed() > Duration::from_secs(300));
        if !due {
            return;
        }
        self.last_equity_write = Some(std::time::Instant::now());
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let _ = self.store.save_equity_snapshot(&today, equity);

        // Speil i UI-tilstanden: erstatt dagens punkt eller legg til nytt.
        let ts = chrono::Utc::now().timestamp() as f64;
        let mut st = self.state.lock().unwrap();
        let last_is_today = st.equity_daily.last().is_some_and(|&(t, _)| {
            chrono::DateTime::from_timestamp(t as i64, 0)
                .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
                .as_deref()
                == Some(today.as_str())
        });
        if last_is_today {
            if let Some(last) = st.equity_daily.last_mut() {
                *last = (ts, equity);
            }
        } else {
            st.equity_daily.push((ts, equity));
        }
    }

    /// Sjekk ventende limit-ordrer mot ferske kurser. KJØP utløses når
    /// kursen faller til/under nivået, SELG når den stiger til/over.
    /// Utløste ordrer legges i den manuelle køen (risikosjekkes der).
    fn check_limit_orders(&mut self, fresh: &[Quote], positions: &[crate::types::Position]) {
        let mut triggered: Vec<(crate::state::LimitOrder, Quote)> = Vec::new();
        {
            let mut st = self.state.lock().unwrap();
            st.limit_orders.retain(|lo| {
                let Some(q) = fresh.iter().find(|q| q.symbol == lo.symbol) else { return true };
                let hit = match lo.side {
                    Side::Buy => q.last <= lo.level,
                    Side::Sell => q.last >= lo.level,
                };
                if hit {
                    triggered.push((lo.clone(), q.clone()));
                }
                !hit
            });
        }
        if triggered.is_empty() {
            return;
        }
        for (lo, q) in &triggered {
            let Some(px) = self.base_price(q) else {
                self.log(format!(
                    "💤 Limit-ordre for {} utløst, men valutakursen mangler — ordren er fjernet.",
                    lo.symbol
                ));
                continue;
            };
            // Beløpsbaserte ordrer regnes om til antall på utløsningskursen.
            let mut qty = if lo.qty > 0.0 {
                lo.qty
            } else if crate::types::is_crypto(&lo.symbol) {
                lo.amount_kr / px
            } else {
                (lo.amount_kr / px).floor()
            };
            if lo.side == Side::Sell {
                let held = positions.iter().find(|p| p.symbol == lo.symbol).map_or(0.0, |p| p.qty);
                qty = qty.min(held);
            }
            if qty <= 0.0 {
                self.log(format!(
                    "💤 Limit-ordre for {} utløst, men ga 0 i antall (beløp {:.0} kr, kurs {:.2}) — fjernet.",
                    lo.symbol, lo.amount_kr, q.last
                ));
                continue;
            }
            let msg = format!(
                "💤 Limit-ordre utløst: {} {} x{:.4} — kursen nådde {:.2} (nivå {:.2}).",
                lo.side, lo.symbol, qty, q.last, lo.level
            );
            self.log(msg.clone());
            self.notify(msg);
            let mut st = self.state.lock().unwrap();
            st.toast(format!("💤 Limit: {} {} utløst.", lo.side, lo.symbol));
            st.manual_orders.push_back((lo.symbol.clone(), lo.side, qty));
        }
        let snapshot = self.state.lock().unwrap().limit_orders.clone();
        let _ = self.store.save_limit_orders(&snapshot);
    }

    /// Spareavtaler: kjøp for fast beløp når måneden er ny og dagen er nådd.
    async fn check_savings_plans(&mut self, fresh: &[Quote]) {
        use chrono::Datelike;
        let now = chrono::Local::now();
        let month_key = now.format("%Y-%m").to_string();
        let day = now.day();

        let due: Vec<crate::state::SavingsPlan> = {
            let mut st = self.state.lock().unwrap();
            let mut due = Vec::new();
            for p in st.savings_plans.iter_mut() {
                if p.last_run != month_key && day >= p.day {
                    p.last_run = month_key.clone();
                    due.push(p.clone());
                }
            }
            due
        };
        if due.is_empty() {
            return;
        }

        for plan in &due {
            // Kursen kan mangle første tikkene etter oppstart — da henter vi den.
            let quote = match fresh.iter().find(|q| q.symbol == plan.symbol) {
                Some(q) => Some(q.clone()),
                None => self.market.quote(&plan.symbol).await.ok(),
            };
            let px = quote.as_ref().and_then(|q| self.base_price(q));
            let (Some(q), Some(px)) = (quote, px) else {
                // Ikke marker som kjørt likevel — prøv igjen neste tikk.
                let mut st = self.state.lock().unwrap();
                if let Some(p) = st
                    .savings_plans
                    .iter_mut()
                    .find(|p| p.symbol == plan.symbol && p.day == plan.day)
                {
                    p.last_run.clear();
                }
                continue;
            };
            let qty = if crate::types::is_crypto(&plan.symbol) {
                plan.amount_kr / px
            } else {
                (plan.amount_kr / px).floor()
            };
            if qty <= 0.0 {
                let msg = format!(
                    "📅 Spareavtale {}: {:.0} kr rekker ikke til én aksje (kurs {:.2}) — hoppet over denne måneden.",
                    plan.symbol, plan.amount_kr, q.last
                );
                self.log(msg.clone());
                self.notify(msg);
                continue;
            }
            let msg = format!(
                "📅 Spareavtale: kjøper {} x{:.4} for ca. {:.0} kr (dag {} i måneden).",
                plan.symbol, qty, plan.amount_kr, plan.day
            );
            self.log(msg.clone());
            self.notify(msg);
            let mut st = self.state.lock().unwrap();
            st.toast(format!("📅 Spareavtale: kjøper {}.", plan.symbol));
            st.manual_orders.push_back((plan.symbol.clone(), Side::Buy, qty));
        }
        let snapshot = self.state.lock().unwrap().savings_plans.clone();
        let _ = self.store.save_savings_plans(&snapshot);
    }

    /// Ukesrapport: fredag fra kl. 16 — porteføljens uke oppsummert i én
    /// melding. Ukestart-egenkapitalen lagres første tikk hver ISO-uke.
    fn maybe_weekly_report(&mut self, equity: f64) {
        use chrono::{Datelike, Timelike, Weekday};
        if equity <= 0.0 {
            return;
        }
        let now = chrono::Local::now();
        let week_key = format!("{}-W{:02}", now.iso_week().year(), now.iso_week().week());

        // Snapshot av egenkapitalen ved ukestart.
        let start_equity = match self.store.meta_get("week_start_equity") {
            Some(v) if v.starts_with(&week_key) => {
                v.split(':').nth(1).and_then(|s| s.parse::<f64>().ok()).unwrap_or(equity)
            }
            _ => {
                let _ = self.store.meta_set("week_start_equity", &format!("{week_key}:{equity}"));
                equity
            }
        };

        if now.weekday() != Weekday::Fri || now.hour() < 16 {
            return;
        }
        if self.store.meta_get("last_weekly_report").as_deref() == Some(week_key.as_str()) {
            return;
        }
        let _ = self.store.meta_set("last_weekly_report", &week_key);

        let diff = equity - start_equity;
        let pct = if start_equity > 0.0 { diff / start_equity * 100.0 } else { 0.0 };

        // Beste og svakeste beholdning denne uken (kurs nå mot ~1 uke siden).
        type SymbolPct = Option<(String, f64)>;
        let (mut best, mut worst): (SymbolPct, SymbolPct) = (None, None);
        {
            let st = self.state.lock().unwrap();
            let week_ago = chrono::Utc::now().timestamp() as f64 - 7.0 * 86400.0;
            for p in &st.positions {
                let Some(h) = st.history.get(&p.symbol) else { continue };
                let Some(&(_, then)) = h.iter().find(|(t, _)| *t >= week_ago) else { continue };
                let Some(&(_, now_px)) = h.back() else { continue };
                if then <= 0.0 {
                    continue;
                }
                let w_pct = (now_px / then - 1.0) * 100.0;
                if best.as_ref().is_none_or(|(_, b)| w_pct > *b) {
                    best = Some((p.symbol.clone(), w_pct));
                }
                if worst.as_ref().is_none_or(|(_, w)| w_pct < *w) {
                    worst = Some((p.symbol.clone(), w_pct));
                }
            }
        }

        let mut msg = format!(
            "📊 Ukesrapport: porteføljen {}{:.1} % ({}{:.0} kr) denne uken. Egenkapital: {:.0} kr.",
            if pct >= 0.0 { "+" } else { "" },
            pct,
            if diff >= 0.0 { "+" } else { "" },
            diff,
            equity
        );
        if let Some((s, p)) = best {
            msg.push_str(&format!(" Beste: {s} {p:+.1} %."));
        }
        if let Some((s, p)) = worst {
            msg.push_str(&format!(" Svakeste: {s} {p:+.1} %."));
        }
        self.log(msg.clone());
        self.notify(msg);
    }

    /// Én global regel i stedet for én alarm per aksje: varsle når noe i
    /// beholdningen faller mer enn grensen på én dag. Maks én gang per
    /// aksje per dag.
    fn check_day_moves(&mut self, fresh: &[Quote], positions: &[crate::types::Position]) {
        let limit = self.cfg.notify.day_move_alarm_pct;
        if limit <= 0.0 {
            return;
        }
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        self.day_move_alerted.retain(|(d, _)| d == &today);
        for p in positions {
            if p.qty <= 0.0 {
                continue;
            }
            let Some(q) = fresh.iter().find(|q| q.symbol == p.symbol) else { continue };
            let chg = q.change_pct();
            if chg <= -limit && self.day_move_alerted.insert((today.clone(), p.symbol.clone())) {
                let msg = format!("⚠ {} er ned {:.1} % i dag (kurs {:.2}).", p.symbol, -chg, q.last);
                self.log(msg.clone());
                self.notify(msg.clone());
                self.state.lock().unwrap().toast(msg);
                if self.cfg.notify.sound {
                    crate::notify::beep();
                }
            }
        }
    }

    /// Dagsoppsummering rett etter børsslutt (16:30 Oslo-tid, hverdager):
    /// porteføljens dag, største bevegelser og antall handler.
    fn maybe_daily_summary(&mut self, fresh: &[Quote], positions: &[crate::types::Position], equity: f64) {
        use chrono::{Datelike, Timelike, Weekday};
        if !self.cfg.notify.daily_summary || equity <= 0.0 {
            return;
        }
        let now = chrono::Local::now();
        if matches!(now.weekday(), Weekday::Sat | Weekday::Sun) {
            return;
        }
        if now.hour() < 16 || (now.hour() == 16 && now.minute() < 30) {
            return;
        }
        let today = now.format("%Y-%m-%d").to_string();
        if self.store.meta_get("last_daily_summary").as_deref() == Some(today.as_str()) {
            return;
        }
        let _ = self.store.meta_set("last_daily_summary", &today);

        // Dagens bevegelse i kroner: qty × (siste − forrige slutt), i kontovaluta.
        let mut day_kr = 0.0;
        let mut moves: Vec<(String, f64)> = Vec::new();
        for p in positions {
            if p.qty <= 0.0 {
                continue;
            }
            let Some(q) = fresh.iter().find(|q| q.symbol == p.symbol) else { continue };
            let rate = self.fx.get(&q.currency).map(|&(r, _)| r).unwrap_or(1.0);
            day_kr += p.qty * (q.last - q.prev_close) * rate;
            moves.push((p.symbol.clone(), q.change_pct()));
        }
        let day_pct = if equity - day_kr > 0.0 { day_kr / (equity - day_kr) * 100.0 } else { 0.0 };
        moves.sort_by(|a, b| b.1.abs().partial_cmp(&a.1.abs()).unwrap_or(std::cmp::Ordering::Equal));

        let trades_today = {
            let st = self.state.lock().unwrap();
            let prefix = now.format("%d.%m.%Y").to_string();
            st.transactions
                .iter()
                .filter(|t| t.ts.starts_with(&prefix) && t.status == "FYLT")
                .count()
        };

        let mut msg = format!(
            "🌇 Dagen: {}{:.1} % ({}{:.0} kr). Egenkapital: {:.0} kr.",
            if day_pct >= 0.0 { "+" } else { "" },
            day_pct,
            if day_kr >= 0.0 { "+" } else { "" },
            day_kr,
            equity
        );
        for (s, pct) in moves.iter().take(4) {
            msg.push_str(&format!(" {s} {pct:+.1} %."));
        }
        if trades_today > 0 {
            msg.push_str(&format!(" {trades_today} handler i dag."));
        }
        self.log(msg.clone());
        self.notify(msg);
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
                    if self.cfg.notify.sound {
                        crate::notify::beep();
                    }
                    self.notify(format!(
                        "{} {} x{:.0} @ {:.2} [{}] — {}",
                        order.side, order.symbol, order.qty, order.avg_price, order.status, order.note
                    ));
                }
                let _ = self.store.record_order(&order, self.broker.name());
                let tx = crate::state::TxRow {
                    ts: order.created.with_timezone(&chrono::Local).format("%d.%m.%Y %H:%M").to_string(),
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

#[cfg(test)]
mod tests {
    use super::roll_bucket;

    #[test]
    fn bucket_closes_only_on_rollover() {
        // Første observasjon: start lys, ingenting lukkes.
        let (entry, closed) = roll_bucket(None, 100, 50.0);
        assert_eq!(entry, (100, 50.0));
        assert_eq!(closed, None);
        // Samme lys: sluttkursen oppdateres, ingenting lukkes.
        let (entry, closed) = roll_bucket(Some(entry), 100, 51.5);
        assert_eq!(entry, (100, 51.5));
        assert_eq!(closed, None);
        // Nytt lys: forrige lukkes med SISTE kurs i lyset (51.5).
        let (entry, closed) = roll_bucket(Some(entry), 101, 52.0);
        assert_eq!(entry, (101, 52.0));
        assert_eq!(closed, Some(51.5));
        // Hopp over flere lys (stille marked): fortsatt bare én lukking.
        let (_, closed) = roll_bucket(Some(entry), 105, 49.0);
        assert_eq!(closed, Some(52.0));
    }
}
