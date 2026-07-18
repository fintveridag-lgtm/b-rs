//! Norsk Tipping-modul: trekningshistorikk og ærlig lotterianalyse.
//!
//! Viktig premiss: hver trekning er uavhengig, og alle rekker har nøyaktig
//! samme vinnersjanse. Historikk kan aldri forutsi neste trekning. Det eneste
//! en spiller faktisk kan påvirke, er *premiedeling*: potten deles mellom alle
//! som har samme rekke, så en rekke få andre spiller gir høyere forventet
//! utbetaling *hvis* den først vinner. «Beste rekker» her betyr derfor
//! upopulære, mønsterfrie rekker — aldri «tall som skal trekkes».
//!
//! Datahenting bruker Norsk Tippings uoffisielle resultat-endepunkt
//! (`/api-{spill}/getResultInfo.json?drawID=`), samme som flere åpne
//! kildekode-prosjekter har brukt. Det kan endres uten varsel; derfor kan
//! endepunktmalen overstyres, og alt lagres lokalt som CSV så analysen
//! fungerer uten nett.

/// Versjonsmerke for kildeoppsettet — vises i GUI og CLI, så det er lett å
/// se om en kjørende binær faktisk har de nyeste hente-kildene.
pub const KILDE_VERSJON: &str = "kildeoppsett v4 · Veikkaus draw-results + NT-side";

use anyhow::{anyhow, bail, Context, Result};
use chrono::{Datelike, NaiveDate};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Spillene analysen støtter, med dagens regler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spill {
    /// 7 av 34 (uendret siden starten i 1986).
    Lotto,
    /// 6 av 48 + 1 vikingtall av 5 (før 2017 var vikingtallet 1 av 8).
    Vikinglotto,
    /// 5 av 50 + 2 stjernetall av 12 (før mars 2022: 2 av 10).
    Eurojackpot,
}

impl Spill {
    pub const ALLE: [Spill; 3] = [Spill::Lotto, Spill::Vikinglotto, Spill::Eurojackpot];

    pub fn navn(self) -> &'static str {
        match self {
            Spill::Lotto => "Lotto",
            Spill::Vikinglotto => "Vikinglotto",
            Spill::Eurojackpot => "Eurojackpot",
        }
    }

    /// Navnet i API-stier og filnavn.
    pub fn api_navn(self) -> &'static str {
        match self {
            Spill::Lotto => "lotto",
            Spill::Vikinglotto => "vikinglotto",
            Spill::Eurojackpot => "eurojackpot",
        }
    }

    pub fn fra_navn(s: &str) -> Option<Spill> {
        match s.to_lowercase().as_str() {
            "lotto" => Some(Spill::Lotto),
            "vikinglotto" | "viking" => Some(Spill::Vikinglotto),
            "eurojackpot" | "euro" => Some(Spill::Eurojackpot),
            _ => None,
        }
    }

    /// Antall hovedtall spilleren velger.
    pub fn hovedtall_antall(self) -> usize {
        match self {
            Spill::Lotto => 7,
            Spill::Vikinglotto => 6,
            Spill::Eurojackpot => 5,
        }
    }

    /// Høyeste hovedtall (1..=maks).
    pub fn hovedtall_maks(self) -> u8 {
        match self {
            Spill::Lotto => 34,
            Spill::Vikinglotto => 48,
            Spill::Eurojackpot => 50,
        }
    }

    /// Antall ekstratall spilleren velger (vikingtall/stjernetall).
    /// Lottos tilleggstall trekkes av maskinen og velges ikke av spilleren.
    pub fn ekstra_antall(self) -> usize {
        match self {
            Spill::Lotto => 0,
            Spill::Vikinglotto => 1,
            Spill::Eurojackpot => 2,
        }
    }

    /// Høyeste ekstratall (1..=maks); 0 der spilleren ikke velger ekstratall.
    pub fn ekstra_maks(self) -> u8 {
        match self {
            Spill::Lotto => 0,
            Spill::Vikinglotto => 5,
            Spill::Eurojackpot => 12,
        }
    }

    /// Antall mulige rekker = 1/vinnersjansen for førstepremien.
    pub fn kombinasjoner(self) -> u128 {
        let hoved = kombinasjoner(self.hovedtall_maks() as u128, self.hovedtall_antall() as u128);
        let ekstra = if self.ekstra_antall() == 0 {
            1
        } else {
            kombinasjoner(self.ekstra_maks() as u128, self.ekstra_antall() as u128)
        };
        hoved * ekstra
    }

    /// Omtrentlig andel av innsatsen som går tilbake til spillerne.
    pub fn tilbakebetaling(self) -> f64 {
        // Norsk Tippings lotterispill betaler ca. halvparten tilbake i premier.
        0.5
    }
}

/// «n over k» — antall måter å velge k av n.
pub fn kombinasjoner(n: u128, k: u128) -> u128 {
    if k > n {
        return 0;
    }
    let k = k.min(n - k);
    let mut resultat: u128 = 1;
    for i in 0..k {
        resultat = resultat * (n - i) / (i + 1);
    }
    resultat
}

/// Én trekning slik den lagres lokalt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trekning {
    pub dato: NaiveDate,
    pub hovedtall: Vec<u8>,
    /// Vikingtall, stjernetall eller Lottos tilleggstall — det dataene gir.
    pub ekstra: Vec<u8>,
}

// ───────────────────────────── CSV-lager ─────────────────────────────

/// Filsti for et spills historikk under datamappen.
pub fn csv_sti(mappe: &Path, spill: Spill) -> PathBuf {
    mappe.join(format!("{}.csv", spill.api_navn()))
}

/// Skriv historikken som CSV: `dato;hovedtall;ekstra` (tall kommaseparert).
pub fn skriv_csv(sti: &Path, trekninger: &[Trekning]) -> Result<()> {
    if let Some(forelder) = sti.parent() {
        std::fs::create_dir_all(forelder)?;
    }
    let mut ut = String::from("dato;hovedtall;ekstra\n");
    for t in trekninger {
        let hoved: Vec<String> = t.hovedtall.iter().map(|n| n.to_string()).collect();
        let ekstra: Vec<String> = t.ekstra.iter().map(|n| n.to_string()).collect();
        ut.push_str(&format!(
            "{};{};{}\n",
            t.dato.format("%Y-%m-%d"),
            hoved.join(","),
            ekstra.join(",")
        ));
    }
    std::fs::write(sti, ut).with_context(|| format!("kunne ikke skrive {}", sti.display()))
}

/// Slå nye trekninger sammen med det som alt står i CSV-en og skriv tilbake.
/// Slik bygger kilder med kort vindu (Lotto-siden viser ~15 uker) seg opp
/// til full historikk over tid. Returnerer totalt antall etter sammenslåing.
pub fn oppdater_csv(sti: &Path, nye: Vec<Trekning>) -> Result<usize> {
    let mut alle = les_csv(sti).unwrap_or_default();
    alle.extend(nye);
    alle.sort_by_key(|t| t.dato);
    alle.dedup_by_key(|t| t.dato);
    skriv_csv(sti, &alle)?;
    Ok(alle.len())
}

/// Les historikk fra CSV; ukjente/ødelagte linjer hoppes over med telling.
pub fn les_csv(sti: &Path) -> Result<Vec<Trekning>> {
    let innhold = std::fs::read_to_string(sti)
        .with_context(|| format!("kunne ikke lese {}", sti.display()))?;
    let mut trekninger = Vec::new();
    for linje in innhold.lines().skip_while(|l| l.starts_with("dato") || l.starts_with('#')) {
        let linje = linje.trim();
        if linje.is_empty() {
            continue;
        }
        let deler: Vec<&str> = linje.split(';').collect();
        if deler.len() < 2 {
            continue;
        }
        let Ok(dato) = NaiveDate::parse_from_str(deler[0], "%Y-%m-%d") else {
            continue;
        };
        let hovedtall = tall_liste(deler[1]);
        let ekstra = if deler.len() > 2 { tall_liste(deler[2]) } else { Vec::new() };
        if hovedtall.is_empty() {
            continue;
        }
        trekninger.push(Trekning { dato, hovedtall, ekstra });
    }
    trekninger.sort_by_key(|t| t.dato);
    Ok(trekninger)
}

