# RDOC SquadLink Lite (Tauri UI)

Desktop-UI über die `companion-core`-Engine. Roster mit Verbindungs-Badges
(DIREKT/RELAY) + Sprech-Anzeige, Push-to-Talk, Chat.

## Dev

```powershell
cd apps/companion
pnpm install
pnpm tauri dev
```

Voraussetzungen: Node/pnpm, Rust + MSVC (wie RDOC-Suite Companion), WebView2.

## Verbinden

Im Connect-Screen:
- **Server**: `ws://127.0.0.1:8080/ws` (Loopback) oder `wss://host:8080/ws`.
- **Room** + **Name**.
- **Room-Token**: nur wenn der Server mit `ROOM_AUTH_SECRET` läuft
  (`init-connection mint <room>`).
- **CERT_SHA256**: nur für `wss://` — der Fingerprint, den der Server beim Start druckt.

Server starten: aus dem Repo-Root `cargo run -p init-connection`
(Dev/Loopback: `TLS_DISABLE=1` für `ws://`).

## Hinweis

`apps/companion-core` bleibt der Headless-Test-Client (gleiche Engine, Konsole).
Diese Tauri-App ist die grafische Variante. Beide nutzen `companion_core::start`.
