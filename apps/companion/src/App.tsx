import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { register, unregister } from "@tauri-apps/plugin-global-shortcut";

const PTT_KEY = "F8";

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
  | { type: "log"; text: string };

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
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    chatEnd.current?.scrollIntoView({ behavior: "smooth" });
  }, [chat]);

  // Hold-to-talk global hotkey while connected: press = transmit, release = stop.
  useEffect(() => {
    if (!connected) return;
    register(PTT_KEY, (e: { state: string }) => {
      invoke("set_transmit", { on: e.state === "Pressed" });
    }).catch(() => {});
    return () => {
      unregister(PTT_KEY).catch(() => {});
    };
  }, [connected]);

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

  if (!connected) {
    return (
      <div className="screen center">
        <div className="card connect">
          <div className="brand">RDOC <span>// SQUADLINK LITE</span></div>
          <div className="sub">P2P Voice + Chat</div>

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
        </div>
      </div>
    );
  }

  return (
    <div className="screen app">
      <header>
        <div className="brand sm">RDOC <span>// SQUADLINK LITE</span></div>
        <div className={`dot ${transmitting ? "tx" : "ok"}`} />
        <div className="hstatus">{transmitting ? "SENDEN" : "VERBUNDEN"}</div>
      </header>

      <main>
        <section className="roster">
          <div className="hsec">Teilnehmer · {participants.length}</div>
          {participants.map((p) => (
            <div key={p.user_id} className={`peer ${p.speaking ? "speaking" : ""}`}>
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
          ))}
          <button className={`ptt ${transmitting ? "live" : ""}`} onClick={ptt}>
            {transmitting ? "● SENDEN AKTIV" : "PUSH TO TALK"}
            <span className="ptthint">{PTT_KEY} halten · oder klick zum Umschalten</span>
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
    </div>
  );
}
