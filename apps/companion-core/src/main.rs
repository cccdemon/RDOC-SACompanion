//! companion-core — headless mesh voice + chat client (Phase 1 / 1b).
//!
//! Env: SERVER (ws/wss url, default ws://127.0.0.1:8080/ws), ROOM, USER_ID,
//!      NAME, TOKEN (room-auth, optional), IN_DEVICE/OUT_DEVICE (substring).
//!
//! Console: a line "/t" toggles transmit (PTT); any other line is broadcast as
//! chat to all peers. Incoming chat prints as "[chat <peer>] …".

mod audio;
mod mesh;
mod signaling;

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use protocol::{ClientMsg, ServerMsg};
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

enum Cmd {
    ToggleTx,
    Chat(String),
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn build_api() -> Result<API> {
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut m)?;
    Ok(APIBuilder::new().with_media_engine(m).with_interceptor_registry(registry).build())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Pin the rustls crypto provider (ring) process-wide so webrtc-rs DTLS and
    // our wss client don't trip the "ambiguous provider" panic when multiple
    // rustls backends are in the dep graph.
    let _ = rustls::crypto::ring::default_provider().install_default();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "companion_core=info".into()),
        )
        .init();

    let server = env_or("SERVER", "ws://127.0.0.1:8080/ws");
    let room = env_or("ROOM", "testroom");
    let user = env_or("USER_ID", "user");
    let name = env_or("NAME", &user);
    let token = std::env::var("TOKEN").ok();

    // Shared audio rings + mix map + PTT flag.
    let cap: Buf = Arc::new(Mutex::new(VecDeque::new()));
    let play: Buf = Arc::new(Mutex::new(VecDeque::new()));
    let mix: MixMap = Arc::new(Mutex::new(HashMap::new()));
    let transmit = Arc::new(AtomicBool::new(false));

    // Device thread (owns cpal streams), then learn the device rates.
    let (rate_tx, rate_rx) = std::sync::mpsc::channel::<(u32, u32)>();
    {
        let (cap, play) = (cap.clone(), play.clone());
        std::thread::spawn(move || audio::run_devices(cap, play, rate_tx));
    }
    let (in_rate, out_rate) = rate_rx.recv()?;

    // Audio worker threads (audiopus off the async runtime).
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

    // WebRTC API + the single shared local audio track.
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
                let sample = Sample { data: b, duration: Duration::from_millis(20), ..Default::default() };
                let _ = local.write_sample(&sample).await;
            }
        });
    }

    // Signaling + join.
    let sig = signaling::connect(&server).await?;
    let out = sig.out.clone();
    let mut incoming = sig.incoming;
    out.send(ClientMsg::Join {
        room: room.clone(),
        user_id: user.clone(),
        name: name.clone(),
        token,
    })?;

    // Console input → commands. Keep a sender clone so the channel stays open
    // even if stdin hits EOF (piped / no tty) — the app must not exit then.
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<Cmd>();
    let _cmd_keep = cmd_tx.clone();
    std::thread::spawn(move || {
        let mut line = String::new();
        loop {
            line.clear();
            if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let t = line.trim();
            if t == "/t" {
                let _ = cmd_tx.send(Cmd::ToggleTx);
            } else if !t.is_empty() {
                let _ = cmd_tx.send(Cmd::Chat(t.to_string()));
            }
        }
    });

    println!("== RDOC-SACompanion :: room '{room}' as '{user}' ==");
    println!("   '/t' + ENTER = Senden an/aus (PTT) · sonst = Chat an alle · Strg+C beendet");

    let mut mesh = Mesh::new(api, local, user.clone(), out.clone(), decode_tx);

    loop {
        tokio::select! {
            msg = incoming.recv() => {
                let Some(msg) = msg else { eprintln!("[signaling closed]"); break; };
                match msg {
                    ServerMsg::Roster { peers } => {
                        let names: Vec<String> = peers.iter().map(|p| p.name.clone()).collect();
                        println!("[roster] {} Teilnehmer: {:?}", peers.len(), names);
                        for p in peers { let _ = mesh.on_peer(&p.user_id).await; }
                    }
                    ServerMsg::PeerJoined { user_id, name } => {
                        println!("[+ {name} ({user_id})]");
                        let _ = mesh.on_peer(&user_id).await;
                    }
                    ServerMsg::PeerLeft { user_id } => {
                        println!("[- {user_id}]");
                        mesh.on_left(&user_id).await;
                    }
                    ServerMsg::Offer { from, sdp } => { let _ = mesh.on_offer(&from, sdp).await; }
                    ServerMsg::Answer { from, sdp } => { let _ = mesh.on_answer(&from, sdp).await; }
                    ServerMsg::Ice { from, candidate } => { mesh.on_ice(&from, candidate).await; }
                    ServerMsg::Ptt { user_id, active } => {
                        if active { println!("[🎙 {user_id} spricht]"); }
                    }
                    ServerMsg::Turn(t) => {
                        println!("[turn creds erhalten]");
                        mesh.add_turn(t.urls, t.username, t.credential);
                    }
                    ServerMsg::Warn { size, cap } => {
                        println!("[WARN] Room {size}/{cap} — Audioqualität kann leiden");
                    }
                    ServerMsg::RoomFull { cap } => { eprintln!("[Room voll @ {cap}]"); break; }
                    ServerMsg::Error { code, message } => { eprintln!("[server error {code}] {message}"); }
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(Cmd::ToggleTx) => {
                        let n = !transmit.load(Ordering::SeqCst);
                        transmit.store(n, Ordering::SeqCst);
                        let _ = out.send(ClientMsg::Ptt { active: n });
                        println!("[Senden {}]", if n { "AN" } else { "AUS" });
                    }
                    Some(Cmd::Chat(t)) => {
                        mesh.broadcast_chat(&t).await;
                        println!("[ich] {t}");
                    }
                    None => break,
                }
            }
        }
    }

    Ok(())
}
