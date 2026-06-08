//! PIN-protected session brokering. A host creates a session → random room +
//! join token + a random 6-digit PIN + a short share code. Mates resolve the
//! code with the PIN (rate-limited, so a 6-digit PIN is safe) and get the
//! room + token to connect config-less. State is in-memory + TTL'd (sessions
//! are ephemeral, like rooms).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rand::Rng;

/// Hard cap: a session ends at most 24h after creation, no matter what.
const MAX_AGE_SECS: u64 = 24 * 3600;
/// Grace after the room goes empty (covers create→connect + brief reconnects).
const EMPTY_GRACE_SECS: u64 = 5 * 60;
const MAX_ATTEMPTS: u32 = 6; // wrong-PIN tries before the code locks

pub struct Session {
    pub room: String,
    pub token: Option<String>,
    pin: String,
    created: u64,
    /// Last time the room had ≥1 connected member (init = created, so the host
    /// has the grace window to connect before it counts as empty).
    last_active: u64,
    attempts: u32,
}

pub enum JoinError {
    NotFound,
    Locked,
    BadPin,
}

#[derive(Default)]
pub struct Sessions {
    inner: Mutex<HashMap<String, Session>>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn rand_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    rand::thread_rng().fill(&mut b[..]);
    hex::encode(b)
}

fn rand_code() -> String {
    // Unambiguous alphabet (no 0/o/1/l): easy to read off a share link.
    const CH: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
    let mut r = rand::thread_rng();
    (0..8).map(|_| CH[r.gen_range(0..CH.len())] as char).collect()
}

fn rand_pin() -> String {
    let n: u32 = rand::thread_rng().gen_range(0..1_000_000);
    format!("{n:06}")
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

impl Sessions {
    /// Create a session. `token_for` mints the room's join token (None in open mode).
    /// Returns (code, pin, room, token).
    pub fn create<F: Fn(&str) -> Option<String>>(
        &self,
        token_for: F,
    ) -> (String, String, String, Option<String>) {
        let room = format!("squad-{}", rand_hex(8)); // 64-bit random room name
        let token = token_for(&room);
        let pin = rand_pin();
        let t = now();
        let mut map = self.inner.lock().unwrap();
        let mut code = rand_code();
        while map.contains_key(&code) {
            code = rand_code();
        }
        map.insert(
            code.clone(),
            Session { room: room.clone(), token: token.clone(), pin: pin.clone(), created: t, last_active: t, attempts: 0 },
        );
        (code, pin, room, token)
    }

    /// Resolve a code with a PIN. Rate-limited per code.
    pub fn join(&self, code: &str, pin: &str) -> Result<(String, Option<String>), JoinError> {
        let mut map = self.inner.lock().unwrap();
        let s = map.get_mut(code).ok_or(JoinError::NotFound)?;
        if s.attempts >= MAX_ATTEMPTS {
            return Err(JoinError::Locked);
        }
        if ct_eq(s.pin.as_bytes(), pin.as_bytes()) {
            s.last_active = now(); // a successful join keeps it alive
            Ok((s.room.clone(), s.token.clone()))
        } else {
            s.attempts += 1;
            Err(JoinError::BadPin)
        }
    }

    /// Lifecycle sweep (call periodically). A session is kept while its room has
    /// connected members; once empty it survives EMPTY_GRACE, and never past
    /// MAX_AGE. `room_nonempty(room)` reports live membership.
    pub fn reap<F: Fn(&str) -> bool>(&self, room_nonempty: F) {
        let n = now();
        let mut map = self.inner.lock().unwrap();
        map.retain(|_, s| {
            if n.saturating_sub(s.created) >= MAX_AGE_SECS {
                return false; // 24h hard cap
            }
            if room_nonempty(&s.room) {
                s.last_active = n;
                return true;
            }
            n.saturating_sub(s.last_active) < EMPTY_GRACE_SECS
        });
    }
}
