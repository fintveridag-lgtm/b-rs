use super::Broker;
use crate::store::Store;
use crate::types::{Order, OrderRequest, OrderStatus, Position, Side};
use anyhow::Result;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

struct State {
    cash: f64,
    positions: HashMap<String, Position>,
}

/// Simulert megler: fyller markedsordrer umiddelbart til siste kjente kurs.
/// Standardvalget — appen bør alltid utvikles og testes mot denne først.
/// Porteføljen lagres i databasen etter hver handel og gjenopprettes ved
/// oppstart, så papirtesting kan pågå over dager og uker.
pub struct PaperBroker {
    state: Mutex<State>,
    seq: AtomicU64,
    store: Option<Arc<Store>>,
}

impl PaperBroker {
    pub fn new(starting_cash: f64, store: Option<Arc<Store>>, reset: bool) -> Self {
        let mut cash = starting_cash;
        let mut positions = HashMap::new();
        if let Some(s) = &store {
            if reset {
                // Nullstill også det som ligger lagret.
                let _ = s.save_paper_state(starting_cash, &[]);
            } else if let Ok(Some((saved_cash, saved))) = s.load_paper_state() {
                cash = saved_cash;
                positions = saved
                    .into_iter()
                    .map(|p| (p.symbol.clone(), p))
                    .collect();
            }
        }
        Self {
            state: Mutex::new(State { cash, positions }),
            seq: AtomicU64::new(1),
            store,
        }
    }

    fn persist(&self, st: &State) {
        if let Some(s) = &self.store {
            let positions: Vec<Position> = st.positions.values().cloned().collect();
            let _ = s.save_paper_state(st.cash, &positions);
        }
    }
}

#[async_trait::async_trait]
impl Broker for PaperBroker {
    fn name(&self) -> &'static str {
        "paper"
    }

    async fn place_order(&self, req: OrderRequest) -> Result<Order> {
        let id = format!("P{}", self.seq.fetch_add(1, Ordering::Relaxed));
        let mut st = self.state.lock().await;
        let price = req.ref_price;
        let value = req.qty * price;

        let status = match req.side {
            Side::Buy => {
                if value > st.cash {
                    OrderStatus::Rejected
                } else {
                    st.cash -= value;
                    let pos = st.positions.entry(req.symbol.clone()).or_insert(Position {
                        symbol: req.symbol.clone(),
                        qty: 0.0,
                        avg_price: 0.0,
                        last: price,
                    });
                    let total_cost = pos.avg_price * pos.qty + value;
                    pos.qty += req.qty;
                    pos.avg_price = total_cost / pos.qty;
                    pos.last = price;
                    OrderStatus::Filled
                }
            }
            Side::Sell => {
                let held = st.positions.get(&req.symbol).map_or(0.0, |p| p.qty);
                if req.qty > held + 1e-9 {
                    OrderStatus::Rejected
                } else {
                    st.cash += value;
                    if let Some(pos) = st.positions.get_mut(&req.symbol) {
                        pos.qty -= req.qty;
                        pos.last = price;
                        if pos.qty < 1e-9 {
                            st.positions.remove(&req.symbol);
                        }
                    }
                    OrderStatus::Filled
                }
            }
        };

        if status == OrderStatus::Filled {
            self.persist(&st);
        }

        let note = if status == OrderStatus::Rejected {
            match req.side {
                Side::Buy => format!("{} — avvist: ikke nok kontanter", req.note),
                Side::Sell => format!("{} — avvist: ikke nok aksjer", req.note),
            }
        } else {
            req.note
        };

        Ok(Order {
            id,
            symbol: req.symbol,
            side: req.side,
            qty: req.qty,
            status,
            avg_price: price,
            created: Utc::now(),
            note,
        })
    }

    async fn cancel_all(&self) -> Result<()> {
        // Papirordrer fylles umiddelbart, så det finnes aldri åpne ordrer.
        Ok(())
    }

    async fn positions(&self) -> Result<Vec<Position>> {
        let st = self.state.lock().await;
        let mut v: Vec<Position> = st.positions.values().cloned().collect();
        v.sort_by(|a, b| a.symbol.cmp(&b.symbol));
        Ok(v)
    }

    async fn cash(&self) -> Result<f64> {
        Ok(self.state.lock().await.cash)
    }

    async fn on_quote(&self, symbol: &str, price: f64) {
        let mut st = self.state.lock().await;
        if let Some(pos) = st.positions.get_mut(symbol) {
            pos.last = price;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn portfolio_survives_restart() {
        let store = Arc::new(Store::open(":memory:").unwrap());

        let broker = PaperBroker::new(100_000.0, Some(store.clone()), false);
        let order = broker
            .place_order(OrderRequest {
                symbol: "EQNR.OL".into(),
                side: Side::Buy,
                qty: 10.0,
                ref_price: 300.0,
                note: "test".into(),
            })
            .await
            .unwrap();
        assert_eq!(order.status, OrderStatus::Filled);

        // "Omstart": ny megler mot samme database.
        let restarted = PaperBroker::new(100_000.0, Some(store.clone()), false);
        assert_eq!(restarted.cash().await.unwrap(), 97_000.0);
        let positions = restarted.positions().await.unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].qty, 10.0);

        // Reset gir blanke ark.
        let fresh = PaperBroker::new(100_000.0, Some(store), true);
        assert_eq!(fresh.cash().await.unwrap(), 100_000.0);
        assert!(fresh.positions().await.unwrap().is_empty());
    }
}
