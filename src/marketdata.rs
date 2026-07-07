use crate::types::Quote;
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::Value;

const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36";

/// Gratis kursdata fra Yahoo Finance. Oslo Børs-tickere har suffiks ".OL",
/// f.eks. "EQNR.OL". Data er forsinket (~15 min) — greit til papirhandel og
/// rolige strategier, ikke til høyfrekvent handel.
pub struct Yahoo {
    client: reqwest::Client,
}

impl Yahoo {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(UA)
            .timeout(std::time::Duration::from_secs(15))
            .build()?;
        Ok(Self { client })
    }

    async fn chart(&self, symbol: &str, range: &str, interval: &str) -> Result<Value> {
        let url = format!(
            "https://query1.finance.yahoo.com/v8/finance/chart/{symbol}?range={range}&interval={interval}"
        );
        let resp = self.client.get(&url).send().await?.error_for_status()?;
        let v: Value = resp.json().await?;
        if let Some(err) = v.pointer("/chart/error").filter(|e| !e.is_null()) {
            anyhow::bail!("Yahoo-feil for {symbol}: {err}");
        }
        v.pointer("/chart/result/0")
            .cloned()
            .with_context(|| format!("tomt Yahoo-svar for {symbol}"))
    }

    pub async fn quote(&self, symbol: &str) -> Result<Quote> {
        let result = self.chart(symbol, "1d", "5m").await?;
        parse_quote(symbol, &result)
    }

    /// Daglige sluttkurser, eldst først. Brukes til å så strategien ved oppstart.
    pub async fn history_daily(&self, symbol: &str, range: &str) -> Result<Vec<f64>> {
        let result = self.chart(symbol, range, "1d").await?;
        parse_closes(symbol, &result)
    }
}

fn parse_quote(symbol: &str, result: &Value) -> Result<Quote> {
    let meta = result
        .pointer("/meta")
        .with_context(|| format!("mangler meta for {symbol}"))?;
    let last = meta
        .get("regularMarketPrice")
        .and_then(Value::as_f64)
        .with_context(|| format!("mangler kurs for {symbol}"))?;
    let prev_close = meta
        .get("chartPreviousClose")
        .or_else(|| meta.get("previousClose"))
        .and_then(Value::as_f64)
        .unwrap_or(last);
    Ok(Quote {
        symbol: symbol.to_string(),
        last,
        prev_close,
        ts: Utc::now(),
    })
}

fn parse_closes(symbol: &str, result: &Value) -> Result<Vec<f64>> {
    let closes = result
        .pointer("/indicators/quote/0/close")
        .and_then(Value::as_array)
        .with_context(|| format!("mangler historikk for {symbol}"))?;
    // Yahoo bruker null for dager uten omsetning — hopp over dem.
    Ok(closes.iter().filter_map(Value::as_f64).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_quote_from_chart_result() {
        let result = json!({
            "meta": {
                "regularMarketPrice": 342.55,
                "chartPreviousClose": 339.10,
                "currency": "NOK"
            }
        });
        let q = parse_quote("EQNR.OL", &result).unwrap();
        assert_eq!(q.last, 342.55);
        assert_eq!(q.prev_close, 339.10);
        assert!((q.change_pct() - 1.0173).abs() < 0.01);
    }

    #[test]
    fn parses_closes_and_skips_nulls() {
        let result = json!({
            "indicators": { "quote": [ { "close": [100.0, null, 101.5, 102.0] } ] }
        });
        let closes = parse_closes("EQNR.OL", &result).unwrap();
        assert_eq!(closes, vec![100.0, 101.5, 102.0]);
    }

    #[test]
    fn missing_price_is_an_error() {
        let result = json!({ "meta": {} });
        assert!(parse_quote("X", &result).is_err());
    }
}
