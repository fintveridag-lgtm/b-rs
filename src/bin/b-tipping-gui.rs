//! b-tipping-gui — vindusapp for Norsk Tipping-analysen.
//!
//! Samme innhold som `b-tipping`-kommandolinjen, men som egen grafisk app:
//! hent trekningshistorikk med én knapp, se frekvensstatistikk per spill og
//! få «de 10 beste rekkene» — der «best» ærlig talt betyr lavest forventet
//! premiedeling, siden alle rekker har nøyaktig samme vinnersjanse.

#![cfg_attr(windows, windows_subsystem = "windows")]

use b_rs::tipping::{self, Analyse, Rekke, Spill};
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
    analyser: HashMap<&'static str, Analyse>,
    rekker: HashMap<&'static str, Vec<Rekke>>,
    fro: u64,
    status: Arc<Mutex<Option<HentStatus>>>,
    melding: Option<(String, Color32)>,
}

impl TippingApp {
    fn ny() -> Self {
        let fro = chrono::Local::now().date_naive().num_days_from_ce() as u64;
        let mut app = TippingApp {
            rt: tokio::runtime::Runtime::new().expect("tokio-runtime"),
            mappe: PathBuf::from("data/tipping"),
            valgt: Spill::Lotto,
            analyser: HashMap::new(),
            rekker: HashMap::new(),
            fro,
            status: Arc::new(Mutex::new(None)),
            melding: None,
        };
        app.les_historikk();
        app.lag_rekker();
        app
    }

    /// Les CSV-ene fra disk og analyser det som finnes.
    fn les_historikk(&mut self) {
        self.analyser.clear();
        for spill in Spill::ALLE {
            let sti = tipping::csv_sti(&self.mappe, spill);
            if let Ok(trekninger) = tipping::les_csv(&sti) {
                if !trekninger.is_empty() {
                    if let Ok(a) = tipping::analyser(spill, &trekninger) {
                        self.analyser.insert(spill.api_navn(), a);
                    }
                }
            }
        }
    }

    fn lag_rekker(&mut self) {
        for spill in Spill::ALLE {
            self.rekker
                .insert(spill.api_navn(), tipping::beste_rekker(spill, 10, self.fro));
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
                        if let Err(e) = tipping::skriv_csv(&sti, &trekninger) {
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
                        let aktiv = self.valgt == spill;
                        if ui
                            .selectable_label(aktiv, RichText::new(spill.navn()).size(15.0))
                            .clicked()
                        {
                            self.valgt = spill;
                        }
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
                });
            });
    }

    fn innhold(&mut self, ctx: &egui::Context) {
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

    fn statistikk_kort(&self, ui: &mut egui::Ui, spill: Spill) {
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
        });
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

fn med_skilletegn(n: u128) -> String {
    let s = n.to_string();
    let mut ut = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            ut.push(' ');
        }
        ut.push(c);
    }
    ut
}
