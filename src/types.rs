use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Side::Buy => write!(f, "KJØP"),
            Side::Sell => write!(f, "SELG"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    Submitted,
    Filled,
    Rejected,
    Cancelled,
}

impl std::fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            OrderStatus::Submitted => "SENDT",
            OrderStatus::Filled => "FYLT",
            OrderStatus::Rejected => "AVVIST",
            OrderStatus::Cancelled => "KANSELLERT",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone)]
pub struct Quote {
    pub symbol: String,
    pub last: f64,
    pub prev_close: f64,
    /// Instrumentets valuta fra Yahoo ("NOK", "USD", …); tom = ukjent,
    /// behandles som kontovaluta.
    pub currency: String,
    pub ts: DateTime<Utc>,
}

/// Kryptopar ("BTC-USD") kjennetegnes av bindestrek uten børssuffiks —
/// de handles døgnet rundt og i brøkdeler.
pub fn is_crypto(symbol: &str) -> bool {
    symbol.contains('-') && !symbol.contains('.')
}

impl Quote {
    pub fn change_pct(&self) -> f64 {
        if self.prev_close > 0.0 {
            (self.last / self.prev_close - 1.0) * 100.0
        } else {
            0.0
        }
    }
}

#[derive(Debug, Clone)]
pub struct OrderRequest {
    pub symbol: String,
    pub side: Side,
    pub qty: f64,
    /// Siste kjente kurs — brukes som fyllkurs i papirmegleren og til risikosjekk.
    pub ref_price: f64,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct Order {
    pub id: String,
    pub symbol: String,
    pub side: Side,
    pub qty: f64,
    pub status: OrderStatus,
    pub avg_price: f64,
    pub created: DateTime<Utc>,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct Position {
    pub symbol: String,
    pub qty: f64,
    pub avg_price: f64,
    pub last: f64,
}

impl Position {
    pub fn market_value(&self) -> f64 {
        self.qty * self.last
    }
    pub fn unrealized(&self) -> f64 {
        (self.last - self.avg_price) * self.qty
    }
}

/// Én dags OHLC-stolpe — grunnlag for candlestick-grafen og backtesting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candle {
    pub ts: f64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

/// Posisjon lest fra Nordnet (lesemodus — handles aldri på).
#[derive(Debug, Clone)]
pub struct ExternalPosition {
    pub symbol: String,
    pub name: String,
    pub qty: f64,
    pub market_value: f64,
}
