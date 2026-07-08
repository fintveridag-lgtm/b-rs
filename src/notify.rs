use crate::config::NotifyCfg;
use anyhow::{Context, Result};
use serde_json::json;

/// Push-varsler til mobil. To tilbydere:
///
/// **ntfy** (enklest, gratis, ingen konto):
///   1. Installer «ntfy»-appen (App Store / Google Play)
///   2. Abonner på et selvvalgt, hemmelig emne, f.eks. "bors-ola-73xk1"
///   3. Sett samme navn i `notify.ntfy_topic` i config.toml
///   Emnenavnet er eneste «passord» — velg noe ugjettelig.
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
