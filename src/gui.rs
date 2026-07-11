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

// Nordnet-inspirert, nesten svart palett med grønn aksent.
const GREEN: Color32 = Color32::from_rgb(0, 196, 106);
const RED: Color32 = Color32::from_rgb(240, 82, 82);
const YELLOW: Color32 = Color32::from_rgb(250, 204, 21);
const BLUE: Color32 = Color32::from_rgb(96, 165, 250);
const GRAY: Color32 = Color32::from_rgb(140, 150, 165);
const BG_PANEL: Color32 = Color32::from_rgb(10, 13, 20);
const BG_DEEP: Color32 = Color32::from_rgb(5, 7, 12);
const BG_CARD: Color32 = Color32::from_rgb(16, 20, 29);
const BORDER: Color32 = Color32::from_rgb(30, 38, 51);

#[derive(Clone, Copy, PartialEq)]
enum View {
    Handel,
    Portefolje,
    Ordrer,
    Transaksjoner,
    Marked,
    Analyse,
    Kalender,
}

#[derive(Clone, Copy, PartialEq)]
enum OrderFilter {
    Alle,
    Aktive,
    Fullforte,
    Kansellerte,
}

impl OrderFilter {
    fn matches(self, status: crate::types::OrderStatus) -> bool {
        use crate::types::OrderStatus::*;
        match self {
            OrderFilter::Alle => true,
            OrderFilter::Aktive => status == Submitted,
            OrderFilter::Fullforte => status == Filled,
            OrderFilter::Kansellerte => matches!(status, Cancelled | Rejected),
        }
    }
}

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
    Year,
    All,
}

impl ChartRange {
    fn seconds(self) -> Option<f64> {
        match self {
            ChartRange::Week => Some(7.0 * 86400.0),
            ChartRange::Month => Some(30.0 * 86400.0),
            ChartRange::ThreeMonths => Some(92.0 * 86400.0),
            ChartRange::Year => Some(365.0 * 86400.0),
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
        view: View::Handel,
        selected: None,
        range: ChartRange::ThreeMonths,
        style: ChartStyle::Line,
        trade_qty: 10.0,
        strategy_choice: String::new(),
        backtest: None,
        order_filter: OrderFilter::Alle,
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

/// Mørkt, moderne tema i samme farger som logoen: nesten svart med grønn
/// aksent, myke hjørner og diskret grønn glød på interaktive elementer.
fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG_PANEL;
    visuals.window_fill = BG_PANEL;
    visuals.extreme_bg_color = BG_DEEP;
    visuals.faint_bg_color = BG_CARD;
    visuals.selection.bg_fill = Color32::from_rgb(14, 92, 53);
    visuals.hyperlink_color = GREEN;

    // Myke hjørner overalt.
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
    visuals.window_rounding = egui::Rounding::same(12.0);
    visuals.widgets.noninteractive.bg_stroke.color = BORDER;
    // Grønn glød ved hover/klikk.
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, GREEN);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(22, 30, 42);
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.2, GREEN);
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    ctx.set_style(style);
}

/// Diskret skygge som løfter kortene fra bakgrunnen.
fn card_shadow() -> egui::epaint::Shadow {
    egui::epaint::Shadow {
        offset: egui::vec2(0.0, 2.0),
        blur: 10.0,
        spread: 0.0,
        color: Color32::from_black_alpha(120),
    }
}

