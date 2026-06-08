import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import logo from "./Squad_Link_Lite.png";

const REPO = "cccdemon/RDOC-SquadLinkLite";

// Parse "0.1.10" → [0,1,10]; true if `a` is a newer version than `b`.
function isNewer(a: string, b: string): boolean {
  const pa = a.split(".").map(Number);
  const pb = b.split(".").map(Number);
  for (let i = 0; i < 3; i++) {
    if ((pa[i] || 0) !== (pb[i] || 0)) return (pa[i] || 0) > (pb[i] || 0);
  }
  return false;
}

// Newest CHANGELOG section: from the first "## " heading to the next one.
function topChangelogSection(md: string): string {
  const start = md.indexOf("## ");
  if (start < 0) return "";
  const next = md.indexOf("\n## ", start + 3);
  return md.slice(start, next < 0 ? undefined : next).trim();
}

// Friendly label for raw rdev codes (e.g. "F8", "KeyR", "Mouse:Unknown(1)").
function pttLabel(code: string): string {
  if (code.startsWith("Mouse:")) {
    const b = code.slice(6);
    const m = b.match(/Unknown\((\d+)\)/);
    if (m) return `Maustaste ${Number(m[1]) + 3}`; // Unknown(1)→Mouse4
    return `Maus ${b}`;
  }
  return code.replace(/^Key/, "");
}

type Participant = {
  user_id: string;
  name: string;
  you: boolean;
  badge: string | null;
  speaking: boolean;
};
type ChatLine = { from: string; text: string };

type UiEvent =
  | { type: "roster"; participants: Participant[] }
  | { type: "chat"; from: string; text: string }
  | { type: "status"; connected: boolean; transmitting: boolean }
  | { type: "log"; text: string }
  | { type: "net"; peers: number; up_kbps: number; down_kbps: number }
  | { type: "rekeyed"; generation: number; by: string }
  | { type: "signaling"; up: boolean };

// Capture-path DSP (must match companion_core::audio::DspConfig field names).
type DspConfig = {
  gate: boolean;
  gate_threshold: number;
  compressor: boolean;
  comp_threshold: number;
  comp_ratio: number;
  comp_makeup: number;
  limiter: boolean;
  limiter_ceiling: number;
};
const DSP_DEFAULT: DspConfig = {
  gate: true,
  gate_threshold: 0.015,
  compressor: true,
  comp_threshold: 0.15,
  comp_ratio: 3.0,
  comp_makeup: 1.4,
  limiter: true,
  limiter_ceiling: 0.97,
};

