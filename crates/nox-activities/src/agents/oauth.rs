//! Claude plan limits from the account, ONLY when the user enabled
//! `agents.allow_limit_queries`. The OAuth access token Claude Code already
//! keeps in the login Keychain (`Claude Code-credentials`) is read for the
//! request and dropped right after: never copied, persisted, refreshed or
//! logged. macOS may ask once to allow Nox to read that Keychain item.

use super::limits;
use nox_core::model::Timestamp;
use nox_core::model::agents::LimitWindow;
use serde::Deserialize;
use std::time::Duration;

const URL: &str = "https://api.anthropic.com/api/oauth/usage";
const SERVICE: &str = "Claude Code-credentials";
/// A reading this recent still stands.
pub const REFRESH: f64 = 5.0 * 60.0;
/// After a failure, wait longer.
pub const RETRY: f64 = 15.0 * 60.0;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    claude_ai_oauth: Option<Token>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Token {
    access_token: String,
    expires_at: Option<f64>,
}

fn token(now: Timestamp) -> Result<String, String> {
    let user = std::env::var("USER").unwrap_or_default();
    let bytes =
        security_framework::passwords::get_generic_password(SERVICE, &user).map_err(|_| {
            "jeton Claude Code introuvable dans le trousseau (ou accès refusé)".to_string()
        })?;
    let stored: Stored =
        serde_json::from_slice(&bytes).map_err(|_| "format de jeton inconnu".to_string())?;
    drop(bytes);
    let t = stored
        .claude_ai_oauth
        .ok_or_else(|| "Claude Code n’est pas connecté avec un compte".to_string())?;
    if t.expires_at.is_some_and(|e| e / 1000.0 <= now) {
        return Err("jeton expiré : ouvrez Claude Code pour le renouveler".into());
    }
    Ok(t.access_token)
}

/// One request; blocking with a short timeout (call from a worker thread).
pub fn fetch(now: Timestamp) -> Result<Vec<LimitWindow>, String> {
    let token = token(now)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .http_status_as_error(false)
        .build()
        .into();
    let res = agent
        .get(URL)
        .header("Authorization", &format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", concat!("Nox/", env!("CARGO_PKG_VERSION")))
        .call();
    drop(token);
    let mut res = res.map_err(|e| format!("réseau : {e}"))?;
    let status = res.status().as_u16();
    if status == 401 || status == 403 {
        return Err("compte refusé (reconnectez Claude Code)".into());
    }
    if status != 200 {
        return Err(format!("réponse HTTP {status}"));
    }
    let body = res
        .body_mut()
        .with_config()
        .limit(256 * 1024)
        .read_to_string()
        .map_err(|e| format!("lecture : {e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&body).map_err(|_| "réponse illisible".to_string())?;
    let w = limits::from_usage_response(&v, now);
    if w.is_empty() {
        return Err("aucune limite dans la réponse".into());
    }
    Ok(w)
}
