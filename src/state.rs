use crate::config::{BacktestCfg, StrategyCfg};
use crate::types::{Candle, ExternalPosition, Order, Position, Quote, Side};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Flagg delt mellom UI (skriver) og engine (leser).
#[derive(Default)]
pub struct Flags {
    pub quit: AtomicBool,
    /// Kill switch: stopp all handel og kanseller åpne ordrer.
    pub killed: AtomicBool,
    /// Pause: strategien evalueres ikke, men kurser oppdateres fortsatt.
    pub paused: AtomicBool,
}

impl Flags {
    pub fn quit(&self) -> bool {
        self.quit.load(Ordering::Relaxed)
    }
    pub fn killed(&self) -> bool {
        self.killed.load(Ordering::Relaxed)
    }
    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }
}

/// Alt UI-et trenger for å tegne skjermen. Engine skriver, UI leser.
pub struct UiState {
    pub mode: String,
    pub broker_name: String,
    pub cash: f64,
    pub equity: f64,
    pub drawdown: f64,
    pub quotes: BTreeMap<String, Quote>,
    /// Kurshistorikk per symbol som (unix-tid, kurs) — først daglige
    /// sluttkurser fra oppstart, deretter live-tikk. Brukes av grafen i GUI-et.
    pub history: BTreeMap<String, VecDeque<(f64, f64)>>,
    pub positions: Vec<Position>,
    pub nordnet_positions: Vec<ExternalPosition>,
    pub nordnet_enabled: bool,
    pub orders: VecDeque<Order>,
    pub logs: VecDeque<(DateTime<Utc>, String)>,
    pub last_tick: Option<DateTime<Utc>>,
    /// Egenkapital over tid denne økten som (unix-tid, verdi).
    pub equity_history: VecDeque<(f64, f64)>,
    /// Manuelle ordrer fra GUI-et — engine tømmer køen hver tikk.
    pub manual_orders: VecDeque<(String, Side, f64)>,
    /// (fast, slow) SMA-vinduer fra konfigen, så grafen kan tegne dem.
    pub sma_windows: (usize, usize),
    /// Daglige OHLC-stolper per symbol — candlestick-graf og backtesting.
    pub candles: BTreeMap<String, Vec<Candle>>,
    /// Strategien engine kjører akkurat nå.
    pub strategy_name: String,
    /// Settes av GUI-et når brukeren vil bytte strategi; engine plukker den opp.
    pub strategy_request: Option<String>,
    /// Strategiparametre fra konfigen — brukes av backtesting i GUI-et.
    pub strategy_cfg: StrategyCfg,
    /// Kostnadsmodell for backtesting (kurtasje, glidning).
    pub backtest_cfg: BacktestCfg,
    /// Symboler boten følger. Starter fra konfigen; GUI-et kan legge til
    /// flere fra markedsskjermene mens appen kjører.
    pub watchlist: Vec<String>,
    /// Markedsoversikten (mest omsatte, daytrading, fond, ukesanalyse).
    pub market: crate::market::MarketOverview,
    /// Startkapital — grunnlag for total avkastning.
    pub start_cash: f64,
    /// Utbytte per aksje siste 12 mnd, per symbol (fra Yahoo).
    pub dividends: BTreeMap<String, f64>,
    /// Komplett ordrehistorikk fra databasen (nyeste først).
    pub transactions: Vec<TxRow>,
    /// Kommende selskapshendelser (rapporter, utbyttedatoer).
    pub calendar: Vec<crate::calendar::CalendarEvent>,
    /// Feilmelding hvis kalenderdata ikke kunne hentes.
    pub calendar_note: Option<String>,
    /// Brukerdefinerte kursalarmer (lagres i databasen).
    pub alarms: Vec<Alarm>,
    /// Strategi-overstyring per symbol; symboler uten oppslag bruker
    /// standardstrategien (strategy_name).
    pub symbol_strategy: BTreeMap<String, String>,
    /// Valutakurser mot kontovaluta, f.eks. "USD" → 10.52.
    pub fx_rates: BTreeMap<String, f64>,
    /// Tikk-intervallet fra konfigen — brukes av vakthund-indikatoren.
    pub poll_secs: u64,
    /// Loggfil — alle hendelser speiles hit for feilsøking i ettertid.
    pub log_path: Option<String>,
    /// Toasts: (utløps-unixtid, melding) — små popup-kort i GUI-et.
    pub toasts: VecDeque<(i64, String)>,
    /// Aksjesøk: resultater og ventestatus.
    /// Aksjesøk: (symbol, beskrivelse, kategori) — kategori er "Aksje",
    /// "Fond/ETF", "Krypto" eller "Indeks".
    pub search_results: Vec<(String, String, String)>,
    pub search_pending: bool,
    /// Ny versjon tilgjengelig: (versjon, nedlastingsside).
    pub update_available: Option<(String, String)>,
    /// «Morgan» — AI-analysesjefen: ferdig rapport, feilmelding og ventestatus.
    pub morgan_report: Option<String>,
    pub morgan_error: Option<String>,
    pub morgan_pending: bool,
    /// Ventende limit-ordrer: handles automatisk når nivået brytes.
    pub limit_orders: Vec<LimitOrder>,
    /// Spareavtaler: fast kjøp i kroner på fast dag i måneden.
    pub savings_plans: Vec<SavingsPlan>,
    /// Nyheter for valgt symbol: (symbolet de gjelder, sakene, ventestatus).
    pub news_symbol: String,
    pub news: Vec<NewsItem>,
    pub news_pending: bool,
    /// Morgans rapportarkiv: (id, tidspunkt, tittel), nyest først.
    pub morgan_archive: Vec<(i64, String, String)>,
    /// 🤖 Autopilotens siste beslutning, klar for visning i GUI-et.
    pub autopilot_status: Option<String>,
    /// Daglig egenkapital over tid (unixtid, verdi) — varig historikk.
    pub equity_daily: Vec<(f64, f64)>,
    /// Referanseindeksen (unixtid, kurs) til «slår jeg børsen?»-grafen.
    pub benchmark: Vec<(f64, f64)>,
    pub benchmark_name: String,
}

