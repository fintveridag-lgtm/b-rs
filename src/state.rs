use crate::types::{ExternalPosition, Order, Position, Quote, Side};
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
