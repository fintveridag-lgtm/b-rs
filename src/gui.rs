use crate::backtest::{self, BacktestResult};
use crate::state::{Flags, SharedState, UiState};
use crate::strategy;
use crate::types::Side;
use anyhow::Result;
use eframe::egui::{self, Color32, RichText};
use egui_plot::{BoxElem, BoxPlot, BoxSpread, Legend, Line, Plot, PlotPoints};
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

const GREEN: Color32 = Color32::from_rgb(34, 197, 94);
const RED: Color32 = Color32::from_rgb(239, 68, 68);
const YELLOW: Color32 = Color32::from_rgb(250, 204, 21);
const BLUE: Color32 = Color32::from_rgb(96, 165, 250);
const GRAY: Color32 = Color32::from_rgb(148, 163, 184);
const BG_PANEL: Color32 = Color32::from_rgb(13, 22, 38);
const BG_DEEP: Color32 = Color32::from_rgb(9, 15, 27);
const BG_CARD: Color32 = Color32::from_rgb(20, 32, 54);
const BORDER: Color32 = Color32::from_rgb(35, 52, 80);

#[derive(Clone, Copy, PartialEq)]
enum ChartStyle {
    Line,
    Candles,
}

#[derive(Clone, Copy, PartialEq)]
enum ChartRange {
    Week,
    Month,
    ThreeMonths,
    All,
}

impl ChartRange {
    fn seconds(self) -> Option<f64> {
        match self {
            ChartRange::Week => Some(7.0 * 86400.0),
            ChartRange::Month => Some(30.0 * 86400.0),
            ChartRange::ThreeMonths => Some(92.0 * 86400.0),
            ChartRange::All => None,
        }
    }
}

/// Grafisk vindu med knapper og kursgraf. Blokkerer til vinduet lukkes.
pub fn run(state: SharedState, flags: Arc<Flags>) -> Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 850.0])
        .with_min_inner_size([980.0, 640.0])
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
        range: ChartRange::ThreeMonths,
        style: ChartStyle::Line,
        trade_qty: 10.0,
        strategy_choice: String::new(),
        backtest: None,
    };
    let result = eframe::run_native(
        "b-rs",
        options,
        Box::new(move |cc| {
            apply_theme(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    );
    flags.quit.store(true, Ordering::Relaxed);
    result.map_err(|e| anyhow::anyhow!("klarte ikke starte GUI: {e}"))
}

/// Mørkt tema i samme farger som logoen: dyp navy med grønne aksenter.
fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG_PANEL;
    visuals.window_fill = BG_PANEL;
    visuals.extreme_bg_color = BG_DEEP;
    visuals.faint_bg_color = BG_CARD;
    visuals.widgets.noninteractive.bg_stroke.color = BORDER;
    visuals.selection.bg_fill = Color32::from_rgb(17, 94, 51);
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    ctx.set_style(style);
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
    range: ChartRange,
    style: ChartStyle,
    trade_qty: f64,
    strategy_choice: String,
    backtest: Option<std::result::Result<BacktestResult, String>>,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Engine oppdaterer tilstanden i bakgrunnen — tegn på nytt jevnlig.
        ctx.request_repaint_after(Duration::from_millis(500));

        let state = self.state.clone();
        let mut st = state.lock().unwrap();
        if self.selected.is_none() {
            self.selected = st.quotes.keys().next().cloned();
        }
        if self.strategy_choice.is_empty() && !st.strategy_name.is_empty() {
            self.strategy_choice = st.strategy_name.clone();
        }

        self.top_bar(ctx, &st);
        self.left_panel(ctx, &mut st);
        self.bottom_panel(ctx, &st);
        self.chart_panel(ctx, &st);
    }
}

