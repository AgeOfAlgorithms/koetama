//! The mixer played into a wav file instead of a sound device, in real time: what a player would hear, kept
//! (`koetama --cli --out-wav`; tools/voice_e2e.py records the listener with it). No sound device is needed. The file
//! is written again every second (48 kHz stereo, 16-bit), so it is complete however the program ends.
use crate::mixer::{lock, SharedMixer, RATE};
use kd_common::Log;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub struct WavOutput {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    path: PathBuf,
}

fn write(path: &PathBuf, x: &[f32]) -> Result<(), String> {
    let spec = hound::WavSpec { channels: 2, sample_rate: RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for v in x {
        w.write_sample((v.clamp(-1.0, 1.0) * 32767.0) as i16).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

impl WavOutput {
    pub fn start(mixer: SharedMixer, path: PathBuf, log: Log) -> Result<WavOutput, String> {
        write(&path, &[]).map_err(|e| format!("{}: {e}", path.display()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let (s2, p2) = (stop.clone(), path.clone());
        let thread = std::thread::spawn(move || {
            let block = (RATE / 100) as usize;
            let mut buf = vec![0.0f32; block * 2];
            let mut all: Vec<f32> = Vec::new();
            let (t0, mut k, mut last_write) = (Instant::now(), 0u64, Instant::now());
            loop {
                let done = s2.load(Ordering::SeqCst);
                if !done {
                    lock(&mixer).render_into(&mut buf);
                    all.extend_from_slice(&buf);
                    k += 1;
                }
                if done || last_write.elapsed() >= Duration::from_secs(1) {
                    if let Err(e) = write(&p2, &all) {
                        log(&format!("{}: {e}", p2.display()));
                    }
                    last_write = Instant::now();
                }
                if done {
                    return;
                }
                let due = t0 + Duration::from_millis(10 * k);
                std::thread::sleep(due.saturating_duration_since(Instant::now()));
            }
        });
        Ok(WavOutput { stop, thread: Some(thread), path })
    }

    pub fn device_name(&self) -> String {
        format!("the file {}", self.path.display())
    }

    pub fn latency_ms(&self) -> f64 {
        10.0
    }
}

impl Drop for WavOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
