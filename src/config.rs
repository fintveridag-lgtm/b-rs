use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct Config {
    /// "paper" (simulering) eller "live" (ekte ordrer via valgt megler).
    #[serde(default = "default_mode")]
    pub mode: String,
    /// "paper" eller "ibkr".
    #[serde(default = "default_broker")]
    pub broker: String,
    /// Sekunder mellom hver kursoppdatering/strategitikk.
    #[serde(default = "default_poll_secs")]
    pub poll_secs: u64,
    /// Yahoo Finance-tickere, f.eks. "EQNR.OL".
    pub watchlist: Vec<String>,
    #[serde(default = "default_starting_cash")]
    pub starting_cash: f64,
    /// Sett true (for én kjøring) for å nullstille papirporteføljen —
    /// ellers gjenopprettes den fra databasen ved oppstart.
    #[serde(default)]
    pub paper_reset: bool,
    /// Kontovaluta — alle posisjoner og all risiko regnes om hit.
    #[serde(default = "default_base_currency")]
    pub base_currency: String,
    /// Strategien handler bare i børsens åpningstid (krypto unntas).
    #[serde(default = "default_true")]
    pub market_hours_only: bool,
    #[serde(default = "default_db_path")]
    pub db_path: String,
    /// Kreves av den konsoll-løse GUI-varianten (b-rs-gui) for live-handel,
    /// siden den ikke kan stille JA-spørsmålet i terminalen.
    #[serde(default)]
    pub live_ok: bool,
    #[serde(default)]
    pub strategy: StrategyCfg,
    #[serde(default)]
    pub risk: RiskCfg,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ibkr: Option<IbkrCfg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revolutx: Option<RevolutXCfg>,
    #[serde(default)]
    pub nordnet: NordnetCfg,
    #[serde(default)]
    pub notify: NotifyCfg,
    #[serde(default)]
    pub backtest: BacktestCfg,
    #[serde(default)]
    pub goal: GoalCfg,
    /// Referanseindeks til «slår jeg børsen?»-grafen (Yahoo-symbol).
    /// ^OSEAX = Oslo Børs All-share. Tom streng slår av sammenligningen.
    #[serde(default = "default_benchmark")]
    pub benchmark: String,
    #[serde(default)]
    pub morgan: MorganCfg,
    #[serde(default)]
    pub uno_x: UnoXCfg,
    /// Brukes når broker = "multi": to meglere samtidig, rutet på symboltype.
    #[serde(default)]
    pub multi: MultiCfg,
}

/// Multi-megler: krypto (BTC-USD o.l.) går til én megler, aksjer/fond til en
/// annen — samtidig. Eksempel: ekte krypto hos Revolut X mens aksjer
/// simuleres i papirmodus (eller handles ekte hos IBKR).
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct MultiCfg {
    /// "paper" eller "revolutx".
    #[serde(default = "default_paper")]
    pub crypto: String,
    /// "paper" eller "ibkr".
    #[serde(default = "default_paper")]
    pub stocks: String,
}

impl Default for MultiCfg {
    fn default() -> Self {
        Self { crypto: default_paper(), stocks: default_paper() }
    }
}

fn default_paper() -> String {
    "paper".to_string()
}

/// Hjernen bak Morgan: "claude" (Anthropic API, best kvalitet, krever
/// API-nøkkel) eller "ollama" (lokal modell på din PC — gratis, privat,
/// offline, men merkbart svakere analyser).
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct MorganCfg {
    #[serde(default = "default_morgan_provider")]
    pub provider: String,
    /// Hvor Ollama lytter (standard etter installasjon fra ollama.com).
    #[serde(default = "default_ollama_url")]
    pub ollama_url: String,
    /// Modellen Ollama skal bruke — må være hentet med `ollama pull <navn>`.
    #[serde(default = "default_ollama_model")]
    pub ollama_model: String,
    #[serde(default)]
    pub autopilot: AutopilotCfg,
}

/// 🔬 Uno-X: teamet på 10 agenter som daglig jakter kjøpskandidater, og
/// Morgan/Stanley-rådslaget hver søndag. Eksperimentelt, koster AI-kall.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct UnoXCfg {
    #[serde(default)]
    pub enabled: bool,
    /// Klokketime (lokal, 0–23) da den daglige analysen kjøres. Standard 17
    /// (etter børsslutt). Søndagsrådslaget kjøres samme time.
    #[serde(default = "default_uno_x_hour")]
    pub hour: u32,
    /// Hjerne: "ollama" (standard, gratis), "claude", eller tom = arv fra
    /// [morgan] provider.
    #[serde(default = "default_uno_x_provider")]
    pub provider: String,
    /// Grundig modus: antall uavhengige analyserunder som slås sammen til
    /// konsensus (1 = av). 2–4 gir mer robuste funn, men bruker mer tid/kraft.
    #[serde(default = "default_uno_x_passes")]
    pub passes: u32,
}

