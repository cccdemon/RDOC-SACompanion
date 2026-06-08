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
        // A fresh offer for an already-established peer is a renegotiation /
        // rekey: drop the old PC so ensure() builds a new one with new keys.
        if let Some(p) = self.peers.get(from) {
            if p.remote_set.load(Ordering::SeqCst) {
                if let Some(old) = self.peers.remove(from) {
                    let _ = old.pc.close().await;
                }
            }
        }
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

    /// Room-wide key rotation: re-handshake every link to get fresh DTLS-SRTP
    /// keys. Glare-aware to avoid a teardown race: for each pair only the
    /// smaller user_id tears down + re-offers; the larger side keeps its old PC
    /// and lets `on_offer` replace it when the fresh offer arrives. Everyone
    /// receives the broadcast Rekey, so every pair gets exactly one re-offer.
    pub async fn rekey(&mut self) -> Result<()> {
        let ids: Vec<String> = self.peers.keys().cloned().collect();
        for id in &ids {
            if self.my_id.as_str() < id.as_str() {
                if let Some(old) = self.peers.remove(id) {
                    let _ = old.pc.close().await;
                }
                self.on_peer(id).await?; // ensure a fresh PC + re-offer
            }
            // else: answerer — on_offer() will swap in the new PC.
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn peer_pc(&self, id: &str) -> Option<Arc<RTCPeerConnection>> {
        self.peers.get(id).map(|p| p.pc.clone())
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
                    up += t.bytes_sent as u64;
                    down += t.bytes_received as u64;
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

#[cfg(test)]
mod rekey_tests {
    use super::*;
    use crate::build_api;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::sync::Mutex as AsyncMutex;
    use webrtc::api::media_engine::MIME_TYPE_OPUS;
    use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;

    fn track() -> Arc<TrackLocalStaticSample> {
        Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_OPUS.to_owned(),
                clock_rate: 48000,
                channels: 2,
                ..Default::default()
            },
            "audio".to_owned(),
            "t".to_owned(),
        ))
    }

    async fn wait_connected(m: &AsyncMutex<Mesh>, peer: &str) -> bool {
        for _ in 0..150 {
            if let Some(pc) = m.lock().await.peer_pc(peer) {
                if pc.connection_state() == RTCPeerConnectionState::Connected {
                    return true;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    // Two real meshes wired through a mock relay: connect, rotate keys, and
    // verify both links re-handshake into brand-new PeerConnections (= new
    // DTLS-SRTP keys) and reach Connected again.
    #[tokio::test]
    async fn rekey_rebuilds_and_reconnects() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (out_a_tx, mut out_a_rx) = unbounded_channel::<ClientMsg>();
        let (out_b_tx, mut out_b_rx) = unbounded_channel::<ClientMsg>();
        let (dec_a, mut dec_a_rx) = unbounded_channel::<(String, Bytes)>();
        let (dec_b, mut dec_b_rx) = unbounded_channel::<(String, Bytes)>();
        let (ev_a, mut ev_a_rx) = unbounded_channel::<MeshEvent>();
        let (ev_b, mut ev_b_rx) = unbounded_channel::<MeshEvent>();
        tokio::spawn(async move { while dec_a_rx.recv().await.is_some() {} });
        tokio::spawn(async move { while dec_b_rx.recv().await.is_some() {} });
        tokio::spawn(async move { while ev_a_rx.recv().await.is_some() {} });
        tokio::spawn(async move { while ev_b_rx.recv().await.is_some() {} });

        let a = Arc::new(AsyncMutex::new(Mesh::new(
            Arc::new(build_api().unwrap()), track(), "a".into(), out_a_tx, dec_a, ev_a,
        )));
        let b = Arc::new(AsyncMutex::new(Mesh::new(
            Arc::new(build_api().unwrap()), track(), "b".into(), out_b_tx, dec_b, ev_b,
        )));

        // Mock relay: forward each side's outbound to the other (stamped `from`).
        {
            let b2 = b.clone();
            tokio::spawn(async move {
                while let Some(msg) = out_a_rx.recv().await {
                    let mut g = b2.lock().await;
                    match msg {
                        ClientMsg::Offer { sdp, .. } => { let _ = g.on_offer("a", sdp).await; }
                        ClientMsg::Answer { sdp, .. } => { let _ = g.on_answer("a", sdp).await; }
                        ClientMsg::Ice { candidate, .. } => { g.on_ice("a", candidate).await; }
                        _ => {}
                    }
                }
            });
        }
        {
            let a2 = a.clone();
            tokio::spawn(async move {
                while let Some(msg) = out_b_rx.recv().await {
                    let mut g = a2.lock().await;
                    match msg {
                        ClientMsg::Offer { sdp, .. } => { let _ = g.on_offer("b", sdp).await; }
                        ClientMsg::Answer { sdp, .. } => { let _ = g.on_answer("b", sdp).await; }
                        ClientMsg::Ice { candidate, .. } => { g.on_ice("b", candidate).await; }
                        _ => {}
                    }
                }
            });
        }

        a.lock().await.on_peer("b").await.unwrap();
        assert!(wait_connected(&a, "b").await, "initial A->B connect failed");
        assert!(wait_connected(&b, "a").await, "initial B->A connect failed");

        let pc_a1 = a.lock().await.peer_pc("b").unwrap();
        let pc_b1 = b.lock().await.peer_pc("a").unwrap();

        // Broadcast Rekey reaches both clients.
        a.lock().await.rekey().await.unwrap();
        b.lock().await.rekey().await.unwrap();

        assert!(wait_connected(&a, "b").await, "A->B reconnect after rekey failed");
        assert!(wait_connected(&b, "a").await, "B->A reconnect after rekey failed");

        let pc_a2 = a.lock().await.peer_pc("b").unwrap();
        let pc_b2 = b.lock().await.peer_pc("a").unwrap();
        assert!(!Arc::ptr_eq(&pc_a1, &pc_a2), "A kept the same PC (no rekey)");
        assert!(!Arc::ptr_eq(&pc_b1, &pc_b2), "B kept the same PC (no rekey)");
    }
}
