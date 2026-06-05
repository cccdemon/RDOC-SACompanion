# RDOC-SACompanion — Architektur

> **Stand-Alone Companion.** Serverloses P2P-Voice-Mesh zwischen mehreren Companion-Apps,
> ohne SFU (kein LiveKit). Eigenständige App, **außerhalb der RDOC-Suite**, aber gleiches
> Design (kit.css / RDOC-Look). Native Audio/Netz in Rust (Bauweg B).

Datum: 2026-06-05 · Status: Design + Spike 0 bestätigt

> **Spike 0 (2026-06-05): encode-once fan-out WORKS.** `spikes/track-fanout/` — ein
> `TrackLocalStaticRTP` an 3 PeerConnections gehängt, ein einziges `write_rtp()` → alle 3
> Receiver bekamen 50/50 Pakete. webrtc-rs rewrited PT/SSRC pro Binding und macht SRTP
> pro Peer. Die Encode-once-Annahme (§4) hält.

---

## 1. Ziel & Abgrenzung

**Ziel:** Mehrere Nutzer sprechen per Push-to-Talk miteinander. Audio läuft **direkt
Peer-zu-Peer** (WebRTC, Opus, DTLS-SRTP). Es gibt **keinen Media-Server** — nur einen
winzigen **InitConnection-Server** (Signaling) und **coturn** als NAT-Fallback.

**Non-Goals:**
- Kein SFU, keine Server-Mischung von Audio.
- Keine Discord-Integration (das macht die RDOC-Suite Companion). SACompanion ist eigenständig.
- Keine großen Fleet-Ops (30-50 Leute) — dafür ist Mesh ungeeignet (siehe §10).

**Design-Cap Raumgröße (MVP):** Warn-Banner ab **12**, Hard-Cap **16** (Join-Ablehnung).
**24** ist das Ziel-Ceiling — erst nach echtem Mesh-Last-Test (Phase 6) hochziehen.

---

## 2. Topologie

```
                   ┌────────────────────────────┐
                   │  InitConnection (Rust/axum) │   ← Signaling: SDP + ICE + Roster + TURN-Creds
                   │  WebSocket, KEIN Media       │      winzig, zustandsarm (Rooms in-memory)
                   └─────┬──────────┬────────┬────┘
                         │          │        │            (nur Steuer-Nachrichten)
                      PeerA       PeerB     PeerC
                         ║          ║        ║
                         ╚═══ direkte P2P-Audiolinks (Opus/DTLS-SRTP) ═══╝
                                 (Full-Mesh: N·(N-1)/2 Links)

                   ┌────────────────────────────┐
                   │  coturn (TURN/STUN)         │   ← nur Fallback-Relay für hard-NAT-Peers
                   │  ephemere REST-Credentials  │      Media nur wenn direkt unmöglich
                   └────────────────────────────┘
```

- **Audio** fließt nie durch InitConnection. Init reicht nur Offer/Answer + ICE-Candidates
  durch und pflegt das Roster.
- **coturn** relayed Media **nur** für Peer-Paare, die kein direktes Candidate-Pair finden
  (symmetric NAT / strikte Firewall). Mehrheit bleibt direkt.

---

## 3. Komponenten

### 3.1 RDOC-SACompanion (Desktop-App)
Tauri v2. **UI im Webview** (React + kit.css, RDOC-Design wiederverwenden), **Audio + Netz
komplett in Rust** — KEIN WebView2-Audio (löst nebenbei OBS-Capture, Device-Wahl, Mic-Gain,
die beim Webview-Companion fragil waren).

| Schicht | Tech | Aufgabe |
|---|---|---|
| UI | React + kit.css | Roster, PTT-Anzeige, Settings, Verbindungs-Badges |
| Hotkey | `rdev` (eigener Thread) | globaler PTT, feuert auch im Vollbild-Spiel |
| Capture/Playback | `cpal` | Mic rein, gemischtes Audio raus, echte Device-Auswahl |
| Codec | `opus` (audiopus/opus) | Encode 1×, Decode (N-1)× |
| Transport | `webrtc` (webrtc-rs) | eine `RTCPeerConnection` pro Peer, ICE, DTLS-SRTP |
| Mixer | eigener Code | (N-1) Streams → ein Playback-Stream |
| Signaling | `tokio-tungstenite` Client | WS zu InitConnection |
| Persistenz | `tauri-plugin-store` | Settings (Hotkey, Devices, Server-URL, Sounds) |

