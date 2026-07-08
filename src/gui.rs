use crate::state::{Flags, SharedState, UiState};
use crate::types::Side;
use anyhow::Result;
use eframe::egui::{self, Color32, RichText};
use egui_plot::{Line, Plot, PlotPoints};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

const GREEN: Color32 = Color32::from_rgb(34, 197, 94);
const RED: Color32 = Color32::from_rgb(239, 68, 68);
const YELLOW: Color32 = Color32::from_rgb(250, 204, 21);
const BLUE: Color32 = Color32::from_rgb(96, 165, 250);
const GRAY: Color32 = Color32::from_rgb(148, 163, 184);

/// Grafisk vindu med knapper og kursgraf. Blokkerer til vinduet lukkes.
pub fn run(state: SharedState, flags: Arc<Flags>) -> Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1240.0, 820.0])
        .with_min_inner_size([900.0, 600.0])
        .with_title("b-rs — børs-konsoll");
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let app = App {
        state,
        flags: flags.clone(),
        selected: None,
    };
    let result = eframe::run_native("b-rs", options, Box::new(move |_cc| Ok(Box::new(app))));
    flags.quit.store(true, Ordering::Relaxed);
    result.map_err(|e| anyhow::anyhow!("klarte ikke starte GUI: {e}"))
}

/// Samme logo som bygges inn i .exe-filen brukes som vindusikon.
fn load_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../assets/logo.png"))
        .ok()?
        .into_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}

struct App {
    state: SharedState,
    flags: Arc<Flags>,
    selected: Option<String>,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Engine oppdaterer tilstanden i bakgrunnen — tegn på nytt jevnlig.
        ctx.request_repaint_after(Duration::from_millis(500));

        let state = self.state.clone();
        let st = state.lock().unwrap();
        if self.selected.is_none() {
            self.selected = st.quotes.keys().next().cloned();
        }

        self.top_bar(ctx, &st);
        self.left_panel(ctx, &st);
        self.bottom_panel(ctx, &st);
        self.chart_panel(ctx, &st);
    }
}

