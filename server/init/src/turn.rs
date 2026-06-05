//! Ephemeral TURN credentials (coturn `use-auth-secret` REST scheme).
//! username = "<unix-expiry>:<user_id>", credential = base64(HMAC-SHA1(secret, username)).
//! coturn validates the HMAC itself — no account store, no state.

use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use protocol::TurnCreds;
use sha1::Sha1;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha1 = Hmac<Sha1>;

pub struct TurnConfig {
    secret: Vec<u8>,
    urls: Vec<String>,
    ttl: u32,
}

impl TurnConfig {
    /// Enabled only when both TURN_SECRET and TURN_URLS (comma-separated) are set.
    pub fn from_env() -> Option<Self> {
        let secret = std::env::var("TURN_SECRET").ok().filter(|s| !s.is_empty())?;
        let urls = std::env::var("TURN_URLS").ok().filter(|s| !s.is_empty())?;
        Some(Self {
            secret: secret.into_bytes(),
            urls: urls.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
            ttl: 3600,
        })
    }

    pub fn mint(&self, user_id: &str) -> TurnCreds {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let expiry = now + self.ttl as u64;
        let username = format!("{expiry}:{user_id}");
        let mut mac = HmacSha1::new_from_slice(&self.secret).expect("HMAC accepts any key length");
        mac.update(username.as_bytes());
        let credential = STANDARD.encode(mac.finalize().into_bytes());
        TurnCreds { urls: self.urls.clone(), username, credential, ttl: self.ttl }
    }
}