Rust↔UI via Tauri `invoke` + Events (Roster-Updates, Verbindungstyp, Pegel).

### 3.2 InitConnection-Server
Rust, **axum** + **tokio-tungstenite**. Stateless bis auf In-Memory-Rooms. Aufgaben:
- Room-Join/Leave, Roster-Broadcast.
- SDP-Offer/Answer + ICE-Trickle weiterreichen (1:1 routing per Peer-ID).
- Glare-Regel durchsetzen (siehe §6).
- PTT-/Mute-State broadcasten.
- **TURN-Credentials** ausgeben (§8).
- Optional: Health `/healthz`, Metrics.

Ein Docker-Container. Kein DB nötig (Rooms flüchtig). Deploy wie RDOC-Suite-Pattern
(Container hinter Reverse-Proxy, `wss://`).

### 3.3 coturn
Standard coturn. STUN (kostenlos für alle, srflx) + TURN (Relay-Fallback). REST-API-Modus
(`use-auth-secret`) → InitConnection erzeugt zeitlimitierte HMAC-Creds. Eine kleine VM/LXC.

---

## 4. Audio-Pipeline

**Senden (einmal encodieren, pro Peer verschlüsseln):**
```
cpal capture ─► [PTT-Gate] ─► Opus encode (1×) ─► RTP-Packetize
   ─► fan-out: für jeden Peer SRTP-encrypt (AES, billig) ─► dessen PeerConnection
```
Opus-Encode passiert **einmal** pro 20ms-Frame; nur die SRTP-Verschlüsselung ist pro Peer.
Spart CPU gegenüber „pro PeerConnection neu encodieren".

> **BESTÄTIGT (Spike 0):** webrtc-rs teilt einen `TrackLocalStaticRTP` über mehrere
> PeerConnections — ein `write_rtp()` fan-outet an alle Bindings, PT/SSRC werden pro Binding
> rewritten, SRTP pro Peer von der Lib gemacht. Encode-once ist also trivial: einmal
> packetisieren, einmal `write_rtp()`. Beleg: `spikes/track-fanout/`.

**Empfangen (decode N-1 + mischen):**
```
Peer_i ─► SRTP-decrypt ─► Opus decode ─► Jitter-Buffer_i (per Sprecher)
                                              │
   20ms-Output-Clock ─► 1 Frame je aktivem Sprecher ziehen
                     ─► sample-weise int16 sum + clamp (Mix)
                     ─► ein gemischter 20ms-Frame ─► cpal playback
```
**Mixer = Port der `relay-bots/src/bot.ts`-Logik nach Rust** (die haben wir am 2026-06-05
gebaut: 20ms-Clock, per-Speaker-Jitter-Cap ~200ms drop-oldest, int16 sum+clamp, idle-reaping).
Bewährte Algorithmik, nur Sprachwechsel JS→Rust.

**Resampling ist Pflicht (Befund Phase-0-Spike 2026-06-05):** Opus ist fix 8/12/16/24/48 kHz,
Geräte laufen aber beliebig — WASAPI Shared-Mode zwingt die eingestellte Mix-Rate (Testhardware:
S/PDIF→FiiO K11 @ **192 kHz**). Capture/Playback müssen **Device-Rate ↔ 48 kHz resamplen**.
Spike nutzt linearen Resampler (reicht zum Hören); Produktion → `rubato` (Sinc, anti-alias).

Echo/AEC: cpal liefert rohes Mic — **AEC ist NICHT optional**, sobald Leute ohne Headset über
Lautsprecher spielen (sonst Rückkopplung → unbrauchbar). Pflicht-Baustein: `webrtc-audio-processing`
(APM: AEC + Noise-Suppression + AGC) im Capture-Pfad. Headset bleibt Empfehlung, ist aber kein
Ersatz. Details §15.2.

