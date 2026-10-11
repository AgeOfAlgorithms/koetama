//! "Hear yourself" (the window's microphone test): the player's own microphone, through the same automatic gain and Mic
//! boost as the voice they send (kd_voice::agc), played back on their speakers - so they hear how loud they come
//! through without a game or another player. The microphone's 48 kHz blocks go in (Microphone, with the voice chat);
//! a mixer of its own pulls them out (MonitorStreams) to an output of its own while the test runs.
use kd_audio::Streams;
use kd_voice::agc::Agc;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// at most this much audio waits (48 kHz samples: 0.3 s) - more is dropped, so it never drifts behind
const MAX_WAIT: usize = 48000 * 3 / 10;

#[derive(Default)]
pub struct Monitor {
    on: AtomicBool,
    inner: Mutex<(VecDeque<f32>, Agc)>,
}

impl Monitor {
    pub fn set_on(&self, on: bool) {
        self.on.store(on, Ordering::SeqCst);
        if !on {
            self.lock().0.clear();
        }
    }

    pub fn is_on(&self) -> bool {
        self.on.load(Ordering::SeqCst)
    }

    pub fn set_boost_db(&self, db: f32) {
        self.lock().1.set_boost_db(db);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (VecDeque<f32>, Agc)> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A microphone block (48 kHz mono), from its callback: through the gain, queued (nothing while off).
    pub fn push(&self, x: &[f32]) {
        if !self.is_on() {
            return;
        }
        let mut g = self.lock();
        let mut y = x.to_vec();
        g.1.process(&mut y);
        g.0.extend(y);
        let over = g.0.len().saturating_sub(MAX_WAIT);
        g.0.drain(..over);
    }
}

/// The monitor as the mixer's stream (any speaker id: there is one).
pub struct MonitorStreams(pub std::sync::Arc<Monitor>);

impl Streams for MonitorStreams {
    fn pull(&mut self, _id: i64, out: &mut [f32]) -> bool {
        let m = &self.0;
        if !m.is_on() {
            out.fill(0.0);
            return false;
        }
        let mut g = m.lock();
        for o in out.iter_mut() {
            *o = g.0.pop_front().unwrap_or(0.0);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn what_goes_in_comes_out_only_while_on() {
        let m = Arc::new(Monitor::default());
        m.push(&[0.1; 480]);
        let mut s = MonitorStreams(m.clone());
        let mut out = [1.0f32; 480];
        assert!(!s.pull(1, &mut out) && out.iter().all(|v| *v == 0.0), "off: silence, nothing kept");
        m.set_on(true);
        m.push(&[0.1; 480]);
        assert!(s.pull(1, &mut out) && out.iter().all(|v| *v > 0.0), "on: the voice, through the gain");
        assert!(s.pull(1, &mut out) && out.iter().all(|v| *v == 0.0), "nothing more: silence");
        m.push(&vec![0.1; 48000]);
        assert!(m.lock().0.len() <= MAX_WAIT, "never far behind");
    }
}
