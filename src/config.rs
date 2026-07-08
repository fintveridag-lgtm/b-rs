use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
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
    #[serde(default = "default_db_path")]
    pub db_path: String,
    #[serde(default)]
    pub strategy: StrategyCfg,
    #[serde(default)]
    pub risk: RiskCfg,
    pub ibkr: Option<IbkrCfg>,
    #[serde(default)]
    pub nordnet: NordnetCfg,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StrategyCfg {
    #[serde(default = "default_strategy_name")]
    pub name: String,
    /// Vindu for rask glidende snitt (antall tikk/dager).
    #[serde(default = "default_fast")]
    pub fast: usize,
    /// Vindu for treg glidende snitt.
    #[serde(default = "default_slow")]
    pub slow: usize,
    /// Antall aksjer per kjøpsordre.
    #[serde(default = "default_order_qty")]
    pub order_qty: f64,
}

impl Default for StrategyCfg {
    fn default() -> Self {
        Self {
            name: default_strategy_name(),
            fast: default_fast(),
            slow: default_slow(),
            order_qty: default_order_qty(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
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
}

impl Default for RiskCfg {
    fn default() -> Self {
        Self {
            max_order_value: default_max_order_value(),
            max_position_value: default_max_position_value(),
            max_orders_per_min: default_max_orders_per_min(),
            max_daily_loss: default_max_daily_loss(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct IbkrCfg {
    /// Client Portal Gateway, typisk https://localhost:5000/v1/api
    #[serde(default = "default_ibkr_base")]
    pub base_url: String,
    /// IBKR-kontonummer, f.eks. "U1234567".
    pub account: String,
    /// Gatewayen bruker selvsignert sertifikat på localhost.
    #[serde(default = "default_true")]
    pub accept_invalid_certs: bool,
}

#[derive(Debug, Clone, Deserialize)]
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
fn default_broker() -> String { "paper".into() }
fn default_poll_secs() -> u64 { 15 }
fn default_starting_cash() -> f64 { 100_000.0 }
fn default_db_path() -> String { "b-rs.db".into() }
fn default_strategy_name() -> String { "sma_cross".into() }
fn default_fast() -> usize { 5 }
fn default_slow() -> usize { 20 }
fn default_order_qty() -> f64 { 10.0 }
fn default_max_order_value() -> f64 { 10_000.0 }
fn default_max_position_value() -> f64 { 25_000.0 }
fn default_max_orders_per_min() -> u32 { 4 }
fn default_max_daily_loss() -> f64 { 5_000.0 }
fn default_ibkr_base() -> String { "https://localhost:5000/v1/api".into() }
fn default_true() -> bool { true }
fn default_nordnet_base() -> String { "https://www.nordnet.no/api/2".into() }
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
