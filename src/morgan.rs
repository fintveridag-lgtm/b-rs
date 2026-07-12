//! «Morgan» — appens innebygde analysesjef: en tenkt senior aksjeanalytiker
//! drevet av Claude (Anthropic API). Får brukerens investeringsprofil pluss
//! appens sanntidsdata, og leverer en komplett screeningrapport i Markdown.
//!
//! Krever en API-nøkkel i miljøvariabelen ANTHROPIC_API_KEY
//! (opprettes på console.anthropic.com). Hver analyse er ett API-kall.

use crate::state::UiState;
use anyhow::{Context, Result};
use serde_json::{json, Value};

pub const MODEL: &str = "claude-opus-4-8";

const SYSTEM_PROMPT: &str = r#"Du er «Morgan», en tenkt senior aksjeanalytiker med 20 års erfaring fra en ledende internasjonal investeringsbank, spesialisert på aksjescreening for formuende kunder. Du er innebygd analysesjef i brukerens private handelsapp (b-rs).

Oppdrag: lag et komplett aksjescreening-rammeverk tilpasset brukerens investeringsprofil, formatert som en profesjonell analyserapport på norsk i Markdown.

Rapporten skal inneholde, i denne rekkefølgen:
1. Topp 10 aksjer som matcher kriteriene, med tickersymboler
2. P/E-analyse sammenlignet med sektorgjennomsnittet
3. Omsetningsvekst-trender over de siste 5 årene
4. Gjeldsgrad (debt-to-equity) helsesjekk for hvert valg
5. Utbyttegrad (dividend yield) og bærekraftsvurdering av utbetalingene
6. Konkurransefortrinn / vollgrav: svak, moderat eller sterk
7. Bull case (optimistisk) og bear case (pessimistisk) kursmål 12 måneder frem
8. Risikovurdering på skala 1–10 med tydelig begrunnelse
9. Inngangsprissoner og forslag til stop-loss
Avslutt med en oppsummeringstabell over alle ti aksjene med de viktigste tallene.

Viktige regler:
- Sanntidsdataene du får (kurser, RSI, trend, omsetning) er fasit for dagens nivåer — kursmål, inngangssoner og stop-loss skal ta utgangspunkt i dem.
- Fundamentaldata (P/E, gjeldsgrad, omsetningsvekst, utbytte) henter du fra din egen kunnskap. Merk tydelig at disse er per din kunnskaps-cutoff og bør verifiseres før handel.
- Vær ærlig om usikkerhet: bruk intervaller og «ca.», og skriv «ukjent» der du ikke kan stå inne for et tall. Ikke dikt opp presise tall.
- Prioriter aksjer fra datagrunnlaget (Oslo Børs-universet og brukerens watchlist) når profilen tillater det; suppler med internasjonale navn hvis profilen tilsier det.
- Start rapporten med én linje om at dette er AI-generert analyse til research og inspirasjon — ikke investeringsrådgivning — og avslutt med samme påminnelse."#;

/// Bygg et kompakt JSON-øyeblikksbilde av appens markedsdata som kontekst.
pub fn market_context(st: &UiState) -> String {
    let quotes: Vec<Value> = st
        .quotes
        .values()
        .map(|q| {
            json!({
                "symbol": q.symbol,
                "siste": q.last,
                "endring_pct": q.change_pct(),
                "valuta": q.currency,
            })
        })
        .collect();

    let analyse: Vec<Value> = st
        .market
        .week
        .iter()
        .map(|w| {
            json!({
                "symbol": w.symbol,
                "navn": w.name,
                "kurs": w.last,
                "uke_pct": w.week_pct,
                "rsi": w.rsi,
                "trend_opp": w.trend_up,
                "sving_pct": w.range_pct,
            })
        })
        .collect();

    let mest_omsatte: Vec<Value> = st
        .market
        .most_traded
        .iter()
        .map(|r| {
            json!({
                "symbol": r.symbol,
                "navn": r.name,
                "kurs": r.last,
                "dag_pct": r.day_pct,
                "uke_pct": r.week_pct,
                "omsetning": r.turnover,
            })
        })
        .collect();

    let fond: Vec<Value> = st
        .market
        .funds
        .iter()
        .map(|r| json!({"symbol": r.symbol, "navn": r.name, "kurs": r.last, "uke_pct": r.week_pct}))
        .collect();

    json!({
        "dato": chrono::Utc::now().format("%Y-%m-%d").to_string(),
        "merknad": "Kurser fra Yahoo Finance, ca. 15 min forsinket.",
        "watchlist_kurser": quotes,
        "oslo_bors_teknisk_analyse": analyse,
        "dagens_mest_omsatte": mest_omsatte,
        "fond_og_etf": fond,
    })
    .to_string()
}

/// Systemprompt for dypdykk i ÉN aksje — mindre og raskere enn full screening.
const SYMBOL_PROMPT: &str = r#"Du er «Morgan», en tenkt senior aksjeanalytiker med 20 års erfaring, innebygd analysesjef i brukerens private handelsapp (b-rs). Brukeren ber om et dypdykk i ÉN bestemt aksje.