/// Mini-graf (sparkline) uten akser — kursutviklingen i én liten strek.
fn sparkline(ui: &mut egui::Ui, id: &str, points: Vec<[f64; 2]>, color: Color32) {
    Plot::new(id.to_string())
        .height(20.0)
        .width(72.0)
        .show_axes(false)
        .show_grid(false)
        .show_background(false)
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        .show_x(false)
        .show_y(false)
        .show(ui, |plot_ui| {
            plot_ui.line(Line::new(PlotPoints::from(points)).color(color).width(1.5));
        });
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
    view: View,
    selected: Option<String>,
    range: ChartRange,
    style: ChartStyle,
    trade_qty: f64,
    strategy_choice: String,
    backtest: Option<std::result::Result<BacktestResult, String>>,
    order_filter: OrderFilter,
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
        match self.view {
            View::Handel => {
                self.left_panel(ctx, &mut st);
                self.bottom_panel(ctx, &st);
                self.chart_panel(ctx, &st);
            }
            View::Portefolje => self.portfolio_view(ctx, &st),
            View::Ordrer => self.orders_view(ctx, &st),
            View::Transaksjoner => self.transactions_view(ctx, &st),
            View::Marked => self.market_view(ctx, &mut st),
            View::Analyse => self.analyse_view(ctx, &st),
            View::Kalender => self.calendar_view(ctx, &st),
        }
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

            // Hovedmeny
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                for (view, label) in [
                    (View::Handel, "📊 Handel"),
                    (View::Portefolje, "💼 Portefølje"),
                    (View::Ordrer, "🧾 Ordrer"),
                    (View::Transaksjoner, "💳 Transaksjoner"),
                    (View::Marked, "🔥 Markedet"),
                    (View::Analyse, "🔮 Uken"),
                    (View::Kalender, "📅 Kalender"),
                ] {
                    let active = self.view == view;
                    let text = if active {
                        RichText::new(label).size(15.0).color(GREEN).strong()
                    } else {
                        RichText::new(label).size(15.0).color(GRAY)
                    };
                    if ui.selectable_label(active, text).clicked() {
                        self.view = view;
                    }
                }
            });
            ui.add_space(4.0);
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
                egui::Grid::new("watchlist").striped(true).min_col_width(64.0).show(ui, |ui| {
                    ui.label(RichText::new("Symbol").strong().color(GRAY));
                    ui.label(RichText::new("Siste").strong().color(GRAY));
                    ui.label(RichText::new("Endring").strong().color(GRAY));
                    ui.label(RichText::new("30 dager").strong().color(GRAY));
                    ui.end_row();
                    for q in st.quotes.values() {
                        let selected = self.selected.as_deref() == Some(q.symbol.as_str());
                        if ui.selectable_label(selected, RichText::new(&q.symbol).strong()).clicked() {
                            self.selected = Some(q.symbol.clone());
                        }
                        ui.label(format!("{:.2}", q.last));
                        let pct = q.change_pct();
                        let color = if pct >= 0.0 { GREEN } else { RED };
                        let arrow = if pct >= 0.0 { "▲" } else { "▼" };
                        ui.label(RichText::new(format!("{arrow} {pct:+.2} %")).color(color));
                        // Sparkline: siste ~30 punkter, farget etter trenden.
                        let points: Vec<[f64; 2]> = st
                            .history
                            .get(&q.symbol)
                            .map(|h| {
                                h.iter()
                                    .rev()
                                    .take(30)
                                    .rev()
                                    .map(|&(t, p)| [t, p])
                                    .collect()
                            })
                            .unwrap_or_default();
                        let trend = match (points.first(), points.last()) {
                            (Some(a), Some(b)) if b[1] >= a[1] => GREEN,
                            (Some(_), Some(_)) => RED,
                            _ => GRAY,
                        };
                        sparkline(ui, &format!("spark_{}", q.symbol), points, trend);
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
                if ui.button("🧪 Backtest på valgt symbol (2 år)").clicked() {
                    self.backtest = Some(run_backtest(self.selected.as_deref(), &self.strategy_choice, st));
                }
                match &self.backtest {
                    Some(Ok(r)) => {
                        ui.label(
                            RichText::new(format!("{} på {}", r.strategy, r.symbol)).strong(),
                        );
                        let color = if r.return_pct >= 0.0 { GREEN } else { RED };
                        ui.label(RichText::new(format!("Avkastning: {:+.1} %", r.return_pct)).color(color).strong().size(17.0));
                        let bh_color = if r.buy_hold_pct >= 0.0 { GREEN } else { RED };
                        ui.horizontal(|ui| {
                            ui.label("Kjøp-og-hold:");
                            ui.label(RichText::new(format!("{:+.1} %", r.buy_hold_pct)).color(bh_color));
                        });
                        ui.horizontal(|ui| {
                            ui.label("Verste fall:");
                            ui.label(RichText::new(format!("{:.1} %", r.max_drawdown_pct)).color(RED));
                        });
                        let wins = r.wins();
                        ui.label(format!(
                            "{} handler, {} med gevinst{} — kostnader: {:.0} kr",
                            r.trades.len(),
                            wins,
                            if r.open_entry.is_some() { " (én fortsatt åpen)" } else { "" },
                            r.costs_paid,
                        ));
                        // Resultatkurve i miniatyr.
                        let curve: Vec<[f64; 2]> = r.equity_curve.clone();
                        if curve.len() > 1 {
                            let min_y = curve.iter().map(|p| p[1]).fold(f64::MAX, f64::min);
                            Plot::new("backtest_kurve")
                                .height(80.0)
                                .show_axes(false)
                                .show_grid(false)
                                .allow_drag(false)
                                .allow_zoom(false)
                                .allow_scroll(false)
                                .allow_boxed_zoom(false)
                                .show(ui, |plot_ui| {
                                    plot_ui.line(
                                        Line::new(PlotPoints::from(curve))
                                            .color(color)
                                            .width(1.5)
                                            .fill(min_y as f32),
                                    );
                                });
                        }
                        ui.small("Inkluderer kurtasje og glidning (justeres i [backtest]-konfig).");
                    }
                    Some(Err(e)) => {
                        ui.label(RichText::new(e).color(RED).small());
                    }
                    None => {
                        ui.small("Test strategien på 2 års historikk før du lar den handle.");
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
                    ui.selectable_value(&mut self.range, ChartRange::Year, "1 år");
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
                            let min_y = price_pts.iter().map(|p| p[1]).fold(f64::MAX, f64::min);
                            plot_ui.line(
                                Line::new(PlotPoints::from(price_pts))
                                    .color(GREEN)
                                    .width(2.0)
                                    .fill(min_y as f32)
                                    .name("Kurs"),
                            );
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
            let eq_min = eq_pts.iter().map(|p| p[1]).fold(f64::MAX, f64::min);
            Plot::new("egenkapital")
                .height(equity_h - 30.0)
                .x_axis_formatter(|mark, _range| {
                    chrono::DateTime::from_timestamp(mark.value as i64, 0)
                        .map(|dt| dt.format("%H:%M").to_string())
                        .unwrap_or_default()
                })
                .show(ui, |plot_ui| {
                    plot_ui.line(
                        Line::new(PlotPoints::from(eq_pts))
                            .color(YELLOW)
                            .width(1.5)
                            .fill(eq_min as f32)
                            .name("Egenkapital"),
                    );
                });
        });
    }
}

impl App {
    fn portfolio_view(&self, ctx: &egui::Context, st: &UiState) {
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().id_salt("portefolje_scroll").show(ui, |ui| {
                ui.add_space(4.0);
                ui.heading(RichText::new("💼 Min portefølje").strong());
                ui.add_space(8.0);

                // Dagens utvikling: sum av qty × (siste − forrige slutt).
                let day_kr: f64 = st
                    .positions
                    .iter()
                    .filter_map(|p| st.quotes.get(&p.symbol).map(|q| p.qty * (q.last - q.prev_close)))
                    .sum();
                let day_pct = if st.equity - day_kr > 0.0 {
                    day_kr / (st.equity - day_kr) * 100.0
                } else {
                    0.0
                };
                let total_kr = st.equity - st.start_cash;
                let total_pct = if st.start_cash > 0.0 { total_kr / st.start_cash * 100.0 } else { 0.0 };

                // Kontooversikt
                section_heading(ui, "🏦 Kontoer");
                ui.horizontal(|ui| {
                    account_card(ui, &format!("Papirkonto ({})", st.broker_name), &[
                        ("Egenkapital", format!("{} kr", fmt_thousands(st.equity)), Color32::WHITE),
                        ("Kontanter", format!("{} kr", fmt_thousands(st.cash)), GRAY),
                        ("I dag", format!("{}{} kr ({:+.2} %)", plus(day_kr), fmt_thousands(day_kr), day_pct), updown(day_kr)),
                        ("Total avkastning", format!("{}{} kr ({:+.2} %)", plus(total_kr), fmt_thousands(total_kr), total_pct), updown(total_kr)),
                    ]);
                    if st.nordnet_enabled {
                        let nn_value: f64 = st.nordnet_positions.iter().map(|p| p.market_value).sum();
                        account_card(ui, "Nordnet (lesemodus)", &[
                            ("Verdi", format!("{} kr", fmt_thousands(nn_value)), Color32::WHITE),
                            ("Posisjoner", format!("{}", st.nordnet_positions.len()), GRAY),
                            ("", "Kun lesing — handles ikke".into(), BLUE),
                        ]);
                    }
                });

                // Utviklingsgraf
                ui.add_space(14.0);
                section_heading(ui, "📈 Utvikling denne økten");
                let eq_pts: Vec<[f64; 2]> = st.equity_history.iter().map(|&(t, v)| [t, v]).collect();
                Plot::new("portefolje_equity")
                    .height(200.0)
                    .x_axis_formatter(|mark, _range| {
                        chrono::DateTime::from_timestamp(mark.value as i64, 0)
                            .map(|dt| dt.format("%H:%M").to_string())
                            .unwrap_or_default()
                    })
                    .show(ui, |plot_ui| {
                        let min_y = eq_pts.iter().map(|p| p[1]).fold(f64::MAX, f64::min);
                        plot_ui.line(
                            Line::new(PlotPoints::from(eq_pts))
                                .color(GREEN)
                                .width(2.0)
                                .fill(min_y as f32)
                                .name("Egenkapital"),
                        );
                    });

                // Beholdning
                ui.add_space(14.0);
                section_heading(ui, "💼 Beholdning");
                if st.positions.is_empty() {
                    ui.small("Ingen posisjoner ennå — boten (eller du, via Hurtighandel) har ikke kjøpt noe.");
                } else {
                    let invested: f64 = st.positions.iter().map(|p| p.market_value()).sum();
                    egui::Grid::new("beholdning").striped(true).min_col_width(60.0).show(ui, |ui| {
                        for h in ["Symbol", "Antall", "Snitt", "Siste", "Verdi", "Urealisert", "I dag", "Utbytte 12m", "Andel"] {
                            ui.label(RichText::new(h).strong().color(GRAY));
                        }
                        ui.end_row();
                        for p in &st.positions {
                            ui.label(RichText::new(&p.symbol).strong());
                            ui.label(format!("{:.0}", p.qty));
                            ui.label(format!("{:.2}", p.avg_price));
                            ui.label(format!("{:.2}", p.last));
                            ui.label(fmt_thousands(p.market_value()));
                            let u = p.unrealized();
                            let u_pct = if p.avg_price > 0.0 { (p.last / p.avg_price - 1.0) * 100.0 } else { 0.0 };
                            ui.label(RichText::new(format!("{u:+.0} kr ({u_pct:+.1} %)")).color(updown(u)));
                            match st.quotes.get(&p.symbol) {
                                Some(q) => {
                                    let d = p.qty * (q.last - q.prev_close);
                                    ui.label(RichText::new(format!("{d:+.0} kr")).color(updown(d)));
                                }
                                None => { ui.label("–"); }
                            }
                            match st.dividends.get(&p.symbol) {
                                Some(&div) if div > 0.0 => {
                                    ui.label(RichText::new(format!("{:.0} kr", div * p.qty)).color(YELLOW));
                                }
                                _ => { ui.label("–"); }
                            }
                            let share = if invested > 0.0 { p.market_value() / invested * 100.0 } else { 0.0 };
                            ui.label(format!("{share:.1} %"));
                            ui.end_row();
                        }
                    });

                    // Forventet utbytte samlet (basert på siste 12 mnd per aksje).
                    let total_div: f64 = st
                        .positions
                        .iter()
                        .filter_map(|p| st.dividends.get(&p.symbol).map(|d| d * p.qty))
                        .sum();
                    if total_div > 0.0 {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!(
                                "💰 Utbytte: beholdningen din betalte {} kr siste 12 mnd (indikasjon på årlig utbytteinntekt).",
                                fmt_thousands(total_div)
                            ))
                            .color(YELLOW),
                        );
                    }

                    // Fordeling
                    ui.add_space(14.0);
                    section_heading(ui, "🥧 Fordeling");
                    let total = invested + st.cash;
                    for p in &st.positions {
                        let frac = if total > 0.0 { p.market_value() / total } else { 0.0 };
                        ui.add(
                            egui::ProgressBar::new(frac as f32)
                                .text(format!("{}  {:.1} %  ({} kr)", p.symbol, frac * 100.0, fmt_thousands(p.market_value()))),
                        );
                    }
                    let cash_frac = if total > 0.0 { st.cash / total } else { 0.0 };
                    ui.add(
                        egui::ProgressBar::new(cash_frac as f32)
                            .text(format!("Kontanter  {:.1} %  ({} kr)", cash_frac * 100.0, fmt_thousands(st.cash))),
                    );
                }
            });
        });
    }

    fn orders_view(&mut self, ctx: &egui::Context, st: &UiState) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(4.0);
            ui.heading(RichText::new("🧾 Ordrer").strong());
            ui.small("Ordrene fra denne økten. Full historikk ligger under 💳 Transaksjoner.");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let count = |f: OrderFilter| st.orders.iter().filter(|o| f.matches(o.status)).count();
                ui.selectable_value(&mut self.order_filter, OrderFilter::Alle, format!("Alle ({})", st.orders.len()));
                ui.selectable_value(&mut self.order_filter, OrderFilter::Aktive, format!("Aktive ({})", count(OrderFilter::Aktive)));
                ui.selectable_value(&mut self.order_filter, OrderFilter::Fullforte, format!("Fullførte ({})", count(OrderFilter::Fullforte)));
                ui.selectable_value(&mut self.order_filter, OrderFilter::Kansellerte, format!("Kansellerte/avviste ({})", count(OrderFilter::Kansellerte)));
            });
            ui.add_space(6.0);
            egui::ScrollArea::vertical().id_salt("ordrer_view").show(ui, |ui| {
                egui::Grid::new("ordrer_full").striped(true).min_col_width(60.0).show(ui, |ui| {
                    for h in ["Tid", "Id", "Side", "Symbol", "Antall", "Kurs", "Status", "Merknad"] {
                        ui.label(RichText::new(h).strong().color(GRAY));
                    }
                    ui.end_row();
                    for o in st.orders.iter().filter(|o| self.order_filter.matches(o.status)) {
                        ui.label(o.created.format("%H:%M:%S").to_string());
                        ui.label(&o.id);
                        let color = match o.side { Side::Buy => GREEN, Side::Sell => RED };
                        ui.label(RichText::new(o.side.to_string()).color(color).strong());
                        ui.label(&o.symbol);
                        ui.label(format!("{:.0}", o.qty));
                        ui.label(format!("{:.2}", o.avg_price));
                        ui.label(o.status.to_string());
                        ui.label(RichText::new(&o.note).small().color(GRAY));
                        ui.end_row();
                    }
                });
                if st.orders.iter().filter(|o| self.order_filter.matches(o.status)).count() == 0 {
                    ui.add_space(8.0);
                    ui.small("Ingen ordrer i denne kategorien ennå.");
                }
            });
        });
    }

    fn transactions_view(&self, ctx: &egui::Context, st: &UiState) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(4.0);
            ui.heading(RichText::new("💳 Transaksjoner").strong());
            ui.small(format!(
                "Komplett historikk fra databasen ({} transaksjoner) — også fra tidligere økter.",
                st.transactions.len()
            ));
            ui.add_space(6.0);
            egui::ScrollArea::vertical().id_salt("tx_view").show(ui, |ui| {
                egui::Grid::new("tx_grid").striped(true).min_col_width(60.0).show(ui, |ui| {
                    for h in ["Tidspunkt", "Side", "Symbol", "Antall", "Kurs", "Beløp", "Status", "Megler", "Merknad"] {
                        ui.label(RichText::new(h).strong().color(GRAY));
                    }
                    ui.end_row();
                    for t in &st.transactions {
                        ui.label(&t.ts);
                        let color = if t.side == "KJØP" { GREEN } else { RED };
                        ui.label(RichText::new(&t.side).color(color).strong());
                        ui.label(&t.symbol);
                        ui.label(format!("{:.0}", t.qty));
                        ui.label(format!("{:.2}", t.price));
                        ui.label(fmt_thousands(t.qty * t.price));
                        ui.label(&t.status);
                        ui.label(RichText::new(&t.broker).color(GRAY));
                        ui.label(RichText::new(&t.note).small().color(GRAY));
                        ui.end_row();
                    }
                });
                if st.transactions.is_empty() {
                    ui.add_space(8.0);
                    ui.small("Ingen transaksjoner ennå.");
                }
            });
        });
    }

    fn calendar_view(&self, ctx: &egui::Context, st: &UiState) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(4.0);
            ui.heading(RichText::new("📅 Selskapskalender").strong());
            ui.small(
                "Kommende kvartalsrapporter og utbyttedatoer for de 25 største Oslo Børs-selskapene. \
                 Datoene er Yahoos estimater og kan endres av selskapene.",
            );
            ui.add_space(6.0);
            if let Some(note) = &st.calendar_note {
                ui.label(RichText::new(note).color(YELLOW));
            }
            if st.calendar.is_empty() && st.calendar_note.is_none() {
                ui.spinner();
                ui.small("Henter kalenderdata …");
            }
            egui::ScrollArea::vertical().id_salt("kalender_view").show(ui, |ui| {
                egui::Grid::new("kalender_grid").striped(true).min_col_width(80.0).show(ui, |ui| {
                    for h in ["Dato", "Om", "Symbol", "Selskap", "Hendelse"] {
                        ui.label(RichText::new(h).strong().color(GRAY));
                    }
                    ui.end_row();
                    let now = chrono::Utc::now();
                    for e in &st.calendar {
                        ui.label(RichText::new(e.date.format("%d.%m.%Y").to_string()).strong());
                        let days = (e.date - now).num_days();
                        let om = match days {
                            d if d <= 0 => "i dag".to_string(),
                            1 => "i morgen".to_string(),
                            d => format!("om {d} dager"),
                        };
                        ui.label(RichText::new(om).color(if days <= 7 { YELLOW } else { GRAY }));
                        ui.label(&e.symbol);
                        ui.label(&e.name);
                        let color = match e.kind {
                            "Kvartalsrapport" => BLUE,
                            "Eks-utbytte" => YELLOW,
                            _ => GREEN,
                        };
                        ui.label(RichText::new(e.kind).color(color).strong());
                        ui.end_row();
                    }
                });
            });
        });
    }

    fn market_view(&mut self, ctx: &egui::Context, st: &mut UiState) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut to_follow: Vec<String> = Vec::new();
            egui::ScrollArea::vertical().id_salt("marked_scroll").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.heading(RichText::new("🔥 Markedet").strong());
                    match st.market.updated {
                        Some(ts) => {
                            ui.label(RichText::new(format!("oppdatert {}", ts.format("%H:%M"))).color(GRAY));
                        }
                        None => {
                            ui.spinner();
                            ui.label(RichText::new("henter markedsdata (kan ta et halvt minutt) …").color(GRAY));
                        }
                    }
                });
                ui.small("Klikk ➕ for å legge en aksje i watchlisten — da følger boten og grafen den.");
                ui.add_space(8.0);

                market_table(
                    ui,
                    "💰 Dagens mest omsatte",
                    "De 10 aksjene med høyest omsetning (kurs × volum) på Oslo Børs i dag.",
                    "mest_omsatte",
                    &st.market.most_traded,
                    &mut to_follow,
                );
                ui.add_space(14.0);
                market_table(
                    ui,
                    "⚡ Beste for daytrading",
                    "Størst dagsbevegelse (snitt siste 10 dager) blant de mest likvide — mye svingning å handle på, men også høyere risiko.",
                    "daytrading",
                    &st.market.day_trade,
                    &mut to_follow,
                );
                ui.add_space(14.0);
                market_table(
                    ui,
                    "🌍 Populære fond og ETF-er",
                    "Kuraterte, folkekjære indeksfond (ETF-er med live-kurser). Norske verdipapirfond mangler åpne sanntidskurser.",
                    "fond",
                    &st.market.funds,
                    &mut to_follow,
                );
            });
            for symbol in to_follow {
                st.follow(&symbol);
            }
        });
    }

    fn analyse_view(&self, ctx: &egui::Context, st: &UiState) {
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().id_salt("analyse_scroll").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.heading(RichText::new("🔮 Ukens analyse").strong());
                    match st.market.updated {
                        Some(ts) => {
                            ui.label(RichText::new(format!("oppdatert {}", ts.format("%H:%M"))).color(GRAY));
                        }
                        None => {
                            ui.spinner();
                            ui.label(RichText::new("analyserer …").color(GRAY));
                        }
                    }
                });
                ui.small(
                    "Automatisk teknisk vurdering av uken som kommer, per aksje: trend (SMA 5/20), \
                     momentum (ukesendring) og RSI. Dette er en enkel maskinvurdering — IKKE investeringsråd.",
                );
                ui.add_space(8.0);

                egui::Grid::new("ukesanalyse").striped(true).min_col_width(60.0).show(ui, |ui| {
                    for h in ["Symbol", "Selskap", "Siste", "Uke", "RSI", "Trend", "Sving/dag", "Vurdering"] {
                        ui.label(RichText::new(h).strong().color(GRAY));
                    }
                    ui.end_row();
                    for w in &st.market.week {
                        ui.label(RichText::new(&w.symbol).strong());
                        ui.label(&w.name);
                        ui.label(format!("{:.2}", w.last));
                        let wc = if w.week_pct >= 0.0 { GREEN } else { RED };
                        ui.label(RichText::new(format!("{:+.1} %", w.week_pct)).color(wc));
                        let rsi_color = if w.rsi > 70.0 || w.rsi < 30.0 { YELLOW } else { GRAY };
                        ui.label(RichText::new(format!("{:.0}", w.rsi)).color(rsi_color));
                        if w.trend_up {
                            ui.label(RichText::new("↗ opp").color(GREEN));
                        } else {
                            ui.label(RichText::new("↘ ned").color(RED));
                        }
                        ui.label(format!("{:.1} %", w.range_pct));
                        let (verdict, color) = verdict_for(w.score);
                        ui.label(RichText::new(verdict).color(color).strong());
                        ui.end_row();
                    }
                });
                if st.market.week.is_empty() {
                    ui.add_space(10.0);
                    ui.spinner();
                }
            });
        });
    }
}

