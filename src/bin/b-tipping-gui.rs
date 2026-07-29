//! b-tipping-gui — vindusapp for Norsk Tipping-analysen.
//!
//! Samme innhold som `b-tipping`-kommandolinjen, men som egen grafisk app:
//! hent trekningshistorikk med én knapp, se frekvensstatistikk per spill og
//! få «de 10 beste rekkene» — der «best» ærlig talt betyr lavest forventet
//! premiedeling, siden alle rekker har nøyaktig samme vinnersjanse.

#![cfg_attr(windows, windows_subsystem = "windows")]

use b_rs::tipping::{self, Analyse, Kupong, PanelResultat, Rekke, Spill, Trekning};
use chrono::Datelike;
use eframe::egui::{self, Color32, RichText};
use egui_plot::{Bar, BarChart, HLine, Plot};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// Samme fargespråk som b-rs-gui: nesten svart med grønn aksent.
const GREEN: Color32 = Color32::from_rgb(0, 196, 106);
const YELLOW: Color32 = Color32::from_rgb(250, 204, 21);
const RED: Color32 = Color32::from_rgb(240, 82, 82);
const GRAY: Color32 = Color32::from_rgb(140, 150, 165);
const BLUE: Color32 = Color32::from_rgb(96, 165, 250);
const BG_PANEL: Color32 = Color32::from_rgb(4, 6, 10);
const BG_DEEP: Color32 = Color32::from_rgb(1, 2, 4);
const BG_CARD: Color32 = Color32::from_rgb(9, 12, 18);
const BORDER: Color32 = Color32::from_rgb(22, 28, 38);
const TEXT_LIGHT: Color32 = Color32::from_rgb(210, 218, 230);

fn main() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1120.0, 780.0])
        .with_min_inner_size([880.0, 600.0])
        .with_title("b-tipping — Norsk Tipping-analyse");
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "b-tipping",
        options,
        Box::new(|cc| {
            apply_theme(&cc.egui_ctx);
            Ok(Box::new(TippingApp::ny()))
        }),
    )
}

/// Status for bakgrunnsnedlastingen, delt med nedlastingstråden.
struct HentStatus {
    melding: String,
    ferdig: bool,
    feil: Vec<String>,
}

struct TippingApp {
    rt: tokio::runtime::Runtime,
    mappe: PathBuf,
    valgt: Spill,
    mine_spill_fane: bool,
    analyser: HashMap<&'static str, Analyse>,
    historikk: HashMap<&'static str, Vec<Trekning>>,
    rekker: HashMap<&'static str, Vec<Rekke>>,
    paneler: HashMap<&'static str, PanelResultat>,
    fro: u64,
    status: Arc<Mutex<Option<HentStatus>>>,
    melding: Option<(String, Color32)>,
    vis_gjengangere: bool,
    vis_panel: bool,
    // Skjema for ny kupong.
    kupong_hoved: String,
    kupong_ekstra: String,
    kupong_innsats: String,
    kupong_gevinst: String,
    kupong_dato: String,
    kupong_spill: Spill,
    kuponger: Vec<Kupong>,
}

impl TippingApp {
    fn ny() -> Self {
        let fro = chrono::Local::now().date_naive().num_days_from_ce() as u64;
        let mut app = TippingApp {
            rt: tokio::runtime::Runtime::new().expect("tokio-runtime"),
            mappe: tipping::standard_mappe(),
            valgt: Spill::Lotto,
            mine_spill_fane: false,
            analyser: HashMap::new(),
            historikk: HashMap::new(),
            rekker: HashMap::new(),
            paneler: HashMap::new(),
            fro,
            status: Arc::new(Mutex::new(None)),
            melding: None,
            vis_gjengangere: false,
            vis_panel: false,
            kupong_hoved: String::new(),
            kupong_ekstra: String::new(),
            kupong_innsats: String::new(),
            kupong_gevinst: String::new(),
            kupong_dato: chrono::Local::now().date_naive().to_string(),
            kupong_spill: Spill::Lotto,
            kuponger: Vec::new(),
        };
        tipping::migrer_gammel_mappe(&app.mappe);
        app.les_historikk();
        app.lag_rekker();
        app.kuponger = tipping::les_kuponger(&tipping::kupong_sti(&app.mappe));
        app
    }