/// En ventende limit-ordre. KJØP utløses når kursen faller til eller under
/// nivået, SELG når den stiger til eller over. Nivået er i instrumentets
/// valuta (samme tall som i grafen); beløpet i kroner.
#[derive(Debug, Clone)]
pub struct LimitOrder {
    pub symbol: String,
    pub side: Side,
    /// Antall — 0 hvis ordren er beløpsbasert.
    pub qty: f64,
    /// Beløp i kroner — 0 hvis ordren er antallsbasert.
    pub amount_kr: f64,
    pub level: f64,
}

/// En spareavtale: kjøp for et fast kronebeløp på en fast dag hver måned.
#[derive(Debug, Clone)]
pub struct SavingsPlan {
    pub symbol: String,
    pub amount_kr: f64,
    /// Dag i måneden (1–28).
    pub day: u32,
    /// Måneden den sist ble utført, "2026-07" — tom hvis aldri.
    pub last_run: String,
}

/// Én nyhetssak fra Yahoo Finance.
#[derive(Debug, Clone)]
pub struct NewsItem {
    pub title: String,
    pub publisher: String,
    pub url: String,
    /// Publisert (unixtid).
    pub ts: i64,
}

/// Brukerdefinert kursalarm — varsler mobil/logg når nivået brytes.
#[derive(Debug, Clone)]
pub struct Alarm {
    pub symbol: String,
    pub level: f64,
    /// true = varsle når kursen går OVER nivået, false = UNDER.
    pub above: bool,
    pub triggered: bool,
}

/// Én rad i transaksjonshistorikken — display-klar.
#[derive(Debug, Clone)]
pub struct TxRow {
    pub ts: String,
    pub symbol: String,
    pub side: String,
    pub qty: f64,
    pub price: f64,
    pub status: String,
    pub broker: String,
    pub note: String,
}

