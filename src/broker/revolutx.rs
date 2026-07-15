use super::Broker;
use crate::config::RevolutXCfg;
use crate::types::{Order, OrderRequest, OrderStatus, Position, Side};
use anyhow::{Context, Result};
use base64::Engine as _;
use chrono::Utc;
use ed25519_dalek::pkcs8::DecodePrivateKey;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};

/// Revolut X — Revoluts kryptobørs, med offisielt REST-API for
/// personkontoer.
///
/// Oppsett:
///   1. Lag et Ed25519-nøkkelpar og registrer offentlig nøkkel i
///      Revolut X → Settings → API keys:
///      `openssl genpkey -algorithm ed25519 -out revolutx.pem`
///      og `openssl pkey -in revolutx.pem -pubout`
///   2. Sett API-nøkkelen du får i miljøvariabelen REVOLUTX_API_KEY,
///      og stien til revolutx.pem i konfigens [revolutx].private_key_path.
///   3. Bruk kryptosymboler i watchlisten, f.eks. "BTC-USD", "ETH-USD" —
///      samme format hos Yahoo (kursdata) og Revolut X (handel).
///
/// Autentisering: hver forespørsel signeres med Ed25519 over
/// tidsstempel + metode + sti + query + kropp (uten skilletegn).
pub struct RevolutXBroker {
    client: reqwest::Client,
    /// F.eks. "https://revx.revolut.com" (uten sti).
    origin: String,
    /// F.eks. "/api/1.0" — inngår i signaturmeldingen.
    base_path: String,
    api_key: String,
    key: SigningKey,
    quote: String,
}

/// Meldingen som signeres — nøyaktig konkatenering per Revoluts spesifikasjon.
fn build_message(ts: &str, method: &str, path: &str, query: &str, body: &str) -> String {
    format!("{ts}{method}{path}{query}{body}")
}

fn map_state(state: &str) -> OrderStatus {
    match state {
        "filled" => OrderStatus::Filled,
        "rejected" => OrderStatus::Rejected,
        "cancelled" => OrderStatus::Cancelled,
        _ => OrderStatus::Submitted, // pending_new | new | partially_filled | replaced
    }
}

fn as_f64_str(v: &Value, key: &str) -> f64 {
    v.get(key)
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0)
}

const FIAT: &[&str] = &["USD", "EUR", "GBP", "CHF", "NOK", "SEK", "DKK", "PLN", "RON", "HUF", "CZK"];

impl RevolutXBroker {
    pub fn new(cfg: &RevolutXCfg) -> Result<Self> {
        let api_key = std::env::var("REVOLUTX_API_KEY")
            .context("miljøvariabelen REVOLUTX_API_KEY er ikke satt")?;
        let pem = std::fs::read_to_string(&cfg.private_key_path)
            .with_context(|| format!("kunne ikke lese privatnøkkelen {}", cfg.private_key_path))?;
        let key = SigningKey::from_pkcs8_pem(&pem)
            .map_err(|e| anyhow::anyhow!("ugyldig Ed25519-privatnøkkel (PKCS#8 PEM): {e}"))?;
        let url = reqwest::Url::parse(cfg.base_url.trim_end_matches('/'))
            .context("ugyldig revolutx.base_url")?;
        let origin = format!(
            "{}://{}",
            url.scheme(),
            url.host_str().context("mangler vert i revolutx.base_url")?
        );
        let base_path = url.path().trim_end_matches('/').to_string();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()?;
        Ok(Self {
            client,
            origin,
            base_path,
            api_key,
            key,
            quote: cfg.quote_currency.clone(),
        })
    }