    /// Les CSV-ene fra disk og analyser det som finnes.
    fn les_historikk(&mut self) {
        self.analyser.clear();
        self.historikk.clear();
        for spill in Spill::ALLE {
            let sti = tipping::csv_sti(&self.mappe, spill);
            if let Ok(trekninger) = tipping::les_csv(&sti) {
                if !trekninger.is_empty() {
                    if let Ok(a) = tipping::analyser(spill, &trekninger) {
                        self.analyser.insert(spill.api_navn(), a);
                    }
                    self.historikk.insert(spill.api_navn(), trekninger);
                }
            }
        }
    }

    fn lag_rekker(&mut self) {
        for spill in Spill::ALLE {
            self.rekker
                .insert(spill.api_navn(), tipping::beste_rekker(spill, 10, self.fro));
            self.paneler
                .insert(spill.api_navn(), tipping::paneldiskusjon(spill, self.fro));
        }
    }

    /// Start nedlasting av alle tre spillene i bakgrunnen.
    fn start_henting(&self, ctx: egui::Context) {
        let status = self.status.clone();
        let mappe = self.mappe.clone();
        *status.lock().unwrap() = Some(HentStatus {
            melding: "Kobler til Norsk Tipping …".into(),
            ferdig: false,
            feil: Vec::new(),
        });
        let fra_aar = chrono::Local::now().year() - 30;
        self.rt.spawn(async move {
            let fra_dato = chrono::NaiveDate::from_ymd_opt(fra_aar, 1, 1).unwrap();
            let klient = reqwest::Client::builder()
                .user_agent("b-tipping/1.0 (hobbyprosjekt; resultathistorikk)")
                .timeout(std::time::Duration::from_secs(20))
                .build();
            let Ok(klient) = klient else { return };
            let mut feil = Vec::new();
            for spill in Spill::ALLE {
                sett_melding(&status, format!("Henter {} …", spill.navn()), &ctx);
                let status_kopi = status.clone();
                let ctx_kopi = ctx.clone();
                let resultat = tipping::hent_historikk(
                    &klient,
                    spill,
                    fra_dato,
                    None,
                    move |antall, dato| {
                        if antall % 25 == 0 {
                            sett_melding(
                                &status_kopi,
                                format!("{}: {} trekninger, kommet til {}", spill.navn(), antall, dato),
                                &ctx_kopi,
                            );
                        }
                    },
                )
                .await;
                match resultat {
                    Ok(trekninger) => {
                        let sti = tipping::csv_sti(&mappe, spill);
                        if let Err(e) = tipping::oppdater_csv(&sti, trekninger) {
                            feil.push(format!("{}: {e:#}", spill.navn()));
                        }
                    }
                    Err(e) => feil.push(format!("{}: {e:#}", spill.navn())),
                }
            }
            if let Ok(mut laas) = status.lock() {
                if let Some(st) = laas.as_mut() {
                    st.ferdig = true;
                    st.feil = feil;
                }
            }
            ctx.request_repaint();
        });
    }
}

fn sett_melding(status: &Arc<Mutex<Option<HentStatus>>>, melding: String, ctx: &egui::Context) {
    if let Ok(mut laas) = status.lock() {
        if let Some(st) = laas.as_mut() {
            st.melding = melding;
        }
    }
    ctx.request_repaint();
}

impl eframe::App for TippingApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Plukk opp ferdig nedlasting.
        let ferdig = {
            let mut laas = self.status.lock().unwrap();
            if laas.as_ref().is_some_and(|s| s.ferdig) {
                laas.take()
            } else {
                None
            }
        };
        if let Some(st) = ferdig {
            self.les_historikk();
            self.melding = Some(if st.feil.is_empty() {
                ("Historikk hentet og lagret ✓".into(), GREEN)
            } else {
                (format!("Delvis hentet — feil: {}", st.feil.join(" · ")), RED)
            });
        }
        let henter = self.status.lock().unwrap().as_ref().map(|s| s.melding.clone());
        if henter.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }

        self.topplinje(ctx, henter.as_deref());
        self.bunnlinje(ctx);
        self.innhold(ctx);
    }
}

