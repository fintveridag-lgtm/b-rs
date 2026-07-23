use crate::config::NotifyCfg;
use anyhow::{Context, Result};
use serde_json::json;

/// Push-varsler til mobil. To tilbydere:
///
/// **ntfy** (enklest, gratis, ingen konto):
///   1. Installer «ntfy»-appen (App Store / Google Play)
///   2. Abonner på et selvvalgt, hemmelig emne, f.eks. "bors-ola-73xk1"
///   3. Sett samme navn i `notify.ntfy_topic` i config.toml
///
/// Emnenavnet er eneste «passord» — velg noe ugjettelig.
///
/// **Telegram**:
///   1. Snakk med @BotFather i Telegram → /newbot → få bot-token
///   2. Sett token i miljøvariabelen TELEGRAM_BOT_TOKEN
///   3. Send en melding til boten din, finn chat-id via
///      https://api.telegram.org/bot<TOKEN>/getUpdates
///   4. Sett `notify.telegram_chat_id` i config.toml
pub struct Notifier {
    client: reqwest::Client,
    cfg: NotifyCfg,
    telegram_token: Option<String>,
}

impl Notifier {
    pub fn new(cfg: &NotifyCfg) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        let telegram_token = std::env::var("TELEGRAM_BOT_TOKEN").ok();
        match cfg.provider.as_str() {
            "ntfy" => anyhow::ensure!(
                !cfg.ntfy_topic.is_empty(),
                "notify.provider er \"ntfy\", men notify.ntfy_topic er tom"
            ),
            "telegram" => {
                anyhow::ensure!(
                    telegram_token.is_some(),
                    "notify.provider er \"telegram\", men miljøvariabelen TELEGRAM_BOT_TOKEN er ikke satt"
                );
                anyhow::ensure!(
                    !cfg.telegram_chat_id.is_empty(),
                    "notify.provider er \"telegram\", men notify.telegram_chat_id er tom"
                );
            }
            other => anyhow::bail!("ukjent varseltilbyder: {other} (bruk \"ntfy\" eller \"telegram\")"),
        }
        Ok(Self {
            client,
            cfg: cfg.clone(),
            telegram_token,
        })
    }

    pub async fn send(&self, message: &str) -> Result<()> {
        match self.cfg.provider.as_str() {
            "ntfy" => self.send_ntfy(message).await,
            "telegram" => self.send_telegram(message).await,
            _ => unreachable!("validert i new()"),
        }
    }

    async fn send_ntfy(&self, message: &str) -> Result<()> {
        let url = format!(
            "{}/{}",
            self.cfg.ntfy_server.trim_end_matches('/'),
            self.cfg.ntfy_topic
        );
        self.client
            .post(&url)
            // Headere må være ASCII — norsk tekst går i meldingskroppen.
            .header("X-Title", "b-rs")
            .header("X-Tags", "chart_with_upwards_trend")
            .body(message.to_string())
            .send()
            .await
            .context("fikk ikke kontakt med ntfy")?
            .error_for_status()
            .context("ntfy avviste varselet")?;
        Ok(())
    }

    async fn send_telegram(&self, message: &str) -> Result<()> {
        let token = self.telegram_token.as_ref().expect("validert i new()");
        let url = format!("https://api.telegram.org/bot{token}/sendMessage");
        self.client
            .post(&url)
            .json(&json!({
                "chat_id": self.cfg.telegram_chat_id,
                "text": format!("📈 b-rs\n{message}"),
            }))
            .send()
            .await
            .context("fikk ikke kontakt med Telegram")?
            .error_for_status()
            .context("Telegram avviste varselet — sjekk token og chat_id")?;
        Ok(())
    }
}

/// Diskret systemlyd ved handel/alarm — Windows-innebygd, stille ellers.
/// Ingen nye avhengigheter: user32::MessageBeep finnes på alle Windows.
pub fn beep() {
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        extern "system" {
            fn MessageBeep(u_type: u32) -> i32;
        }
        unsafe {
            MessageBeep(0xFFFF_FFFF);
        }
    }
}

