#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Tauri shell for RDOC SquadLink Lite. Thin layer over companion-core: commands
//! drive the engine, engine state is forwarded to the webview as "ui" events.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use companion_core::serverless::Serverless;
use companion_core::{start, Engine, EngineConfig, Sink, UiEvent};
use tauri::{AppHandle, Emitter, State};

struct AppState {
    engine: Mutex<Option<Engine>>,
    serverless: Mutex<Option<Arc<Serverless>>>,
}

// ── Configurable PTT via RAW global input (keyboard + mouse buttons) ──────────
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();
static PTT_BINDING: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static PTT_CAPTURE: AtomicBool = AtomicBool::new(false);

fn ptt_binding() -> &'static Mutex<Option<String>> {
    PTT_BINDING.get_or_init(|| Mutex::new(Some("F8".into())))
}

/// Stable string code for a raw key or mouse button (+ pressed flag).
fn raw_code(ev: &rdev::EventType) -> Option<(String, bool)> {
    use rdev::EventType::{ButtonPress, ButtonRelease, KeyPress, KeyRelease};
    match ev {
        KeyPress(k) => Some((format!("{k:?}"), true)),
        KeyRelease(k) => Some((format!("{k:?}"), false)),
        ButtonPress(b) => Some((format!("Mouse:{b:?}"), true)),
        ButtonRelease(b) => Some((format!("Mouse:{b:?}"), false)),
        _ => None,
    }
}

/// Global raw-input listener. In capture mode the next press becomes the new
/// binding (emitted as `ptt-bound`); otherwise the bound code toggles transmit
/// via the `ptt` event. Runs on its own thread (rdev::listen blocks).
fn start_raw_input() {
    std::thread::spawn(|| {
        let _ = rdev::listen(move |event| {
            let Some((code, down)) = raw_code(&event.event_type) else { return };
            if PTT_CAPTURE.load(Ordering::SeqCst) {
                if down {
                    PTT_CAPTURE.store(false, Ordering::SeqCst);
                    *ptt_binding().lock().unwrap() = Some(code.clone());
                    if let Some(app) = APP_HANDLE.get() {
                        let _ = app.emit("ptt-bound", code);
                    }
                }
                return;
            }
            let bound = ptt_binding().lock().unwrap().clone();
            if bound.as_deref() == Some(code.as_str()) {
                if let Some(app) = APP_HANDLE.get() {
                    let _ = app.emit("ptt", down);
                }
            }
        });
    });
}

#[tauri::command]
fn set_ptt_binding(code: Option<String>) {
    *ptt_binding().lock().unwrap() = code;
}

#[tauri::command]
fn start_ptt_capture() {
    PTT_CAPTURE.store(true, Ordering::SeqCst);
}

fn make_sink(app: &AppHandle) -> Sink {
    let app = app.clone();
    Arc::new(move |ev: UiEvent| {
        let _ = app.emit("ui", ev);
    })
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
    input_device: Option<String>,
    output_device: Option<String>,
) -> Result<(), String> {
    let engine = start(
        EngineConfig { server, room, user_id, name, token, cert_sha256, input_device, output_device },
        make_sink(&app),
    )
    .await
    .map_err(|e| e.to_string())?;
    *state.engine.lock().unwrap() = Some(engine);
    Ok(())
}

// ── Serverless 1:1 (copy-paste SDP) ───────────────────────────────────────
#[tauri::command]
async fn serverless_offer(app: AppHandle, state: State<'_, AppState>, name: String) -> Result<String, String> {
    let (s, code) = Serverless::create_offer(make_sink(&app), name).await.map_err(|e| e.to_string())?;
    *state.serverless.lock().unwrap() = Some(Arc::new(s));
    Ok(code)
}

#[tauri::command]
async fn serverless_accept_offer(app: AppHandle, state: State<'_, AppState>, name: String, code: String) -> Result<String, String> {
    let (s, answer) = Serverless::accept_offer(code, make_sink(&app), name).await.map_err(|e| e.to_string())?;
    *state.serverless.lock().unwrap() = Some(Arc::new(s));
    Ok(answer)
}

#[tauri::command]
async fn serverless_accept_answer(state: State<'_, AppState>, code: String) -> Result<(), String> {
    let s = state.serverless.lock().unwrap().clone();
    match s {
        Some(s) => s.accept_answer(code).await.map_err(|e| e.to_string()),
        None => Err("keine offene Serverless-Sitzung".into()),
    }
}

#[tauri::command]
fn toggle_transmit(state: State<AppState>) {
    if let Some(s) = state.serverless.lock().unwrap().as_ref() {
        s.toggle_transmit();
        return;
    }
    if let Some(e) = state.engine.lock().unwrap().as_ref() {
        e.toggle_transmit();
    }
}

#[tauri::command]
fn set_transmit(state: State<AppState>, on: bool) {
    if let Some(s) = state.serverless.lock().unwrap().as_ref() {
        s.set_transmit(on);
        return;
    }
    if let Some(e) = state.engine.lock().unwrap().as_ref() {
        e.set_transmit(on);
    }
}

#[tauri::command]
fn send_chat(state: State<AppState>, text: String) {
    if let Some(s) = state.serverless.lock().unwrap().as_ref() {
        s.send_chat(text);
        return;
    }
    if let Some(e) = state.engine.lock().unwrap().as_ref() {
        e.send_chat(text);
    }
}

#[tauri::command]
fn set_master_volume(state: State<AppState>, volume: f32) {
    if let Some(e) = state.engine.lock().unwrap().as_ref() {
        e.set_master_volume(volume);
    }
}

#[tauri::command]
fn set_peer_volume(state: State<AppState>, user_id: String, volume: f32) {
    if let Some(e) = state.engine.lock().unwrap().as_ref() {
        e.set_peer_volume(&user_id, volume);
    }
}

#[tauri::command]
fn list_audio_devices() -> (Vec<String>, Vec<String>) {
    companion_core::audio::list_devices()
}

fn main() {
    tauri::Builder::default()
        .manage(AppState { engine: Mutex::new(None), serverless: Mutex::new(None) })
        .setup(|app| {
            let _ = APP_HANDLE.set(app.handle().clone());
            start_raw_input();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connect,
            serverless_offer,
            serverless_accept_offer,
            serverless_accept_answer,
            toggle_transmit,
            set_transmit,
            send_chat,
            set_master_volume,
            set_peer_volume,
            list_audio_devices,
            set_ptt_binding,
            start_ptt_capture
        ])
        .run(tauri::generate_context!())
        .expect("error running tauri app");
}
