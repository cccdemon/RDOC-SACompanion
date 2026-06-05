import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

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
  const [form, setForm] = useState({
    server: "ws://127.0.0.1:8080/ws",
    room: "op1",
    name: "",
    token: "",
    certSha256: "",
  });
  const [msg, setMsg] = useState("");
  const chatEnd = useRef<HTMLDivElement>(null);

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

  const onConnect = async () => {
    setLog("");
    setConnecting(true);
    const name = form.name.trim() || "Commander";
    const userId = crypto.randomUUID().slice(0, 8);
    try {
      await invoke("connect", {
        server: form.server.trim(),
        room: form.room.trim(),
        userId,
        name,
        token: form.token.trim() || null,
        certSha256: form.certSha256.trim() || null,
      });
    } catch (err) {
      setLog(String(err));
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
          <div className="brand">RDOC <span>// SACOMPANION</span></div>
          <div className="sub">Serverless P2P Voice-Mesh</div>
          <label>Server</label>
          <input value={form.server} onChange={(e) => setForm({ ...form, server: e.target.value })} spellCheck={false} />
          <label>Room</label>
          <input value={form.room} onChange={(e) => setForm({ ...form, room: e.target.value })} spellCheck={false} />
          <label>Name</label>
          <input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="Commander" />
          <label>Room-Token <span className="opt">(optional)</span></label>
          <input value={form.token} onChange={(e) => setForm({ ...form, token: e.target.value })} spellCheck={false} />
          <label>CERT_SHA256 <span className="opt">(nur für wss://)</span></label>
          <input value={form.certSha256} onChange={(e) => setForm({ ...form, certSha256: e.target.value })} spellCheck={false} className="mono" />
          <button className="btn primary" onClick={onConnect} disabled={connecting}>
            {connecting ? "VERBINDE…" : "VERBINDEN"}
          </button>
          {log && <div className="err">{log}</div>}
        </div>
      </div>
    );
  }

  return (
    <div className="screen app">
      <header>
        <div className="brand sm">RDOC <span>// SACOMPANION</span></div>
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
            {transmitting ? "● SENDEN AKTIV — klick zum Stoppen" : "PUSH TO TALK — klick zum Senden"}
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
