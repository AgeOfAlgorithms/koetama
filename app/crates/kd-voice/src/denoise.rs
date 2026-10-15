//! Noise suppression for the voice this player sends (docs/noise-suppression.md, measured 2026-10-15 on a Ryzen 7
//! 3700X): Off; Standard - RNNoise (nnnoiseless, its weights built in: 0.5 % of one core, 10 ms); Strong - DeepFilterNet
//! 3 (libDF with tract: 7.8 % of one core, 30 ms; its model, 8 MB, downloaded the first time it is chosen). 48 kHz mono
//! in 10 ms frames (both models' own); a block of any length in, the frames done so far out (the mic's 20 ms blocks:
//! the same length, no extra delay). Speech to text never sees it: it takes the raw microphone (both models made it
//! worse in a noisy room: bench/denoise.py).
use kd_common::{fetch, paths, Log};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::time::Duration;

/// samples per frame (10 ms at 48 kHz: both models')
pub const FRAME: usize = 480;
/// DeepFilterNet 3's model, at the commit the library is pinned to (Cargo.toml), and its SHA-256
pub const DFN3_URL: &str =
    "https://raw.githubusercontent.com/Rikorose/DeepFilterNet/d375b2d8309e0935d165700c91da9de862a99c31/models/DeepFilterNet3_onnx.tar.gz";
pub const DFN3_SHA256: &str = "c94d91f70911001c946e0fabb4aa9adc37045f45a03b56008cb0c8244cb63616";

/// (Off by default here, as nothing else in kd-voice does anything unasked; Koetama's own default is Standard)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Level {
    #[default]
    Off,
    /// RNNoise
    Standard,
    /// DeepFilterNet 3
    Strong,
}

impl Level {
    pub fn from_name(s: &str) -> Option<Level> {
        match s {
            "off" => Some(Level::Off),
            "standard" => Some(Level::Standard),
            "strong" => Some(Level::Strong),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Level::Off => "off",
            Level::Standard => "standard",
            Level::Strong => "strong",
        }
    }
}

enum Model {
    Off,
    Rnnoise { st: Box<nnnoiseless::DenoiseState<'static>>, i: Vec<f32>, o: Vec<f32> },
    Dfn3 { m: Box<df::tract::DfTract>, enh: ndarray::Array2<f32> },
}

pub struct Denoiser {
    level: Level,
    model: Model,
    pending: Vec<f32>,
}

impl Denoiser {
    /// Off: nothing done.
    pub fn off() -> Denoiser {
        Denoiser { level: Level::Off, model: Model::Off, pending: Vec::new() }
    }

    /// Standard is ready at once; Strong loads its model from `model` (dfn3_model: downloaded first).
    pub fn new(level: Level, model: Option<PathBuf>) -> Result<Denoiser, String> {
        let m = match level {
            Level::Off => Model::Off,
            Level::Standard => Model::Rnnoise { st: nnnoiseless::DenoiseState::new(), i: Vec::with_capacity(FRAME), o: vec![0.0; FRAME] },
            Level::Strong => {
                let path = model.ok_or("no DeepFilterNet 3 model")?;
                let p = df::tract::DfParams::new(path.clone()).map_err(|e| format!("{}: {e}", path.display()))?;
                // (the thresholds of DeepFilterNet's own command line: with the library's defaults it took a steady
                //  noise for clean speech and left it - bench/denoise.py)
                let rp = df::tract::RuntimeParams::default_with_ch(1).with_thresholds(-15.0, 35.0, 35.0);
                let m = df::tract::DfTract::new(p, &rp).map_err(|e| format!("DeepFilterNet 3: {e}"))?;
                if m.sr != crate::RATE as usize || m.hop_size != FRAME {
                    return Err(format!("DeepFilterNet 3: {} Hz, {} samples a frame (48000, {FRAME} expected)", m.sr, m.hop_size));
                }
                Model::Dfn3 { m: Box::new(m), enh: ndarray::Array2::zeros((1, FRAME)) }
            }
        };
        Ok(Denoiser { level, model: m, pending: Vec::new() })
    }

    pub fn level(&self) -> Level {
        self.level
    }

