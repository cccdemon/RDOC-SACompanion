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
    return { server: "ws://127.0.0.1:8080/ws", room: "op1", name: "", token: "", certSha256: "" };
  });
  const [msg, setMsg] = useState("");
  const chatEnd = useRef<HTMLDivElement>(null);

  // Serverless 1:1 (copy-paste SDP) state.
  const [mode, setMode] = useState<"server" | "serverless">("server");
  const [srole, setSrole] = useState<"a" | "b" | null>(null);
  const [offerOut, setOfferOut] = useState("");
  const [answerIn, setAnswerIn] = useState("");
  const [offerIn, setOfferIn] = useState("");
  const [answerOut, setAnswerOut] = useState("");
  const [busy, setBusy] = useState(false);

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

  const onConnect = async () => {
    setLog("");
    setConnecting(true);
    try {
      localStorage.setItem("sa.form", JSON.stringify(form));
    } catch {
      /* ignore */
    }
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

  const copy = (t: string) => navigator.clipboard?.writeText(t);
  const slOffer = async () => {
    setBusy(true);
    setLog("");
    try {
      const c = await invoke<string>("serverless_offer", { name: form.name.trim() || "Commander" });
      setOfferOut(c);
    } catch (e) {
      setLog(String(e));
    }
    setBusy(false);
  };
  const slAcceptAnswer = async () => {
    try {
      await invoke("serverless_accept_answer", { code: answerIn.trim() });
    } catch (e) {
      setLog(String(e));
    }
  };
  const slAcceptOffer = async () => {
    setBusy(true);
    setLog("");
    try {
      const a = await invoke<string>("serverless_accept_offer", {
        name: form.name.trim() || "Commander",
        code: offerIn.trim(),
      });
      setAnswerOut(a);
    } catch (e) {
      setLog(String(e));
    }
    setBusy(false);
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

          <div className="tabs">
            <button className={mode === "server" ? "tab on" : "tab"} onClick={() => setMode("server")}>SERVER</button>
            <button className={mode === "serverless" ? "tab on" : "tab"} onClick={() => setMode("serverless")}>SERVERLESS</button>
          </div>

          <label>Name</label>
          <input value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="Commander" />

          {mode === "server" ? (
            <>
              <label>Server</label>
              <input value={form.server} onChange={(e) => setForm({ ...form, server: e.target.value })} spellCheck={false} className="mono" />
              <label>Room</label>
              <input value={form.room} onChange={(e) => setForm({ ...form, room: e.target.value })} spellCheck={false} />
              <label>Room-Token <span className="opt">(optional)</span></label>
              <input value={form.token} onChange={(e) => setForm({ ...form, token: e.target.value })} spellCheck={false} />
              <label>CERT_SHA256 <span className="opt">(nur für wss://)</span></label>
              <input value={form.certSha256} onChange={(e) => setForm({ ...form, certSha256: e.target.value })} spellCheck={false} className="mono" />
              <button className="btn primary" onClick={onConnect} disabled={connecting}>
                {connecting ? "VERBINDE…" : "VERBINDEN"}
              </button>
            </>
          ) : (
            <div className="serverless">
              <div className="sub2">Kein Server — SDP-Codes per Discord/Chat austauschen. STUN für NAT; hartes NAT ohne TURN = evtl. kein Connect.</div>
              {!srole && (
                <div className="srole">
                  <button className="btn" onClick={() => setSrole("a")}>ANRUF STARTEN (A)</button>
                  <button className="btn" onClick={() => setSrole("b")}>ANRUF ANNEHMEN (B)</button>
                </div>
              )}
              {srole === "a" && (
                <>
                  {!offerOut ? (
                    <button className="btn primary" onClick={slOffer} disabled={busy}>{busy ? "ERZEUGE…" : "1) OFFER ERZEUGEN"}</button>
                  ) : (
                    <>
                      <label>Dein Offer-Code → an Peer schicken</label>
                      <textarea readOnly value={offerOut} className="mono code" />
                      <button className="btn sm" onClick={() => copy(offerOut)}>KOPIEREN</button>
                      <label>2) Answer-Code vom Peer einfügen</label>
                      <textarea value={answerIn} onChange={(e) => setAnswerIn(e.target.value)} className="mono code" placeholder="Answer-Code…" />
                      <button className="btn primary" onClick={slAcceptAnswer} disabled={!answerIn.trim()}>VERBINDEN</button>
                    </>
                  )}
                  <button className="btn ghost sm" onClick={() => { setSrole(null); setOfferOut(""); setAnswerIn(""); }}>zurück</button>
                </>
              )}
              {srole === "b" && (
                <>
                  <label>1) Offer-Code vom Peer einfügen</label>
                  <textarea value={offerIn} onChange={(e) => setOfferIn(e.target.value)} className="mono code" placeholder="Offer-Code…" />
                  {!answerOut ? (
                    <button className="btn primary" onClick={slAcceptOffer} disabled={busy || !offerIn.trim()}>{busy ? "…" : "ANNEHMEN"}</button>
                  ) : (
                    <>
                      <label>2) Dein Answer-Code → zurück an Peer</label>
                      <textarea readOnly value={answerOut} className="mono code" />
                      <button className="btn sm" onClick={() => copy(answerOut)}>KOPIEREN</button>
                      <div className="sub2">Sobald der Peer den Code einfügt, verbindet ihr euch.</div>
                    </>
                  )}
                  <button className="btn ghost sm" onClick={() => { setSrole(null); setOfferIn(""); setAnswerOut(""); }}>zurück</button>
                </>
              )}
            </div>
          )}
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
