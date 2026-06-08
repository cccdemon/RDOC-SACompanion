# RDOC SquadLink Lite

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
- **Manuell:** Actions → „Build SquadLink Lite (Windows)" → *Run workflow* → danach den
  Artefakt **`rdoc-squadlink-lite-windows`** (NSIS-`.exe` + `.msi`) herunterladen.
- **Release:** Tag `squadlink-lite-v*` pushen → Workflow legt einen Draft-Release mit den Installern an.

Der Prototyp ist **unsigniert** → SmartScreen warnt beim ersten Start („Weitere Informationen →
Trotzdem ausführen").

Lokaler Dev-Build (optional, braucht Rust + Node + pnpm):
`cd apps/companion && pnpm install && pnpm tauri dev`

## Verschlüsselung & Key-Rotation

Alles verlässt den Rechner verschlüsselt:

- **Audio:** WebRTC **DTLS-SRTP** (P2P, der Server sieht kein Medium)
- **Chat:** WebRTC DataChannel über **DTLS-SCTP**
- **Signaling:** **TLS / wss** zum InitConnection-Server

**Keys sind pro Session ephemer:** jeder DTLS-Handshake handelt frische SRTP-Keys aus —
zwischen verschiedenen Sessions (und Peer-Paaren) gibt es keine gemeinsamen, langlebigen Keys.

**Key-Rotation auf Knopfdruck:** der Button **„🔑 Key rotieren"** in der laufenden Session
löst eine **room-weite** Rotation aus: alle Teilnehmer reißen ihre PeerConnections ab und
handeln neu aus → neue DTLS-SRTP-Keys auf jedem Link. Die aktuelle **Schlüssel-Generation**
(+ Zeitpunkt der letzten Rotation) wird in der Verschlüsselungs-Fußzeile angezeigt.
Protokoll: `ClientMsg::Rekey` → Server-Broadcast `ServerMsg::Rekey` → `mesh.rekey()`.

## Sicherheit / Härtung

- **Loopback-Erkennung** parst den URL-Host (kein Substring-Match): `ws://` nur zu
  `localhost`/`127.0.0.1`/`::1`, sonst `wss://` erzwungen (`signaling::server_url_ok`, Unit-getestet).
- **Tauri-CSP** strikt gesetzt (kein `null`, keine Wildcards): nur self + `squadlink.raumdock.org`
  (https/wss) + IPC.
- **Eingabevalidierung** Rust-seitig für alle Tauri-Commands (Server-URL, room/user_id/name/token/
  cert_sha256, Chat-Länge, Volume-Clamp, PTT-Code).
- **InitConnection:** WS-Frame-Limit (64 KB), Längen-Caps (room/user_id/name/SDP/ICE), REST-Body-Limit,
  **bounded** per-Peer-Channels (Backpressure), **per-IP-Rate-Limits** auf `/session` + PIN-Join
  (zusätzlich zum per-Code `MAX_ATTEMPTS`).
- **Auth fail-closed:** ohne `ROOM_AUTH_SECRET` startet der Server **nicht** — außer
  `ALLOW_OPEN_AUTH=1` (nur Dev).
- **Dependencies:** Vite 6 / esbuild 0.25; CI installiert mit `--frozen-lockfile` und führt
  `pnpm audit` + `cargo audit` aus.

## Deployment / Konfiguration (InitConnection)

Env-Variablen:

| Variable | Zweck |
|---|---|
| `ROOM_AUTH_SECRET` | **Pflicht in Prod** — HMAC-Secret für Room-Join-Tokens. Einmal erzeugen: `openssl rand -hex 32`. |
| `ALLOW_OPEN_AUTH` | `1` erlaubt Open-Mode **ohne** Secret (nur Dev). |
| `PORT` | Listen-Port (Default 8080). |
| `TLS_DISABLE` | `1` = plain ws (nur hinter TLS-terminierendem Proxy / loopback). |
| `PUBLIC_BASE` / `PUBLIC_WS` | Öffentliche URLs für Share-Links + zurückgegebene ws-URL. |
| `TURN_SECRET` / `TURN_URLS` | optionale coturn-Creds (NAT-Fallback). |

Prod läuft hinter dem RDOC-Suite-Caddy auf `squadlink.raumdock.org`
(`deploy/docker-compose.proxy.yml`); `.env` **muss** `ROOM_AUTH_SECRET` setzen
(sonst Fail-Closed-Abbruch). Der Server serviert auch die Web-Seiten `/`, `/privacy`,
`/legal`, `/license` und die Share-Landing `/j/:code`.

## License

© head87x & justcallmedeimos — **PolyForm Noncommercial License 1.0.0** (see [LICENSE](LICENSE)).

Free for any **non-commercial** purpose (private, community, education, research).

**Commercial use requires a separate commercial license.** Commercial use includes selling,
sublicensing, hosting as a paid service, integrating into commercial products, or using the
software in revenue-generating activities.

For commercial licensing inquiries: **commercialusage@raumdock.org**