impl App {
    fn top_bar(&mut self, ctx: &egui::Context, st: &UiState) {
        egui::TopBottomPanel::top("topp").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading(RichText::new("b-rs").strong().size(24.0));
                if st.mode == "live" {
                    badge(ui, " LIVE ", Color32::WHITE, RED);
                } else {
                    badge(ui, " PAPIR ", Color32::BLACK, GREEN);
                }
                ui.label(RichText::new(format!("megler: {}", st.broker_name)).color(GRAY));
                ui.separator();
                if let Some(ts) = st.last_tick {
                    ui.label(RichText::new(format!("oppdatert {}", ts.format("%H:%M:%S"))).color(GRAY));
                } else {
                    ui.spinner();
                    ui.label(RichText::new("henter kursdata …").color(GRAY));
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let killed = self.flags.killed();
                    let kill_text = if killed { "HANDEL STOPPET — klikk for å gjenoppta" } else { "⛔ KILL SWITCH" };
                    let kill_fill = if killed { Color32::from_rgb(120, 20, 20) } else { RED };
                    if ui
                        .add(egui::Button::new(RichText::new(kill_text).color(Color32::WHITE).strong()).fill(kill_fill))
                        .clicked()
                    {
                        self.flags.killed.store(!killed, Ordering::Relaxed);
                    }

                    let paused = self.flags.paused();
                    let pause_text = if paused { "▶ Fortsett strategi" } else { "⏸ Pause strategi" };
                    if ui
                        .add(egui::Button::new(RichText::new(pause_text).color(Color32::BLACK)).fill(if paused { GREEN } else { YELLOW }))
                        .clicked()
                    {
                        self.flags.paused.store(!paused, Ordering::Relaxed);
                    }
                });
            });

            // Nøkkeltall-kort
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                stat_card(ui, "Kontanter", format!("{} kr", fmt_thousands(st.cash)), Color32::WHITE);
                stat_card(ui, "Egenkapital", format!("{} kr", fmt_thousands(st.equity)), Color32::WHITE);
                let pnl_color = if st.drawdown >= 0.0 { GREEN } else { RED };
                let sign = if st.drawdown >= 0.0 { "+" } else { "" };
                stat_card(ui, "P&L siden start", format!("{sign}{} kr", fmt_thousands(st.drawdown)), pnl_color);
                stat_card(ui, "Posisjoner", format!("{}", st.positions.len()), BLUE);
                let status = if self.flags.killed() {
                    ("STOPPET", RED)
                } else if self.flags.paused() {
                    ("PAUSE", YELLOW)
                } else {
                    ("AKTIV", GREEN)
                };
                stat_card(ui, "Strategi", status.0.to_string(), status.1);
            });
            ui.add_space(6.0);
        });
    }

    fn left_panel(&mut self, ctx: &egui::Context, st: &mut UiState) {
        egui::SidePanel::left("venstre")
            .default_width(310.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().id_salt("venstre_scroll").show(ui, |ui| {
                ui.add_space(6.0);
                section_heading(ui, "📋 Watchlist");
                ui.small("Klikk på et symbol for å vise grafen.");
                ui.add_space(4.0);
                egui::Grid::new("watchlist").striped(true).min_col_width(72.0).show(ui, |ui| {
                    ui.label(RichText::new("Symbol").strong().color(GRAY));
                    ui.label(RichText::new("Siste").strong().color(GRAY));
                    ui.label(RichText::new("Endring").strong().color(GRAY));
                    ui.end_row();
                    for q in st.quotes.values() {
                        let selected = self.selected.as_deref() == Some(q.symbol.as_str());
                        if ui.selectable_label(selected, RichText::new(&q.symbol).strong()).clicked() {
                            self.selected = Some(q.symbol.clone());
                        }
                        ui.label(format!("{:.2}", q.last));
                        let pct = q.change_pct();
                        let color = if pct >= 0.0 { GREEN } else { RED };
                        ui.label(RichText::new(format!("{pct:+.2} %")).color(color));
                        ui.end_row();
                    }
                });
                if st.quotes.is_empty() {
                    ui.spinner();
                }

                // Strategivalg og backtesting.
                ui.add_space(10.0);
                ui.separator();
                section_heading(ui, "🧠 Strategi");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("strategivalg")
                        .selected_text(&self.strategy_choice)
                        .show_ui(ui, |ui| {
                            for name in strategy::AVAILABLE {
                                ui.selectable_value(&mut self.strategy_choice, name.to_string(), name);
                            }
                        });
                    let active = self.strategy_choice == st.strategy_name;
                    if active {
                        ui.label(RichText::new("aktiv").color(GREEN).small());
                    } else if ui.button("Aktiver").clicked() {
                        st.strategy_request = Some(self.strategy_choice.clone());
                    }
                });
                if ui.button("🧪 Backtest på valgt symbol (3 mnd)").clicked() {
                    self.backtest = Some(run_backtest(self.selected.as_deref(), &self.strategy_choice, st));
                }
                match &self.backtest {
                    Some(Ok(r)) => {
                        ui.label(
                            RichText::new(format!("{} på {}", r.strategy, r.symbol)).strong(),
                        );
                        let color = if r.return_pct >= 0.0 { GREEN } else { RED };
                        ui.label(RichText::new(format!("Avkastning: {:+.1} %", r.return_pct)).color(color).strong());
                        let bh_color = if r.buy_hold_pct >= 0.0 { GREEN } else { RED };
                        ui.horizontal(|ui| {
                            ui.label("Kjøp-og-hold:");
                            ui.label(RichText::new(format!("{:+.1} %", r.buy_hold_pct)).color(bh_color));
                        });
                        let wins = r.wins();
                        ui.label(format!(
                            "{} handler, {} med gevinst{}",
                            r.trades.len(),
                            wins,
                            if r.open_entry.is_some() { " (én fortsatt åpen)" } else { "" }
                        ));
                        ui.small("Forenklet: uten kurtasje og glidning.");
                    }
                    Some(Err(e)) => {
                        ui.label(RichText::new(e).color(RED).small());
                    }
                    None => {
                        ui.small("Test strategien på historikken før du lar den handle.");
                    }
                }

                // Hurtighandel — manuelle ordrer på valgt symbol.
                ui.add_space(10.0);
                ui.separator();
                section_heading(ui, "⚡ Hurtighandel");
                if let Some(symbol) = self.selected.clone() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&symbol).strong());
                        ui.label("antall:");
                        ui.add(
                            egui::DragValue::new(&mut self.trade_qty)
                                .range(1.0..=1_000_000.0)
                                .speed(1)
                                .max_decimals(0),
                        );
                    });
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new(RichText::new("KJØP").color(Color32::BLACK).strong()).fill(GREEN))
                            .clicked()
                        {
                            st.manual_orders.push_back((symbol.clone(), Side::Buy, self.trade_qty));
                            st.log(format!("Manuell KJØP {symbol} x{:.0} lagt i kø.", self.trade_qty));
                        }
                        if ui
                            .add(egui::Button::new(RichText::new("SELG").color(Color32::WHITE).strong()).fill(RED))
                            .clicked()
                        {
                            st.manual_orders.push_back((symbol.clone(), Side::Sell, self.trade_qty));
                            st.log(format!("Manuell SELG {symbol} x{:.0} lagt i kø.", self.trade_qty));
                        }
                    });
                    ui.small("Utføres på neste tikk, gjennom risikoreglene.");
                } else {
                    ui.small("Velg et symbol i watchlisten først.");
                }

                ui.add_space(10.0);
                ui.separator();
                section_heading(ui, "💼 Posisjoner");
                if st.positions.is_empty() && st.nordnet_positions.is_empty() {
                    ui.small("Ingen posisjoner ennå.");
                }
                egui::Grid::new("posisjoner").striped(true).min_col_width(58.0).show(ui, |ui| {
                    ui.label(RichText::new("Symbol").strong().color(GRAY));
                    ui.label(RichText::new("Antall").strong().color(GRAY));
                    ui.label(RichText::new("Verdi").strong().color(GRAY));
                    ui.label(RichText::new("Urealisert").strong().color(GRAY));
                    ui.end_row();
                    for p in &st.positions {
                        ui.label(&p.symbol);
                        ui.label(format!("{:.0}", p.qty));
                        ui.label(fmt_thousands(p.market_value()));
                        let u = p.unrealized();
                        let color = if u >= 0.0 { GREEN } else { RED };
                        ui.label(RichText::new(format!("{u:+.0}")).color(color));
                        ui.end_row();
                    }
                    for p in &st.nordnet_positions {
                        let label = if p.symbol == "?" { &p.name } else { &p.symbol };
                        ui.label(RichText::new(format!("[NN] {label}")).color(BLUE));
                        ui.label(format!("{:.0}", p.qty));
                        ui.label(fmt_thousands(p.market_value));
                        ui.label("–");
                        ui.end_row();
                    }
                });
                if st.nordnet_enabled {
                    ui.small("[NN] = Nordnet-portefølje (kun lesing).");
                }
                });
            });
    }

    fn bottom_panel(&self, ctx: &egui::Context, st: &UiState) {
        egui::TopBottomPanel::bottom("bunn")
            .default_height(210.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.columns(2, |cols| {
                    section_heading(&mut cols[0], "🧾 Ordrer");
                    egui::ScrollArea::vertical().id_salt("ordrer").show(&mut cols[0], |ui| {
                        egui::Grid::new("ordre_grid").striped(true).min_col_width(52.0).show(ui, |ui| {
                            for t in ["Tid", "Side", "Symbol", "Antall", "Kurs", "Status", "Merknad"] {
                                ui.label(RichText::new(t).strong().color(GRAY));
                            }
                            ui.end_row();
                            for o in &st.orders {
                                ui.label(o.created.format("%H:%M:%S").to_string());
                                let color = match o.side {
                                    Side::Buy => GREEN,
                                    Side::Sell => RED,
                                };
                                ui.label(RichText::new(o.side.to_string()).color(color).strong());
                                ui.label(&o.symbol);
                                ui.label(format!("{:.0}", o.qty));
                                ui.label(format!("{:.2}", o.avg_price));
                                ui.label(o.status.to_string());
                                ui.label(RichText::new(&o.note).small().color(GRAY));
                                ui.end_row();
                            }
                        });
                        if st.orders.is_empty() {
                            ui.small("Ingen ordrer ennå — strategien venter på et signal.");
                        }
                    });

                    section_heading(&mut cols[1], "📜 Hendelser");
                    egui::ScrollArea::vertical().id_salt("logg").show(&mut cols[1], |ui| {
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

    fn chart_panel(&mut self, ctx: &egui::Context, st: &UiState) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let Some(symbol) = self.selected.clone() else {
                ui.centered_and_justified(|ui| {
                    ui.spinner();
                });
                return;
            };

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading(RichText::new(format!("📈 {symbol}")).strong());
                if let Some(q) = st.quotes.get(&symbol) {
                    let pct = q.change_pct();
                    let color = if pct >= 0.0 { GREEN } else { RED };
                    ui.label(RichText::new(format!("{:.2}  ({pct:+.2} %)", q.last)).color(color).strong().size(18.0));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.selectable_value(&mut self.range, ChartRange::All, "Alt");
                    ui.selectable_value(&mut self.range, ChartRange::ThreeMonths, "3 mnd");
                    ui.selectable_value(&mut self.range, ChartRange::Month, "1 mnd");
                    ui.selectable_value(&mut self.range, ChartRange::Week, "1 uke");
                    ui.separator();
                    ui.selectable_value(&mut self.style, ChartStyle::Candles, "🕯 Candles");
                    ui.selectable_value(&mut self.style, ChartStyle::Line, "📈 Linje");
                });
            });

            let history = st.history.get(&symbol);
            let cutoff = self
                .range
                .seconds()
                .map(|s| chrono::Utc::now().timestamp() as f64 - s);
            let filter = |pts: Vec<[f64; 2]>| -> Vec<[f64; 2]> {
                match cutoff {
                    Some(c) => pts.into_iter().filter(|p| p[0] >= c).collect(),
                    None => pts,
                }
            };

            let (fast_n, slow_n) = st.sma_windows;
            let (price_pts, fast_pts, slow_pts) = match history {
                Some(h) => (
                    filter(h.iter().map(|&(t, p)| [t, p]).collect()),
                    filter(sma_series(h, fast_n)),
                    filter(sma_series(h, slow_n)),
                ),
                None => (Vec::new(), Vec::new(), Vec::new()),
            };

            // Kursgraf med strategiens SMA-linjer — krysningene er kjøps-/salgspunktene.
            let equity_h = 130.0;
            let chart_h = (ui.available_height() - equity_h - 60.0).max(220.0);
            Plot::new("kursgraf")
                .height(chart_h)
                .legend(Legend::default())
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
                    match self.style {
                        ChartStyle::Line => {
                            plot_ui.line(Line::new(PlotPoints::from(price_pts)).color(GREEN).width(2.0).name("Kurs"));
                        }
                        ChartStyle::Candles => {
                            let elems: Vec<BoxElem> = st
                                .candles
                                .get(&symbol)
                                .map(|candles| {
                                    candles
                                        .iter()
                                        .filter(|c| cutoff.is_none_or(|cut| c.ts >= cut))
                                        .map(|c| {
                                            let up = c.close >= c.open;
                                            let color = if up { GREEN } else { RED };
                                            let body_lo = c.open.min(c.close);
                                            let body_hi = c.open.max(c.close);
                                            BoxElem::new(
                                                c.ts,
                                                BoxSpread::new(c.low, body_lo, (body_lo + body_hi) / 2.0, body_hi, c.high),
                                            )
                                            .box_width(86400.0 * 0.6)
                                            .whisker_width(0.0)
                                            .fill(color.gamma_multiply(0.7))
                                            .stroke(egui::Stroke::new(1.0, color))
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            plot_ui.box_plot(BoxPlot::new(elems).name("Dagsstolper"));
                        }
                    }
                    plot_ui.line(
                        Line::new(PlotPoints::from(fast_pts))
                            .color(YELLOW)
                            .width(1.2)
                            .name(format!("SMA {fast_n} (rask)")),
                    );
                    plot_ui.line(
                        Line::new(PlotPoints::from(slow_pts))
                            .color(BLUE)
                            .width(1.2)
                            .name(format!("SMA {slow_n} (treg)")),
                    );
                });
            ui.small("Strategien sma_cross kjøper når gul (rask) krysser over blå (treg), og selger ved kryss under. Zoom med musehjulet.");

            ui.add_space(6.0);
            ui.label(RichText::new("Egenkapital denne økten").strong().color(GRAY));
            let eq_pts: Vec<[f64; 2]> = st.equity_history.iter().map(|&(t, v)| [t, v]).collect();
            Plot::new("egenkapital")
                .height(equity_h - 30.0)
                .x_axis_formatter(|mark, _range| {
                    chrono::DateTime::from_timestamp(mark.value as i64, 0)
                        .map(|dt| dt.format("%H:%M").to_string())
                        .unwrap_or_default()
                })
                .show(ui, |plot_ui| {
                    plot_ui.line(Line::new(PlotPoints::from(eq_pts)).color(YELLOW).width(1.5).name("Egenkapital"));
                });
        });
    }
}

/// Kjør backtest på valgt symbol med valgt strategi over dagsstolpene.
fn run_backtest(
    selected: Option<&str>,
    strategy_name: &str,
    st: &UiState,
) -> std::result::Result<BacktestResult, String> {
    let Some(symbol) = selected else {
        return Err("Velg et symbol i watchlisten først.".into());
    };
    let Some(candles) = st.candles.get(symbol) else {
        return Err("Ingen historikk ennå — vent til kursene er lastet.".into());
    };
    backtest::run(symbol, candles, strategy_name, &st.strategy_cfg).map_err(|e| format!("{e:#}"))
}

/// Rullerende gjennomsnitt over (tid, kurs)-serien — samme beregning som
/// strategien bruker, så grafen viser nøyaktig det strategien ser.
fn sma_series(history: &VecDeque<(f64, f64)>, window: usize) -> Vec<[f64; 2]> {
    if window == 0 || history.len() < window {
        return Vec::new();
    }
    let vals: Vec<(f64, f64)> = history.iter().copied().collect();
    let mut out = Vec::with_capacity(vals.len() - window + 1);
    let mut sum: f64 = vals[..window].iter().map(|&(_, p)| p).sum();
    out.push([vals[window - 1].0, sum / window as f64]);
    for i in window..vals.len() {
        sum += vals[i].1 - vals[i - window].1;
        out.push([vals[i].0, sum / window as f64]);
    }
    out
}

fn badge(ui: &mut egui::Ui, text: &str, fg: Color32, bg: Color32) {
    ui.label(RichText::new(text).color(fg).background_color(bg).strong());
}

fn section_heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).strong().size(16.0));
}

fn stat_card(ui: &mut egui::Ui, title: &str, value: String, color: Color32) {
    egui::Frame::group(ui.style())
        .fill(BG_CARD)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .inner_margin(egui::Margin::symmetric(14.0, 8.0))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(title).small().color(GRAY));
                ui.label(RichText::new(value).size(20.0).color(color).strong());
            });
        });
}

/// 1234567.8 → "1 234 568" — norsk tusenskilletegn gjør store tall lesbare.
fn fmt_thousands(v: f64) -> String {
    let negative = v < 0.0;
    let rounded = v.abs().round() as i64;
    let digits = rounded.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    if negative {
        format!("-{out}")
    } else {
        out
    }
}