---

## 5. Mesh-Verwaltung (Lifecycle)

**Join:**
1. Client → Init `join {room, userId, displayName}`.
2. Init prüft Cap (§10), antwortet `roster {peers:[…]}` + `turn {…creds}`.
3. Für **jeden** bestehenden Peer baut der Neue eine PeerConnection auf; Glare-Regel
   bestimmt, wer offert (§6).
4. Init broadcastet `peer-joined` an die anderen → sie bauen ihre Seite auf.

**Leave / Disconnect:**
- Init broadcastet `peer-left {userId}` → alle schließen ihre PeerConnection zu dem Peer,
  Jitter-Buffer + Decoder freigeben.
- WS-Drop = implizites Leave (Heartbeat-Timeout).

**Renegotiation:** bei Device-/Track-Wechsel → neues Offer über Init an betroffene Peers.

---

## 6. Signaling-Protokoll (InitConnection)

WebSocket, JSON. Glare-Vermeidung: **bei einem Peer-Paar offert immer die lexikografisch
kleinere userId**; die größere wartet auf das Offer. Verhindert doppelte/kollidierende Offers.

**Client → Server**
```jsonc
{ "t": "join",       "room": "op-42", "userId": "A", "name": "Falcon" }
{ "t": "offer",      "to": "B", "sdp": "…" }
{ "t": "answer",     "to": "B", "sdp": "…" }
{ "t": "ice",        "to": "B", "candidate": "…" }
{ "t": "ptt",        "active": true }          // optional, für Roster-Sprechanzeige
{ "t": "leave" }
```

**Server → Client**
```jsonc
{ "t": "roster",      "peers": [{ "userId": "B", "name": "Wolf" }, …] }
{ "t": "turn",        "urls": ["turn:…:3478"], "username": "169…:A", "credential": "…", "ttl": 3600 }
{ "t": "peer-joined", "userId": "C", "name": "Hawk" }
{ "t": "peer-left",   "userId": "C" }
{ "t": "offer",       "from": "A", "sdp": "…" }   // weitergereicht
{ "t": "answer",      "from": "A", "sdp": "…" }
{ "t": "ice",         "from": "A", "candidate": "…" }
{ "t": "ptt",         "userId": "A", "active": true }
{ "t": "room-full",   "cap": 24 }                 // Join abgelehnt
```

Auth: Room-Token (geteilter Invite-Link) oder offen — Entscheidung in §15. Init signiert TURN-Creds.

---

## 7. NAT-Traversal

- **STUN**: immer, public (Google/Cloudflare) ODER coturn-STUN. Liefert srflx-Candidates.
- **TURN**: coturn, **für jeden User** verfügbar (Creds beim Join). Greift nur, wenn ICE kein
  direktes Pair findet. `iceTransportPolicy: "all"` (relay nur als letzter Ausweg).
- Trickle-ICE über Init (Candidates einzeln durchreichen, schnellerer Connect).

---

## 8. TURN-Credential-Flow (ephemer, für jeden)

coturn `use-auth-secret` (long-term shared secret S, nur Init + coturn kennen es).
```
username  = "<unixExpiry>:<userId>"          z.B. "1717600000:A"
credential = base64( HMAC-SHA1(S, username) )
ttl       = z.B. 1h
```
Init generiert das pro Join und schickt `turn {…}`. coturn validiert die HMAC selbst —
**kein Account-System, kein State**. Jeder Joiner kriegt gültige Creds → „möglich für jeden".

---

## 9. Verbindungs-Transparenz (Pflicht-Anforderung)

Pro Peer ein **Verbindungs-Badge**, abgeleitet aus dem **ICE selected candidate-pair**
(`webrtc-rs` liefert das via Stats / `on_selected_candidate_pair_change`):

| Selected local/remote candidate-Typ | Badge | Farbe |
|---|---|---|
| `host` / `srflx` / `prflx` | **DIREKT** | grün |
| `relay` (eine Seite via TURN) | **RELAY (TURN)** | gelb |

