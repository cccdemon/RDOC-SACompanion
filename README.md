# RDOC-SACompanion

Stand-Alone Companion — serverloses **P2P-Voice-Mesh** zwischen mehreren Companion-Apps,
ohne SFU (kein LiveKit). Native Audio/Netz in Rust (Tauri-App), gleiches Design wie die
RDOC-Suite Companion, aber eigenständig und außerhalb der RDOC-Suite.

- Audio läuft **direkt Peer-zu-Peer** (WebRTC, Opus, DTLS-SRTP).
- Einziger zentraler Dienst: **InitConnection** (Signaling, kein Media) + **coturn** (NAT-Fallback).
- Zielgröße: kleine/mittlere Squads (Soft-Cap 16, Hard-Max 24).

→ Vollständige Architektur: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)

Status: **Design** (noch kein Code).