fn tall_liste(felt: &str) -> Vec<u8> {
    felt.split(',')
        .filter_map(|d| d.trim().parse::<u8>().ok())
        .collect()
}

// ───────────────────────── Henting av historikk ─────────────────────────
//
// To kilder, prøvd i rekkefølge:
//
// 1. **Veikkaus** (det finske spillselskapet) har et åpent, veldokumentert
//    resultat-API. Vikinglotto og Eurojackpot er *fellestrekninger* på tvers
//    av landene, så vinnertallene der er identiske med Norsk Tippings.
//    Gjelder ikke norsk Lotto (finsk Lotto er et annet spill).
// 2. **Norsk Tippings uoffisielle** drawID-endepunkt (historisk
//    `/api-{spill}/getResultInfo.json?drawID=`). Kan være lagt ned;
//    `--endepunkt` overstyrer malen, og `b-tipping sonde` tester kandidatene.

/// Standard endepunktmal for Norsk Tipping-kilden. `{spill}` og `{id}`
/// byttes ut; tom `{id}` gir siste trekning.
pub const ENDEPUNKT_MAL: &str =
    "https://www.norsk-tipping.no/api-{spill}/getResultInfo.json?drawID={id}";

/// Kandidat-maler for Norsk Tipping-kilden, prøvd i rekkefølge.
pub const NT_MALER: [&str; 2] = [
    ENDEPUNKT_MAL,
    "https://www.norsk-tipping.no/api/{spill}/getResultInfo.json?drawID={id}",
];

impl Spill {
    /// Spillnavn-kandidater i Veikkaus-API-et (tom = ikke fellestrekning).
    /// Bekreftet mot API-et juli 2026: `VIKING` og `EJACKPOT` gir 200 OK;
    /// `VIKINGLOTTO` avvises med INVALID_VALUE.
    pub fn veikkaus_navn(self) -> &'static [&'static str] {
        match self {
            Spill::Lotto => &[],
            Spill::Vikinglotto => &["VIKING"],
            Spill::Eurojackpot => &["EJACKPOT"],
        }
    }
}

/// Base-URL-kandidater for Veikkaus, prøvd i rekkefølge. `draw-results` er
/// dagens API; `draw-games` var forgjengeren.
pub const VEIKKAUS_BASER: [&str; 2] = [
    "https://www.veikkaus.fi/api/draw-results/v1",
    "https://www.veikkaus.fi/api/draw-games/v1",
];

/// Sonde-kandidater for Norsk Tippings *nye* API (brukes kun av `sonde`,
/// siden sidestrukturen deres er ukjent — utskriften avslører hva som finnes).
pub const NT_SONDE_KANDIDATER: [&str; 3] = [
    "https://api.norsk-tipping.no/DrawGameResultsAPI/v1/api/results/{spill}/latest",
    "https://www.norsk-tipping.no/api/results/v1/{spill}/latest",
    "https://www.norsk-tipping.no/api/draw-results/v1/games/{spill}/draws",
];

/// URL for én ISO-uke i Veikkaus-API-et.
pub fn veikkaus_uke_url(base: &str, spillnavn: &str, dato: NaiveDate) -> String {
    let uke = dato.iso_week();
    format!(
        "{}/games/{}/draws/by-week/{}-W{:02}",
        base,
        spillnavn,
        uke.year(),
        uke.week()
    )
}

/// Hent historikk bakover fra i dag til `fra_dato`, fra første kilde som
/// svarer med data. Med `endepunkt_mal` satt brukes kun Norsk Tipping-kilden
/// med den malen.
pub async fn hent_historikk(
    klient: &reqwest::Client,
    spill: Spill,
    fra_dato: NaiveDate,
    endepunkt_mal: Option<&str>,
    fremdrift: impl Fn(usize, NaiveDate),
) -> Result<Vec<Trekning>> {
    if let Some(mal) = endepunkt_mal {
        return hent_nt_drawid(klient, spill, fra_dato, mal, &fremdrift).await;
    }
    let mut feil: Vec<String> = Vec::new();
    for navn in spill.veikkaus_navn() {
        for base in VEIKKAUS_BASER {
            match hent_veikkaus(klient, spill, base, navn, fra_dato, &fremdrift).await {
                Ok(t) if !t.is_empty() => return Ok(t),
                Ok(_) => feil.push(format!("Veikkaus {navn} ({base}): ingen trekninger")),
                Err(e) => feil.push(format!("Veikkaus {navn} ({base}): {e:#}")),
            }
        }
    }
    // Resultatsiden med innbakt JSON — eneste kjente kilde for norsk Lotto.
    match hent_nt_side(klient, spill, fra_dato, &fremdrift).await {
        Ok(t) if !t.is_empty() => return Ok(t),
        Ok(_) => feil.push("Norsk Tippings resultatside: fant ingen trekninger i HTML-en".into()),
        Err(e) => feil.push(format!("Norsk Tippings resultatside: {e:#}")),
    }
    for mal in NT_MALER {
        match hent_nt_drawid(klient, spill, fra_dato, mal, &fremdrift).await {
            Ok(t) if !t.is_empty() => return Ok(t),
            Ok(_) => feil.push(format!("Norsk Tipping ({mal}): ingen trekninger")),
            Err(e) => feil.push(format!("Norsk Tipping ({mal}): {e:#}")),
        }
    }
    bail!(
        "alle kilder feilet for {} — kjør `b-tipping sonde` og se hva som svarer. \
         Detaljer: {}",
        spill.navn(),
        feil.join(" · ")
    )
}

/// Veikkaus: gå uke for uke bakover og les fellestrekningene.
async fn hent_veikkaus(
    klient: &reqwest::Client,
    spill: Spill,
    base: &str,
    spillnavn: &str,
    fra_dato: NaiveDate,
    fremdrift: &impl Fn(usize, NaiveDate),
) -> Result<Vec<Trekning>> {
    let mut trekninger: Vec<Trekning> = Vec::new();
    let mut dato = chrono::Local::now().date_naive();
    let mut tomme_paa_rad = 0usize;
    // Første uken må svare — ellers er kilden/navnet feil, og vi gir oss raskt.
    let forste_url = veikkaus_uke_url(base, spillnavn, dato);
    let forste = hent_json(klient, &forste_url)
        .await
        .with_context(|| format!("fikk ikke kontakt med Veikkaus ({forste_url})"))?;
    for t in tolk_trekninger_liste(&forste, spill) {
        trekninger.push(t);
    }
    dato -= chrono::Duration::weeks(1);

    while dato >= fra_dato && tomme_paa_rad < 26 {
        match hent_json(klient, &veikkaus_uke_url(base, spillnavn, dato)).await {
            Ok(v) => {
                let ny = tolk_trekninger_liste(&v, spill);
                if ny.is_empty() {
                    tomme_paa_rad += 1;
                } else {
                    tomme_paa_rad = 0;
                    for t in ny {
                        fremdrift(trekninger.len() + 1, t.dato);
                        trekninger.push(t);
                    }
                }
            }
            Err(_) => tomme_paa_rad += 1,
        }
        dato -= chrono::Duration::weeks(1);
        // Vær høflig mot serveren.
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }

    rydd(&mut trekninger, fra_dato);
    Ok(trekninger)
}