- Roster zeigt je Peer den Status, plus global „X/Y direkt".
- Wechselt der Pair-Typ live (ICE-Restart) → Badge updaten.
- Nutzer sieht **immer**, ob seine Stimme direkt oder über TURN geht. Erfüllt „muss
  ersichtlich sein".
- **Teilnehmer-Transparenz (Pflicht):** Jeder Teilnehmer sieht **immer die vollständige
  Teilnehmerliste**. Init broadcastet `roster` (beim Join) + `peer-joined`/`peer-left`; der
  Client rendert die komplette Liste mit Name + Verbindungs-Badge. **Keine versteckten
  Teilnehmer** — wer im Room ist, ist für alle sichtbar.

---

## 9b. RTC Text-Chat

Text-Chat läuft über einen **WebRTC DataChannel pro Peer** (SCTP über DTLS — **gleich
verschlüsselt wie Audio**, end-to-end peerweise). Label `chat`, ordered + reliable.

- **Geht NICHT über InitConnection** → der Server sieht den Chat nie (bleibt medienblind).
- Mesh-Broadcast: ein Sender schickt seine Nachricht über **alle** seine DataChannels; der
  Empfänger kennt den Absender = der Peer, dem der Channel gehört (kein Spoofing über den Server).
- Nachrichtenformat (`protocol::ChatMsg`): `{ text, ts }`. Absender-Identität ergibt sich aus
  dem Channel, nicht aus dem Payload.
- DataChannel-Aufbau: der Offerer (kleinere userId, Glare-Regel) erstellt den Channel
  (`create_data_channel("chat")`); die Gegenseite nimmt ihn via `on_data_channel` an.

---

## 10. Skalierung & Limits

Full-Mesh: `N·(N-1)/2` Links, jeder uploaded `(N-1)×`.

| Limit | Faustformel | Bemerkung |
|---|---|---|
| Upload-BW | `(N-1)×~48 kbps` | **schwächster Uploader deckelt den Room** |
| CPU Decode | `(N-1)` Opus-Decoder + Mix | ~1-2%/Stream |
| Connection-State | `(N-1)` ICE/DTLS/Jitter | praktische Decke (Join-Storm) |

