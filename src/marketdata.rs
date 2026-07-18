use crate::types::{Candle, Quote};
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::Value;

const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36";

/// Gratis kursdata fra Yahoo Finance. Oslo Børs-tickere har suffiks ".OL",
/// f.eks. "EQNR.OL". Data er forsinket (~15 min) — greit til papirhandel og
/// rolige strategier, ikke til høyfrekvent handel.
///
/// Alle kall går gjennom en høflighetskø (minst 250 ms mellom forespørsler)
/// med ett gjenforsøk ved 429/serverfeil, så en stor watchlist ikke får
/// oss midlertidig blokkert hos Yahoo.
pub struct Yahoo {
    client: reqwest::Client,
    gate: tokio::sync::Mutex<std::time::Instant>,
}

impl Yahoo {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(UA)
            .timeout(std::time::Duration::from_secs(15))
            .build()?;
        Ok(Self {
            client,
            gate: tokio::sync::Mutex::new(std::time::Instant::now()),
        })
    }

    /// Felles inngang for alle Yahoo-kall: kø + backoff.
    async fn get_json(&self, url: &str) -> Result<Value> {
        {
            let mut last = self.gate.lock().await;
            let min_gap = std::time::Duration::from_millis(250);
            let elapsed = last.elapsed();
            if elapsed < min_gap {
                tokio::time::sleep(min_gap - elapsed).await;
            }
            *last = std::time::Instant::now();
        }
        let mut resp = self.client.get(url).send().await?;
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS || resp.status().is_server_error() {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            resp = self.client.get(url).send().await?;
        }
        let resp = resp.error_for_status()?;
        Ok(resp.json().await?)
    }

    async fn chart(&self, symbol: &str, range: &str, interval: &str) -> Result<Value> {
        let url = format!(
            "https://query1.finance.yahoo.com/v8/finance/chart/{symbol}?range={range}&interval={interval}"
        );
        let v = self.get_json(&url).await?;
        if let Some(err) = v.pointer("/chart/error").filter(|e| !e.is_null()) {
            anyhow::bail!("Yahoo-feil for {symbol}: {err}");
        }
        v.pointer("/chart/result/0")
            .cloned()
            .with_context(|| format!("tomt Yahoo-svar for {symbol}"))
    }

    pub async fn quote(&self, symbol: &str) -> Result<Quote> {
        // Krypto: hent ekte sanntidskurs fra Kraken (gratis, uten nøkkel).
        // Feiler det (ukjent par, nede), faller vi stille tilbake til Yahoo.
        if crate::types::is_crypto(symbol) {
            if let Ok(q) = self.kraken_quote(symbol).await {
                return Ok(q);
            }
        }
        let result = self.chart(symbol, "1d", "5m").await?;
        parse_quote(symbol, &result)
    }

    /// Sanntidskurs fra Krakens åpne ticker-API. "BTC-USD" → paret "XBTUSD"
    /// (Kraken kaller Bitcoin XBT); andre par brukes som de er uten bindestrek.
    async fn kraken_quote(&self, symbol: &str) -> Result<Quote> {
        let (base, quote_cur) = symbol.split_once('-').context("ikke et kryptopar")?;
        let pair = format!("{}{}", if base == "BTC" { "XBT" } else { base }, quote_cur);
        let url = format!("https://api.kraken.com/0/public/Ticker?pair={pair}");
        let v = self.get_json(&url).await?;
        parse_kraken_quote(symbol, quote_cur, &v)
    }

    /// Daglige OHLC-stolper, eldst først. Brukes til å så strategien ved
    /// oppstart, som startdata for kursgrafen, og til backtesting.
    pub async fn history_daily(&self, symbol: &str, range: &str) -> Result<Vec<Candle>> {
        let result = self.chart(symbol, range, "1d").await?;
        parse_history(symbol, &result)
    }

    /// Intradag-historikk: 5-minutterslys for de siste ~60 dagene (Yahoos
    /// maksgrense for 5m-oppløsning). Brukes til seeding og backtesting
    /// når strategien kjører med tidsramme.
    pub async fn history_intraday(&self, symbol: &str) -> Result<Vec<Candle>> {
        let result = self.chart(symbol, "60d", "5m").await?;
        parse_history(symbol, &result)
    }

    /// Søk etter aksjer/fond/krypto med fritekst («kongsberg») — returnerer
    /// (symbol, beskrivelse, kategori) der kategorien er norsk og klar for
    /// filtrering: "Aksje", "Fond/ETF", "Krypto" eller "Indeks".
    pub async fn search(&self, query: &str) -> Result<Vec<(String, String, String)>> {
        let mut out = self.search_once(query).await?;
        // Yahoo er kresen: «dnb teknologi a fond» matcher ofte ikke, mens
        // «dnb teknologi» gjør det. Prøv igjen uten fyllord ved null treff.
        if out.is_empty() {
            let renset: String = query
                .split_whitespace()
                .filter(|w| !matches!(w.to_lowercase().as_str(), "fond" | "fund" | "aksje" | "aksjer" | "etf"))
                .collect::<Vec<_>>()
                .join(" ");
            if !renset.is_empty() && renset != query {
                out = self.search_once(&renset).await?;
            }
        }
        Ok(out)
    }

    async fn search_once(&self, query: &str) -> Result<Vec<(String, String, String)>> {
        let url = reqwest::Url::parse_with_params(
            "https://query1.finance.yahoo.com/v1/finance/search",
            &[("q", query), ("quotesCount", "15"), ("newsCount", "0")],
        )?;
        let v = self.get_json(url.as_str()).await?;
        let mut out = Vec::new();
        for q in v.get("quotes").and_then(Value::as_array).cloned().unwrap_or_default() {
            let Some(symbol) = q.get("symbol").and_then(Value::as_str) else { continue };
            let kind = q.get("quoteType").and_then(Value::as_str).unwrap_or("");
            let kategori = match kind {
                "EQUITY" => "Aksje",
                "ETF" | "MUTUALFUND" => "Fond/ETF",
                "CRYPTOCURRENCY" => "Krypto",
                "INDEX" => "Indeks",
                _ => continue,
            };
            let name = q
                .get("shortname")
                .or_else(|| q.get("longname"))
                .and_then(Value::as_str)
                .unwrap_or(symbol);
            let exchange = q.get("exchDisp").and_then(Value::as_str).unwrap_or("");
            out.push((symbol.to_string(), format!("{name} ({exchange})"), kategori.to_string()));
        }
        Ok(out)
    }

    /// Siste nyhetssaker for et symbol fra Yahoo Finance, nyest først.
    pub async fn news(&self, symbol: &str) -> Result<Vec<crate::state::NewsItem>> {
        let url = reqwest::Url::parse_with_params(
            "https://query1.finance.yahoo.com/v1/finance/search",
            &[("q", symbol), ("quotesCount", "0"), ("newsCount", "8")],
        )?;
        let v = self.get_json(url.as_str()).await?;
        let mut out = Vec::new();
        for n in v.get("news").and_then(Value::as_array).cloned().unwrap_or_default() {
            let Some(title) = n.get("title").and_then(Value::as_str) else { continue };
            let Some(link) = n.get("link").and_then(Value::as_str) else { continue };
            out.push(crate::state::NewsItem {
                title: title.to_string(),
                publisher: n.get("publisher").and_then(Value::as_str).unwrap_or("").to_string(),
                url: link.to_string(),
                ts: n.get("providerPublishTime").and_then(Value::as_i64).unwrap_or(0),
            });
        }
        out.sort_by_key(|n| -n.ts);
        Ok(out)
    }

    /// Alt markedsskjermene trenger i ett kall: siste kurs, dagsvolum og
    /// tre måneder daglig historikk.
    pub async fn snapshot(&self, symbol: &str) -> Result<Snapshot> {
        let result = self.chart(symbol, "3mo", "1d").await?;
        let meta = result
            .pointer("/meta")
            .with_context(|| format!("mangler meta for {symbol}"))?;
        let last = meta
            .get("regularMarketPrice")
            .and_then(Value::as_f64)
            .with_context(|| format!("mangler kurs for {symbol}"))?;
        let volume = meta
            .get("regularMarketVolume")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let candles = parse_history(symbol, &result)?;
        Ok(Snapshot { last, volume, candles })
    }
}

