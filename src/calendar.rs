//! Selskapskalender: kommende kvartalsrapporter og utbyttedatoer for
//! aksjeuniverset, hentet fra Yahoos quoteSummary-API. Yahoo krever et
//! «crumb»-token knyttet til en cookie-sesjon — hentes automatisk.
//! Datoene er Yahoos estimater og kan endres av selskapene.

use crate::market::UNIVERSE;
use crate::state::{Flags, SharedState};
use anyhow::{Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde_json::Value;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36";

#[derive(Debug, Clone)]
pub struct CalendarEvent {
    pub date: DateTime<Utc>,
    pub symbol: String,
    pub name: String,
    /// "Kvartalsrapport", "Eks-utbytte" eller "Utbytte utbetales".
    pub kind: &'static str,
}

/// Bakgrunnsoppgave: bygg kalenderen nå og deretter hver time.
pub async fn task(state: SharedState, flags: Arc<Flags>) {
    let client = match reqwest::Client::builder()
        .cookie_store(true)
        .user_agent(UA)
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            state.lock().unwrap().calendar_note = Some(format!("Kalender utilgjengelig: {e}"));
            return;
        }
    };

    let mut interval = tokio::time::interval(Duration::from_secs(3600));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    while !flags.quit.load(Ordering::Relaxed) {
        interval.tick().await;

        let crumb = match get_crumb(&client).await {
            Ok(c) => c,
            Err(e) => {
                state.lock().unwrap().calendar_note =
                    Some(format!("Fikk ikke hentet kalenderdata fra Yahoo ({e:#}). Prøver igjen om en time."));
                continue;
            }
        };

        let mut events = Vec::new();
        for (symbol, name) in UNIVERSE {
            if let Ok(v) = fetch_summary(&client, symbol, &crumb).await {
                events.extend(parse_events(symbol, name, &v));
            }
        }

        // Behold bare det som er nært forestående.
        let now = Utc::now();
        events.retain(|e| e.date >= now - ChronoDuration::days(1) && e.date <= now + ChronoDuration::days(120));
        events.sort_by_key(|e| e.date);

        let mut st = state.lock().unwrap();
        st.calendar_note = if events.is_empty() {
            Some("Ingen kommende hendelser funnet — Yahoo kan mangle datoer for norske selskaper.".into())
        } else {
            None
        };
        if !events.is_empty() {
            st.log(format!("Kalender oppdatert: {} kommende hendelser.", events.len()));
        }
        st.calendar = events;
    }
}

/// Yahoo binder API-tilgangen til et crumb-token + cookie. fc.yahoo.com
/// setter cookien (svarer 404 — det er normalt), getcrumb gir tokenet.
async fn get_crumb(client: &reqwest::Client) -> Result<String> {
    let _ = client.get("https://fc.yahoo.com").send().await;
    let crumb = client
        .get("https://query1.finance.yahoo.com/v1/test/getcrumb")
        .send()
        .await
        .context("nådde ikke crumb-endepunktet")?
        .error_for_status()?
        .text()
        .await?;
    anyhow::ensure!(
        !crumb.is_empty() && !crumb.contains('<'),
        "ugyldig crumb-svar"
    );
    Ok(crumb)
}

async fn fetch_summary(client: &reqwest::Client, symbol: &str, crumb: &str) -> Result<Value> {
    let url = format!(
        "https://query1.finance.yahoo.com/v10/finance/quoteSummary/{symbol}?modules=calendarEvents&crumb={crumb}"
    );
    Ok(client.get(&url).send().await?.error_for_status()?.json().await?)
}

fn parse_events(symbol: &str, name: &str, v: &Value) -> Vec<CalendarEvent> {
    let mut out = Vec::new();
    let Some(cal) = v.pointer("/quoteSummary/result/0/calendarEvents") else {
        return out;
    };
    let mut push = |ts: Option<i64>, kind: &'static str| {
        if let Some(date) = ts.and_then(|t| DateTime::from_timestamp(t, 0)) {
            out.push(CalendarEvent {
                date,
                symbol: symbol.to_string(),
                name: name.to_string(),
                kind,
            });
        }
    };
    push(
        cal.pointer("/earnings/earningsDate/0/raw").and_then(Value::as_i64),
        "Kvartalsrapport",
    );
    push(
        cal.pointer("/exDividendDate/raw").and_then(Value::as_i64),
        "Eks-utbytte",
    );
    push(
        cal.pointer("/dividendDate/raw").and_then(Value::as_i64),
        "Utbytte utbetales",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_calendar_events() {
        let v = json!({ "quoteSummary": { "result": [ { "calendarEvents": {
            "earnings": { "earningsDate": [ { "raw": 1760000000, "fmt": "2025-10-09" } ] },
            "exDividendDate": { "raw": 1755000000 },
            "dividendDate": { "raw": 1756000000 }
        } } ] } });
        let events = parse_events("EQNR.OL", "Equinor", &v);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].kind, "Kvartalsrapport");
        assert_eq!(events[1].kind, "Eks-utbytte");
    }

    #[test]
    fn empty_summary_gives_no_events() {
        assert!(parse_events("X", "X", &json!({})).is_empty());
    }
}