    /// 48 kHz mono in -> the whole frames done so far out (Off: as it came).
    pub fn process(&mut self, x: &[f32]) -> Vec<f32> {
        if matches!(self.model, Model::Off) {
            return x.to_vec();
        }
        self.pending.extend_from_slice(x);
        let whole = self.pending.len() / FRAME * FRAME;
        let mut out = Vec::with_capacity(whole);
        for f in self.pending[..whole].chunks_exact(FRAME) {
            match &mut self.model {
                Model::Off => out.extend_from_slice(f),
                Model::Rnnoise { st, i, o } => {
                    // (RNNoise takes samples in the 16-bit range)
                    i.clear();
                    i.extend(f.iter().map(|v| v * 32768.0));
                    st.process_frame(o, i);
                    out.extend(o.iter().map(|v| v / 32768.0));
                }
                Model::Dfn3 { m, enh } => {
                    let noisy = ndarray::ArrayView2::from_shape((1, FRAME), f).expect("one frame");
                    match m.process(noisy, enh.view_mut()) {
                        Ok(_) => out.extend(enh.iter()),
                        Err(_) => out.extend_from_slice(f),
                    }
                }
            }
        }
        self.pending.drain(..whole);
        out
    }
}

/// Where DeepFilterNet 3's model is kept (in Koetama's models folder).
pub fn dfn3_path() -> PathBuf {
    paths::models_dir().join("deepfilternet3").join("DeepFilterNet3_onnx.tar.gz")
}

fn sha256_hex(path: &PathBuf) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Sha256::digest(&b).iter().map(|x| format!("{x:02x}")).collect())
}

/// DeepFilterNet 3's model: downloaded the first time (8 MB), checked against DFN3_SHA256.
pub fn dfn3_model(log: &Log) -> Result<PathBuf, String> {
    let path = dfn3_path();
    if path.exists() && sha256_hex(&path)? == DFN3_SHA256 {
        return Ok(path);
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    log("noise suppression: downloading DeepFilterNet 3 (8 MB)...");
    fetch::download(DFN3_URL, &path, &|_, _| {}, 3).map_err(|e| format!("DeepFilterNet 3 could not be downloaded: {e}"))?;
    let got = sha256_hex(&path)?;
    if got != DFN3_SHA256 {
        let _ = std::fs::remove_file(&path);
        return Err(format!("DeepFilterNet 3's download is not the expected file (SHA-256 {got})"));
    }
    log("noise suppression: DeepFilterNet 3 downloaded");
    Ok(path)
}

/// A denoiser on a thread of its own, which it never leaves (DeepFilterNet's model cannot move between threads): blocks
/// go in, cleaned blocks come back. It is made there too - Strong may download its model and takes ~0.4 s to load -
/// so nobody waits for it; a failure is logged and it falls back to Standard.
struct Worker {
    tx: SyncSender<Vec<f32>>,
    rx: Receiver<Vec<f32>>,
    ready: Arc<AtomicBool>,
}

impl Worker {
    fn spawn(level: Level, log: Log) -> Worker {
        let (tx, blocks) = sync_channel::<Vec<f32>>(64);
        let (done, rx) = sync_channel::<Vec<f32>>(64);
        let ready = Arc::new(AtomicBool::new(false));
        let r2 = ready.clone();
        std::thread::spawn(move || {
            let d = match level {
                Level::Strong => dfn3_model(&log).and_then(|p| Denoiser::new(Level::Strong, Some(p))),
                l => Denoiser::new(l, None),
            };
            let mut d = d.unwrap_or_else(|e| {
                log(&format!("noise suppression: {e} - Standard instead"));
                Denoiser::new(Level::Standard, None).unwrap_or_else(|_| Denoiser::off())
            });
            r2.store(true, Ordering::SeqCst);
            // (until the Switch lets go of it)
            while let Ok(x) = blocks.recv() {
                if done.send(d.process(&x)).is_err() {
                    return;
                }
            }
        });
        Worker { tx, rx, ready }
    }

    fn ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }
}

/// What the voice thread (and "Hear yourself") hold: the level in use, on its worker (none: Off), and a worker being
/// made for a newly chosen level - taken over once its model is loaded, so the voice never waits for one.
#[derive(Default)]
pub struct Switch {
    now: Option<Worker>,
    next: Option<(Level, Option<Worker>)>,
    want: Level,
}

impl Switch {
    /// The level the player chose (Off: at once).
    pub fn want(&mut self, level: Level, log: &Log) {
        if level == self.want {
            return;
        }
        self.want = level;
        self.next = Some((level, (level != Level::Off).then(|| Worker::spawn(level, log.clone()))));
    }

