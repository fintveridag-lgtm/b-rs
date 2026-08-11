//! Multi-megler: to meglere samtidig, rutet på symboltype. Krypto
//! (bindestrek-symboler som "BTC-USD") går til kryptomegleren, alt annet
//! (aksjer, fond, ETF-er) til aksjemegleren. Resten av appen ser én megler.
//!
//! Valuta: aksjemegleren fører kontoen i basisvaluta (kroner), krypto-
//! megleren i sin egen (USD hos Revolut X). Posisjoner og samlet kontant-
//! beholdning regnes om til basisvaluta med valutakursene motoren allerede
//! henter — så grafer, risiko og portefølje forblir i kroner.

use super::Broker;
use crate::state::SharedState;
use crate::types::{is_crypto, Order, OrderRequest, Position};
use anyhow::Result;
use std::sync::Arc;

pub struct MultiBroker {
    stocks: Arc<dyn Broker>,
    crypto: Arc<dyn Broker>,
    /// Kryptomeglerens kontovaluta ("USD" hos Revolut X); tom = basisvaluta.
    crypto_currency: String,
    /// Delt UI-tilstand — kilden til valutakursene (motoren skriver dem).
    state: SharedState,
}

impl MultiBroker {
    pub fn new(
        stocks: Arc<dyn Broker>,
        crypto: Arc<dyn Broker>,
        crypto_currency: String,
        state: SharedState,
    ) -> Self {
        Self { stocks, crypto, crypto_currency, state }
    }

    /// Kurs for å regne kryptomeglerens valuta om til basisvaluta.
    /// 1.0 til valutakursen er hentet (første tikkene etter oppstart).
    fn crypto_rate(&self) -> f64 {
        if self.crypto_currency.is_empty() {
            return 1.0;
        }
        self.state
            .lock()
            .unwrap()
            .fx_rates
            .get(&self.crypto_currency)
            .copied()
            .unwrap_or(1.0)
    }

    fn route(&self, symbol: &str) -> &Arc<dyn Broker> {
        if is_crypto(symbol) {
            &self.crypto
        } else {
            &self.stocks
        }
    }
}

#[async_trait::async_trait]
impl Broker for MultiBroker {
    fn name(&self) -> &'static str {
        "multi"
    }

    async fn place_order(&self, req: OrderRequest) -> Result<Order> {
        self.route(&req.symbol).place_order(req).await
    }

    async fn real_time_price(&self, symbol: &str) -> Option<f64> {
        self.route(symbol).real_time_price(symbol).await
    }

    async fn cancel_all(&self) -> Result<()> {
        // Kill switch skal nå begge — samle feilene i stedet for å stoppe
        // på første.
        let a = self.stocks.cancel_all().await;
        let b = self.crypto.cancel_all().await;
        match (a, b) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(e), Ok(())) => Err(e.context("aksjemegleren")),
            (Ok(()), Err(e)) => Err(e.context("kryptomegleren")),
            (Err(e1), Err(e2)) => Err(e1.context(format!("kryptomegleren feilet også: {e2:#}"))),
        }
    }

    async fn positions(&self) -> Result<Vec<Position>> {
        let mut out = self.stocks.positions().await?;
        let rate = self.crypto_rate();
        for mut p in self.crypto.positions().await? {
            // Kryptoposisjoner føres i meglerens valuta — regn om til
            // basisvaluta så portefølje/risiko/grafer forblir i kroner.
            p.avg_price *= rate;
            p.last *= rate;
            out.push(p);
        }
        Ok(out)
    }

    async fn cash(&self) -> Result<f64> {
        let stocks = self.stocks.cash().await?;
        let crypto = self.crypto.cash().await?;
        Ok(stocks + crypto * self.crypto_rate())
    }

    async fn on_quote(&self, symbol: &str, price: f64) {
        self.route(symbol).on_quote(symbol, price).await;
    }

    async fn accounts(&self) -> Vec<(String, f64, String)> {
        let stocks_cash = self.stocks.cash().await.unwrap_or(0.0);
        let crypto_cash = self.crypto.cash().await.unwrap_or(0.0);
        let crypto_cur = if self.crypto_currency.is_empty() {
            "kr".to_string()
        } else {
            self.crypto_currency.clone()
        };
        vec![
            (format!("{} (aksjer)", self.stocks.name()), stocks_cash, "kr".to_string()),
            (format!("{} (krypto)", self.crypto.name()), crypto_cash, crypto_cur),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::paper::PaperBroker;
    use crate::state::UiState;
    use crate::types::Side;
    use std::sync::Mutex;

    fn multi() -> MultiBroker {
        let state = Arc::new(Mutex::new(UiState::new("paper", "test", false)));
        // Kjent kurs: 10 kr per USD.
        state.lock().unwrap().fx_rates.insert("USD".into(), 10.0);
        let stocks: Arc<dyn Broker> = Arc::new(PaperBroker::new(100_000.0, None, false));
        let crypto: Arc<dyn Broker> = Arc::new(PaperBroker::new(1_000.0, None, false));
        MultiBroker::new(stocks, crypto, "USD".into(), state)
    }

    #[tokio::test]
    async fn routes_orders_by_symbol_type() {
        let m = multi();
        // Aksje → aksjemegleren, krypto → kryptomegleren.
        m.place_order(OrderRequest {
            symbol: "EQNR.OL".into(),
            side: Side::Buy,
            qty: 10.0,
            ref_price: 340.0,
            note: "test".into(),
        })
        .await
        .unwrap();
        m.place_order(OrderRequest {
            symbol: "BTC-USD".into(),
            side: Side::Buy,
            qty: 0.001,
            ref_price: 60_000.0,
            note: "test".into(),
        })
        .await
        .unwrap();

        let stocks_pos = m.stocks.positions().await.unwrap();
        let crypto_pos = m.crypto.positions().await.unwrap();
        assert_eq!(stocks_pos.len(), 1);
        assert_eq!(stocks_pos[0].symbol, "EQNR.OL");
        assert_eq!(crypto_pos.len(), 1);
        assert_eq!(crypto_pos[0].symbol, "BTC-USD");

        // Samlet posisjonsliste har begge, med krypto omregnet (×10).
        let all = m.positions().await.unwrap();
        assert_eq!(all.len(), 2);
        let btc = all.iter().find(|p| p.symbol == "BTC-USD").unwrap();
        assert!((btc.avg_price - 600_000.0).abs() < 1.0, "60 000 USD × 10 = 600 000 kr");
    }

    #[tokio::test]
    async fn cash_is_converted_and_accounts_listed() {
        let m = multi();
        // 100 000 kr + 1 000 USD × 10 = 110 000 kr.
        let cash = m.cash().await.unwrap();
        assert!((cash - 110_000.0).abs() < 0.01);

        let accounts = m.accounts().await;
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].2, "kr");
        assert_eq!(accounts[1].2, "USD");
        assert!((accounts[1].1 - 1_000.0).abs() < 0.01, "kryptokontoen vises i egen valuta");
    }
}
