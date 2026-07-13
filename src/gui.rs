use crate::backtest::{self, BacktestResult};
use crate::pnl::{self, RealizedTrade};
use crate::state::{Alarm, Flags, SharedState, UiState};
use crate::store::Store;
use crate::strategy;
use crate::types::Side;
use anyhow::Result;
use eframe::egui::{self, Color32, RichText};
use egui_plot::{
    BoxElem, BoxPlot, BoxSpread, HLine, Legend, Line, LineStyle, MarkerShape, Plot, PlotPoints,
    Points,
};
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
const BG_PANEL: Color32 = Color32::from_rgb(4, 6, 10);
const BG_DEEP: Color32 = Color32::from_rgb(1, 2, 4);
const BG_CARD: Color32 = Color32::from_rgb(9, 12, 18);
const BORDER: Color32 = Color32::from_rgb(22, 28, 38);

#[derive(Clone, Copy, PartialEq)]
enum View {
    Handel,
    Portefolje,
    Ordrer,
    Transaksjoner,
    Marked,
    Analyse,
    Morgan,
    Kalender,
    Innstillinger,
    Hjelp,
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

/// En ordre som venter på brukerens bekreftelse — vises som klartekst-kort
/// («Du kjøper 14 stk EQNR for ca. 4 928 kr …») før den legges i køen.
struct PendingOrder {
    symbol: String,
    side: Side,
    qty: f64,
    /// Anslått kurs per aksje i kontovaluta (kroner).
    price_nok: f64,
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

/// Alt GUI-et trenger fra oppstarten, samlet.
pub struct GuiDeps {
    pub state: SharedState,
    pub flags: Arc<Flags>,
    pub store: Arc<Store>,
    pub market: Arc<crate::marketdata::Yahoo>,
    pub rt: tokio::runtime::Handle,
    pub cfg: crate::config::Config,
    pub config_path: Option<std::path::PathBuf>,
}

/// Grafisk vindu med knapper og kursgraf. Blokkerer til vinduet lukkes.
pub fn run(deps: GuiDeps) -> Result<()> {
    let GuiDeps { state, flags, store, market, rt, cfg, config_path } = deps;
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
    // Velkomstguiden vises bare aller første gang appen åpnes.
    let store_welcome_pending = store.meta_get("welcome_shown").is_none();
    let app = App {
        state,
        flags: flags.clone(),
        store,
        market,
        rt,
        settings: cfg,
        config_path: config_path.unwrap_or_else(|| std::path::PathBuf::from("config.toml")),
        settings_msg: None,
        search_query: String::new(),
        allow_close: false,
        show_close_dialog: false,
        view: View::Handel,
        selected: None,
        range: ChartRange::ThreeMonths,
        style: ChartStyle::Line,
        trade_qty: 10.0,
        trade_amount_kr: 5_000.0,
        trade_use_kr: true,
        pending_order: None,
        simple_chart: false,
        fills_cache: (usize::MAX, Vec::new()),
        strategy_choice: String::new(),
        backtest: None,
        compare: None,
        order_filter: OrderFilter::Alle,
        alarm_level: 0.0,
        alarm_above: false,
        limit_level: 0.0,
        savings_amount: 2_000.0,
        savings_day: 1,
        realized_cache: (usize::MAX, Vec::new()),
        morgan_profile: String::from(
            "Risikotoleranse: moderat\n\
             Investeringsbeløp: 100 000 kr\n\
             Tidshorisont: 5–10 år\n\
             Foretrukne sektorer: energi, sjømat, teknologi\n\
             Annet: utbytte er et pluss, helst Oslo Børs",
        ),
        morgan_symbol: String::new(),
        welcome_open: store_welcome_pending,
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
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(15, 21, 30);
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
    store: Arc<Store>,
    market: Arc<crate::marketdata::Yahoo>,
    rt: tokio::runtime::Handle,
    /// Redigerbar kopi av konfigurasjonen — lagres til config_path.
    settings: crate::config::Config,
    config_path: std::path::PathBuf,
    settings_msg: Option<(String, bool)>,
    search_query: String,
    allow_close: bool,
    show_close_dialog: bool,
    view: View,
    selected: Option<String>,
    range: ChartRange,
    style: ChartStyle,
    trade_qty: f64,
    /// Hurtighandel i kroner: beløp og om kronemodus er valgt (standard).
    trade_amount_kr: f64,
    trade_use_kr: bool,
    /// Ordre som venter på bekreftelse i klartekst-kortet.
    pending_order: Option<PendingOrder>,
    /// Enkel graf: bare kurslinjen, uten SMA og candlesticks.
    simple_chart: bool,
    /// (antall transaksjoner da cachen ble bygget, fylte ordrer som
    /// (symbol, unixtid, kjøp?, kurs)) — kjøps-/salgsmerkene i grafen.
    fills_cache: (usize, Vec<(String, f64, bool, f64)>),
    strategy_choice: String,
    backtest: Option<std::result::Result<BacktestResult, String>>,
    /// Resultat av «sammenlign strategier» — én rad per strategi.
    compare: Option<Vec<std::result::Result<BacktestResult, String>>>,
    order_filter: OrderFilter,
    alarm_level: f64,
    alarm_above: bool,
    /// Nivåfeltet for nye limit-ordrer (0 = fylles med dagens kurs).
    limit_level: f64,
    /// Skjema for ny spareavtale: beløp og dag i måneden.
    savings_amount: f64,
    savings_day: u32,
    /// (antall transaksjoner da cachen ble bygget, realiserte handler).
    realized_cache: (usize, Vec<RealizedTrade>),
    /// Investeringsprofilen brukeren sender til Morgan.
    morgan_profile: String,
    /// Symbolvalget i Morgans dypdykk-nedtrekk.
    morgan_symbol: String,
    /// Velkomstguiden ved aller første oppstart.
    welcome_open: bool,
}

impl App {
    /// Skriv innstillingene til konfigfilen (kommentarer erstattes).
    fn write_settings(&mut self) -> anyhow::Result<()> {
        let body = toml::to_string_pretty(&self.settings)?;
        let content = format!(
            "# b-rs-konfigurasjon — skrevet av Innstillinger-fanen i appen.\n# Full dokumentasjon: config.example.toml i prosjektmappen.\n\n{body}"
        );
        std::fs::write(&self.config_path, content)?;
        Ok(())
    }

    /// Hold konfigfilens watchlist i takt med endringer gjort i appen.
    fn sync_watchlist(&mut self, watchlist: &[String]) {
        self.settings.watchlist = watchlist.to_vec();
        let _ = self.write_settings();
    }

    /// Tegn aktive toasts øverst til høyre; utløpte fjernes.
    fn draw_toasts(&self, ctx: &egui::Context, st: &mut UiState) {
        let now = chrono::Utc::now().timestamp();
        st.toasts.retain(|(expiry, _)| *expiry > now);
        if st.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::RIGHT_TOP, [-14.0, 96.0])
            .interactable(false)
            .show(ctx, |ui| {
                for (_, msg) in &st.toasts {
                    egui::Frame::group(ui.style())
                        .fill(BG_CARD)
                        .stroke(egui::Stroke::new(1.0, GREEN))
                        .rounding(egui::Rounding::same(10.0))
                        .shadow(card_shadow())
                        .inner_margin(egui::Margin::symmetric(12.0, 8.0))
                        .show(ui, |ui| {
                            ui.label(RichText::new(msg).strong());
                        });
                    ui.add_space(4.0);
                }
            });
    }

    /// Lukkeknappen: tilby å minimere i stedet, så boten jobber videre.
    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.show_close_dialog = true;
        }
        if !self.show_close_dialog {
            return;
        }
        egui::Window::new("Avslutte b-rs?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Boten stopper helt når appen lukkes.");
                ui.label("Vil du heller minimere den, så den jobber videre?");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(RichText::new("Minimer").color(Color32::BLACK)).fill(GREEN))
                        .clicked()
                    {
                        self.show_close_dialog = false;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                    if ui
                        .add(egui::Button::new(RichText::new("Avslutt").color(Color32::WHITE)).fill(RED))
                        .clicked()
                    {
                        self.allow_close = true;
                        self.show_close_dialog = false;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("Avbryt").clicked() {
                        self.show_close_dialog = false;
                    }
                });
            });
    }

    /// Realiserte handler (FIFO), gjenberegnet når nye transaksjoner kommer.
    fn realized(&mut self, st: &UiState) -> &[RealizedTrade] {
        let marker = st.transactions.len();
        if self.realized_cache.0 != marker {
            let fills = self.store.fills_chronological().unwrap_or_default();
            self.realized_cache = (marker, pnl::realized_fifo(&fills));
        }
        &self.realized_cache.1
    }

    /// Fylte ordrer som (symbol, unixtid, kjøp?, kurs) — merkene i grafen.
    /// Bygges på nytt når transaksjonslisten endrer seg.
    fn chart_fills(&mut self, st: &UiState) -> &[(String, f64, bool, f64)] {
        let marker = st.transactions.len();
        if self.fills_cache.0 != marker {
            let fills = self.store.fills_chronological().unwrap_or_default();
            let parsed = fills
                .iter()
                .filter_map(|f| {
                    chrono::DateTime::parse_from_rfc3339(&f.ts_rfc3339)
                        .ok()
                        .map(|dt| (f.symbol.clone(), dt.timestamp() as f64, f.is_buy, f.price))
                })
                .collect();
            self.fills_cache = (marker, parsed);
        }
        &self.fills_cache.1
    }
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
                self.chart_panel(ctx, &mut st);
            }
            View::Portefolje => self.portfolio_view(ctx, &st),
            View::Ordrer => self.orders_view(ctx, &st),
            View::Transaksjoner => self.transactions_view(ctx, &mut st),
            View::Marked => self.market_view(ctx, &mut st),
            View::Analyse => self.analyse_view(ctx, &st),
            View::Morgan => self.morgan_view(ctx, &mut st),
            View::Kalender => self.calendar_view(ctx, &st),
            View::Innstillinger => self.settings_view(ctx, &mut st),
            View::Hjelp => self.help_view(ctx),
        }

        self.confirm_order_dialog(ctx, &mut st);
        self.welcome_dialog(ctx);
        self.draw_toasts(ctx, &mut st);
        self.handle_close_request(ctx);
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
                    let age = (chrono::Utc::now() - ts).num_seconds();
                    let stale_after = (st.poll_secs.max(15) * 4) as i64;
                    if age > stale_after {
                        // Vakthund: datastrømmen har stoppet.
                        ui.label(
                            RichText::new(format!("⚠ ingen ferske kurser på {age} s"))
                                .color(RED)
                                .strong(),
                        );
                    } else {
                        ui.label(RichText::new(format!("oppdatert {}", ts.format("%H:%M:%S"))).color(GRAY));
                    }
                } else {
                    ui.spinner();
                    ui.label(RichText::new("henter kursdata …").color(GRAY));
                }

                if let Some((version, url)) = st.update_available.clone() {
                    let btn = egui::Button::new(
                        RichText::new(format!("📥 Ny versjon {version} — last ned")).color(Color32::BLACK),
                    )
                    .fill(GREEN);
                    if ui.add(btn).clicked() {
                        ctx.open_url(egui::OpenUrl::new_tab(url));
                    }
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
                    (View::Morgan, "🧠 Morgan"),
                    (View::Kalender, "📅 Kalender"),
                    (View::Innstillinger, "⚙ Innstillinger"),
                    (View::Hjelp, "❓ Hjelp"),
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

                // Aksjesøk: skriv navn eller ticker, trykk Enter.
                ui.horizontal(|ui| {
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text("Søk aksje/fond/krypto …")
                            .desired_width(180.0),
                    );
                    let go = ui.button("🔍").clicked()
                        || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                    if go && !self.search_query.trim().is_empty() {
                        st.search_pending = true;
                        st.search_results.clear();
                        let market = self.market.clone();
                        let state = self.state.clone();
                        let query = self.search_query.trim().to_string();
                        self.rt.spawn(async move {
                            let results = market.search(&query).await.unwrap_or_default();
                            let mut st = state.lock().unwrap();
                            st.search_pending = false;
                            if results.is_empty() {
                                st.log(format!("Søket «{query}» ga ingen treff."));
                            }
                            st.search_results = results;
                        });
                    }
                });
                if st.search_pending {
                    ui.spinner();
                }
                let mut follow_from_search: Option<String> = None;
                for (symbol, name) in &st.search_results {
                    if ui
                        .button(RichText::new(format!("➕ {symbol} — {name}")).small())
                        .clicked()
                    {
                        follow_from_search = Some(symbol.clone());
                    }
                }
                if let Some(symbol) = follow_from_search {
                    st.follow(&symbol);
                    st.search_results.clear();
                    self.search_query.clear();
                    self.sync_watchlist(&st.watchlist);
                }
                ui.add_space(4.0);
                let mut unfollow: Option<String> = None;
                egui::Grid::new("watchlist").striped(true).min_col_width(58.0).show(ui, |ui| {
                    ui.label(RichText::new("Symbol").strong().color(GRAY));
                    ui.label(RichText::new("Siste").strong().color(GRAY));
                    ui.label(RichText::new("Endring").strong().color(GRAY));
                    ui.label(RichText::new("30 dager").strong().color(GRAY));
                    ui.label("");
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
                        if ui.small_button("🗑").on_hover_text("Fjern fra watchlisten").clicked() {
                            unfollow = Some(q.symbol.clone());
                        }
                        ui.end_row();
                    }
                });
                if let Some(symbol) = unfollow {
                    st.watchlist.retain(|s| s != &symbol);
                    st.quotes.remove(&symbol);
                    if self.selected.as_deref() == Some(symbol.as_str()) {
                        self.selected = st.quotes.keys().next().cloned();
                    }
                    st.log(format!("{symbol} fjernet fra watchlisten."));
                    self.sync_watchlist(&st.watchlist);
                }
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
                // Strategi-overstyring for valgt symbol.
                if let Some(sel) = self.selected.clone() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("For {sel}:")).small());
                        let current = st
                            .symbol_strategy
                            .get(&sel)
                            .cloned()
                            .unwrap_or_else(|| "standard".to_string());
                        let mut changed = false;
                        egui::ComboBox::from_id_salt("symbolstrategi")
                            .selected_text(&current)
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(current == "standard", "standard").clicked()
                                    && st.symbol_strategy.remove(&sel).is_some()
                                {
                                    changed = true;
                                }
                                for name in strategy::AVAILABLE {
                                    if ui.selectable_label(current == name, name).clicked() {
                                        st.symbol_strategy.insert(sel.clone(), name.to_string());
                                        st.log(format!("{sel} bruker nå strategien {name}."));
                                        changed = true;
                                    }
                                }
                            });
                        if changed {
                            let _ = self.store.save_symbol_strategies(&st.symbol_strategy);
                        }
                    });
                }
                ui.horizontal(|ui| {
                    if ui.button("🧪 Backtest (2 år)").clicked() {
                        self.backtest = Some(run_backtest(self.selected.as_deref(), &self.strategy_choice, st));
                        self.compare = None;
                    }
                    if ui.button("⚖ Sammenlign alle").clicked() {
                        self.compare = Some(
                            strategy::AVAILABLE
                                .iter()
                                .map(|name| run_backtest(self.selected.as_deref(), name, st))
                                .collect(),
                        );
                        self.backtest = None;
                    }
                });

                // Sammenligningstabell: alle strategier på valgt symbol.
                if let Some(results) = &self.compare {
                    let best = results
                        .iter()
                        .filter_map(|r| r.as_ref().ok())
                        .map(|r| r.return_pct)
                        .fold(f64::MIN, f64::max);
                    egui::Grid::new("sammenlign").striped(true).min_col_width(52.0).show(ui, |ui| {
                        for h in ["Strategi", "Avkastning", "Fall", "Handler", "Treff"] {
                            ui.label(RichText::new(h).strong().color(GRAY));
                        }
                        ui.end_row();
                        for r in results {
                            match r {
                                Ok(r) => {
                                    let is_best = (r.return_pct - best).abs() < 1e-9;
                                    let name = if is_best {
                                        RichText::new(format!("🏆 {}", r.strategy)).color(GREEN).strong()
                                    } else {
                                        RichText::new(r.strategy.clone())
                                    };
                                    ui.label(name);
                                    let color = if r.return_pct >= 0.0 { GREEN } else { RED };
                                    ui.label(RichText::new(format!("{:+.1} %", r.return_pct)).color(color));
                                    ui.label(RichText::new(format!("{:.1} %", r.max_drawdown_pct)).color(RED));
                                    ui.label(format!("{}", r.trades.len()));
                                    let hits = if r.trades.is_empty() {
                                        "–".to_string()
                                    } else {
                                        format!("{}/{}", r.wins(), r.trades.len())
                                    };
                                    ui.label(hits);
                                    ui.end_row();
                                }
                                Err(e) => {
                                    ui.label(RichText::new(e).color(RED).small());
                                    ui.end_row();
                                }
                            }
                        }
                    });
                    ui.small("Samme periode og kostnader for alle. 🏆 = best avkastning.");
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
                    let price_nok = st
                        .quotes
                        .get(&symbol)
                        .map(|q| to_nok(st, &q.currency, q.last))
                        .unwrap_or(0.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&symbol).strong());
                        ui.selectable_value(&mut self.trade_use_kr, true, "💰 kroner")
                            .on_hover_text("Skriv hvor mye du vil handle for — appen regner ut antallet.");
                        ui.selectable_value(&mut self.trade_use_kr, false, "antall stk");
                    });
                    if self.trade_use_kr {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut self.trade_amount_kr)
                                    .range(100.0..=100_000_000.0)
                                    .speed(100)
                                    .max_decimals(0)
                                    .suffix(" kr"),
                            );
                            for beløp in [1_000.0, 5_000.0, 10_000.0] {
                                if ui.small_button(fmt_thousands(beløp)).clicked() {
                                    self.trade_amount_kr = beløp;
                                }
                            }
                            if ui
                                .small_button("Maks")
                                .on_hover_text("Alle tilgjengelige kontanter")
                                .clicked()
                            {
                                self.trade_amount_kr = st.cash.floor().max(0.0);
                            }
                        });
                        if price_nok > 0.0 {
                            let antall = kjopbart_antall(&symbol, self.trade_amount_kr, price_nok);
                            if antall > 0.0 {
                                ui.small(format!(
                                    "≈ {} stk til ca. {price_nok:.2} kr",
                                    fmt_qty(antall)
                                ));
                            } else {
                                ui.small(
                                    RichText::new(format!(
                                        "Beløpet holder ikke til én aksje (kurs ca. {price_nok:.0} kr)."
                                    ))
                                    .color(YELLOW),
                                );
                            }
                        }
                    } else {
                        ui.horizontal(|ui| {
                            ui.label("antall:");
                            ui.add(
                                egui::DragValue::new(&mut self.trade_qty)
                                    .range(1.0..=1_000_000.0)
                                    .speed(1)
                                    .max_decimals(0),
                            );
                        });
                    }
                    ui.horizontal(|ui| {
                        let qty = if self.trade_use_kr {
                            if price_nok > 0.0 {
                                kjopbart_antall(&symbol, self.trade_amount_kr, price_nok)
                            } else {
                                0.0
                            }
                        } else {
                            self.trade_qty
                        };
                        let klar = qty > 0.0 && price_nok > 0.0;
                        if ui
                            .add_enabled(
                                klar,
                                egui::Button::new(RichText::new("KJØP").color(Color32::BLACK).strong()).fill(GREEN),
                            )
                            .clicked()
                        {
                            self.pending_order =
                                Some(PendingOrder { symbol: symbol.clone(), side: Side::Buy, qty, price_nok });
                        }
                        if ui
                            .add_enabled(
                                klar,
                                egui::Button::new(RichText::new("SELG").color(Color32::WHITE).strong()).fill(RED),
                            )
                            .clicked()
                        {
                            // I kronemodus: ikke selg mer enn du eier.
                            let eid = st.positions.iter().find(|p| p.symbol == symbol).map(|p| p.qty);
                            let qty = match (self.trade_use_kr, eid) {
                                (true, Some(e)) => qty.min(e),
                                _ => qty,
                            };
                            self.pending_order =
                                Some(PendingOrder { symbol: symbol.clone(), side: Side::Sell, qty, price_nok });
                        }
                    });
                    ui.small("Du får en oppsummering i klartekst før ordren sendes.");
                } else {
                    ui.small("Velg et symbol i watchlisten først.");
                }

                // Limit-ordrer: handle automatisk når kursen når et nivå.
                ui.add_space(10.0);
                ui.separator();
                section_heading(ui, "💤 Ventende ordrer");
                ui.small("Handler automatisk når kursen når nivået ditt — også når du ikke ser på.");
                if let Some(sel) = self.selected.clone() {
                    let current = st.quotes.get(&sel).map(|q| q.last).unwrap_or(0.0);
                    if self.limit_level <= 0.0 && current > 0.0 {
                        self.limit_level = (current * 100.0).round() / 100.0;
                    }
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&sel).strong());
                        ui.label("nivå:");
                        ui.add(
                            egui::DragValue::new(&mut self.limit_level)
                                .range(0.0..=100_000_000.0)
                                .speed(0.5)
                                .max_decimals(2),
                        );
                    });
                    ui.horizontal(|ui| {
                        let (qty, amount) = if self.trade_use_kr {
                            (0.0, self.trade_amount_kr)
                        } else {
                            (self.trade_qty, 0.0)
                        };
                        let hva = if self.trade_use_kr {
                            format!("for {} kr", fmt_thousands(self.trade_amount_kr))
                        } else {
                            format!("{:.0} stk", self.trade_qty)
                        };
                        if ui
                            .button(RichText::new("KJØP hvis ≤ nivå").color(GREEN))
                            .on_hover_text(format!("Kjøp {hva} hvis kursen faller til {:.2} eller lavere", self.limit_level))
                            .clicked()
                            && self.limit_level > 0.0
                        {
                            st.limit_orders.push(crate::state::LimitOrder {
                                symbol: sel.clone(),
                                side: Side::Buy,
                                qty,
                                amount_kr: amount,
                                level: self.limit_level,
                            });
                            let _ = self.store.save_limit_orders(&st.limit_orders);
                            st.log(format!("💤 Limit-ordre: KJØP {sel} {hva} hvis kurs ≤ {:.2}.", self.limit_level));
                        }
                        if ui
                            .button(RichText::new("SELG hvis ≥ nivå").color(RED))
                            .on_hover_text(format!("Selg {hva} hvis kursen stiger til {:.2} eller høyere", self.limit_level))
                            .clicked()
                            && self.limit_level > 0.0
                        {
                            st.limit_orders.push(crate::state::LimitOrder {
                                symbol: sel.clone(),
                                side: Side::Sell,
                                qty,
                                amount_kr: amount,
                                level: self.limit_level,
                            });
                            let _ = self.store.save_limit_orders(&st.limit_orders);
                            st.log(format!("💤 Limit-ordre: SELG {sel} {hva} hvis kurs ≥ {:.2}.", self.limit_level));
                        }
                    });
                }
                let mut delete_limit: Option<usize> = None;
                for (i, lo) in st.limit_orders.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("🗑").clicked() {
                            delete_limit = Some(i);
                        }
                        let hva = if lo.amount_kr > 0.0 {
                            format!("for {} kr", fmt_thousands(lo.amount_kr))
                        } else {
                            format!("{} stk", fmt_qty(lo.qty))
                        };
                        let (retning, farge) = match lo.side {
                            Side::Buy => ("≤", GREEN),
                            Side::Sell => ("≥", RED),
                        };
                        ui.label(
                            RichText::new(format!(
                                "{} {} {hva} når kurs {retning} {:.2}",
                                lo.side, lo.symbol, lo.level
                            ))
                            .color(farge)
                            .small(),
                        );
                    });
                }
                if let Some(i) = delete_limit {
                    st.limit_orders.remove(i);
                    let _ = self.store.save_limit_orders(&st.limit_orders);
                }
                if st.limit_orders.is_empty() {
                    ui.small("Ingen ventende ordrer.");
                }

                // Spareavtaler: månedlig kjøp for fast beløp.
                ui.add_space(10.0);
                ui.separator();
                section_heading(ui, "📅 Spareavtale");
                ui.small("Kjøper automatisk for et fast beløp samme dag hver måned.");
                if let Some(sel) = self.selected.clone() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&sel).strong());
                        ui.add(
                            egui::DragValue::new(&mut self.savings_amount)
                                .range(100.0..=10_000_000.0)
                                .speed(100)
                                .max_decimals(0)
                                .suffix(" kr"),
                        );
                        ui.label("dag");
                        ui.add(
                            egui::DragValue::new(&mut self.savings_day)
                                .range(1..=28)
                                .speed(1),
                        );
                        if ui.button("Legg til").clicked() && self.savings_amount > 0.0 {
                            // Er dagen alt passert denne måneden, starter
                            // avtalen neste måned — ingen overraskelseskjøp nå.
                            let now = chrono::Local::now();
                            let last_run = if chrono::Datelike::day(&now) >= self.savings_day {
                                now.format("%Y-%m").to_string()
                            } else {
                                String::new()
                            };
                            st.savings_plans.push(crate::state::SavingsPlan {
                                symbol: sel.clone(),
                                amount_kr: self.savings_amount,
                                day: self.savings_day,
                                last_run,
                            });
                            let _ = self.store.save_savings_plans(&st.savings_plans);
                            st.log(format!(
                                "📅 Spareavtale: {} kr i {sel} den {}. hver måned.",
                                fmt_thousands(self.savings_amount),
                                self.savings_day
                            ));
                        }
                    });
                }
                let mut delete_plan: Option<usize> = None;
                for (i, p) in st.savings_plans.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("🗑").clicked() {
                            delete_plan = Some(i);
                        }
                        let sist = if p.last_run.is_empty() {
                            String::new()
                        } else {
                            format!(" (sist: {})", p.last_run)
                        };
                        ui.label(
                            RichText::new(format!(
                                "{} kr i {} den {}. hver mnd{sist}",
                                fmt_thousands(p.amount_kr),
                                p.symbol,
                                p.day
                            ))
                            .color(BLUE)
                            .small(),
                        );
                    });
                }
                if let Some(i) = delete_plan {
                    st.savings_plans.remove(i);
                    let _ = self.store.save_savings_plans(&st.savings_plans);
                }
                if st.savings_plans.is_empty() {
                    ui.small("Ingen spareavtaler — jevn månedlig sparing slår som regel timing.");
                }

                // Kursalarmer
                ui.add_space(10.0);
                ui.separator();
                section_heading(ui, "🔔 Alarmer");
                if let Some(sel) = self.selected.clone() {
                    if self.alarm_level <= 0.0 {
                        if let Some(q) = st.quotes.get(&sel) {
                            self.alarm_level = (q.last * 100.0).round() / 100.0;
                        }
                    }
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&sel).strong());
                        let dir_text = if self.alarm_above { "over ▲" } else { "under ▼" };
                        if ui.button(dir_text).clicked() {
                            self.alarm_above = !self.alarm_above;
                        }
                        ui.add(
                            egui::DragValue::new(&mut self.alarm_level)
                                .range(0.0..=10_000_000.0)
                                .speed(0.5)
                                .max_decimals(2),
                        );
                        if ui.button("Legg til").clicked() && self.alarm_level > 0.0 {
                            st.alarms.push(Alarm {
                                symbol: sel.clone(),
                                level: self.alarm_level,
                                above: self.alarm_above,
                                triggered: false,
                            });
                            let _ = self.store.save_alarms(&st.alarms);
                            st.log(format!(
                                "Alarm lagt til: {sel} {} {:.2}",
                                if self.alarm_above { "over" } else { "under" },
                                self.alarm_level
                            ));
                        }
                    });
                } else {
                    ui.small("Velg et symbol i watchlisten først.");
                }
                let mut delete: Option<usize> = None;
                for (i, a) in st.alarms.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("🗑").clicked() {
                            delete = Some(i);
                        }
                        let text = format!(
                            "{} {} {:.2}",
                            a.symbol,
                            if a.above { "over" } else { "under" },
                            a.level
                        );
                        if a.triggered {
                            ui.label(RichText::new(format!("{text} — utløst ✔")).color(GRAY).strikethrough());
                        } else {
                            ui.label(RichText::new(text).color(YELLOW));
                        }
                    });
                }
                if let Some(i) = delete {
                    st.alarms.remove(i);
                    let _ = self.store.save_alarms(&st.alarms);
                }
                if st.alarms.is_empty() {
                    ui.small("Ingen alarmer — varsles i logg og på mobil.");
                }

                ui.add_space(10.0);
                ui.separator();
                section_heading(ui, "💼 Posisjoner");
                if st.positions.is_empty() && st.nordnet_positions.is_empty() {
                    ui.small("Ingen posisjoner ennå.");
                }
                let mut sell_request: Option<PendingOrder> = None;
                egui::Grid::new("posisjoner").striped(true).min_col_width(58.0).show(ui, |ui| {
                    ui.label(RichText::new("Symbol").strong().color(GRAY));
                    ui.label(RichText::new("Antall").strong().color(GRAY));
                    ui.label(RichText::new("Verdi").strong().color(GRAY));
                    ui.label(RichText::new("Urealisert").strong().color(GRAY));
                    ui.label("");
                    ui.end_row();
                    for p in &st.positions {
                        ui.label(&p.symbol);
                        ui.label(fmt_qty(p.qty));
                        ui.label(fmt_thousands(p.market_value()));
                        let u = p.unrealized();
                        let color = if u >= 0.0 { GREEN } else { RED };
                        ui.label(RichText::new(format!("{u:+.0}")).color(color));
                        ui.horizontal(|ui| {
                            let price_nok = st
                                .quotes
                                .get(&p.symbol)
                                .map(|q| to_nok(st, &q.currency, q.last))
                                .unwrap_or(p.last);
                            if ui.small_button("Selg alt").clicked() {
                                sell_request = Some(PendingOrder {
                                    symbol: p.symbol.clone(),
                                    side: Side::Sell,
                                    qty: p.qty,
                                    price_nok,
                                });
                            }
                            let halv = if crate::types::is_crypto(&p.symbol) {
                                p.qty / 2.0
                            } else {
                                (p.qty / 2.0).floor()
                            };
                            if halv > 0.0
                                && ui.small_button("½").on_hover_text("Selg halvparten").clicked()
                            {
                                sell_request = Some(PendingOrder {
                                    symbol: p.symbol.clone(),
                                    side: Side::Sell,
                                    qty: halv,
                                    price_nok,
                                });
                            }
                        });
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
                if let Some(req) = sell_request {
                    self.pending_order = Some(req);
                }
                if st.nordnet_enabled {
                    ui.small("[NN] = Nordnet-portefølje (kun lesing).");
                }
                });
            });
    }

    /// Bekreftelseskortet: oppsummerer ordren i klartekst («Du kjøper 14 stk
    /// EQNR for ca. 4 928 kr …») med advarsler, før den legges i køen.
    fn confirm_order_dialog(&mut self, ctx: &egui::Context, st: &mut UiState) {
        let Some(po) = &self.pending_order else { return };
        let (symbol, side, qty, price) = (po.symbol.clone(), po.side, po.qty, po.price_nok);
        let sum = qty * price;
        let mut close = false;
        let mut send = false;

        egui::Window::new(match side {
            Side::Buy => "🟢 Bekreft kjøp",
            Side::Sell => "🔴 Bekreft salg",
        })
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
        .show(ctx, |ui| {
            ui.set_min_width(340.0);
            ui.add_space(4.0);
            let verb = match side {
                Side::Buy => "kjøper",
                Side::Sell => "selger",
            };
            ui.label(
                RichText::new(format!(
                    "Du {verb} {} stk {symbol} til ca. {price:.2} kr",
                    fmt_qty(qty)
                ))
                .size(16.0)
                .strong(),
            );
            ui.label(
                RichText::new(format!("Totalt: ≈ {} kr", fmt_thousands(sum)))
                    .size(20.0)
                    .strong()
                    .color(if side == Side::Buy { GREEN } else { RED }),
            );
            ui.add_space(6.0);

            let posisjon = st.positions.iter().find(|p| p.symbol == symbol);
            match side {
                Side::Buy => {
                    ui.label(format!(
                        "Kontanter etterpå: ≈ {} kr",
                        fmt_thousands(st.cash - sum)
                    ));
                    // Hvor stor bit av porteføljen blir denne aksjen?
                    if st.equity > 0.0 {
                        let eksisterende = posisjon
                            .map(|p| {
                                let kurs_nok = st
                                    .quotes
                                    .get(&p.symbol)
                                    .map(|q| to_nok(st, &q.currency, q.last))
                                    .unwrap_or(p.last);
                                p.qty * kurs_nok
                            })
                            .unwrap_or(0.0);
                        let andel = (eksisterende + sum) / st.equity * 100.0;
                        ui.label(format!("{symbol} blir ca. {andel:.0} % av porteføljen din."));
                        if andel > 25.0 {
                            ui.label(
                                RichText::new(
                                    "⚠ Det er en stor bit av porteføljen — vurder å spre kjøpene.",
                                )
                                .color(YELLOW),
                            );
                        }
                    }
                    if sum > st.cash {
                        ui.label(
                            RichText::new(format!(
                                "⚠ Du har bare {} kr i kontanter — ordren vil trolig bli avvist.",
                                fmt_thousands(st.cash)
                            ))
                            .color(RED),
                        );
                    }
                }
                Side::Sell => match posisjon {
                    Some(p) => {
                        let avg_nok = st
                            .quotes
                            .get(&p.symbol)
                            .map(|q| to_nok(st, &q.currency, p.avg_price))
                            .unwrap_or(p.avg_price);
                        let gevinst = (price - avg_nok) * qty;
                        let pct = if avg_nok > 0.0 { (price / avg_nok - 1.0) * 100.0 } else { 0.0 };
                        let color = if gevinst >= 0.0 { GREEN } else { RED };
                        let tekst = if gevinst >= 0.0 {
                            format!("Du låser inn +{} kr ({pct:+.1} %) i gevinst.", fmt_thousands(gevinst))
                        } else {
                            format!("Du tar et tap på {} kr ({pct:+.1} %).", fmt_thousands(gevinst))
                        };
                        ui.label(RichText::new(tekst).color(color));
                        if qty > p.qty {
                            ui.label(
                                RichText::new(format!(
                                    "⚠ Du eier bare {} stk — ordren vil trolig bli avvist.",
                                    fmt_qty(p.qty)
                                ))
                                .color(RED),
                            );
                        }
                    }
                    None => {
                        ui.label(
                            RichText::new(format!("⚠ Du eier ingen {symbol} — ordren vil trolig bli avvist."))
                                .color(RED),
                        );
                    }
                },
            }

            ui.add_space(4.0);
            ui.small("Utføres på neste tikk, gjennom risikoreglene. Kursen er ca. 15 min forsinket.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let (tekst, fyll, fg) = match side {
                    Side::Buy => ("✅ Bekreft kjøp", GREEN, Color32::BLACK),
                    Side::Sell => ("✅ Bekreft salg", RED, Color32::WHITE),
                };
                if ui
                    .add(egui::Button::new(RichText::new(tekst).color(fg).strong()).fill(fyll))
                    .clicked()
                {
                    send = true;
                }
                if ui.button("Avbryt").clicked() {
                    close = true;
                }
            });
        });

        if send {
            st.manual_orders.push_back((symbol.clone(), side, qty));
            let verb = if side == Side::Buy { "KJØP" } else { "SELG" };
            st.log(format!("Manuell {verb} {symbol} x{} lagt i kø.", fmt_qty(qty)));
            st.toast(format!("{verb} {symbol} sendt — utføres på neste tikk."));
            close = true;
        }
        if close {
            self.pending_order = None;
        }
    }

    /// Velkomstguiden — vises én gang, aller første oppstart.
    fn welcome_dialog(&mut self, ctx: &egui::Context) {
        if !self.welcome_open {
            return;
        }
        egui::Window::new("👋 Velkommen til b-rs!")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -30.0])
            .show(ctx, |ui| {
                ui.set_min_width(430.0);
                ui.add_space(4.0);
                ui.label(RichText::new("Tre ting før du begynner:").strong());
                ui.add_space(6.0);
                ui.label("1️⃣  Appen starter i PAPIR-modus: lekepenger og ekte kurser — helt risikofritt. Live-handel må skrus på bevisst i konfigurasjonen.");
                ui.add_space(4.0);
                ui.label("2️⃣  Søk opp aksjer i venstrepanelet og følg dem — grafen, boten og analysene jobber med watchlisten din.");
                ui.add_space(4.0);
                ui.label("3️⃣  Test alltid en strategi med 🧪 Backtest før du lar den handle, og bruk 🧠 Morgan-fanen når du vil ha analysehjelp.");
                ui.add_space(8.0);
                ui.small("Alt forklares i ❓ Hjelp-fanen. Ingenting handles uten at du bekrefter eller har slått det på selv.");
                ui.add_space(8.0);
                if ui
                    .add(
                        egui::Button::new(RichText::new("Skjønner — la oss begynne! 🚀").color(Color32::BLACK).strong())
                            .fill(GREEN),
                    )
                    .clicked()
                {
                    self.welcome_open = false;
                    let _ = self.store.meta_set("welcome_shown", "1");
                }
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

    /// Send Morgan på dypdykk i én bestemt aksje (rapporten vises i 🧠-fanen).
    fn ask_morgan_about(&self, symbol: &str, st: &mut UiState) {
        let Some(key) = morgan_key(st) else { return };
        st.morgan_pending = true;
        st.morgan_error = None;
        st.log(format!("🧠 Morgan dykker ned i {symbol} …"));
        let ctx_json = crate::morgan::symbol_context(st, symbol);
        let state = self.state.clone();
        let symbol = symbol.to_string();
        self.rt.spawn(async move {
            let result = crate::morgan::analyze_symbol(&key, &symbol, &ctx_json).await;
            deliver_morgan_result(&state, result, "dypdykket");
        });
    }

    /// Be Morgan vurdere hele porteføljen (rapporten vises i 🧠-fanen).
    fn ask_morgan_portfolio(&self, st: &mut UiState) {
        let Some(key) = morgan_key(st) else { return };
        st.morgan_pending = true;
        st.morgan_error = None;
        st.log("🧠 Morgan vurderer porteføljen …");
        let ctx_json = crate::morgan::portfolio_context(st);
        let state = self.state.clone();
        self.rt.spawn(async move {
            let result = crate::morgan::analyze_portfolio(&key, &ctx_json).await;
            deliver_morgan_result(&state, result, "porteføljevurderingen");
        });
    }

    fn chart_panel(&mut self, ctx: &egui::Context, st: &mut UiState) {
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
                    ui.toggle_value(&mut self.simple_chart, "✨ Enkel")
                        .on_hover_text("Skjul faglinjene — vis bare kursen, grønn når den har steget i perioden, rød når den har falt.");
                    if !self.simple_chart {
                        ui.selectable_value(&mut self.style, ChartStyle::Candles, "🕯 Candles");
                        ui.selectable_value(&mut self.style, ChartStyle::Line, "📈 Linje");
                    }
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

            // Retningen i perioden bestemmer fargen: grønn opp, rød ned —
            // da ser hvem som helst utviklingen uten å lese tall.
            let start_price = price_pts.first().map(|p| p[1]);
            let trend_color = match (price_pts.first(), price_pts.last()) {
                (Some(a), Some(b)) if b[1] >= a[1] => GREEN,
                (Some(_), Some(_)) => RED,
                _ => GREEN,
            };

            // Posisjonen i denne aksjen (om noen) — gir kjøpskurs- og
            // sikkerhetsnett-linjene i grafen.
            let posisjon = st.positions.iter().find(|p| p.symbol == symbol).cloned();
            let (stop_pct, profit_pct) =
                (self.settings.risk.stop_loss_pct, self.settings.risk.take_profit_pct);

            // Kjøps- og salgsmerker: hvor det faktisk ble handlet.
            let (buy_marks, sell_marks): (Vec<[f64; 2]>, Vec<[f64; 2]>) = {
                let mut buys = Vec::new();
                let mut sells = Vec::new();
                for (sym, ts, is_buy, price) in self.chart_fills(st) {
                    if sym == &symbol && cutoff.is_none_or(|c| *ts >= c) {
                        if *is_buy {
                            buys.push([*ts, *price]);
                        } else {
                            sells.push([*ts, *price]);
                        }
                    }
                }
                (buys, sells)
            };

            let valuta = st
                .quotes
                .get(&symbol)
                .map(|q| if q.currency.is_empty() { "kr".to_string() } else { q.currency.clone() })
                .unwrap_or_else(|| "kr".to_string());

            let equity_h = 130.0;
            // Reserver plass til forklaringsboksen og nyhetene under grafen.
            let chart_h = (ui.available_height() - equity_h - 230.0).max(220.0);
            let simple = self.simple_chart;
            let price_pts_for_plot = price_pts.clone();
            Plot::new("kursgraf")
                .height(chart_h)
                .legend(Legend::default())
                .x_axis_formatter(|mark, _range| {
                    chrono::DateTime::from_timestamp(mark.value as i64, 0)
                        .map(|dt| dt.format("%d.%m").to_string())
                        .unwrap_or_default()
                })
                .label_formatter(move |name, value| {
                    let when = chrono::DateTime::from_timestamp(value.x as i64, 0)
                        .map(|dt| dt.format("%d.%m.%Y").to_string())
                        .unwrap_or_default();
                    if name.is_empty() {
                        format!("{when}\n{:.2} {valuta}", value.y)
                    } else {
                        format!("{name}\n{when}\n{:.2} {valuta}", value.y)
                    }
                })
                .show(ui, |plot_ui| {
                    let line_style = if simple { ChartStyle::Line } else { self.style };
                    match line_style {
                        ChartStyle::Line => {
                            let min_y = price_pts_for_plot.iter().map(|p| p[1]).fold(f64::MAX, f64::min);
                            plot_ui.line(
                                Line::new(PlotPoints::from(price_pts_for_plot))
                                    .color(trend_color)
                                    .width(2.2)
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
                    if !simple {
                        plot_ui.line(
                            Line::new(PlotPoints::from(fast_pts.clone()))
                                .color(YELLOW)
                                .width(1.2)
                                .name(format!("SMA {fast_n} (rask)")),
                        );
                        plot_ui.line(
                            Line::new(PlotPoints::from(slow_pts.clone()))
                                .color(BLUE)
                                .width(1.2)
                                .name(format!("SMA {slow_n} (treg)")),
                        );
                    }

                    // Der perioden startet — alt over er pluss, alt under er minus.
                    if let Some(start) = start_price {
                        plot_ui.hline(
                            HLine::new(start)
                                .color(GRAY)
                                .width(1.0)
                                .style(LineStyle::dashed_loose())
                                .name("Periodestart"),
                        );
                    }

                    // Eier du aksjen, tegnes kjøpskursen og sikkerhetsnettene inn.
                    if let Some(p) = &posisjon {
                        plot_ui.hline(
                            HLine::new(p.avg_price)
                                .color(BLUE)
                                .width(1.4)
                                .style(LineStyle::dashed_dense())
                                .name(format!("Din kjøpskurs {:.2}", p.avg_price)),
                        );
                        if stop_pct > 0.0 {
                            let nivaa = p.avg_price * (1.0 - stop_pct / 100.0);
                            plot_ui.hline(
                                HLine::new(nivaa)
                                    .color(RED)
                                    .width(1.0)
                                    .style(LineStyle::dashed_loose())
                                    .name(format!("Stop-loss {nivaa:.2} (−{stop_pct:.0} %)")),
                            );
                        }
                        if profit_pct > 0.0 {
                            let nivaa = p.avg_price * (1.0 + profit_pct / 100.0);
                            plot_ui.hline(
                                HLine::new(nivaa)
                                    .color(GREEN)
                                    .width(1.0)
                                    .style(LineStyle::dashed_loose())
                                    .name(format!("Gevinstsikring {nivaa:.2} (+{profit_pct:.0} %)")),
                            );
                        }
                    }

                    // ▲ = kjøpt her, ▼ = solgt her.
                    if !buy_marks.is_empty() {
                        plot_ui.points(
                            Points::new(PlotPoints::from(buy_marks))
                                .shape(MarkerShape::Up)
                                .radius(6.0)
                                .color(GREEN)
                                .name("Kjøpt her ▲"),
                        );
                    }
                    if !sell_marks.is_empty() {
                        plot_ui.points(
                            Points::new(PlotPoints::from(sell_marks))
                                .shape(MarkerShape::Down)
                                .radius(6.0)
                                .color(RED)
                                .name("Solgt her ▼"),
                        );
                    }
                });
            if !self.simple_chart {
                ui.small("Strategien sma_cross kjøper når gul (rask) krysser over blå (treg), og selger ved kryss under. Zoom med musehjulet.")
                    .on_hover_text("SMA = gjennomsnittskursen siste N dager. Når det korte snittet krysser det lange, har retningen ofte snudd.");
            }

            // «Hva ser jeg?» — grafen forklart i klartekst.
            let forklaring = chart_explainer(&symbol, st, &price_pts, self.range, fast_pts.last().zip(slow_pts.last()).map(|(f, s)| f[1] >= s[1]), posisjon.as_ref());
            egui::Frame::none()
                .fill(BG_CARD)
                .stroke(egui::Stroke::new(1.0, BORDER))
                .rounding(egui::Rounding::same(8.0))
                .inner_margin(egui::Margin::symmetric(12.0, 8.0))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("💡 Hva ser jeg?").strong().color(GRAY));
                        ui.label(forklaring);
                    });
                });

            // 📰 Nyheter for valgt symbol — hentes når du bytter aksje.
            if st.news_symbol != symbol && !st.news_pending {
                st.news_pending = true;
                st.news_symbol = symbol.clone();
                st.news.clear();
                let market = self.market.clone();
                let state = self.state.clone();
                let sym = symbol.clone();
                self.rt.spawn(async move {
                    let items = market.news(&sym).await.unwrap_or_default();
                    let mut st = state.lock().unwrap();
                    st.news_pending = false;
                    // Bare hvis brukeren fortsatt ser på samme symbol.
                    if st.news_symbol == sym {
                        st.news = items;
                    }
                });
            }
            egui::CollapsingHeader::new(RichText::new("📰 Nyheter").strong())
                .default_open(true)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("nyheter").max_height(90.0).show(ui, |ui| {
                        if st.news_pending {
                            ui.spinner();
                        }
                        for n in &st.news {
                            ui.horizontal_wrapped(|ui| {
                                ui.hyperlink_to(RichText::new(&n.title).small(), &n.url);
                                ui.label(
                                    RichText::new(format!("— {}{}", n.publisher, fmt_news_age(n.ts)))
                                        .small()
                                        .color(GRAY),
                                );
                            });
                        }
                        if !st.news_pending && st.news.is_empty() {
                            ui.small("Fant ingen nyheter for dette symbolet.");
                        }
                    });
                });

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
    fn portfolio_view(&mut self, ctx: &egui::Context, st: &UiState) {
        let realized = self.realized(st).to_vec();
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

                // Sparemålet: fremdriftslinje mot beløpet og året i konfigen.
                if self.settings.goal.amount > 0.0 {
                    use chrono::Datelike;
                    let goal = self.settings.goal.amount;
                    let frac = (st.equity / goal).clamp(0.0, 1.0) as f32;
                    ui.add(
                        egui::ProgressBar::new(frac)
                            .fill(GREEN)
                            .text(
                                RichText::new(format!(
                                    "🎯 {} av {} kr ({:.0} %)",
                                    fmt_thousands(st.equity),
                                    fmt_thousands(goal),
                                    frac * 100.0
                                ))
                                .color(Color32::WHITE)
                                .strong(),
                            ),
                    );
                    if st.equity >= goal {
                        ui.label(RichText::new("Målet er nådd! 🎉").color(GREEN).strong());
                    } else if self.settings.goal.year > 0 {
                        let now = chrono::Local::now();
                        let months = ((self.settings.goal.year - now.year()) * 12
                            + (12 - now.month() as i32))
                            .max(1);
                        let trengs = (goal - st.equity) / months as f64;
                        ui.small(format!(
                            "Mål: {} kr innen utgangen av {}. Med jevn sparing trengs ca. {} kr/mnd ({} måneder igjen). Settes i ⚙ Innstillinger.",
                            fmt_thousands(goal),
                            self.settings.goal.year,
                            fmt_thousands(trengs),
                            months
                        ));
                    }
                    ui.add_space(12.0);
                }

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

                // Forventet utbytte fra beholdningen (siste 12 mnd per aksje).
                let annual_div: f64 = st
                    .positions
                    .iter()
                    .filter_map(|p| {
                        st.dividends.get(&p.symbol).map(|d| {
                            let cur = st
                                .quotes
                                .get(&p.symbol)
                                .map(|q| q.currency.clone())
                                .unwrap_or_default();
                            to_nok(st, &cur, d * p.qty)
                        })
                    })
                    .sum();
                if annual_div > 0.0 {
                    ui.add_space(14.0);
                    section_heading(ui, "💰 Utbytte");
                    let value_nok: f64 = st
                        .positions
                        .iter()
                        .map(|p| {
                            let cur = st
                                .quotes
                                .get(&p.symbol)
                                .map(|q| q.currency.clone())
                                .unwrap_or_default();
                            to_nok(st, &cur, p.market_value())
                        })
                        .sum();
                    let yield_pct = if value_nok > 0.0 { annual_div / value_nok * 100.0 } else { 0.0 };
                    ui.horizontal(|ui| {
                        stat_card(ui, "Forventet årlig utbytte", format!("{} kr", fmt_thousands(annual_div)), GREEN);
                        stat_card(ui, "Utbytteyield", format!("{yield_pct:.1} %"), GREEN);
                    });
                    ui.small("Basert på utbetalt utbytte siste 12 måneder per aksje du eier — ingen garanti for neste år.");
                }

                // Langsiktig utvikling: ett punkt per dag, mot referanseindeksen.
                ui.add_space(14.0);
                section_heading(ui, "📈 Utvikling over tid");
                if st.equity_daily.len() > 1 {
                    let pts: Vec<[f64; 2]> = st.equity_daily.iter().map(|&(t, v)| [t, v]).collect();
                    let (start_ts, start_eq) = (pts[0][0], pts[0][1]);
                    // Indeksen skaleres til samme startverdi som porteføljen —
                    // da er linjene direkte sammenlignbare.
                    let bench: Vec<[f64; 2]> = {
                        let b0 = st
                            .benchmark
                            .iter()
                            .find(|(t, _)| *t >= start_ts)
                            .map(|&(_, v)| v)
                            .filter(|v| *v > 0.0);
                        match b0 {
                            Some(b0) => st
                                .benchmark
                                .iter()
                                .filter(|(t, _)| *t >= start_ts)
                                .map(|&(t, v)| [t, start_eq * v / b0])
                                .collect(),
                            None => Vec::new(),
                        }
                    };
                    let farge = if pts.last().is_some_and(|p| p[1] >= start_eq) { GREEN } else { RED };
                    let bench_navn = if st.benchmark_name.is_empty() {
                        "Indeks".to_string()
                    } else {
                        st.benchmark_name.clone()
                    };
                    Plot::new("portefolje_lang")
                        .height(220.0)
                        .legend(Legend::default())
                        .x_axis_formatter(|mark, _range| {
                            chrono::DateTime::from_timestamp(mark.value as i64, 0)
                                .map(|dt| dt.format("%d.%m.%y").to_string())
                                .unwrap_or_default()
                        })
                        .show(ui, |plot_ui| {
                            plot_ui.line(
                                Line::new(PlotPoints::from(pts)).color(farge).width(2.2).name("Porteføljen"),
                            );
                            if bench.len() > 1 {
                                plot_ui.line(
                                    Line::new(PlotPoints::from(bench))
                                        .color(GRAY)
                                        .width(1.4)
                                        .style(LineStyle::dashed_dense())
                                        .name(format!("{bench_navn} (indeksert)")),
                                );
                            }
                        });
                    ui.small(
                        "Referanseindeksen er skalert til samme startverdi — ligger porteføljelinjen over, slår du børsen. \
                         Ett punkt lagres per dag appen er åpen.",
                    );
                } else {
                    ui.small("Bygges opp etter hvert som appen lagrer ett egenkapital-punkt per dag — kom tilbake i morgen.");
                }

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

                    // Realisert gevinst/tap (FIFO) — det skattbare.
                    ui.add_space(10.0);
                    section_heading(ui, "✅ Realisert gevinst/tap");
                    let this_year = chrono::Utc::now().format("%Y").to_string().parse().unwrap_or(0);
                    let year_gain = pnl::total_gain(&realized, this_year);
                    let all_gain = pnl::total_gain(&realized, 0);
                    ui.horizontal(|ui| {
                        ui.label(format!("I år ({this_year}):"));
                        ui.label(RichText::new(format!("{}{} kr", plus(year_gain), fmt_thousands(year_gain))).color(updown(year_gain)).strong());
                        ui.separator();
                        ui.label("Totalt:");
                        ui.label(RichText::new(format!("{}{} kr", plus(all_gain), fmt_thousands(all_gain))).color(updown(all_gain)).strong());
                        ui.separator();
                        ui.label(format!("{} realiserte salg", realized.len()));
                    });
                    ui.small("FIFO-beregnet fra fylte ordrer — eksporter skatterapport under 💳 Transaksjoner.");

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

    fn transactions_view(&mut self, ctx: &egui::Context, st: &mut UiState) {
        let realized = self.realized(st).to_vec();
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(4.0);
            ui.heading(RichText::new("💳 Transaksjoner").strong());
            ui.small(format!(
                "Komplett historikk fra databasen ({} transaksjoner) — også fra tidligere økter.",
                st.transactions.len()
            ));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("📤 Eksporter skatterapport (CSV)").clicked() {
                    let path = std::path::Path::new("b-rs-realisert-gevinst.csv");
                    match pnl::export_realized_csv(&realized, path) {
                        Ok(()) => st.log(format!(
                            "Skatterapport skrevet til {} ({} realiserte salg).",
                            path.display(),
                            realized.len()
                        )),
                        Err(e) => st.log(format!("Eksport feilet: {e:#}")),
                    }
                }
                if ui.button("📤 Eksporter alle transaksjoner (CSV)").clicked() {
                    let path = std::path::Path::new("b-rs-transaksjoner.csv");
                    match export_transactions_csv(&self.store, path) {
                        Ok(n) => st.log(format!("{n} transaksjoner skrevet til {}.", path.display())),
                        Err(e) => st.log(format!("Eksport feilet: {e:#}")),
                    }
                }
                ui.label(RichText::new("(semikolon-separert — åpnes rett i Excel)").small().color(GRAY));
            });
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

    fn settings_view(&mut self, ctx: &egui::Context, st: &mut UiState) {
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().id_salt("innstillinger_scroll").show(ui, |ui| {
                ui.add_space(4.0);
                ui.heading(RichText::new("⚙ Innstillinger").strong());
                ui.small(format!(
                    "Lagres til {} — de fleste endringer krever omstart av appen. Modus/megler endres i filen (med vilje).",
                    self.config_path.display()
                ));
                ui.add_space(10.0);

                let s = &mut self.settings;
                section_heading(ui, "Generelt");
                egui::Grid::new("innst_generelt").min_col_width(190.0).show(ui, |ui| {
                    ui.label("Sekunder mellom kursoppdateringer");
                    ui.add(egui::DragValue::new(&mut s.poll_secs).range(10..=300));
                    ui.end_row();
                    ui.label("Handle bare i børsens åpningstid");
                    ui.checkbox(&mut s.market_hours_only, "(krypto handles alltid)");
                    ui.end_row();
                    ui.label("Kontovaluta");
                    ui.text_edit_singleline(&mut s.base_currency);
                    ui.end_row();
                    ui.label("Startkapital (papirmodus)");
                    ui.add(egui::DragValue::new(&mut s.starting_cash).range(1000.0..=100_000_000.0).speed(1000));
                    ui.end_row();
                    ui.label("Nullstill papirporteføljen ved neste start");
                    ui.checkbox(&mut s.paper_reset, "");
                    ui.end_row();
                });

                ui.add_space(10.0);
                section_heading(ui, "Strategi (standard)");
                egui::Grid::new("innst_strategi").min_col_width(190.0).show(ui, |ui| {
                    ui.label("Strategi");
                    egui::ComboBox::from_id_salt("innst_strateginavn")
                        .selected_text(&s.strategy.name)
                        .show_ui(ui, |ui| {
                            for name in strategy::AVAILABLE {
                                ui.selectable_value(&mut s.strategy.name, name.to_string(), name);
                            }
                        });
                    ui.end_row();
                    ui.label("Kjøp for beløp per ordre (kr; 0 = bruk antall)");
                    ui.add(egui::DragValue::new(&mut s.strategy.order_value).range(0.0..=10_000_000.0).speed(500));
                    ui.end_row();
                    ui.label("Antall per ordre (når beløp = 0)");
                    ui.add(egui::DragValue::new(&mut s.strategy.order_qty).range(1.0..=1_000_000.0));
                    ui.end_row();
                    ui.label("SMA rask / treg");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut s.strategy.fast).range(2..=100));
                        ui.add(egui::DragValue::new(&mut s.strategy.slow).range(3..=400));
                    });
                    ui.end_row();
                    ui.label("RSI periode / kjøp under / selg over");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut s.strategy.rsi_period).range(2..=50));
                        ui.add(egui::DragValue::new(&mut s.strategy.rsi_buy_below).range(1.0..=50.0));
                        ui.add(egui::DragValue::new(&mut s.strategy.rsi_sell_above).range(50.0..=99.0));
                    });
                    ui.end_row();
                    ui.label("Momentum-vindu (dager)");
                    ui.add(egui::DragValue::new(&mut s.strategy.momentum_window).range(3..=200));
                    ui.end_row();
                });

                ui.add_space(10.0);
                section_heading(ui, "Risiko");
                egui::Grid::new("innst_risiko").min_col_width(190.0).show(ui, |ui| {
                    ui.label("Maks verdi per ordre (kr)");
                    ui.add(egui::DragValue::new(&mut s.risk.max_order_value).range(100.0..=100_000_000.0).speed(500));
                    ui.end_row();
                    ui.label("Maks posisjonsverdi per aksje (kr)");
                    ui.add(egui::DragValue::new(&mut s.risk.max_position_value).range(100.0..=100_000_000.0).speed(500));
                    ui.end_row();
                    ui.label("Maks ordrer per minutt");
                    ui.add(egui::DragValue::new(&mut s.risk.max_orders_per_min).range(1..=60));
                    ui.end_row();
                    ui.label("Stopp all handel ved tap på (kr)");
                    ui.add(egui::DragValue::new(&mut s.risk.max_daily_loss).range(100.0..=100_000_000.0).speed(500));
                    ui.end_row();
                    ui.label("Stop-loss % fra kjøpskurs (0 = av)");
                    ui.add(egui::DragValue::new(&mut s.risk.stop_loss_pct).range(0.0..=90.0).speed(0.5));
                    ui.end_row();
                    ui.label("Take-profit % (0 = av)");
                    ui.add(egui::DragValue::new(&mut s.risk.take_profit_pct).range(0.0..=500.0).speed(0.5));
                    ui.end_row();
                    ui.label("Trailing stop % fra topp (0 = av)");
                    ui.add(egui::DragValue::new(&mut s.risk.trailing_stop_pct).range(0.0..=90.0).speed(0.5));
                    ui.end_row();
                });

                ui.add_space(10.0);
                section_heading(ui, "Sparemål");
                egui::Grid::new("innst_maal").min_col_width(190.0).show(ui, |ui| {
                    ui.label("Målbeløp (kr, 0 = av)");
                    ui.add(egui::DragValue::new(&mut s.goal.amount).range(0.0..=1_000_000_000.0).speed(5000));
                    ui.end_row();
                    ui.label("Innen år");
                    ui.add(egui::DragValue::new(&mut s.goal.year).range(0..=2100));
                    ui.end_row();
                });
                ui.small("Vises som fremdriftslinje øverst i 💼 Portefølje.");

                ui.add_space(10.0);
                section_heading(ui, "Mobilvarsler");
                egui::Grid::new("innst_varsler").min_col_width(190.0).show(ui, |ui| {
                    ui.label("Varsler på");
                    ui.checkbox(&mut s.notify.enabled, "");
                    ui.end_row();
                    ui.label("Tjeneste");
                    egui::ComboBox::from_id_salt("innst_varseltjeneste")
                        .selected_text(&s.notify.provider)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut s.notify.provider, "ntfy".into(), "ntfy");
                            ui.selectable_value(&mut s.notify.provider, "telegram".into(), "telegram");
                        });
                    ui.end_row();
                    ui.label("ntfy-emne (hemmelig navn)");
                    ui.text_edit_singleline(&mut s.notify.ntfy_topic);
                    ui.end_row();
                    ui.label("Telegram chat-id");
                    ui.text_edit_singleline(&mut s.notify.telegram_chat_id);
                    ui.end_row();
                });

                // Autostart med Windows.
                if std::env::consts::OS == "windows" {
                    ui.add_space(10.0);
                    section_heading(ui, "Windows");
                    let mut auto = autostart_enabled();
                    if ui.checkbox(&mut auto, "Start b-rs automatisk når Windows starter").changed() {
                        match set_autostart(auto) {
                            Ok(()) => st.log(if auto {
                                "Autostart skrudd på."
                            } else {
                                "Autostart skrudd av."
                            }),
                            Err(e) => st.log(format!("Autostart feilet: {e:#}")),
                        }
                    }
                }

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(RichText::new("💾 Lagre innstillinger").color(Color32::BLACK).strong()).fill(GREEN))
                        .clicked()
                    {
                        self.settings_msg = Some(if self.settings.strategy.fast >= self.settings.strategy.slow {
                            ("SMA rask må være mindre enn treg — ikke lagret.".to_string(), false)
                        } else {
                            match self.write_settings() {
                                Ok(()) => {
                                    st.log(format!("Innstillinger lagret til {}.", self.config_path.display()));
                                    ("Lagret! Start appen på nytt for at alt skal tre i kraft.".to_string(), true)
                                }
                                Err(e) => (format!("Lagring feilet: {e:#}"), false),
                            }
                        });
                    }
                    if let Some((msg, ok)) = &self.settings_msg {
                        ui.label(RichText::new(msg).color(if *ok { GREEN } else { RED }));
                    }
                });
            });
        });
    }

    fn help_view(&self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().id_salt("hjelp_scroll").show(ui, |ui| {
                ui.add_space(4.0);
                ui.heading(RichText::new("❓ Hjelp").strong());
                ui.small(format!("b-rs versjon {}", env!("CARGO_PKG_VERSION")));
                ui.add_space(10.0);

                for (title, body) in HELP_SECTIONS {
                    section_heading(ui, title);
                    ui.label(*body);
                    ui.add_space(12.0);
                }
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
            if !to_follow.is_empty() {
                for symbol in to_follow {
                    st.follow(&symbol);
                }
                self.sync_watchlist(&st.watchlist);
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

    /// «Morgan» — AI-analysesjefen. Investeringsprofil inn, screeningrapport ut.
    fn morgan_view(&mut self, ctx: &egui::Context, st: &mut UiState) {
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().id_salt("morgan_scroll").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.heading(RichText::new("🧠 Morgan — analysesjefen").strong());
                    if st.morgan_pending {
                        ui.spinner();
                        ui.label(RichText::new("analyserer … dette kan ta noen minutter").color(GRAY));
                    }
                });
                ui.small(
                    "Morgan er en tenkt senior aksjeanalytiker (drevet av Claude, Anthropic). Han bruker \
                     appens sanntidsdata og kan tre ting: screene markedet etter profilen din, dykke ned \
                     i én aksje, og vurdere porteføljen din som helhet. AI-generert research — IKKE investeringsråd.",
                );
                ui.add_space(8.0);

                let ledig = !st.morgan_pending;
                ui.columns(3, |cols| {
                    // 1) Full screening etter investeringsprofilen.
                    let ui = &mut cols[0];
                    section_heading(ui, "📋 Screening");
                    ui.small("Topp 10 aksjer etter profilen din: P/E, gjeld, utbytte, vollgrav, kursmål, risiko og inngangssoner.");
                    ui.add(
                        egui::TextEdit::multiline(&mut self.morgan_profile)
                            .desired_rows(5)
                            .desired_width(f32::INFINITY),
                    );
                    let ready = ledig && !self.morgan_profile.trim().is_empty();
                    if ui
                        .add_enabled(
                            ready,
                            egui::Button::new(RichText::new("🧠 Kjør screening").color(Color32::BLACK).strong())
                                .fill(GREEN),
                        )
                        .clicked()
                    {
                        if let Some(key) = morgan_key(st) {
                            st.morgan_pending = true;
                            st.morgan_error = None;
                            st.log(format!("🧠 Morgan screener markedet ({}) …", crate::morgan::MODEL));
                            let profile = self.morgan_profile.trim().to_string();
                            let market_json = crate::morgan::market_context(st);
                            let state = self.state.clone();
                            self.rt.spawn(async move {
                                let result = crate::morgan::analyze(&key, &profile, &market_json).await;
                                deliver_morgan_result(&state, result, "screeningen");
                            });
                        }
                    }

                    // 2) Dypdykk i én aksje.
                    let ui = &mut cols[1];
                    section_heading(ui, "🔍 Dypdykk i én aksje");
                    ui.small("For og imot, nøkkeltall, teknisk bilde, kursmål og nivåer — og en vurdering av posisjonen din hvis du eier den.");
                    if self.morgan_symbol.is_empty() {
                        self.morgan_symbol = self
                            .selected
                            .clone()
                            .or_else(|| st.quotes.keys().next().cloned())
                            .unwrap_or_default();
                    }
                    egui::ComboBox::from_id_salt("morgan_symbol")
                        .selected_text(&self.morgan_symbol)
                        .show_ui(ui, |ui| {
                            for s in st.quotes.keys() {
                                ui.selectable_value(&mut self.morgan_symbol, s.clone(), s);
                            }
                        });
                    let ready = ledig && !self.morgan_symbol.is_empty();
                    if ui
                        .add_enabled(
                            ready,
                            egui::Button::new(RichText::new("🧠 Kjør dypdykk").color(Color32::BLACK).strong())
                                .fill(GREEN),
                        )
                        .clicked()
                    {
                        let symbol = self.morgan_symbol.clone();
                        self.ask_morgan_about(&symbol, st);
                    }

                    // 3) Porteføljevurdering.
                    let ui = &mut cols[2];
                    section_heading(ui, "💼 Porteføljevurdering");
                    ui.small("Konsentrasjon, overlapp, risiko, vurdering per posisjon, hull i porteføljen og tre konkrete grep.");
                    let ready = ledig && !st.positions.is_empty();
                    if ui
                        .add_enabled(
                            ready,
                            egui::Button::new(RichText::new("🧠 Vurder porteføljen").color(Color32::BLACK).strong())
                                .fill(GREEN),
                        )
                        .clicked()
                    {
                        self.ask_morgan_portfolio(st);
                    }
                    if st.positions.is_empty() {
                        ui.small("Krever minst én posisjon.");
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!("modell: {} · krever ANTHROPIC_API_KEY · hver analyse er ett betalt API-kall", crate::morgan::MODEL))
                        .color(GRAY)
                        .small(),
                );

                if let Some(err) = &st.morgan_error {
                    ui.add_space(6.0);
                    ui.label(RichText::new(format!("⚠ {err}")).color(RED));
                }

                if let Some(report) = st.morgan_report.clone() {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        section_heading(ui, "Rapport");
                        if ui.button("📋 Kopier").clicked() {
                            ctx.copy_text(report.clone());
                            st.toast("Rapporten er kopiert til utklippstavlen.");
                        }
                        if ui.button("💾 Lagre som fil").clicked() {
                            let name = format!(
                                "morgan-rapport-{}.md",
                                chrono::Local::now().format("%Y-%m-%d-%H%M")
                            );
                            match std::fs::write(&name, &report) {
                                Ok(()) => st.toast(format!("Lagret som {name}")),
                                Err(e) => st.toast(format!("Klarte ikke lagre: {e}")),
                            }
                        }
                    });
                    ui.add_space(4.0);
                    egui::Frame::none()
                        .fill(BG_CARD)
                        .stroke(egui::Stroke::new(1.0, BORDER))
                        .rounding(egui::Rounding::same(10.0))
                        .shadow(card_shadow())
                        .inner_margin(egui::Margin::same(14.0))
                        .show(ui, |ui| {
                            markdown_lite(ui, &report);
                        });
                }
                ui.add_space(12.0);
            });
        });
    }
}