impl UiState {
    pub fn new(mode: &str, broker_name: &str, nordnet_enabled: bool) -> Self {
        Self {
            mode: mode.to_string(),
            broker_name: broker_name.to_string(),
            cash: 0.0,
            equity: 0.0,
            drawdown: 0.0,
            quotes: BTreeMap::new(),
            history: BTreeMap::new(),
            positions: Vec::new(),
            nordnet_positions: Vec::new(),
            nordnet_enabled,
            orders: VecDeque::new(),
            logs: VecDeque::new(),
            last_tick: None,
            equity_history: VecDeque::new(),
            manual_orders: VecDeque::new(),
            sma_windows: (5, 20),
            candles: BTreeMap::new(),
            strategy_name: String::new(),
            strategy_request: None,
            strategy_cfg: StrategyCfg::default(),
            backtest_cfg: BacktestCfg::default(),
            watchlist: Vec::new(),
            market: crate::market::MarketOverview::default(),
            start_cash: 0.0,
            dividends: BTreeMap::new(),
            transactions: Vec::new(),
            calendar: Vec::new(),
            calendar_note: None,
            alarms: Vec::new(),
            symbol_strategy: BTreeMap::new(),
            fx_rates: BTreeMap::new(),
            poll_secs: 15,
            log_path: None,
            toasts: VecDeque::new(),
            search_results: Vec::new(),
            search_pending: false,
            update_available: None,
            morgan_report: None,
            morgan_error: None,
            morgan_pending: false,
            limit_orders: Vec::new(),
            savings_plans: Vec::new(),
            news_symbol: String::new(),
            news: Vec::new(),
            news_pending: false,
            morgan_archive: Vec::new(),
            autopilot_status: None,
            equity_daily: Vec::new(),
            benchmark: Vec::new(),
            benchmark_name: String::new(),
        }
    }

    /// Vis et lite popup-kort i 6 sekunder.
    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toasts.push_back((Utc::now().timestamp() + 6, msg.into()));
        while self.toasts.len() > 5 {
            self.toasts.pop_front();
        }
    }

    pub fn push_transaction(&mut self, tx: TxRow) {
        self.transactions.insert(0, tx);
        self.transactions.truncate(1000);
    }

    /// Legg et symbol til i watchlisten (fra markedsskjermene).
    pub fn follow(&mut self, symbol: &str) {
        if !self.watchlist.iter().any(|s| s == symbol) {
            self.watchlist.push(symbol.to_string());
            self.log(format!("{symbol} lagt til i watchlisten."));
        }
    }

    pub fn push_equity(&mut self, ts: f64, equity: f64) {
        self.equity_history.push_back((ts, equity));
        if self.equity_history.len() > 5000 {
            self.equity_history.pop_front();
        }
    }

    pub fn log(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        let now = Utc::now();
        // Speil til loggfil så hendelser kan feilsøkes etter at appen er lukket.
        if let Some(path) = &self.log_path {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(f, "{} {msg}", now.format("%Y-%m-%d %H:%M:%S"));
            }
        }
        self.logs.push_front((now, msg));
        self.logs.truncate(200);
    }

    pub fn push_price(&mut self, symbol: &str, ts: f64, price: f64) {
        let h = self.history.entry(symbol.to_string()).or_default();
        h.push_back((ts, price));
        if h.len() > 5000 {
            h.pop_front();
        }
    }

    pub fn push_order(&mut self, order: Order) {
        let icon = if order.status == crate::types::OrderStatus::Rejected { "❌" } else { "✅" };
        self.toast(format!(
            "{icon} {} {} x{} @ {:.2}",
            order.side, order.symbol, order.qty, order.avg_price
        ));
        self.orders.push_front(order);
        self.orders.truncate(100);
    }
}

pub type SharedState = Arc<Mutex<UiState>>;
