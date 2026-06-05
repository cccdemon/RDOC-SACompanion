#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Tauri shell for RDOC-SACompanion. Thin layer over companion-core: commands
//! drive the engine, engine state is forwarded to the webview as "ui" events.

use std::sync::{Arc, Mutex};

use companion_core::{start, Engine, EngineConfig, Sink, UiEvent};
use tauri::{AppHandle, Emitter, State};

struct AppState {
    engine: Mutex<Option<Engine>>,
}

#[tauri::command]
async fn connect(
    app: AppHandle,
    state: State<'_, AppState>,
    server: String,
    room: String,
    user_id: String,
    name: String,
    token: Option<String>,
    cert_sha256: Option<String>,
) -> Result<(), String> {
    let app2 = app.clone();
    let sink: Sink = Arc::new(move |ev: UiEvent| {
        let _ = app2.emit("ui", ev);
    });
    let engine = start(
        EngineConfig { server, room, user_id, name, token, cert_sha256 },
        sink,
    )
    .await
    .map_err(|e| e.to_string())?;
    *state.engine.lock().unwrap() = Some(engine);
    Ok(())
}

#[tauri::command]
fn toggle_transmit(state: State<AppState>) {
    if let Some(e) = state.engine.lock().unwrap().as_ref() {
        e.toggle_transmit();
    }
}

#[tauri::command]
fn set_transmit(state: State<AppState>, on: bool) {
    if let Some(e) = state.engine.lock().unwrap().as_ref() {
        e.set_transmit(on);
    }
}

#[tauri::command]
fn send_chat(state: State<AppState>, text: String) {
    if let Some(e) = state.engine.lock().unwrap().as_ref() {
        e.send_chat(text);
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(AppState { engine: Mutex::new(None) })
        .invoke_handler(tauri::generate_handler![
            connect,
            toggle_transmit,
            set_transmit,
            send_chat
        ])
        .run(tauri::generate_context!())
        .expect("error running tauri app");
}