fn verdict_for(score: i32) -> (&'static str, Color32) {
    match score {
        s if s >= 2 => ("Positiv 📈", GREEN),
        s if s <= -2 => ("Svak 📉", RED),
        _ => ("Nøytral ➖", GRAY),
    }
}

fn market_table(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: &str,
    id: &str,
    rows: &[crate::market::MarketRow],
    to_follow: &mut Vec<String>,
) {
    section_heading(ui, title);
    ui.small(subtitle);
    ui.add_space(4.0);
    egui::Grid::new(id).striped(true).min_col_width(58.0).show(ui, |ui| {
        for h in ["", "Symbol", "Navn", "Siste", "I dag", "Uke", "Omsetning", "Sving/dag"] {
            ui.label(RichText::new(h).strong().color(GRAY));
        }
        ui.end_row();
        for r in rows {
            if ui.button("➕").on_hover_text("Legg til i watchlisten").clicked() {
                to_follow.push(r.symbol.clone());
            }
            ui.label(RichText::new(&r.symbol).strong());
            ui.label(&r.name);
            ui.label(format!("{:.2}", r.last));
            let dc = if r.day_pct >= 0.0 { GREEN } else { RED };
            ui.label(RichText::new(format!("{:+.2} %", r.day_pct)).color(dc));
            let wc = if r.week_pct >= 0.0 { GREEN } else { RED };
            ui.label(RichText::new(format!("{:+.1} %", r.week_pct)).color(wc));
            ui.label(fmt_turnover(r.turnover));
            ui.label(format!("{:.1} %", r.range_pct));
            ui.end_row();
        }
    });
    if rows.is_empty() {
        ui.small("Venter på data …");
    }
}

