use crate::config::StrategyCfg;
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
            watchlist: Vec::new(),
            market: crate::market::MarketOverview::default(),
            start_cash: 0.0,
            dividends: BTreeMap::new(),
            transactions: Vec::new(),
            calendar: Vec::new(),
            calendar_note: None,
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
        self.logs.push_front((Utc::now(), msg.into()));
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
        self.orders.push_front(order);
        self.orders.truncate(100);
    }
}

pub type SharedState = Arc<Mutex<UiState>>;