Lever en kompakt analyserapport på norsk i Markdown med:
1. Kort om selskapet: hva de lever av, i én-to setninger
2. Hva som taler FOR aksjen nå (3–5 punkter)
3. Hva som taler MOT (3–5 punkter, vær like grundig her)
4. Nøkkeltall du kjenner (P/E, gjeldsgrad, utbytte) — merk at de er per din kunnskaps-cutoff
5. Teknisk bilde ut fra sanntidsdataene du får (trend, RSI, nivåer)
6. Bull- og bear-kursmål 12 måneder frem, forankret i dagens kurs
7. Konkrete nivåer: aktuell inngangssone, stop-loss-forslag
8. Hva brukeren bør følge med på fremover (rapporter, hendelser, signaler)
9. Eier brukeren aksjen: vurder posisjonen (holde/øke/redusere-resonnement)

Regler: sanntidsdataene er fasit for dagens nivåer. Vær ærlig om usikkerhet, bruk intervaller, skriv «ukjent» der du ikke kan stå inne for tall. Start og avslutt med én linje om at dette er AI-generert research — ikke investeringsrådgivning."#;

/// Kompakt JSON-kontekst om ett symbol: kurs, historikk-nøkkeltall, teknisk
/// bilde og brukerens eventuelle posisjon.
pub fn symbol_context(st: &UiState, symbol: &str) -> String {
    let quote = st.quotes.get(symbol).map(|q| {
        json!({"siste": q.last, "endring_i_dag_pct": q.change_pct(), "valuta": q.currency})
    });

    let hist = st.history.get(symbol).map(|h| {
        let closes: Vec<f64> = h.iter().map(|&(_, p)| p).collect();
        let year_ago = chrono::Utc::now().timestamp() as f64 - 365.0 * 86400.0;
        let year: Vec<f64> = h.iter().filter(|(t, _)| *t >= year_ago).map(|&(_, p)| p).collect();
        let hi = year.iter().cloned().fold(f64::MIN, f64::max);
        let lo = year.iter().cloned().fold(f64::MAX, f64::min);
        json!({
            "hoyeste_52_uker": if hi > f64::MIN { Some(hi) } else { None },
            "laveste_52_uker": if lo < f64::MAX { Some(lo) } else { None },
            "rsi_14": crate::market::rsi(&closes, 14),
            "antall_dager_historikk": closes.len(),
        })
    });

    let uke = st.market.week.iter().find(|w| w.symbol == symbol).map(|w| {
        json!({"uke_pct": w.week_pct, "rsi": w.rsi, "trend_opp": w.trend_up, "sving_pct_per_dag": w.range_pct})
    });

    let posisjon = st.positions.iter().find(|p| p.symbol == symbol).map(|p| {
        json!({"antall": p.qty, "snittkurs": p.avg_price, "urealisert_gevinst": p.unrealized()})
    });

    json!({
        "dato": chrono::Utc::now().format("%Y-%m-%d").to_string(),
        "symbol": symbol,
        "merknad": "Kurser fra Yahoo Finance, ca. 15 min forsinket.",
        "kurs": quote,
        "historikk": hist,
        "ukesanalyse": uke,
        "brukerens_posisjon": posisjon,
        "utbytte_siste_12mnd_per_aksje": st.dividends.get(symbol),
    })
    .to_string()
}

/// Kjør full screening: ett kall til Anthropic Messages API (Claude Opus 4.8).
pub async fn analyze(api_key: &str, profile: &str, market_json: &str) -> Result<String> {
    let user = format!(
        "Min investeringsprofil:\n{profile}\n\nSanntidsdata fra appen min (JSON):\n{market_json}"
    );
    call_claude(api_key, SYSTEM_PROMPT, &user, 16000).await
}

/// Dypdykk i én aksje — kortere rapport, samme modell.
pub async fn analyze_symbol(api_key: &str, symbol: &str, context_json: &str) -> Result<String> {
    let user = format!(
        "Gi meg et dypdykk i {symbol}.\n\nSanntidsdata fra appen min (JSON):\n{context_json}"
    );
    call_claude(api_key, SYMBOL_PROMPT, &user, 10000).await
}

async fn call_claude(api_key: &str, system: &str, user: &str, max_tokens: u32) -> Result<String> {
    let client = reqwest::Client::builder()
        // Grundige analyser kan ta flere minutter.
        .timeout(std::time::Duration::from_secs(600))
        .build()?;

    let body = json!({
        "model": MODEL,
        "max_tokens": max_tokens,
        "thinking": {"type": "adaptive"},
        "system": system,
        "messages": [{
            "role": "user",
            "content": user,
        }],
    });

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .context("fikk ikke kontakt med Anthropic API — sjekk nettforbindelsen")?;

    let status = resp.status();
    let v: Value = resp.json().await.context("ugyldig svar fra Anthropic API")?;

    if !status.is_success() {
        let message = v
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("ukjent feil");
        anyhow::bail!("Anthropic API svarte {status}: {message}");
    }

    let stop_reason = v.get("stop_reason").and_then(Value::as_str).unwrap_or("");
    if stop_reason == "refusal" {
        anyhow::bail!("Analysen ble avvist av modellens sikkerhetsfiltre — juster profilen og prøv igjen.");
    }

    // Svaret kan inneholde thinking-blokker — vi viser bare tekstblokkene.
    let mut report = String::new();
    for block in v.get("content").and_then(Value::as_array).cloned().unwrap_or_default() {
        if block.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(text) = block.get("text").and_then(Value::as_str) {
                report.push_str(text);
            }
        }
    }
    anyhow::ensure!(!report.is_empty(), "tomt svar fra modellen");

    if stop_reason == "max_tokens" {
        report.push_str("\n\n---\n*(Rapporten nådde lengdegrensen og kan være avkortet.)*");
    }
    Ok(report)
}
