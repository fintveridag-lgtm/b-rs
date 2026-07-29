//! b-tipping — trekningshistorikk og ærlig lotterianalyse for Norsk Tipping.
//!
//! `hent` laster ned historikk (Lotto, Vikinglotto, Eurojackpot) til CSV,
//! `analyse` viser frekvensstatistikk og genererer «de 10 beste rekkene» —
//! der «best» ærlig talt betyr *lavest forventet premiedeling*, siden ingen
//! rekke har bedre vinnersjanse enn andre.

use b_rs::tipping::{self, Spill};
use chrono::{Datelike, NaiveDate};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let kommando = args.first().map(String::as_str);
    let resultat = match kommando {
        Some("hent") => hent(&args[1..]),
        Some("analyse") => analyse(&args[1..]),
        Some("sonde") => sonde(),
        Some("jakt") => jakt(),
        _ => {
            hjelp();
            Ok(())
        }
    };
    if let Err(e) = resultat {
        eprintln!("Feil: {e:#}");
        std::process::exit(1);
    }
}

fn hjelp() {
    println!(
        "b-tipping — trekningshistorikk og ærlig lotterianalyse

Bruk:
  b-tipping hent    [lotto|vikinglotto|eurojackpot|alle] [--fra-aar ÅÅÅÅ]
                    [--mappe STI] [--endepunkt URL-MAL]
  b-tipping analyse [lotto|vikinglotto|eurojackpot|alle] [--mappe STI]
                    [--rekker N] [--fro N]

  hent      Last ned trekningshistorikk til CSV (standard: siste 30 år,
            mappe data/tipping). Vikinglotto/Eurojackpot hentes fra Veikkaus'
            åpne API (fellestrekninger — samme vinnertall som hos Norsk
            Tipping); Lotto fra Norsk Tippings uoffisielle endepunkt.
  analyse   Frekvensstatistikk over historikken + 10 foreslåtte rekker
            med lav forventet premiedeling. --fro gir reproduserbare rekker.
  sonde     Test alle kjente endepunkt-kandidater og vis hva de svarer —
            kjør denne hvis `hent` feiler, og del utskriften ved feilsøking.
  jakt      Let automatisk etter Norsk Tippings nye Lotto-endepunkt: leser
            resultatsiden og skanner JavaScript-bundlene etter API-stier.
            Del utskriften ved feilsøking.

CSV-format (kan også lages for hånd fra andre kilder):
  dato;hovedtall;ekstra          f.eks.  2026-01-03;2,9,17,22,28,31,34;5

Ærlig påminnelse: alle rekker har nøyaktig samme vinnersjanse, og
historikk kan ikke forutsi neste trekning."
    );
}

struct Valg {
    spill: Vec<Spill>,
    mappe: PathBuf,
    fra_aar: i32,
    endepunkt: Option<String>,
    rekker: usize,
    fro: u64,
}

fn tolk_valg(args: &[String]) -> anyhow::Result<Valg> {
    let naa = chrono::Local::now().date_naive();
    let mut valg = Valg {
        spill: Spill::ALLE.to_vec(),
        mappe: tipping::standard_mappe(),
        fra_aar: naa.year() - 30,
        endepunkt: None,
        rekker: 10,
        fro: naa.num_days_from_ce() as u64, // nytt frø hver dag, men stabilt innen dagen
    };
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let verdi = |i: &mut usize| -> anyhow::Result<String> {
            *i += 1;
            args.get(*i).cloned().ok_or_else(|| anyhow::anyhow!("{a} mangler verdi"))
        };
        match a.as_str() {
            "--mappe" => valg.mappe = PathBuf::from(verdi(&mut i)?),
            "--fra-aar" => valg.fra_aar = verdi(&mut i)?.parse()?,
            "--endepunkt" => valg.endepunkt = Some(verdi(&mut i)?),
            "--rekker" => valg.rekker = verdi(&mut i)?.parse()?,
            "--fro" | "--frø" => valg.fro = verdi(&mut i)?.parse()?,
            "alle" => valg.spill = Spill::ALLE.to_vec(),
            navn => match Spill::fra_navn(navn) {
                Some(s) => valg.spill = vec![s],
                None => anyhow::bail!("ukjent spill/flagg: {navn} (prøv `b-tipping` for hjelp)"),
            },
        }
        i += 1;
    }
    Ok(valg)
}

