//! Spike 0 — encode-once fan-out.
//!
//! Question: can ONE `TrackLocalStaticRTP` be added to N PeerConnections so a
//! single `write_rtp()` reaches all N remotes? If yes, the SACompanion mesh
//! encodes Opus once and the lib handles per-peer SRTP — the core efficiency
//! assumption in docs/ARCHITECTURE.md §4.
//!
//! Method: in-process loopback. For each of N links, a sender PC (holding the
//! SHARED track) is connected to its own receiver PC. We write 50 RTP packets
//! ONCE on the shared track and count what each receiver got.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use tokio::time::{sleep, Duration};

use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_OPUS};
use webrtc::api::APIBuilder;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp::header::Header;
use webrtc::rtp::packet::Packet;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::{TrackLocal, TrackLocalWriter};

const N: usize = 3;
const PACKETS: usize = 50;

async fn new_pc() -> Result<Arc<RTCPeerConnection>> {
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut m)?;
    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .build();
    // No ICE servers: on loopback the host candidates connect directly.
    let config = RTCConfiguration::default();
    Ok(Arc::new(api.new_peer_connection(config).await?))
}

/// Non-trickle offer/answer exchange in-process (wait for full ICE gathering,
/// then hand over the complete SDP — candidates embedded).
async fn connect(send_pc: &Arc<RTCPeerConnection>, recv_pc: &Arc<RTCPeerConnection>) -> Result<()> {
    let offer = send_pc.create_offer(None).await?;
    let mut gather = send_pc.gathering_complete_promise().await;
    send_pc.set_local_description(offer).await?;
    let _ = gather.recv().await;
    let offer_sdp = send_pc.local_description().await.expect("offer set");

    recv_pc.set_remote_description(offer_sdp).await?;
    let answer = recv_pc.create_answer(None).await?;
    let mut gather2 = recv_pc.gathering_complete_promise().await;
    recv_pc.set_local_description(answer).await?;
    let _ = gather2.recv().await;
    let answer_sdp = recv_pc.local_description().await.expect("answer set");

    send_pc.set_remote_description(answer_sdp).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // The ONE shared track. Opus, stereo, 48k — same params the real app uses.
    let track = Arc::new(TrackLocalStaticRTP::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_OPUS.to_owned(),
            clock_rate: 48000,
            channels: 2,
            ..Default::default()
        },
        "audio".to_owned(),
        "rdoc-mesh".to_owned(),
    ));

    let mut counters: Vec<Arc<AtomicUsize>> = Vec::new();
    // Keep PCs alive for the whole run.
    let mut keep: Vec<Arc<RTCPeerConnection>> = Vec::new();

    for i in 0..N {
        let send_pc = new_pc().await?;
        let recv_pc = new_pc().await?;

        // Add the SAME track to this sender PC.
        let rtp_sender = send_pc
            .add_track(Arc::clone(&track) as Arc<dyn TrackLocal + Send + Sync>)
            .await?;
        // Drain RTCP from the sender, otherwise the transport back-pressures.
        tokio::spawn(async move {
            let mut buf = vec![0u8; 1500];
            while rtp_sender.read(&mut buf).await.is_ok() {}
        });

        // Count RTP packets that actually arrive at this receiver.
        let counter = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&counter);
        recv_pc.on_track(Box::new(move |track, _receiver, _transceiver| {
            let c = Arc::clone(&c);
            Box::pin(async move {
                tokio::spawn(async move {
                    while track.read_rtp().await.is_ok() {
                        c.fetch_add(1, Ordering::SeqCst);
                    }
                });
            })
        }));

        connect(&send_pc, &recv_pc).await?;
        println!("link {i} connected");

        counters.push(counter);
        keep.push(send_pc);
        keep.push(recv_pc);
    }

    // Let DTLS/SRTP settle.
    sleep(Duration::from_secs(2)).await;

    // Write 50 packets ONCE on the shared track. If fan-out works, every
    // receiver should see ~all of them. (write_rtp rewrites PT/SSRC per
    // binding, so our placeholder header values are fine.)
    let mut seq: u16 = 0;
    let mut ts: u32 = 0;
    for _ in 0..PACKETS {
        let pkt = Packet {
            header: Header {
                version: 2,
                payload_type: 111,
                sequence_number: seq,
                timestamp: ts,
                ssrc: 0x5043_4d50,
                ..Default::default()
            },
            payload: Bytes::from_static(&[0u8; 80]),
        };
        track.write_rtp(&pkt).await?;
        seq = seq.wrapping_add(1);
        ts = ts.wrapping_add(960);
        sleep(Duration::from_millis(20)).await;
    }

    sleep(Duration::from_millis(800)).await;

    println!("\n--- RESULT (wrote {PACKETS} packets ONCE on the shared track) ---");
    let mut ok = true;
    for (i, c) in counters.iter().enumerate() {
        let got = c.load(Ordering::SeqCst);
        println!("receiver {i}: received {got} packets");
        if got < PACKETS * 4 / 5 {
            ok = false;
        }
    }
    println!(
        "\nVERDICT: encode-once fan-out {}",
        if ok {
            "WORKS — one write_rtp() reached all N receivers."
        } else {
            "FAILED — receivers did not all get the stream."
        }
    );

    Ok(())
}