impl TippingApp {
    fn topplinje(&mut self, ctx: &egui::Context, henter: Option<&str>) {
        egui::TopBottomPanel::top("topp")
            .frame(egui::Frame::default().fill(BG_DEEP).inner_margin(10.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("🎱 b-tipping").size(20.0).strong().color(GREEN));
                    ui.separator();
                    for spill in Spill::ALLE {
                        let aktiv = !self.mine_spill_fane && self.valgt == spill;
                        if ui
                            .selectable_label(aktiv, RichText::new(spill.navn()).size(15.0))
                            .clicked()
                        {
                            self.valgt = spill;
                            self.mine_spill_fane = false;
                        }
                    }
                    ui.separator();
                    if ui
                        .selectable_label(
                            self.mine_spill_fane,
                            RichText::new("🎟 Mine spill").size(15.0),
                        )
                        .clicked()
                    {
                        self.mine_spill_fane = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        match henter {
                            Some(melding) => {
                                ui.spinner();
                                ui.label(RichText::new(melding).color(GRAY));
                            }
                            None => {
                                if ui
                                    .button("⬇ Hent historikk (30 år, alle spill)")
                                    .on_hover_text(
                                        "Laster ned trekning for trekning fra Norsk Tippings \
                                         uoffisielle resultat-API og lagrer som CSV i data/tipping/. \
                                         Tar noen minutter.",
                                    )
                                    .clicked()
                                {
                                    self.melding = None;
                                    self.start_henting(ctx.clone());
                                }
                            }
                        }
                    });
                });
                if let Some((tekst, farge)) = &self.melding {
                    ui.label(RichText::new(tekst).color(*farge));
                }
            });
    }

    fn bunnlinje(&self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("bunn")
            .frame(egui::Frame::default().fill(BG_DEEP).inner_margin(8.0))
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(
                            "Forventet tap: ~50 kr per 100 kr spilt. Dette er underholdning, \
                             ikke sparing. Sett grenser hos Norsk Tipping — og tar det overhånd: \
                             Hjelpelinjen 800 800 40 (gratis og anonymt).",
                        )
                        .color(GRAY)
                        .size(12.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(tipping::KILDE_VERSJON).color(BORDER).size(11.0));
                    });
                });
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("💾 Historikk lagres permanent i: {}", self.mappe.display()))
                            .color(GRAY)
                            .size(11.0),
                    );
                    if ui
                        .small_button("Åpne mappe")
                        .on_hover_text("Åpne mappen der trekningshistorikken lagres")
                        .clicked()
                    {
                        aapne_mappe(&self.mappe);
                    }
                });
            });
    }

    fn innhold(&mut self, ctx: &egui::Context) {
        if self.mine_spill_fane {
            self.mine_spill_innhold(ctx);
            return;
        }
        let spill = self.valgt;
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(BG_PANEL).inner_margin(12.0))
            .show(ctx, |ui| {
                // Ærlighetsplakaten — alltid synlig, alltid først.
                kort(ui, |ui| {
                    ui.label(RichText::new("Ærlig påminnelse").strong().color(YELLOW));
                    ui.label(RichText::new(format!(
                        "Alle rekker har nøyaktig samme vinnersjanse: 1 : {} for førstepremien i {}. \
                         Historikken kan ikke forutsi neste trekning. Det eneste som kan optimaliseres \
                         er premiedeling — å velge rekker få andre spiller, så du deler potten med \
                         færrest mulig hvis du først vinner.",
                        med_skilletegn(spill.kombinasjoner()),
                        spill.navn()
                    ))
                    .color(TEXT_LIGHT));
                });
                ui.add_space(8.0);

                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.columns(2, |kolonner| {
                        self.statistikk_kort(&mut kolonner[0], spill);
                        self.rekker_kort(&mut kolonner[1], spill);
                    });
                });
            });
    }

    fn statistikk_kort(&mut self, ui: &mut egui::Ui, spill: Spill) {
        let mut veksle_gjengangere = false;
        let vis_gjengangere = self.vis_gjengangere;
        kort(ui, |ui| {
            ui.label(RichText::new("Trekningshistorikk").strong().size(16.0));
            let Some(a) = self.analyser.get(spill.api_navn()) else {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Ingen historikk lastet ned ennå.\n\nTrykk «Hent historikk» øverst, \
                         så lastes inntil 30 år med trekninger ned og lagres lokalt. \
                         Analysen virker deretter uten nett.",
                    )
                    .color(GRAY),
                );
                return;
            };

            ui.label(
                RichText::new(format!(
                    "{} trekninger · {} – {}",
                    a.antall_trekninger, a.forste, a.siste
                ))
                .color(GRAY),
            );
            ui.add_space(6.0);

            // Frekvensgraf: én serie, én farge; grå referanselinje = forventet.
            let forventet = a.hovedtall.first().map(|s| s.forventet).unwrap_or(0.0);
            let stolper: Vec<Bar> = a
                .hovedtall
                .iter()
                .map(|s| {
                    Bar::new(s.tall as f64, s.antall as f64)
                        .width(0.7)
                        .fill(GREEN)
                        .name(format!("Tall {}: {}× (forventet {:.0})", s.tall, s.antall, s.forventet))
                })
                .collect();
            Plot::new("frekvens")
                .height(230.0)
                .allow_drag(false)
                .allow_zoom(false)
                .allow_scroll(false)
                .allow_boxed_zoom(false)
                .show_y(false)
                .label_formatter(|_, _| String::new())
                .show(ui, |plot_ui| {
                    plot_ui.bar_chart(BarChart::new(stolper).name("Antall ganger trukket"));
                    plot_ui.hline(
                        HLine::new(forventet)
                            .color(GRAY)
                            .style(egui_plot::LineStyle::dashed_loose())
                            .name("Forventet ved ren tilfeldighet"),
                    );
                });
            ui.label(
                RichText::new(format!(
                    "Antall ganger hvert hovedtall (1–{}) er trukket. Stiplet linje: forventet ({:.0}).",
                    spill.hovedtall_maks(),
                    forventet
                ))
                .color(GRAY)
                .size(12.0),
            );
            ui.add_space(8.0);

            let mut etter_antall = a.hovedtall.clone();
            etter_antall.sort_by(|x, y| y.antall.cmp(&x.antall));
            let topp: Vec<String> =
                etter_antall.iter().take(5).map(|s| format!("{} ({}×)", s.tall, s.antall)).collect();
            let bunn: Vec<String> = etter_antall
                .iter()
                .rev()
                .take(5)
                .map(|s| format!("{} ({}×)", s.tall, s.antall))
                .collect();
            ui.label(format!("Oftest: {}", topp.join("  ")));
            ui.label(format!("Sjeldnest: {}", bunn.join("  ")));
            ui.add_space(8.0);

            let (dom, farge) = if a.innenfor_tilfeldighet {
                (
                    "Avvikene er helt forenlige med ren tilfeldighet — «varme» og «kalde» tall \
                     er støy, ikke signal.",
                    GREEN,
                )
            } else {
                (
                    "Større avvik enn ren tilfeldighet skulle tilsi — sjekk datakvaliteten \
                     (regelendringer i historikken?) før du tolker noe som helst.",
                    YELLOW,
                )
            };
            ui.label(
                RichText::new(format!(
                    "Chi-kvadrat {:.1} (df {}): {}",
                    a.chi2, a.chi2_frihetsgrader, dom
                ))
                .color(farge),
            );

            ui.add_space(8.0);
            let knapp = if vis_gjengangere {
                "🔁 Skjul gjenganger-analysen"
            } else {
                "🔁 Gjenganger-analyse"
            };
            if ui
                .button(knapp)
                .on_hover_text(
                    "Setter de mest trukne tallene sammen til én rekke, og sjekker om \
                     noen vinnerrekke faktisk har gjentatt seg i historikken.",
                )
                .clicked()
            {
                veksle_gjengangere = true;
            }
            if vis_gjengangere {
                let hot = tipping::gjenganger_rekke(a);
                ui.add_space(4.0);
                ui.label(RichText::new("Gjenganger-rekka (mest trukne tall):").strong());
                ui.horizontal(|ui| {
                    let tall: Vec<String> =
                        hot.hovedtall.iter().map(|t| format!("{t:>2}")).collect();
                    ui.label(
                        RichText::new(tall.join("  "))
                            .monospace()
                            .size(16.0)
                            .color(TEXT_LIGHT),
                    );
                    if !hot.ekstra.is_empty() {
                        let e: Vec<String> = hot.ekstra.iter().map(u8::to_string).collect();
                        ui.label(
                            RichText::new(format!("+ {}", e.join(" ")))
                                .monospace()
                                .size(16.0)
                                .color(GREEN),
                        );
                    }
                });
                ui.label(
                    RichText::new(format!(
                        "Ærlig talt: denne rekka har nøyaktig samme vinnersjanse \
                         (1 : {}) som alle andre — chi-kvadraten over viser at \
                         frekvensforskjellene er støy. Og «varme» tall spilles av \
                         mange, så vinner den, deler du trolig potten med flere. \
                         Rekkene til høyre er derfor det smartere valget.",
                        med_skilletegn(spill.kombinasjoner())
                    ))
                    .color(YELLOW)
                    .size(12.0),
                );

                ui.add_space(6.0);
                ui.label(RichText::new("Har en vinnerrekke gjentatt seg?").strong());
                if let Some(trekninger) = self.historikk.get(spill.api_navn()) {
                    let gjentak = tipping::gjentatte_rekker(trekninger);
                    let forventet = tipping::forventet_gjentak(a.antall_trekninger, spill);
                    if gjentak.is_empty() {
                        ui.label(
                            RichText::new(format!(
                                "Nei — aldri i de {} trekningene som er lastet. Ren \
                                 tilfeldighet forventer {:.2} gjentak i et så lite utvalg \
                                 av {} mulige rekker, så dette er akkurat som ventet.",
                                a.antall_trekninger,
                                forventet,
                                med_skilletegn(spill.kombinasjoner())
                            ))
                            .color(GRAY)
                            .size(12.0),
                        );
                    } else {
                        for (rekke, datoer) in gjentak.iter().take(5) {
                            let tall: Vec<String> = rekke.iter().map(u8::to_string).collect();
                            let d: Vec<String> =
                                datoer.iter().map(|d| d.to_string()).collect();
                            ui.label(
                                RichText::new(format!(
                                    "Ja! {} — trukket {}",
                                    tall.join(" "),
                                    d.join(" og ")
                                ))
                                .color(GREEN)
                                .size(12.0),
                            );
                        }
                        ui.label(
                            RichText::new(format!(
                                "(Forventet ved ren tilfeldighet: {:.2} — et gjentak gjør \
                                 uansett ikke rekka mer eller mindre sannsynlig fremover.)",
                                tipping::forventet_gjentak(a.antall_trekninger, spill)
                            ))
                            .color(GRAY)
                            .size(12.0),
                        );
                    }
                }
            }
        });
        if veksle_gjengangere {
            self.vis_gjengangere = !self.vis_gjengangere;
        }
    }

    fn rekker_kort(&mut self, ui: &mut egui::Ui, spill: Spill) {
        kort(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("De 10 beste rekkene").strong().size(16.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("🎲 Nye rekker")
                        .on_hover_text("Trekker et nytt sett like gode rekker (nytt frø).")
                        .clicked()
                    {
                        self.fro = self.fro.wrapping_mul(6364136223846793005).wrapping_add(1);
                        self.lag_rekker();
                    }
                    if ui.button("📋 Kopier").clicked() {
                        if let Some(rekker) = self.rekker.get(spill.api_navn()) {
                            let tekst: Vec<String> = rekker
                                .iter()
                                .map(|r| {
                                    let hoved: Vec<String> =
                                        r.hovedtall.iter().map(u8::to_string).collect();
                                    if r.ekstra.is_empty() {
                                        hoved.join(" ")
                                    } else {
                                        let e: Vec<String> =
                                            r.ekstra.iter().map(u8::to_string).collect();
                                        format!("{} + {}", hoved.join(" "), e.join(" "))
                                    }
                                })
                                .collect();
                            ui.ctx().copy_text(tekst.join("\n"));
                        }
                    }
                });
            });
            ui.label(
                RichText::new(
                    "Lavest forventet premiedeling: unngår fødselsdagstall, «lykketall», \
                     rekkefølger og mønstre mange spiller.",
                )
                .color(GRAY)
                .size(12.0),
            );

            let panel_knapp = if self.vis_panel {
                "🧠 Skjul AI-panelet"
            } else {
                "🧠 La AI-panelet diskutere"
            };
            if ui
                .button(panel_knapp)
                .on_hover_text(
                    "Flere AI-stemmer resonnerer seg fram til beste rekke — ærlig om at \
                     vinnersjansen er lik for alle, og at bare premiedeling kan optimaliseres.",
                )
                .clicked()
            {
                self.vis_panel = !self.vis_panel;
            }
            if self.vis_panel {
                if let Some(panel) = self.paneler.get(spill.api_navn()) {
                    egui::Frame::default()
                        .fill(BG_DEEP)
                        .rounding(8.0)
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            for (taler, tekst) in &panel.innlegg {
                                ui.label(RichText::new(taler).strong().color(BLUE).size(13.0));
                                ui.label(RichText::new(tekst).color(TEXT_LIGHT).size(12.0));
                                ui.add_space(5.0);
                            }
                        });
                }
            }
            ui.add_space(6.0);

            let Some(rekker) = self.rekker.get(spill.api_navn()) else {
                return;
            };
            for (i, r) in rekker.iter().enumerate() {
                egui::Frame::default()
                    .fill(BG_DEEP)
                    .rounding(8.0)
                    .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{:>2}.", i + 1)).color(GRAY).monospace());
                            let hoved: Vec<String> =
                                r.hovedtall.iter().map(|t| format!("{t:>2}")).collect();
                            ui.label(
                                RichText::new(hoved.join("  "))
                                    .monospace()
                                    .size(16.0)
                                    .color(TEXT_LIGHT),
                            );
                            if !r.ekstra.is_empty() {
                                let e: Vec<String> = r.ekstra.iter().map(u8::to_string).collect();
                                ui.label(
                                    RichText::new(format!("+ {}", e.join(" ")))
                                        .monospace()
                                        .size(16.0)
                                        .color(GREEN),
                                );
                            }
                        });
                        ui.label(RichText::new(&r.begrunnelse).color(GRAY).size(11.0));
                    });
                ui.add_space(4.0);
            }
        });
    }
}

