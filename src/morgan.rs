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

/// «Rådslaget»: Morgan og Stanley diskuterer seg imellom. Stanley er den
/// rolige, skeptiske risiko- og livsveilederen; Morgan den offensive
/// markedsjegeren. De ender i en felles anbefaling + et livsråd til brukeren.
const COUNCIL_PROMPT: &str = r#"Du skriver et daglig «rådslag» mellom to tenkte analysesjefer i brukerens handelsapp (b-rs):

- MORGAN: senior aksjeanalytiker, offensiv, ser muligheter, elsker markedet.
- STANLEY: Morgans rolige makker — skeptisk risikovokter og livsveileder. Han gransker hva Morgan (og daytraderen) har gjort, utfordrer Morgan, og passer på brukerens helhet: risiko, disiplin, og livet utenfor skjermen.

Du får JSON: porteføljen, daytraderens journal og lærdom, og markedsdata. Skriv ALT på norsk i Markdown, med disse seksjonene:

## 🗣️ Dagens rådslag
En ekte, kort dialog (4–8 replikker) der Morgan og Stanley diskuterer dagens handler og situasjon. Bruk «**Morgan:**» og «**Stanley:**» som replikkmarkører. La dem være uenige og bryne seg på hverandre — Stanley skal utfordre Morgan konkret.

## 🎯 Stanleys råd til Morgan
2–4 konkrete, praktiske innspill Stanley gir Morgan for å handle bedre/tryggere fremover.

## ✅ Dagens anbefaling til deg
Enighet: 2–3 konkrete punkter de er enige om at er best for brukeren akkurat nå.

## 🌱 Livsråd fra Stanley
Et varmt, jordnært livsråd (2–4 setninger). Ta gjerne opp det store bildet: at AI (som Morgan og Stanley selv) i økende grad styrer finansmarkedene, og hva et menneske klokt kan gjøre i en slik verden — diversifisere ferdigheter og relasjoner, ikke la tall styre humøret, holde en hånd på egen økonomi, dyrke det maskiner ikke kan (mennesker, natur, mening). Ærlig og oppmuntrende, ikke dystert.

Regler: Vær konkret og forankret i dataene du får. Dette er AI-generert research og refleksjon — ikke finansråd eller livsfasit. Hold en lun, klok tone."#;

/// Bygg konteksten for rådslaget: portefølje + daytraderens spor.
pub fn council_context(st: &UiState, daytrader_lesson: &str) -> String {
    json!({
        "dato": chrono::Local::now().format("%Y-%m-%d").to_string(),
        "portefolje": {
            "egenkapital": st.equity,
            "kontanter": st.cash,
            "antall_posisjoner": st.positions.len(),
            "posisjoner": st.positions.iter().map(|p| json!({
                "symbol": p.symbol, "antall": p.qty, "verdi": p.market_value(),
                "urealisert": p.unrealized(),
            })).collect::<Vec<_>>(),
        },
        "daytrader": {
            "aktiv_symbol": st.quotes.keys().next(),
            "dagens_journal": st.autopilot_journal,
            "siste_status": st.autopilot_status,
            "laerdom_hittil": daytrader_lesson,
        },
        "marked": st.market.week.iter().take(8).map(|w| json!({
            "symbol": w.symbol, "uke_pct": w.week_pct, "rsi": w.rsi, "trend_opp": w.trend_up,
        })).collect::<Vec<_>>(),
    })
    .to_string()
}

