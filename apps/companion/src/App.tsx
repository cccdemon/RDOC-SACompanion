import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

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
  | { type: "net"; peers: number; up_kbps: number; down_kbps: number };

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
    invoke("set_master_volume", { volume: v / 100 }).catch(() => {});
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
        if (p.connected) setConnecting(false);
      } else if (p.type === "log") setLog(p.text);
      else if (p.type === "net") setNet({ peers: p.peers, up: p.up_kbps, down: p.down_kbps });
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
  const rebindPtt = () => {
    setCapturing(true);
    invoke("start_ptt_capture").catch(() => {});
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
    </div>
  );
  const encFooter = (
    <div className="encfoot">
      🔒 Encryption: <b>DTLS-SRTP</b> (Audio) · <b>DTLS-SCTP</b> (Chat) · <b>TLS/wss</b> (Signaling)
      — Ende-zu-Ende P2P, encrypted by default &amp; by session
    </div>
  );

  if (!connected) {
    return (
      <div className="screen center">
        <div className="card connect">
          <div className="brandrow">
            <div className="brand">RDOC <span>// SQUADLINK LITE</span></div>
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
      </header>
      {showSettings && deviceSettings}

      <div className="netbar">
        <span>P2P: <b>{p2pCount}</b></span>
        <span>↑ {measured ? "" : "~"}{up} kbps</span>
        <span>↓ {measured ? "" : "~"}{down} kbps</span>
        <span className="netest">({measured ? "gemessen" : "geschätzt"})</span>
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
          <button className={`ptt ${transmitting ? "live" : ""}`} onClick={ptt}>
            {transmitting ? "● SENDEN AKTIV" : "PUSH TO TALK"}
            <span className="ptthint">{pttLabel(pttBinding)} halten · oder klick zum Umschalten</span>
          </button>
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
