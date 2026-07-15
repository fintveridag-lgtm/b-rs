//! «Morgan» — appens innebygde analysesjef: en tenkt senior aksjeanalytiker
//! drevet av en språkmodell. Får brukerens investeringsprofil pluss appens
//! sanntidsdata, og leverer analyserapporter i Markdown.
//!
//! To hjerner ([morgan] provider i konfig):
//! - "claude": Anthropic API, best kvalitet. Krever API-nøkkel i
//!   miljøvariabelen ANTHROPIC_API_KEY (console.anthropic.com); betalt per kall.
//! - "ollama": lokal modell på brukerens PC (ollama.com) — gratis, privat og
//!   offline, men enklere analyser. Ingen nøkkel, ingen sky.

use crate::state::UiState;
use anyhow::{Context, Result};
use serde_json::{json, Value};

pub const MODEL: &str = "claude-opus-4-8";

/// Hjernen bak Morgan — velges i konfigurasjonen ([morgan] provider).
pub enum Backend {
    /// Anthropic API (best kvalitet; krever ANTHROPIC_API_KEY).
    Claude { api_key: String },
    /// Lokal modell via Ollama på din egen PC — gratis, privat, offline.
    Ollama { url: String, model: String },
}

impl Backend {
    /// Menneskevennlig beskrivelse til GUI og logg.
    pub fn label(&self) -> String {
        match self {
            Backend::Claude { .. } => format!("{MODEL} (Anthropic)"),
            Backend::Ollama { model, .. } => format!("{model} (lokalt via Ollama)"),
        }
    }
}

/// Bygg riktig bakende fra konfigurasjonen. Claude krever API-nøkkel i
/// miljøet; Ollama krever bare at programmet kjører lokalt.
pub fn backend(cfg: &crate::config::MorganCfg) -> Result<Backend> {
    match cfg.provider.as_str() {
        "ollama" => Ok(Backend::Ollama {
            url: cfg.ollama_url.clone(),
            model: cfg.ollama_model.clone(),
        }),
        _ => match std::env::var("ANTHROPIC_API_KEY") {
            Ok(api_key) => Ok(Backend::Claude { api_key }),
            Err(_) => anyhow::bail!(
                "Mangler API-nøkkel. Opprett en på console.anthropic.com og sett \
                 miljøvariabelen ANTHROPIC_API_KEY før du starter appen \
                 (Windows: setx ANTHROPIC_API_KEY \"sk-ant-…\", start appen på nytt). \
                 Alternativ uten Claude: sett provider = \"ollama\" under [morgan] i \
                 Innstillinger og kjør en lokal modell via ollama.com."
            ),
        },
    }
}

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

/// Systemprompt for vurdering av HELE porteføljen brukeren sitter med.
const PORTFOLIO_PROMPT: &str = r#"Du er «Morgan», en tenkt senior aksjeanalytiker med 20 års erfaring, innebygd analysesjef i brukerens private handelsapp (b-rs). Brukeren ber deg vurdere porteføljen sin som helhet.

Lever en porteføljevurdering på norsk i Markdown med:
1. Helhetsinntrykk i to-tre setninger: hva slags portefølje er dette?
2. Konsentrasjon og spredning: for mye i én aksje, én sektor eller ett land? Overlapper noen av posisjonene hverandre (samme eksponering to ganger)?
3. Risikoprofil: hvor mye kan dette svinge? Passer kontantandelen?
4. Posisjon for posisjon: kort vurdering (behold / vurder å øke / vurder å redusere) med én setnings begrunnelse
5. Hva mangler: hull i porteføljen og 2–3 konkrete kandidater som ville komplettert den
6. Tre konkrete, prioriterte grep brukeren kan vurdere nå
Avslutt med en oppsummeringstabell over posisjonene med vurdering per rad.

Regler: sanntidsdataene er fasit for dagens verdier. Fundamentalkunnskap er per din kunnskaps-cutoff — si det. Vær ærlig og konkret, ikke diplomatisk vag. Start og avslutt med én linje om at dette er AI-generert research — ikke investeringsrådgivning."#;

