//! InitConnection — WebSocket signaling for the RDOC-SACompanion mesh.
//!
//! Dumb relay: routes offer/answer/ice by `to`, keeps the per-room roster,
//! enforces room-auth + cap, mints ephemeral TURN creds. No media here.
//!
//! Env:
//!   PORT             listen port (default 8080)
//!   ROOM_AUTH_SECRET HMAC secret for room tokens (unset = open dev mode)
//!   TURN_SECRET      coturn shared secret (optional)
//!   TURN_URLS        comma-separated turn: urls (optional)
//!
//! Subcommand: `init-connection mint <room>` prints that room's join token.

mod auth;
mod tls;
mod turn;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use futures::{SinkExt, StreamExt};
use protocol::{ClientMsg, PeerInfo, ServerMsg};
use tokio::sync::mpsc;

use auth::AuthConfig;
use turn::TurnConfig;

/// Soft cap → quality warning. Hard cap → join refused. (ARCHITECTURE §10.)
const WARN_CAP: usize = 12;
const HARD_CAP: usize = 16;

struct PeerHandle {
    name: String,
    tx: mpsc::UnboundedSender<ServerMsg>,
}

type Room = HashMap<String, PeerHandle>; // user_id -> handle

struct AppState {
    rooms: Mutex<HashMap<String, Room>>,
    auth: AuthConfig,
    turn: Option<TurnConfig>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "init_connection=info,tower_http=info".into()),
        )
        .init();

    let auth = AuthConfig::from_env();

    // `mint <room>` helper: print the join token for a room and exit.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("mint") {
        let room = args.get(2).cloned().unwrap_or_default();
        match auth.token_for(&room) {
            Some(t) => println!("{t}"),
            None => eprintln!("ROOM_AUTH_SECRET not set — open mode, no token needed"),
        }
        return Ok(());
    }

    if matches!(auth, AuthConfig::Open) {
        tracing::warn!("ROOM_AUTH_SECRET unset — OPEN mode, any client may join any room (dev only)");
    }
    let turn = TurnConfig::from_env();
    tracing::info!("TURN minting: {}", if turn.is_some() { "enabled" } else { "disabled" });

    let state = Arc::new(AppState { rooms: Mutex::new(HashMap::new()), auth, turn });

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state);

    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8080);

    // TLS by default (wss). TLS_DISABLE=1 → plain ws, loopback dev only.
    if std::env::var("TLS_DISABLE").is_ok() {
        tracing::warn!("TLS_DISABLE set — serving PLAIN ws (loopback dev only)");
        let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
        tracing::info!("InitConnection listening (ws) on :{port} (warn@{WARN_CAP} hard@{HARD_CAP})");
        axum::serve(listener, app).await?;
        return Ok(());
    }

    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert_path = std::env::var("TLS_CERT").unwrap_or_else(|_| "init-cert.pem".into());
    let key_path = std::env::var("TLS_KEY").unwrap_or_else(|_| "init-key.pem".into());
    let cert = tls::ensure(&cert_path, &key_path)?;
    tracing::info!("InitConnection listening (wss) on :{port} (warn@{WARN_CAP} hard@{HARD_CAP})");
    tracing::info!("TLS cert SHA-256 (pin this on the client): {}", cert.fingerprint);
    println!("CERT_SHA256={}", cert.fingerprint);
    let rustls_config = axum_server::tls_rustls::RustlsConfig::from_pem(
        cert.cert_pem.into_bytes(),
        cert.key_pem.into_bytes(),
    )
    .await?;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    axum_server::bind_rustls(addr, rustls_config)
        .serve(app.into_make_service())
        .await?;
    Ok(())
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMsg>();

    // Writer task: serialize ServerMsg → WS text frames.
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let Ok(txt) = serde_json::to_string(&msg) else { continue };
            if sink.send(Message::Text(txt)).await.is_err() {
                break;
            }
        }
    });

    // (room, user_id) once this socket has joined.
    let mut me: Option<(String, String)> = None;

    while let Some(Ok(msg)) = stream.next().await {
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };
        let cmsg: ClientMsg = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                let _ = tx.send(ServerMsg::Error { code: "bad_json".into(), message: e.to_string() });
                continue;
            }
        };

        match cmsg {
            ClientMsg::Join { room, user_id, name, token } => {
                if me.is_some() {
                    let _ = tx.send(ServerMsg::Error {
                        code: "already_joined".into(),
                        message: "this socket already joined a room".into(),
                    });
                    continue;
                }
                if !state.auth.check(&room, token.as_deref()) {
                    let _ = tx.send(ServerMsg::Error {
                        code: "bad_token".into(),
                        message: "invalid room token".into(),
                    });
                    break;
                }

                let (roster, size) = {
                    let mut rooms = state.rooms.lock().unwrap();
                    let r = rooms.entry(room.clone()).or_default();
                    // Cap check (a rejoining same user_id doesn't count as growth).
                    if r.len() >= HARD_CAP && !r.contains_key(&user_id) {
                        let _ = tx.send(ServerMsg::RoomFull { cap: HARD_CAP });
                        drop(rooms);
                        break;
                    }
                    // Supersede a stale connection with the same user_id.
                    if let Some(old) = r.remove(&user_id) {
                        let _ = old.tx.send(ServerMsg::Error {
                            code: "superseded".into(),
                            message: "joined from another connection".into(),
                        });
                    }
                    let roster: Vec<PeerInfo> = r
                        .iter()
                        .map(|(id, h)| PeerInfo { user_id: id.clone(), name: h.name.clone() })
                        .collect();
                    r.insert(user_id.clone(), PeerHandle { name: name.clone(), tx: tx.clone() });
                    // Tell existing peers about the newcomer.
                    for (id, h) in r.iter() {
                        if id != &user_id {
                            let _ = h.tx.send(ServerMsg::PeerJoined {
                                user_id: user_id.clone(),
                                name: name.clone(),
                            });
                        }
                    }
                    let size = r.len();
                    // Soft-cap warning to everyone in the room.
                    if size >= WARN_CAP {
                        for h in r.values() {
                            let _ = h.tx.send(ServerMsg::Warn { size, cap: WARN_CAP });
                        }
                    }
                    (roster, size)
                };

                let _ = tx.send(ServerMsg::Roster { peers: roster });
                if let Some(turn) = &state.turn {
                    let _ = tx.send(ServerMsg::Turn(turn.mint(&user_id)));
                }
                tracing::info!(%room, %user_id, size, "join");
                me = Some((room, user_id));
            }

            ClientMsg::Offer { to, sdp } => {
                relay_to(&state, &me, &to, |from| ServerMsg::Offer { from, sdp });
            }
            ClientMsg::Answer { to, sdp } => {
                relay_to(&state, &me, &to, |from| ServerMsg::Answer { from, sdp });
            }
            ClientMsg::Ice { to, candidate } => {
                relay_to(&state, &me, &to, |from| ServerMsg::Ice { from, candidate });
            }
            ClientMsg::Ptt { active } => {
                if let Some((room, from)) = &me {
                    let rooms = state.rooms.lock().unwrap();
                    if let Some(r) = rooms.get(room) {
                        for (id, h) in r.iter() {
                            if id != from {
                                let _ = h.tx.send(ServerMsg::Ptt {
                                    user_id: from.clone(),
                                    active,
                                });
                            }
                        }
                    }
                }
            }
            ClientMsg::Leave => break,
        }
    }

    // Cleanup: drop from room, notify peers.
    if let Some((room, user_id)) = me {
        let mut rooms = state.rooms.lock().unwrap();
        if let Some(r) = rooms.get_mut(&room) {
            r.remove(&user_id);
            for h in r.values() {
                let _ = h.tx.send(ServerMsg::PeerLeft { user_id: user_id.clone() });
            }
            if r.is_empty() {
                rooms.remove(&room);
            }
        }
        tracing::info!(%room, %user_id, "leave");
    }
    writer.abort();
}

/// Relay a built message to a single peer (by user_id) in the sender's room,
/// stamping the sender's id as `from`.
fn relay_to(
    state: &AppState,
    me: &Option<(String, String)>,
    to: &str,
    build: impl FnOnce(String) -> ServerMsg,
) {
    let Some((room, from)) = me else { return };
    let rooms = state.rooms.lock().unwrap();
    if let Some(r) = rooms.get(room) {
        if let Some(h) = r.get(to) {
            let _ = h.tx.send(build(from.clone()));
        }
    }
}
