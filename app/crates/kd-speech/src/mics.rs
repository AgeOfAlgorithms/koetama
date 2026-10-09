//! Microphones without a microphone (asr.py WavMicrophone, PlaylistMicrophone): recordings played into a Listener
//! in real time. (The real microphone, kd_audio::Input at 16 kHz -> Listener::push, is in the program.)
use crate::listener::Listener;
use crate::RATE;
use kd_common::Log;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A source of the player's voice the program opens only while it is wanted.
pub trait Mic: Send {
    fn is_open(&self) -> bool;
    fn open(&mut self) -> bool;
    fn close(&mut self);
    /// dBFS of the last block (-120: none yet)
    fn level(&self) -> f64;
}

/// 50 ms blocks, as the microphone gives them
const BLOCK: usize = (RATE as usize) / 20;

/// A block's level in dBFS, as Python's 20 log10(max(1e-6, rms)).
fn dbfs(x: &[f32]) -> f64 {
    let ms = if x.is_empty() { 0.0 } else { x.iter().map(|&v| v as f64 * v as f64).sum::<f64>() / x.len() as f64 };
    20.0 * ms.sqrt().max(1e-6).log10()
}

/// Sleeps until `k` samples after t0 (real time).
fn pace(t0: Instant, k: usize) {
    let due = t0 + Duration::from_secs_f64(k as f64 / RATE as f64);
    let now = Instant::now();
    if due > now {
        std::thread::sleep(due - now);
    }
}

fn store(level: &AtomicU64, db: f64) {
    level.store(db.to_bits(), Ordering::Relaxed);
}

/// Where a recording microphone's blocks also go (the voice chat).
pub type Tap = Arc<dyn Fn(&[f32]) + Send + Sync>;

/// A recording played into the listener in real time as if it were the microphone (tests without one):
/// from the start each time it is opened, then quiet.
pub struct WavMicrophone {
    listener: Listener,
    audio: Arc<Vec<f32>>,
    log: Log,
    run: Option<Arc<AtomicBool>>, // (the playing thread's "keep going"; None: closed)
    level: Arc<AtomicU64>,
    /// each block also goes here (the voice chat, as the real microphone's does)
    tap: Option<Tap>,
}

impl WavMicrophone {
    pub fn new(listener: Listener, audio: Vec<f32>, log: Log) -> WavMicrophone {
        WavMicrophone {
            listener,
            audio: Arc::new(audio),
            log,
            run: None,
            level: Arc::new(AtomicU64::new((-120f64).to_bits())),
            tap: None,
        }
    }

    /// Each block (RATE) also to `tap` - the voice chat.
    pub fn with_tap(mut self, tap: Tap) -> WavMicrophone {
        self.tap = Some(tap);
        self
    }
}

impl Mic for WavMicrophone {
    fn is_open(&self) -> bool {
        self.run.is_some()
    }

    fn open(&mut self) -> bool {
        if self.run.is_none() {
            let go = Arc::new(AtomicBool::new(true));
            let (listener, audio, level, g) = (self.listener.clone(), self.audio.clone(), self.level.clone(), go.clone());
            let tap = self.tap.clone();
            std::thread::spawn(move || {
                let t0 = Instant::now();
                let quiet = vec![0f32; BLOCK];
                let mut k = 0;
                while g.load(Ordering::Relaxed) {
                    let x = if k < audio.len() { &audio[k..(k + BLOCK).min(audio.len())] } else { &quiet[..] };
                    store(&level, dbfs(x));
                    listener.push(x);
                    if let Some(t) = &tap {
                        t(x);
                    }
                    k += BLOCK;
                    pace(t0, k);
                }
            });
            self.run = Some(go);
            (self.log)(&format!(
                "microphone: playing the recording ({:.1} s) as the microphone",
                self.audio.len() as f64 / RATE as f64
            ));
        }
        true
    }

    fn close(&mut self) {
        if let Some(g) = self.run.take() {
            g.store(false, Ordering::Relaxed);
        }
    }