fn hent(args: &[String]) -> anyhow::Result<()> {
    let valg = tolk_valg(args)?;
    tipping::migrer_gammel_mappe(&valg.mappe);
    println!("({})", tipping::KILDE_VERSJON);
    println!("Lagres permanent i: {}", valg.mappe.display());
    let fra_dato = NaiveDate::from_ymd_opt(valg.fra_aar, 1, 1).unwrap();
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let klient = reqwest::Client::builder()
            .user_agent("b-tipping/1.0 (hobbyprosjekt; resultathistorikk)")
            .timeout(std::time::Duration::from_secs(20))
            .build()?;
        let mut feilede: Vec<String> = Vec::new();
        for spill in &valg.spill {
            println!("Henter {} fra {} og fremover …", spill.navn(), fra_dato);
            let resultat = tipping::hent_historikk(
                &klient,
                *spill,
                fra_dato,
                valg.endepunkt.as_deref(),
                |antall, dato| {
                    if antall % 50 == 0 {
                        eprintln!("  … {antall} trekninger, kommet til {dato}");
                    }
                },
            )
            .await;
            // Ett spill som feiler skal ikke stoppe de andre.
            let trekninger = match resultat {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("  FEIL: {e:#}");
                    feilede.push(spill.navn().to_string());
                    continue;
                }
            };
            let sti = tipping::csv_sti(&valg.mappe, *spill);
            let (forste, siste) =
                (trekninger.first().unwrap().dato, trekninger.last().unwrap().dato);
            let hentet = trekninger.len();
            let totalt = tipping::oppdater_csv(&sti, trekninger)?;
            println!(
                "  {hentet} trekninger hentet ({forste} – {siste}); {totalt} totalt i {}",
                sti.display()
            );
        }
        if !feilede.is_empty() {
            anyhow::bail!("henting feilet for: {}", feilede.join(", "));
        }
        Ok::<(), anyhow::Error>(())
    })
}

fn sonde() -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let klient = reqwest::Client::builder()
            .user_agent("b-tipping/1.0 (hobbyprosjekt; resultathistorikk)")
            .timeout(std::time::Duration::from_secs(20))
            .build()?;
        println!("Tester endepunkt-kandidater …\n");
        for (url, utfall) in tipping::sonde(&klient).await {
            println!("{url}\n  → {utfall}\n");
        }
        println!("Del denne utskriften ved feilsøking, så kan riktig kilde velges.");
        Ok::<(), anyhow::Error>(())
    })
}

fn jakt() -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        // Nettleser-aktig UA: NT har vist seg å tvangslukke ukjente klienter.
        let klient = reqwest::Client::builder()
            .user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/126.0 Safari/537.36",
            )
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        for linje in tipping::lotto_jakt(&klient).await {
            println!("{linje}");
        }
        println!("\nDel denne utskriften ved feilsøking, så kan Lotto-kilden bygges rundt riktig sti.");
        Ok::<(), anyhow::Error>(())
    })
}

