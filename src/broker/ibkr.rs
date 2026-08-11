use super::Broker;
use crate::config::IbkrCfg;
use crate::types::{Order, OrderRequest, OrderStatus, Position, Side};
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::sync::Mutex;

/// Interactive Brokers via Client Portal Gateway (REST).
///
/// Forutsetter at gatewayen kjører lokalt og at du er innlogget:
///   1. Last ned "Client Portal Gateway" fra IBKR
///   2. `bin/run.sh root/conf.yaml`
///   3. Logg inn på https://localhost:5000 i nettleseren
///
/// Symboler i watchlisten er Yahoo-format ("EQNR.OL"); adapteren søker opp
/// IBKR-conid med delen før punktum ("EQNR").
pub struct IbkrBroker {
    client: reqwest::Client,
    base: String,
    account: String,
    limit_orders: bool,
    limit_slippage_pct: f64,
    realtime_quotes: bool,
    conid_cache: Mutex<HashMap<String, i64>>,
}

impl IbkrBroker {
    pub fn new(cfg: &IbkrCfg) -> Result<Self> {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(cfg.accept_invalid_certs)
            .cookie_store(true)
            .timeout(std::time::Duration::from_secs(20))
            .build()?;
        Ok(Self {
            client,
            base: cfg.base_url.trim_end_matches('/').to_string(),
            account: cfg.account.clone(),
            limit_orders: cfg.limit_orders,
            limit_slippage_pct: cfg.limit_slippage_pct,
            realtime_quotes: cfg.realtime_quotes,
            conid_cache: Mutex::new(HashMap::new()),
        })
    }

    /// Verifiser at gateway-sesjonen er i live. Kall ved oppstart.
    pub async fn check_session(&self) -> Result<()> {
        let url = format!("{}/iserver/accounts", self.base);
        let resp = self.client.get(&url).send().await
            .context("fikk ikke kontakt med IBKR-gatewayen — kjører den, og er du innlogget?")?;
        anyhow::ensure!(
            resp.status().is_success(),
            "IBKR-gateway svarte {} — logg inn på nytt i nettleseren",
            resp.status()
        );
        Ok(())
    }

    fn ibkr_symbol(yahoo_symbol: &str) -> &str {
        yahoo_symbol.split('.').next().unwrap_or(yahoo_symbol)
    }

    async fn resolve_conid(&self, yahoo_symbol: &str) -> Result<i64> {
        if let Some(&c) = self.conid_cache.lock().await.get(yahoo_symbol) {
            return Ok(c);
        }
        let sym = Self::ibkr_symbol(yahoo_symbol);
        let url = format!("{}/iserver/secdef/search?symbol={sym}", self.base);
        let v: Value = self.client.get(&url).send().await?.error_for_status()?.json().await?;
        let first = v
            .as_array()
            .and_then(|a| a.first())
            .with_context(|| format!("fant ingen IBKR-instrument for {sym}"))?;
        let conid = match first.get("conid") {
            Some(Value::Number(n)) => n.as_i64(),
            Some(Value::String(s)) => s.parse().ok(),
            _ => None,
        }
        .with_context(|| format!("ugyldig conid for {sym}"))?;
        self.conid_cache.lock().await.insert(yahoo_symbol.to_string(), conid);
        Ok(conid)
    }
}

