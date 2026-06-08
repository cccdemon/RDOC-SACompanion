//! companion-core engine: runs the encrypted P2P mesh (audio + chat) and
//! reports state to any frontend via a `Sink` callback. The headless bin and
//! the Tauri app both drive this same engine.

pub mod audio;
pub mod mesh;
pub mod serverless;
pub mod signaling;

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use protocol::{ClientMsg, ServerMsg};
use serde::Serialize;
use tokio::sync::mpsc;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_OPUS};
use webrtc::api::{APIBuilder, API};
use webrtc::interceptor::registry::Registry;
use webrtc::media::Sample;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

use audio::{Buf, MixMap};
use mesh::Mesh;

/// One room member as shown in the UI.
#[derive(Debug, Clone, Serialize)]
pub struct Participant {
    pub user_id: String,
    pub name: String,
    pub you: bool,
    /// "DIREKT" | "RELAY (TURN)" once the link is up; None while connecting.
    pub badge: Option<String>,
    pub speaking: bool,
}

/// Events the engine pushes to the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiEvent {
    Roster { participants: Vec<Participant> },
    Chat { from: String, text: String },
    Status { connected: bool, transmitting: bool },
    Log { text: String },
    /// Live network: connected peer count + measured up/down kbps.
    Net { peers: usize, up_kbps: u32, down_kbps: u32 },
    /// Encryption keys rotated: fresh DTLS-SRTP keys negotiated across the mesh.
    Rekeyed { generation: u32, by: String },
}

pub type Sink = Arc<dyn Fn(UiEvent) + Send + Sync>;

/// Internal events from the mesh layer back to the engine loop.
pub(crate) enum MeshEvent {
    Chat { from: String, text: String },
    Badge { peer: String, badge: String },
}

pub struct EngineConfig {
    pub server: String,
    pub room: String,
    pub user_id: String,
    pub name: String,
    pub token: Option<String>,
    pub cert_sha256: Option<String>,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
}

enum Cmd {
    ToggleTx,
    SetTx(bool),
    Chat(String),
    Rekey,
}

/// Handle to the running engine; methods are non-blocking.
pub struct Engine {
    cmd_tx: mpsc::UnboundedSender<Cmd>,
    gains: Arc<audio::Gains>,
}
impl Engine {
    pub fn toggle_transmit(&self) {
        let _ = self.cmd_tx.send(Cmd::ToggleTx);
    }
    /// Hold-to-talk: set transmit explicitly (idempotent).
    pub fn set_transmit(&self, on: bool) {
        let _ = self.cmd_tx.send(Cmd::SetTx(on));
    }
    pub fn send_chat(&self, text: String) {
        let _ = self.cmd_tx.send(Cmd::Chat(text));
    }
    /// Rotate the session encryption keys: triggers a room-wide DTLS-SRTP
    /// re-handshake so all links get fresh keys.
    pub fn rotate_key(&self) {
        let _ = self.cmd_tx.send(Cmd::Rekey);
    }
    /// Overall output volume (0.0 mute … 1.0 normal … 2.0 +6 dB). Live.
    pub fn set_master_volume(&self, v: f32) {
        self.gains.set_master(v);
    }
    /// Per-participant output volume (by user_id). Live.
    pub fn set_peer_volume(&self, user_id: &str, v: f32) {
        self.gains.set_peer(user_id, v);
    }
}

pub(crate) fn build_api() -> Result<API> {
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut m)?;
    Ok(APIBuilder::new().with_media_engine(m).with_interceptor_registry(registry).build())
}

/// Spin up the audio rig (device + encode/decode/mix threads) shared by both
/// the server-signaled mesh and the serverless 1:1 mode. Returns the transmit
/// flag, the encoded-Opus receiver (feed to the local track writer), and the
/// decode sender (feed remote RTP payloads in).
pub(crate) fn setup_audio(
    in_name: Option<String>,
    out_name: Option<String>,
) -> Result<(
    Arc<AtomicBool>,
    mpsc::UnboundedReceiver<Bytes>,
    mpsc::UnboundedSender<(String, Bytes)>,
    Arc<audio::Gains>,
)> {
    let cap: Buf = Arc::new(Mutex::new(VecDeque::new()));
    let play: Buf = Arc::new(Mutex::new(VecDeque::new()));
    let mix: MixMap = Arc::new(Mutex::new(HashMap::new()));
    let transmit = Arc::new(AtomicBool::new(false));
    let gains = Arc::new(audio::Gains::new());

    let (rate_tx, rate_rx) = std::sync::mpsc::channel::<(u32, u32)>();
    {
        let (cap, play) = (cap.clone(), play.clone());
        std::thread::spawn(move || audio::run_devices(cap, play, rate_tx, in_name, out_name));
    }
    let (in_rate, out_rate) = rate_rx.recv()?;

    let (opus_tx, opus_rx) = mpsc::unbounded_channel::<Bytes>();
    let (decode_tx, decode_rx) = mpsc::unbounded_channel::<(String, Bytes)>();
    {
        let (cap, transmit) = (cap.clone(), transmit.clone());
        std::thread::spawn(move || audio::encode_loop(cap, in_rate, transmit, opus_tx));
    }
    {
        let mix = mix.clone();
        std::thread::spawn(move || audio::decode_loop(decode_rx, mix));
    }
    {
        let gains = gains.clone();
        std::thread::spawn(move || audio::mixer_loop(mix, play, out_rate, gains));
    }

    Ok((transmit, opus_rx, decode_tx, gains))
}

