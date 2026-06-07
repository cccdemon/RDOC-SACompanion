//! Signaling wire format for RDOC VoiceMesh.
//!
//! JSON over WebSocket. Tag field is `t`; variants are kebab-case
//! (`peer-joined`, `room-full`). This crate is the single source of truth —
//! the doc examples in ARCHITECTURE.md are illustrative; these types win.
//!
//! The InitConnection server is a dumb relay: it routes `offer`/`answer`/`ice`
//! by `to`, keeps the roster, enforces auth + cap, and mints TURN creds. It
//! never sees media. Glare (who offers) is decided client-side from user ids.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub user_id: String,
    pub name: String,
}

/// Text-chat payload sent over the per-peer WebRTC DataChannel (NOT through
/// the signaling server). Sender identity = the peer owning the channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMsg {
    pub text: String,
    pub ts: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnCreds {
    pub urls: Vec<String>,
    pub username: String,
    pub credential: String,
    pub ttl: u32,
}

/// Client → Server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum ClientMsg {
    /// First message on a connection. `token` is the room-auth token
    /// (required when the server runs with ROOM_AUTH_SECRET).
    Join {
        room: String,
        user_id: String,
        name: String,
        #[serde(default)]
        token: Option<String>,
    },
    Offer { to: String, sdp: String },
    Answer { to: String, sdp: String },
    Ice { to: String, candidate: String },
    /// Speaking-state for the roster (optional UX).
    Ptt { active: bool },
    Leave,
}

/// Server → Client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "kebab-case")]
pub enum ServerMsg {
    /// Existing peers in the room (sent to the joiner; self excluded).
    Roster { peers: Vec<PeerInfo> },
    /// Ephemeral TURN credentials for this session.
    Turn(TurnCreds),
    PeerJoined { user_id: String, name: String },
    PeerLeft { user_id: String },
    Offer { from: String, sdp: String },
    Answer { from: String, sdp: String },
    Ice { from: String, candidate: String },
    Ptt { user_id: String, active: bool },
    /// Join refused — room at hard cap.
    RoomFull { cap: usize },
    /// Soft cap reached: client should show a quality-warning banner.
    Warn { size: usize, cap: usize },
    Error { code: String, message: String },
}
