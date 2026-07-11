use crate::state::TxRow;
use crate::types::{Order, Position};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
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
            );
            CREATE TABLE IF NOT EXISTS paper_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                cash REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS paper_positions (
                symbol TEXT PRIMARY KEY,
                qty REAL NOT NULL,
                avg_price REAL NOT NULL
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

    /// Lagre papirporteføljen så den overlever omstart.
    pub fn save_paper_state(&self, cash: f64, positions: &[Position]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO paper_state (id, cash) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET cash = excluded.cash",
            params![cash],
        )?;
        tx.execute("DELETE FROM paper_positions", [])?;
        for p in positions {
            tx.execute(
                "INSERT INTO paper_positions (symbol, qty, avg_price) VALUES (?1, ?2, ?3)",
                params![p.symbol, p.qty, p.avg_price],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Hent lagret papirportefølje, hvis noen finnes.
    pub fn load_paper_state(&self) -> Result<Option<(f64, Vec<Position>)>> {
        let conn = self.conn.lock().unwrap();
        let cash: Option<f64> = conn
            .query_row("SELECT cash FROM paper_state WHERE id = 1", [], |r| r.get(0))
            .optional()?;
        let Some(cash) = cash else { return Ok(None) };
        let mut stmt = conn.prepare("SELECT symbol, qty, avg_price FROM paper_positions")?;
        let positions = stmt
            .query_map([], |r| {
                let avg_price: f64 = r.get(2)?;
                Ok(Position {
                    symbol: r.get(0)?,
                    qty: r.get(1)?,
                    avg_price,
                    // Siste kurs hentes ferskt ved neste tikk.
                    last: avg_price,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(Some((cash, positions)))
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

    #[test]
    fn paper_state_roundtrip() {
        let store = Store::open(":memory:").unwrap();
        assert!(store.load_paper_state().unwrap().is_none());

        let positions = vec![Position { symbol: "EQNR.OL".into(), qty: 10.0, avg_price: 340.0, last: 342.0 }];
        store.save_paper_state(87_500.0, &positions).unwrap();

        let (cash, loaded) = store.load_paper_state().unwrap().unwrap();
        assert_eq!(cash, 87_500.0);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].symbol, "EQNR.OL");
        assert_eq!(loaded[0].avg_price, 340.0);

        // Tom portefølje overskriver.
        store.save_paper_state(100_000.0, &[]).unwrap();
        let (cash, loaded) = store.load_paper_state().unwrap().unwrap();
        assert_eq!(cash, 100_000.0);
        assert!(loaded.is_empty());
    }
}
