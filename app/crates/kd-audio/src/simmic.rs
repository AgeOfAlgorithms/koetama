//! Simulated microphones: the ways real ones differ - channels (a silent one, a copy, one with its polarity inverted,
//! capsules a little apart), level (quiet, clipping), a DC offset, noise, the sample rate and the sample type - applied
//! to a voice, then run through Koetama's own capture (capture.rs) as a device's samples would be. The tests run every
//! one (kd-audio's mic_zoo, kd-voice's voice_zoo: through the voice chat too), and `koetama --cli --sim-mic <name>`
//! plays a recording through one in real time (tools/voice_e2e.py). A player's microphone that is heard wrongly
//! becomes a new entry here. (Added 2026-10-15, after a headset whose second channel was inverted came through ~30 dB
//! too quiet: its channels cancelled when averaged.)
use crate::capture::Capture;
use crate::channels::Choice;
use crate::devices::BlockFn;
use cpal::{Sample, I24};
use kd_common::Log;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The sample type a simulated device delivers (each through cpal's own conversion, as a real one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimFormat {
    F32,
    I16,
    I24,
    I32,
    U8,
}

#[derive(Clone, Debug)]
pub struct SimMic {
    pub name: &'static str,
    pub about: &'static str,
    pub rate: u32,
    pub format: SimFormat,
    /// each channel: the voice's gain in it (negative: polarity inverted; 0: silent) and its delay in samples
    pub channels: Vec<(f32, usize)>,
    /// dB the voice is changed by at the microphone (a quiet one, a hot one)
    pub level_db: f32,
    /// a DC offset added to every channel
    pub dc: f32,
    /// white noise in every channel at this level (dBFS RMS)
    pub noise_db: Option<f32>,
    /// what a right capture gives, in dB against the voice (None: not a fixed level - clipping)
    pub expect_db: Option<f32>,
}

impl SimMic {
    fn new(name: &'static str, about: &'static str, rate: u32, format: SimFormat, channels: &[(f32, usize)]) -> SimMic {
        SimMic {
            name,
            about,
            rate,
            format,
            channels: channels.to_vec(),
            level_db: 0.0,
            dc: 0.0,
            noise_db: None,
            expect_db: Some(0.0),
        }
    }

    fn level(mut self, db: f32, expect: Option<f32>) -> SimMic {
        self.level_db = db;
        self.expect_db = expect;
        self
    }

    /// The voice (48 kHz mono) as this device delivers it: interleaved frames at its rate, every value one its
    /// sample type can hold.
    pub fn frames(&self, voice: &[f32]) -> Vec<f32> {
        let x = crate::resample(voice, crate::RATE, self.rate);
        let g = 10f32.powf(self.level_db / 20.0);
        let ch = self.channels.len().max(1);
        let noise = self.noise_db.map(|db| 10f32.powf(db / 20.0) * 3f32.sqrt());
        let mut seed = 0x2545_f491_u32;
        let mut out = Vec::with_capacity(x.len() * ch);
        for i in 0..x.len() {
            for &(gain, delay) in &self.channels {
                let v = if i >= delay { x[i - delay] } else { 0.0 };
                let mut s = v * g * gain + self.dc;
                if let Some(n) = noise {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    s += n * (seed as f32 / u32::MAX as f32 * 2.0 - 1.0);
                }
                out.push(quantize(s, self.format));
            }
        }
        out
    }

    /// The voice through this device and Koetama's capture, offline: what Koetama hears (mono at `rate`), and what it
    /// did with the channels in the end.
    pub fn capture(&self, voice: &[f32], rate: u32) -> Result<(Vec<f32>, Choice), String> {
        let frames = self.frames(voice);
        let ch = self.channels.len().max(1);
        let mut c = Capture::new(ch as u16, self.rate, rate)?;
        let mut out = Vec::new();
        // (in 10 ms callbacks, as a device delivers)
        for piece in frames.chunks((self.rate as usize / 100).max(1) * ch) {
            c.push(piece, &mut |b| out.extend_from_slice(b));
        }
        Ok((out, c.choice()))
    }
}

/// A value through the sample type and back, as cpal converts a device's samples.
fn quantize(s: f32, f: SimFormat) -> f32 {
    match f {
        SimFormat::F32 => s,
        SimFormat::I16 => i16::from_sample(s).to_sample::<f32>(),
        SimFormat::I24 => I24::from_sample(s).to_sample::<f32>(),
        SimFormat::I32 => i32::from_sample(s).to_sample::<f32>(),
        SimFormat::U8 => u8::from_sample(s).to_sample::<f32>(),
    }
}

