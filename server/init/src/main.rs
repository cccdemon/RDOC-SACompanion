//! InitConnection — WebSocket signaling for the RDOC SquadLink Lite mesh.
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
mod sessions;
mod tls;
mod turn;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{SinkExt, StreamExt};
use protocol::{ClientMsg, PeerInfo, ServerMsg};
use serde_json::json;
use tokio::sync::mpsc;
use tower_http::cors::CorsLayer;

use auth::AuthConfig;
use sessions::{JoinError, Sessions};
use turn::TurnConfig;

/// Public base URL (for share links) + ws URL handed back on join.
fn public_base() -> String {
    std::env::var("PUBLIC_BASE").unwrap_or_else(|_| "https://squadlink.raumdock.org".into())
}
fn public_ws() -> String {
    std::env::var("PUBLIC_WS").unwrap_or_else(|_| "wss://squadlink.raumdock.org/ws".into())
}

/// Soft cap → quality warning. Hard cap → join refused. (ARCHITECTURE §10.)
const WARN_CAP: usize = 12;
const HARD_CAP: usize = 16;

// ── Input limits (defense against oversized/abusive frames) ──────────────────
const MAX_WS_MSG: usize = 64 * 1024; // whole WS text frame
const MAX_REST_BODY: usize = 4 * 1024; // REST JSON body
const MAX_ID: usize = 64; // room / user_id
const MAX_NAME: usize = 64;
const MAX_TOKEN: usize = 128;
const MAX_SDP: usize = 16 * 1024;
const MAX_ICE: usize = 4 * 1024;
const MAX_CODE: usize = 32;
const MAX_PIN: usize = 12;
/// Per-peer outbound queue depth before backpressure drops signaling messages.
const PEER_CHAN: usize = 256;

fn len_ok(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── Per-IP rate limiting (fixed window) against session/PIN bruteforce ───────
const RL_WINDOW: u64 = 300; // 5 min
const RL_JOIN_MAX: u32 = 30; // PIN tries per IP per window
const RL_CREATE_MAX: u32 = 20; // session creations per IP per window

#[derive(Default)]
struct RateLimiter {
    inner: Mutex<HashMap<String, (u64, u32)>>, // ip -> (window_start, count)
}
impl RateLimiter {
    /// Returns true if allowed, false if the IP exceeded `max` in `window`.
    fn allow(&self, ip: &str, max: u32, window: u64) -> bool {
        let now = now_secs();
        let mut m = self.inner.lock().unwrap();
        let e = m.entry(ip.to_string()).or_insert((now, 0));
        if now.saturating_sub(e.0) >= window {
            *e = (now, 0);
        }
        e.1 += 1;
        e.1 <= max
    }
    fn prune(&self, window: u64) {
        let now = now_secs();
        self.inner.lock().unwrap().retain(|_, (start, _)| now.saturating_sub(*start) < window);
    }
}

/// Best-effort client IP: first hop of X-Forwarded-For (set by our reverse
/// proxy), else a constant bucket. Good enough for coarse abuse throttling.
fn client_ip(headers: &axum::http::HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

struct PeerHandle {
    name: String,
    tx: mpsc::Sender<ServerMsg>,
}

type Room = HashMap<String, PeerHandle>; // user_id -> handle

struct AppState {
    rooms: Mutex<HashMap<String, Room>>,
    auth: AuthConfig,
    turn: Option<TurnConfig>,
    sessions: Sessions,
    rate: RateLimiter,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "init_connection=info,tower_http=info".into()),
        )
        .init();

    let auth = AuthConfig::from_env().map_err(|e| anyhow::anyhow!(e))?;

    // `mint <room>` helper: print the join token for a room and exit.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("mint") {
        let room = args.get(2).cloned().unwrap_or_default();
        match auth.token_for(&room) {
            Some(t) => println!("{t}"),
            None => eprintln!("ALLOW_OPEN_AUTH set — open mode, no token needed"),
        }
        return Ok(());
    }

    if matches!(auth, AuthConfig::Open) {
        tracing::warn!("ALLOW_OPEN_AUTH set — OPEN mode, any client may join any room (dev only)");
    }
    let turn = TurnConfig::from_env();
    tracing::info!("TURN minting: {}", if turn.is_some() { "enabled" } else { "disabled" });

    let state = Arc::new(AppState {
        rooms: Mutex::new(HashMap::new()),
        auth,
        turn,
        sessions: Sessions::default(),
        rate: RateLimiter::default(),
    });

    // Session lifecycle: keep a session alive while its room has members,
    // grace after empty, 24h hard cap. Swept once a minute.
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut iv = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                iv.tick().await;
                state.sessions.reap(|room| {
                    state.rooms.lock().unwrap().get(room).map(|r| !r.is_empty()).unwrap_or(false)
                });
                state.rate.prune(RL_WINDOW);
            }
        });
    }

    let app = Router::new()
        .route("/", get(home))
        .route("/privacy", get(privacy))
        .route("/legal", get(legal))
        .route("/license", get(license_page))
        .route("/ws", get(ws_handler))
        .route("/healthz", get(|| async { "ok" }))
        // PIN-protected session brokering (REST, called by the app webview → CORS).
        .route("/session", post(create_session))
        .route("/session/:code/join", post(join_session))
        .route("/j/:code", get(landing))
        .layer(DefaultBodyLimit::max(MAX_REST_BODY))
        .layer(CorsLayer::permissive())
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
    ws.max_message_size(MAX_WS_MSG)
        .max_frame_size(MAX_WS_MSG)
        .on_upgrade(move |socket| handle_socket(socket, state))
}

