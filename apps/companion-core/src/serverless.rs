//! Serverless 1:1 mode — NO InitConnection server. Two peers exchange the SDP
//! offer/answer as base64 copy-paste codes (out-of-band: Discord, etc.). Uses
//! public STUN for NAT; no TURN (no creds server) → hard-NAT pairs can't relay.
//! Encryption is unchanged: DTLS-SRTP audio + DTLS-SCTP chat.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use bytes::Bytes;
use protocol::ChatMsg;
use tokio::sync::mpsc::UnboundedSender;
use webrtc::api::media_engine::MIME_TYPE_OPUS;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::media::Sample;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_remote::TrackRemote;

use crate::{build_api, setup_audio, Participant, Sink, UiEvent};

type ChatSlot = Arc<Mutex<Option<Arc<RTCDataChannel>>>>;

fn wire_chat(slot: ChatSlot, dc: Arc<RTCDataChannel>, sink: Sink) {
    dc.on_message(Box::new(move |msg: DataChannelMessage| {
        let sink = sink.clone();
        Box::pin(async move {
            if let Ok(c) = serde_json::from_slice::<ChatMsg>(&msg.data) {
                sink(UiEvent::Chat { from: "Peer".to_string(), text: c.text });
            }
        })
    }));
    *slot.lock().unwrap() = Some(dc);
}

async fn read_track(track: Arc<TrackRemote>, dtx: UnboundedSender<(String, Bytes)>) {
    loop {
        match track.read_rtp().await {
            Ok((p, _)) => {
                if !p.payload.is_empty() {
                    let _ = dtx.send(("peer".to_string(), p.payload));
                }
            }
            Err(_) => break,
        }
    }
}

async fn badge_of(pc: &RTCPeerConnection) -> String {
    let sctp = pc.sctp();
    let dtls = sctp.transport();
    let ice = dtls.ice_transport();
    for _ in 0..10 {
        if let Some(pair) = ice.get_selected_candidate_pair().await {
            return if format!("{pair}").contains("relay") {
                "RELAY (TURN)".to_string()
            } else {
                "DIREKT".to_string()
            };
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    "DIREKT".to_string()
}

fn decode_code(code: &str) -> Result<String> {
    let bytes = STANDARD.decode(code.trim()).map_err(|_| anyhow!("ungültiger Code (kein base64)"))?;
    String::from_utf8(bytes).map_err(|_| anyhow!("ungültiger Code (kein UTF-8)"))
}

/// Build a single PeerConnection wired to the audio rig + chat slot + UI sink.
async fn build_pc(sink: Sink, me: String) -> Result<(Arc<RTCPeerConnection>, Arc<AtomicBool>, ChatSlot)> {
    let (transmit, mut opus_rx, decode_tx, _gains) = setup_audio(None, None)?;
    let api = build_api()?;
    let local = Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_OPUS.to_owned(),
            clock_rate: 48000,
            channels: 2,
            ..Default::default()
        },
        "audio".to_owned(),
        "rdoc-sa".to_owned(),
    ));
    {
        let local = local.clone();
        tokio::spawn(async move {
            while let Some(b) = opus_rx.recv().await {
                let s = Sample { data: b, duration: Duration::from_millis(20), ..Default::default() };
                let _ = local.write_sample(&s).await;
            }
        });
    }

    let cfg = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_owned()],
            ..Default::default()
        }],
        ..Default::default()
    };
    let pc = Arc::new(api.new_peer_connection(cfg).await?);

    let sender = pc.add_track(Arc::clone(&local) as Arc<dyn TrackLocal + Send + Sync>).await?;
    tokio::spawn(async move {
        let mut b = vec![0u8; 1500];
        while sender.read(&mut b).await.is_ok() {}
    });

    let dtx = decode_tx.clone();
    pc.on_track(Box::new(move |track: Arc<TrackRemote>, _, _| {
        let dtx = dtx.clone();
        Box::pin(async move {
            tokio::spawn(read_track(track, dtx));
        })
    }));

    let chat: ChatSlot = Arc::new(Mutex::new(None));
    {
        let chat = chat.clone();
        let sink = sink.clone();
        pc.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
            let chat = chat.clone();
            let sink = sink.clone();
            Box::pin(async move {
                wire_chat(chat, dc, sink);
            })
        }));
    }

    let pc_state = pc.clone();
    let sink2 = sink.clone();
    let me2 = me.clone();
    pc.on_peer_connection_state_change(Box::new(move |s| {
        let pc_state = pc_state.clone();
        let sink2 = sink2.clone();
        let me2 = me2.clone();
        Box::pin(async move {
            match s {
                RTCPeerConnectionState::Connected => {
                    let badge = badge_of(&pc_state).await;
                    sink2(UiEvent::Status { connected: true, transmitting: false });
                    sink2(UiEvent::Roster {
                        participants: vec![
                            Participant {
                                user_id: "me".into(),
                                name: me2.clone(),
                                you: true,
                                badge: None,
                                speaking: false,
                            },
                            Participant {
                                user_id: "peer".into(),
                                name: "Peer".into(),
                                you: false,
                                badge: Some(badge),
                                speaking: false,
                            },
                        ],
                    });
                }
                RTCPeerConnectionState::Failed | RTCPeerConnectionState::Disconnected => {
                    sink2(UiEvent::Status { connected: false, transmitting: false });
                }
                _ => {}
            }
        })
    }));

    Ok((pc, transmit, chat))
}