impl Default for UnoXCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            hour: default_uno_x_hour(),
            provider: default_uno_x_provider(),
            passes: default_uno_x_passes(),
        }
    }
}

fn default_uno_x_hour() -> u32 {
    17
}
fn default_uno_x_provider() -> String {
    "ollama".to_string()
}
fn default_uno_x_passes() -> u32 {
    1
}

impl Default for MorganCfg {
    fn default() -> Self {
        Self {
            provider: default_morgan_provider(),
            ollama_url: default_ollama_url(),
            ollama_model: default_ollama_model(),
            autopilot: AutopilotCfg::default(),
        }
    }
}

/// 🤖 Morgan Autopilot/Daytrader: la AI-en handle ett symbol automatisk
/// innenfor et lite, hardt budsjett. Eksperimentelt — kjør i papirmodus.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct AutopilotCfg {
    #[serde(default)]
    pub enabled: bool,
    /// Symbolet Morgan får handle (bakoverkompatibelt enkelt-symbol).
    #[serde(default = "default_autopilot_symbol")]
    pub symbol: String,
    /// Flere symboler samtidig (papir-eksperiment: Morgan vs. botene).
    /// Tom liste = bruk `symbol` alene. Aksjer handles kun i åpningstid.
    /// Budsjett, maks handler/dag og dagstap-brems deles på tvers.
    #[serde(default)]
    pub symbols: Vec<String>,
    /// Hard grense: samlet beholdning i symbolet får aldri overstige dette.
    #[serde(default = "default_autopilot_budget")]
    pub budget_kr: f64,
    /// Minutter mellom hver vurdering (minimum 5 håndheves).
    #[serde(default = "default_autopilot_interval")]
    pub interval_min: u64,
    /// Maks antall handler per dag — resten blir AVVENT.
    #[serde(default = "default_autopilot_max_trades")]
    pub max_trades_per_day: u32,
    /// Hjernen for autopiloten: "" = arv fra [morgan] provider,
    /// "claude", "ollama", eller "duo" (Ollama speider hver puls og
    /// tilkaller Claude kun når noe ser interessant ut — billig OG smart).
    #[serde(default)]
    pub provider: String,
    /// Dagstap-brems: taper autopiloten mer enn dette i dag (ca., kr),
    /// settes den på benken til i morgen. 0 = av.
    #[serde(default = "default_autopilot_day_loss")]
    pub max_day_loss_kr: f64,
    /// Kjøletid etter en tapshandel: ingen nye kjøp før det har gått
    /// så mange minutter — hindrer «revansje-trading». 0 = av.
    #[serde(default = "default_autopilot_cooldown")]
    pub cooldown_min: u64,
}

impl Default for AutopilotCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            symbol: default_autopilot_symbol(),
            symbols: Vec::new(),
            budget_kr: default_autopilot_budget(),
            interval_min: default_autopilot_interval(),
            max_trades_per_day: default_autopilot_max_trades(),
            provider: String::new(),
            max_day_loss_kr: default_autopilot_day_loss(),
            cooldown_min: default_autopilot_cooldown(),
        }
    }
}

impl AutopilotCfg {
    /// Symbolene daytraderen faktisk handler: listen hvis satt, ellers
    /// enkelt-symbolet. Tomme/dupliserte oppføringer lukes bort.
    pub fn active_symbols(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let kilde: Vec<String> = if self.symbols.is_empty() {
            vec![self.symbol.clone()]
        } else {
            self.symbols.clone()
        };
        for s in kilde {
            let s = s.trim().to_string();
            if !s.is_empty() && !out.contains(&s) {
                out.push(s);
            }
        }
        out
    }
}

fn default_autopilot_day_loss() -> f64 {
    300.0
}
fn default_autopilot_cooldown() -> u64 {
    30
}

fn default_autopilot_symbol() -> String {
    "BTC-USD".to_string()
}
fn default_autopilot_budget() -> f64 {
    1_000.0
}
fn default_autopilot_interval() -> u64 {
    60
}
fn default_autopilot_max_trades() -> u32 {
    4
}

fn default_morgan_provider() -> String {
    "claude".to_string()
}
fn default_ollama_url() -> String {
    "http://localhost:11434".to_string()
}
fn default_ollama_model() -> String {
    "llama3.1:8b".to_string()
}

