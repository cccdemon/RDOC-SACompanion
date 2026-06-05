//! WebSocket signaling client.
//!
//! Encryption rule ("Nichts verlässt unverschlüsselt den Rechner"):
//!   - `ws://`  allowed ONLY for loopback (never leaves the machine).
//!   - `wss://` required for every other host; the server's self-signed cert
//!     is pinned by SHA-256 via env CERT_SHA256 (no CA needed).

use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use futures::{SinkExt, StreamExt};
use protocol::{ClientMsg, ServerMsg};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::Connector;

pub struct Signaling {
    pub out: mpsc::UnboundedSender<ClientMsg>,
    pub incoming: mpsc::UnboundedReceiver<ServerMsg>,
}

fn is_loopback(url: &str) -> bool {
    url.contains("127.0.0.1") || url.contains("//localhost") || url.contains("[::1]")
}

/// Verifier that trusts exactly one cert, matched by its SHA-256 fingerprint.
#[derive(Debug)]
struct PinnedCert {
    sha256: [u8; 32],
}
impl ServerCertVerifier for PinnedCert {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let mut h = Sha256::new();
        h.update(end_entity.as_ref());
        if h.finalize().as_slice() == self.sha256 {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("certificate pin mismatch".into()))
        }
    }
    // The cert itself is pinned, so accept its handshake signatures.
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn pinned_connector(cert_sha256: &str) -> Result<Connector> {
    let bytes = hex::decode(cert_sha256.trim()).map_err(|_| anyhow!("CERT_SHA256 ist kein gültiges Hex"))?;
    if bytes.len() != 32 {
        bail!("CERT_SHA256 muss 32 Byte (64 Hex-Zeichen) sein");
    }
    let mut sha256 = [0u8; 32];
    sha256.copy_from_slice(&bytes);

    let _ = rustls::crypto::ring::default_provider().install_default();
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedCert { sha256 }))
        .with_no_client_auth();
    Ok(Connector::Rustls(Arc::new(config)))
}

pub async fn connect(url: &str, cert_sha256: Option<&str>) -> Result<Signaling> {
    let ws = if url.starts_with("wss://") {
        let pin = cert_sha256
            .ok_or_else(|| anyhow!("wss:// braucht CERT_SHA256 (Fingerprint vom Server-Start) zum Pinnen"))?;
        let connector = pinned_connector(pin)?;
        let (ws, _) =
            tokio_tungstenite::connect_async_tls_with_config(url, None, false, Some(connector))
                .await?;
        ws
    } else {
        if url.starts_with("ws://") && !is_loopback(url) {
            bail!("ws:// nur für Loopback erlaubt — nutze wss:// (Nichts verlässt unverschlüsselt den Rechner)");
        }
        let (ws, _) = tokio_tungstenite::connect_async(url).await?;
        ws
    };

    let (mut sink, mut stream) = ws.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<ClientMsg>();
    let (in_tx, in_rx) = mpsc::unbounded_channel::<ServerMsg>();

    tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if let Ok(s) = serde_json::to_string(&m) {
                if sink.send(Message::Text(s)).await.is_err() {
                    break;
                }
            }
        }
    });
    tokio::spawn(async move {
        while let Some(Ok(msg)) = stream.next().await {
            if let Message::Text(t) = msg {
                if let Ok(sm) = serde_json::from_str::<ServerMsg>(&t) {
                    if in_tx.send(sm).is_err() {
                        break;
                    }
                }
            }
        }
    });

    Ok(Signaling { out: out_tx, incoming: in_rx })
}