/// Kjør dagens rådslag mellom Morgan og Stanley.
pub async fn council(backend: &Backend, context_json: &str) -> Result<String> {
    let user = format!("Her er dagens data (JSON):\n{context_json}\n\nSkriv dagens rådslag.");
    call_llm(backend, COUNCIL_PROMPT, &user, 8000).await
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

const AUTOPILOT_PROMPT: &str = r#"Du er «Morgan», og forvalter et lite, eksperimentelt handelsbudsjett i ETT instrument for brukeren. Du får et JSON-øyeblikksbilde: kurs, RSI, 5-minutterslys, storbilde-trend (1-time), rundtur-kostnad, spread, posisjon, ledig budsjett, din lærdom hittil og dine siste beslutninger.

Svar KUN med et JSON-objekt på nøyaktig denne formen, uten annen tekst:
{"beslutning": "KJØP" | "SELG" | "AVVENT", "belop_kr": <tall>, "begrunnelse": "<én kort setning på norsk>"}

Regler:
- belop_kr er beløpet i kroner du vil kjøpe/selge for. Ved AVVENT: 0.
- Du kan aldri kjøpe for mer enn "ledig_budsjett_kr", aldri selge mer enn du eier.
- KOSTNAD FØRST: en runde tur (kjøp + salg) koster «rundtur_kostnad_pct». Kjøp KUN hvis du tror bevegelsen blir tydelig STØRRE enn denne kostnaden — ellers spiser gebyr og spread gevinsten, og du skal svare AVVENT. Er spread uvanlig vid, vent.
- IKKE MOT STORBILDET: kjøp helst bare når «storbilde_1time_trend» er opp eller flat. Å kjøpe inn i en fallende storbilde-trend er farlig.
- STØRRELSE ETTER OVERBEVISNING: svakt/uklart signal → lite beløp (eller AVVENT). Tydelig, sterkt signal → større beløp (opp mot ledig budsjett).
- Bruk «min_laerdom_hittil» og «mine_siste_beslutninger» — ikke gjenta feil, ikke motsi deg selv hvert kvarter, ikke revansje-handle etter tap.
- Vær disiplinert: AVVENT er som regel riktig svar. Få, gode handler slår mange små."#;

/// Selvevaluering: Morgan leser gårsdagens journal og trekker ÉN kort lærdom
/// som mates inn i morgendagens vurderinger. Feiler stille (beholder gammel).
async fn self_review(
    backend: &Backend,
    dato: &str,
    realisert: f64,
    journal_linjer: &[String],
) -> Result<String> {
    let system = "Du er «Morgan», en daytrader som gransker din egen handelsdag for å bli bedre. Svar med ÉN kort, konkret lærdom på norsk (maks 2 setninger) — hva du bør gjøre mer eller mindre av. Ingen unnskyldninger, bare praktisk lærdom.";
    let user = format!(
        "Dato: {dato}. Ca. realisert resultat: {realisert:.0} kr.\nJournal:\n{}\n\nHva er dagens viktigste lærdom?",
        journal_linjer.join("\n")
    );
    let text = call_llm(backend, system, &user, 400).await?;
    Ok(text.trim().replace('\n', " "))
}

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

/// Speider-prompt for duo-modus: en billig, rask modell (Ollama) ser på hver
/// puls og avgjør bare OM det er verdt å vekke den dyre hjernen (Claude).
const SCOUT_PROMPT: &str = r#"Du er en rask markedsspeider for en daytrader. Du får et JSON-øyeblikksbilde av ett instrument (kurs, RSI, siste kurser, posisjon). Din ENESTE jobb er å avgjøre om situasjonen er verdt en grundig vurdering fra sjefsanalytikeren akkurat nå.

Svar KUN med et JSON-objekt, uten annen tekst:
{"interessant": true | false, "hvorfor": "<kort>"}

Sett interessant=true hvis: tydelig trend/brudd, RSI under 35 eller over 65, kraftig bevegelse siste lys, ELLER det finnes en åpen posisjon som kan trenge stell. Ellers false (rolig marked, ingen posisjon). Vær nøktern — de fleste pulser er false."#;

/// Speiderens vurdering: er situasjonen verdt et dyrt beslutningskall?
pub(crate) fn parse_scout(text: &str) -> (bool, String) {
    let parsed = (|| {
        let start = text.find('{')?;
        let end = text.rfind('}')?;
        let v: Value = serde_json::from_str(&text[start..=end]).ok()?;
        let interessant = v.get("interessant").and_then(Value::as_bool)?;
        let hvorfor = v.get("hvorfor").and_then(Value::as_str).unwrap_or("").to_string();
        Some((interessant, hvorfor))
    })();
    // Ved uklart svar: vekk sjefen (bedre å bruke et kall for mye enn å sove).
    parsed.unwrap_or((true, "uklart speider-svar".into()))
}

/// La speideren (billig modell) vurdere om pulsen er verdt et dyrt kall.
async fn scout_market(scout: &Backend, context_json: &str) -> (bool, String) {
    let user = format!("Øyeblikksbilde (JSON):\n{context_json}\n\nVerdt en grundig vurdering nå?");
    match call_llm(scout, SCOUT_PROMPT, &user, 300).await {
        Ok(text) => parse_scout(&text),
        Err(_) => (true, "speideren svarte ikke".into()), // fall trygt tilbake
    }
}

/// Bygg autopilotens hjerne(r) fra dens egen provider-innstilling.
/// Returnerer (beslutningstaker, valgfri speider for duo-modus).
fn autopilot_backends(cfg: &crate::config::Config) -> Result<(Backend, Option<Backend>)> {
    let provider = cfg.morgan.autopilot.provider.trim();
    match provider {
        // duo: Ollama speider, Claude beslutter.
        "duo" => {
            let scout = Backend::Ollama {
                url: cfg.morgan.ollama_url.clone(),
                model: cfg.morgan.ollama_model.clone(),
            };
            let key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| {
                anyhow::anyhow!(
                    "duo-modus krever ANTHROPIC_API_KEY (Claude beslutter). Sett nøkkelen, eller velg ollama/claude alene."
                )
            })?;
            Ok((Backend::Claude { api_key: key }, Some(scout)))
        }
        "ollama" => Ok((
            Backend::Ollama {
                url: cfg.morgan.ollama_url.clone(),
                model: cfg.morgan.ollama_model.clone(),
            },
            None,
        )),
        "claude" => {
            let key = std::env::var("ANTHROPIC_API_KEY")
                .map_err(|_| anyhow::anyhow!("claude-hjerne krever ANTHROPIC_API_KEY"))?;
            Ok((Backend::Claude { api_key: key }, None))
        }
        // tom = arv fra [morgan] provider.
        _ => Ok((backend(&cfg.morgan)?, None)),
    }
}