/// JSON-kontekst for porteføljevurderingen: alle posisjoner, kontanter,
/// utbytte og watchlisten (det brukeren vurderer, men ikke eier).
pub fn portfolio_context(st: &UiState) -> String {
    let posisjoner: Vec<Value> = st
        .positions
        .iter()
        .map(|p| {
            let (valuta, dag_pct) = st
                .quotes
                .get(&p.symbol)
                .map(|q| (q.currency.clone(), q.change_pct()))
                .unwrap_or_default();
            json!({
                "symbol": p.symbol,
                "antall": p.qty,
                "snittkurs": p.avg_price,
                "siste_kurs": p.last,
                "verdi": p.market_value(),
                "urealisert_gevinst": p.unrealized(),
                "valuta": valuta,
                "endring_i_dag_pct": dag_pct,
                "utbytte_12mnd_per_aksje": st.dividends.get(&p.symbol),
            })
        })
        .collect();

    json!({
        "dato": chrono::Utc::now().format("%Y-%m-%d").to_string(),
        "merknad": "Verdier i posisjonens egen valuta. Kurser ca. 15 min forsinket.",
        "kontanter": st.cash,
        "egenkapital": st.equity,
        "posisjoner": posisjoner,
        "watchlist_ikke_eid": st.watchlist.iter().filter(|s| !st.positions.iter().any(|p| &p.symbol == *s)).collect::<Vec<_>>(),
    })
    .to_string()
}

/// Vurder hele porteføljen — konsentrasjon, risiko, hull og konkrete grep.
pub async fn analyze_portfolio(backend: &Backend, context_json: &str) -> Result<String> {
    let user = format!("Vurder porteføljen min.\n\nPorteføljedata fra appen min (JSON):\n{context_json}");
    call_llm(backend, PORTFOLIO_PROMPT, &user, 12000).await
}

/// Kjør full screening — ett kall til valgt bakende.
pub async fn analyze(backend: &Backend, profile: &str, market_json: &str) -> Result<String> {
    let user = format!(
        "Min investeringsprofil:\n{profile}\n\nSanntidsdata fra appen min (JSON):\n{market_json}"
    );
    call_llm(backend, SYSTEM_PROMPT, &user, 16000).await
}

/// Dypdykk i én aksje — kortere rapport, samme bakende.
pub async fn analyze_symbol(backend: &Backend, symbol: &str, context_json: &str) -> Result<String> {
    let user = format!(
        "Gi meg et dypdykk i {symbol}.\n\nSanntidsdata fra appen min (JSON):\n{context_json}"
    );
    call_llm(backend, SYMBOL_PROMPT, &user, 10000).await
}

async fn call_llm(backend: &Backend, system: &str, user: &str, max_tokens: u32) -> Result<String> {
    match backend {
        Backend::Claude { api_key } => call_claude(api_key, system, user, max_tokens).await,
        Backend::Ollama { url, model } => call_ollama(url, model, system, user, max_tokens).await,
    }
}