impl App {
    fn top_bar(&mut self, ctx: &egui::Context, st: &UiState) {
        egui::TopBottomPanel::top("topp").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading(RichText::new("b-rs").strong());
                if st.mode == "live" {
                    ui.label(RichText::new(" LIVE ").color(Color32::WHITE).background_color(RED).strong());
                } else {
                    ui.label(RichText::new(" PAPIR ").color(Color32::BLACK).background_color(GREEN).strong());
                }
                ui.label(format!("megler: {}", st.broker_name));
                ui.separator();
                ui.label(format!("Kontanter: {:.0}", st.cash));
                ui.label(format!("Egenkapital: {:.0}", st.equity));
                let pnl_color = if st.drawdown >= 0.0 { GREEN } else { RED };
                ui.label(RichText::new(format!("P&L: {:+.0}", st.drawdown)).color(pnl_color).strong());
                ui.separator();
                if let Some(ts) = st.last_tick {
                    ui.label(RichText::new(format!("oppdatert {}", ts.format("%H:%M:%S"))).color(GRAY));
                } else {
                    ui.spinner();
                    ui.label(RichText::new("venter på kursdata …").color(GRAY));
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Kill switch — alltid synlig, alltid rød.
                    let killed = self.flags.killed();
                    let kill_text = if killed { "HANDEL STOPPET — trykk for å gjenoppta" } else { "⛔ KILL SWITCH" };
                    let kill_fill = if killed { Color32::from_rgb(120, 20, 20) } else { RED };
                    let kill_btn = egui::Button::new(RichText::new(kill_text).color(Color32::WHITE).strong())
                        .fill(kill_fill);
                    if ui.add(kill_btn).clicked() {
                        self.flags.killed.store(!killed, Ordering::Relaxed);
                    }

                    let paused = self.flags.paused();
                    let pause_text = if paused { "▶ Fortsett strategi" } else { "⏸ Pause strategi" };
                    let pause_btn = egui::Button::new(RichText::new(pause_text).color(Color32::BLACK))
                        .fill(if paused { GREEN } else { YELLOW });
                    if ui.add(pause_btn).clicked() {
                        self.flags.paused.store(!paused, Ordering::Relaxed);
                    }
                });
            });
            ui.add_space(4.0);
        });
    }

    fn left_panel(&mut self, ctx: &egui::Context, st: &UiState) {
        egui::SidePanel::left("venstre")
            .default_width(300.0)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.heading("Watchlist");
                ui.small("Klikk på et symbol for å vise grafen.");
                ui.add_space(4.0);
                egui::Grid::new("watchlist").striped(true).min_col_width(70.0).show(ui, |ui| {
                    ui.label(RichText::new("Symbol").strong());
                    ui.label(RichText::new("Siste").strong());
                    ui.label(RichText::new("Endring").strong());
                    ui.end_row();
                    for q in st.quotes.values() {
                        let selected = self.selected.as_deref() == Some(q.symbol.as_str());
                        if ui.selectable_label(selected, &q.symbol).clicked() {
                            self.selected = Some(q.symbol.clone());
                        }
                        ui.label(format!("{:.2}", q.last));
                        let pct = q.change_pct();
                        let color = if pct >= 0.0 { GREEN } else { RED };
                        ui.label(RichText::new(format!("{pct:+.2} %")).color(color));
                        ui.end_row();
                    }
                });

                ui.add_space(12.0);
                ui.separator();
                ui.heading("Posisjoner");
                if st.positions.is_empty() && st.nordnet_positions.is_empty() {
                    ui.small("Ingen posisjoner ennå.");
                }
                egui::Grid::new("posisjoner").striped(true).min_col_width(55.0).show(ui, |ui| {
                    ui.label(RichText::new("Symbol").strong());
                    ui.label(RichText::new("Antall").strong());
                    ui.label(RichText::new("Verdi").strong());
                    ui.label(RichText::new("Urealisert").strong());
                    ui.end_row();
                    for p in &st.positions {
                        ui.label(&p.symbol);
                        ui.label(format!("{:.0}", p.qty));
                        ui.label(format!("{:.0}", p.market_value()));
                        let u = p.unrealized();
                        let color = if u >= 0.0 { GREEN } else { RED };
                        ui.label(RichText::new(format!("{u:+.0}")).color(color));
                        ui.end_row();
                    }
                    for p in &st.nordnet_positions {
                        let label = if p.symbol == "?" { &p.name } else { &p.symbol };
                        ui.label(RichText::new(format!("[NN] {label}")).color(BLUE));
                        ui.label(format!("{:.0}", p.qty));
                        ui.label(format!("{:.0}", p.market_value));
                        ui.label("–");
                        ui.end_row();
                    }
                });
                if st.nordnet_enabled {
                    ui.small("[NN] = Nordnet-portefølje (kun lesing).");
                }
            });
    }

    fn bottom_panel(&self, ctx: &egui::Context, st: &UiState) {
        egui::TopBottomPanel::bottom("bunn")
            .default_height(220.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.columns(2, |cols| {
                    cols[0].heading("Ordrer");
                    egui::ScrollArea::vertical()
                        .id_salt("ordrer")
                        .show(&mut cols[0], |ui| {
                            egui::Grid::new("ordre_grid").striped(true).min_col_width(50.0).show(ui, |ui| {
                                for t in ["Tid", "Side", "Symbol", "Antall", "Kurs", "Status"] {
                                    ui.label(RichText::new(t).strong());
                                }
                                ui.end_row();
                                for o in &st.orders {
                                    ui.label(o.created.format("%H:%M:%S").to_string());
                                    let color = match o.side {
                                        Side::Buy => GREEN,
                                        Side::Sell => RED,
                                    };
                                    ui.label(RichText::new(o.side.to_string()).color(color));
                                    ui.label(&o.symbol);
                                    ui.label(format!("{:.0}", o.qty));
                                    ui.label(format!("{:.2}", o.avg_price));
                                    ui.label(o.status.to_string());
                                    ui.end_row();
                                }
                            });
                            if st.orders.is_empty() {
                                ui.small("Ingen ordrer ennå — strategien venter på et signal.");
                            }
                        });

                    cols[1].heading("Hendelser");
                    egui::ScrollArea::vertical()
                        .id_salt("logg")
                        .show(&mut cols[1], |ui| {
                            for (ts, msg) in &st.logs {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(RichText::new(ts.format("%H:%M:%S").to_string()).color(GRAY).monospace());
                                    ui.label(RichText::new(msg).small());
                                });
                            }
                        });
                });
            });
    }

    fn chart_panel(&self, ctx: &egui::Context, st: &UiState) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let Some(symbol) = self.selected.clone() else {
                ui.centered_and_justified(|ui| {
                    ui.label("Venter på kursdata …");
                });
                return;
            };
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading(format!("Kursgraf — {symbol}"));
                if let Some(q) = st.quotes.get(&symbol) {
                    let pct = q.change_pct();
                    let color = if pct >= 0.0 { GREEN } else { RED };
                    ui.label(
                        RichText::new(format!("{:.2}  ({pct:+.2} %)", q.last))
                            .color(color)
                            .strong(),
                    );
                }
            });

            let points: PlotPoints = st
                .history
                .get(&symbol)
                .map(|h| h.iter().map(|&(t, p)| [t, p]).collect())
                .unwrap_or_default();

            Plot::new("kursgraf")
                .x_axis_formatter(|mark, _range| {
                    chrono::DateTime::from_timestamp(mark.value as i64, 0)
                        .map(|dt| dt.format("%d.%m").to_string())
                        .unwrap_or_default()
                })
                .label_formatter(|name, value| {
                    let when = chrono::DateTime::from_timestamp(value.x as i64, 0)
                        .map(|dt| dt.format("%d.%m.%Y %H:%M").to_string())
                        .unwrap_or_default();
                    if name.is_empty() {
                        format!("{when}\n{:.2}", value.y)
                    } else {
                        format!("{name}\n{when}\n{:.2}", value.y)
                    }
                })
                .show(ui, |plot_ui| {
                    plot_ui.line(Line::new(points).color(GREEN).width(2.0).name(&symbol));
                });
        });
    }
}
