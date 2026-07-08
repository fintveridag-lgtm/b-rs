use crate::types::{Candle, Quote};
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

    /// Daglige OHLC-stolper, eldst først. Brukes til å så strategien ved
    /// oppstart, som startdata for kursgrafen, og til backtesting.
    pub async fn history_daily(&self, symbol: &str, range: &str) -> Result<Vec<Candle>> {
        let result = self.chart(symbol, range, "1d").await?;
        parse_history(symbol, &result)
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

fn parse_history(symbol: &str, result: &Value) -> Result<Vec<Candle>> {
    let timestamps = result
        .pointer("/timestamp")
        .and_then(Value::as_array)
        .with_context(|| format!("mangler tidsstempler for {symbol}"))?;
    let quote = result
        .pointer("/indicators/quote/0")
        .with_context(|| format!("mangler historikk for {symbol}"))?;
    let series = |key: &str| quote.get(key).and_then(Value::as_array);
    let (Some(open), Some(high), Some(low), Some(close)) =
        (series("open"), series("high"), series("low"), series("close"))
    else {
        anyhow::bail!("mangler OHLC-serier for {symbol}");
    };
    // Yahoo bruker null for dager uten omsetning — hopp over dem.
    let mut out = Vec::with_capacity(timestamps.len());
    for i in 0..timestamps.len() {
        let value = |arr: &Vec<Value>| arr.get(i).and_then(Value::as_f64);
        if let (Some(ts), Some(o), Some(h), Some(l), Some(c)) = (
            timestamps.get(i).and_then(Value::as_i64),
            value(open),
            value(high),
            value(low),
            value(close),
        ) {
            out.push(Candle { ts: ts as f64, open: o, high: h, low: l, close: c });
        }
    }
    Ok(out)
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
    fn parses_history_and_skips_nulls() {
        let result = json!({
            "timestamp": [1700000000, 1700086400, 1700172800],
            "indicators": { "quote": [ {
                "open":  [ 99.0, null, 101.0],
                "high":  [101.0, null, 103.0],
                "low":   [ 98.0, null, 100.5],
                "close": [100.0, null, 102.0]
            } ] }
        });
        let candles = parse_history("EQNR.OL", &result).unwrap();
        assert_eq!(candles.len(), 2);
        assert_eq!(candles[0].ts, 1700000000.0);
        assert_eq!(candles[0].close, 100.0);
        assert_eq!(candles[1].high, 103.0);
    }

    #[test]
    fn missing_price_is_an_error() {
        let result = json!({ "meta": {} });
        assert!(parse_quote("X", &result).is_err());
    }
}