/// Every simulated microphone.
pub fn sim_mics() -> Vec<SimMic> {
    use SimFormat::*;
    let m = SimMic::new;
    vec![
        m("mono", "one channel, 48 kHz float (the reference)", 48000, F32, &[(1.0, 0)]),
        m("stereo-copy", "a driver that copies the voice into both channels", 48000, I16, &[(1.0, 0), (1.0, 0)]),
        m("stereo-silent-right", "a headset: the voice in the left channel, the right one silent", 48000, I16, &[(1.0, 0), (0.0, 0)]),
        m("stereo-silent-left", "the voice in the right channel, the left one silent", 48000, I16, &[(0.0, 0), (1.0, 0)]),
        m("stereo-inverted", "a headset: the right channel is the left one inverted (2026-10-15)", 48000, I16, &[(1.0, 0), (-1.0, 0)]),
        m("stereo-inverted-uneven", "inverted and a little quieter in the right channel", 48000, I16, &[(1.0, 0), (-0.7, 0)]),
        m("stereo-capsules", "a stereo microphone: two capsules, a little apart", 48000, F32, &[(1.0, 0), (0.9, 2)]),
        m("array-4", "a laptop's microphone array: four capsules in a row", 48000, F32, &[(1.0, 0), (1.0, 1), (1.0, 2), (1.0, 3)]),
        m("array-4-one-inverted", "a microphone array with one capsule inverted", 48000, F32, &[(1.0, 0), (1.0, 0), (-1.0, 0), (1.0, 0)]),
        m("quad-voice-in-one", "four channels, the voice in the third only", 48000, I16, &[(0.0, 0), (0.0, 0), (1.0, 0), (0.0, 0)]),
        m("quiet", "a quiet microphone: 25 dB down", 48000, I16, &[(1.0, 0)]).level(-25.0, Some(-25.0)),
        // (2026-10-21: a player's audio interface gave speech at -45..-52 dBFS; the automatic gain took it for silence)
        m("very-quiet", "a very quiet microphone: 30 dB down (speech at -50 dBFS)", 44100, F32, &[(1.0, 0)]).level(-30.0, Some(-30.0)),
        m("hot", "a hot microphone: 20 dB up, clipping", 48000, I16, &[(1.0, 0)]).level(20.0, None),
        SimMic { dc: 0.02, ..m("dc-offset", "a DC offset (a cheap USB adapter)", 48000, I16, &[(1.0, 0)]) },
        SimMic { noise_db: Some(-45.0), ..m("noisy", "hiss at -45 dBFS", 48000, F32, &[(1.0, 0)]) },
        m("44k1-i16", "44.1 kHz, 16-bit, stereo copied", 44100, I16, &[(1.0, 0), (1.0, 0)]),
        m("16k-i16", "16 kHz, 16-bit (a cheap headset)", 16000, I16, &[(1.0, 0)]),
        m("96k-i24", "96 kHz, 24-bit, stereo copied (an audio interface)", 96000, I24, &[(1.0, 0), (1.0, 0)]),
        m("48k-i32-silent-right", "32-bit, the voice in the left channel only", 48000, I32, &[(1.0, 0), (0.0, 0)]),
        m("48k-u8", "8-bit", 48000, U8, &[(1.0, 0)]),
    ]
}

pub fn sim_mic(name: &str) -> Option<SimMic> {
    sim_mics().into_iter().find(|m| m.name == name)
}

/// A voice-like test signal (48 kHz mono, `secs` long): a buzz at a speaking pitch with a few formants, in syllables
/// and pauses, its speech at -20 dBFS RMS. For tests without recordings (CI has none).
pub fn test_voice(secs: f64) -> Vec<f32> {
    let n = (secs * crate::RATE as f64) as usize;
    let mut x: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f64 / crate::RATE as f64;
            let f0 = 140.0 + 20.0 * (t * 1.3).sin();
            let mut v = 0.0;
            for h in 1..=20 {
                let f = f0 * h as f64;
                // (formants near 500, 1500, 2500 Hz)
                let shape: f64 = [500.0, 1500.0, 2500.0].iter().map(|&c| (-((f - c) / 300.0).powi(2)).exp()).sum::<f64>() + 0.05;
                v += shape * (2.0 * std::f64::consts::PI * f * t).sin() / h as f64;
            }
            // (syllables, 4 a second, and a pause every 1.5 s)
            let syl = (std::f64::consts::PI * 4.0 * t).sin().abs();
            let pause = if (t % 1.5) > 1.2 { 0.0 } else { 1.0 };
            (v * syl * pause) as f32
        })
        .collect();
    let sp: Vec<f32> = x.iter().copied().filter(|v| v.abs() > 1e-4).collect();
    let r = (sp.iter().map(|v| v * v).sum::<f32>() / sp.len().max(1) as f32).sqrt();
    for v in &mut x {
        *v *= 0.1 / r.max(1e-9);
    }
    x
}

/// A simulated microphone in real time: a recording played through it into Koetama's capture, blocks to on_block as a
/// device's callback would hand them (every 10 ms). Dropping it stops it.
pub struct SimInput {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    name: String,
    format: String,
}

impl SimInput {
    /// `voice`: 48 kHz mono; `repeat`: start over at its end (else silence after it).
    pub fn start(mic: SimMic, voice: &[f32], rate: u32, mut on_block: BlockFn, log: Log, repeat: bool) -> Result<SimInput, String> {
        let ch = mic.channels.len().max(1);
        let frames = mic.frames(voice);
        let silence = mic.frames(&vec![0.0; crate::RATE as usize / 100]);
        let mut capture = Capture::new(ch as u16, mic.rate, rate)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let piece = (mic.rate as usize / 100).max(1) * ch;
        let format = format!("{} Hz, {ch} ch, {:?} (simulated: {})", mic.rate, mic.format, mic.name);
        let thread = std::thread::spawn(move || {
            let t0 = Instant::now();
            let mut at = 0usize;
            let mut k = 0u64;
            while !stop2.load(Ordering::SeqCst) {
                let x: &[f32] = if at < frames.len() {
                    let e = (at + piece).min(frames.len());
                    let p = &frames[at..e];
                    at = e;
                    p
                } else {
                    if repeat {
                        at = 0;
                    }
                    &silence
                };
                let (_, told) = capture.push(x, &mut |b| on_block(b));
                if let Some(line) = told {
                    log(&line);
                }
                k += 1;
                let due = t0 + Duration::from_millis(10 * k);
                std::thread::sleep(due.saturating_duration_since(Instant::now()));
            }
        });
        Ok(SimInput { stop, thread: Some(thread), name: mic.name.to_string(), format })
    }

    pub fn device_name(&self) -> String {
        format!("simulated microphone \"{}\"", self.name)
    }

    pub fn format(&self) -> String {
        self.format.clone()
    }
}

impl Drop for SimInput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
