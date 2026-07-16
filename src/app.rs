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
    start_with_path(cfg, use_tui, None)
}

/// Som `start`, men husker hvilken konfigfil som ble brukt — så
/// Innstillinger-fanen i GUI-et kan lagre tilbake til riktig fil.
pub fn start_with_path(cfg: Config, use_tui: bool, config_path: Option<std::path::PathBuf>) -> Result<()> {
    // Engine og meglere er async — de kjører på en tokio-runtime i bakgrunnen,
    // mens GUI/TUI eier hovedtråden.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    // Sikkerhetskopi før databasen åpnes — én per dag, 14 beholdes.
    let backup_msg = backup_database(&cfg.db_path);

    let store = Arc::new(store::Store::open(&cfg.db_path)?);

    // Tilstanden lages FØR megleren: multi-megleren leser valutakursene
    // derfra for å regne kryptokontoen om til kroner.
    let effective_mode = if cfg.is_live() && cfg.broker != "paper" { "live" } else { "paper" };
    let broker_label = if !cfg.is_live() || cfg.broker == "paper" {
        "paper".to_string()
    } else if cfg.broker == "multi" {
        format!("{} (aksjer) + {} (krypto)", cfg.multi.stocks, cfg.multi.crypto)
    } else {
        cfg.broker.clone()
    };
    let market = Arc::new(marketdata::Yahoo::new()?);
    let state = Arc::new(Mutex::new(UiState::new(
        effective_mode,
        &broker_label,
        cfg.nordnet.enabled,
    )));

    // Saldo-oppsummeringer fra meglerne ved oppstart — logges når UI-et finnes.
    let mut broker_summaries: Vec<String> = Vec::new();

    // Én undermegler etter navn — brukes både alene og inni multi.
    let build_sub = |name: &str, summaries: &mut Vec<String>| -> Result<Arc<dyn Broker>> {
        match name {
            "paper" => Ok(Arc::new(broker::paper::PaperBroker::new(
                cfg.starting_cash,
                Some(store.clone()),
                cfg.paper_reset,
            ))),
            "ibkr" => {
                let ibkr_cfg = cfg.ibkr.as_ref().context("[ibkr]-seksjon mangler i konfig")?;
                let b = broker::ibkr::IbkrBroker::new(ibkr_cfg)?;
                rt.block_on(b.check_session())?;
                Ok(Arc::new(b))
            }
            "revolutx" => {
                let rx_cfg =
                    cfg.revolutx.as_ref().context("[revolutx]-seksjon mangler i konfig")?;
                let b = broker::revolutx::RevolutXBroker::new(rx_cfg)?;
                summaries.push(rt.block_on(b.check_session())?);
                Ok(Arc::new(b))
            }
            other => anyhow::bail!("ukjent megler: {other}"),
        }
    };

    let broker: Arc<dyn Broker> = match (cfg.is_live(), cfg.broker.as_str()) {
        (false, _) | (true, "paper") => Arc::new(broker::paper::PaperBroker::new(
            cfg.starting_cash,
            Some(store.clone()),
            cfg.paper_reset,
        )),
        (true, "multi") => {
            let stocks = build_sub(&cfg.multi.stocks, &mut broker_summaries)?;
            let crypto = build_sub(&cfg.multi.crypto, &mut broker_summaries)?;
            let crypto_currency = if cfg.multi.crypto == "revolutx" {
                cfg.revolutx
                    .as_ref()
                    .map(|r| r.quote_currency.clone())
                    .unwrap_or_default()
            } else {
                String::new() // papir fører kontoen i kroner
            };
            Arc::new(broker::multi::MultiBroker::new(stocks, crypto, crypto_currency, state.clone()))
        }
        (true, name) => build_sub(name, &mut broker_summaries)?,
    };
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
        st.limit_orders = store.load_limit_orders().unwrap_or_default();
        st.savings_plans = store.load_savings_plans().unwrap_or_default();
        st.equity_daily = store.load_equity_history().unwrap_or_default();
        st.morgan_archive = store.list_morgan_reports().unwrap_or_default();
        let (n_limits, n_plans) = (st.limit_orders.len(), st.savings_plans.len());
        if n_limits > 0 {
            st.log(format!("{n_limits} ventende limit-ordrer lastet fra forrige økt."));
        }
        if n_plans > 0 {
            st.log(format!("{n_plans} spareavtaler aktive."));
        }
        if let Some(msg) = backup_msg {
            st.log(msg);
        }
        for msg in broker_summaries {
            st.log(msg);
        }
        // Kontanter/egenkapital vises i meglerens valuta — USD hos Revolut X.
        // Multi aggregerer alt til kroner (kontoene vises hver for seg).
        if cfg.is_live() && cfg.broker == "revolutx" {
            if let Some(rx) = &cfg.revolutx {
                st.cash_currency = rx.quote_currency.clone();
            }
        }
        // Fortell brukeren om papirporteføljen ble gjenopprettet.
        let uses_paper = !cfg.is_live()
            || cfg.broker == "paper"
            || (cfg.broker == "multi"
                && (cfg.multi.stocks == "paper" || cfg.multi.crypto == "paper"));
        if uses_paper {
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
        market.clone(),
        store.clone(),
        state.clone(),
        flags.clone(),
        notifier,
    )?;
    rt.spawn(engine.run());

    // Sjekk om en nyere versjon er publisert på GitHub (stille ved feil).
    spawn_update_check(&rt, state.clone());

    if cfg.nordnet.enabled {
        rt.spawn(engine::nordnet_task(cfg.clone(), state.clone(), flags.clone()));
    }

    // 🤖 Morgan Autopilot: automatisk handel i ett symbol, lite budsjett.
    if cfg.morgan.autopilot.enabled {
        {
            // Symbolet må være i watchlisten så motoren henter kurs for det.
            let mut st = state.lock().unwrap();
            let symbol = cfg.morgan.autopilot.symbol.clone();
            if !st.watchlist.iter().any(|s| s == &symbol) {
                st.watchlist.push(symbol);
            }
        }
        rt.spawn(crate::morgan::autopilot_task(cfg.clone(), state.clone(), flags.clone()));
    }

    // Referanseindeksen til «slår jeg børsen?»-grafen (stille ved feil).
    if !cfg.benchmark.is_empty() {
        let market_bm = market.clone();
        let state_bm = state.clone();
        let symbol = cfg.benchmark.clone();
        rt.spawn(async move {
            match market_bm.history_daily(&symbol, "2y").await {
                Ok(bars) if !bars.is_empty() => {
                    let pts: Vec<(f64, f64)> = bars.iter().map(|b| (b.ts, b.close)).collect();
                    let mut st = state_bm.lock().unwrap();
                    st.benchmark = pts;
                    st.benchmark_name = if symbol == "^OSEAX" {
                        "Oslo Børs".to_string()
                    } else {
                        symbol.clone()
                    };
                }
                _ => {
                    state_bm
                        .lock()
                        .unwrap()
                        .log(format!("Fikk ikke hentet referanseindeksen {symbol}."));
                }
            }
        });
    }

    // Markedsoversikten (mest omsatte, daytrading, fond, ukesanalyse).
    rt.spawn(crate::market::task(state.clone(), flags.clone()));

    // Selskapskalenderen (rapporter og utbyttedatoer).
    rt.spawn(crate::calendar::task(state.clone(), flags.clone()));

    // UI-et blokkerer hovedtråden til brukeren avslutter.
    let result = if use_tui {
        ui::run(state, flags.clone())
    } else {
        gui::run(gui::GuiDeps {
            state,
            flags: flags.clone(),
            store,
            market,
            rt: rt.handle().clone(),
            cfg: cfg.clone(),
            config_path,
        })
    };

    flags.quit.store(true, std::sync::atomic::Ordering::Relaxed);
    rt.shutdown_timeout(std::time::Duration::from_secs(5));
    result
}

