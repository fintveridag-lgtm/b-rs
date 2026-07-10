use crate::broker::{self, Broker};
use crate::config::{self, Config};
use crate::engine;
use crate::gui;
use crate::marketdata;
use crate::notify::Notifier;
use crate::state::{Flags, UiState};
use crate::store;
use crate::strategy;
use crate::ui;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Felles oppstart for begge programfilene: bygg megler, engine og UI,
/// og kjør til brukeren avslutter.
pub fn start(cfg: Config, use_tui: bool) -> Result<()> {
    // Engine og meglere er async — de kjører på en tokio-runtime i bakgrunnen,
    // mens GUI/TUI eier hovedtråden.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let broker: Arc<dyn Broker> = match (cfg.is_live(), cfg.broker.as_str()) {
        (false, _) | (true, "paper") => Arc::new(broker::paper::PaperBroker::new(cfg.starting_cash)),
        (true, "ibkr") => {
            let ibkr_cfg = cfg.ibkr.as_ref().context("[ibkr]-seksjon mangler i konfig")?;
            let b = broker::ibkr::IbkrBroker::new(ibkr_cfg)?;
            rt.block_on(b.check_session())?;
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
    {
        let mut st = state.lock().unwrap();
        st.sma_windows = (cfg.strategy.fast, cfg.strategy.slow);
        st.strategy_name = cfg.strategy.name.clone();
        st.strategy_cfg = cfg.strategy.clone();
        st.watchlist = cfg.watchlist.clone();
    }
    let flags = Arc::new(Flags::default());

    let notifier = if cfg.notify.enabled {
        Some(Arc::new(Notifier::new(&cfg.notify)?))
    } else {
        None
    };

    let engine = engine::Engine::new(
        cfg.clone(),
        broker,
        market,
        strategy,
        store,
        state.clone(),
        flags.clone(),
        notifier,
    );
    rt.spawn(engine.run());

    if cfg.nordnet.enabled {
        rt.spawn(engine::nordnet_task(cfg.clone(), state.clone(), flags.clone()));
    }

    // Markedsoversikten (mest omsatte, daytrading, fond, ukesanalyse).
    rt.spawn(crate::market::task(state.clone(), flags.clone()));

    // UI-et blokkerer hovedtråden til brukeren avslutter.
    let result = if use_tui {
        ui::run(state, flags.clone())
    } else {
        gui::run(state, flags.clone())
    };

    flags.quit.store(true, std::sync::atomic::Ordering::Relaxed);
    rt.shutdown_timeout(std::time::Duration::from_secs(5));
    result
}

/// Finn konfigurasjonen, i prioritert rekkefølge:
///   1. sti gitt som kommandolinjeargument
///   2. config.toml i arbeidsmappen
///   3. config.toml / config.example.toml ved siden av programfilen
///   4. innebygd standardkonfig (papirhandel)
/// Punkt 3 og 4 gjør at programfilen kan dobbeltklikkes hvor som helst.
pub fn load_config(config_arg: Option<String>) -> Result<Config> {
    if let Some(arg) = config_arg {
        return config::Config::load(&PathBuf::from(arg));
    }

    let local = PathBuf::from("config.toml");
    if local.exists() {
        return config::Config::load(&local);
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in ["config.toml", "config.example.toml"] {
                let candidate = dir.join(name);
                if candidate.exists() {
                    eprintln!("Bruker konfig fra {}", candidate.display());
                    return config::Config::load(&candidate);
                }
            }
        }
    }

    eprintln!("Fant ingen config.toml — bruker innebygd standardkonfig (papirhandel).");
    config::Config::parse(include_str!("../config.example.toml"))
}
