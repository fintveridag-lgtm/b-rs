use crate::broker::{self, Broker};
use crate::config::{self, Config};
use crate::engine;
use crate::gui;
use crate::marketdata;
use crate::notify::Notifier;
use crate::state::{Flags, UiState};
use crate::store;
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

    // Sikkerhetskopi før databasen åpnes — én per dag, 14 beholdes.
    let backup_msg = backup_database(&cfg.db_path);

    let store = Arc::new(store::Store::open(&cfg.db_path)?);

    let broker: Arc<dyn Broker> = match (cfg.is_live(), cfg.broker.as_str()) {
        (false, _) | (true, "paper") => Arc::new(broker::paper::PaperBroker::new(
            cfg.starting_cash,
            Some(store.clone()),
            cfg.paper_reset,
        )),
        (true, "ibkr") => {
            let ibkr_cfg = cfg.ibkr.as_ref().context("[ibkr]-seksjon mangler i konfig")?;
            let b = broker::ibkr::IbkrBroker::new(ibkr_cfg)?;
            rt.block_on(b.check_session())?;
            Arc::new(b)
        }
        (true, "revolutx") => {
            let rx_cfg = cfg.revolutx.as_ref().context("[revolutx]-seksjon mangler i konfig")?;
            let b = broker::revolutx::RevolutXBroker::new(rx_cfg)?;
            rt.block_on(b.check_session())?;
            Arc::new(b)
        }
        (true, other) => anyhow::bail!("ukjent megler: {other}"),
    };

    let effective_mode = if cfg.is_live() && cfg.broker != "paper" { "live" } else { "paper" };
    let market = marketdata::Yahoo::new()?;
    let state = Arc::new(Mutex::new(UiState::new(
        effective_mode,
        broker.name(),
        cfg.nordnet.enabled,
    )));
    {
        let mut st = state.lock().unwrap();
        st.log_path = prepare_log_file(&cfg.db_path);
        st.poll_secs = cfg.poll_secs;
        st.sma_windows = (cfg.strategy.fast, cfg.strategy.slow);
        st.strategy_name = cfg.strategy.name.clone();
        st.strategy_cfg = cfg.strategy.clone();
        st.backtest_cfg = cfg.backtest.clone();
        st.watchlist = cfg.watchlist.clone();
        st.start_cash = cfg.starting_cash;
        // Transaksjonshistorikk og alarmer fra tidligere økter.
        st.transactions = store.recent_orders(500).unwrap_or_default();
        st.alarms = store.load_alarms().unwrap_or_default();
        st.symbol_strategy = store.load_symbol_strategies().unwrap_or_default();
        if let Some(msg) = backup_msg {
            st.log(msg);
        }
        // Fortell brukeren om papirporteføljen ble gjenopprettet.
        if !cfg.is_live() || cfg.broker == "paper" {
            if cfg.paper_reset {
                st.log("Papirporteføljen er nullstilt (paper_reset = true).");
            } else if let Ok(Some((cash, positions))) = store.load_paper_state() {
                if !positions.is_empty() || (cash - cfg.starting_cash).abs() > 0.005 {
                    st.log(format!(
                        "Papirportefølje gjenopprettet fra forrige økt: {cash:.0} kr kontanter, {} posisjoner.",
                        positions.len()
                    ));
                }
            }
        }
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
        store.clone(),
        state.clone(),
        flags.clone(),
        notifier,
    )?;
    rt.spawn(engine.run());

    if cfg.nordnet.enabled {
        rt.spawn(engine::nordnet_task(cfg.clone(), state.clone(), flags.clone()));
    }

    // Markedsoversikten (mest omsatte, daytrading, fond, ukesanalyse).
    rt.spawn(crate::market::task(state.clone(), flags.clone()));

    // Selskapskalenderen (rapporter og utbyttedatoer).
    rt.spawn(crate::calendar::task(state.clone(), flags.clone()));

    // UI-et blokkerer hovedtråden til brukeren avslutter.
    let result = if use_tui {
        ui::run(state, flags.clone())
    } else {
        gui::run(state, flags.clone(), store)
    };

    flags.quit.store(true, std::sync::atomic::Ordering::Relaxed);
    rt.shutdown_timeout(std::time::Duration::from_secs(5));
    result
}

/// Loggfil ved siden av databasen; roteres til .old når den passerer 5 MB.
fn prepare_log_file(db_path: &str) -> Option<String> {
    let dir = std::path::Path::new(db_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf();
    let path = dir.join("b-rs.log");
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 5_000_000 {
            let _ = std::fs::rename(&path, dir.join("b-rs.log.old"));
        }
    }
    Some(path.display().to_string())
}

/// Daglig sikkerhetskopi av databasen til backups/-mappen ved siden av den.
/// Databasen er appens hukommelse: portefølje, historikk og skattegrunnlag.
fn backup_database(db_path: &str) -> Option<String> {
    let src = std::path::Path::new(db_path);
    if !src.exists() {
        return None;
    }
    let dir = src.parent().unwrap_or(std::path::Path::new(".")).join("backups");
    std::fs::create_dir_all(&dir).ok()?;
    let stem = src.file_stem()?.to_string_lossy().to_string();
    let dest = dir.join(format!("{stem}-{}.db", chrono::Local::now().format("%Y-%m-%d")));
    if dest.exists() {
        return None; // dagens kopi finnes allerede
    }
    std::fs::copy(src, &dest).ok()?;

    // Behold bare de 14 nyeste kopiene.
    let mut backups: Vec<_> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "db"))
        .collect();
    backups.sort();
    while backups.len() > 14 {
        let oldest = backups.remove(0);
        let _ = std::fs::remove_file(oldest);
    }

    Some(format!("Sikkerhetskopi av databasen: {}", dest.display()))
}

/// Finn konfigurasjonen, i prioritert rekkefølge:
///   1. sti gitt som kommandolinjeargument
///   2. config.toml i arbeidsmappen
///   3. config.toml / config.example.toml ved siden av programfilen
///   4. innebygd standardkonfig (papirhandel)
///
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