- Realistisch nutzbar: **~12-16**. Ideal-Decke: ~25-30.
- **MVP: Warn-Banner ab 12** („Audioqualität kann leiden"), **Hard-Cap 16** → `room-full`.
- **Ziel-Ceiling 24** erst nach Mesh-Last-Test (Phase 6) freischalten (276 Links bei N=24 →
  Join/ICE-Storm muss erst bewiesen sein).
- Drüber = SFU-Territorium → **bewusst out of scope**.

---

## 11. Sicherheit & Privacy

**Grundsatz: „Nichts verlässt unverschlüsselt den Rechner."** Jede ausgehende Verbindung ist
verschlüsselt:

| Pfad | Verschlüsselung | Status |
|---|---|---|
| Audio Peer↔Peer | DTLS-**SRTP** (WebRTC-Pflicht) | ✅ automatisch |
| Chat Peer↔Peer | DTLS-**SCTP** DataChannel (WebRTC-Pflicht) | ✅ automatisch |
| Signaling → Init | **wss:// (TLS)** — **Pflicht** | ✅ implementiert (Phase 1b) |
| TURN-Relay | **turns:// (TLS)** statt `turn:` | ⛓️ coturn-Config (Phase 2) |

**Signaling-TLS:** Init **muss** `wss://` sprechen. **Self-signed Cert / PEM reicht** — der
Client **pinnt/vertraut** dem mitgelieferten Cert (PEM-Fingerprint), keine CA nötig. **`ws://`
nur für Loopback** (`127.0.0.1`/`localhost` — verlässt den Rechner physisch nie). Für jeden
Nicht-Loopback-Host erzwingt der Client `wss://` (Verbindung sonst verweigert). Cert-Erzeugung:
self-signed via `rcgen`/openssl, als PEM ausgeliefert; Rotation später.

- **DTLS-SRTP** ist Pflicht in WebRTC → Audio Ende-zu-Ende peerweise verschlüsselt.
- **TURN kann Audio nicht entschlüsseln** — sieht nur SRTP-Bytes. Media bleibt privat, auch über Relay.
- **ABER: der Signaling-Server (Init) muss vertrauenswürdig sein.** SDP + DTLS-Fingerprints
  laufen über Init. Ein bösartiger/kompromittierter Init könnte Fingerprints tauschen → MITM
  auf den Medienpfad. „Kein Server hört Audio" gilt nur solange Init nicht feindlich ist. Härtung:
  `wss://` (TLS zum Init), optional Fingerprint-Pinning/Out-of-band-Verifikation später. Also
  **nicht** als „zero-trust" verkaufen — Init ist Trusted Component.
- Keine Aufnahme, kein Speichern (Projektprinzip).
- TURN-Secret nur Init+coturn; Creds zeitlimitiert.
- **Room-Auth ab Phase 1 (nicht optional):** signierter Invite-Token / Room-Key. Offene
  Room-Namen sind zu schwach (jeder errät/joint). Siehe §15.4.

---

## 12. Modul-Layout (Vorschlag)

```
RDOC-SACompanion/
├─ apps/
│  └─ companion/                 # Tauri-App
│     ├─ src/                    # React UI (kit.css aus RDOC-Suite übernehmen)
│     └─ src-tauri/
│        └─ src/
│           ├─ main.rs
│           ├─ audio/            # cpal capture+playback, device-mgmt
│           ├─ codec/            # opus encode/decode
│           ├─ mesh/             # webrtc-rs PeerConnections, ICE, renegotiation
│           ├─ mixer/            # 20ms-clock int16 mixer (Port relay-bots/bot.ts)
│           ├─ signaling/        # tokio-tungstenite client + protocol types
│           ├─ hotkey.rs         # rdev
│           └─ state.rs          # room/peer state, badges
├─ server/
│  └─ init/                      # InitConnection (axum + tokio-tungstenite)
│     ├─ src/main.rs
│     ├─ src/room.rs             # in-memory rooms, glare, cap
│     ├─ src/protocol.rs         # shared message enum
│     └─ src/turn.rs             # HMAC ephemeral creds
├─ deploy/
│  ├─ docker-compose.yml         # init + coturn
│  └─ turnserver.conf
└─ docs/
   └─ ARCHITECTURE.md            # dieses Dokument
```

Optional: `protocol`-Crate als Workspace-Member, von Companion **und** Init geteilt
(ein Rust-Workspace, eine Quelle der Message-Typen — kein Drift).

---

## 13. Build & Deploy

- **Companion:** Tauri-Build (Windows, MSVC + Rust). Lokal / GitHub Actions → NSIS, wie
  RDOC-Companion. Eigenes Repo, eigener Release-Tag.
- **InitConnection + coturn:** ein `docker compose` auf einer kleinen VM/LXC. `wss://init.…`
  hinter Reverse-Proxy; coturn braucht UDP-Range offen (49152-65535) + 3478.

---

## 14. Warum kein LiveKit (Begründung der Existenz)

LiveKit = SFU = Upload-1× + Server-Skalierung für große Räume. SACompanion tauscht **Skalierung
gegen Serverlosigkeit**: kleine Squads bekommen direktes P2P (niedrigere Latenz, beste Privacy,
kein Media-Server), zahlen dafür mit der ~16er-Decke. Bewusster Trade-off.

---

## 15. Offene Punkte / Spikes (vor Implementierung)

1. ~~**webrtc-rs Encode-once**~~ — **GELÖST (Spike 0, 2026-06-05): WORKS.** Ein
   `TrackLocalStaticRTP` an N PCs, ein `write_rtp()` → alle Receiver. `spikes/track-fanout/`.
2. **AEC/Noise — entschieden: Pflicht, nicht optional.** `webrtc-audio-processing` (APM) im
   Capture-Pfad (AEC + NS + AGC). Offen bleibt nur die Crate-Integration (Windows-Build) + Tuning.
3. **Jitter-Buffer-Tuning** für Mesh (variable RTT je Peer).
4. **Room-Auth — entschieden: ab Phase 1.** Signierter Invite-Token / Room-Key. Offen: Format
   (HMAC-Token vom Init wie TURN-Creds, oder vorab geteilter Room-Key).
5. **Init-HA:** ein Container reicht? Reconnect-Verhalten der Clients bei Init-Neustart
   (laufende P2P-Links überleben Init-Restart — Init nur für Join/Renegotiation nötig).
6. **kit.css-Reuse:** als git-Subtree/Copy aus RDOC-Suite, oder eigenes geforktes Theme.
7. **cpal Geräte-Hotswap** unter Windows (Device-Wechsel zur Laufzeit).
8. **Resampler — entschieden: Pflicht** (Device-Rate ↔ 48 kHz). Spike: linear. Produktion:
   `rubato` (Sinc/FFT, anti-alias). WASAPI Shared-Mode liefert nur die Mix-Rate des Geräts
   (z.B. 192 kHz beim FiiO K11) → ohne Resampler kein Opus.

---

## 16. Phasenplan (Vorschlag)

Inkrementelles Härten (Codex-Review 2026-06-05): erst ein funktionierender 1:1-Audiopfad,
dann TURN, dann Mesh, **UI/Branding zuletzt**. Jede Phase ist für sich testbar.

| Phase | Inhalt | Gate |
|---|---|---|
| 0 ✓ | **DONE.** Spike 0 encode-once (webrtc-rs: WORKS). Mini-Spike cpal+opus+resample Roundtrip auf realer Hardware (FiiO @192k) — bestätigt gut hörbar 2026-06-05. | ✅ erfüllt |
| 1 ✓ | **DONE (2026-06-05).** 1:1 echtes Audio: cpal→opus→webrtc-rs→decode→cpal, InitConnection (Join/Roster/SDP/ICE-Relay + Room-Auth + Glare). Verifiziert: 2 Instanzen hören sich, PeerConnection Connected. | ✅ 2 Leute hören sich |
| 1b ✓ | **DONE.** Chat (DataChannel, verschlüsselt) + **Signaling-TLS**: Init serviert `wss://` (rcgen self-signed, persistiert, druckt SHA-256), Client pinnt via `CERT_SHA256`; falscher Pin → abgelehnt (verifiziert). `ws://` nur Loopback. `TLS_DISABLE=1` für ws-Loopback-Dev. | ✅ voll verschlüsselt |
| 2 ◑ | **Verbindungs-Badge DIREKT/RELAY = DONE** (aus ICE selected-pair, DIREKT auf Loopback verifiziert). TURN-Creds-Plumbing (Init mint + Client add) DONE. **Offen: coturn-Deploy** (deploy/ Scaffold da, ungetestet) + echter RELAY-Nachweis am Server. | Badge ✅ · coturn-Deploy offen |
| 3 ✓ | **DONE (Connection-Level).** 4er-Full-Mesh verifiziert: jede Instanz mit allen 3 Peers Connected (DIREKT), vollständiges Roster (`[Teilnehmer 4]`), Mixer mischt Partial-Frames mehrerer Sprecher. Audio-Mix-Qualität bei N = manueller Hör-Test. | ✅ 4 verbunden + alle sichtbar |
| 4 | **AEC/APM** (`webrtc-audio-processing`) im Capture-Pfad; ohne-Headset-Test. | kein Echo über Speaker |
| 5 ◑ | **UI DONE (Code).** Core zu `companion_core`-Lib refactored (Engine + UiEvent-Sink; Headless-Bin bleibt). Tauri-App `apps/companion` (React, RDOC-Theme): Connect-Screen, Roster mit DIREKT/RELAY-Badges + Sprech-Dots, Push-to-Talk, Chat. **Offen: `pnpm tauri dev` Build/Test beim User** (Tauri-Toolchain) + Icons/Branding-Politur. | UI gebaut · Build-Test offen |
| 6 | Härtung: Reconnect (Init-Restart-Überleben), Device-Hotswap, Cap-Verhalten, Last-Test Richtung 12-16. | release-fähig |

**Cap (entschieden 2026-06-05):** MVP **warn@12 / hard@16**. Ziel-Ceiling 24 erst nach
Mesh-Last-Test in Phase 6 hochziehen (276 Links bei N=24 → Join/ICE-Storm muss erst bewiesen sein).