/// Host creates a session → random room + token + 6-digit PIN + share code.
async fn create_session(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    if !state.rate.allow(&client_ip(&headers), RL_CREATE_MAX, RL_WINDOW) {
        return (StatusCode::TOO_MANY_REQUESTS, Json(json!({ "error": "rate_limited" }))).into_response();
    }
    let (code, pin, room, token) = state.sessions.create(|r| state.auth.token_for(r));
    let base = public_base();
    tracing::info!(%code, %room, "session created");
    Json(json!({
        "code": code,
        "pin": pin,
        "room": room,
        "token": token,
        "ws": public_ws(),
        "link": format!("{base}/j/{code}"),
    }))
    .into_response()
}

/// Mate resolves a code with the PIN (rate-limited) → room + token.
async fn join_session(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if !state.rate.allow(&client_ip(&headers), RL_JOIN_MAX, RL_WINDOW) {
        return (StatusCode::TOO_MANY_REQUESTS, Json(json!({ "error": "rate_limited" }))).into_response();
    }
    let pin = body.get("pin").and_then(|v| v.as_str()).unwrap_or("");
    // Bound code/PIN lengths before touching the session store.
    if !len_ok(&code, MAX_CODE) || !len_ok(pin, MAX_PIN) {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response();
    }
    match state.sessions.join(&code, pin) {
        Ok((room, token)) => {
            Json(json!({ "room": room, "token": token, "ws": public_ws() })).into_response()
        }
        Err(JoinError::NotFound) => {
            (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response()
        }
        Err(JoinError::Locked) => {
            (StatusCode::TOO_MANY_REQUESTS, Json(json!({ "error": "locked" }))).into_response()
        }
        Err(JoinError::BadPin) => {
            (StatusCode::FORBIDDEN, Json(json!({ "error": "bad_pin" }))).into_response()
        }
    }
}

/// Human landing page for a share link: shows the code + download + instructions.
async fn landing(Path(code): Path<String>) -> Html<String> {
    let base = public_base();
    Html(format!(
        r#"<!doctype html><html lang="de"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>RDOC SquadLink Lite — Session beitreten</title>
<style>body{{font-family:system-ui,sans-serif;background:#020814;color:#e2e8f0;max-width:34rem;margin:6vh auto;padding:0 1.2rem;line-height:1.6}}
.code{{font-size:1.6rem;font-weight:700;letter-spacing:.1em;background:#0b1626;border:1px solid #1e293b;border-radius:8px;padding:.6rem 1rem;display:inline-block}}
a.btn{{display:inline-block;margin-top:1rem;padding:.6rem 1.1rem;background:#0284c7;color:#fff;border-radius:8px;text-decoration:none;font-weight:600}}
.muted{{color:#94a3b8;font-size:.92rem}}</style></head>
<body>
<h1>RDOC SquadLink Lite</h1>
<p>Du wurdest zu einer Voice-Session eingeladen.</p>
<p class="muted">Session-Code:</p>
<p class="code">{code}</p>
<ol>
<li>App noch nicht installiert? <a href="{base}/download/">Hier herunterladen</a> und installieren.</li>
<li>App öffnen → <b>Beitreten</b> → Code <code>{code}</code> + die <b>6-stellige PIN</b> (bekommst du vom Host) eingeben.</li>
</ol>
<p><a class="btn" href="{base}/download/">SquadLink Lite herunterladen</a></p>
<p class="muted">Audio läuft direkt Peer-zu-Peer (verschlüsselt). Der Server vermittelt nur.</p>
</body></html>"#
    ))
}

const PAGE_CSS: &str = r#"<style>
:root{color-scheme:dark}
body{font-family:system-ui,-apple-system,Segoe UI,sans-serif;background:#0f1115;color:#dfe3e8;margin:0;line-height:1.6}
main{max-width:40rem;margin:0 auto;padding:1.4rem 1.2rem 3rem}
.top{display:flex;align-items:center;gap:.55rem;padding:.9rem 1.2rem;border-bottom:1px solid #242833}
.top img{width:26px;height:26px;display:block}
.top a{color:#dfe3e8;text-decoration:none;font-weight:600}
h1{font-size:1.45rem;font-weight:600;margin:.3rem 0 .7rem}
h2{font-size:1.05rem;font-weight:600;margin:1.5rem 0 .3rem}
p{margin:.6rem 0}
a{color:#7fb0ff}
.muted{color:#9aa3ad;font-size:.9rem}
ul{padding-left:1.25rem;margin:.5rem 0}
.links a{display:block;margin:.25rem 0}
.dl{display:inline-block;margin:.5rem 0;padding:.5rem .9rem;border:1px solid #3a414e;border-radius:5px;text-decoration:none;color:#dfe3e8}
footer{max-width:40rem;margin:0 auto;padding:1rem 1.2rem;border-top:1px solid #242833;color:#9aa3ad;font-size:.82rem}
footer a{color:#9aa3ad;margin-right:1rem;display:inline-block}
code{background:#1a1d23;padding:.1rem .3rem;border-radius:3px}
</style>"#;

// The standard raumdock logo (same SVG used across the RDOC web surfaces).
const LOGO: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'%3E%3Ccircle cx='50' cy='50' r='48' fill='%230a0a0a'/%3E%3Cpath d='M22 62 q28 14 56 0 l-6-22 q-24-10-44 0 z' fill='%23444'/%3E%3Cellipse cx='50' cy='46' rx='26' ry='18' fill='%23f6c200'/%3E%3Cellipse cx='62' cy='40' rx='8' ry='5' fill='%23ffffff' opacity='.5'/%3E%3C/svg%3E";

const GITHUB_URL: &str = "https://github.com/cccdemon/RDOC-SquadLinkLite";
const RAUMDOCK_URL: &str = "https://raumdock.org";
const FLEET_URL: &str = "https://suite.raumdock.org/fleetplanner";

fn footer_html(base: &str) -> String {
    format!(
        r#"<a href="/">Start</a><a href="{base}/download/">Download</a><a href="/privacy">Datenschutz</a><a href="/legal">Impressum</a><a href="/license">Lizenz</a><a href="{GITHUB_URL}">GitHub</a><a href="{RAUMDOCK_URL}">raumdock.org</a><span class="muted">· serverless P2P voice mesh</span>"#
    )
}

fn shell(title: &str, body: &str) -> Html<String> {
    let base = public_base();
    Html(format!(
        "<!doctype html><html lang=\"de\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{title} — RDOC SquadLink Lite</title><link rel=\"icon\" href=\"{base}/download/sl-logo.png\">{css}</head><body>\
<header class=\"top\"><img src=\"{base}/download/sl-logo.png\" alt=\"SquadLink Lite\" onerror=\"this.onerror=null;this.src='{logo}'\"><a href=\"/\">RDOC SquadLink Lite</a></header>\
<main>{body}</main><footer>{footer}</footer></body></html>",
        base = base,
        logo = LOGO,
        css = PAGE_CSS,
        footer = footer_html(&base),
    ))
}

async fn home() -> Html<String> {
    let base = public_base();
    shell(
        "Was ist das?",
        &format!(
            r#"<h1>RDOC SquadLink Lite</h1>
<p>Ein einfacher Peer-to-Peer-Voice-Chat für kleine Gruppen. Push-to-Talk,
ohne Account, ohne Aufnahme, verschlüsselt.</p>
<p>Die Stimme läuft direkt zwischen den Spielern (WebRTC/Opus). Es gibt keinen Server,
der mithört — ein kleiner Dienst stellt nur die Verbindung her.</p>
<h2>So funktioniert es</h2>
<ul>
<li>Host erstellt in der App eine Session und erhält einen Link und eine 6-stellige PIN.</li>
<li>Mitspieler öffnen den Link, installieren die App, geben Code und PIN ein.</li>
<li>Die Session bleibt bestehen, solange Teilnehmer verbunden sind (maximal 24&nbsp;Stunden).</li>
</ul>
<p><a class="dl" href="{base}/download/">App herunterladen (Windows)</a></p>
<p class="muted">Prototyp, unsigniert. Windows SmartScreen: „Weitere Informationen" und „Trotzdem ausführen".</p>
<h2>Links</h2>
<p class="links">
<a href="{RAUMDOCK_URL}">raumdock.org</a>
<a href="{FLEET_URL}">RDOC Fleetmanager</a>
<a href="{GITHUB_URL}">Quellcode auf GitHub</a>
</p>
<p class="muted"><a href="/privacy">Datenschutz</a> · <a href="/legal">Impressum</a> · <a href="/license">Lizenz</a></p>"#
        ),
    )
}

async fn privacy() -> Html<String> {
    shell(
        "Datenschutz",
        r#"<h1>Datenschutzerklärung</h1>
<p class="muted">Stand: 2026-06. RDOC SquadLink Lite ist auf Datensparsamkeit ausgelegt.</p>
<h2>Was NICHT passiert</h2>
<ul>
<li><b>Keine Audio-/Chat-Aufzeichnung.</b> Sprache und Text laufen direkt Peer-to-Peer
(DTLS-SRTP bzw. verschlüsselter DataChannel) und werden nirgends gespeichert.</li>
<li><b>Keine Benutzerkonten</b>, kein Login, kein Tracking, keine Werbung, keine Cookies.</li>
<li>Der Vermittlungsserver <b>sieht den Medieninhalt nicht</b> — Stimme/Chat fließen nie über ihn.</li>
</ul>
<h2>Was verarbeitet wird</h2>
<ul>
<li><b>Signaling</b> (Vermittlung): Beim Verbinden tauschen die Apps über den Server Verbindungsdaten
aus (SDP/ICE-Kandidaten, gewählter Anzeigename, Raum-/Session-Zuordnung). Diese Daten liegen nur
<b>flüchtig im Arbeitsspeicher</b> und werden gelöscht, sobald der Raum leer ist (spätestens nach 24&nbsp;h).</li>
<li><b>Session-Vermittlung</b>: Ein zufälliger Code + 6-stellige PIN werden temporär im Speicher
gehalten (max. 24&nbsp;h), um Mitspielern den konfigurationslosen Beitritt zu ermöglichen.</li>
<li><b>Verbindungs-Metadaten</b>: Wie bei jedem Internetdienst sind dem Server beim Verbinden die
IP-Adressen technisch bekannt; sie werden nicht dauerhaft protokolliert.</li>
<li><b>TURN-Relay (nur Fallback)</b>: Falls keine direkte Verbindung möglich ist, kann verschlüsselter
Audioverkehr über einen Relay laufen. Der Relay leitet nur <b>verschlüsselte Bytes</b> weiter und
kann den Inhalt nicht entschlüsseln.</li>
</ul>
<h2>Drittanbieter</h2>
<p>Die App-Installer werden über GitHub Releases bereitgestellt; beim Download gelten die
Datenschutzbestimmungen von GitHub. STUN/TURN kann öffentliche STUN-Server (z. B. von Cloudflare)
zur NAT-Erkennung nutzen.</p>
<h2>Kontakt</h2>
<p>Verantwortlich: siehe <a href="/legal">Impressum</a>. Anfragen über die Kontaktwege auf
<a href="https://raumdock.org">raumdock.org</a>.</p>"#,
    )
}

async fn legal() -> Html<String> {
    shell(
        "Impressum",
        r#"<h1>Impressum / Rechtliches</h1>
<p>RDOC SquadLink Lite ist ein nicht-kommerzielles Community-Projekt
(<a href="https://raumdock.org">raumdock.org</a>).</p>
<h2>Autoren</h2>
<p>head87x &amp; justcallmedeimos</p>
<h2>Anbieter</h2>
<p class="muted">
<!-- TODO: vollständige Anbieterkennzeichnung (Name, Anschrift, Kontakt) gemäß §5 DDG eintragen -->
Verantwortlicher Betreiber: raumdock.org<br>
Kontakt: über <a href="https://raumdock.org">raumdock.org</a>
</p>
<h2>Haftung</h2>
<p>Die Software wird „wie besehen", ohne Gewähr und ohne Haftung bereitgestellt (siehe
<a href="/license">Lizenz</a>). Für Inhalte verlinkter externer Seiten sind deren Betreiber verantwortlich.</p>"#,
    )
}

async fn license_page() -> Html<String> {
    shell(
        "Lizenz",
        &format!(
            r#"<h1>Lizenz — nicht-kommerziell</h1>
<p>RDOC SquadLink Lite steht unter der <b>PolyForm Noncommercial License 1.0.0</b>.</p>
<h2>Kurz gesagt</h2>
<ul>
<li>Nutzen, kopieren, ändern, weitergeben — für jeden nicht-kommerziellen Zweck
(privat, Community, Bildung, Forschung).</li>
<li>Keine kommerzielle Nutzung ohne gesonderte Lizenz.</li>
<li>Lizenz- und Urhebervermerke beibehalten.</li>
<li>Ohne Gewähr / ohne Haftung.</li>
</ul>
<h2>Kommerzielle Nutzung</h2>
<p>Kommerzielle Nutzung erfordert eine separate kommerzielle Lizenz. Dazu zählen u.&nbsp;a.
Verkauf, Unterlizenzierung, Betrieb als bezahlter Dienst, Integration in kommerzielle Produkte
oder Nutzung in umsatzgenerierenden Aktivitäten.</p>
<p>Anfragen: <a href="mailto:commercialusage@raumdock.org">commercialusage@raumdock.org</a></p>
<p>Dies ist eine Zusammenfassung — verbindlich ist der vollständige Lizenztext:</p>
<p><a class="dl" href="{GITHUB_URL}/blob/main/LICENSE">Vollständige Lizenz (LICENSE) ansehen</a></p>
<p class="muted">© head87x &amp; justcallmedeimos. PolyForm Noncommercial License 1.0.0 — siehe polyformproject.org.</p>"#
        ),
    )
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sink, mut stream) = socket.split();
    // Bounded: a slow/stuck peer applies backpressure instead of growing memory.
    let (tx, mut rx) = mpsc::channel::<ServerMsg>(PEER_CHAN);

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
                let _ = tx.try_send(ServerMsg::Error { code: "bad_json".into(), message: e.to_string() });
                continue;
            }
        };

        match cmsg {
            ClientMsg::Join { room, user_id, name, token } => {
                if me.is_some() {
                    let _ = tx.try_send(ServerMsg::Error {
                        code: "already_joined".into(),
                        message: "this socket already joined a room".into(),
                    });
                    continue;
                }
                // Bound all client-supplied fields before doing anything with them.
                if !len_ok(&room, MAX_ID)
                    || !len_ok(&user_id, MAX_ID)
                    || !len_ok(&name, MAX_NAME)
                    || token.as_deref().map(|t| t.len() > MAX_TOKEN).unwrap_or(false)
                {
                    let _ = tx.try_send(ServerMsg::Error {
                        code: "bad_input".into(),
                        message: "field empty or too long".into(),
                    });
                    break;
                }
                if !state.auth.check(&room, token.as_deref()) {
                    let _ = tx.try_send(ServerMsg::Error {
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
                        let _ = tx.try_send(ServerMsg::RoomFull { cap: HARD_CAP });
                        drop(rooms);
                        break;
                    }
                    // Supersede a stale connection with the same user_id.
                    if let Some(old) = r.remove(&user_id) {
                        let _ = old.tx.try_send(ServerMsg::Error {
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
                            let _ = h.tx.try_send(ServerMsg::PeerJoined {
                                user_id: user_id.clone(),
                                name: name.clone(),
                            });
                        }
                    }
                    let size = r.len();
                    // Soft-cap warning to everyone in the room.
                    if size >= WARN_CAP {
                        for h in r.values() {
                            let _ = h.tx.try_send(ServerMsg::Warn { size, cap: WARN_CAP });
                        }
                    }
                    (roster, size)
                };

                let _ = tx.try_send(ServerMsg::Roster { peers: roster });
                if let Some(turn) = &state.turn {
                    let _ = tx.try_send(ServerMsg::Turn(turn.mint(&user_id)));
                }
                tracing::info!(%room, %user_id, size, "join");
                me = Some((room, user_id));
            }

            ClientMsg::Offer { to, sdp } => {
                if !len_ok(&to, MAX_ID) || sdp.len() > MAX_SDP {
                    continue;
                }
                relay_to(&state, &me, &to, |from| ServerMsg::Offer { from, sdp });
            }
            ClientMsg::Answer { to, sdp } => {
                if !len_ok(&to, MAX_ID) || sdp.len() > MAX_SDP {
                    continue;
                }
                relay_to(&state, &me, &to, |from| ServerMsg::Answer { from, sdp });
            }
            ClientMsg::Ice { to, candidate } => {
                if !len_ok(&to, MAX_ID) || candidate.len() > MAX_ICE {
                    continue;
                }
                relay_to(&state, &me, &to, |from| ServerMsg::Ice { from, candidate });
            }
            ClientMsg::Ptt { active } => {
                if let Some((room, from)) = &me {
                    let rooms = state.rooms.lock().unwrap();
                    if let Some(r) = rooms.get(room) {
                        for (id, h) in r.iter() {
                            if id != from {
                                let _ = h.tx.try_send(ServerMsg::Ptt {
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
                let _ = h.tx.try_send(ServerMsg::PeerLeft { user_id: user_id.clone() });
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
            let _ = h.tx.try_send(build(from.clone()));
        }
    }
}
