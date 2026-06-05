//! WebSocket signaling client. Encryption rule: `ws://` only for loopback
//! (never leaves the machine); any other host MUST be `wss://` (refused
//! otherwise). wss self-signed/PEM-pin support lands with the TLS sub-step.

use anyhow::{bail, Result};
use futures::{SinkExt, StreamExt};
use protocol::{ClientMsg, ServerMsg};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

pub struct Signaling {
    pub out: mpsc::UnboundedSender<ClientMsg>,
    pub incoming: mpsc::UnboundedReceiver<ServerMsg>,
}

fn is_loopback(url: &str) -> bool {
    url.contains("127.0.0.1") || url.contains("//localhost") || url.contains("[::1]")
}

pub async fn connect(url: &str) -> Result<Signaling> {
    if url.starts_with("ws://") && !is_loopback(url) {
        bail!("ws:// nur für Loopback erlaubt — nutze wss:// (Nichts verlässt unverschlüsselt den Rechner)");
    }
    let (ws, _) = connect_async(url).await?;
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