pub struct Snapshot {
    pub last: f64,
    pub volume: f64,
    pub candles: Vec<Candle>,
}

impl Yahoo {
    /// Sum utbytte per aksje siste 12 måneder (0.0 hvis ingen).
    pub async fn dividends_12m(&self, symbol: &str) -> Result<f64> {
        let url = format!(
            "https://query1.finance.yahoo.com/v8/finance/chart/{symbol}?range=1y&interval=1d&events=div"
        );
        let v = self.get_json(&url).await?;
        let result = v
            .pointer("/chart/result/0")
            .cloned()
            .with_context(|| format!("tomt utbyttesvar for {symbol}"))?;
        Ok(parse_dividends(&result))
    }
}

fn parse_dividends(result: &Value) -> f64 {
    result
        .pointer("/events/dividends")
        .and_then(Value::as_object)
        .map(|m| {
            m.values()
                .filter_map(|d| d.get("amount").and_then(Value::as_f64))
                .sum()
        })
        .unwrap_or(0.0)
}

/// Tolk Krakens ticker-svar: "c" = [siste kurs, volum], "o" = dagens
/// åpningskurs (brukes som referanse for dagsendringen — krypto handles
/// døgnet rundt, så «forrige slutt» er uansett et valg).
fn parse_kraken_quote(symbol: &str, quote_cur: &str, v: &Value) -> Result<Quote> {
    let errors = v.get("error").and_then(Value::as_array).cloned().unwrap_or_default();
    anyhow::ensure!(errors.is_empty(), "Kraken: {errors:?}");
    let result = v
        .get("result")
        .and_then(Value::as_object)
        .context("uventet svar fra Kraken")?;
    let (_, t) = result.iter().next().context("tomt svar fra Kraken")?;
    let last: f64 = t
        .pointer("/c/0")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .with_context(|| format!("mangler siste kurs for {symbol}"))?;
    anyhow::ensure!(last > 0.0, "ugyldig kurs for {symbol}");
    let open: f64 = t
        .get("o")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .unwrap_or(last);
    Ok(Quote {
        symbol: symbol.to_string(),
        last,
        prev_close: open,
        currency: quote_cur.to_string(),
        ts: chrono::Utc::now(),
    })
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
    let currency = meta
        .get("currency")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok(Quote {
        symbol: symbol.to_string(),
        last,
        prev_close,
        currency,
        ts: Utc::now(),
    })
}

