//! Native audio: cpal capture/playback + linear resampling around 48 kHz Opus.
//! Device thread owns the cpal streams (kept off the async runtime). Encode/
//! decode/mix run on plain std threads (audiopus stays out of tokio).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use audiopus::coder::{Decoder, Encoder};
use audiopus::{Application, Channels, SampleRate};
use bytes::Bytes;
use nnnoiseless::DenoiseState;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

pub const OPUS_SR: u32 = 48000;
pub const FRAME: usize = 960; // 20 ms mono @ 48 kHz

pub type Buf = Arc<Mutex<VecDeque<i16>>>;
pub type MixMap = Arc<Mutex<HashMap<String, VecDeque<i16>>>>;

/// Output gains: one master + per-peer (user_id → factor). 1.0 = unchanged.
/// Applied live in the mixer; both clamp to 0.0..2.0 (0 = mute, 2 = +6 dB).
#[derive(Default)]
pub struct Gains {
    master: Mutex<Option<f32>>, // None ⇒ 1.0
    peers: Mutex<HashMap<String, f32>>,
}
impl Gains {
    pub fn new() -> Self {
        Gains { master: Mutex::new(Some(1.0)), peers: Mutex::new(HashMap::new()) }
    }
    pub fn set_master(&self, v: f32) {
        *self.master.lock().unwrap() = Some(v.clamp(0.0, 2.0));
    }
    pub fn set_peer(&self, peer: &str, v: f32) {
        self.peers.lock().unwrap().insert(peer.to_string(), v.clamp(0.0, 2.0));
    }
    fn master_v(&self) -> f32 {
        self.master.lock().unwrap().unwrap_or(1.0)
    }
    fn peer_v(&self, peer: &str) -> f32 {
        *self.peers.lock().unwrap().get(peer).unwrap_or(&1.0)
    }
}

/// List input + output device names for the settings UI.
pub fn list_devices() -> (Vec<String>, Vec<String>) {
    let host = cpal::default_host();
    let ins = host
        .input_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    let outs = host
        .output_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default();
    (ins, outs)
}

/// Streaming linear resampler (mono f32), phase preserved across calls.
pub struct Resampler {
    step: f64,
    t: f64,
    prev: f32,
    have_prev: bool,
}
impl Resampler {
    pub fn new(src: u32, dst: u32) -> Self {
        Self { step: src as f64 / dst as f64, t: 0.0, prev: 0.0, have_prev: false }
    }
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        for &cur in input {
            if !self.have_prev {
                self.prev = cur;
                self.have_prev = true;
                continue;
            }
            while self.t < 1.0 {
                out.push(self.prev + (cur - self.prev) * self.t as f32);
                self.t += self.step;
            }
            self.t -= 1.0;
            self.prev = cur;
        }
    }
}

struct Picked {
    device: cpal::Device,
    config: StreamConfig,
    fmt: SampleFormat,
    channels: u16,
    rate: u32,
}

fn choose(host: &cpal::Host, input: bool, want: Option<&str>) -> Result<Picked> {
    let kind = if input { "Input" } else { "Output" };
    let devs: Vec<cpal::Device> =
        if input { host.input_devices()?.collect() } else { host.output_devices()?.collect() };
    let device = if let Some(w) = want.filter(|s| !s.is_empty()) {
        devs.iter()
            .find(|d| d.name().map(|n| n.contains(w)).unwrap_or(false))
            .cloned()
            .ok_or_else(|| anyhow!("{kind}-Device '{w}' nicht gefunden"))?
    } else if input {
        host.default_input_device().ok_or_else(|| anyhow!("kein Default-Input"))?
    } else {
        host.default_output_device().ok_or_else(|| anyhow!("kein Default-Output"))?
    };
    let s = if input { device.default_input_config()? } else { device.default_output_config()? };
    Ok(Picked {
        config: s.config(),
        fmt: s.sample_format(),
        channels: s.channels(),
        rate: s.sample_rate().0,
        device,
    })
}

