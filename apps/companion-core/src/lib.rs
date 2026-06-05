//! companion-core engine: runs the encrypted P2P mesh (audio + chat) and
//! reports state to any frontend via a `Sink` callback. The headless bin and
//! the Tauri app both drive this same engine.

pub mod audio;
pub mod mesh;
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
}

enum Cmd {
    ToggleTx,
    Chat(String),
}

/// Handle to the running engine; methods are non-blocking.
pub struct Engine {
    cmd_tx: mpsc::UnboundedSender<Cmd>,
}
impl Engine {
    pub fn toggle_transmit(&self) {
        let _ = self.cmd_tx.send(Cmd::ToggleTx);
    }
    pub fn send_chat(&self, text: String) {
        let _ = self.cmd_tx.send(Cmd::Chat(text));
    }
}

fn build_api() -> Result<API> {
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut m)?;
    Ok(APIBuilder::new().with_media_engine(m).with_interceptor_registry(registry).build())
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

    let cap: Buf = Arc::new(Mutex::new(VecDeque::new()));
    let play: Buf = Arc::new(Mutex::new(VecDeque::new()));
    let mix: MixMap = Arc::new(Mutex::new(HashMap::new()));
    let transmit = Arc::new(AtomicBool::new(false));

    let (rate_tx, rate_rx) = std::sync::mpsc::channel::<(u32, u32)>();
    {
        let (cap, play) = (cap.clone(), play.clone());
        std::thread::spawn(move || audio::run_devices(cap, play, rate_tx));
    }
    let (in_rate, out_rate) = rate_rx.recv()?;

    let (opus_tx, mut opus_rx) = mpsc::unbounded_channel::<Bytes>();
    let (decode_tx, decode_rx) = mpsc::unbounded_channel::<(String, Bytes)>();
    {
        let (cap, transmit, opus_tx) = (cap.clone(), transmit.clone(), opus_tx.clone());
        std::thread::spawn(move || audio::encode_loop(cap, in_rate, transmit, opus_tx));
    }
    {
        let mix = mix.clone();
        std::thread::spawn(move || audio::decode_loop(decode_rx, mix));
    }
    {
        let (mix, play) = (mix.clone(), play.clone());
        std::thread::spawn(move || audio::mixer_loop(mix, play, out_rate));
    }

    let api = Arc::new(build_api()?);
    let local = Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_OPUS.to_owned(),
            clock_rate: 48000,
            channels: 2,
            ..Default::default()
        },
        "audio".to_owned(),
        "rdoc-sacompanion".to_owned(),
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
        loop {
            tokio::select! {
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
                        ServerMsg::Turn(t) => { mesh.add_turn(t.urls, t.username, t.credential); }
                        ServerMsg::Warn { size, cap } => { sink(UiEvent::Log { text: format!("Room {size}/{cap} — Audioqualität kann leiden") }); }
                        ServerMsg::RoomFull { cap } => { sink(UiEvent::Log { text: format!("Room voll @ {cap}") }); break; }
                        ServerMsg::Error { code, message } => { sink(UiEvent::Log { text: format!("{code}: {message}") }); }
                    }
                }
                ev = mesh_rx.recv() => {
                    match ev {
                        Some(MeshEvent::Chat { from, text }) => sink(UiEvent::Chat { from, text }),
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
                        Some(Cmd::Chat(t)) => {
                            mesh.broadcast_chat(&t).await;
                            sink(UiEvent::Chat { from: me_name.clone(), text: t });
                        }
                        None => break,
                    }
                }
            }
        }
    });

    Ok(Engine { cmd_tx })
}