fn default_benchmark() -> String {
    "^OSEAX".to_string()
}

/// Sparemål: «jeg vil ha X kr innen år Y» — vises som fremdriftslinje
/// i Portefølje-fanen. amount = 0 betyr at målet er slått av.
#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct GoalCfg {
    #[serde(default)]
    pub amount: f64,
    #[serde(default)]
    pub year: i32,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct BacktestCfg {
    /// Kurtasje i prosent av handelsverdi, per handel.
    #[serde(default = "default_commission_pct")]
    pub commission_pct: f64,
    /// Glidning: forventet ekstra kostnad fordi du sjelden får siste kurs.
    #[serde(default = "default_slippage_pct")]
    pub slippage_pct: f64,
}

impl Default for BacktestCfg {
    fn default() -> Self {
        Self {
            commission_pct: default_commission_pct(),
            slippage_pct: default_slippage_pct(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct NotifyCfg {
    /// Push-varsler til mobil når boten handler m.m.
    #[serde(default)]
    pub enabled: bool,
    /// "ntfy" (enklest) eller "telegram".
    #[serde(default = "default_notify_provider")]
    pub provider: String,
    #[serde(default = "default_ntfy_server")]
    pub ntfy_server: String,
    /// Hemmelig emnenavn du abonnerer på i ntfy-appen.
    #[serde(default)]
    pub ntfy_topic: String,
    /// Telegram chat-id; bot-token settes i miljøvariabelen TELEGRAM_BOT_TOKEN.
    #[serde(default)]
    pub telegram_chat_id: String,
    /// Fjernstyring fra mobilen (kun ntfy): send «STOPP» til styre-emnet for
    /// å utløse kill switch. Av som standard — slås bevisst på.
    #[serde(default)]
    pub remote_control: bool,
    /// Eget, hemmelig ntfy-emne for kommandoer. Tomt = <ntfy_topic>-styr.
    /// HOLD DETTE HEMMELIG — den som kjenner emnet kan stoppe handelen din.
    #[serde(default)]
    pub control_topic: String,
    /// Diskret systemlyd ved utført handel og utløst alarm (kun Windows).
    #[serde(default = "default_true")]
    pub sound: bool,
    /// Dagsoppsummering ved børsslutt (ca. 16:30) — krever varsler på.
    #[serde(default)]
    pub daily_summary: bool,
    /// Varsle hvis noe du eier faller mer enn X % på én dag (0 = av).
    #[serde(default)]
    pub day_move_alarm_pct: f64,
}

impl Default for NotifyCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_notify_provider(),
            ntfy_server: default_ntfy_server(),
            ntfy_topic: String::new(),
            telegram_chat_id: String::new(),
            remote_control: false,
            control_topic: String::new(),
            sound: true,
            daily_summary: false,
            day_move_alarm_pct: 0.0,
        }
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct StrategyCfg {
    #[serde(default = "default_strategy_name")]
    pub name: String,
    /// Vindu for rask glidende snitt (antall tikk/dager).
    #[serde(default = "default_fast")]
    pub fast: usize,
    /// Vindu for treg glidende snitt.
    #[serde(default = "default_slow")]
    pub slow: usize,
    /// Kjøp for et fast BELØP (i kontovaluta) per ordre — gir jevn risiko
    /// per posisjon uansett aksjekurs. 0 = bruk order_qty i stedet.
    #[serde(default)]
    pub order_value: f64,
    /// Antall aksjer per kjøpsordre (brukes bare når order_value = 0).
    #[serde(default = "default_order_qty")]
    pub order_qty: f64,
    /// Tidsramme i minutter for strategisignalene: tikkene samles i jevne
    /// lys, og strategien ser bare sluttkursen per lys. 0 = hvert tikk
    /// (rå, rask). F.eks. 5 = klassisk intradag, 60 = timelys.
    /// Vinduene (fast/slow) teller da lys av denne lengden.
    #[serde(default)]
    pub timeframe_min: u64,
    /// RSI-strategien: periode og terskler.
    #[serde(default = "default_rsi_period")]
    pub rsi_period: usize,
    #[serde(default = "default_rsi_buy_below")]
    pub rsi_buy_below: f64,
    #[serde(default = "default_rsi_sell_above")]
    pub rsi_sell_above: f64,
    /// Momentum-strategien: vindu for brudd på høyeste/laveste.
    #[serde(default = "default_momentum_window")]
    pub momentum_window: usize,
}

impl Default for StrategyCfg {
    fn default() -> Self {
        Self {
            name: default_strategy_name(),
            order_value: 0.0,
            fast: default_fast(),
            slow: default_slow(),
            order_qty: default_order_qty(),
            timeframe_min: 0,
            rsi_period: default_rsi_period(),
            rsi_buy_below: default_rsi_buy_below(),
            rsi_sell_above: default_rsi_sell_above(),
            momentum_window: default_momentum_window(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct RiskCfg {
    /// Maks verdi (i kontovaluta) per enkeltordre.
    #[serde(default = "default_max_order_value")]
    pub max_order_value: f64,
    /// Maks total posisjonsverdi per symbol etter ordren.
    #[serde(default = "default_max_position_value")]
    pub max_position_value: f64,
    /// Maks antall ordrer per minutt.
    #[serde(default = "default_max_orders_per_min")]
    pub max_orders_per_min: u32,
    /// Maks tap (i kontovaluta) siden oppstart før all handel stoppes.
    #[serde(default = "default_max_daily_loss")]
    pub max_daily_loss: f64,
    /// Selg posisjonen automatisk ved −X % fra kjøpskurs (0 = av).
    #[serde(default = "default_stop_loss_pct")]
    pub stop_loss_pct: f64,
    /// Sikre gevinst automatisk ved +X % fra kjøpskurs (0 = av).
    #[serde(default)]
    pub take_profit_pct: f64,
    /// Trailing stop: selg hvis kursen faller X % fra høyeste nivå etter
    /// kjøp — beskytter gevinst som allerede er opparbeidet (0 = av).
    #[serde(default)]
    pub trailing_stop_pct: f64,
}

impl Default for RiskCfg {
    fn default() -> Self {
        Self {
            max_order_value: default_max_order_value(),
            max_position_value: default_max_position_value(),
            max_orders_per_min: default_max_orders_per_min(),
            max_daily_loss: default_max_daily_loss(),
            stop_loss_pct: default_stop_loss_pct(),
            take_profit_pct: 0.0,
            trailing_stop_pct: 0.0,
        }
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct IbkrCfg {
    /// Client Portal Gateway, typisk https://localhost:5000/v1/api
    #[serde(default = "default_ibkr_base")]
    pub base_url: String,
    /// IBKR-kontonummer, f.eks. "U1234567".
    pub account: String,
    /// Gatewayen bruker selvsignert sertifikat på localhost.
    #[serde(default = "default_true")]
    pub accept_invalid_certs: bool,
    /// Bruk limit-ordrer i stedet for markedsordrer — beskytter mot stygge
    /// fyllinger på tynne aksjer/forsinket data. På som standard (tryggest).
    #[serde(default = "default_true")]
    pub limit_orders: bool,
    /// Hvor langt forbi siste kurs limit-prisen settes (%), så ordren fortsatt
    /// fylles nær markedet men aldri til en vill pris. 0,3 = 0,3 %.
    #[serde(default = "default_limit_slippage")]
    pub limit_slippage_pct: f64,
    /// Hent sanntidskurs fra IBKR for aksjer (i stedet for ~15 min forsinket
    /// Yahoo). Krever markedsdata-abonnement hos IBKR — ellers fortsatt forsinket.
    #[serde(default = "default_true")]
    pub realtime_quotes: bool,
}

fn default_limit_slippage() -> f64 {
    0.3
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct RevolutXCfg {
    #[serde(default = "default_revolutx_base")]
    pub base_url: String,
    /// Sti til Ed25519-privatnøkkelen (PKCS#8 PEM) du registrerte hos
    /// Revolut X. API-nøkkelen leses fra miljøvariabelen REVOLUTX_API_KEY.
    pub private_key_path: String,
    /// Fiat-valutaen kontoen handler mot, f.eks. "USD" — brukes som
    /// kontantsaldo og som suffiks i symbolene ("BTC-USD").
    #[serde(default = "default_revolutx_quote")]
    pub quote_currency: String,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct NordnetCfg {
    /// Lesemodus: hent portefølje fra Nordnet (uoffisielt API).
    /// Brukernavn/passord leses fra NORDNET_USERNAME / NORDNET_PASSWORD.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_nordnet_base")]
    pub base_url: String,
    /// Sekunder mellom porteføljeoppdateringer.
    #[serde(default = "default_nordnet_poll")]
    pub poll_secs: u64,
}

impl Default for NordnetCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: default_nordnet_base(),
            poll_secs: default_nordnet_poll(),
        }
    }
}

fn default_mode() -> String { "paper".into() }
fn default_base_currency() -> String { "NOK".into() }
fn default_broker() -> String { "paper".into() }
fn default_poll_secs() -> u64 { 15 }
fn default_starting_cash() -> f64 { 100_000.0 }
fn default_db_path() -> String { "b-rs.db".into() }
fn default_strategy_name() -> String { "sma_cross".into() }
fn default_fast() -> usize { 5 }
fn default_slow() -> usize { 20 }
fn default_order_qty() -> f64 { 10.0 }
fn default_rsi_period() -> usize { 14 }
fn default_rsi_buy_below() -> f64 { 30.0 }
fn default_rsi_sell_above() -> f64 { 70.0 }
fn default_momentum_window() -> usize { 20 }
fn default_max_order_value() -> f64 { 10_000.0 }
fn default_max_position_value() -> f64 { 25_000.0 }
fn default_max_orders_per_min() -> u32 { 4 }
fn default_max_daily_loss() -> f64 { 5_000.0 }
fn default_stop_loss_pct() -> f64 { 8.0 }
fn default_commission_pct() -> f64 { 0.15 }
fn default_slippage_pct() -> f64 { 0.05 }
fn default_ibkr_base() -> String { "https://localhost:5000/v1/api".into() }
fn default_revolutx_base() -> String { "https://revx.revolut.com/api/1.0".into() }
fn default_revolutx_quote() -> String { "USD".into() }
fn default_true() -> bool { true }
fn default_nordnet_base() -> String { "https://www.nordnet.no/api/2".into() }
fn default_notify_provider() -> String { "ntfy".into() }
fn default_ntfy_server() -> String { "https://ntfy.sh".into() }
fn default_nordnet_poll() -> u64 { 300 }

impl Config {
    pub fn parse(raw: &str) -> Result<Self> {
        let cfg: Config = toml::from_str(raw).context("ugyldig konfig")?;
        anyhow::ensure!(!cfg.watchlist.is_empty(), "watchlist kan ikke være tom");
        anyhow::ensure!(
            cfg.strategy.fast < cfg.strategy.slow,
            "strategy.fast må være mindre enn strategy.slow"
        );
        if cfg.mode == "live" && cfg.broker == "ibkr" {
            anyhow::ensure!(cfg.ibkr.is_some(), "mode=live med broker=ibkr krever [ibkr]-seksjon");
        }
        if cfg.mode == "live" && cfg.broker == "revolutx" {
            anyhow::ensure!(
                cfg.revolutx.is_some(),
                "mode=live med broker=revolutx krever [revolutx]-seksjon"
            );
        }
        if cfg.broker == "multi" {
            anyhow::ensure!(
                !(cfg.multi.crypto == "paper" && cfg.multi.stocks == "paper"),
                "[multi] med både crypto=paper og stocks=paper er det samme som broker=\"paper\" — bruk det i stedet"
            );
            anyhow::ensure!(
                matches!(cfg.multi.crypto.as_str(), "paper" | "revolutx"),
                "[multi] crypto må være \"paper\" eller \"revolutx\""
            );
            anyhow::ensure!(
                matches!(cfg.multi.stocks.as_str(), "paper" | "ibkr"),
                "[multi] stocks må være \"paper\" eller \"ibkr\""
            );
            if cfg.mode == "live" && cfg.multi.crypto == "revolutx" {
                anyhow::ensure!(
                    cfg.revolutx.is_some(),
                    "[multi] crypto=revolutx i live-modus krever [revolutx]-seksjon"
                );
            }
            if cfg.mode == "live" && cfg.multi.stocks == "ibkr" {
                anyhow::ensure!(
                    cfg.ibkr.is_some(),
                    "[multi] stocks=ibkr i live-modus krever [ibkr]-seksjon"
                );
            }
        }
        Ok(cfg)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("kunne ikke lese konfigfil {}", path.display()))?;
        Self::parse(&raw).with_context(|| format!("feil i {}", path.display()))
    }

    pub fn is_live(&self) -> bool {
        self.mode == "live"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_symbols_falls_back_and_dedupes() {
        let mut ap = AutopilotCfg::default();
        // Tom liste → enkelt-symbolet.
        assert_eq!(ap.active_symbols(), vec![ap.symbol.clone()]);
        // Liste med rot: trimmes, tomme og duplikater lukes.
        ap.symbols = vec![" BTC-USD ".into(), "EQNR.OL".into(), "".into(), "BTC-USD".into()];
        assert_eq!(ap.active_symbols(), vec!["BTC-USD".to_string(), "EQNR.OL".to_string()]);
    }
}
