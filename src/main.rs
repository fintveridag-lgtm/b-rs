//! Konsoll-varianten: viser feilmeldinger i terminalen, støtter --tui,
//! og krever eksplisitt JA-bekreftelse for live-handel.

use anyhow::Result;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Standard er grafisk vindu; --tui gir terminalversjonen.
    let use_tui = args.iter().any(|a| a == "--tui");
    let config_arg = args.iter().find(|a| !a.starts_with("--")).cloned();

    let cfg = b_rs::app::load_config(config_arg)?;

    // Sikkerhetsbarriere: live-handel må bekreftes eksplisitt i terminalen.
    if cfg.is_live() {
        eprintln!("⚠️  MODUS ER 'live' — ordrer sendes til {} med EKTE PENGER.", cfg.broker);
        eprintln!("Skriv JA (store bokstaver) for å fortsette:");
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        anyhow::ensure!(answer.trim() == "JA", "avbrutt — endre mode til \"paper\" for simulering");
    }

    b_rs::app::start(cfg, use_tui)
}
