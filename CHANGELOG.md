# Changelog

All notable changes to RDOC SquadLink Lite. Tags: `squadlink-lite-v*`.

## v0.1.9 — 2026-06-08

### Fixed
- **Occasional audio crackle**: the capture-path compressor's makeup gain could
  clip on the final clamp. Added a smooth peak **limiter** (instant attack, 50 ms
  release) after the compressor and lowered default makeup 1.8→1.4 — no more clip.

### Added
- **Configurable audio chain in the gear menu**: Noise Gate, Compressor (threshold/
  ratio/makeup) and Limiter (ceiling) — each toggleable + adjustable, persisted,
  pushed live (`DspConfig`, `set_dsp`). All on by default.
- **Mic self-check** (gear menu): local monitor playback of your own (processed) mic.
- **Disconnect / "Verlassen"** button → returns to the create/join screen. Cleanly
  stops the engine **and** the audio threads (shutdown flag) so a later reconnect
  doesn't stack duplicate capture/playback rigs.

## v0.1.8 — 2026-06-08

### Fixed
- **Signaling drop no longer ends the session.** The WS signaling link is now
  decoupled from the P2P mesh: if it drops (e.g. server restart), audio/chat keep
  running and the UI shows a "Signaling getrennt" banner instead of going
  disconnected. Engine keeps the mesh alive via an internal uplink channel.

### Added
- **"Session wiederaufnehmen"** button — reconnects signaling + re-joins the room
  without tearing down the live mesh (`reconnect_session` / `Cmd::Reconnect`,
  `UiEvent::Signaling`).
- **Self-mute mic** (🎙️) — stop sending while still hearing everyone (gates PTT).
- **Deafen / Ton aus** (🔊) — mute all output without losing the volume value.
- **Explicit toggle-transmit button** next to push-to-talk.

## v0.1.7 — 2026-06-08

### Fixed
- **Glare-aware key rotation:** `mesh.rekey()` no longer has both peers tear down
  and re-offer independently (which could race and leave an answerer stuck). Per
  pair only the smaller user_id rebuilds + re-offers; the larger side lets
  `on_offer` swap in the new PC. Added a two-mesh integration test (real ICE/DTLS
  over a mock relay) proving both links reconnect into fresh PeerConnections.

## v0.1.6 — 2026-06-08

### Added
- **On-demand session key rotation.** Button "🔑 Key rotieren" triggers a room-wide
  DTLS-SRTP re-handshake (new keys on every link). Protocol `ClientMsg::Rekey` →
  server broadcast `ServerMsg::Rekey` → `mesh.rekey()`. UI shows the current key
  generation + last-rotation time in the encryption footer (`UiEvent::Rekeyed`).

## v0.1.5 — 2026-06-08

### Security
- **Loopback detection** now parses the URL host instead of substring-matching
  (`ws://` only to `localhost`/`127.0.0.1`/`::1`); added `signaling::server_url_ok`
  + unit tests (incl. `ws://evil.example/127.0.0.1`).
- **Tauri CSP** set to a strict policy (was `null`): self + `squadlink.raumdock.org`
  (https/wss) + IPC, no wildcards.
- **Tauri command input validation** (server URL, room/user_id/name/token/cert_sha256,
  chat length, volume clamp, PTT code) with clean `Result` errors.
- **InitConnection hardening:** 64 KB WS frame cap, length caps on room/user_id/name/SDP/ICE,
  REST body limit, bounded per-peer channels (backpressure), per-IP rate limits on
  `/session` and the PIN join (on top of per-code `MAX_ATTEMPTS`).
- **Auth fail-closed:** missing `ROOM_AUTH_SECRET` aborts startup unless `ALLOW_OPEN_AUTH=1`
  (dev only). Production now runs in HMAC mode.
- **Dependencies/CI:** Vite 6 + esbuild 0.25 (override); CI uses `--frozen-lockfile` and runs
  `pnpm audit` + `cargo audit`.

## v0.1.4 — 2026-06-08

### Added
- Public web surface served by InitConnection: `/` (what-is + links to raumdock.org,
  Fleetmanager, GitHub), `/privacy`, `/legal`, `/license`.
- **PolyForm Noncommercial License 1.0.0** (`LICENSE`); authors head87x & justcallmedeimos;
  commercial-use clause + contact `commercialusage@raumdock.org`.
- App icon + in-app logo generated from `Squad_Link_Lite.png` (CI `tauri icon`).

### Changed
- Repository renamed to `cccdemon/RDOC-SquadLinkLite` (GITHUB_URL + installer pull updated).

## v0.1.3 — 2026-06-08

### Added
- **Configurable RAW push-to-talk** (any key or mouse button via `rdev`), rebind via the
  gear menu; binding persisted.
- **Live bandwidth**: real WebRTC transport-stats polling → measured up/down kbps + peer count.
- **Audio compressor** in the capture path (RNNoise noise-suppression already on by default).

### Changed
- Volume sliders are 0–100 % (100 = unity), no longer 0–200.

## v0.1.2 — 2026-06-07

### Added
- Master + per-participant output volume; audio device selection behind a gear icon.
- In-session share panel (code + link + PIN stay visible to the host).
- Encryption footer.

### Changed
- Session-only UI (removed Server/Serverless tabs); chat shows display names, not raw ids.

### Fixed
- Session persistence: a session now lives while its room has members (5-min grace after
  empty, 24 h hard cap), instead of a fixed 12 h TTL from creation.
