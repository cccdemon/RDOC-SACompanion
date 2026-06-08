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
    // Codes are rdev Debug strings like "F8" / "Mouse:Unknown(1)"; bound length.
    let code = code.filter(|c| !c.is_empty() && c.len() <= 64);
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

const MAX_CHAT_LEN: usize = 2000;

/// Identifier guard: non-empty, bounded, safe charset.
fn valid_id(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}
fn valid_hex(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.chars().all(|c| c.is_ascii_hexdigit())
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
    // ── Rust-side validation (never trust the webview) ──────────────────────
    let server = server.trim().to_string();
    if !companion_core::signaling::server_url_ok(&server) {
        return Err("invalid server URL — use wss:// or loopback ws://".into());
    }
    if !valid_id(&room, 64) {
        return Err("invalid room".into());
    }
    if !valid_id(&user_id, 64) {
        return Err("invalid user_id".into());
    }
    let name = name.trim().to_string();
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(|c| c.is_control()) {
        return Err("invalid name (1–64 chars, no control chars)".into());
    }
    let token = match token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        Some(t) if valid_hex(&t, 128) => Some(t),
        Some(_) => return Err("invalid token".into()),
        None => None,
    };
    let cert_sha256 = match cert_sha256.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()) {
        Some(c) if c.len() == 64 && valid_hex(&c, 64) => Some(c),
        Some(_) => return Err("invalid cert_sha256 (64 hex chars)".into()),
        None => None,
    };
    let clean_dev = |d: Option<String>| -> Result<Option<String>, String> {
        match d.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
            Some(s) if s.len() <= 256 => Ok(Some(s)),
            Some(_) => Err("invalid device name".into()),
            None => Ok(None),
        }
    };
    let input_device = clean_dev(input_device)?;
    let output_device = clean_dev(output_device)?;

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
    // Bound the message length (defense against oversized webview input).
    let text: String = text.chars().take(MAX_CHAT_LEN).collect();
    if text.trim().is_empty() {
        return;
    }
    if let Some(s) = state.serverless.lock().unwrap().as_ref() {
        s.send_chat(text);
        return;
    }
    if let Some(e) = state.engine.lock().unwrap().as_ref() {
        e.send_chat(text);
    }
}

/// Clamp incoming volume to a sane gain range (also clamped in core).
fn clamp_vol(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 2.0)
    } else {
        1.0
    }
}

#[tauri::command]
fn set_master_volume(state: State<AppState>, volume: f32) {
    if let Some(e) = state.engine.lock().unwrap().as_ref() {
        e.set_master_volume(clamp_vol(volume));
    }
}

#[tauri::command]
fn set_peer_volume(state: State<AppState>, user_id: String, volume: f32) -> Result<(), String> {
    if !valid_id(&user_id, 64) {
        return Err("invalid user_id".into());
    }
    if let Some(e) = state.engine.lock().unwrap().as_ref() {
        e.set_peer_volume(&user_id, clamp_vol(volume));
    }
    Ok(())
}

#[tauri::command]
fn list_audio_devices() -> (Vec<String>, Vec<String>) {
    companion_core::audio::list_devices()
}

#[tauri::command]
fn rotate_key(state: State<AppState>) {
    if let Some(e) = state.engine.lock().unwrap().as_ref() {
        e.rotate_key();
    }
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
            rotate_key,
            set_ptt_binding,
            start_ptt_capture
        ])
        .run(tauri::generate_context!())
        .expect("error running tauri app");
}