/// Kort med ramme og diskret skygge, samme uttrykk som resten av appen.
fn kort(ui: &mut egui::Ui, innhold: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::default()
        .fill(BG_CARD)
        .rounding(10.0)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .inner_margin(12.0)
        .show(ui, innhold);
}

fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG_PANEL;
    visuals.window_fill = BG_PANEL;
    visuals.extreme_bg_color = BG_DEEP;
    visuals.faint_bg_color = BG_CARD;
    visuals.selection.bg_fill = Color32::from_rgb(14, 92, 53);
    visuals.hyperlink_color = GREEN;
    let rounding = egui::Rounding::same(8.0);
    for w in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        w.rounding = rounding;
    }
    visuals.widgets.noninteractive.bg_stroke.color = BORDER;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, GREEN);
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.2, GREEN);
    ctx.set_visuals(visuals);
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    ctx.set_style(style);
}

fn load_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../../assets/logo.png"))
        .ok()?
        .into_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width, height })
}

impl TippingApp {
    /// «Mine spill»-fanen: registrer egne kuponger og se ditt reelle resultat.
    fn mine_spill_innhold(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(BG_PANEL).inner_margin(12.0))
            .show(ctx, |ui| {
                kort(ui, |ui| {
                    ui.label(RichText::new("🎟 Mine spill").strong().size(18.0));
                    ui.label(
                        RichText::new(
                            "Før inn kupongene du faktisk har spilt, så regner appen ut ditt \
                             ekte resultat over tid — og sjekker automatisk mot trekningene du \
                             har lastet ned. Ærlig fasit på hva spillingen koster.",
                        )
                        .color(GRAY)
                        .size(12.0),
                    );
                });
                ui.add_space(8.0);

                // Skjema for ny kupong.
                kort(ui, |ui| {
                    ui.label(RichText::new("Registrer ny kupong").strong());
                    ui.add_space(4.0);
                    egui::Grid::new("kupong_skjema").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                        ui.label("Spill:");
                        egui::ComboBox::from_id_salt("kupong_spill")
                            .selected_text(self.kupong_spill.navn())
                            .show_ui(ui, |ui| {
                                for s in Spill::ALLE {
                                    ui.selectable_value(&mut self.kupong_spill, s, s.navn());
                                }
                            });
                        ui.end_row();

                        ui.label("Dato (ÅÅÅÅ-MM-DD):");
                        ui.text_edit_singleline(&mut self.kupong_dato);
                        ui.end_row();

                        ui.label(format!("Hovedtall ({} stk):", self.kupong_spill.hovedtall_antall()));
                        ui.text_edit_singleline(&mut self.kupong_hoved);
                        ui.end_row();

                        if self.kupong_spill.ekstra_antall() > 0 {
                            let navn = match self.kupong_spill {
                                Spill::Vikinglotto => "Vikingtall",
                                Spill::Eurojackpot => "Stjernetall",
                                Spill::Lotto => "Tilleggstall",
                            };
                            ui.label(format!("{navn}:"));
                            ui.text_edit_singleline(&mut self.kupong_ekstra);
                            ui.end_row();
                        }

                        ui.label("Innsats (kr):");
                        ui.text_edit_singleline(&mut self.kupong_innsats);
                        ui.end_row();

                        ui.label("Gevinst (kr, valgfritt):");
                        ui.text_edit_singleline(&mut self.kupong_gevinst);
                        ui.end_row();
                    });
                    ui.add_space(4.0);
                    if ui.button("➕ Legg til kupong").clicked() {
                        self.legg_til_kupong();
                    }
                    ui.label(
                        RichText::new("Tall skilles med mellomrom eller komma, f.eks. «3 7 11 16 18 24 30».")
                            .color(GRAY)
                            .size(11.0),
                    );
                });
                ui.add_space(8.0);