    /// Verifiser nøklene ved oppstart med et harmløst kall.
    pub async fn check_session(&self) -> Result<()> {
        self.request(reqwest::Method::GET, "/balances", "", None)
            .await
            .context("Revolut X avviste API-nøklene — sjekk REVOLUTX_API_KEY og privatnøkkelen")?;
        Ok(())
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &str,
        body: Option<String>,
    ) -> Result<Value> {
        let ts = Utc::now().timestamp_millis().to_string();
        let full_path = format!("{}{path}", self.base_path);
        let message = build_message(
            &ts,
            method.as_str(),
            &full_path,
            query,
            body.as_deref().unwrap_or(""),
        );
        let signature = base64::engine::general_purpose::STANDARD
            .encode(self.key.sign(message.as_bytes()).to_bytes());

        let url = if query.is_empty() {
            format!("{}{full_path}", self.origin)
        } else {
            format!("{}{full_path}?{query}", self.origin)
        };
        let mut rb = self
            .client
            .request(method, &url)
            .header("X-Revx-API-Key", &self.api_key)
            .header("X-Revx-Timestamp", &ts)
            .header("X-Revx-Signature", signature);
        if let Some(b) = body {
            rb = rb.header("Content-Type", "application/json").body(b);
        }

        let resp = rb.send().await.context("fikk ikke kontakt med Revolut X")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Revolut X svarte {status}: {text}");
        }
        if status == reqwest::StatusCode::NO_CONTENT {
            return Ok(Value::Null);
        }
        resp.json().await.context("ugyldig JSON fra Revolut X")
    }
}

#[async_trait::async_trait]
impl Broker for RevolutXBroker {
    fn name(&self) -> &'static str {
        "revolutx"
    }

    async fn place_order(&self, req: OrderRequest) -> Result<Order> {
        let side = match req.side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };
        let body_json = json!({
            "client_order_id": uuid::Uuid::new_v4().to_string(),
            "symbol": req.symbol,
            "side": side,
            "order_configuration": {
                "market": { "base_size": format!("{:.8}", req.qty) }
            }
        });
        let body = serde_json::to_string(&body_json)?;
        let v = self
            .request(reqwest::Method::POST, "/orders", "", Some(body))
            .await?;
        let data = v
            .pointer("/data")
            .with_context(|| format!("uventet ordresvar fra Revolut X: {v}"))?;
        let id = data
            .get("venue_order_id")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        let state = data.get("state").and_then(Value::as_str).unwrap_or("new");

        Ok(Order {
            id,
            symbol: req.symbol,
            side: req.side,
            qty: req.qty,
            status: map_state(state),
            avg_price: req.ref_price,
            created: Utc::now(),
            note: req.note,
        })
    }

    async fn cancel_all(&self) -> Result<()> {
        let v = self
            .request(
                reqwest::Method::GET,
                "/orders/active",
                "order_states=pending_new,new,partially_filled",
                None,
            )
            .await?;
        for o in v.pointer("/data").and_then(Value::as_array).cloned().unwrap_or_default() {
            if let Some(id) = o.get("venue_order_id").and_then(Value::as_str) {
                let _ = self
                    .request(reqwest::Method::DELETE, &format!("/orders/{id}"), "", None)
                    .await;
            }
        }
        Ok(())
    }

    async fn positions(&self) -> Result<Vec<Position>> {
        let v = self.request(reqwest::Method::GET, "/balances", "", None).await?;
        let mut positions = Vec::new();
        for b in v.as_array().cloned().unwrap_or_default() {
            let Some(currency) = b.get("currency").and_then(Value::as_str) else { continue };
            if FIAT.contains(&currency) {
                continue;
            }
            let total = as_f64_str(&b, "total");
            if total <= 0.0 {
                continue;
            }
            positions.push(Position {
                symbol: format!("{currency}-{}", self.quote),
                qty: total,
                avg_price: 0.0,
                last: 0.0,
            });
        }

        // Prising via tickere. Revolut X oppgir ikke kostpris i API-et,
        // så snittkurs settes lik siste — urealisert vises derfor som 0.
        if !positions.is_empty() {
            let symbols: Vec<String> = positions.iter().map(|p| p.symbol.clone()).collect();
            if let Ok(t) = self
                .request(
                    reqwest::Method::GET,
                    "/tickers",
                    &format!("symbols={}", symbols.join(",")),
                    None,
                )
                .await
            {
                for tick in t.pointer("/data").and_then(Value::as_array).cloned().unwrap_or_default() {
                    // Svar bruker skråstrek ("BTC/USD"), forespørsler bindestrek.
                    let Some(sym) = tick.get("symbol").and_then(Value::as_str) else { continue };
                    let dashed = sym.replace('/', "-");
                    let last = as_f64_str(&tick, "last_price");
                    if let Some(p) = positions.iter_mut().find(|p| p.symbol == dashed) {
                        p.last = last;
                        p.avg_price = last;
                    }
                }
            }
        }
        Ok(positions)
    }

    async fn cash(&self) -> Result<f64> {
        let v = self.request(reqwest::Method::GET, "/balances", "", None).await?;
        for b in v.as_array().cloned().unwrap_or_default() {
            if b.get("currency").and_then(Value::as_str) == Some(self.quote.as_str()) {
                return Ok(as_f64_str(&b, "available"));
            }
        }
        Ok(0.0)
    }
}