/// Norsk Tipping: finn siste drawID og gå ID-ene nedover.
async fn hent_nt_drawid(
    klient: &reqwest::Client,
    spill: Spill,
    fra_dato: NaiveDate,
    mal: &str,
    fremdrift: &impl Fn(usize, NaiveDate),
) -> Result<Vec<Trekning>> {
    let url_for = |id: Option<u64>| {
        mal.replace("{spill}", spill.api_navn())
            .replace("{id}", &id.map(|i| i.to_string()).unwrap_or_default())
    };

    let siste = hent_json(klient, &url_for(None)).await.with_context(|| {
        format!("fikk ikke kontakt med resultat-API-et for {}", spill.navn())
    })?;
    let siste_id = finn_tall_felt(&siste, &["drawID", "drawId", "drawNumber", "trekningsnummer"])
        .ok_or_else(|| anyhow!("fant ikke drawID i svaret fra API-et"))? as u64;

    let mut trekninger = Vec::new();
    if let Some(t) = tolk_trekning(&siste, spill) {
        trekninger.push(t);
    }

    let maks_feil = 10usize;
    let mut feil_paa_rad = 0usize;
    let mut id = siste_id;
    while id > 1 {
        id -= 1;
        match hent_json(klient, &url_for(Some(id))).await.ok().and_then(|v| tolk_trekning(&v, spill)) {
            Some(t) => {
                feil_paa_rad = 0;
                let dato = t.dato;
                trekninger.push(t);
                fremdrift(trekninger.len(), dato);
                if dato < fra_dato {
                    break;
                }
            }
            None => {
                feil_paa_rad += 1;
                if feil_paa_rad >= maks_feil {
                    break;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }

    rydd(&mut trekninger, fra_dato);
    if trekninger.is_empty() {
        bail!("ingen trekninger hentet for {}", spill.navn());
    }
    Ok(trekninger)
}

fn rydd(trekninger: &mut Vec<Trekning>, fra_dato: NaiveDate) {
    trekninger.retain(|t| t.dato >= fra_dato);
    trekninger.sort_by_key(|t| t.dato);
    trekninger.dedup_by_key(|t| t.dato);
}

/// Test alle kjente endepunkt-kandidater og rapporter hva de svarer.
/// Til feilsøking når kildene endrer seg: `b-tipping sonde`.
pub async fn sonde(klient: &reqwest::Client) -> Vec<(String, String)> {
    let mut urler: Vec<String> = Vec::new();
    let idag = chrono::Local::now().date_naive();
    for spill in Spill::ALLE {
        for navn in spill.veikkaus_navn() {
            for base in VEIKKAUS_BASER {
                urler.push(veikkaus_uke_url(base, navn, idag));
            }
            // Uten by-week: avslører om basen finnes og hvordan svaret ser ut.
            urler.push(format!("{}/games/{}/draws", VEIKKAUS_BASER[0], navn));
        }
        for mal in NT_MALER {
            urler.push(mal.replace("{spill}", spill.api_navn()).replace("{id}", ""));
        }
        for mal in NT_SONDE_KANDIDATER {
            urler.push(mal.replace("{spill}", spill.api_navn()));
        }
    }
    urler.dedup();
    let mut rapport = Vec::new();
    for url in urler {
        let utfall = match klient.get(&url).header("Accept", "application/json").send().await {
            Ok(svar) => {
                let status = svar.status();
                let tekst = svar.text().await.unwrap_or_default();
                let utdrag: String = tekst.chars().take(160).collect();
                format!("{} — {}", status, utdrag.replace(['\n', '\r'], " "))
            }
            Err(e) => format!("FEIL: {e}"),
        };
        rapport.push((url, utfall));
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }
    rapport
}

/// Norsk Tippings resultatside per spill. Serverrendret HTML med trekningene
/// innbakt som escaped JSON — vår Lotto-kilde etter at API-et forsvant.
/// Siden viser bare ~15 uker historikk, så CSV-en bygges opp over tid.
pub fn nt_side_url(spill: Spill) -> String {
    format!("https://www.norsk-tipping.no/lotteri/{}/resultater", spill.api_navn())
}

/// Trekk trekninger ut av resultatsidens HTML: finn `"drawDate"`-objektene i
/// den innbakte (escaped) JSON-en og tolk hvert objekt tolerant.
pub fn trekk_ut_innbakte_trekninger(html: &str, spill: Spill) -> Vec<Trekning> {
    let renset = html.replace("\\\"", "\"");
    let mut ut: Vec<Trekning> = Vec::new();
    let mut fra = 0usize;
    while let Some(pos) = renset[fra..].find("\"drawDate\"") {
        let midt = fra + pos;
        if let Some(obj) = objekt_rundt(&renset, midt) {
            if let Ok(v) = serde_json::from_str::<Value>(obj) {
                if let Some(t) = tolk_trekning(&v, spill) {
                    ut.push(t);
                }
            }
        }
        fra = midt + "\"drawDate\"".len();
    }
    ut.sort_by_key(|t| t.dato);
    ut.dedup_by_key(|t| t.dato);
    ut
}

/// Finn JSON-objektet `{ … }` som omslutter posisjonen `midt`.
fn objekt_rundt(tekst: &str, midt: usize) -> Option<&str> {
    // Bakover til objektets '{' (teller '}' vi passerer på veien).
    let mut dybde = 0i32;
    let mut start = None;
    for (i, b) in tekst[..midt].bytes().enumerate().rev() {
        match b {
            b'}' => dybde += 1,
            b'{' => {
                if dybde == 0 {
                    start = Some(i);
                    break;
                }
                dybde -= 1;
            }
            _ => {}
        }
    }
    let start = start?;
    // Fremover til matchende '}', med respekt for strenger og escapes.
    let bytes = tekst.as_bytes();
    let mut dybde = 0i32;
    let mut i_streng = false;
    let mut forrige_escape = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if i_streng {
            if forrige_escape {
                forrige_escape = false;
            } else if b == b'\\' {
                forrige_escape = true;
            } else if b == b'"' {
                i_streng = false;
            }
            continue;
        }
        match b {
            b'"' => i_streng = true,
            b'{' => dybde += 1,
            b'}' => {
                dybde -= 1;
                if dybde == 0 {
                    return Some(&tekst[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Hent trekninger fra Norsk Tippings resultatside (SSR-skraping).
///
/// Serveren har vist seg å tvangslukke aller første forespørsel fra en ny
/// klient (os error 10054) og godta de neste — derfor eget nettleser-likt
/// klientoppsett (én User-Agent, som i `jakt`, som virker) og gjenforsøk.
async fn hent_nt_side(
    _klient: &reqwest::Client,
    spill: Spill,
    fra_dato: NaiveDate,
    fremdrift: &impl Fn(usize, NaiveDate),
) -> Result<Vec<Trekning>> {
    let url = nt_side_url(spill);
    let nettleser = reqwest::Client::builder()
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/126.0 Safari/537.36",
        )
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let mut siste_feil = None;
    for forsok in 0..4u64 {
        if forsok > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(600 * forsok)).await;
        }
        let svar = nettleser
            .get(&url)
            .header("Accept", "text/html")
            .send()
            .await
            .and_then(|s| s.error_for_status());
        match svar {
            Ok(s) => {
                let html = s.text().await.unwrap_or_default();
                let mut trekninger = trekk_ut_innbakte_trekninger(&html, spill);
                rydd(&mut trekninger, fra_dato);
                for t in &trekninger {
                    fremdrift(trekninger.len(), t.dato);
                }
                return Ok(trekninger);
            }
            Err(e) => siste_feil = Some(e),
        }
    }
    Err(siste_feil.map(anyhow::Error::from).unwrap_or_else(|| anyhow!("ukjent feil")))
        .with_context(|| format!("resultatsiden svarte ikke etter 4 forsøk ({url})"))
}

/// Automatisk jakt på Norsk Tippings nye Lotto-endepunkt: last resultatsiden,
/// let etter innbakt JSON med vinnertall, og skann sidens JavaScript-bundler
/// etter API-stier. Returnerer en tekstrapport til å dele ved feilsøking.
pub async fn lotto_jakt(klient: &reqwest::Client) -> Vec<String> {
    let mut rapport = Vec::new();
    let side_url = "https://www.norsk-tipping.no/lotteri/lotto/resultater";
    rapport.push(format!("Henter {side_url} …"));
    let html = match klient
        .get(side_url)
        .header("Accept", "text/html")
        .send()
        .await
    {
        Ok(svar) => {
            rapport.push(format!("  status {}", svar.status()));
            svar.text().await.unwrap_or_default()
        }
        Err(e) => {
            rapport.push(format!("  FEIL: {e}"));
            return rapport;
        }
    };

    // 1) Innbakt JSON i selve siden? (SSR-data inneholder ofte siste trekning.)
    for marker in [
        "__NEXT_DATA__", "winningNumbers", "mainNumbers", "vinnertall",
        "umbers", "tilleggstall", "prizeLevel",
    ] {
        for (funn, utdrag) in finn_utdrag(&html, marker, 300).into_iter().take(2) {
            rapport.push(format!("  fant «{funn}» i HTML: …{utdrag}…"));
        }
    }

    // 2) Skriv ut HELE det første trekningsobjektet — da ser vi alle feltene,
    //    inkludert hva vinnertallene faktisk heter.
    let renset = html.replace("\\\"", "\"");
    if let Some(pos) = renset.find("\"drawDate\"") {
        match objekt_rundt(&renset, pos) {
            Some(obj) => {
                let pen = serde_json::from_str::<Value>(obj)
                    .and_then(|v| serde_json::to_string_pretty(&v))
                    .unwrap_or_else(|_| obj.to_string());
                let kuttet: String = pen.chars().take(4000).collect();
                rapport.push(format!("Hele første trekningsobjekt:\n{kuttet}"));
                if pen.len() > 4000 {
                    rapport.push("  … (kuttet ved 4000 tegn)".into());
                }
            }
            None => rapport.push("Fant drawDate, men klarte ikke avgrense objektet.".into()),
        }
    } else {
        rapport.push("Ingen drawDate i HTML-en — trekningene lastes trolig via API.".into());
    }

    // 3) Skann JavaScript-bundlene etter API-stier med result/draw/lotto.
    let bundler = finn_js_urler(&html, side_url);
    rapport.push(format!("Fant {} JavaScript-filer, skanner alle …", bundler.len()));
    let mut kandidater: Vec<String> = Vec::new();
    for url in bundler.into_iter().take(40) {
        let Ok(svar) = klient.get(&url).send().await else { continue };
        let Ok(js) = svar.text().await else { continue };
        for token in finn_api_tokener(&js) {
            if !kandidater.contains(&token) {
                kandidater.push(token);
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    }
    kandidater.truncate(60);
    if kandidater.is_empty() {
        rapport.push("Ingen API-stier funnet i bundlene.".into());
    } else {
        rapport.push(format!("Mulige API-stier ({}):", kandidater.len()));
        for k in kandidater {
            rapport.push(format!("  {k}"));
        }
    }
    rapport
}

/// Finn korte utdrag rundt et søkeord (uten regex-avhengighet).
fn finn_utdrag(tekst: &str, marker: &str, lengde: usize) -> Vec<(String, String)> {
    let mut ut = Vec::new();
    let mut fra = 0usize;
    while let Some(pos) = tekst[fra..].find(marker) {
        let start = fra + pos;
        let slutt = (start + lengde).min(tekst.len());
        // Klipp på tegn-grenser så vi ikke deler en UTF-8-sekvens.
        let mut s = start;
        while !tekst.is_char_boundary(s) {
            s -= 1;
        }
        let mut e = slutt;
        while !tekst.is_char_boundary(e) {
            e -= 1;
        }
        ut.push((
            marker.to_string(),
            tekst[s..e].replace(['\n', '\r'], " "),
        ));
        fra = slutt;
        if ut.len() >= 4 {
            break;
        }
    }
    ut
}

/// Plukk `<script src="…js">`-URL-er (og preload-lenker) ut av HTML.
fn finn_js_urler(html: &str, side_url: &str) -> Vec<String> {
    let mut urler = Vec::new();
    let mut fra = 0usize;
    while let Some(pos) = html[fra..].find(".js") {
        let slutt = fra + pos + 3;
        // Gå bakover til nærmeste fnutt for å få hele URL-en.
        let start = html[..slutt]
            .rfind(['"', '\''])
            .map(|i| i + 1)
            .unwrap_or(slutt);
        let kandidat = &html[start..slutt];
        if !kandidat.contains(' ') && kandidat.len() > 4 {
            let full = if kandidat.starts_with("http") {
                kandidat.to_string()
            } else if kandidat.starts_with("//") {
                format!("https:{kandidat}")
            } else if kandidat.starts_with('/') {
                format!("https://www.norsk-tipping.no{kandidat}")
            } else {
                format!("{}/{}", side_url.trim_end_matches('/'), kandidat)
            };
            if !urler.contains(&full) {
                urler.push(full);
            }
        }
        fra = slutt;
    }
    urler
}

/// Finn strenger i JavaScript som ligner API-stier for resultater.
fn finn_api_tokener(js: &str) -> Vec<String> {
    let mut ut = Vec::new();
    let js_lav = js.to_lowercase();
    let mut fra = 0usize;
    while let Some(pos) = js_lav[fra..].find("api") {
        let midt = fra + pos;
        // Utvid til hele tokenet, avgrenset av fnutter/mellomrom.
        let grense = |c: char| c == '"' || c == '\'' || c == '`' || c.is_whitespace();
        let start = js[..midt].rfind(grense).map(|i| i + 1).unwrap_or(0);
        let slutt = js[midt..].find(grense).map(|i| midt + i).unwrap_or(js.len());
        fra = slutt.max(midt + 3);
        if slutt <= start || slutt - start > 160 {
            continue;
        }
        let token = &js[start..slutt];
        let lav = token.to_lowercase();
        // Sti-aktige tokener med result/draw/lotto, ELLER tjenestenavn som
        // åpenbart handler om resultater (uansett prefiks).
        let sti_aktig = (lav.contains("result") || lav.contains("draw") || lav.contains("lotto"))
            && (token.starts_with('/') || token.starts_with("http"));
        let tjeneste = lav.contains("gameresult")
            || lav.contains("getresult")
            || lav.contains("drawgame")
            || lav.contains("lotteryresult");
        if sti_aktig || tjeneste {
            let t = token.to_string();
            if !ut.contains(&t) {
                ut.push(t);
            }
        }
        if ut.len() >= 80 {
            break;
        }
    }
    ut
}

async fn hent_json(klient: &reqwest::Client, url: &str) -> Result<Value> {
    let tekst = klient
        .get(url)
        .header("Accept", "application/json")
        // Veikkaus' egne eksempler bruker denne; ufarlig for andre kilder.
        .header("X-ESA-API-Key", "ROBOT")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    // Norsk Tipping har historisk lagt `while(true);/* 0;` o.l. foran JSON-en.
    let renset = tekst
        .trim_start_matches("while(true);")
        .trim_start_matches("/* 0;")
        .trim_start_matches("/* */")
        .trim();
    // Klipp til ytterste objekt ELLER liste — svar kan være begge deler.
    let obj = renset.find('{').zip(renset.rfind('}'));
    let liste = renset.find('[').zip(renset.rfind(']'));
    let json_del = match (obj, liste) {
        (Some((of, _)), Some((lf, lt))) if lf < of => &renset[lf..=lt],
        (Some((of, ot)), _) if ot > of => &renset[of..=ot],
        (_, Some((lf, lt))) if lt > lf => &renset[lf..=lt],
        _ => renset,
    };
    Ok(serde_json::from_str(json_del)?)
}

/// Tolk et svar som kan inneholde flere trekninger (liste, eller objekt med
/// `draws`-liste), i tillegg til enkelttrekninger.
pub fn tolk_trekninger_liste(v: &Value, spill: Spill) -> Vec<Trekning> {
    match v {
        Value::Array(liste) => liste.iter().filter_map(|e| tolk_trekning(e, spill)).collect(),
        Value::Object(kart) => {
            if let Some(Value::Array(liste)) = kart.get("draws") {
                liste.iter().filter_map(|e| tolk_trekning(e, spill)).collect()
            } else {
                tolk_trekning(v, spill).into_iter().collect()
            }
        }
        _ => Vec::new(),
    }
}

/// Norsk Tippings resultatside-format: `winnerNumber` er en liste av
/// objekter med `number`, `type` (1 = vinnertall, 2 = tilleggstall) og
/// `name` («Vinnertall 3» / «Tilleggstall 1»). Returnerer (hoved, ekstra).
fn tolk_vinnertall_objekter(v: &Value) -> Option<(Vec<u8>, Vec<u8>)> {
    let Value::Array(liste) = finn_felt(v, &["winnerNumber", "winnerNumbers"])? else {
        return None;
    };
    let mut hoved = Vec::new();
    let mut ekstra = Vec::new();
    for e in liste {
        let Some(nr) = finn_tall_felt(e, &["number", "value", "tall"]) else { continue };
        let type_ = finn_tall_felt(e, &["type"]).unwrap_or(1);
        let navn = finn_felt(e, &["name"]).and_then(Value::as_str).unwrap_or("");
        if type_ >= 2 || navn.to_lowercase().contains("tilleggstall") {
            ekstra.push(nr as u8);
        } else {
            hoved.push(nr as u8);
        }
    }
    if hoved.is_empty() {
        None
    } else {
        Some((hoved, ekstra))
    }
}

/// Tolk et API-svar til en trekning, tolerant for ulike feltnavn.
pub fn tolk_trekning(v: &Value, spill: Spill) -> Option<Trekning> {
    let dato = finn_dato(v)?;
    // Resultatsidens winnerNumber-objekter har både tall og type — bruk dem
    // direkte når de finnes.
    if let Some((mut hoved, ekstra)) = tolk_vinnertall_objekter(v) {
        if hoved.len() < spill.hovedtall_antall() {
            return None;
        }
        hoved.truncate(spill.hovedtall_antall());
        hoved.sort_unstable();
        return Some(Trekning { dato, hovedtall: hoved, ekstra });
    }
    let hovedtall = finn_tall_serie(
        v,
        &[
            "winningNumbers", "mainNumbers", "vinnertall", "hovedtall", "primary",
            "drawNumbers", "winningNumberList", "lottoNumbers", "numbers",
        ],
    )?;
    if hovedtall.len() < spill.hovedtall_antall() {
        return None;
    }
    let ekstra = finn_tall_serie(
        v,
        &[
            "vikingNumbers", "vikingNumber", "vikingtall",
            "starNumbers", "euroNumbers", "stjernetall",
            "additionalNumbers", "bonusNumbers", "tilleggstall", "secondary",
        ],
    )
    .unwrap_or_default();
    let mut hovedtall: Vec<u8> = hovedtall
        .into_iter()
        .take(spill.hovedtall_antall())
        .collect();
    hovedtall.sort_unstable();
    Some(Trekning { dato, hovedtall, ekstra })
}

/// Let rekursivt etter første felt med et av navnene.
fn finn_felt<'a>(v: &'a Value, nokler: &[&str]) -> Option<&'a Value> {
    match v {
        Value::Object(kart) => {
            for (k, verdi) in kart {
                if nokler.iter().any(|n| n.eq_ignore_ascii_case(k)) {
                    return Some(verdi);
                }
            }
            kart.values().find_map(|verdi| finn_felt(verdi, nokler))
        }
        Value::Array(liste) => liste.iter().find_map(|verdi| finn_felt(verdi, nokler)),
        _ => None,
    }
}

fn finn_tall_felt(v: &Value, nokler: &[&str]) -> Option<i64> {
    match finn_felt(v, nokler)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Tolk et felt som en liste av tall: `[1,2,3]`, `["1","2"]`, `"1,2,3"` eller ett tall.
fn finn_tall_serie(v: &Value, nokler: &[&str]) -> Option<Vec<u8>> {
    let felt = finn_felt(v, nokler)?;
    let tall = match felt {
        Value::Array(liste) => liste
            .iter()
            .filter_map(|e| match e {
                Value::Number(n) => n.as_u64().map(|x| x as u8),
                Value::String(s) => s.trim().parse().ok(),
                // Noen varianter pakker tallet inn: {"number": 7}
                Value::Object(_) => finn_tall_felt(e, &["number", "value", "tall"]).map(|x| x as u8),
                _ => None,
            })
            .collect(),
        Value::String(s) => tall_liste(s),
        Value::Number(n) => n.as_u64().map(|x| vec![x as u8]).unwrap_or_default(),
        _ => return None,
    };
    if tall.is_empty() {
        None
    } else {
        Some(tall)
    }
}

fn finn_dato(v: &Value) -> Option<NaiveDate> {
    let felt = finn_felt(v, &["drawDate", "drawTime", "date", "trekningsdato", "drawDateTime"])?;
    match felt {
        Value::Number(n) => {
            let mut epoke = n.as_i64()?;
            if epoke > 100_000_000_000 {
                epoke /= 1000; // millisekunder
            }
            chrono::DateTime::from_timestamp(epoke, 0).map(|dt| dt.date_naive())
        }
        Value::String(s) => tolk_dato_tekst(s),
        _ => None,
    }
}

fn tolk_dato_tekst(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    for format in ["%Y-%m-%d", "%d.%m.%Y", "%d.%m.%y"] {
        if let Ok(d) = NaiveDate::parse_from_str(s.get(..10.min(s.len()))?, format) {
            return Some(d);
        }
    }
    // ISO med klokkeslett: ta datodelen.
    NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()
}

// ───────────────────────────── Analyse ─────────────────────────────

/// Statistikk for ett tall.
#[derive(Debug, Clone)]
pub struct TallStatistikk {
    pub tall: u8,
    pub antall: usize,
    pub forventet: f64,
    /// Standardavvik fra forventet — |z| < 3 er helt normalt.
    pub z: f64,
    pub sist_trukket: Option<NaiveDate>,
}

#[derive(Debug, Clone)]
pub struct Analyse {
    pub spill: Spill,
    pub antall_trekninger: usize,
    pub forste: NaiveDate,
    pub siste: NaiveDate,
    pub hovedtall: Vec<TallStatistikk>,
    pub ekstra: Vec<TallStatistikk>,
    /// Chi-kvadrat for hovedtallene mot uniform fordeling.
    pub chi2: f64,
    pub chi2_frihetsgrader: usize,
    /// Sant hvis avvikene er godt innenfor det ren tilfeldighet gir.
    pub innenfor_tilfeldighet: bool,
}

/// Tell opp historikken. Tall utenfor dagens verdiområde (fra gamle regler)
/// ignoreres i opptellingen.
pub fn analyser(spill: Spill, trekninger: &[Trekning]) -> Result<Analyse> {
    if trekninger.is_empty() {
        bail!("ingen trekninger å analysere for {}", spill.navn());
    }
    let maks = spill.hovedtall_maks();
    let mut antall: BTreeMap<u8, usize> = (1..=maks).map(|t| (t, 0)).collect();
    let mut sist: BTreeMap<u8, NaiveDate> = BTreeMap::new();
    let mut gyldige_trekk = 0usize;
    for t in trekninger {
        for &tall in &t.hovedtall {
            if (1..=maks).contains(&tall) {
                *antall.get_mut(&tall).unwrap() += 1;
                gyldige_trekk += 1;
                let d = sist.entry(tall).or_insert(t.dato);
                if t.dato > *d {
                    *d = t.dato;
                }
            }
        }
    }

    let forventet = gyldige_trekk as f64 / maks as f64;
    // Varians for antall ganger ett bestemt tall trekkes ligner binomisk;
    // std ≈ sqrt(forventet · (1 − k/N)).
    let andel = spill.hovedtall_antall() as f64 / maks as f64;
    let std = (forventet * (1.0 - andel)).sqrt().max(1e-9);

    let hovedtall: Vec<TallStatistikk> = antall
        .iter()
        .map(|(&tall, &n)| TallStatistikk {
            tall,
            antall: n,
            forventet,
            z: (n as f64 - forventet) / std,
            sist_trukket: sist.get(&tall).copied(),
        })
        .collect();

    let chi2: f64 = hovedtall
        .iter()
        .map(|s| (s.antall as f64 - forventet).powi(2) / forventet.max(1e-9))
        .sum();
    let frihetsgrader = maks as usize - 1;
    // Chi-kvadrat har snitt = df og std = sqrt(2·df); innenfor 3 std regnes
    // som fullt forenlig med ren tilfeldighet.
    let innenfor = chi2 < frihetsgrader as f64 + 3.0 * (2.0 * frihetsgrader as f64).sqrt();

    // Samme øvelse for spillerens ekstratall (vikingtall/stjernetall).
    let ekstra = if spill.ekstra_antall() > 0 {
        let e_maks = spill.ekstra_maks();
        let mut e_antall: BTreeMap<u8, usize> = (1..=e_maks).map(|t| (t, 0)).collect();
        let mut e_sist: BTreeMap<u8, NaiveDate> = BTreeMap::new();
        let mut e_trekk = 0usize;
        for t in trekninger {
            for &tall in &t.ekstra {
                if (1..=e_maks).contains(&tall) {
                    *e_antall.get_mut(&tall).unwrap() += 1;
                    e_trekk += 1;
                    let d = e_sist.entry(tall).or_insert(t.dato);
                    if t.dato > *d {
                        *d = t.dato;
                    }
                }
            }
        }
        let e_forventet = e_trekk as f64 / e_maks as f64;
        let e_andel = spill.ekstra_antall() as f64 / e_maks as f64;
        let e_std = (e_forventet * (1.0 - e_andel)).sqrt().max(1e-9);
        e_antall
            .iter()
            .map(|(&tall, &n)| TallStatistikk {
                tall,
                antall: n,
                forventet: e_forventet,
                z: (n as f64 - e_forventet) / e_std,
                sist_trukket: e_sist.get(&tall).copied(),
            })
            .collect()
    } else {
        Vec::new()
    };

    Ok(Analyse {
        spill,
        antall_trekninger: trekninger.len(),
        forste: trekninger.iter().map(|t| t.dato).min().unwrap(),
        siste: trekninger.iter().map(|t| t.dato).max().unwrap(),
        hovedtall,
        ekstra,
        chi2,
        chi2_frihetsgrader: frihetsgrader,
        innenfor_tilfeldighet: innenfor,
    })
}

// ─────────────────────── «Beste rekker»-generatoren ───────────────────────

/// En foreslått rekke med poengsum og begrunnelse.
#[derive(Debug, Clone)]
pub struct Rekke {
    pub hovedtall: Vec<u8>,
    pub ekstra: Vec<u8>,
    /// Lavere = mer upopulær = mindre premiedeling om den vinner.
    pub popularitet: f64,
    pub begrunnelse: String,
}

/// Enkel, rask xorshift64* — god nok til å trekke kandidater, og frøstyrt
/// så resultatet kan reproduseres.
struct Rng(u64);

impl Rng {
    fn ny(fro: u64) -> Rng {
        Rng(fro.max(1))
    }
    fn neste(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    /// Uniformt tall i 1..=maks.
    fn tall(&mut self, maks: u8) -> u8 {
        (self.neste() % maks as u64) as u8 + 1
    }
}

/// Popularitetspoeng for en rekke: hvor mange andre spillere som *trolig*
/// har den samme. Basert på velkjente funn om spilleratferd: folk spiller
/// fødselsdager (1–31, og 1–12 dobbelt), «lykketall», mønstre og rekkefølger.
/// Lavere er bedre.
pub fn popularitet(hovedtall: &[u8], ekstra: &[u8], spill: Spill) -> f64 {
    let mut poeng = 0.0;
    let lykketall = [3u8, 7, 11, 13, 17];

    for &t in hovedtall {
        if t <= 12 {
            poeng += 2.0; // både dag og måned i fødselsdatoer
        } else if t <= 31 {
            poeng += 1.0; // dag i fødselsdatoer
        }
        if lykketall.contains(&t) {
            poeng += 0.5;
        }
    }

    // Mønstre folk spiller mye: rekkefølger og tall med samme sluttsiffer.
    let mut sortert = hovedtall.to_vec();
    sortert.sort_unstable();
    for par in sortert.windows(2) {
        if par[1] == par[0] + 1 {
            poeng += 1.5;
        }
    }
    let mut sluttsiffer = [0u8; 10];
    for &t in &sortert {
        sluttsiffer[(t % 10) as usize] += 1;
    }
    for &n in &sluttsiffer {
        if n >= 3 {
            poeng += (n as f64 - 2.0) * 1.5;
        }
    }
    // Full aritmetisk rekke (5-10-15-20-…) er ekstremt populær.
    if sortert.len() >= 3 {
        let diff = sortert[1] as i16 - sortert[0] as i16;
        if sortert.windows(2).all(|p| p[1] as i16 - p[0] as i16 == diff) {
            poeng += 10.0;
        }
    }
    // Alle tall klumpet i én tiergruppe ser «valgt» ut og deles oftere.
    let spredning = sortert.last().unwrap_or(&0) - sortert.first().unwrap_or(&0);
    if (spredning as usize) < sortert.len() * 3 {
        poeng += 3.0;
    }

    // Skjev paritet (bare partall / bare oddetall) er også et populært mønster.
    let odde = sortert.iter().filter(|t| *t % 2 == 1).count();
    let balanse = (odde as f64 - sortert.len() as f64 / 2.0).abs();
    if balanse > 1.5 {
        poeng += balanse;
    }

    // Ekstratall: små tall (fødselsdager/lykketall) er mest spilt.
    for &t in ekstra {
        if t <= 12 {
            poeng += 0.5;
        }
        if lykketall.contains(&t) {
            poeng += 0.3;
        }
    }

    // Normaliser lett mot antall tall så spillene kan sammenlignes.
    poeng / spill.hovedtall_antall() as f64
}

/// Generer `antall` rekker med lav forventet premiedeling. Frøstyrt og
/// deterministisk. Rekkene tvinges til å være innbyrdes ulike
/// (maks 2 felles hovedtall).
pub fn beste_rekker(spill: Spill, antall: usize, fro: u64) -> Vec<Rekke> {
    let mut rng = Rng::ny(fro);
    let mut kandidater: Vec<(f64, Vec<u8>, Vec<u8>)> = Vec::new();

    // Trekk mange tilfeldige rekker og behold de mest upopulære.
    let forsok = 30_000;
    for _ in 0..forsok {
        let mut hoved = Vec::with_capacity(spill.hovedtall_antall());
        while hoved.len() < spill.hovedtall_antall() {
            let t = rng.tall(spill.hovedtall_maks());
            if !hoved.contains(&t) {
                hoved.push(t);
            }
        }
        hoved.sort_unstable();
        let mut ekstra = Vec::with_capacity(spill.ekstra_antall());
        while ekstra.len() < spill.ekstra_antall() {
            let t = rng.tall(spill.ekstra_maks().max(1));
            if !ekstra.contains(&t) {
                ekstra.push(t);
            }
        }
        ekstra.sort_unstable();
        let poeng = popularitet(&hoved, &ekstra, spill);
        kandidater.push((poeng, hoved, ekstra));
    }
    kandidater.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // Plukk grådig med krav om innbyrdes ulikhet.
    let mut valgte: Vec<Rekke> = Vec::new();
    for (poeng, hoved, ekstra) in kandidater {
        if valgte.len() >= antall {
            break;
        }
        let for_lik = valgte.iter().any(|r| {
            r.hovedtall.iter().filter(|t| hoved.contains(t)).count() > 2
        });
        if for_lik {
            continue;
        }
        let begrunnelse = beskriv(&hoved, spill);
        valgte.push(Rekke { hovedtall: hoved, ekstra, popularitet: poeng, begrunnelse });
    }
    valgte
}

fn beskriv(hoved: &[u8], spill: Spill) -> String {
    let over_31 = hoved.iter().filter(|t| **t > 31).count();
    let sum: u32 = hoved.iter().map(|t| *t as u32).sum();
    let odde = hoved.iter().filter(|t| *t % 2 == 1).count();
    let mut deler = Vec::new();
    if spill.hovedtall_maks() > 31 && over_31 > 0 {
        deler.push(format!("{} tall over 31 (unngår bursdagsrekker)", over_31));
    }
    deler.push(format!("sum {}", sum));
    deler.push(format!("{} odde/{} par", odde, hoved.len() - odde));
    deler.join(", ")
}

// ───────────────────────────── Tester ─────────────────────────────

#[cfg(test)]
mod tester {
    use super::*;

    #[test]
    fn kombinatorikk_stemmer_med_offisielle_odds() {
        assert_eq!(kombinasjoner(34, 7), 5_379_616); // Lotto
        assert_eq!(Spill::Lotto.kombinasjoner(), 5_379_616);
        assert_eq!(Spill::Vikinglotto.kombinasjoner(), 61_357_560); // C(48,6)·5
        assert_eq!(Spill::Eurojackpot.kombinasjoner(), 139_838_160); // C(50,5)·C(12,2)
    }

    #[test]
    fn csv_rundtur() {
        let mappe = std::env::temp_dir().join("b-tipping-test");
        let sti = mappe.join("lotto.csv");
        let trekninger = vec![
            Trekning {
                dato: NaiveDate::from_ymd_opt(2026, 1, 3).unwrap(),
                hovedtall: vec![2, 9, 17, 22, 28, 31, 34],
                ekstra: vec![5],
            },
            Trekning {
                dato: NaiveDate::from_ymd_opt(2026, 1, 10).unwrap(),
                hovedtall: vec![1, 4, 12, 19, 25, 30, 33],
                ekstra: vec![],
            },
        ];
        skriv_csv(&sti, &trekninger).unwrap();
        let lest = les_csv(&sti).unwrap();
        assert_eq!(lest, trekninger);
        let _ = std::fs::remove_dir_all(&mappe);
    }

    #[test]
    fn tolker_api_svar_med_ulike_feltnavn() {
        let svar: Value = serde_json::from_str(
            r#"{"drawID": 1234, "drawDate": "14.06.2026",
                "resultInfo": {"winningNumbers": ["4","18","23","29","33","41"],
                                "vikingNumbers": [3]}}"#,
        )
        .unwrap();
        let t = tolk_trekning(&svar, Spill::Vikinglotto).unwrap();
        assert_eq!(t.dato, NaiveDate::from_ymd_opt(2026, 6, 14).unwrap());
        assert_eq!(t.hovedtall, vec![4, 18, 23, 29, 33, 41]);
        assert_eq!(t.ekstra, vec![3]);

        // Epoke i millisekunder og tall som objekter.
        let svar2: Value = serde_json::from_str(
            r#"{"drawDate": 1770000000000,
                "mainNumbers": [{"number": 7}, {"number": 12}, {"number": 20},
                                 {"number": 31}, {"number": 45}],
                "starNumbers": [{"number": 2}, {"number": 9}]}"#,
        )
        .unwrap();
        let t2 = tolk_trekning(&svar2, Spill::Eurojackpot).unwrap();
        assert_eq!(t2.hovedtall, vec![7, 12, 20, 31, 45]);
        assert_eq!(t2.ekstra, vec![2, 9]);
    }

    #[test]
    fn tolker_veikkaus_ukesvar() {
        // Formen Veikkaus' draw-games-API svarer med: liste av trekninger,
        // tall som strenger i results[0].primary/secondary, drawTime i ms.
        let svar: Value = serde_json::from_str(
            r#"[{"gameName":"EJACKPOT","status":"RESULTS_AVAILABLE",
                 "drawTime":1770000000000,
                 "results":[{"primary":["7","12","20","31","45"],
                              "secondary":["2","9"]}]},
                {"gameName":"EJACKPOT","status":"OPEN",
                 "drawTime":1770604800000,
                 "results":[]}]"#,
        )
        .unwrap();
        let trekninger = tolk_trekninger_liste(&svar, Spill::Eurojackpot);
        assert_eq!(trekninger.len(), 1, "åpen fremtidig trekning skal hoppes over");
        assert_eq!(trekninger[0].hovedtall, vec![7, 12, 20, 31, 45]);
        assert_eq!(trekninger[0].ekstra, vec![2, 9]);

        // Objekt med draws-liste skal også tolkes.
        let pakket: Value = serde_json::from_str(
            r#"{"draws":[{"drawTime":1770000000000,
                           "results":[{"primary":["4","18","23","29","33","41"],
                                        "secondary":["3"]}]}]}"#,
        )
        .unwrap();
        assert_eq!(tolk_trekninger_liste(&pakket, Spill::Vikinglotto).len(), 1);

        // URL-bygging bruker ISO-uke, og draw-results-basen kommer først.
        let url = veikkaus_uke_url(
            VEIKKAUS_BASER[0],
            "EJACKPOT",
            NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(),
        );
        assert_eq!(
            url,
            "https://www.veikkaus.fi/api/draw-results/v1/games/EJACKPOT/draws/by-week/2026-W01"
        );
    }

    #[test]
    fn tolker_resultatsidens_winner_number_format() {
        // Nøyaktig formen `jakt` viste fra norsk-tipping.no juli 2026.
        let svar: Value = serde_json::from_str(
            r#"{"drawDate":"2026-07-11T16:30:00.000Z","drawId":1575,
                "drawName":"LOTTO-11.07.2026 18:30","isFinalized":true,
                "prize":[{"id":1,"name":"7 rette","value":"2855715","winners":"5"}],
                "winnerNumber":[
                  {"drawOrder":1,"name":"Vinnertall 1","number":"30","type":1},
                  {"drawOrder":2,"name":"Vinnertall 2","number":"11","type":1},
                  {"drawOrder":3,"name":"Vinnertall 3","number":"3","type":1},
                  {"drawOrder":4,"name":"Vinnertall 4","number":"24","type":1},
                  {"drawOrder":5,"name":"Vinnertall 5","number":"18","type":1},
                  {"drawOrder":6,"name":"Vinnertall 6","number":"16","type":1},
                  {"drawOrder":7,"name":"Vinnertall 7","number":"7","type":1},
                  {"drawOrder":11,"name":"Tilleggstall 1","number":"28","type":2}]}"#,
        )
        .unwrap();
        let t = tolk_trekning(&svar, Spill::Lotto).unwrap();
        assert_eq!(t.dato, NaiveDate::from_ymd_opt(2026, 7, 11).unwrap());
        assert_eq!(t.hovedtall, vec![3, 7, 11, 16, 18, 24, 30]);
        assert_eq!(t.ekstra, vec![28]);
    }

    #[test]
    fn skraper_innbakt_json_fra_resultatsiden() {
        // Escaped SSR-JSON slik `jakt`-utskriften viste den (forkortet),
        // med et vinnertall-felt slik tolk_trekning forstår det.
        let html = r#"<script>window.__STATE__={"data":"{\"draws\":[{\"drawDate\":\"2026-07-11T16:30:00.000Z\",\"drawId\":1575,\"drawState\":12,\"isFinalized\":true,\"mainNumbers\":[2,9,17,22,28,31,34],\"additionalNumbers\":[5],\"prize\":[{\"name\":\"7 rette\"}]},{\"drawDate\":\"2026-07-04T16:30:00.000Z\",\"drawId\":1574,\"mainNumbers\":[1,4,12,19,25,30,33],\"additionalNumbers\":[7]}]}"}</script>"#;
        let trekninger = trekk_ut_innbakte_trekninger(html, Spill::Lotto);
        assert_eq!(trekninger.len(), 2);
        assert_eq!(trekninger[0].dato, NaiveDate::from_ymd_opt(2026, 7, 4).unwrap());
        assert_eq!(trekninger[1].hovedtall, vec![2, 9, 17, 22, 28, 31, 34]);
        assert_eq!(trekninger[1].ekstra, vec![5]);
    }

    #[test]
    fn oppdater_csv_slaar_sammen_uten_duplikater() {
        let mappe = std::env::temp_dir().join("b-tipping-test-oppdater");
        let sti = mappe.join("lotto.csv");
        let _ = std::fs::remove_file(&sti);
        let a = Trekning {
            dato: NaiveDate::from_ymd_opt(2026, 7, 4).unwrap(),
            hovedtall: vec![1, 4, 12, 19, 25, 30, 33],
            ekstra: vec![7],
        };
        let b = Trekning {
            dato: NaiveDate::from_ymd_opt(2026, 7, 11).unwrap(),
            hovedtall: vec![2, 9, 17, 22, 28, 31, 34],
            ekstra: vec![5],
        };
        assert_eq!(oppdater_csv(&sti, vec![a.clone()]).unwrap(), 1);
        // Overlappende ny henting: a igjen + b → totalt 2, ikke 3.
        assert_eq!(oppdater_csv(&sti, vec![a, b]).unwrap(), 2);
        let _ = std::fs::remove_dir_all(&mappe);
    }

    #[test]
    fn jakten_finner_js_urler_og_api_stier() {
        let html = r#"<html><script src="/static/main.abc123.js"></script>
            <link href="https://cdn.norsk-tipping.no/chunk.def.js"><body>
            {"winningNumbers":[1,2,3]}</body></html>"#;
        let urler = finn_js_urler(html, "https://www.norsk-tipping.no/lotteri/lotto/resultater");
        assert!(urler.contains(&"https://www.norsk-tipping.no/static/main.abc123.js".to_string()));
        assert!(urler.contains(&"https://cdn.norsk-tipping.no/chunk.def.js".to_string()));

        let js = r#"fetch("/api/lottery/v2/results/lotto/latest");const x="https://api.norsk-tipping.no/gameresult/draws";const uinteressant="/api/banner/v1";"#;
        let tokener = finn_api_tokener(js);
        assert!(tokener.contains(&"/api/lottery/v2/results/lotto/latest".to_string()));
        assert!(tokener.contains(&"https://api.norsk-tipping.no/gameresult/draws".to_string()));
        assert!(!tokener.iter().any(|t| t.contains("banner")));

        let utdrag = finn_utdrag(html, "winningNumbers", 40);
        assert_eq!(utdrag.len(), 1);
        assert!(utdrag[0].1.starts_with("winningNumbers"));
    }

    #[test]
    fn bursdagstung_rekke_er_mer_populaer_enn_hoy_rekke() {
        // Klassisk «familiens fødselsdager»-rekke …
        let bursdag = popularitet(&[3, 7, 11, 14, 21, 24, 31], &[], Spill::Lotto);
        // … mot en spredt rekke med høye tall.
        let hoy = popularitet(&[2, 16, 25, 28, 32, 33, 34], &[], Spill::Lotto);
        assert!(bursdag > hoy, "bursdag={bursdag} burde være > høy={hoy}");
        // 1-2-3-4-5-6-7 skal straffes hardt.
        let rekkefolge = popularitet(&[1, 2, 3, 4, 5, 6, 7], &[], Spill::Lotto);
        assert!(rekkefolge > bursdag);
    }

    #[test]
    fn beste_rekker_er_gyldige_og_ulike() {
        for spill in Spill::ALLE {
            let rekker = beste_rekker(spill, 10, 42);
            assert_eq!(rekker.len(), 10, "{}", spill.navn());
            for r in &rekker {
                assert_eq!(r.hovedtall.len(), spill.hovedtall_antall());
                assert!(r.hovedtall.iter().all(|t| (1..=spill.hovedtall_maks()).contains(t)));
                assert_eq!(r.ekstra.len(), spill.ekstra_antall());
                assert!(r.ekstra.iter().all(|t| (1..=spill.ekstra_maks()).contains(t)));
                // Ingen duplikater.
                let mut h = r.hovedtall.clone();
                h.dedup();
                assert_eq!(h.len(), r.hovedtall.len());
            }
            // Innbyrdes ulikhet: maks 2 felles hovedtall.
            for (i, a) in rekker.iter().enumerate() {
                for b in &rekker[i + 1..] {
                    let felles = a.hovedtall.iter().filter(|t| b.hovedtall.contains(t)).count();
                    assert!(felles <= 2, "{}: {} felles tall", spill.navn(), felles);
                }
            }
            // Samme frø → samme resultat.
            let igjen = beste_rekker(spill, 10, 42);
            assert_eq!(rekker[0].hovedtall, igjen[0].hovedtall);
        }
    }

    #[test]
    fn analyse_teller_riktig_og_chi2_er_fornuftig() {
        // Syntetisk, jevnt fordelt historikk: hvert tall like ofte.
        let mut trekninger = Vec::new();
        let start = NaiveDate::from_ymd_opt(1996, 1, 6).unwrap();
        for uke in 0..340u32 {
            let dato = start + chrono::Duration::weeks(uke as i64);
            let forste = (uke * 7) % 34;
            let hovedtall: Vec<u8> = (0..7).map(|i| ((forste + i) % 34) as u8 + 1).collect();
            trekninger.push(Trekning { dato, hovedtall, ekstra: vec![] });
        }
        let analyse = analyser(Spill::Lotto, &trekninger).unwrap();
        assert_eq!(analyse.antall_trekninger, 340);
        let sum_antall: usize = analyse.hovedtall.iter().map(|s| s.antall).sum();
        assert_eq!(sum_antall, 340 * 7);
        // Perfekt jevn fordeling → chi2 ≈ 0, godt innenfor tilfeldighet.
        assert!(analyse.chi2 < 1.0);
        assert!(analyse.innenfor_tilfeldighet);
    }
}
