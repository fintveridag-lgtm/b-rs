//! Ren vindusapp UTEN konsollvindu bak (kun Windows-effekt; på andre
//! plattformer oppfører den seg som vanlig).
//!
//! Uten konsoll kan vi ikke stille JA-spørsmålet for live-handel, så denne
//! varianten krever i tillegg `live_ok = true` i config.toml — ellers
//! tvinges papirmodus.

#![cfg_attr(windows, windows_subsystem = "windows")]

use anyhow::Result;

fn main() -> Result<()> {
    let config_arg = std::env::args().skip(1).find(|a| !a.starts_with("--"));
    let (mut cfg, config_path) = b_rs::app::load_config(config_arg)?;

    if cfg.is_live() && !cfg.live_ok {
        // Ingen konsoll å spørre i — fall trygt tilbake til simulering.
        cfg.mode = "paper".into();
    }

    b_rs::app::start_with_path(cfg, false, config_path)
}
