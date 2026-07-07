use crate::config::NordnetCfg;
use crate::types::ExternalPosition;
use anyhow::{Context, Result};
use serde_json::{json, Value};

/// Nordnet-LESEMODUS via det uoffisielle web-API-et.
///
/// VIKTIG:
///  - Dette er ikke et offisielt, støttet API. Endepunktene kan endres uten
///    varsel, og bruk kan være i strid med Nordnets vilkår. Brukes på eget
///    ansvar, og KUN til lesing — denne modulen legger aldri ordrer.
///  - Innlogging med brukernavn/passord feiler hvis kontoen krever
///    BankID-innlogging. Sett da nordnet.enabled = false.
///
/// Legitimasjon leses fra miljøvariablene NORDNET_USERNAME og NORDNET_PASSWORD.
pub struct NordnetReader {
    client: reqwest::Client,
    base: String,
    logged_in: bool,
}

impl NordnetReader {
    pub fn new(cfg: &NordnetCfg) -> Result<Self> {
        let client = reqwest::Client::builder()
            .cookie_store(true)
            .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36")
            .timeout(std::time::Duration::from_secs(20))
            .build()?;
        Ok(Self {
            client,
            base: cfg.base_url.trim_end_matches('/').to_string(),
            logged_in: false,
        })
    }

    async fn login(&mut self) -> Result<()> {
        let username = std::env::var("NORDNET_USERNAME")
            .context("miljøvariabelen NORDNET_USERNAME er ikke satt")?;
        let password = std::env::var("NORDNET_PASSWORD")
            .context("miljøvariabelen NORDNET_PASSWORD er ikke satt")?;

        // Steg 1: anonym sesjon (setter cookies).
        self.client
            .get(format!("{}/login", self.base))
            .header("client-id", "NEXT")
            .header("Accept", "application/json")
            .send()
            .await
            .context("fikk ikke kontakt med Nordnet")?;

        // Steg 2: brukernavn/passord.
        let resp = self.client
            .post(format!("{}/authentication/basic/login", self.base))
            .header("client-id", "NEXT")
            .header("Accept", "application/json")
            .json(&json!({ "username": username, "password": password }))
            .send()
            .await?;

        anyhow::ensure!(
            resp.status().is_success(),
            "Nordnet-innlogging feilet ({}). Krever kontoen BankID? Da fungerer ikke lesemodus.",
            resp.status()
        );
        self.logged_in = true;
        Ok(())
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let resp = self.client
            .get(format!("{}{path}", self.base))
            .header("client-id", "NEXT")
            .header("Accept", "application/json")
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    /// Hent alle posisjoner på tvers av kontoer. Logger inn ved behov,
    /// og på nytt hvis sesjonen har utløpt.
    pub async fn positions(&mut self) -> Result<Vec<ExternalPosition>> {
        if !self.logged_in {
            self.login().await?;
        }
        match self.fetch_positions().await {
            Ok(p) => Ok(p),
            Err(_) => {
                // Sesjonen kan ha utløpt — prøv én reinnlogging.
                self.logged_in = false;
                self.login().await?;
                self.fetch_positions().await
            }
        }
    }

    async fn fetch_positions(&self) -> Result<Vec<ExternalPosition>> {
        let accounts = self.get("/accounts").await?;
        let mut out = Vec::new();
        for acc in accounts.as_array().cloned().unwrap_or_default() {
            let Some(accid) = acc.get("accid").and_then(Value::as_i64) else { continue };
            let positions = self.get(&format!("/accounts/{accid}/positions")).await?;
            for pos in positions.as_array().cloned().unwrap_or_default() {
                out.push(parse_position(&pos));
            }
        }
        Ok(out)
    }
}

/// Feltnavnene i det uoffisielle API-et varierer — parse defensivt.
fn parse_position(pos: &Value) -> ExternalPosition {
    let instrument = pos.get("instrument").cloned().unwrap_or(Value::Null);
    let symbol = instrument
        .get("symbol")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string();
    let name = instrument
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(&symbol)
        .to_string();
    let qty = pos
        .get("qty")
        .or_else(|| pos.get("quantity"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let market_value = pos
        .pointer("/market_value_acc/value")
        .or_else(|| pos.pointer("/market_value/value"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    ExternalPosition { symbol, name, qty, market_value }
}
