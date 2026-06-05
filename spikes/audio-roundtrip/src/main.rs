//! Phase-0 Mini-Spike — native Audio-Roundtrip (mit Resampling).
//!
//! cpal capture (Mic, native rate) -> mono -> linear resample -> 48k
//! -> Opus encode (20ms) -> Opus decode -> linear resample -> out-rate
//! -> cpal playback. Hörst du dich, hält der native Stack auf DEINER Hardware.
//!
//! Wichtig: Opus ist fix 48 kHz, Geräte laufen aber beliebig (dein FiiO @192k).
//! Darum Resampler um Opus herum. Hier linear (reicht zum Hören); Produktion
//! nutzt `rubato` (Sinc, anti-alias) — siehe ARCHITECTURE §15.
//!
//! Device-Wahl: System-Default. Override per Env IN_DEVICE / OUT_DEVICE (Substring).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Result};
use audiopus::coder::{Decoder, Encoder};
use audiopus::{Application, Channels, SampleRate};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};

const OPUS_SR: u32 = 48000;
const FRAME: usize = 960; // 20 ms mono @ 48 kHz

type Buf = Arc<Mutex<VecDeque<i16>>>;

/// Streaming linear resampler (mono f32), keeps phase across calls.
struct Resampler {
    step: f64, // input samples per output sample = src/dst
    t: f64,
    prev: f32,
    have_prev: bool,
}
impl Resampler {
    fn new(src: u32, dst: u32) -> Self {
        Self { step: src as f64 / dst as f64, t: 0.0, prev: 0.0, have_prev: false }
    }
    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        for &cur in input {
            if !self.have_prev {
                self.prev = cur;
                self.have_prev = true;
                continue;
            }
            while self.t < 1.0 {
                let v = self.prev + (cur - self.prev) * self.t as f32;
                out.push(v);
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

/// System-default device + its native config (any rate). Env override by name.
fn choose(host: &cpal::Host, input: bool) -> Result<Picked> {
    let kind = if input { "Input" } else { "Output" };
    let devs: Vec<cpal::Device> =
        if input { host.input_devices()?.collect() } else { host.output_devices()?.collect() };

    println!("-- {kind}-Devices --");
    for d in &devs {
        println!("   {}", d.name().unwrap_or_else(|_| "<?>".into()));
    }

    let want = std::env::var(if input { "IN_DEVICE" } else { "OUT_DEVICE" }).ok();
    let device = if let Some(w) = &want {
        devs.iter()
            .find(|d| d.name().map(|n| n.contains(w)).unwrap_or(false))
            .cloned()
            .ok_or_else(|| anyhow!("{kind}-Device '{w}' nicht gefunden"))?
    } else if input {
        host.default_input_device().ok_or_else(|| anyhow!("kein Default-Input"))?
    } else {
        host.default_output_device().ok_or_else(|| anyhow!("kein Default-Output"))?
    };

    let supported =
        if input { device.default_input_config()? } else { device.default_output_config()? };
    Ok(Picked {
        config: supported.config(),
        fmt: supported.sample_format(),
        channels: supported.channels(),
        rate: supported.sample_rate().0,
        device,
    })
}

fn build_input(p: &Picked, cap: Buf) -> Result<cpal::Stream> {
    let ch = p.channels as usize;
    let err = |e| eprintln!("input stream error: {e}");
    let s = match p.fmt {
        SampleFormat::F32 => p.device.build_input_stream(
            &p.config,
            move |data: &[f32], _: &_| {
                let mut b = cap.lock().unwrap();
                for fr in data.chunks(ch) {
                    let mut s = 0f32;
                    for &x in fr {
                        s += x;
                    }
                    b.push_back(((s / ch as f32).clamp(-1.0, 1.0) * 32767.0) as i16);
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
                    let mut s = 0i32;
                    for &x in fr {
                        s += x as i32;
                    }
                    b.push_back((s / ch as i32) as i16);
                }
            },
            err,
            None,
        )?,
        other => return Err(anyhow!("Input-Format {other:?} nicht unterstützt")),
    };
    Ok(s)
}

fn build_output(p: &Picked, play: Buf) -> Result<cpal::Stream> {
    let ch = p.channels as usize;
    let err = |e| eprintln!("output stream error: {e}");
    let s = match p.fmt {
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
    };
    Ok(s)
}

fn main() -> Result<()> {
    let host = cpal::default_host();
    let inp = choose(&host, true)?;
    let outp = choose(&host, false)?;
    println!(
        "\nInput : {} ({}ch {:?} @ {}Hz)",
        inp.device.name()?,
        inp.channels,
        inp.fmt,
        inp.rate
    );
    println!(
        "Output: {} ({}ch {:?} @ {}Hz)",
        outp.device.name()?,
        outp.channels,
        outp.fmt,
        outp.rate
    );

    let cap: Buf = Arc::new(Mutex::new(VecDeque::new()));
    let play: Buf = Arc::new(Mutex::new(VecDeque::new()));

    let in_stream = build_input(&inp, cap.clone())?;
    let out_stream = build_output(&outp, play.clone())?;
    in_stream.play()?;
    out_stream.play()?;

    let in_rate = inp.rate;
    let out_rate = outp.rate;
    let stop = Arc::new(AtomicBool::new(false));
    let stop_w = stop.clone();
    let cap_w = cap.clone();
    let play_w = play.clone();
    let worker = thread::spawn(move || -> Result<()> {
        let enc = Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip)?;
        let mut dec = Decoder::new(SampleRate::Hz48000, Channels::Mono)?;
        let mut up = Resampler::new(in_rate, OPUS_SR); // capture rate -> 48k
        let mut down = Resampler::new(OPUS_SR, out_rate); // 48k -> playback rate

        let mut buf48: Vec<f32> = Vec::new();
        let mut frame_i16 = [0i16; FRAME];
        let mut encoded = [0u8; 4000];
        let mut decoded = [0i16; FRAME];
        let mut out_f: Vec<f32> = Vec::new();
        let mut frames = 0u64;
        let mut bytes_total = 0u64;

        while !stop_w.load(Ordering::SeqCst) {
            // Drain capture (i16 @ in_rate) into f32.
            let chunk: Vec<f32> = {
                let mut b = cap_w.lock().unwrap();
                if b.is_empty() {
                    Vec::new()
                } else {
                    b.drain(..).map(|s| s as f32 / 32768.0).collect()
                }
            };
            if chunk.is_empty() {
                thread::sleep(Duration::from_millis(2));
                continue;
            }
            up.process(&chunk, &mut buf48);

            while buf48.len() >= FRAME {
                for (i, s) in buf48.drain(..FRAME).enumerate() {
                    frame_i16[i] = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                }
                let n = enc.encode(&frame_i16[..], &mut encoded[..])?;
                let samples = dec.decode(Some(&encoded[..n]), &mut decoded[..], false)?;

                out_f.clear();
                let dec_f: Vec<f32> =
                    decoded[..samples].iter().map(|&s| s as f32 / 32768.0).collect();
                down.process(&dec_f, &mut out_f);

                let mut p = play_w.lock().unwrap();
                for v in &out_f {
                    p.push_back((v.clamp(-1.0, 1.0) * 32767.0) as i16);
                }
                frames += 1;
                bytes_total += n as u64;
                if frames % 50 == 0 {
                    println!("frames={frames}  avg_opus_bytes/frame={}", bytes_total / frames);
                }
            }
        }
        Ok(())
    });

    println!("\n>>> Sprich ins Mic — du solltest dich (Opus en/decoded) hören. 15s…\n");
    thread::sleep(Duration::from_secs(15));
    stop.store(true, Ordering::SeqCst);
    let _ = worker.join();
    drop(in_stream);
    drop(out_stream);

    println!(
        "\nVERDICT: cpal capture + resample + opus + resample + cpal playback —\n         wenn du dich gehört hast, hält der native Audio-Stack auf deiner Hardware."
    );
    Ok(())
}
