//! Headless console client — drives the companion-core engine and renders its
//! events to stdout. Console input: "/t" toggles transmit (PTT), any other
//! line is broadcast as chat.
//!
//! Env: SERVER, ROOM, USER_ID, NAME, TOKEN, CERT_SHA256 (for wss://),
//!      IN_DEVICE/OUT_DEVICE.

use std::sync::Arc;

use anyhow::Result;
use companion_core::{start, EngineConfig, Sink, UiEvent};

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "companion_core=info".into()),
        )
        .init();

    let user = env_or("USER_ID", "user");
    let cfg = EngineConfig {
        server: env_or("SERVER", "ws://127.0.0.1:8080/ws"),
        room: env_or("ROOM", "testroom"),
        name: env_or("NAME", &user),
        user_id: user,
        token: std::env::var("TOKEN").ok(),
        cert_sha256: std::env::var("CERT_SHA256").ok(),
        input_device: std::env::var("IN_DEVICE").ok(),
        output_device: std::env::var("OUT_DEVICE").ok(),
        relay_enabled: std::env::var("RELAY_DISABLED").is_err(),
    };

    let sink: Sink = Arc::new(|ev| match ev {
        UiEvent::Roster { participants } => {
            let names: Vec<String> = participants
                .iter()
                .map(|p| {
                    let you = if p.you { " (du)" } else { "" };
                    let badge = p.badge.as_ref().map(|b| format!(" [{b}]")).unwrap_or_default();
                    let talk = if p.speaking { " 🎙" } else { "" };
                    format!("{}{you}{badge}{talk}", p.name)
                })
                .collect();
            println!("[Teilnehmer {}]: {}", participants.len(), names.join(", "));
        }
        UiEvent::Chat { from, text } => println!("[chat {from}] {text}"),
        UiEvent::Status { connected, transmitting } => {
            println!("[status verbunden={connected} senden={transmitting}]")
        }
        UiEvent::Log { text } => println!("[log] {text}"),
        UiEvent::Net { peers, up_kbps, down_kbps } => {
            println!("[net peers={peers} up={up_kbps}kbps down={down_kbps}kbps]")
        }
        UiEvent::Rekeyed { generation, by } => println!("[rekey #{generation} by {by}]"),
        UiEvent::Signaling { up } => println!("[signaling {}]", if up { "up" } else { "down" }),
    });

    let engine = Arc::new(start(cfg, sink).await?);
    println!("== RDOC SquadLink Lite (headless) ==  '/t'=Senden toggle · sonst Chat · Strg+C beendet");

    let e2 = engine.clone();
    std::thread::spawn(move || {
        let mut line = String::new();
        loop {
            line.clear();
            if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let t = line.trim();
            if t == "/t" {
                e2.toggle_transmit();
            } else if !t.is_empty() {
                e2.send_chat(t.to_string());
            }
        }
    });

    tokio::signal::ctrl_c().await.ok();
    Ok(())
}
