//! Network self-check: verifies the WebRTC data path (send/receive) via a local
//! two-PeerConnection DataChannel echo, whether a public STUN reflexive
//! candidate is reachable (outbound UDP / NAT traversal viable), and whether the
//! signaling server is reachable. No external test peer needed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use bytes::Bytes;
use serde::Serialize;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

#[derive(Serialize, Clone, Copy, Debug)]
pub struct SelfCheck {
    /// Signaling server (wss) reachable.
    pub signaling: bool,
    /// Our outbound data reached the other end (can send).
    pub can_send: bool,
    /// Data reached us back (can receive).
    pub can_receive: bool,
    /// A public STUN server-reflexive candidate was gathered (internet UDP ok).
    pub stun: bool,
}

pub async fn run(server: &str, cert: Option<&str>) -> SelfCheck {
    let signaling = crate::signaling::connect(server, cert).await.is_ok();
    let (can_send, can_receive, stun) = webrtc_loopback().await.unwrap_or((false, false, false));
    SelfCheck { signaling, can_send, can_receive, stun }
}

fn config() -> RTCConfiguration {
    RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_owned()],
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// Returns (can_send, can_receive, stun_reachable).
async fn webrtc_loopback() -> Result<(bool, bool, bool)> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let api = crate::build_api()?;
    let a = Arc::new(api.new_peer_connection(config()).await?);
    let b = Arc::new(api.new_peer_connection(config()).await?);

    let b_recv = Arc::new(AtomicBool::new(false)); // B got A's data → A can send
    let a_recv = Arc::new(AtomicBool::new(false)); // A got the echo → A can receive

    // B side: echo back anything it receives.
    {
        let b_recv = b_recv.clone();
        b.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
            let b_recv = b_recv.clone();
            Box::pin(async move {
                let dc_echo = dc.clone();
                dc.on_message(Box::new(move |_m: DataChannelMessage| {
                    let b_recv = b_recv.clone();
                    let dc_echo = dc_echo.clone();
                    Box::pin(async move {
                        b_recv.store(true, Ordering::SeqCst);
                        let _ = dc_echo.send(&Bytes::from_static(b"pong")).await;
                    })
                }));
            })
        }));
    }

    // A side: open a channel, send "ping" on open, mark receive on the echo.
    let dc = a.create_data_channel("selfcheck", None).await?;
    {
        let a_recv = a_recv.clone();
        dc.on_message(Box::new(move |_m: DataChannelMessage| {
            let a_recv = a_recv.clone();
            Box::pin(async move { a_recv.store(true, Ordering::SeqCst) })
        }));
    }
    {
        let dc_send = dc.clone();
        dc.on_open(Box::new(move || {
            let dc_send = dc_send.clone();
            Box::pin(async move {
                let _ = dc_send.send(&Bytes::from_static(b"ping")).await;
            })
        }));
    }

    // Non-trickle exchange: wait for ICE gathering so the SDP carries candidates.
    let mut ga = a.gathering_complete_promise().await;
    let offer = a.create_offer(None).await?;
    a.set_local_description(offer).await?;
    let _ = ga.recv().await;
    let full_offer = a.local_description().await.ok_or_else(|| anyhow!("no local offer"))?;

    b.set_remote_description(full_offer.clone()).await?;
    let mut gb = b.gathering_complete_promise().await;
    let answer = b.create_answer(None).await?;
    b.set_local_description(answer).await?;
    let _ = gb.recv().await;
    let full_answer = b.local_description().await.ok_or_else(|| anyhow!("no local answer"))?;
    a.set_remote_description(full_answer.clone()).await?;

    let stun = sdp_has_srflx(&full_offer) || sdp_has_srflx(&full_answer);

    // Give the data path up to ~8s to round-trip.
    for _ in 0..40 {
        if a_recv.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let send = b_recv.load(Ordering::SeqCst);
    let recv = a_recv.load(Ordering::SeqCst);
    let _ = a.close().await;
    let _ = b.close().await;
    Ok((send, recv, stun))
}

fn sdp_has_srflx(d: &RTCSessionDescription) -> bool {
    d.sdp.contains("typ srflx")
}