/// Enkel Markdown-visning uten ekstra avhengigheter: overskrifter, skillelinjer
/// og tabeller (monospace). Nok til å gjøre Morgans rapport lettlest.
fn markdown_lite(ui: &mut egui::Ui, text: &str) {
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            ui.add_space(4.0);
        } else if let Some(h) = trimmed.strip_prefix("### ") {
            ui.add_space(4.0);
            ui.label(RichText::new(h.replace("**", "")).strong().size(15.0));
        } else if let Some(h) = trimmed.strip_prefix("## ") {
            ui.add_space(6.0);
            ui.label(RichText::new(h.replace("**", "")).strong().size(17.0).color(GREEN));
        } else if let Some(h) = trimmed.strip_prefix("# ") {
            ui.add_space(8.0);
            ui.label(RichText::new(h.replace("**", "")).strong().size(20.0).color(GREEN));
        } else if trimmed.trim() == "---" {
            ui.separator();
        } else if trimmed.trim_start().starts_with('|') {
            // Tabellrader: monospace så kolonnene står noenlunde rett.
            ui.label(RichText::new(trimmed).monospace().size(12.0));
        } else {
            ui.label(trimmed.replace("**", ""));
        }
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

/// Hjelpetekstene — korte, norske forklaringer på alt i appen.
const HELP_SECTIONS: &[(&str, &str)] = &[
    (
        "🚀 Kom i gang",
        "Appen starter alltid i PAPIR-modus: boten handler med lekepenger og ekte kurser, helt risikofritt. \
         Følg med noen dager, kjør backtester, og juster innstillingene før du i det hele tatt vurderer live-handel. \
         Kursene kommer fra Yahoo Finance og er ca. 15 minutter forsinket — Oslo Børs er åpen 09:00–16:30.",
    ),
    (
        "🧠 Strategiene",
        "sma_cross: kjøper når snittkursen siste 5 dager krysser over snittet siste 20 (og selger ved kryss under) — følger trender.\n\
         rsi: kjøper når aksjen er «oversolgt» (RSI under 30) og selger når den er «overkjøpt» (over 70) — satser på rekyl.\n\
         momentum: kjøper når kursen bryter over det høyeste på 20 dager, selger ved brudd under det laveste — følger utbrudd.\n\
         macd: kjøper når MACD-linjen (12/26) krysser over signallinjen (9), selger ved kryss under — fanger trendskifter.\n\
         bollinger: kjøper når kursen faller under nedre bånd (snitt −2 standardavvik = uvanlig billig mot seg selv), selger over øvre bånd.\n\n\
         Bytt strategi i Strategi-panelet, overstyr per aksje, og test alltid med 🧪 Backtest eller ⚖ Sammenlign først.",
    ),
    (
        "⚡ Kjøp og salg",
        "Enklest mulig: velg en aksje, skriv hvor mange KRONER du vil handle for (eller bruk \
         hurtigknappene 1 000/5 000/10 000/Maks), og trykk KJØP eller SELG. Appen regner ut antallet \
         og viser en oppsummering i klartekst — hva det koster, hva du har igjen etterpå, og hvor \
         stor bit av porteføljen aksjen blir — før noe som helst sendes. På hver posisjon finnes også \
         «Selg alt» og «½», med gevinsten du låser inn vist før du bekrefter.\n\
         I grafen: grønn linje = steget i perioden, rød = falt. ▲/▼ viser hvor det ble kjøpt og \
         solgt, blå stiplet linje er din kjøpskurs, og de røde/grønne stiplede linjene er stop-loss og \
         gevinstsikring. «Hva ser jeg?»-boksen under grafen forklarer alt i vanlige ord — og \
         ✨ Enkel-knappen skjuler faglinjene helt.",
    ),
    (
        "💤 Automatikk som jobber for deg",
        "Ventende ordrer (limit): «kjøp for 5 000 kr hvis kursen faller til 340» — appen handler \
         automatisk når nivået brytes, også når du ikke ser på. KJØP utløses når kursen faller til \
         eller under nivået, SELG når den stiger til eller over.\n\
         Spareavtale: kjøp for et fast kronebeløp samme dag hver måned — jevn sparing slår som \
         regel forsøk på timing. Legges den til etter at dagen er passert, starter den neste måned.\n\
         Ukesrapport: har du mobilvarsler på, får du hver fredag ettermiddag en oppsummering av uken \
         — porteføljens utvikling, beste og svakeste aksje.\n\
         Alt utføres gjennom risikoreglene og stoppes av kill switch. Begge deler overlever omstart.",
    ),
    (
        "🎯 Sparemål",
        "Sett et målbeløp og årstall i ⚙ Innstillinger → Sparemål, så viser 💼 Portefølje en \
         fremdriftslinje mot målet og hvor mye du må spare i måneden for å nå det med jevn sparing.",
    ),
    (
        "🛡️ Sikkerhetsnettene",
        "Stop-loss: selger automatisk hvis en posisjon faller X % under kjøpskursen.\n\
         Take-profit: sikrer gevinst ved +X %.\n\
         Trailing stop: selger hvis kursen faller X % fra toppen etter kjøpet — låser inn gevinst.\n\
         Tapsgrense: all handel stopper hvis porteføljen har tapt mer enn grensen.\n\
         ⛔ KILL SWITCH (knappen øverst): stopper ALT umiddelbart og kansellerer åpne ordrer.",
    ),
    (
        "📊 Begreper",
        "SMA: gjennomsnittskurs over N dager — jevner ut støy.\n\
         RSI: måler om en aksje er «overkjøpt» (nær 100) eller «oversolgt» (nær 0).\n\
         Drawdown / verste fall: hvor dypt porteføljen sank fra toppen — mål på smerte underveis.\n\
         Kjøp-og-hold: hva du hadde fått ved å bare kjøpe og vente — strategien bør slå dette.\n\
         Urealisert: gevinst/tap på papiret. Realisert: låst inn ved salg — det er dette du skatter av.",
    ),
    (
        "📱 Mobilvarsler (ntfy)",
        "1. Installer «ntfy»-appen fra App Store/Google Play (gratis, ingen konto).\n\
         2. Trykk + i appen og abonner på et hemmelig emnenavn du finner på, f.eks. bors-ola-73xk1.\n\
         3. Skriv samme navn i Innstillinger → Mobilvarsler → ntfy-emne, slå på varsler, lagre og start appen på nytt.\n\
         Du får da varsel ved hver handel, kill switch, tapsgrense, alarmer og oppstart.",
    ),
    (
        "🧠 Morgan — AI-analysesjefen",
        "Morgan er en tenkt senior aksjeanalytiker drevet av Claude (Anthropic). Alt han kan, ligger \
         i 🧠-fanen: SCREENING (topp 10 aksjer etter profilen din, med P/E, gjeld, utbytte, vollgrav, \
         kursmål, risiko og inngangssoner), DYPDYKK i én aksje (for/imot, nøkkeltall, nivåer og \
         vurdering av posisjonen din), og PORTEFØLJEVURDERING (konsentrasjon, overlapp, hull og tre \
         konkrete grep). Alt basert på appens sanntidsdata.\n\
         Krever en API-nøkkel fra console.anthropic.com i miljøvariabelen ANTHROPIC_API_KEY \
         (Windows: setx ANTHROPIC_API_KEY \"sk-ant-…\", start appen på nytt). Hver analyse er ett \
         betalt API-kall (noen kroner). Fundamentaltallene er fra modellens kunnskap — verifiser før handel.",
    ),
    (
        "🗂 Filene appen bruker",
        "config.toml — innstillingene (redigeres tryggest via ⚙-fanen).\n\
         b-rs.db — databasen: portefølje, alle handler, alarmer. Dette er appens hukommelse!\n\
         backups/ — daglig kopi av databasen, 14 beholdes.\n\
         b-rs.log — logg over alt som skjer, for feilsøking.\n\
         b-rs-realisert-gevinst.csv — skatterapporten (eksporteres fra 💳-fanen).",
    ),
    (
        "⚠️ Viktig",
        "Dette er et hobbyverktøy, ikke investeringsrådgivning. Automatisk handel kan gi raske tap. \
         Ved live-handel via IBKR eller Revolut X er du selv ansvarlig for ordrer og skatt \
         (de rapporterer ikke til Skatteetaten slik norske meglere gjør — bruk skatterapporten i appen).",
    ),
];

