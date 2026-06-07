# RDOC VoiceMesh

Stand-Alone Companion — serverloses **P2P-Voice-Mesh** zwischen mehreren Companion-Apps,
ohne SFU (kein LiveKit). Native Audio/Netz in Rust (Tauri-App), gleiches Design wie die
RDOC-Suite Companion, aber eigenständig und außerhalb der RDOC-Suite.

- Audio läuft **direkt Peer-zu-Peer** (WebRTC, Opus, DTLS-SRTP).
- Einziger zentraler Dienst: **InitConnection** (Signaling, kein Media) + **coturn** (NAT-Fallback).
- Zielgröße: kleine/mittlere Squads (Soft-Cap 16, Hard-Max 24).

→ Vollständige Architektur: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)

## Status

**Prototyp.** Headless-Core (1:1 + 4er-Mesh), InitConnection-Server, Chat, TLS-Signaling
und Verbindungs-Badges verifiziert; **Tauri-GUI** (`apps/companion`, React + RDOC-Theme) ist
gebaut. Offen: Hör-Tuning bei N, coturn-RELAY live, Phase-6-Härtung.

## GUI-Prototyp bauen (Windows, ohne lokale Toolchain)

GitHub Actions baut die GUI auf einem sauberen Windows-Runner:

- **Workflow:** [`.github/workflows/build-companion.yml`](.github/workflows/build-companion.yml)
- **Manuell:** Actions → „Build VoiceMesh (Windows)" → *Run workflow* → danach den
  Artefakt **`rdoc-voicemesh-windows`** (NSIS-`.exe` + `.msi`) herunterladen.
- **Release:** Tag `voicemesh-v*` pushen → Workflow legt einen Draft-Release mit den Installern an.

Der Prototyp ist **unsigniert** → SmartScreen warnt beim ersten Start („Weitere Informationen →
Trotzdem ausführen").

Lokaler Dev-Build (optional, braucht Rust + Node + pnpm):
`cd apps/companion && pnpm install && pnpm tauri dev`