    /// A block through the level in use. `wait`: how long to wait for it to come back (the voice thread: a little,
    /// so it goes on at once; an audio callback: not at all) - what is not back yet comes with a later block.
    pub fn process(&mut self, x: &[f32], wait: Duration) -> Vec<f32> {
        if matches!(&self.next, Some((_, w)) if w.as_ref().is_none_or(Worker::ready)) {
            self.now = self.next.take().and_then(|(_, w)| w);
        }
        let Some(w) = &self.now else {
            return x.to_vec();
        };
        if w.tx.try_send(x.to_vec()).is_err() {
            return x.to_vec(); // (it fell far behind: this block as it is)
        }
        let mut out = match w.rx.recv_timeout(wait) {
            Ok(y) => y,
            Err(_) => Vec::new(),
        };
        while let Ok(y) = w.rx.try_recv() {
            out.extend(y);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// white noise at an RMS of `rms` (deterministic)
    fn noise(n: usize, rms: f32) -> Vec<f32> {
        let mut s = 0x9e37_79b9_u32;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s as f32 / u32::MAX as f32 * 2.0 - 1.0) * rms * 3f32.sqrt()
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn off_passes_through_and_blocks_keep_their_length() {
        let x = noise(960 * 5, 0.05);
        let mut d = Denoiser::off();
        assert_eq!(d.process(&x), x);
        let mut s = Denoiser::new(Level::Standard, None).unwrap();
        for b in x.chunks(960) {
            assert_eq!(s.process(b).len(), 960, "the microphone's 20 ms blocks: whole frames, nothing held back");
        }
        // (an odd block: what is left waits for the next)
        assert_eq!(s.process(&x[..700]).len(), 480);
        assert_eq!(s.process(&x[..260]).len(), 480);
    }

    #[test]
    fn standard_takes_hiss_down() {
        let x = noise(48000 * 2, 0.03);
        let mut d = Denoiser::new(Level::Standard, None).unwrap();
        let out: Vec<f32> = x.chunks(960).flat_map(|b| d.process(b)).collect();
        let (before, after) = (rms(&x[48000..]), rms(&out[48000..]));
        assert!(after < before * 0.5, "hiss {before:.4} -> {after:.4}: at least 6 dB down (measured on real noise: bench/denoise.py)");
    }

    #[test]
    fn strong_takes_noise_down_when_its_model_is_here() {
        // (the model is downloaded the first time Strong is chosen: here only if this PC has it - KOETAMA_DFN3 or
        //  Koetama's models folder)
        let path = std::env::var_os("KOETAMA_DFN3").map(PathBuf::from).unwrap_or_else(dfn3_path);
        if !path.exists() {
            eprintln!("(no DeepFilterNet 3 model at {}: skipped)", path.display());
            return;
        }
        let x = noise(48000 * 2, 0.03);
        let mut d = Denoiser::new(Level::Strong, Some(path)).unwrap();
        let out: Vec<f32> = x.chunks(960).flat_map(|b| d.process(b)).collect();
        assert_eq!(out.len(), x.len());
        assert!(rms(&out[48000..]) < rms(&x[48000..]) * 0.3);
    }

    #[test]
    fn levels_by_name() {
        for l in [Level::Off, Level::Standard, Level::Strong] {
            assert_eq!(Level::from_name(l.name()), Some(l));
        }
        assert_eq!(Level::from_name("loud"), None);
        assert_eq!(Level::default(), Level::Off);
    }
}

#[cfg(test)]
mod switch_tests {
    use super::*;

    #[test]
    fn the_switch_goes_from_off_to_standard_without_waiting() {
        let log = kd_common::null_log();
        let mut s = Switch::default();
        let x: Vec<f32> = (0..960).map(|i| (i as f32 * 0.05).sin() * 0.1).collect();
        assert_eq!(s.process(&x, Duration::from_millis(5)), x, "Off: as it came");
        s.want(Level::Standard, &log);
        let t0 = std::time::Instant::now();
        let mut got = 0;
        while t0.elapsed() < Duration::from_secs(5) {
            got += s.process(&x, Duration::from_millis(5)).len();
            if s.now.is_some() && got > 960 * 3 {
                break;
            }
        }
        assert!(s.now.is_some(), "Standard taken over");
        s.want(Level::Off, &log);
        assert_eq!(s.process(&x, Duration::from_millis(5)), x, "Off again: at once");
    }
}
