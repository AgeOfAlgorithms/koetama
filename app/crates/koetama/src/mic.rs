//! The microphone (engine/asr.py Microphone): the chosen (or default) input, its blocks to the listener at 16 kHz;
//! open only while the game wants it. With the voice chat (kd_voice) it runs at 48 kHz: each block goes to the voice
//! chat as it is (with whether the speech detector hears speech) and, resampled to 16 kHz, to the listener.
use kd_audio::{Input, Rechunk};
use kd_common::Log;
use kd_speech::{Listener, Mic, RATE};
use kd_voice::Voice;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub struct Microphone {
    listener: Listener,
    device: Option<String>,
    log: Log,
    input: Option<Input>,
    level: Arc<AtomicU64>, // (f64 bits: dBFS of the last block)
    voice: Option<Voice>,
    /// "Hear yourself" (the window's microphone test): the voice's blocks go to it too
    monitor: Option<Arc<crate::monitor::Monitor>>,
}

impl Microphone {
    pub fn new(listener: Listener, device: Option<String>, log: Log) -> Microphone {
        Microphone {
            listener,
            device,
            log,
            input: None,
            level: Arc::new(AtomicU64::new((-120f64).to_bits())),
            voice: None,
            monitor: None,
        }
    }

    /// "Hear yourself": the microphone's 48 kHz blocks (with the voice chat) go to this monitor too.
    pub fn with_monitor(mut self, monitor: Option<Arc<crate::monitor::Monitor>>) -> Microphone {
        self.monitor = monitor;
        self
    }

    /// The voice chat gets the microphone's audio too (None: only the listener).
    pub fn with_voice(mut self, voice: Option<Voice>) -> Microphone {
        self.voice = voice;
        self
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
        // (the voice chat: 48 kHz in, the listener's 16 kHz made here; 10 ms chunks: little delay)
        let tap = self.voice.clone().and_then(|v| match Rechunk::new(kd_voice::RATE, RATE, kd_voice::RATE as usize / 100, 1) {
            Ok(down) => Some((v, down)),
            Err(e) => {
                (self.log)(&format!("voice: the microphone cannot be shared ({e}): your voice is not sent"));
                None
            }
        });
        let monitor = self.monitor.clone();
        let (rate, on_block): (u32, kd_audio::BlockFn) = match tap {
            Some((v, mut down)) => {
                let mut low = Vec::new();
                (
                    kd_voice::RATE,
                    Box::new(move |x: &[f32]| {
                        level.store(level_db(x).to_bits(), Ordering::Relaxed);
                        v.push_mic(x, l.talking());
                        if let Some(m) = &monitor {
                            m.push(x);
                        }
                        low.clear();
                        down.push(x, &mut |y| low.extend_from_slice(y));
                        if !low.is_empty() {
                            l.push(&low);
                        }
                    }),
                )
            }
            None => (
                RATE,
                Box::new(move |x: &[f32]| {
                    level.store(level_db(x).to_bits(), Ordering::Relaxed);
                    l.push(x);
                }),
            ),
        };
        match Input::open(self.device.as_deref(), rate, on_block, self.log.clone()) {
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