/// Spør GitHub om siste utgivelse og flagg i UI-et hvis den er nyere.
/// Feiler stille (privat repo uten token, ingen nett, ingen releases).
fn spawn_update_check(rt: &tokio::runtime::Runtime, state: crate::state::SharedState) {
    rt.spawn(async move {
        let Ok(client) = reqwest::Client::builder()
            .user_agent("b-rs")
            .timeout(std::time::Duration::from_secs(10))
            .build()
        else {
            return;
        };
        let mut req = client.get("https://api.github.com/repos/fintveridag-lgtm/b-rs/releases/latest");
        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            req = req.bearer_auth(token);
        }
        let Ok(resp) = req.send().await else { return };
        if !resp.status().is_success() {
            return;
        }
        let Ok(v) = resp.json::<serde_json::Value>().await else { return };
        let Some(tag) = v.get("tag_name").and_then(|t| t.as_str()) else { return };
        let url = v
            .get("html_url")
            .and_then(|u| u.as_str())
            .unwrap_or("https://github.com/fintveridag-lgtm/b-rs/releases")
            .to_string();
        if is_newer_version(tag, env!("CARGO_PKG_VERSION")) {
            let mut st = state.lock().unwrap();
            st.log(format!("📥 Ny versjon tilgjengelig: {tag} (du har v{}).", env!("CARGO_PKG_VERSION")));
            st.update_available = Some((tag.to_string(), url));
        }
    });
}

/// Er "v1.2.3" nyere enn "1.0.0"? Numerisk sammenligning per ledd.
fn is_newer_version(tag: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim_start_matches('v')
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect()
    };
    let (a, b) = (parse(tag), parse(current));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a > b
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
pub fn load_config(config_arg: Option<String>) -> Result<(Config, Option<PathBuf>)> {
    if let Some(arg) = config_arg {
        let path = PathBuf::from(arg);
        return Ok((config::Config::load(&path)?, Some(path)));
    }

    let local = PathBuf::from("config.toml");
    if local.exists() {
        return Ok((config::Config::load(&local)?, Some(local)));
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in ["config.toml", "config.example.toml"] {
                let candidate = dir.join(name);
                if candidate.exists() {
                    eprintln!("Bruker konfig fra {}", candidate.display());
                    return Ok((config::Config::load(&candidate)?, Some(candidate)));
                }
            }
        }
    }

    eprintln!("Fant ingen config.toml — bruker innebygd standardkonfig (papirhandel).");
    Ok((config::Config::parse(include_str!("../config.example.toml"))?, None))
}

#[cfg(test)]
mod tests {
    use super::is_newer_version;

    #[test]
    fn version_comparison() {
        assert!(is_newer_version("v1.1.0", "1.0.0"));
        assert!(is_newer_version("v2.0.0", "1.9.9"));
        assert!(!is_newer_version("v1.0.0", "1.0.0"));
        assert!(!is_newer_version("v0.9.0", "1.0.0"));
        assert!(!is_newer_version("tull", "1.0.0"));
    }
}
