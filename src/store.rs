use crate::state::TxRow;
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

    /// Hele transaksjonshistorikken (nyeste først) — også fra tidligere økter.
    pub fn recent_orders(&self, limit: usize) -> Result<Vec<TxRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT ts, symbol, side, qty, price, status, broker, note
             FROM orders ORDER BY ts DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok(TxRow {
                ts: format_ts(&r.get::<_, String>(0)?),
                symbol: r.get(1)?,
                side: r.get(2)?,
                qty: r.get(3)?,
                price: r.get(4)?,
                status: r.get(5)?,
                broker: r.get(6)?,
                note: r.get(7)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}

fn format_ts(raw: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|d| d.format("%d.%m.%Y %H:%M").to_string())
        .unwrap_or_else(|_| raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{OrderStatus, Side};
    use chrono::Utc;

    #[test]
    fn records_and_reads_back_orders() {
        let store = Store::open(":memory:").unwrap();
        let order = Order {
            id: "T1".into(),
            symbol: "EQNR.OL".into(),
            side: Side::Buy,
            qty: 10.0,
            status: OrderStatus::Filled,
            avg_price: 342.5,
            created: Utc::now(),
            note: "test".into(),
        };
        store.record_order(&order, "paper").unwrap();
        let txs = store.recent_orders(10).unwrap();
        assert_eq!(txs.len(), 1);
        assert_eq!(txs[0].symbol, "EQNR.OL");
        assert_eq!(txs[0].side, "KJØP");
        assert_eq!(txs[0].broker, "paper");
    }
}