export default function App() {
  const [connected, setConnected] = useState(false);
  const [transmitting, setTransmitting] = useState(false);
  const [participants, setParticipants] = useState<Participant[]>([]);
  const [chat, setChat] = useState<ChatLine[]>([]);
  const [log, setLog] = useState("");
  const [connecting, setConnecting] = useState(false);
  const [form, setForm] = useState(() => {
    try {
      const s = localStorage.getItem("sa.form");
      if (s) return JSON.parse(s);
    } catch {
      /* ignore */
    }
    return { name: "" };
  });
  const [msg, setMsg] = useState("");
  const chatEnd = useRef<HTMLDivElement>(null);

  // Session brokering (PIN-protected link).
  const [sessionInfo, setSessionInfo] = useState<{ link: string; pin: string; code: string } | null>(null);
  const [joinInput, setJoinInput] = useState("");
  const [joinPin, setJoinPin] = useState("");

  // Audio settings (gear): device choice + volumes.
  const [showSettings, setShowSettings] = useState(false);
  const [devices, setDevices] = useState<{ inputs: string[]; outputs: string[] }>({ inputs: [], outputs: [] });
  const [audioCfg, setAudioCfg] = useState<{ input: string; output: string }>(() => {
    try {
      const s = localStorage.getItem("sa.audio");
      if (s) return JSON.parse(s);
    } catch {
      /* ignore */
    }
    return { input: "", output: "" };
  });
  const [masterVol, setMasterVol] = useState(100); // percent
  const [peerVol, setPeerVol] = useState<Record<string, number>>({});
  const [net, setNet] = useState<{ peers: number; up: number; down: number } | null>(null);
  const [keyInfo, setKeyInfo] = useState<{ gen: number; at: number }>({ gen: 1, at: 0 });
  const [rotating, setRotating] = useState(false);
  const [sigUp, setSigUp] = useState(true);
  const [resuming, setResuming] = useState(false);
  const [micMuted, setMicMuted] = useState(false);
  const micMutedRef = useRef(false);
  const [deaf, setDeaf] = useState(false);
  const [dsp, setDsp] = useState<DspConfig>(() => {
    try {
      return { ...DSP_DEFAULT, ...JSON.parse(localStorage.getItem("sa.dsp") || "{}") };
    } catch {
      return DSP_DEFAULT;
    }
  });
  const [monitoring, setMonitoring] = useState(false);
  const [update, setUpdate] = useState<{ version: string; notes: string } | null>(null);
  const [showUpdate, setShowUpdate] = useState(true);
  const [pttBinding, setPttBinding] = useState<string>(() => {
    try {
      return localStorage.getItem("sa.ptt") || "F8";
    } catch {
      return "F8";
    }
  });
  const [capturing, setCapturing] = useState(false);

  // Load device list once (for the gear settings).
  useEffect(() => {
    invoke<[string[], string[]]>("list_audio_devices")
      .then(([inputs, outputs]) => setDevices({ inputs, outputs }))
      .catch(() => {});
  }, []);
  const saveAudioCfg = (next: { input: string; output: string }) => {
    setAudioCfg(next);
    try {
      localStorage.setItem("sa.audio", JSON.stringify(next));
    } catch {
      /* ignore */
    }
  };
  const onMaster = (v: number) => {
    setMasterVol(v);
    invoke("set_master_volume", { volume: deaf ? 0 : v / 100 }).catch(() => {});
  };
  // Self-mute mic: stop sending now + gate PTT (I still hear everyone).
  const toggleMic = () => {
    setMicMuted((m) => {
      const nv = !m;
      micMutedRef.current = nv;
      if (nv) invoke("set_transmit", { on: false }).catch(() => {});
      return nv;
    });
  };
  // Deafen: mute all output without losing the slider value.
  const toggleDeaf = () => {
    setDeaf((d) => {
      const nv = !d;
      invoke("set_master_volume", { volume: nv ? 0 : masterVol / 100 }).catch(() => {});
      return nv;
    });
  };
  const toggleMonitor = () => {
    setMonitoring((m) => {
      const nv = !m;
      invoke("set_monitor", { on: nv }).catch(() => {});
      return nv;
    });
  };
  const onDisconnect = () => {
    invoke("disconnect").catch(() => {});
    setMonitoring(false);
    setMicMuted(false);
    micMutedRef.current = false;
    setDeaf(false);
    setShowSettings(false);
    // The engine emits Status{connected:false}, which returns us to the start screen.
  };
  const updateDsp = (patch: Partial<DspConfig>) => {
    setDsp((d) => {
      const nv = { ...d, ...patch };
      try {
        localStorage.setItem("sa.dsp", JSON.stringify(nv));
      } catch {
        /* ignore */
      }
      invoke("set_dsp", { cfg: nv }).catch(() => {});
      return nv;
    });
  };
  const onPeerVol = (userId: string, v: number) => {
    setPeerVol((m) => ({ ...m, [userId]: v }));
    invoke("set_peer_volume", { userId, volume: v / 100 }).catch(() => {});
  };

  useEffect(() => {
    const un = listen<UiEvent>("ui", (e) => {
      const p = e.payload;
      if (p.type === "roster") setParticipants(p.participants);
      else if (p.type === "chat") setChat((c) => [...c, { from: p.from, text: p.text }]);
      else if (p.type === "status") {
        setConnected(p.connected);
        setTransmitting(p.transmitting);
        if (p.connected) {
          setConnecting(false);
          setSigUp(true);
        }
      } else if (p.type === "log") setLog(p.text);
      else if (p.type === "net") setNet({ peers: p.peers, up: p.up_kbps, down: p.down_kbps });
      else if (p.type === "rekeyed") {
        setKeyInfo({ gen: p.generation, at: Date.now() });
        setRotating(false);
        setLog(`🔑 Schlüssel rotiert (Generation #${p.generation}${p.by ? `, durch ${p.by}` : ""})`);
      } else if (p.type === "signaling") {
        setSigUp(p.up);
        if (p.up) setResuming(false);
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    chatEnd.current?.scrollIntoView({ behavior: "smooth" });
  }, [chat]);

  // Configurable PTT via RAW global input (rdev in Rust). The bound key/mouse
  // button emits "ptt" (down/up); "ptt-bound" fires after a rebind capture.
  useEffect(() => {
    invoke("set_ptt_binding", { code: pttBinding }).catch(() => {});
    const offPtt = listen<boolean>("ptt", (e) => {
      if (micMutedRef.current) return; // self-muted: ignore push-to-talk
      invoke("set_transmit", { on: e.payload }).catch(() => {});
    });
    const offBound = listen<string>("ptt-bound", (e) => {
      setPttBinding(e.payload);
      setCapturing(false);
      try {
        localStorage.setItem("sa.ptt", e.payload);
      } catch {
        /* ignore */
      }
    });
    return () => {
      offPtt.then((f) => f());
      offBound.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  // Update check: compare the newest GitHub release (prereleases included) to the
  // running version; if newer, surface it with the changelog.
  useEffect(() => {
    (async () => {
      try {
        const cur = await getVersion();
        const r = await fetch(`https://api.github.com/repos/${REPO}/releases?per_page=5`, {
          headers: { Accept: "application/vnd.github+json" },
        });
        const rels = await r.json();
        const latest = Array.isArray(rels) ? rels.find((x: { draft?: boolean }) => !x.draft) : null;
        const lv: string | undefined = latest?.tag_name?.match(/(\d+\.\d+\.\d+)/)?.[1];
        if (!lv || !isNewer(lv, cur)) return;
        let notes: string = latest.body || "";
        try {
          const cl = await fetch(`https://raw.githubusercontent.com/${REPO}/main/CHANGELOG.md`);
          notes = topChangelogSection(await cl.text()) || notes;
        } catch {
          /* fall back to release body */
        }
        setUpdate({ version: lv, notes });
        setShowUpdate(true);
      } catch {
        /* offline / API error: silently skip */
      }
    })();
  }, []);

  // Push saved DSP settings to the engine once connected.
  useEffect(() => {
    if (connected) invoke("set_dsp", { cfg: dsp }).catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connected]);
  const rebindPtt = () => {
    setCapturing(true);
    invoke("start_ptt_capture").catch(() => {});
  };
  const rotateKey = () => {
    setRotating(true);
    invoke("rotate_key").catch(() => setRotating(false));
    // safety: clear the spinner even if no rekeyed event arrives
    setTimeout(() => setRotating(false), 8000);
  };
  const resumeSession = () => {
    setResuming(true);
    invoke("reconnect_session").catch(() => setResuming(false));
    setTimeout(() => setResuming(false), 8000);
  };

  const copy = (t: string) => navigator.clipboard?.writeText(t);

  // ── Session brokering (PIN-protected link via InitConnection REST) ──────────
  // The session service is the hosted public endpoint.
  const SESSION_BASE = "https://squadlink.raumdock.org";
  const parseCode = (s: string) => {
    const t = s.trim();
    const m = t.match(/\/j\/([A-Za-z0-9]+)/);
    return m ? m[1] : t;
  };
  const baseFromInput = (input: string) => {
    const t = input.trim();
    if (/^https?:\/\//.test(t)) {
      try {
        const u = new URL(t);
        return `${u.protocol}//${u.host}`;
      } catch {
        /* fall through */
      }
    }
    return SESSION_BASE;
  };
  const connectWith = async (ws: string, room: string, token: string | null) => {
    try {
      localStorage.setItem("sa.form", JSON.stringify(form));
    } catch {
      /* ignore */
    }
    await invoke("connect", {
      server: ws,
      room,
      userId: crypto.randomUUID().slice(0, 8),
      name: form.name.trim() || "Commander",
      token: token || null,
      certSha256: null,
      inputDevice: audioCfg.input || null,
      outputDevice: audioCfg.output || null,
    });
  };
  const createSession = async () => {
    setConnecting(true);
    setLog("");
    try {
      const r = await fetch(`${SESSION_BASE}/session`, { method: "POST" });
      if (!r.ok) throw new Error("Server " + r.status);
      const j = await r.json();
      setSessionInfo({ link: j.link, pin: j.pin, code: j.code });
      await connectWith(j.ws, j.room, j.token);
    } catch (e) {
      setLog(String(e));
      setConnecting(false);
    }
  };
  const joinSession = async () => {
    setConnecting(true);
    setLog("");
    try {
      const code = parseCode(joinInput);
      const base = baseFromInput(joinInput);
      const r = await fetch(`${base}/session/${encodeURIComponent(code)}/join`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ pin: joinPin.trim() }),
      });
      if (r.status === 403) throw new Error("Falsche PIN");
      if (r.status === 429) throw new Error("Zu viele Versuche — Session gesperrt");
      if (r.status === 404) throw new Error("Session nicht gefunden / abgelaufen");
      if (!r.ok) throw new Error("Server " + r.status);
      const j = await r.json();
      await connectWith(j.ws, j.room, j.token);
    } catch (e) {
      setLog(String(e));
      setConnecting(false);
    }
  };

  const ptt = () => invoke("toggle_transmit");
  const send = () => {
    const t = msg.trim();
    if (t) {
      invoke("send_chat", { text: t });
      setMsg("");
    }
  };

  const deviceSettings = (
    <div className="settings">
      <label>🎤 Mikrofon</label>
      <select value={audioCfg.input} onChange={(e) => saveAudioCfg({ ...audioCfg, input: e.target.value })}>
        <option value="">Standard-Gerät</option>
        {devices.inputs.map((d) => <option key={d} value={d}>{d}</option>)}
      </select>
      <label>🔊 Ausgabe</label>
      <select value={audioCfg.output} onChange={(e) => saveAudioCfg({ ...audioCfg, output: e.target.value })}>
        <option value="">Standard-Gerät</option>
        {devices.outputs.map((d) => <option key={d} value={d}>{d}</option>)}
      </select>
      <label>🎙 Push-to-Talk</label>
      <div className="pttrow">
        <span className="pttcur">{capturing ? "Drücke Taste / Maustaste…" : pttLabel(pttBinding)}</span>
        <button className="btn sm" onClick={rebindPtt} disabled={capturing}>Neu belegen</button>
      </div>
      <div className="sub2" style={{ opacity: 0.7 }}>
        Push-to-Talk: jede Taste oder Maustaste (RAW). Geräteänderung wird beim nächsten Verbinden aktiv.
      </div>

      <label>🎧 Mikrofon-Test</label>
      <button className={`btn sm ${monitoring ? "primary" : ""}`} onClick={toggleMonitor}>
        {monitoring ? "■ Test stoppen" : "▶ Eigenwiedergabe"}
      </button>
      <div className="sub2" style={{ opacity: 0.7 }}>
        Hörst dein eigenes Mikrofon (inkl. Aufbereitung). Headset empfohlen (sonst Rückkopplung).
      </div>

      <label>🎚 Audio-Aufbereitung</label>
      <div className="dsp">
        <div className="dsphead">
          <label className="chk"><input type="checkbox" checked={dsp.gate} onChange={(e) => updateDsp({ gate: e.target.checked })} /> Noise Gate</label>
        </div>
        <div className="dsprow">
          <span>Schwelle</span>
          <input type="range" min={0} max={80} value={Math.round(dsp.gate_threshold * 1000)} disabled={!dsp.gate} onChange={(e) => updateDsp({ gate_threshold: Number(e.target.value) / 1000 })} />
          <span className="vval">{Math.round(dsp.gate_threshold * 1000)}</span>
        </div>

        <div className="dsphead">
          <label className="chk"><input type="checkbox" checked={dsp.compressor} onChange={(e) => updateDsp({ compressor: e.target.checked })} /> Kompressor</label>
        </div>
        <div className="dsprow">
          <span>Schwelle</span>
          <input type="range" min={5} max={50} value={Math.round(dsp.comp_threshold * 100)} disabled={!dsp.compressor} onChange={(e) => updateDsp({ comp_threshold: Number(e.target.value) / 100 })} />
          <span className="vval">{Math.round(dsp.comp_threshold * 100)}</span>
        </div>
        <div className="dsprow">
          <span>Ratio</span>
          <input type="range" min={10} max={100} value={Math.round(dsp.comp_ratio * 10)} disabled={!dsp.compressor} onChange={(e) => updateDsp({ comp_ratio: Number(e.target.value) / 10 })} />
          <span className="vval">{(dsp.comp_ratio).toFixed(1)}:1</span>
        </div>
        <div className="dsprow">
          <span>Makeup</span>
          <input type="range" min={10} max={30} value={Math.round(dsp.comp_makeup * 10)} disabled={!dsp.compressor} onChange={(e) => updateDsp({ comp_makeup: Number(e.target.value) / 10 })} />
          <span className="vval">{(dsp.comp_makeup).toFixed(1)}×</span>
        </div>

        <div className="dsphead">
          <label className="chk"><input type="checkbox" checked={dsp.limiter} onChange={(e) => updateDsp({ limiter: e.target.checked })} /> Limiter (gegen Knacken)</label>
        </div>
        <div className="dsprow">
          <span>Ceiling</span>
          <input type="range" min={50} max={100} value={Math.round(dsp.limiter_ceiling * 100)} disabled={!dsp.limiter} onChange={(e) => updateDsp({ limiter_ceiling: Number(e.target.value) / 100 })} />
          <span className="vval">{Math.round(dsp.limiter_ceiling * 100)}%</span>
        </div>
      </div>
    </div>
  );
  const rotatedAt = keyInfo.at
    ? new Date(keyInfo.at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : null;
  const encFooter = (
    <div className="encfoot">
      🔒 Encryption: <b>DTLS-SRTP</b> (Audio) · <b>DTLS-SCTP</b> (Chat) · <b>TLS/wss</b> (Signaling)
      — Ende-zu-Ende P2P, encrypted by default &amp; by session
      <span className="keygen">
        · Schlüssel-Generation <b>#{keyInfo.gen}</b>
        {rotatedAt ? ` (rotiert ${rotatedAt})` : ""}
      </span>
    </div>
  );

  const updateBanner =
    update && showUpdate ? (
      <div className="updbar">
        <div className="updhead">
          <b>⬆ Neue Version {update.version} verfügbar</b>
          <button className="x" title="Schließen" onClick={() => setShowUpdate(false)}>×</button>
        </div>
        <pre className="updnotes">{update.notes}</pre>
        <div className="updact">
          <button className="btn primary" onClick={() => invoke("open_download").catch(() => {})}>Herunterladen</button>
          <button className="btn" onClick={() => setShowUpdate(false)}>Später</button>
        </div>
      </div>
    ) : null;

  if (!connected) {
    return (
      <div className="screen center">
        {updateBanner}
        <div className="card connect">
          <div className="brandrow">
            <div className="brandwrap">
              <img src={logo} className="applogo" alt="" />
              <div className="brand">RDOC <span>// SQUADLINK LITE</span></div>
            </div>
            <button className="gear" title="Audio-Einstellungen" onClick={() => setShowSettings((s) => !s)}>⚙</button>
          </div>
          <div className="sub">P2P Voice + Chat</div>
          {showSettings && deviceSettings}

          <label>Name</label>
          <input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="Commander" />

          <div className="session">
            <div className="sub2">
              <b>Host:</b> Session erstellen → <b>Link + 6-stellige PIN</b> an die Mitspieler geben.
              <br /><b>Mitspieler:</b> Link/Code + PIN eingeben — komplett ohne Konfiguration.
            </div>
            <button className="btn primary" onClick={createSession} disabled={connecting}>
              {connecting ? "…" : "SESSION ERSTELLEN (HOST)"}
            </button>
            {sessionInfo && (
              <div className="sessbox">
                <label>Link — an Mitspieler</label>
                <input readOnly value={sessionInfo.link} className="mono" onFocus={(e) => e.currentTarget.select()} />
                <label>PIN — separat weitergeben</label>
                <div className="pin mono">{sessionInfo.pin}</div>
                <button className="btn sm" onClick={() => copy(`${sessionInfo.link}\nPIN: ${sessionInfo.pin}`)}>
                  LINK + PIN KOPIEREN
                </button>
              </div>
            )}
            <div className="sub2" style={{ marginTop: "1rem", opacity: 0.7 }}>— oder beitreten —</div>
            <label>Link oder Code</label>
            <input value={joinInput} onChange={(e) => setJoinInput(e.target.value)} placeholder="https://…/j/abc oder abc" className="mono" spellCheck={false} />
            <label>PIN (6-stellig)</label>
            <input value={joinPin} onChange={(e) => setJoinPin(e.target.value)} inputMode="numeric" maxLength={6} placeholder="123456" />
            <button className="btn primary" onClick={joinSession} disabled={connecting || !joinInput.trim() || joinPin.trim().length < 6}>
              {connecting ? "VERBINDE…" : "BEITRETEN"}
            </button>
          </div>
          {log && <div className="err">{log}</div>}
          {encFooter}
        </div>
      </div>
    );
  }

  const estPeers = participants.filter((p) => !p.you && p.badge).length;
  const p2pCount = net?.peers ?? estPeers;
  const up = net ? net.up : estPeers * 32;
  const down = net ? net.down : estPeers * 32;
  const measured = net != null;

  return (
    <div className="screen app">
      <header>
        <div className="brand sm">RDOC <span>// SQUADLINK LITE</span></div>
        <div className={`dot ${transmitting ? "tx" : "ok"}`} />
        <div className="hstatus">{transmitting ? "SENDEN" : "VERBUNDEN"}</div>
        <button className="gear" title="Audio-Einstellungen" onClick={() => setShowSettings((s) => !s)}>⚙</button>
        <button className="leave" title="Session verlassen" onClick={onDisconnect}>Verlassen</button>
      </header>
      {updateBanner}
      {showSettings && deviceSettings}

      {!sigUp && (
        <div className="sigbanner">
          <span>⚠ Signaling getrennt — P2P-Audio läuft weiter.</span>
          <button className="btn sm" onClick={resumeSession} disabled={resuming}>
            {resuming ? "Verbinde…" : "Session wiederaufnehmen"}
          </button>
        </div>
      )}

      <div className="netbar">
        <span>P2P: <b>{p2pCount}</b></span>
        <span>↑ {measured ? "" : "~"}{up} kbps</span>
        <span>↓ {measured ? "" : "~"}{down} kbps</span>
        <span className="netest">({measured ? "gemessen" : "geschätzt"})</span>
        <button
          className="rekey"
          title="Erzeugt für alle Teilnehmer neue Verschlüsselungs-Keys (DTLS-SRTP re-handshake)"
          onClick={rotateKey}
          disabled={rotating}
        >
          {rotating ? "⏳ Rotiere…" : `🔑 Key rotieren · #${keyInfo.gen}`}
        </button>
      </div>

      <div className="volrow">
        <span className="vlabel">🔊 Gesamt</span>
        <input type="range" min={0} max={100} value={masterVol} onChange={(e) => onMaster(Number(e.target.value))} />
        <span className="vval">{masterVol}%</span>
      </div>

      <main>
        <section className="roster">
          {sessionInfo && (
            <div className="sessbox sessbox-live">
              <div className="hsec">Session teilen</div>
              <input readOnly value={sessionInfo.link} className="mono" onFocus={(e) => e.currentTarget.select()} />
              <div className="pinrow">
                <span className="pin mono">PIN {sessionInfo.pin}</span>
                <button className="btn sm" onClick={() => copy(`${sessionInfo.link}\nPIN: ${sessionInfo.pin}`)}>
                  LINK + PIN
                </button>
              </div>
            </div>
          )}
          <div className="hsec">Teilnehmer · {participants.length}</div>
          {participants.map((p) => (
            <div key={p.user_id} className={`peer ${p.speaking ? "speaking" : ""}`}>
              <div className="peerhead">
                <span className={`talk ${p.speaking ? "on" : ""}`} />
                <span className="pname">
                  {p.name}
                  {p.you && <span className="me"> (du)</span>}
                </span>
                {p.badge && (
                  <span className={`badge ${p.badge.includes("RELAY") ? "relay" : "direct"}`}>
                    {p.badge}
                  </span>
                )}
              </div>
              {!p.you && (
                <div className="peervol">
                  <span className="vmini">🔊</span>
                  <input
                    type="range"
                    min={0}
                    max={100}
                    value={peerVol[p.user_id] ?? 100}
                    onChange={(e) => onPeerVol(p.user_id, Number(e.target.value))}
                  />
                  <span className="vval">{peerVol[p.user_id] ?? 100}%</span>
                </div>
              )}
            </div>
          ))}
          <button className={`ptt ${transmitting ? "live" : ""} ${micMuted ? "muted" : ""}`} onClick={ptt} disabled={micMuted}>
            {micMuted ? "🔇 MIKRO STUMM" : transmitting ? "● SENDEN AKTIV" : "PUSH TO TALK"}
            <span className="ptthint">{pttLabel(pttBinding)} halten · oder klick zum Umschalten</span>
          </button>
          <div className="selfctl">
            <button className={`ctl ${transmitting ? "on" : ""}`} onClick={ptt} disabled={micMuted} title="Dauersenden ein/aus">
              {transmitting ? "🟢 Sendet (Toggle)" : "🔘 Toggle senden"}
            </button>
            <button className={`ctl ${micMuted ? "on" : ""}`} onClick={toggleMic} title="Eigenes Mikrofon stummschalten (du hörst weiter)">
              {micMuted ? "🔇 Mikro stumm" : "🎙️ Mikro an"}
            </button>
            <button className={`ctl ${deaf ? "on" : ""}`} onClick={toggleDeaf} title="Ton aus (nichts hören)">
              {deaf ? "🔕 Ton aus" : "🔊 Ton an"}
            </button>
          </div>
        </section>

        <section className="chat">
          <div className="hsec">Chat</div>
          <div className="chatlog">
            {chat.map((c, i) => (
              <div key={i} className="line">
                <span className="from">{c.from}</span>
                <span className="text">{c.text}</span>
              </div>
            ))}
            <div ref={chatEnd} />
          </div>
          <div className="chatin">
            <input
              value={msg}
              onChange={(e) => setMsg(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && send()}
              placeholder="Nachricht an alle…"
            />
            <button className="btn" onClick={send}>SENDEN</button>
          </div>
        </section>
      </main>
      {log && <div className="footlog">{log}</div>}
      {encFooter}
    </div>
  );
}