                // Totaler.
                let innsats: f64 = self.kuponger.iter().map(|k| k.innsats_kr).sum();
                let gevinst: f64 = self.kuponger.iter().map(|k| k.gevinst_kr).sum();
                let netto = gevinst - innsats;
                kort(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Spilt for: {innsats:.0} kr")).size(14.0));
                        ui.separator();
                        ui.label(RichText::new(format!("Vunnet: {gevinst:.0} kr")).size(14.0));
                        ui.separator();
                        let (tekst, farge) = if netto >= 0.0 {
                            (format!("Netto: +{netto:.0} kr"), GREEN)
                        } else {
                            (format!("Netto: {netto:.0} kr"), RED)
                        };
                        ui.label(RichText::new(tekst).strong().size(15.0).color(farge));
                    });
                    if !self.kuponger.is_empty() && gevinst < innsats {
                        ui.label(
                            RichText::new(
                                "Akkurat som matematikken forutsier: over tid taper alle på lotteri. \
                                 Det er prisen for spenningen — hold den innenfor et budsjett.",
                            )
                            .color(GRAY)
                            .size(11.0),
                        );
                    }
                });
                ui.add_space(8.0);

                // Liste over kuponger med automatisk resultatsjekk.
                if self.kuponger.is_empty() {
                    return;
                }
                let mut slett: Option<usize> = None;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    // Nyeste øverst.
                    for (idx, k) in self.kuponger.iter().enumerate().rev() {
                        kort(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(k.spill.navn()).strong().color(BLUE));
                                ui.label(RichText::new(k.dato.to_string()).color(GRAY));
                                let hoved: Vec<String> = k.hovedtall.iter().map(u8::to_string).collect();
                                let mut tall = hoved.join(" ");
                                if !k.ekstra.is_empty() {
                                    let e: Vec<String> = k.ekstra.iter().map(u8::to_string).collect();
                                    tall.push_str(&format!("  + {}", e.join(" ")));
                                }
                                ui.label(RichText::new(tall).monospace());
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button("🗑").on_hover_text("Slett kupongen").clicked() {
                                        slett = Some(idx);
                                    }
                                });
                            });
                            // Resultatsjekk mot nedlastet historikk.
                            let historikk = self.historikk.get(k.spill.api_navn());
                            match historikk.and_then(|h| tipping::vurder_kupong(k, h)) {
                                Some(r) => {
                                    let tekst = match &r.premie {
                                        Some(p) => format!(
                                            "✓ {} hovedtreff{} — GEVINST: {}",
                                            r.hovedtreff,
                                            if r.ekstratreff > 0 {
                                                format!(" + {} ekstra", r.ekstratreff)
                                            } else {
                                                String::new()
                                            },
                                            p
                                        ),
                                        None => format!(
                                            "{} hovedtreff{} — ingen premie",
                                            r.hovedtreff,
                                            if r.ekstratreff > 0 {
                                                format!(" + {} ekstra", r.ekstratreff)
                                            } else {
                                                String::new()
                                            }
                                        ),
                                    };
                                    let farge = if r.premie.is_some() { GREEN } else { GRAY };
                                    ui.label(RichText::new(tekst).color(farge).size(12.0));
                                }
                                None => {
                                    ui.label(
                                        RichText::new(
                                            "Trekningen for denne datoen er ikke lastet ned ennå — \
                                             hent historikk for å se resultatet.",
                                        )
                                        .color(GRAY)
                                        .size(11.0),
                                    );
                                }
                            }
                            ui.label(
                                RichText::new(format!(
                                    "Innsats {:.0} kr · gevinst {:.0} kr",
                                    k.innsats_kr, k.gevinst_kr
                                ))
                                .color(GRAY)
                                .size(11.0),
                            );
                        });
                        ui.add_space(4.0);
                    }
                });
                if let Some(idx) = slett {
                    self.kuponger.remove(idx);
                    let _ = tipping::skriv_kuponger(&tipping::kupong_sti(&self.mappe), &self.kuponger);
                }
            });
    }

    /// Tolk skjemaet og lagre en ny kupong.
    fn legg_til_kupong(&mut self) {
        let dato = match chrono::NaiveDate::parse_from_str(self.kupong_dato.trim(), "%Y-%m-%d") {
            Ok(d) => d,
            Err(_) => {
                self.melding = Some(("Ugyldig dato — bruk ÅÅÅÅ-MM-DD".into(), RED));
                return;
            }
        };
        let parse_tall = |s: &str| -> Vec<u8> {
            s.split(|c: char| !c.is_ascii_digit())
                .filter_map(|d| d.trim().parse::<u8>().ok())
                .collect()
        };
        let hovedtall = parse_tall(&self.kupong_hoved);
        if hovedtall.len() != self.kupong_spill.hovedtall_antall() {
            self.melding = Some((
                format!(
                    "{} skal ha {} hovedtall (du skrev {})",
                    self.kupong_spill.navn(),
                    self.kupong_spill.hovedtall_antall(),
                    hovedtall.len()
                ),
                RED,
            ));
            return;
        }
        let kupong = Kupong {
            dato,
            spill: self.kupong_spill,
            hovedtall,
            ekstra: parse_tall(&self.kupong_ekstra),
            innsats_kr: self.kupong_innsats.trim().replace(',', ".").parse().unwrap_or(0.0),
            gevinst_kr: self.kupong_gevinst.trim().replace(',', ".").parse().unwrap_or(0.0),
        };
        let sti = tipping::kupong_sti(&self.mappe);
        match tipping::legg_til_kupong(&sti, kupong) {
            Ok(()) => {
                self.kuponger = tipping::les_kuponger(&sti);
                self.kupong_hoved.clear();
                self.kupong_ekstra.clear();
                self.kupong_innsats.clear();
                self.kupong_gevinst.clear();
                self.melding = Some(("Kupong lagret ✓".into(), GREEN));
            }
            Err(e) => self.melding = Some((format!("Kunne ikke lagre: {e:#}"), RED)),
        }
    }
}

/// Åpne en mappe i systemets filutforsker (best effort, feil ignoreres).
fn aapne_mappe(mappe: &std::path::Path) {
    let _ = std::fs::create_dir_all(mappe);
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(mappe).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(mappe).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(mappe).spawn();
}

use b_rs::tipping::med_skilletegn;
