use crate::pnl::Fill;
use crate::state::{Alarm, LimitOrder, SavingsPlan, TxRow};
use crate::types::{Order, Position, Side};
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
            );
            CREATE TABLE IF NOT EXISTS alarms (
                symbol TEXT NOT NULL,
                level REAL NOT NULL,
                above INTEGER NOT NULL,
                triggered INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS symbol_strategy (
                symbol TEXT PRIMARY KEY,
                strategy TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS limit_orders (
                symbol TEXT NOT NULL,
                side TEXT NOT NULL,
                qty REAL NOT NULL,
                amount_kr REAL NOT NULL,
                level REAL NOT NULL
            );
            CREATE TABLE IF NOT EXISTS savings_plans (
                symbol TEXT NOT NULL,
                amount_kr REAL NOT NULL,
                day INTEGER NOT NULL,
                last_run TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
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

    /// Lagre alle alarmer (erstatter det som ligger der).
    pub fn save_alarms(&self, alarms: &[Alarm]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM alarms", [])?;
        for a in alarms {
            tx.execute(
                "INSERT INTO alarms (symbol, level, above, triggered) VALUES (?1, ?2, ?3, ?4)",
                params![a.symbol, a.level, a.above as i32, a.triggered as i32],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load_alarms(&self) -> Result<Vec<Alarm>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT symbol, level, above, triggered FROM alarms")?;
        let alarms = stmt
            .query_map([], |r| {
                Ok(Alarm {
                    symbol: r.get(0)?,
                    level: r.get(1)?,
                    above: r.get::<_, i32>(2)? != 0,
                    triggered: r.get::<_, i32>(3)? != 0,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(alarms)
    }

    /// Lagre alle ventende limit-ordrer (erstatter det som ligger der).
    pub fn save_limit_orders(&self, orders: &[LimitOrder]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM limit_orders", [])?;
        for o in orders {
            tx.execute(
                "INSERT INTO limit_orders (symbol, side, qty, amount_kr, level)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![o.symbol, o.side.to_string(), o.qty, o.amount_kr, o.level],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load_limit_orders(&self) -> Result<Vec<LimitOrder>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT symbol, side, qty, amount_kr, level FROM limit_orders")?;
        let orders = stmt
            .query_map([], |r| {
                let side: String = r.get(1)?;
                Ok(LimitOrder {
                    symbol: r.get(0)?,
                    side: if side == "KJØP" { Side::Buy } else { Side::Sell },
                    qty: r.get(2)?,
                    amount_kr: r.get(3)?,
                    level: r.get(4)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(orders)
    }

    /// Lagre alle spareavtaler (erstatter det som ligger der).
    pub fn save_savings_plans(&self, plans: &[SavingsPlan]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM savings_plans", [])?;
        for p in plans {
            tx.execute(
                "INSERT INTO savings_plans (symbol, amount_kr, day, last_run)
                 VALUES (?1, ?2, ?3, ?4)",
                params![p.symbol, p.amount_kr, p.day, p.last_run],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load_savings_plans(&self) -> Result<Vec<SavingsPlan>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT symbol, amount_kr, day, last_run FROM savings_plans")?;
        let plans = stmt
            .query_map([], |r| {
                Ok(SavingsPlan {
                    symbol: r.get(0)?,
                    amount_kr: r.get(1)?,
                    day: r.get(2)?,
                    last_run: r.get(3)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(plans)
    }

    /// Små nøkkel/verdi-fakta som må overleve omstart (ukesrapport-status o.l.).
    pub fn meta_get(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Lagre strategi-per-aksje-valgene (erstatter det som ligger der).
    pub fn save_symbol_strategies(&self, map: &std::collections::BTreeMap<String, String>) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM symbol_strategy", [])?;
        for (symbol, strategy) in map {
            tx.execute(
                "INSERT INTO symbol_strategy (symbol, strategy) VALUES (?1, ?2)",
                params![symbol, strategy],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load_symbol_strategies(&self) -> Result<std::collections::BTreeMap<String, String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT symbol, strategy FROM symbol_strategy")?;
        let map = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(map)
    }

    /// Alle FYLTE ordrer i kronologisk rekkefølge — grunnlag for
    /// FIFO-beregning av realisert gevinst/tap.
    pub fn fills_chronological(&self) -> Result<Vec<Fill>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT ts, symbol, side, qty, price FROM orders
             WHERE status = 'FYLT' ORDER BY ts ASC",
        )?;
        let fills = stmt
            .query_map([], |r| {
                Ok(Fill {
                    ts_rfc3339: r.get(0)?,
                    symbol: r.get(1)?,
                    is_buy: r.get::<_, String>(2)? == "KJØP",
                    qty: r.get(3)?,
                    price: r.get(4)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(fills)
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
    fn symbol_strategy_roundtrip() {
        let store = Store::open(":memory:").unwrap();
        assert!(store.load_symbol_strategies().unwrap().is_empty());
        let mut map = std::collections::BTreeMap::new();
        map.insert("FRO.OL".to_string(), "momentum".to_string());
        map.insert("TEL.OL".to_string(), "rsi".to_string());
        store.save_symbol_strategies(&map).unwrap();
        assert_eq!(store.load_symbol_strategies().unwrap(), map);
        // Fjerning lagres også.
        map.remove("TEL.OL");
        store.save_symbol_strategies(&map).unwrap();
        assert_eq!(store.load_symbol_strategies().unwrap().len(), 1);
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

    #[test]
    fn limit_orders_roundtrip() {
        let store = Store::open(":memory:").unwrap();
        assert!(store.load_limit_orders().unwrap().is_empty());
        let orders = vec![
            LimitOrder { symbol: "EQNR.OL".into(), side: Side::Buy, qty: 0.0, amount_kr: 5_000.0, level: 340.0 },
            LimitOrder { symbol: "MOWI.OL".into(), side: Side::Sell, qty: 25.0, amount_kr: 0.0, level: 210.0 },
        ];
        store.save_limit_orders(&orders).unwrap();
        let loaded = store.load_limit_orders().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].symbol, "EQNR.OL");
        assert!(matches!(loaded[0].side, Side::Buy));
        assert_eq!(loaded[0].amount_kr, 5_000.0);
        assert!(matches!(loaded[1].side, Side::Sell));
        assert_eq!(loaded[1].qty, 25.0);
    }

    #[test]
    fn savings_plans_roundtrip() {
        let store = Store::open(":memory:").unwrap();
        assert!(store.load_savings_plans().unwrap().is_empty());
        let plans = vec![SavingsPlan {
            symbol: "EQNR.OL".into(),
            amount_kr: 2_000.0,
            day: 5,
            last_run: "2026-06".into(),
        }];
        store.save_savings_plans(&plans).unwrap();
        let loaded = store.load_savings_plans().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].day, 5);
        assert_eq!(loaded[0].last_run, "2026-06");
    }

    #[test]
    fn meta_roundtrip() {
        let store = Store::open(":memory:").unwrap();
        assert!(store.meta_get("finnes_ikke").is_none());
        store.meta_set("last_weekly_report", "2026-W28").unwrap();
        assert_eq!(store.meta_get("last_weekly_report").as_deref(), Some("2026-W28"));
        // Overskriving.
        store.meta_set("last_weekly_report", "2026-W29").unwrap();
        assert_eq!(store.meta_get("last_weekly_report").as_deref(), Some("2026-W29"));
    }
}