fn build_input(p: &Picked, cap: Buf) -> Result<cpal::Stream> {
    let ch = p.channels as usize;
    let err = |e| eprintln!("input stream error: {e}");
    Ok(match p.fmt {
        SampleFormat::F32 => p.device.build_input_stream(
            &p.config,
            move |data: &[f32], _: &_| {
                let mut b = cap.lock().unwrap();
                for fr in data.chunks(ch) {
                    let s: f32 = fr.iter().copied().sum::<f32>() / ch as f32;
                    b.push_back((s.clamp(-1.0, 1.0) * 32767.0) as i16);
                }
            },
            err,
            None,
        )?,
        SampleFormat::I16 => p.device.build_input_stream(
            &p.config,
            move |data: &[i16], _: &_| {
                let mut b = cap.lock().unwrap();
                for fr in data.chunks(ch) {
                    let s: i32 = fr.iter().map(|&x| x as i32).sum::<i32>() / ch as i32;
                    b.push_back(s as i16);
                }
            },
            err,
            None,
        )?,
        other => return Err(anyhow!("Input-Format {other:?} nicht unterstützt")),
    })
}

fn build_output(p: &Picked, play: Buf) -> Result<cpal::Stream> {
    let ch = p.channels as usize;
    let err = |e| eprintln!("output stream error: {e}");
    Ok(match p.fmt {
        SampleFormat::F32 => p.device.build_output_stream(
            &p.config,
            move |data: &mut [f32], _: &_| {
                let mut b = play.lock().unwrap();
                for fr in data.chunks_mut(ch) {
                    let v = b.pop_front().unwrap_or(0) as f32 / 32768.0;
                    for o in fr.iter_mut() {
                        *o = v;
                    }
                }
            },
            err,
            None,
        )?,
        SampleFormat::I16 => p.device.build_output_stream(
            &p.config,
            move |data: &mut [i16], _: &_| {
                let mut b = play.lock().unwrap();
                for fr in data.chunks_mut(ch) {
                    let v = b.pop_front().unwrap_or(0);
                    for o in fr.iter_mut() {
                        *o = v;
                    }
                }
            },
            err,
            None,
        )?,
        other => return Err(anyhow!("Output-Format {other:?} nicht unterstützt")),
    })
}