/// Er autostart-snarveien på plass i Windows' oppstartsmappe?
fn autostart_enabled() -> bool {
    autostart_path().map(|p| p.exists()).unwrap_or(false)
}

fn autostart_path() -> Option<std::path::PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    Some(
        std::path::Path::new(&appdata)
            .join(r"Microsoft\Windows\Start Menu\Programs\Startup")
            .join("b-rs-gui.cmd"),
    )
}

/// Slå autostart av/på ved å legge/fjerne et lite skript i oppstartsmappen.
fn set_autostart(enable: bool) -> anyhow::Result<()> {
    let path = autostart_path().ok_or_else(|| anyhow::anyhow!("fant ikke APPDATA-mappen"))?;
    if enable {
        let exe = std::env::current_exe()?;
        std::fs::write(&path, format!("@echo off\r\nstart \"\" \"{}\"\r\n", exe.display()))?;
    } else if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Skriv hele transaksjonshistorikken som CSV; returnerer antall rader.
fn export_transactions_csv(store: &Store, path: &std::path::Path) -> anyhow::Result<usize> {
    let txs = store.recent_orders(1_000_000)?;
    let mut out = String::from("Tidspunkt;Side;Symbol;Antall;Kurs;Beløp;Status;Megler;Merknad\n");
    for t in &txs {
        out.push_str(&format!(
            "{};{};{};{:.4};{:.4};{:.2};{};{};{}\n",
            t.ts,
            t.side,
            t.symbol,
            t.qty,
            t.price,
            t.qty * t.price,
            t.status,
            t.broker,
            t.note.replace(';', ",")
        ));
    }
    std::fs::write(path, out)?;
    Ok(txs.len())
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
/// Grafen oppsummert i klartekst for folk som ikke leser candlesticks:
/// retning i valgt periode, trend, RSI på folkemunne og din egen posisjon.
fn chart_explainer(
    symbol: &str,
    st: &UiState,
    price_pts: &[[f64; 2]],
    range: ChartRange,
    trend_up: Option<bool>,
    posisjon: Option<&crate::types::Position>,
) -> String {
    let periode = match range {
        ChartRange::Week => "den siste uken",
        ChartRange::Month => "den siste måneden",
        ChartRange::ThreeMonths => "de siste 3 månedene",
        ChartRange::Year => "det siste året",
        ChartRange::All => "hele perioden",
    };
    let mut deler: Vec<String> = Vec::new();

    if let (Some(a), Some(b)) = (price_pts.first(), price_pts.last()) {
        if a[1] > 0.0 {
            let pct = (b[1] / a[1] - 1.0) * 100.0;
            if pct >= 0.0 {
                deler.push(format!("📈 {symbol} har steget {pct:.1} % {periode}."));
            } else {
                deler.push(format!("📉 {symbol} har falt {:.1} % {periode}.", -pct));
            }
        }
    }

    match trend_up {
        Some(true) => deler.push("Trenden peker opp — snittkursen de siste dagene ligger over det lengre snittet.".into()),
        Some(false) => deler.push("Trenden peker ned — snittkursen de siste dagene ligger under det lengre snittet.".into()),
        None => {}
    }

    if let Some(h) = st.history.get(symbol) {
        let closes: Vec<f64> = h.iter().map(|&(_, p)| p).collect();
        if let Some(v) = crate::market::rsi(&closes, 14) {
            if v <= 30.0 {
                deler.push(format!("RSI på {v:.0} betyr at aksjen kan være billig akkurat nå (oversolgt)."));
            } else if v >= 70.0 {
                deler.push(format!("RSI på {v:.0} betyr at aksjen kan være dyr akkurat nå (overkjøpt)."));
            } else {
                deler.push(format!("RSI på {v:.0} er normalt — verken billig eller dyr."));
            }
        }
    }

    match posisjon {
        Some(p) if p.avg_price > 0.0 => {
            let pct = (p.last / p.avg_price - 1.0) * 100.0;
            let retning = if pct >= 0.0 { "pluss" } else { "minus" };
            deler.push(format!(
                "Du eier {} stk kjøpt til snitt {:.2} (blå stiplet linje) — du ligger {pct:+.1} % i {retning}.",
                fmt_qty(p.qty),
                p.avg_price
            ));
        }
        _ => deler.push(format!("Du eier ingen {symbol} akkurat nå.")),
    }

    deler.join(" ")
}

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

/// Hent API-nøkkelen til Morgan, eller forklar hva som mangler.
fn morgan_key(st: &mut UiState) -> Option<String> {
    match std::env::var("ANTHROPIC_API_KEY") {
        Ok(key) => Some(key),
        Err(_) => {
            st.morgan_error = Some(
                "Mangler API-nøkkel. Opprett en på console.anthropic.com og sett \
                 miljøvariabelen ANTHROPIC_API_KEY før du starter appen \
                 (Windows: setx ANTHROPIC_API_KEY \"sk-ant-…\", start appen på nytt)."
                    .into(),
            );
            None
        }
    }
}

/// Felles landing for alle Morgan-svar: rapport eller feilmelding inn i tilstanden.
fn deliver_morgan_result(state: &SharedState, result: Result<String>, hva: &str) {
    let mut st = state.lock().unwrap();
    st.morgan_pending = false;
    match result {
        Ok(report) => {
            st.morgan_report = Some(report);
            st.toast(format!("🧠 Morgan er ferdig med {hva}."));
            st.log(format!("🧠 Morgan leverte {hva}."));
        }
        Err(e) => {
            let msg = format!("{e:#}");
            st.log(format!("🧠 Morgan feilet: {msg}"));
            st.morgan_error = Some(msg);
        }
    }
}

/// «for 2 t siden» / «for 3 d siden» — alder på en nyhetssak.
fn fmt_news_age(ts: i64) -> String {
    if ts <= 0 {
        return String::new();
    }
    let secs = (chrono::Utc::now().timestamp() - ts).max(0);
    match secs {
        s if s < 3600 => format!(", for {} min siden", s / 60),
        s if s < 86400 => format!(", for {} t siden", s / 3600),
        s => format!(", for {} d siden", s / 86400),
    }
}

/// Omregn en verdi i instrumentets valuta til kontovaluta (kroner).
fn to_nok(st: &UiState, currency: &str, value: f64) -> f64 {
    if currency.is_empty() {
        return value;
    }
    match st.fx_rates.get(currency) {
        Some(rate) => value * rate,
        None => value, // samme valuta som kontoen, eller kurs ikke hentet ennå
    }
}

/// Hvor mange aksjer får du for beløpet? Hele aksjer for aksjer,
/// brøkdeler for krypto.
fn kjopbart_antall(symbol: &str, belop_kr: f64, price_nok: f64) -> f64 {
    if price_nok <= 0.0 {
        return 0.0;
    }
    if crate::types::is_crypto(symbol) {
        belop_kr / price_nok
    } else {
        (belop_kr / price_nok).floor()
    }
}

/// Antall med brøkdeler bare når det trengs (krypto).
fn fmt_qty(qty: f64) -> String {
    if (qty - qty.round()).abs() < 1e-9 {
        format!("{qty:.0}")
    } else {
        format!("{qty:.6}").trim_end_matches('0').trim_end_matches('.').to_string()
    }
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

#[cfg(test)]
mod tests {
    use super::{fmt_qty, kjopbart_antall};

    #[test]
    fn kronebelop_gir_hele_aksjer() {
        // 5 000 kr / 352 kr = 14,2 → 14 hele aksjer.
        assert_eq!(kjopbart_antall("EQNR.OL", 5_000.0, 352.0), 14.0);
        // For lite til én aksje.
        assert_eq!(kjopbart_antall("EQNR.OL", 100.0, 352.0), 0.0);
        // Krypto handles i brøkdeler.
        let btc = kjopbart_antall("BTC-USD", 5_000.0, 700_000.0);
        assert!((btc - 5_000.0 / 700_000.0).abs() < 1e-12);
        // Ugyldig kurs skal aldri gi antall.
        assert_eq!(kjopbart_antall("EQNR.OL", 5_000.0, 0.0), 0.0);
    }

    #[test]
    fn antall_formateres_uten_unodige_desimaler() {
        assert_eq!(fmt_qty(14.0), "14");
        assert_eq!(fmt_qty(0.5), "0.5");
        assert_eq!(fmt_qty(0.007143), "0.007143");
    }
}