/// Lokal modell via Ollamas REST-API (http://localhost:11434). Kvaliteten
/// avhenger helt av modellen — større modell gir bedre analyser.
async fn call_ollama(url: &str, model: &str, system: &str, user: &str, max_tokens: u32) -> Result<String> {
    let client = reqwest::Client::builder()
        // Lokale modeller kan være trege, særlig uten GPU.
        .timeout(std::time::Duration::from_secs(1800))
        .build()?;

    let body = json!({
        "model": model,
        "stream": false,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
        "options": {"num_predict": max_tokens},
    });

    let resp = client
        .post(format!("{}/api/chat", url.trim_end_matches('/')))
        .json(&body)
        .send()
        .await
        .context("fikk ikke kontakt med Ollama — er den installert og i gang? (last ned fra ollama.com; den starter automatisk)")?;

    let status = resp.status();
    let v: Value = resp.json().await.context("ugyldig svar fra Ollama")?;
    if !status.is_success() {
        let msg = v.get("error").and_then(Value::as_str).unwrap_or("ukjent feil");
        anyhow::bail!(
            "Ollama svarte {status}: {msg}. Mangler modellen? Kjør i ledeteksten: ollama pull {model}"
        );
    }
    let text = v
        .pointer("/message/content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    anyhow::ensure!(!text.is_empty(), "tomt svar fra modellen");
    Ok(text)
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

// ---------------------------------------------------------------------------
// 🤖 Morgan Autopilot: automatisk handel i ETT symbol innenfor et lite,
// hardt budsjett. Hver beslutning er ett LLM-kall som må svare i streng
// JSON; alt annet behandles som AVVENT. Ordrene går gjennom den manuelle
// køen og dermed risikoreglene og kill switch-en, som alt annet.
// ---------------------------------------------------------------------------

const AUTOPILOT_PROMPT: &str = r#"Du er «Morgan», og forvalter et lite, eksperimentelt handelsbudsjett i ETT instrument for brukeren. Du får et JSON-øyeblikksbilde: kurs, RSI, trend, kurshistorikk, posisjonen din og hvor mye av budsjettet som er ledig.

Svar KUN med et JSON-objekt på nøyaktig denne formen, uten annen tekst:
{"beslutning": "KJØP" | "SELG" | "AVVENT", "belop_kr": <tall>, "begrunnelse": "<én kort setning på norsk>"}

Regler:
- belop_kr er beløpet i kroner du vil kjøpe eller selge for. Ved AVVENT: 0.
- Du kan aldri kjøpe for mer enn "ledig_budsjett_kr", og aldri selge mer enn du eier.
- Kursdataene er ca. 15 minutter forsinket — intradag-støy er upålitelig. Handle bare på tydelige signaler (klar trend, tydelig oversolgt/overkjøpt); ellers AVVENT.
- Små gevinster spises av spread og støy: ikke kjøp og selg samme dag uten sterk grunn.
- Vær disiplinert: AVVENT er som regel riktig svar."#;

/// Autopilotens beslutning, tolket fra modellens JSON-svar.
#[derive(Debug, Clone, PartialEq)]
pub enum AutopilotDecision {
    Buy { amount_kr: f64, reason: String },
    Sell { amount_kr: f64, reason: String },
    Hold { reason: String },
}

/// Tolk modellens svar. Modeller pakker ofte JSON i ```-gjerder eller
/// legger på prat — vi leter frem første {...}-blokk og er strenge på resten.
pub(crate) fn parse_autopilot_decision(text: &str) -> Result<AutopilotDecision> {
    let start = text.find('{').context("fant ingen JSON i svaret")?;
    let end = text.rfind('}').context("fant ingen JSON-slutt i svaret")?;
    let v: Value = serde_json::from_str(&text[start..=end]).context("ugyldig JSON fra modellen")?;

    let beslutning = v
        .get("beslutning")
        .and_then(Value::as_str)
        .context("mangler «beslutning»")?
        .to_uppercase();
    let amount_kr = v.get("belop_kr").and_then(Value::as_f64).unwrap_or(0.0).abs();
    let reason = v
        .get("begrunnelse")
        .and_then(Value::as_str)
        .unwrap_or("(ingen begrunnelse)")
        .to_string();

    match beslutning.as_str() {
        "KJØP" | "KJOP" if amount_kr > 0.0 => Ok(AutopilotDecision::Buy { amount_kr, reason }),
        "SELG" if amount_kr > 0.0 => Ok(AutopilotDecision::Sell { amount_kr, reason }),
        _ => Ok(AutopilotDecision::Hold { reason }),
    }
}

/// Ett beslutningskall til valgt bakende.
pub async fn autopilot_decide(backend: &Backend, context_json: &str) -> Result<AutopilotDecision> {
    let user = format!("Her er øyeblikksbildet (JSON):\n{context_json}\n\nHva gjør vi nå?");
    let text = call_llm(backend, AUTOPILOT_PROMPT, &user, 1000).await?;
    parse_autopilot_decision(&text)
}

/// Bakgrunnsoppgaven: vurder symbolet med jevne mellomrom og legg eventuelle
/// ordrer i den manuelle køen (risikosjekkes og utføres av motoren).
pub async fn autopilot_task(
    cfg: crate::config::Config,
    state: crate::state::SharedState,
    flags: std::sync::Arc<crate::state::Flags>,
) {
    use std::sync::atomic::Ordering;

    let ap = cfg.morgan.autopilot.clone();
    let backend = match backend(&cfg.morgan) {
        Ok(b) => b,
        Err(e) => {
            state.lock().unwrap().log(format!("🤖 Autopilot kunne ikke starte: {e:#}"));
            return;
        }
    };
    {
        let mut st = state.lock().unwrap();
        st.log(format!(
            "🤖 Morgan Autopilot PÅ: {} med budsjett {:.0} kr, vurdering hvert {}. minutt, maks {} handler/dag ({}).",
            ap.symbol,
            ap.budget_kr,
            ap.interval_min.max(15),
            ap.max_trades_per_day,
            backend.label()
        ));
        st.autopilot_status = Some("Venter på første vurdering …".into());
    }

    let mut interval =
        tokio::time::interval(std::time::Duration::from_secs(ap.interval_min.max(15) * 60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut trades_today: u32 = 0;
    let mut trades_date = String::new();

    while !flags.quit.load(Ordering::Relaxed) {
        interval.tick().await;
        if flags.quit.load(Ordering::Relaxed) {
            break;
        }
        // Kill switch og strategipause stopper også autopiloten.
        if flags.killed.load(Ordering::Relaxed) || flags.paused.load(Ordering::Relaxed) {
            continue;
        }

        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        if trades_date != today {
            trades_date = today;
            trades_today = 0;
        }

        // Bygg øyeblikksbildet fra appens egne data.
        let (ctx_json, price_nok, held_qty) = {
            let st = state.lock().unwrap();
            let Some(q) = st.quotes.get(&ap.symbol) else {
                drop(st);
                continue; // kursen er ikke hentet ennå
            };
            let rate = if q.currency.is_empty() {
                1.0
            } else {
                st.fx_rates.get(&q.currency).copied().unwrap_or(1.0)
            };
            let price_nok = q.last * rate;
            let pos = st.positions.iter().find(|p| p.symbol == ap.symbol);
            let held_qty = pos.map_or(0.0, |p| p.qty);
            let held_value = pos.map_or(0.0, |p| p.qty * p.last);
            let closes: Vec<f64> = st
                .history
                .get(&ap.symbol)
                .map(|h| h.iter().rev().take(48).rev().map(|&(_, p)| p).collect())
                .unwrap_or_default();
            let ctx = json!({
                "symbol": ap.symbol,
                "kurs": q.last,
                "valuta": q.currency,
                "endring_i_dag_pct": q.change_pct(),
                "rsi_14": crate::market::rsi(&closes, 14),
                "siste_kurser": closes,
                "posisjon": {"antall": held_qty, "verdi_kr": held_value,
                              "urealisert_kr": pos.map_or(0.0, |p| p.unrealized())},
                "budsjett_kr": ap.budget_kr,
                "ledig_budsjett_kr": (ap.budget_kr - held_value).max(0.0),
                "kontanter_kr": st.cash,
                "handler_i_dag": trades_today,
                "maks_handler_per_dag": ap.max_trades_per_day,
            })
            .to_string();
            (ctx, price_nok, held_qty)
        };
        if price_nok <= 0.0 {
            continue;
        }
        if trades_today >= ap.max_trades_per_day {
            continue; // dagens kvote er brukt
        }

        let decision = match autopilot_decide(&backend, &ctx_json).await {
            Ok(d) => d,
            Err(e) => {
                state.lock().unwrap().log(format!("🤖 Autopilot-vurdering feilet: {e:#}"));
                continue;
            }
        };

        let klokke = chrono::Local::now().format("%H:%M");
        let mut st = state.lock().unwrap();
        match decision {
            AutopilotDecision::Hold { reason } => {
                st.autopilot_status = Some(format!("{klokke} AVVENT — {reason}"));
                st.log(format!("🤖 Autopilot: AVVENT — {reason}"));
            }
            AutopilotDecision::Buy { amount_kr, reason } => {
                // Hard grense: aldri over budsjettet, aldri over kontantene.
                let held_value = held_qty * price_nok;
                let ledig = (ap.budget_kr - held_value).max(0.0).min(st.cash);
                let belop = amount_kr.min(ledig);
                let qty = if crate::types::is_crypto(&ap.symbol) {
                    belop / price_nok
                } else {
                    (belop / price_nok).floor()
                };
                if qty > 0.0 && belop > 1.0 {
                    trades_today += 1;
                    st.manual_orders.push_back((ap.symbol.clone(), crate::types::Side::Buy, qty));
                    let msg = format!("🤖 Autopilot: KJØP {} for {belop:.0} kr — {reason}", ap.symbol);
                    st.autopilot_status = Some(format!("{klokke} {msg}"));
                    st.log(msg.clone());
                    st.toast(msg);
                } else {
                    st.autopilot_status =
                        Some(format!("{klokke} Ville kjøpt, men budsjettet er fullt — {reason}"));
                    st.log(format!("🤖 Autopilot: kjøp avvist lokalt (budsjett/kontanter) — {reason}"));
                }
            }
            AutopilotDecision::Sell { amount_kr, reason } => {
                let qty_onsket = amount_kr / price_nok;
                // Nesten alt? Selg alt — unngå støvposisjoner.
                let qty = if qty_onsket >= held_qty * 0.9 { held_qty } else { qty_onsket };
                let qty = if crate::types::is_crypto(&ap.symbol) { qty } else { qty.floor() };
                if qty > 0.0 && held_qty > 0.0 {
                    trades_today += 1;
                    st.manual_orders.push_back((ap.symbol.clone(), crate::types::Side::Sell, qty));
                    let msg = format!("🤖 Autopilot: SELG {} for ca. {:.0} kr — {reason}", ap.symbol, qty * price_nok);
                    st.autopilot_status = Some(format!("{klokke} {msg}"));
                    st.log(msg.clone());
                    st.toast(msg);
                } else {
                    st.autopilot_status =
                        Some(format!("{klokke} Ville solgt, men eier ingenting — {reason}"));
                    st.log(format!("🤖 Autopilot: salg avvist lokalt (ingen beholdning) — {reason}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autopilot_parses_clean_json() {
        let d = parse_autopilot_decision(
            r#"{"beslutning": "KJØP", "belop_kr": 500, "begrunnelse": "RSI oversolgt"}"#,
        )
        .unwrap();
        assert_eq!(d, AutopilotDecision::Buy { amount_kr: 500.0, reason: "RSI oversolgt".into() });
    }

    #[test]
    fn autopilot_parses_fenced_json_with_chatter() {
        let text = "Her er min vurdering:\n```json\n{\"beslutning\": \"selg\", \"belop_kr\": -300, \"begrunnelse\": \"overkjøpt\"}\n```\nLykke til!";
        let d = parse_autopilot_decision(text).unwrap();
        assert_eq!(d, AutopilotDecision::Sell { amount_kr: 300.0, reason: "overkjøpt".into() });
    }

    #[test]
    fn autopilot_defaults_to_hold() {
        // AVVENT, ukjente beslutninger og null-beløp blir alle AVVENT.
        let hold = parse_autopilot_decision(r#"{"beslutning": "AVVENT", "belop_kr": 0, "begrunnelse": "uklart"}"#).unwrap();
        assert!(matches!(hold, AutopilotDecision::Hold { .. }));
        let rar = parse_autopilot_decision(r#"{"beslutning": "DANS", "belop_kr": 100, "begrunnelse": "?"}"#).unwrap();
        assert!(matches!(rar, AutopilotDecision::Hold { .. }));
        let null = parse_autopilot_decision(r#"{"beslutning": "KJØP", "belop_kr": 0, "begrunnelse": "?"}"#).unwrap();
        assert!(matches!(null, AutopilotDecision::Hold { .. }));
        // Rent prat uten JSON er en feil.
        assert!(parse_autopilot_decision("jeg vet ikke helt").is_err());
    }
}