struct Member {
    name: String,
    badge: Option<String>,
    speaking: bool,
}

fn emit_roster(
    sink: &Sink,
    members: &HashMap<String, Member>,
    me_id: &str,
    me_name: &str,
    transmitting: bool,
) {
    let mut participants = vec![Participant {
        user_id: me_id.to_string(),
        name: me_name.to_string(),
        you: true,
        badge: None,
        speaking: transmitting,
    }];
    let mut others: Vec<Participant> = members
        .iter()
        .map(|(id, m)| Participant {
            user_id: id.clone(),
            name: m.name.clone(),
            you: false,
            badge: m.badge.clone(),
            speaking: m.speaking,
        })
        .collect();
    others.sort_by(|a, b| a.name.cmp(&b.name));
    participants.extend(others);
    sink(UiEvent::Roster { participants });
}

/// Start the engine: open audio, connect signaling, join, and run the mesh.
/// Returns a handle; state flows out through `sink`.
pub async fn start(cfg: EngineConfig, sink: Sink) -> Result<Engine> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let (transmit, mut opus_rx, decode_tx, gains) =
        setup_audio(cfg.input_device.clone(), cfg.output_device.clone())?;

    let api = Arc::new(build_api()?);
    let local = Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_OPUS.to_owned(),
            clock_rate: 48000,
            channels: 2,
            ..Default::default()
        },
        "audio".to_owned(),
        "rdoc-squadlink-lite".to_owned(),
    ));
    {
        let local = local.clone();
        tokio::spawn(async move {
            while let Some(b) = opus_rx.recv().await {
                let sample =
                    Sample { data: b, duration: Duration::from_millis(20), ..Default::default() };
                let _ = local.write_sample(&sample).await;
            }
        });
    }

    let sig = signaling::connect(&cfg.server, cfg.cert_sha256.as_deref()).await?;
    let out = sig.out.clone();
    let mut incoming = sig.incoming;
    out.send(ClientMsg::Join {
        room: cfg.room.clone(),
        user_id: cfg.user_id.clone(),
        name: cfg.name.clone(),
        token: cfg.token.clone(),
    })?;
    sink(UiEvent::Status { connected: true, transmitting: false });

    let (mesh_tx, mut mesh_rx) = mpsc::unbounded_channel::<MeshEvent>();
    let mut mesh = Mesh::new(api, local, cfg.user_id.clone(), out.clone(), decode_tx, mesh_tx);

    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<Cmd>();

    let me_id = cfg.user_id.clone();
    let me_name = cfg.name.clone();
    tokio::spawn(async move {
        let mut members: HashMap<String, Member> = HashMap::new();
        let mut key_gen: u32 = 1; // generation #1 = the initial DTLS-SRTP keys
        let mut net_iv = tokio::time::interval(Duration::from_secs(2));
        let mut last_bytes = (0u64, 0u64);
        let mut last_inst = std::time::Instant::now();
        let mut net_primed = false;
        loop {
            tokio::select! {
                _ = net_iv.tick() => {
                    let (up, down) = mesh.stats_bytes().await;
                    if !net_primed {
                        last_bytes = (up, down);
                        last_inst = std::time::Instant::now();
                        net_primed = true;
                    } else {
                        let dt = last_inst.elapsed().as_secs_f64().max(0.001);
                        let up_kbps = ((up.saturating_sub(last_bytes.0)) as f64 * 8.0 / 1000.0 / dt) as u32;
                        let down_kbps = ((down.saturating_sub(last_bytes.1)) as f64 * 8.0 / 1000.0 / dt) as u32;
                        last_bytes = (up, down);
                        last_inst = std::time::Instant::now();
                        sink(UiEvent::Net { peers: members.len(), up_kbps, down_kbps });
                    }
                }
                msg = incoming.recv() => {
                    let Some(msg) = msg else {
                        sink(UiEvent::Status { connected: false, transmitting: transmit.load(Ordering::SeqCst) });
                        break;
                    };
                    match msg {
                        ServerMsg::Roster { peers } => {
                            members = peers.iter().map(|p| (p.user_id.clone(), Member { name: p.name.clone(), badge: None, speaking: false })).collect();
                            for p in &peers { let _ = mesh.on_peer(&p.user_id).await; }
                            emit_roster(&sink, &members, &me_id, &me_name, transmit.load(Ordering::SeqCst));
                        }
                        ServerMsg::PeerJoined { user_id, name } => {
                            members.insert(user_id.clone(), Member { name, badge: None, speaking: false });
                            let _ = mesh.on_peer(&user_id).await;
                            emit_roster(&sink, &members, &me_id, &me_name, transmit.load(Ordering::SeqCst));
                        }
                        ServerMsg::PeerLeft { user_id } => {
                            members.remove(&user_id);
                            mesh.on_left(&user_id).await;
                            emit_roster(&sink, &members, &me_id, &me_name, transmit.load(Ordering::SeqCst));
                        }
                        ServerMsg::Offer { from, sdp } => { let _ = mesh.on_offer(&from, sdp).await; }
                        ServerMsg::Answer { from, sdp } => { let _ = mesh.on_answer(&from, sdp).await; }
                        ServerMsg::Ice { from, candidate } => { mesh.on_ice(&from, candidate).await; }
                        ServerMsg::Ptt { user_id, active } => {
                            if let Some(m) = members.get_mut(&user_id) { m.speaking = active; }
                            emit_roster(&sink, &members, &me_id, &me_name, transmit.load(Ordering::SeqCst));
                        }
                        ServerMsg::Rekey { by } => {
                            let _ = mesh.rekey().await;
                            key_gen = key_gen.saturating_add(1);
                            sink(UiEvent::Rekeyed { generation: key_gen, by });
                        }
                        ServerMsg::Turn(t) => { mesh.add_turn(t.urls, t.username, t.credential); }
                        ServerMsg::Warn { size, cap } => { sink(UiEvent::Log { text: format!("Room {size}/{cap} — Audioqualität kann leiden") }); }
                        ServerMsg::RoomFull { cap } => { sink(UiEvent::Log { text: format!("Room voll @ {cap}") }); break; }
                        ServerMsg::Error { code, message } => { sink(UiEvent::Log { text: format!("{code}: {message}") }); }
                    }
                }
                ev = mesh_rx.recv() => {
                    match ev {
                        Some(MeshEvent::Chat { from, text }) => {
                            // `from` is the peer's user_id → show their display name.
                            let name = members.get(&from).map(|m| m.name.clone()).unwrap_or(from);
                            sink(UiEvent::Chat { from: name, text });
                        }
                        Some(MeshEvent::Badge { peer, badge }) => {
                            if let Some(m) = members.get_mut(&peer) { m.badge = Some(badge); }
                            emit_roster(&sink, &members, &me_id, &me_name, transmit.load(Ordering::SeqCst));
                        }
                        None => {}
                    }
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(Cmd::ToggleTx) => {
                            let n = !transmit.load(Ordering::SeqCst);
                            transmit.store(n, Ordering::SeqCst);
                            let _ = out.send(ClientMsg::Ptt { active: n });
                            sink(UiEvent::Status { connected: true, transmitting: n });
                            emit_roster(&sink, &members, &me_id, &me_name, n);
                        }
                        Some(Cmd::SetTx(on)) => {
                            if transmit.load(Ordering::SeqCst) != on {
                                transmit.store(on, Ordering::SeqCst);
                                let _ = out.send(ClientMsg::Ptt { active: on });
                                sink(UiEvent::Status { connected: true, transmitting: on });
                                emit_roster(&sink, &members, &me_id, &me_name, on);
                            }
                        }
                        Some(Cmd::Chat(t)) => {
                            mesh.broadcast_chat(&t).await;
                            sink(UiEvent::Chat { from: me_name.clone(), text: t });
                        }
                        Some(Cmd::Rekey) => {
                            // Broadcast → everyone (incl. us) rekeys on the echoed Rekey.
                            let _ = out.send(ClientMsg::Rekey);
                        }
                        None => break,
                    }
                }
            }
        }
    });

    Ok(Engine { cmd_tx, gains })
}
