//! Room-auth. Each room's join token = HMAC-SHA256(secret, room) as hex.
//! Whoever holds the server secret + room name can mint the invite token
//! (see the `mint` subcommand). Expiry/rotation: later (§15.4).

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub enum AuthConfig {
    /// Dev mode: no token required. Loudly warned at startup.
    Open,
    Hmac(Vec<u8>),
}

impl AuthConfig {
    /// Build from env, FAIL-CLOSED: Open mode is only allowed when explicitly
    /// opted into via `ALLOW_OPEN_AUTH=1` (dev). Otherwise a missing
    /// `ROOM_AUTH_SECRET` is a hard error so production never silently runs open.
    pub fn from_env() -> Result<Self, String> {
        match std::env::var("ROOM_AUTH_SECRET") {
            Ok(s) if !s.is_empty() => Ok(Self::Hmac(s.into_bytes())),
            _ => {
                let allow_open = std::env::var("ALLOW_OPEN_AUTH")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);
                if allow_open {
                    Ok(Self::Open)
                } else {
                    Err("ROOM_AUTH_SECRET is not set — refusing to start in OPEN auth mode. \
Set ROOM_AUTH_SECRET=<secret> for production, or ALLOW_OPEN_AUTH=1 for local dev only."
                        .into())
                }
            }
        }
    }

    /// The valid join token for a room (None in Open mode).
    pub fn token_for(&self, room: &str) -> Option<String> {
        match self {
            Self::Open => None,
            Self::Hmac(key) => Some(hmac_hex(key, room.as_bytes())),
        }
    }

    /// Validate a presented token for a room (constant-time).
    pub fn check(&self, room: &str, token: Option<&str>) -> bool {
        match self {
            Self::Open => true,
            Self::Hmac(key) => {
                let expected = hmac_hex(key, room.as_bytes());
                token.map(|t| ct_eq(t.as_bytes(), expected.as_bytes())).unwrap_or(false)
            }
        }
    }
}

fn hmac_hex(key: &[u8], msg: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(msg);
    hex::encode(mac.finalize().into_bytes())
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}
