mod broker;
mod config;
mod engine;
mod marketdata;
mod nordnet;
mod risk;
mod state;
mod store;
mod strategy;
mod types;

use anyhow::{Context, Result};
use broker::Broker;
use state::{Flags, UiState};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[tokio::main]
async fn main() -> Result<()> {
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config.toml"));

    let cfg = if config_path.exists() {
        config::Config::load(&config_path)?
    } else {
        let fallback = PathBuf::from("config.example.toml");
        anyhow::ensure!(
            fallback.exists(),
            "fant verken {} eller config.example.toml — kopier eksempelfilen til config.toml",
            config_path.display()
        );
        eprintln!("Fant ikke {} — bruker config.example.toml (papirhandel).", config_path.display());
        config::Config::load(&fallback)?
    };

    // Sikkerhetsbarriere: live-handel må bekreftes eksplisitt i terminalen.
    if cfg.is_live() {
        eprintln!("⚠️  MODUS ER 'live' — ordrer sendes til {} med EKTE PENGER.", cfg.broker);
        eprintln!("Skriv JA (store bokstaver) for å fortsette:");
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        anyhow::ensure!(answer.trim() == "JA", "avbrutt — endre mode til \"paper\" for simulering");
    }

    let broker: Arc<dyn Broker> = match (cfg.is_live(), cfg.broker.as_str()) {
        (false, _) | (true, "paper") => Arc::new(broker::paper::PaperBroker::new(cfg.starting_cash)),
        (true, "ibkr") => {
            let ibkr_cfg = cfg.ibkr.as_ref().context("[ibkr]-seksjon mangler i konfig")?;
            let b = broker::ibkr::IbkrBroker::new(ibkr_cfg)?;
            b.check_session().await?;
            Arc::new(b)
        }
        (true, other) => anyhow::bail!("ukjent megler: {other}"),
    };

    let effective_mode = if cfg.is_live() && cfg.broker != "paper" { "live" } else { "paper" };
    let strategy = strategy::build(&cfg.strategy)?;
    let market = marketdata::Yahoo::new()?;
    let store = Arc::new(store::Store::open(&cfg.db_path)?);
    let state = Arc::new(Mutex::new(UiState::new(
        effective_mode,
        broker.name(),
        cfg.nordnet.enabled,
    )));
    let flags = Arc::new(Flags::default());

    // Engine i bakgrunnen.
    let engine = engine::Engine::new(
        cfg.clone(),
        broker,
        market,
        strategy,
        store,
        state.clone(),
        flags.clone(),
    );
    let engine_handle = tokio::spawn(engine.run());

    // Nordnet-lesemodus i egen oppgave.
    let nordnet_handle = if cfg.nordnet.enabled {
        Some(tokio::spawn(engine::nordnet_task(
            cfg.clone(),
            state.clone(),
            flags.clone(),
        )))
    } else {
        None
    };

    // TUI-et blokkerer til brukeren avslutter.
    let ui_state = state.clone();
    let ui_flags = flags.clone();
    let ui_result = tokio::task::spawn_blocking(move || ui::run(ui_state, ui_flags)).await?;

    flags.quit.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = engine_handle.await;
    if let Some(h) = nordnet_handle {
        h.abort();
    }
    ui_result
}

mod ui;