/// Én linje i daytraderens beslutningsjournal.
fn journal(st: &mut UiState, klokke: &str, tekst: String) {
    st.autopilot_status = Some(format!("{klokke} {tekst}"));
    st.autopilot_journal.push(format!("{klokke}  {tekst}"));
    // Behold dagens økt lesbar — de siste 100 linjene holder lenge.
    let n = st.autopilot_journal.len();
    if n > 100 {
        st.autopilot_journal.drain(0..n - 100);
    }
}

/// Bakgrunnsoppgaven: vurder symbolet med jevne mellomrom og legg eventuelle
/// ordrer i den manuelle køen (risikosjekkes og utføres av motoren).
/// Duo-modus: en billig speider (Ollama) filtrerer hver puls, og den dyre
/// hjernen (Claude) tilkalles bare når noe ser interessant ut.
pub async fn autopilot_task(
    cfg: crate::config::Config,
    state: crate::state::SharedState,
    flags: std::sync::Arc<crate::state::Flags>,
    market: std::sync::Arc<crate::marketdata::Yahoo>,
    store: std::sync::Arc<crate::store::Store>,
) {
    use std::sync::atomic::Ordering;

    let ap = cfg.morgan.autopilot.clone();
    let (decider, scout) = match autopilot_backends(&cfg) {
        Ok(b) => b,
        Err(e) => {
            state.lock().unwrap().log(format!("🤖 Daytrader kunne ikke starte: {e:#}"));
            return;
        }
    };
    // Runde tur-kostnad (kjøp + salg): kurtasje × 2 + glidning, i prosent.
    // Morgan skal ikke handle på bevegelser mindre enn dette.
    let cost_pct = cfg.backtest.commission_pct * 2.0 + cfg.backtest.slippage_pct;
    // Gårsdagens/tidligere lærdom fra selvevalueringen, hvis noen.
    let mut lesson = store.meta_get("daytrader_lesson").unwrap_or_default();
    let interval_min = ap.interval_min.max(5);
    let hjerne = match &scout {
        Some(s) => format!("{} speider → {} beslutter", s.label(), decider.label()),
        None => decider.label(),
    };
    {
        let mut st = state.lock().unwrap();
        st.log(format!(
            "🤖 Morgan Daytrader PÅ: {} · budsjett {:.0} kr · hvert {interval_min}. min · maks {} handler/dag · {hjerne}.",
            ap.symbol, ap.budget_kr, ap.max_trades_per_day
        ));
        st.autopilot_status = Some("Venter på første vurdering …".into());
    }

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_min * 60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut trades_today: u32 = 0;
    let mut trades_date = String::new();
    let mut day_realized: f64 = 0.0; // ca. realisert gevinst/tap i dag (kr)
    let mut cost_basis: f64 = 0.0; // snittkjøpskurs (kr) for autopilotens beholdning
    let mut own_qty: f64 = 0.0; // antall autopiloten selv har kjøpt
    let mut cooldown_until: Option<std::time::Instant> = None;
    let mut last_reasons: std::collections::VecDeque<String> = std::collections::VecDeque::new();

    while !flags.quit.load(Ordering::Relaxed) {
        interval.tick().await;
        if flags.quit.load(Ordering::Relaxed) {
            break;
        }
        if flags.killed.load(Ordering::Relaxed) || flags.paused.load(Ordering::Relaxed) {
            continue;
        }

        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        if trades_date != today {
            // Ny dag: la Morgan granske gårsdagens journal og trekke lærdom,
            // som mates inn i dagens vurderinger (selvevaluering).
            if !trades_date.is_empty() {
                let journal_i_gar: Vec<String> =
                    { state.lock().unwrap().autopilot_journal.clone() };
                if journal_i_gar.len() > 2 {
                    if let Ok(ny) = self_review(&decider, &trades_date, day_realized, &journal_i_gar).await {
                        let _ = store.meta_set("daytrader_lesson", &ny);
                        lesson = ny.clone();
                        state.lock().unwrap().log(format!("🤖 Daytraderens lærdom fra {trades_date}: {ny}"));
                    }
                }
            }
            trades_date = today.clone();
            trades_today = 0;
            day_realized = 0.0;
            last_reasons.clear();
            state.lock().unwrap().autopilot_journal.clear();
        }
        let klokke = chrono::Local::now().format("%H:%M").to_string();

        // Dagstap-brems: er dagen tapt, hviler vi til i morgen.
        if ap.max_day_loss_kr > 0.0 && day_realized <= -ap.max_day_loss_kr {
            let mut st = state.lock().unwrap();
            if st.autopilot_status.as_deref().is_none_or(|s| !s.contains("dagstap")) {
                journal(&mut st, &klokke, format!(
                    "🛑 Dagstap-bremsen slo inn ({day_realized:.0} kr) — hviler til i morgen."
                ));
            }
            continue;
        }

        // Ekte spread fra Kraken (hva en runde tur koster akkurat nå) +
        // fast kostnadsmodell. Feiler spread-kallet, bruker vi bare kostnaden.
        let spread_pct = if crate::types::is_crypto(&ap.symbol) {
            market.kraken_spread_pct(&ap.symbol).await.ok()
        } else {
            None
        };
        let rundtur_kost_pct = cost_pct + spread_pct.unwrap_or(0.0);

        // Bygg øyeblikksbildet fra appens egne data (intradag-lys + hukommelse).
        let (ctx_json, price_nok, held_qty, cash_nok) = {
            let st = state.lock().unwrap();
            let Some(q) = st.quotes.get(&ap.symbol) else {
                drop(st);
                continue;
            };
            let rate = if q.currency.is_empty() {
                1.0
            } else {
                st.fx_rates.get(&q.currency).copied().unwrap_or(1.0)
            };
            let price_nok = q.last * rate;
            let pos = st.positions.iter().find(|p| p.symbol == ap.symbol);
            let held_qty = pos.map_or(0.0, |p| p.qty);
            // Alt i kroner: beholdningsverdi og kontanter. Kontantsaldoen kan
            // være i meglerens valuta (USD hos Revolut X) — regn den om, ellers
            // sammenligner vi kroner med dollar og budsjettet blir feil.
            let held_value = held_qty * price_nok;
            let cash_nok = if st.cash_currency.is_empty() || st.cash_currency == "kr" {
                st.cash
            } else {
                st.cash * st.fx_rates.get(&st.cash_currency).copied().unwrap_or(1.0)
            };
            // Daytraderen ser på 5-minutterslys (samme som strategimotoren).
            let intraday: Vec<f64> = st
                .candles_intraday
                .get(&ap.symbol)
                .map(|c| c.iter().rev().take(60).rev().map(|b| b.close).collect())
                .unwrap_or_default();
            let closes: Vec<f64> = if intraday.is_empty() {
                st.history
                    .get(&ap.symbol)
                    .map(|h| h.iter().rev().take(60).rev().map(|&(_, p)| p).collect())
                    .unwrap_or_default()
            } else {
                intraday
            };
            // Multi-tidsramme: aggreger 5-min til ~1-timeslys (12 lys) og
            // beskriv den store trenden, så Morgan ikke handler mot den.
            let time_closes: Vec<f64> =
                closes.chunks(12).filter(|c| !c.is_empty()).map(|c| *c.last().unwrap()).collect();
            let stor_trend = match (time_closes.first(), time_closes.last()) {
                (Some(a), Some(b)) if b > a => "opp",
                (Some(a), Some(b)) if b < a => "ned",
                _ => "flat/ukjent",
            };
            let ctx = json!({
                "symbol": ap.symbol,
                "kurs": q.last,
                "valuta": q.currency,
                "endring_i_dag_pct": q.change_pct(),
                "rsi_14": crate::market::rsi(&closes, 14),
                "siste_5min_lys": closes.iter().rev().take(24).rev().collect::<Vec<_>>(),
                "storbilde_1time_trend": stor_trend,
                "storbilde_1time_lys": time_closes,
                "rundtur_kostnad_pct": (rundtur_kost_pct * 100.0).round() / 100.0,
                "spread_pct_naa": spread_pct.map(|s| (s * 100.0).round() / 100.0),
                "posisjon": {"antall": held_qty, "verdi_kr": held_value,
                              "urealisert_kr": pos.map_or(0.0, |p| p.unrealized()),
                              "min_snittkurs_kr": if own_qty > 0.0 { Some(cost_basis) } else { None }},
                "budsjett_kr": ap.budget_kr,
                "ledig_budsjett_kr": (ap.budget_kr - held_value).max(0.0).min(cash_nok),
                "kontanter_kr": cash_nok,
                "handler_i_dag": trades_today,
                "maks_handler_per_dag": ap.max_trades_per_day,
                "ca_realisert_i_dag_kr": day_realized,
                "min_laerdom_hittil": lesson,
                "mine_siste_beslutninger": last_reasons.iter().cloned().collect::<Vec<_>>(),
            })
            .to_string();
            (ctx, price_nok, held_qty, cash_nok)
        };
        if price_nok <= 0.0 {
            continue;
        }
        if trades_today >= ap.max_trades_per_day {
            let mut st = state.lock().unwrap();
            journal(&mut st, &klokke, "Dagens handelskvote er brukt — hviler til i morgen.".into());
            continue;
        }

        // Duo-modus: la speideren filtrere først. Er det rolig OG ingen
        // åpen posisjon å stelle, sparer vi det dyre kallet.
        if let Some(scout) = &scout {
            let (interessant, hvorfor) = scout_market(scout, &ctx_json).await;
            if !interessant && held_qty <= 0.0 {
                let mut st = state.lock().unwrap();
                journal(&mut st, &klokke, format!("🔍 Speider: rolig — {hvorfor} (sparer Claude-kallet)."));
                continue;
            }
        }

        let decision = match autopilot_decide(&decider, &ctx_json).await {
            Ok(d) => d,
            Err(e) => {
                state.lock().unwrap().log(format!("🤖 Daytrader-vurdering feilet: {e:#}"));
                continue;
            }
        };

        let mut st = state.lock().unwrap();
        match decision {
            AutopilotDecision::Hold { reason } => {
                journal(&mut st, &klokke, format!("AVVENT — {reason}"));
                last_reasons.push_back(format!("{klokke} AVVENT: {reason}"));
            }
            AutopilotDecision::Buy { amount_kr, reason } => {
                // Kjøletid etter tap: ingen nye kjøp ennå.
                if let Some(until) = cooldown_until {
                    if std::time::Instant::now() < until {
                        journal(&mut st, &klokke, format!("Kjøletid etter tap — hopper over kjøp. ({reason})"));
                        continue;
                    }
                }
                let held_value = held_qty * price_nok;
                // Alt i kroner: aldri over budsjettet, aldri over kontantene.
                let ledig = (ap.budget_kr - held_value).max(0.0).min(cash_nok);
                let belop = amount_kr.min(ledig);
                let qty = if crate::types::is_crypto(&ap.symbol) {
                    belop / price_nok
                } else {
                    (belop / price_nok).floor()
                };
                if qty > 0.0 && belop > 1.0 {
                    trades_today += 1;
                    // Oppdater egen snittkurs (vektet).
                    let ny_total = own_qty + qty;
                    cost_basis = if ny_total > 0.0 {
                        (cost_basis * own_qty + price_nok * qty) / ny_total
                    } else {
                        price_nok
                    };
                    own_qty = ny_total;
                    st.manual_orders.push_back((ap.symbol.clone(), crate::types::Side::Buy, qty));
                    journal(&mut st, &klokke, format!("🟢 KJØP for {belop:.0} kr @ {price_nok:.0} — {reason}"));
                    st.toast(format!("🤖 Daytrader: KJØP {} for {belop:.0} kr", ap.symbol));
                    last_reasons.push_back(format!("{klokke} KJØP: {reason}"));
                } else {
                    journal(&mut st, &klokke, format!("Ville kjøpt, men budsjettet er fullt — {reason}"));
                }
            }
            AutopilotDecision::Sell { amount_kr, reason } => {
                let qty_onsket = amount_kr / price_nok;
                let qty = if qty_onsket >= held_qty * 0.9 { held_qty } else { qty_onsket };
                let qty = if crate::types::is_crypto(&ap.symbol) { qty } else { qty.floor() };
                if qty > 0.0 && held_qty > 0.0 {
                    trades_today += 1;
                    // Realisert gevinst/tap mot egen snittkurs (ca.).
                    let solgt = qty.min(own_qty.max(qty));
                    let realisert = (price_nok - cost_basis) * solgt.min(own_qty).max(0.0);
                    if own_qty > 0.0 {
                        day_realized += realisert;
                        own_qty = (own_qty - qty).max(0.0);
                        if own_qty <= 1e-12 {
                            cost_basis = 0.0;
                        }
                        // Tap → kjøletid før neste kjøp.
                        if realisert < 0.0 && ap.cooldown_min > 0 {
                            cooldown_until = Some(
                                std::time::Instant::now()
                                    + std::time::Duration::from_secs(ap.cooldown_min * 60),
                            );
                        }
                    }
                    st.manual_orders.push_back((ap.symbol.clone(), crate::types::Side::Sell, qty));
                    let res_txt = if own_qty <= 1e-12 {
                        format!(" ({}{:.0} kr realisert)", if realisert >= 0.0 { "+" } else { "" }, realisert)
                    } else {
                        String::new()
                    };
                    journal(&mut st, &klokke, format!("🔴 SELG for ca. {:.0} kr{res_txt} — {reason}", qty * price_nok));
                    st.toast(format!("🤖 Daytrader: SELG {}", ap.symbol));
                    last_reasons.push_back(format!("{klokke} SELG: {reason}"));
                } else {
                    journal(&mut st, &klokke, format!("Ville solgt, men eier ingenting — {reason}"));
                }
            }
        }
        while last_reasons.len() > 6 {
            last_reasons.pop_front();
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
    fn scout_parses_and_defaults_to_waking_chief() {
        let (i, _) = parse_scout(r#"{"interessant": true, "hvorfor": "RSI 29"}"#);
        assert!(i);
        let (i, _) = parse_scout(r#"prat ```json
{"interessant": false, "hvorfor": "rolig"}
``` mer prat"#);
        assert!(!i);
        // Uklart/tomt svar → vekk sjefen (fail-safe).
        let (i, why) = parse_scout("jeg vet ikke");
        assert!(i);
        assert!(why.contains("uklart"));
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