fn plus(v: f64) -> &'static str {
    if v >= 0.0 { "+" } else { "" }
}

fn updown(v: f64) -> Color32 {
    if v >= 0.0 { GREEN } else { RED }
}

/// Kontokort med tittel og rader av (etikett, verdi, farge).
fn account_card(ui: &mut egui::Ui, title: &str, rows: &[(&str, String, Color32)]) {
    egui::Frame::group(ui.style())
        .fill(BG_CARD)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .rounding(egui::Rounding::same(12.0))
        .shadow(card_shadow())
        .inner_margin(egui::Margin::symmetric(16.0, 12.0))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(title).strong().size(15.0));
                ui.add_space(4.0);
                for (label, value, color) in rows {
                    ui.horizontal(|ui| {
                        if !label.is_empty() {
                            ui.label(RichText::new(*label).small().color(GRAY));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(value).color(*color).strong());
                        });
                    });
                }
            });
        });
}

fn fmt_turnover(v: f64) -> String {
    if v >= 1e9 {
        format!("{:.1} mrd", v / 1e9)
    } else {
        format!("{:.0} mill", v / 1e6)
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
    backtest::run(symbol, candles, strategy_name, &st.strategy_cfg, &st.backtest_cfg)
        .map_err(|e| format!("{e:#}"))
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
        .rounding(egui::Rounding::same(10.0))
        .shadow(card_shadow())
        .inner_margin(egui::Margin::symmetric(14.0, 8.0))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(title).small().color(GRAY));
                ui.label(RichText::new(value).size(21.0).color(color).strong());
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
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
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