fn analyse(args: &[String]) -> anyhow::Result<()> {
    let valg = tolk_valg(args)?;
    println!(
        "╔══════════════════════════════════════════════════════════════════════╗
║  ÆRLIG PÅMINNELSE FØRST                                              ║
║  • Alle rekker har NØYAKTIG samme vinnersjanse — alltid.             ║
║  • Historikken under kan ikke forutsi neste trekning.                ║
║  • Ca. halvparten av innsatsen betales tilbake: forventet tap er     ║
║    ~50 kr per 100 kr spilt. Dette er underholdning, ikke sparing.    ║
║  • «Beste rekker» = rekker FÅ ANDRE spiller: vinner du, deler du     ║
║    potten med færrest mulig. Det er alt som kan optimaliseres.       ║
╚══════════════════════════════════════════════════════════════════════╝"
    );

    for spill in &valg.spill {
        let odds = spill.kombinasjoner();
        println!("\n━━━ {} — 1 : {} per rekke ━━━", spill.navn(), med_skilletegn(odds));

        // Statistikkdelen krever nedlastet historikk; rekkene gjør ikke.
        let sti = tipping::csv_sti(&valg.mappe, *spill);
        match tipping::les_csv(&sti) {
            Ok(trekninger) if !trekninger.is_empty() => {
                let a = tipping::analyser(*spill, &trekninger)?;
                println!(
                    "Historikk: {} trekninger, {} – {}",
                    a.antall_trekninger, a.forste, a.siste
                );
                let mut etter_antall = a.hovedtall.clone();
                etter_antall.sort_by(|x, y| y.antall.cmp(&x.antall));
                let topp: Vec<String> = etter_antall
                    .iter()
                    .take(10)
                    .map(|s| format!("{} ({}×)", s.tall, s.antall))
                    .collect();
                let bunn: Vec<String> = etter_antall
                    .iter()
                    .rev()
                    .take(10)
                    .map(|s| format!("{} ({}×)", s.tall, s.antall))
                    .collect();
                println!("Oftest trukket:   {}", topp.join("  "));
                println!("Sjeldnest trukket: {}", bunn.join("  "));
                let maks_z = a
                    .hovedtall
                    .iter()
                    .map(|s| s.z.abs())
                    .fold(0.0f64, f64::max);
                println!(
                    "Chi-kvadrat {:.1} (df {}, største avvik {:.1}σ): {}",
                    a.chi2,
                    a.chi2_frihetsgrader,
                    maks_z,
                    if a.innenfor_tilfeldighet {
                        "helt forenlig med ren tilfeldighet — «varme» og «kalde» tall er støy"
                    } else {
                        "større avvik enn ventet — sjekk datakvaliteten før du tolker noe"
                    }
                );

                let hot = tipping::gjenganger_rekke(&a);
                let tall: Vec<String> = hot.hovedtall.iter().map(u8::to_string).collect();
                let e = if hot.ekstra.is_empty() {
                    String::new()
                } else {
                    let e: Vec<String> = hot.ekstra.iter().map(u8::to_string).collect();
                    format!(" + [{}]", e.join(","))
                };
                println!(
                    "Gjenganger-rekka (mest trukne tall): {}{}\n  (samme vinnersjanse som alle andre rekker — og mest delt om den vinner)",
                    tall.join(" "),
                    e
                );
                let gjentak = tipping::gjentatte_rekker(&trekninger);
                let forventet = tipping::forventet_gjentak(a.antall_trekninger, *spill);
                if gjentak.is_empty() {
                    println!(
                        "Gjentatte vinnerrekker: ingen i {} trekninger (ren tilfeldighet forventer {:.2})",
                        a.antall_trekninger, forventet
                    );
                } else {
                    for (rekke, datoer) in gjentak.iter().take(5) {
                        let tall: Vec<String> = rekke.iter().map(u8::to_string).collect();
                        let d: Vec<String> = datoer.iter().map(|d| d.to_string()).collect();
                        println!("Gjentatt vinnerrekke: {} — trukket {}", tall.join(" "), d.join(" og "));
                    }
                    println!("  (forventet ved ren tilfeldighet: {forventet:.2} — sier ingenting om fremtiden)");
                }
            }
            _ => {
                println!(
                    "(Ingen historikk i {} — kjør `b-tipping hent` først, eller legg\n inn CSV manuelt. Rekkene under er uansett gyldige: de bygger ikke på\n historikk, for historikk kan ikke forbedre vinnersjansen.)",
                    sti.display()
                );
            }
        }

        println!("\nDe {} beste rekkene (lavest forventet premiedeling):", valg.rekker);
        for (i, r) in tipping::beste_rekker(*spill, valg.rekker, valg.fro).iter().enumerate() {
            let hoved: Vec<String> = r.hovedtall.iter().map(|t| format!("{t:>2}")).collect();
            let ekstra = if r.ekstra.is_empty() {
                String::new()
            } else {
                let e: Vec<String> = r.ekstra.iter().map(u8::to_string).collect();
                format!("  +[{}]", e.join(","))
            };
            println!(
                "  {:>2}. {}{}   ({})",
                i + 1,
                hoved.join(" "),
                ekstra,
                r.begrunnelse
            );
        }

        println!("\n🧠 AI-panelet diskuterer seg fram til beste rekke:");
        for (taler, tekst) in tipping::paneldiskusjon(*spill, valg.fro).innlegg {
            println!("\n  {taler}:\n    {}", ombryt(&tekst, 72, "    "));
        }
    }

    println!(
        "\nSpiller du, så sett grenser hos Norsk Tipping først. Kjenner du at det\ntar overhånd: Hjelpelinjen 800 800 40 — gratis og anonymt."
    );
    Ok(())
}

/// Enkel ordbryting for terminalen.
fn ombryt(tekst: &str, bredde: usize, innrykk: &str) -> String {
    let mut linjer = Vec::new();
    let mut linje = String::new();
    for ord in tekst.split_whitespace() {
        if !linje.is_empty() && linje.chars().count() + 1 + ord.chars().count() > bredde {
            linjer.push(std::mem::take(&mut linje));
        }
        if !linje.is_empty() {
            linje.push(' ');
        }
        linje.push_str(ord);
    }
    if !linje.is_empty() {
        linjer.push(linje);
    }
    linjer.join(&format!("\n{innrykk}"))
}

use tipping::med_skilletegn;
