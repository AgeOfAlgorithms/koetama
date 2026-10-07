//! The microphone (engine/asr.py Microphone): the chosen (or default) input at 16 kHz, its blocks to the listener;
//! open only while the game wants it.
use kd_audio::Input;
use kd_common::Log;
use kd_speech::{Listener, Mic, RATE};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub struct Microphone {
    listener: Listener,
    device: Option<String>,
    log: Log,
    input: Option<Input>,
    level: Arc<AtomicU64>, // (f64 bits: dBFS of the last block)
}

impl Microphone {
    pub fn new(listener: Listener, device: Option<String>, log: Log) -> Microphone {
        Microphone {
            listener,
            device,
            log,
            input: None,
            level: Arc::new(AtomicU64::new((-120f64).to_bits())),
        }
    }
}

/// dBFS of a block (-120 for silence), as Python's 20 log10(rms).
pub fn level_db(x: &[f32]) -> f64 {
    if x.is_empty() {
        return -120.0;
    }
    let ms = x.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / x.len() as f64;
    20.0 * ms.sqrt().max(1e-6).log10()
}

impl Mic for Microphone {
    fn is_open(&self) -> bool {
        self.input.is_some()
    }

    fn open(&mut self) -> bool {
        if self.input.is_some() {
            return true;
        }
        let (l, level) = (self.listener.clone(), self.level.clone());
        let on_block = Box::new(move |x: &[f32]| {
            level.store(level_db(x).to_bits(), Ordering::Relaxed);
            l.push(x);
        });
        match Input::open(self.device.as_deref(), RATE, on_block, self.log.clone()) {
            Ok(i) => {
                (self.log)(&format!("microphone on: {}", i.device_name()));
                self.input = Some(i);
                true
            }
            Err(e) => {
                (self.log)(&format!("the microphone could not be opened: {e}"));
                false
            }
        }
    }

    fn close(&mut self) {
        if self.input.take().is_some() {
            self.level.store((-120f64).to_bits(), Ordering::Relaxed);
            (self.log)("microphone off");
        }
    }

    fn level(&self) -> f64 {
        f64::from_bits(self.level.load(Ordering::Relaxed))
    }
}