/// Lag et nytt Ed25519-nøkkelpar uten openssl: privatnøkkelen skrives til
/// `path` (PKCS#8 PEM), og den OFFENTLIGE nøkkelen returneres — det er den
/// som skal limes inn i Revolut X → Settings → API keys.
///
/// Nekter å overskrive en eksisterende fil: en registrert nøkkel som
/// overskrives er tapt for alltid.
pub fn generate_keypair(path: &std::path::Path) -> Result<String> {
    use ed25519_dalek::pkcs8::{EncodePrivateKey, EncodePublicKey};

    anyhow::ensure!(
        !path.exists(),
        "{} finnes allerede — slett eller flytt den gamle først (en registrert nøkkel som overskrives er tapt)",
        path.display()
    );
    let key = SigningKey::generate(&mut rand_core::OsRng);
    // Default::default() = plattformens linjeskift — typen er ikke
    // re-eksportert fra ed25519-dalek, men trengs heller ikke ved navn.
    let private_pem = key
        .to_pkcs8_pem(Default::default())
        .map_err(|e| anyhow::anyhow!("klarte ikke kode privatnøkkelen: {e}"))?;
    std::fs::write(path, private_pem.as_bytes())
        .with_context(|| format!("klarte ikke skrive {}", path.display()))?;
    let public_pem = key
        .verifying_key()
        .to_public_key_pem(Default::default())
        .map_err(|e| anyhow::anyhow!("klarte ikke kode offentlig nøkkel: {e}"))?;
    Ok(public_pem)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Verifier;

    #[test]
    fn message_matches_documented_format() {
        let msg = build_message(
            "1765360896219",
            "POST",
            "/api/1.0/orders",
            "",
            r#"{"client_order_id":"abc","symbol":"BTC-USD"}"#,
        );
        assert_eq!(
            msg,
            r#"1765360896219POST/api/1.0/orders{"client_order_id":"abc","symbol":"BTC-USD"}"#
        );
        // Query flettes inn uten '?' mellom sti og kropp.
        let msg = build_message("1", "GET", "/api/1.0/orders/active", "limit=10", "");
        assert_eq!(msg, "1GET/api/1.0/orders/activelimit=10");
    }

    #[test]
    fn signature_roundtrip_verifies() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let msg = build_message("123", "GET", "/api/1.0/balances", "", "");
        let sig = key.sign(msg.as_bytes());
        assert!(key.verifying_key().verify(msg.as_bytes(), &sig).is_ok());
        // og base64-koding gir gyldig tekst
        let encoded = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
        assert!(!encoded.is_empty());
    }

    #[test]
    fn generated_keypair_roundtrips_and_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("b-rs-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("revolutx.pem");

        let public = generate_keypair(&path).unwrap();
        assert!(public.contains("BEGIN PUBLIC KEY"));
        // Privatnøkkelen på disk kan leses tilbake og hører til samme par.
        let pem = std::fs::read_to_string(&path).unwrap();
        assert!(pem.contains("BEGIN PRIVATE KEY"));
        let key = SigningKey::from_pkcs8_pem(&pem).unwrap();
        use ed25519_dalek::pkcs8::EncodePublicKey;
        let roundtrip: String = key.verifying_key().to_public_key_pem(Default::default()).unwrap();
        assert_eq!(roundtrip, public);
        // Aldri overskriv en eksisterende nøkkel.
        assert!(generate_keypair(&path).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn maps_order_states() {
        assert_eq!(map_state("filled"), OrderStatus::Filled);
        assert_eq!(map_state("rejected"), OrderStatus::Rejected);
        assert_eq!(map_state("cancelled"), OrderStatus::Cancelled);
        assert_eq!(map_state("new"), OrderStatus::Submitted);
        assert_eq!(map_state("partially_filled"), OrderStatus::Submitted);
    }
}