    fn level(&self) -> f64 {
        f64::from_bits(self.level.load(Ordering::Relaxed))
    }
}

impl Drop for WavMicrophone {
    fn drop(&mut self) {
        self.close();
    }
}

/// Quiet hiss between the recorded lines: a small seeded generator (normal values, Box-Muller), not numpy's -
/// any noise this quiet does the same.
struct Noise(u64);

impl Noise {
    fn uniform(&mut self) -> f64 {
        // (xorshift64*)
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        ((self.0.wrapping_mul(0x2545F4914F6CDD1D) >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn normal(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }
}

/// Recorded lines played into the listener in real time, one after another, each in its own language (the
/// listener is switched to it first): the whole pipeline on real audio, for watching it in the game without a
/// microphone. items: [(lang, audio 16 kHz float32, what is said)]; gap: s of quiet hiss after each (Python: 2.5).
pub struct PlaylistMicrophone {
    listener: Listener,
    items: Arc<Vec<(String, Vec<f32>, String)>>,
    gap: f64,
    log: Log,
    run: Option<Arc<AtomicBool>>,
    level: Arc<AtomicU64>,
    done: Arc<AtomicBool>,
}

impl PlaylistMicrophone {
    pub fn new(listener: Listener, items: Vec<(String, Vec<f32>, String)>, gap: f64, log: Log) -> PlaylistMicrophone {
        PlaylistMicrophone {
            listener,
            items: Arc::new(items),
            gap,
            log,
            run: None,
            level: Arc::new(AtomicU64::new((-120f64).to_bits())),
            done: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Every line has been played.
    pub fn done(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }
}

impl Mic for PlaylistMicrophone {
    fn is_open(&self) -> bool {
        self.run.is_some()
    }

    fn open(&mut self) -> bool {
        if self.run.is_none() {
            let go = Arc::new(AtomicBool::new(true));
            let (listener, items, level, done, log, g) =
                (self.listener.clone(), self.items.clone(), self.level.clone(), self.done.clone(), self.log.clone(), go.clone());
            let gap = self.gap;
            std::thread::spawn(move || {
                let mut rng = Noise(4u64.wrapping_mul(0x9E3779B97F4A7C15) | 1);
                for (k, (lang, audio, said)) in items.iter().enumerate() {
                    if !g.load(Ordering::Relaxed) {
                        return;
                    }
                    if let Err(e) = listener.warm(Some(lang)) {
                        log(&format!("speech: {e}"));
                    }
                    listener.set_language(lang);
                    log(&format!("  playing {} of {} [{lang}]: {said}", k + 1, items.len()));
                    let mut x = audio.clone();
                    x.extend((0..(RATE as f64 * gap) as usize).map(|_| (rng.normal() * 0.002) as f32));
                    let t0 = Instant::now();
                    for i in (0..x.len()).step_by(BLOCK) {
                        if !g.load(Ordering::Relaxed) {
                            return;
                        }
                        let blk = &x[i..(i + BLOCK).min(x.len())];
                        store(&level, dbfs(blk));
                        listener.push(blk);
                        pace(t0, i + BLOCK);
                    }
                }
                done.store(true, Ordering::Relaxed);
                log("auto speech: all lines played");
                let quiet = vec![0f32; BLOCK];
                while g.load(Ordering::Relaxed) {
                    // (then quiet)
                    listener.push(&quiet);
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
            self.run = Some(go);
            (self.log)(&format!(
                "auto speech: {} recorded lines through the real speech-to-text, {:.1} s apart",
                self.items.len(),
                self.gap
            ));
        }
        true
    }

    fn close(&mut self) {
        if let Some(g) = self.run.take() {
            g.store(false, Ordering::Relaxed);
        }
    }

    fn level(&self) -> f64 {
        f64::from_bits(self.level.load(Ordering::Relaxed))
    }
}

impl Drop for PlaylistMicrophone {
    fn drop(&mut self) {
        self.close();
    }
}