/// 📱 Fjernstyring fra mobilen via ntfy: appen abonnerer på et hemmelig
/// styre-emne og reagerer på kommandoer du sender fra ntfy-appen.
///
/// Kommandoer (ikke bokstavfølsomme):
///   STOPP / STOP / KILL  → kill switch PÅ (stopper alt, kansellerer ordrer)
///   PAUSE                → pause strategien
///   FORTSETT / START     → kill switch AV + fortsett strategien
///
/// Trygge kommandoer (stopp/pause) har forrang. Emnenavnet er eneste
/// passord — hold det hemmelig, ellers kan andre stoppe handelen din.
pub async fn remote_control_task(
    cfg: NotifyCfg,
    flags: std::sync::Arc<crate::state::Flags>,
    state: crate::state::SharedState,
) {
    use std::sync::atomic::Ordering;

    let server = cfg.ntfy_server.trim_end_matches('/').to_string();
    let topic = if cfg.control_topic.is_empty() {
        format!("{}-styr", cfg.ntfy_topic)
    } else {
        cfg.control_topic.clone()
    };
    if cfg.ntfy_topic.is_empty() && cfg.control_topic.is_empty() {
        state.lock().unwrap().log("📱 Fjernstyring av: mangler ntfy-emne.");
        return;
    }
    state
        .lock()
        .unwrap()
        .log(format!("📱 Fjernstyring PÅ — send STOPP/PAUSE/FORTSETT til ntfy-emnet «{topic}»."));

    // Egen klient uten timeout: strømmen er langlevd med vilje.
    let Ok(client) = reqwest::Client::builder().build() else { return };
    let url = format!("{server}/{topic}/json");

    while !flags.quit.load(Ordering::Relaxed) {
        let resp = match client.get(&url).send().await {
            Ok(r) => r,
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                continue;
            }
        };
        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            if flags.quit.load(Ordering::Relaxed) {
                return;
            }
            let Ok(bytes) = chunk else { break };
            buf.push_str(&String::from_utf8_lossy(&bytes));
            // ntfy sender én JSON-linje per hendelse.
            while let Some(nl) = buf.find('\n') {
                let line = buf[..nl].trim().to_string();
                buf.drain(..=nl);
                if line.is_empty() {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                if v.get("event").and_then(|e| e.as_str()) != Some("message") {
                    continue; // hopp over "open"/"keepalive"
                }
                let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
                handle_command(msg, &flags, &state, &client, &server, &cfg.ntfy_topic).await;
            }
        }
        // Strømmen falt — vent litt og koble på igjen.
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

async fn handle_command(
    raw: &str,
    flags: &std::sync::Arc<crate::state::Flags>,
    state: &crate::state::SharedState,
    client: &reqwest::Client,
    server: &str,
    reply_topic: &str,
) {
    use std::sync::atomic::Ordering;
    let cmd = raw.trim().to_uppercase();
    let (svar, gjort) = match cmd.as_str() {
        "STOPP" | "STOP" | "KILL" => {
            flags.killed.store(true, Ordering::Relaxed);
            ("⛔ KILL SWITCH slått PÅ fra mobil — all handel stoppet.", true)
        }
        "PAUSE" => {
            flags.paused.store(true, Ordering::Relaxed);
            ("⏸ Strategien satt på pause fra mobil.", true)
        }
        "FORTSETT" | "START" | "RESUME" | "GJENOPPTA" => {
            flags.killed.store(false, Ordering::Relaxed);
            flags.paused.store(false, Ordering::Relaxed);
            ("▶ Handel gjenopptatt fra mobil (kill switch av, strategi i gang).", true)
        }
        _ => ("", false),
    };
    if !gjort {
        return;
    }
    {
        let mut st = state.lock().unwrap();
        st.log(format!("📱 Fjernkommando: {cmd} → {svar}"));
        st.toast(svar);
    }
    // Bekreft tilbake til hovedemnet så du ser det på mobilen.
    if !reply_topic.is_empty() {
        let url = format!("{server}/{reply_topic}");
        let _ = client
            .post(&url)
            .header("X-Title", "b-rs")
            .body(svar.to_string())
            .send()
            .await;
    }
}
