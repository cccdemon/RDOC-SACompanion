//! Mesh: one PeerConnection per remote peer. A single shared local audio track
//! (StaticSample) is added to every PC — encode-once, the lib does per-peer
//! SRTP (proven in spikes/track-fanout). Each pair also gets a DataChannel for
//! encrypted text chat. Glare: the lexicographically smaller user_id offers.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use bytes::Bytes;
use protocol::{ChatMsg, ClientMsg};
use tokio::sync::mpsc::UnboundedSender;

use crate::MeshEvent;
use webrtc::api::API;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_remote::TrackRemote;

pub type DecodeTx = UnboundedSender<(String, Bytes)>;
type ChatSlot = Arc<Mutex<Option<Arc<RTCDataChannel>>>>;

struct PeerConn {
    pc: Arc<RTCPeerConnection>,
    pending_ice: Mutex<Vec<String>>,
    remote_set: AtomicBool,
    chat: ChatSlot,
}
impl PeerConn {
    async fn add_or_queue_ice(&self, cand: String) {
        if self.remote_set.load(Ordering::SeqCst) {
            if let Ok(init) = serde_json::from_str::<RTCIceCandidateInit>(&cand) {
                let _ = self.pc.add_ice_candidate(init).await;
            }
        } else {
            self.pending_ice.lock().unwrap().push(cand);
        }
    }
    async fn flush_ice(&self) {
        self.remote_set.store(true, Ordering::SeqCst);
        let pending = std::mem::take(&mut *self.pending_ice.lock().unwrap());
        for c in pending {
            if let Ok(init) = serde_json::from_str::<RTCIceCandidateInit>(&c) {
                let _ = self.pc.add_ice_candidate(init).await;
            }
        }
    }
}

/// Forward incoming chat (sender = the channel's peer) to the engine and stash
/// the channel.
fn wire_chat(peer: String, slot: ChatSlot, dc: Arc<RTCDataChannel>, events: UnboundedSender<MeshEvent>) {
    let p = peer.clone();
    dc.on_message(Box::new(move |msg: DataChannelMessage| {
        let p = p.clone();
        let events = events.clone();
        Box::pin(async move {
            if let Ok(c) = serde_json::from_slice::<ChatMsg>(&msg.data) {
                let _ = events.send(MeshEvent::Chat { from: p, text: c.text });
            }
        })
    }));
    *slot.lock().unwrap() = Some(dc);
}

pub struct Mesh {
    api: Arc<API>,
    local: Arc<TrackLocalStaticSample>,
    my_id: String,
    out: UnboundedSender<ClientMsg>,
    decode_tx: DecodeTx,
    ice_servers: Vec<RTCIceServer>,
    peers: HashMap<String, PeerConn>,
    events: UnboundedSender<MeshEvent>,
}