#[async_trait::async_trait]
impl Broker for IbkrBroker {
    fn name(&self) -> &'static str {
        "ibkr"
    }

    async fn place_order(&self, req: OrderRequest) -> Result<Order> {
        let conid = self.resolve_conid(&req.symbol).await?;
        let side = match req.side {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        };
        // Limit-ordre (tryggest): sett prisen litt forbi siste kurs, så ordren
        // fylles nær markedet men aldri til en vill pris ved forsinket/tynn data.
        // Kjøp: maks litt OVER siste. Salg: minst litt UNDER siste.
        let use_limit = self.limit_orders && req.ref_price > 0.0;
        let body = if use_limit {
            let price = limit_price(req.side, req.ref_price, self.limit_slippage_pct);
            json!({
                "orders": [{
                    "conid": conid,
                    "orderType": "LMT",
                    "price": price,
                    "side": side,
                    "quantity": req.qty,
                    "tif": "DAY",
                }]
            })
        } else {
            json!({
                "orders": [{
                    "conid": conid,
                    "orderType": "MKT",
                    "side": side,
                    "quantity": req.qty,
                    "tif": "DAY",
                }]
            })
        };
        let url = format!("{}/iserver/account/{}/orders", self.base, self.account);
        let mut v: Value = self.client.post(&url).json(&body).send().await?
            .error_for_status()?.json().await?;

        // IBKR kan svare med bekreftelsesspørsmål ("are you sure ...") som må
        // besvares via /iserver/reply/{id} før ordren går gjennom.
        for _ in 0..3 {
            let Some(first) = v.as_array().and_then(|a| a.first()) else { break };
            if first.get("order_id").is_some() {
                break;
            }
            let Some(reply_id) = first.get("id").and_then(Value::as_str).map(String::from) else {
                anyhow::bail!("uventet ordresvar fra IBKR: {v}");
            };
            let reply_url = format!("{}/iserver/reply/{reply_id}", self.base);
            v = self.client.post(&reply_url)
                .json(&json!({"confirmed": true}))
                .send().await?.error_for_status()?.json().await?;
        }

        let first = v.as_array().and_then(|a| a.first())
            .with_context(|| format!("tomt ordresvar fra IBKR: {v}"))?;
        let order_id = first.get("order_id")
            .map(|x| x.to_string().trim_matches('"').to_string())
            .with_context(|| format!("ordre ikke bekreftet av IBKR: {v}"))?;

        Ok(Order {
            id: order_id,
            symbol: req.symbol,
            side: req.side,
            qty: req.qty,
            status: OrderStatus::Submitted,
            avg_price: req.ref_price,
            created: Utc::now(),
            note: req.note,
        })
    }

    async fn cancel_all(&self) -> Result<()> {
        let url = format!("{}/iserver/account/orders", self.base);
        let v: Value = self.client.get(&url).send().await?.error_for_status()?.json().await?;
        let empty = vec![];
        let orders = v.get("orders").and_then(Value::as_array).unwrap_or(&empty);
        for o in orders {
            let status = o.get("status").and_then(Value::as_str).unwrap_or("");
            if matches!(status, "Filled" | "Cancelled" | "Inactive") {
                continue;
            }
            if let Some(oid) = o.get("orderId") {
                let del = format!(
                    "{}/iserver/account/{}/order/{}",
                    self.base,
                    self.account,
                    oid.to_string().trim_matches('"')
                );
                let _ = self.client.delete(&del).send().await;
            }
        }
        Ok(())
    }

    async fn positions(&self) -> Result<Vec<Position>> {
        let url = format!("{}/portfolio/{}/positions/0", self.base, self.account);
        let v: Value = self.client.get(&url).send().await?.error_for_status()?.json().await?;
        let mut out = Vec::new();
        for p in v.as_array().cloned().unwrap_or_default() {
            let qty = p.get("position").and_then(Value::as_f64).unwrap_or(0.0);
            if qty.abs() < 1e-9 {
                continue;
            }
            let symbol = p.get("ticker")
                .or_else(|| p.get("contractDesc"))
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string();
            out.push(Position {
                symbol,
                qty,
                avg_price: p.get("avgCost").and_then(Value::as_f64).unwrap_or(0.0),
                last: p.get("mktPrice").and_then(Value::as_f64).unwrap_or(0.0),
            });
        }
        Ok(out)
    }

    async fn cash(&self) -> Result<f64> {
        let url = format!("{}/portfolio/{}/ledger", self.base, self.account);
        let v: Value = self.client.get(&url).send().await?.error_for_status()?.json().await?;
        v.pointer("/BASE/cashbalance")
            .and_then(Value::as_f64)
            .context("fant ikke kontantsaldo i IBKR-ledger")
    }

    /// Sanntids siste-kurs fra IBKR (felt 31). Krever markedsdata-abonnement
    /// hos IBKR for ekte sanntid; ellers gir gatewayen forsinket kurs. Feiler
    /// stille (None) så motoren faller tilbake på Yahoo.
    async fn real_time_price(&self, symbol: &str) -> Option<f64> {
        if !self.realtime_quotes || crate::types::is_crypto(symbol) {
            return None;
        }
        let conid = self.resolve_conid(symbol).await.ok()?;
        let url = format!("{}/iserver/marketdata/snapshot?conids={conid}&fields=31", self.base);
        let v: Value = self.client.get(&url).send().await.ok()?.json().await.ok()?;
        let raw = v.as_array()?.first()?.get("31")?;
        // Felt 31 kan være tall eller streng, av og til med prefiks (C/H) når
        // markedet er stengt — plukk ut selve tallet.
        let px = match raw {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => {
                let cleaned: String =
                    s.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
                cleaned.parse().ok()
            }
            _ => None,
        }?;
        (px > 0.0).then_some(px)
    }
}

/// Limit-pris litt forbi siste kurs: kjøp maks litt OVER, salg minst litt
/// UNDER — så ordren fylles nær markedet, men aldri til en vill pris.
/// Rundet til to desimaler (gyldig for de fleste aksjer i NOK/USD).
fn limit_price(side: Side, ref_price: f64, slippage_pct: f64) -> f64 {
    let slip = slippage_pct / 100.0;
    let raw = match side {
        Side::Buy => ref_price * (1.0 + slip),
        Side::Sell => ref_price * (1.0 - slip),
    };
    (raw * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_price_caps_buy_above_and_sell_below() {
        // Kjøp: taket litt over siste kurs.
        assert_eq!(limit_price(Side::Buy, 100.0, 0.3), 100.30);
        // Salg: gulvet litt under siste kurs.
        assert_eq!(limit_price(Side::Sell, 100.0, 0.3), 99.70);
        // Alltid to desimaler.
        assert_eq!(limit_price(Side::Buy, 123.456, 0.0), 123.46);
    }
}