/// Er børsen dette symbolet handles på åpen akkurat nå? Krypto er alltid
/// åpent; ellers brukes børssuffikset og lokal børstid (man–fre).
pub fn is_trading_open(symbol: &str, now: chrono::DateTime<Utc>) -> bool {
    use chrono::{Datelike, Timelike, Weekday};
    if crate::types::is_crypto(symbol) {
        return true;
    }
    let (tz, open, close): (chrono_tz::Tz, (u32, u32), (u32, u32)) =
        match symbol.rsplit_once('.').map(|(_, suffix)| suffix) {
            Some("OL") => (chrono_tz::Europe::Oslo, (9, 0), (16, 30)),
            Some("DE") => (chrono_tz::Europe::Berlin, (9, 0), (17, 30)),
            Some("L") => (chrono_tz::Europe::London, (8, 0), (16, 30)),
            Some("ST") => (chrono_tz::Europe::Stockholm, (9, 0), (17, 30)),
            Some("CO") => (chrono_tz::Europe::Copenhagen, (9, 0), (17, 0)),
            // Uten suffiks: amerikansk børs.
            _ => (chrono_tz::America::New_York, (9, 30), (16, 0)),
        };
    let local = now.with_timezone(&tz);
    if matches!(local.weekday(), Weekday::Sat | Weekday::Sun) {
        return false;
    }
    let minutes = local.hour() * 60 + local.minute();
    minutes >= open.0 * 60 + open.1 && minutes < close.0 * 60 + close.1
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
    fn parses_kraken_ticker() {
        let v = json!({
            "error": [],
            "result": {
                "XXBTZUSD": {
                    "c": ["60123.4", "0.012"],
                    "o": "59800.0",
                    "h": ["60500.0", "60500.0"],
                }
            }
        });
        let q = parse_kraken_quote("BTC-USD", "USD", &v).unwrap();
        assert_eq!(q.symbol, "BTC-USD");
        assert_eq!(q.last, 60123.4);
        assert_eq!(q.prev_close, 59800.0);
        assert_eq!(q.currency, "USD");
        // Dagsendring mot åpning: (60123.4/59800 − 1) ≈ +0,54 %.
        assert!((q.change_pct() - 0.5408).abs() < 0.01);
    }

    #[test]
    fn kraken_errors_are_errors() {
        let v = json!({"error": ["EQuery:Unknown asset pair"], "result": {}});
        assert!(parse_kraken_quote("TULL-USD", "USD", &v).is_err());
        // Kurs på 0 skal aldri slippe gjennom.
        let v = json!({"error": [], "result": {"X": {"c": ["0.0", "1"]}}});
        assert!(parse_kraken_quote("BTC-USD", "USD", &v).is_err());
    }

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

    #[test]
    fn trading_hours_respect_exchange_and_weekends() {
        use chrono::TimeZone;
        // Onsdag 15. juli 2026 kl. 08:00 UTC = 10:00 i Oslo → åpent.
        let wed_open = chrono::Utc.with_ymd_and_hms(2026, 7, 15, 8, 0, 0).unwrap();
        assert!(is_trading_open("EQNR.OL", wed_open));
        // Samme onsdag kl. 20:00 UTC → stengt i Oslo.
        let wed_evening = chrono::Utc.with_ymd_and_hms(2026, 7, 15, 20, 0, 0).unwrap();
        assert!(!is_trading_open("EQNR.OL", wed_evening));
        // Lørdag → stengt.
        let saturday = chrono::Utc.with_ymd_and_hms(2026, 7, 18, 10, 0, 0).unwrap();
        assert!(!is_trading_open("EQNR.OL", saturday));
        // Krypto er alltid åpent — også lørdag kveld.
        assert!(is_trading_open("BTC-USD", saturday));
        // USA er stengt når Oslo har formiddag.
        assert!(!is_trading_open("AAPL", wed_open));
    }

    #[test]
    fn quote_parses_currency() {
        let result = json!({
            "meta": { "regularMarketPrice": 61000.0, "chartPreviousClose": 60000.0, "currency": "USD" }
        });
        let q = parse_quote("BTC-USD", &result).unwrap();
        assert_eq!(q.currency, "USD");
    }

    #[test]
    fn sums_dividends() {
        let result = json!({
            "events": { "dividends": {
                "1710000000": { "amount": 3.5, "date": 1710000000 },
                "1720000000": { "amount": 4.0, "date": 1720000000 }
            } }
        });
        assert!((parse_dividends(&result) - 7.5).abs() < 1e-9);
        assert_eq!(parse_dividends(&json!({})), 0.0);
    }
}
