use crate::types::Order;
use anyhow::Result;
use rusqlite::{params, Connection};
use std::sync::Mutex;

/// SQLite-logg over alle ordrer og hendelser. Historikken trengs både til
/// feilsøking av strategien og som grunnlag for skattemeldingen.
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS orders (
                id TEXT NOT NULL,
                ts TEXT NOT NULL,
                symbol TEXT NOT NULL,
                side TEXT NOT NULL,
                qty REAL NOT NULL,
                price REAL NOT NULL,
                status TEXT NOT NULL,
                broker TEXT NOT NULL,
                note TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS events (
                ts TEXT NOT NULL,
                message TEXT NOT NULL
            );",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn record_order(&self, order: &Order, broker: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO orders (id, ts, symbol, side, qty, price, status, broker, note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                order.id,
                order.created.to_rfc3339(),
                order.symbol,
                order.side.to_string(),
                order.qty,
                order.avg_price,
                order.status.to_string(),
                broker,
                order.note,
            ],
        )?;
        Ok(())
    }

    pub fn record_event(&self, message: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO events (ts, message) VALUES (?1, ?2)",
            params![chrono::Utc::now().to_rfc3339(), message],
        )?;
        Ok(())
    }
}