pub struct Serverless {
    pc: Arc<RTCPeerConnection>,
    transmit: Arc<AtomicBool>,
    chat: ChatSlot,
    sink: Sink,
    me: String,
}

impl Serverless {
    /// Role A: create an offer, return the base64 code to hand to the peer.
    pub async fn create_offer(sink: Sink, me: String) -> Result<(Serverless, String)> {
        let (pc, transmit, chat) = build_pc(sink.clone(), me.clone()).await?;
        let dc = pc.create_data_channel("chat", None).await?;
        wire_chat(chat.clone(), dc, sink.clone());

        let offer = pc.create_offer(None).await?;
        let mut gather = pc.gathering_complete_promise().await;
        pc.set_local_description(offer).await?;
        let _ = gather.recv().await; // non-trickle: full SDP with candidates
        let sdp = pc.local_description().await.ok_or_else(|| anyhow!("no local description"))?.sdp;
        Ok((Serverless { pc, transmit, chat, sink, me }, STANDARD.encode(sdp)))
    }

    /// Role B: accept the peer's offer code, return our answer code.
    pub async fn accept_offer(code: String, sink: Sink, me: String) -> Result<(Serverless, String)> {
        let (pc, transmit, chat) = build_pc(sink.clone(), me.clone()).await?;
        pc.set_remote_description(RTCSessionDescription::offer(decode_code(&code)?)?).await?;
        let answer = pc.create_answer(None).await?;
        let mut gather = pc.gathering_complete_promise().await;
        pc.set_local_description(answer).await?;
        let _ = gather.recv().await;
        let sdp = pc.local_description().await.ok_or_else(|| anyhow!("no local description"))?.sdp;
        Ok((Serverless { pc, transmit, chat, sink, me }, STANDARD.encode(sdp)))
    }

    /// Role A: finish the handshake with the peer's answer code.
    pub async fn accept_answer(&self, code: String) -> Result<()> {
        self.pc
            .set_remote_description(RTCSessionDescription::answer(decode_code(&code)?)?)
            .await?;
        Ok(())
    }

    fn set(&self, on: bool) {
        self.transmit.store(on, Ordering::SeqCst);
        (self.sink)(UiEvent::Status { connected: true, transmitting: on });
    }
    pub fn toggle_transmit(&self) {
        let n = !self.transmit.load(Ordering::SeqCst);
        self.set(n);
    }
    pub fn set_transmit(&self, on: bool) {
        if self.transmit.load(Ordering::SeqCst) != on {
            self.set(on);
        }
    }
    pub fn send_chat(&self, text: String) {
        let dc = self.chat.lock().unwrap().clone();
        let me = self.me.clone();
        let sink = self.sink.clone();
        tokio::spawn(async move {
            let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            if let Some(dc) = dc {
                if let Ok(j) = serde_json::to_string(&ChatMsg { text: text.clone(), ts }) {
                    let _ = dc.send_text(j).await;
                }
            }
            sink(UiEvent::Chat { from: me, text });
        });
    }
}