impl Mesh {
    pub fn new(
        api: Arc<API>,
        local: Arc<TrackLocalStaticSample>,
        my_id: String,
        out: UnboundedSender<ClientMsg>,
        decode_tx: DecodeTx,
        events: UnboundedSender<MeshEvent>,
    ) -> Self {
        let ice_servers = vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_owned()],
            ..Default::default()
        }];
        Self { api, local, my_id, out, decode_tx, ice_servers, peers: HashMap::new(), events }
    }

    pub fn add_turn(&mut self, urls: Vec<String>, username: String, credential: String) {
        self.ice_servers.push(RTCIceServer { urls, username, credential, ..Default::default() });
    }

    fn config(&self) -> RTCConfiguration {
        RTCConfiguration { ice_servers: self.ice_servers.clone(), ..Default::default() }
    }

    async fn ensure(&mut self, peer: &str) -> Result<()> {
        if self.peers.contains_key(peer) {
            return Ok(());
        }
        let pc = Arc::new(self.api.new_peer_connection(self.config()).await?);

        // Send our audio to this peer (shared track → encode-once).
        let sender = pc.add_track(Arc::clone(&self.local) as Arc<dyn TrackLocal + Send + Sync>).await?;
        tokio::spawn(async move {
            let mut b = vec![0u8; 1500];
            while sender.read(&mut b).await.is_ok() {}
        });

        // Local ICE candidates → signaling.
        let out = self.out.clone();
        let to = peer.to_string();
        pc.on_ice_candidate(Box::new(move |c| {
            let out = out.clone();
            let to = to.clone();
            Box::pin(async move {
                if let Some(c) = c {
                    if let Ok(init) = c.to_json() {
                        if let Ok(s) = serde_json::to_string(&init) {
                            let _ = out.send(ClientMsg::Ice { to, candidate: s });
                        }
                    }
                }
            })
        }));

        // Connection-state + transparency badge (DIREKT vs RELAY/TURN).
        let pid_s = peer.to_string();
        let pc_state = Arc::clone(&pc);
        let ev_state = self.events.clone();
        pc.on_peer_connection_state_change(Box::new(move |s| {
            let pid_s = pid_s.clone();
            let pc_state = Arc::clone(&pc_state);
            let ev_state = ev_state.clone();
            Box::pin(async move {
                if s == RTCPeerConnectionState::Connected {
                    let badge = selected_kind(&pc_state).await.unwrap_or("DIREKT").to_string();
                    let _ = ev_state.send(MeshEvent::Badge { peer: pid_s, badge });
                }
            })
        }));

        // Remote audio track → decode pipeline.
        let dtx = self.decode_tx.clone();
        let pid = peer.to_string();
        pc.on_track(Box::new(move |track: Arc<TrackRemote>, _, _| {
            let dtx = dtx.clone();
            let pid = pid.clone();
            Box::pin(async move {
                tokio::spawn(read_track(track, pid, dtx));
            })
        }));

        // Answerer side accepts the chat DataChannel the offerer creates.
        let chat: ChatSlot = Arc::new(Mutex::new(None));
        let chat_h = chat.clone();
        let pid2 = peer.to_string();
        let ev_chat = self.events.clone();
        pc.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
            let chat_h = chat_h.clone();
            let pid2 = pid2.clone();
            let ev_chat = ev_chat.clone();
            Box::pin(async move {
                wire_chat(pid2, chat_h, dc, ev_chat);
            })
        }));

        self.peers.insert(
            peer.to_string(),
            PeerConn {
                pc,
                pending_ice: Mutex::new(Vec::new()),
                remote_set: AtomicBool::new(false),
                chat,
            },
        );
        Ok(())
    }

    /// Saw a peer (roster or peer-joined): ensure a PC; offer if we're the
    /// smaller id (glare rule).
    pub async fn on_peer(&mut self, peer: &str) -> Result<()> {
        self.ensure(peer).await?;
        if self.my_id.as_str() < peer {
            self.offer(peer).await?;
        }
        Ok(())
    }

    async fn offer(&mut self, peer: &str) -> Result<()> {
        let p = self.peers.get(peer).unwrap();
        // Offerer creates the chat channel (must exist before the offer).
        let dc = p.pc.create_data_channel("chat", None).await?;
        wire_chat(peer.to_string(), p.chat.clone(), dc, self.events.clone());

        let offer = p.pc.create_offer(None).await?;
        p.pc.set_local_description(offer.clone()).await?;
        let _ = self.out.send(ClientMsg::Offer { to: peer.to_string(), sdp: offer.sdp });
        Ok(())
    }

    pub async fn on_offer(&mut self, from: &str, sdp: String) -> Result<()> {
        self.ensure(from).await?;
        let p = self.peers.get(from).unwrap();
        p.pc.set_remote_description(RTCSessionDescription::offer(sdp)?).await?;
        p.flush_ice().await;
        let answer = p.pc.create_answer(None).await?;
        p.pc.set_local_description(answer.clone()).await?;
        let _ = self.out.send(ClientMsg::Answer { to: from.to_string(), sdp: answer.sdp });
        Ok(())
    }

    pub async fn on_answer(&mut self, from: &str, sdp: String) -> Result<()> {
        if let Some(p) = self.peers.get(from) {
            p.pc.set_remote_description(RTCSessionDescription::answer(sdp)?).await?;
            p.flush_ice().await;
        }
        Ok(())
    }

    pub async fn on_ice(&mut self, from: &str, cand: String) {
        if let Some(p) = self.peers.get(from) {
            p.add_or_queue_ice(cand).await;
        }
    }

    pub async fn on_left(&mut self, peer: &str) {
        if let Some(p) = self.peers.remove(peer) {
            let _ = p.pc.close().await;
        }
    }

    /// Sum transport bytes (sent, received) across all peer connections.
    /// Counts the real DTLS/SRTP+SCTP traffic — for live up/down bandwidth.
    pub async fn stats_bytes(&self) -> (u64, u64) {
        let mut up = 0u64;
        let mut down = 0u64;
        for p in self.peers.values() {
            let report = p.pc.get_stats().await;
            for v in report.reports.values() {
                if let webrtc::stats::StatsReportType::Transport(t) = v {
                    up += t.bytes_sent;
                    down += t.bytes_received;
                }
            }
        }
        (up, down)
    }

    /// Broadcast a chat line over every peer's DataChannel.
    pub async fn broadcast_chat(&self, text: &str) {
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let json = match serde_json::to_string(&ChatMsg { text: text.to_string(), ts }) {
            Ok(j) => j,
            Err(_) => return,
        };
        for p in self.peers.values() {
            let dc = p.chat.lock().unwrap().clone();
            if let Some(dc) = dc {
                let _ = dc.send_text(json.clone()).await;
            }
        }
    }
}

/// Classify the live connection from the selected ICE candidate pair:
/// any `relay` candidate → via TURN, else direct. Retries briefly because the
/// pair can lag the Connected state. (ARCHITECTURE §9 transparency.)
async fn selected_kind(pc: &RTCPeerConnection) -> Option<&'static str> {
    let sctp = pc.sctp();
    let dtls = sctp.transport();
    let ice = dtls.ice_transport();
    for _ in 0..10 {
        if let Some(pair) = ice.get_selected_candidate_pair().await {
            // Pair fields are private; its Display lists each candidate's type
            // ("host"/"srflx"/"prflx"/"relay"). A relay candidate ⇒ via TURN.
            let relay = format!("{pair}").contains("relay");
            return Some(if relay { "RELAY (TURN)" } else { "DIREKT" });
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    None
}

async fn read_track(track: Arc<TrackRemote>, peer: String, dtx: DecodeTx) {
    loop {
        match track.read_rtp().await {
            Ok((pkt, _)) => {
                if !pkt.payload.is_empty() {
                    let _ = dtx.send((peer.clone(), pkt.payload));
                }
            }
            Err(_) => break,
        }
    }
}