/// Device thread: pick devices, build + play streams, report rates, then park
/// (cpal streams must outlive the program and stay off the async runtime).
pub fn run_devices(
    cap: Buf,
    play: Buf,
    rate_tx: std::sync::mpsc::Sender<(u32, u32)>,
    in_name: Option<String>,
    out_name: Option<String>,
) {
    let host = cpal::default_host();
    let in_want = in_name.or_else(|| std::env::var("IN_DEVICE").ok());
    let out_want = out_name.or_else(|| std::env::var("OUT_DEVICE").ok());
    let inp = choose(&host, true, in_want.as_deref()).expect("Input-Device");
    let outp = choose(&host, false, out_want.as_deref()).expect("Output-Device");
    eprintln!(
        "Input : {} @ {}Hz | Output: {} @ {}Hz",
        inp.device.name().unwrap_or_default(),
        inp.rate,
        outp.device.name().unwrap_or_default(),
        outp.rate
    );
    let _ = rate_tx.send((inp.rate, outp.rate));
    let in_s = build_input(&inp, cap).expect("input stream");
    let out_s = build_output(&outp, play).expect("output stream");
    in_s.play().expect("play input");
    out_s.play().expect("play output");
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// Simple feed-forward dynamics compressor (mono, −1..1). Smooths loud/quiet
/// swings so everyone sits at a similar level. On by default in the capture path.
struct Compressor {
    env: f32,
    atk: f32,
    rel: f32,
    thr: f32,
    ratio: f32,
    makeup: f32,
}
impl Compressor {
    fn new() -> Self {
        Compressor {
            env: 0.0,
            atk: (-1.0f32 / (0.005 * OPUS_SR as f32)).exp(), // ~5 ms attack
            rel: (-1.0f32 / (0.080 * OPUS_SR as f32)).exp(), // ~80 ms release
            thr: 0.15,                                        // ~ −16 dBFS
            ratio: 3.0,
            makeup: 1.8,
        }
    }
    fn process(&mut self, x: f32) -> f32 {
        let a = x.abs();
        let coef = if a > self.env { self.atk } else { self.rel };
        self.env = coef * self.env + (1.0 - coef) * a;
        let gain = if self.env > self.thr {
            (self.thr + (self.env - self.thr) / self.ratio) / self.env.max(1e-6)
        } else {
            1.0
        };
        (x * gain * self.makeup).clamp(-1.0, 1.0)
    }
}

/// Capture → resample(in→48k) → RNNoise noise-suppression (10ms blocks) →
/// compressor → 20ms frame → (if transmitting) Opus encode → WebRTC writer task.
///
/// RNNoise removes background noise (fan, keyboard, hum). It is NOT echo
/// cancellation — without a headset, speaker echo still leaks; full APM-AEC
/// (libwebrtc) doesn't build on Windows-MSVC, so headset stays recommended.
pub fn encode_loop(cap: Buf, in_rate: u32, transmit: Arc<AtomicBool>, opus_tx: UnboundedSender<Bytes>) {
    const NS: usize = DenoiseState::FRAME_SIZE; // 480 = 10ms @ 48k
    let enc = Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip)
        .expect("opus encoder");
    let mut up = Resampler::new(in_rate, OPUS_SR);
    let mut den = DenoiseState::new();
    let mut comp = Compressor::new();
    let mut buf48: Vec<f32> = Vec::new(); // post-resample, −1..1
    let mut den_in = [0f32; NS];
    let mut den_out = [0f32; NS];
    let mut clean: Vec<f32> = Vec::new(); // post-denoise, −1..1
    let mut frame = [0i16; FRAME];
    let mut encoded = [0u8; 4000];
    loop {
        let chunk: Vec<f32> = {
            let mut b = cap.lock().unwrap();
            if b.is_empty() {
                Vec::new()
            } else {
                b.drain(..).map(|s| s as f32 / 32768.0).collect()
            }
        };
        if chunk.is_empty() {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        up.process(&chunk, &mut buf48);
        // Denoise in 10ms blocks. RNNoise works in i16-scaled f32.
        while buf48.len() >= NS {
            for (i, s) in buf48.drain(..NS).enumerate() {
                den_in[i] = s * 32768.0;
            }
            den.process_frame(&mut den_out, &den_in);
            for s in den_out.iter() {
                clean.push((s / 32768.0).clamp(-1.0, 1.0));
            }
        }
        while clean.len() >= FRAME {
            for (i, s) in clean.drain(..FRAME).enumerate() {
                frame[i] = (comp.process(s) * 32767.0) as i16;
            }
            if transmit.load(Ordering::SeqCst) {
                if let Ok(n) = enc.encode(&frame[..], &mut encoded[..]) {
                    let _ = opus_tx.send(Bytes::copy_from_slice(&encoded[..n]));
                }
            }
        }
    }
}

/// Decode incoming Opus frames (per peer) → push i16 @48k into the mix map.
pub fn decode_loop(mut rx: UnboundedReceiver<(String, Bytes)>, mix: MixMap) {
    let mut decoders: HashMap<String, Decoder> = HashMap::new();
    let mut out = [0i16; FRAME];
    while let Some((peer, payload)) = rx.blocking_recv() {
        let dec = decoders
            .entry(peer.clone())
            .or_insert_with(|| Decoder::new(SampleRate::Hz48000, Channels::Mono).expect("opus decoder"));
        if let Ok(n) = dec.decode(Some(&payload[..]), &mut out[..], false) {
            let mut m = mix.lock().unwrap();
            let b = m.entry(peer).or_default();
            for s in &out[..n] {
                b.push_back(*s);
            }
            while b.len() > 19200 {
                b.pop_front(); // cap ~400ms jitter
            }
        }
    }
}

/// 20ms clock: sum each peer's 48k frame (int16 clamp), resample 48k→out, push
/// to the playback ring.
pub fn mixer_loop(mix: MixMap, play: Buf, out_rate: u32, gains: Arc<Gains>) {
    let mut down = Resampler::new(OPUS_SR, out_rate);
    loop {
        std::thread::sleep(Duration::from_millis(20));
        let mut mixed = [0i32; FRAME];
        let mut any = false;
        {
            let mut m = mix.lock().unwrap();
            for (peer, b) in m.iter_mut() {
                // Take up to one frame; a partially-filled peer contributes what
                // it has (rest = silence) instead of being dropped → smoother
                // mix with several simultaneous speakers. Apply this peer's gain.
                let n = b.len().min(FRAME);
                if n > 0 {
                    any = true;
                    let g = gains.peer_v(peer);
                    for x in mixed.iter_mut().take(n) {
                        *x += (b.pop_front().unwrap() as f32 * g) as i32;
                    }
                }
            }
        }
        if !any {
            continue;
        }
        let master = gains.master_v();
        let f: Vec<f32> =
            mixed.iter().map(|&v| (v as f32 * master).clamp(-32768.0, 32767.0) / 32768.0).collect();
        let mut o: Vec<f32> = Vec::new();
        down.process(&f, &mut o);
        let mut p = play.lock().unwrap();
        for v in o {
            p.push_back((v.clamp(-1.0, 1.0) * 32767.0) as i16);
        }
        let cap = out_rate as usize / 2; // ~0.5s playback cap
        while p.len() > cap {
            p.pop_front();
        }
    }
}
