//! When this player's voice is sent (PROTOCOL.md "When it sends"): while the microphone is open and they talk -
//! push to talk: while the key is held (from PTT_PREROLL before the game said so: the key takes a feed or two to
//! arrive) and PTT_TAIL after it is let go; always on: while the speech detector hears speech, starting PREROLL before
//! it noticed (the first syllable is what made it notice).
use crate::RATE;
use std::collections::VecDeque;

/// s of audio still sent after the talk key is let go (the last syllable; kd_speech's PTT_TAIL)
pub const PTT_TAIL: f64 = 0.25;
/// s of audio from before the speech detector noticed the player talking
pub const PREROLL: f64 = 0.3;
/// s of audio from before the talk key's press arrived (the feed's delay: ~50-80 ms, and a 50 ms block)
pub const PTT_PREROLL: f64 = 0.15;

/// What the player is doing during a block of microphone audio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// nothing is sent (the microphone is closed, no room, ...)
    Off,
    /// push to talk: the key is held (true) or up
    PushToTalk(bool),
    /// always on: the speech detector hears speech (true) or not
    Detector(bool),
}

/// What a block gives: the audio to send now (with the pre-roll when a stretch starts), and whether the stretch of
/// talking ends after it.
#[derive(Debug, Default, PartialEq)]
pub struct Gated {
    pub audio: Vec<f32>,
    pub end: bool,
}

pub struct Gate {
    /// the last PREROLL s before the current block
    ring: VecDeque<f32>,
    open: bool,
    /// push to talk: samples sent since the key was let go
    tail: usize,
}

impl Default for Gate {
    fn default() -> Gate {
        Gate::new()
    }
}

impl Gate {
    pub fn new() -> Gate {
        Gate { ring: VecDeque::new(), open: false, tail: 0 }
    }

    /// A stretch of talking is being sent.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// One block of microphone audio (RATE, mono) and what the player was doing meanwhile.
    pub fn push(&mut self, x: &[f32], mode: Mode) -> Gated {
        let mut g = Gated::default();
        match mode {
            Mode::Off => {
                if self.open {
                    self.open = false;
                    g.end = true;
                }
            }
            Mode::PushToTalk(true) => {
                if !self.open {
                    // (from a moment before the press arrived)
                    let n = ((RATE as f64 * PTT_PREROLL) as usize).min(self.ring.len());
                    g.audio.extend(self.ring.iter().skip(self.ring.len() - n));
                }
                self.open = true;
                self.tail = 0;
                g.audio.extend_from_slice(x);
            }
            Mode::PushToTalk(false) => {
                if self.open {
                    let tail = (RATE as f64 * PTT_TAIL) as usize;
                    let take = (tail - self.tail).min(x.len());
                    g.audio.extend_from_slice(&x[..take]);
                    self.tail += take;
                    if self.tail >= tail {
                        self.open = false;
                        g.end = true;
                    }
                }
            }
            Mode::Detector(true) => {
                if !self.open {
                    // (from before it noticed)
                    self.open = true;
                    g.audio.extend(self.ring.iter());
                }
                g.audio.extend_from_slice(x);
            }
            Mode::Detector(false) => {
                if self.open {
                    self.open = false;
                    g.end = true;
                }
            }
        }
        let keep = (RATE as f64 * PREROLL) as usize;
        self.ring.extend(x.iter().copied());
        let cut = self.ring.len().saturating_sub(keep);
        self.ring.drain(..cut);
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: usize = 2400; // (a 50 ms block, as the microphone gives)

    fn block(v: f32) -> Vec<f32> {
        vec![v; B]
    }

    #[test]
    fn push_to_talk_and_its_tail() {
        let mut g = Gate::new();
        assert_eq!(g.push(&block(1.0), Mode::PushToTalk(false)), Gated::default(), "the key up: nothing");
        let a = g.push(&block(2.0), Mode::PushToTalk(true));
        // (held: 0.15 s from before - 7200 samples, of which only the one 50 ms block there was - and this block)
        assert!(a.audio.len() == 2 * B && a.audio[0] == 1.0 && a.audio[B] == 2.0 && !a.end && g.is_open(), "held: the pre-roll, then this block");
        // let go: 0.25 s more (12000 samples: five blocks), then the end
        let mut sent = 0;
        let mut ended = false;
        for _ in 0..8 {
            let r = g.push(&block(3.0), Mode::PushToTalk(false));
            sent += r.audio.len();
            ended |= r.end;
            if r.end {
                break;
            }
        }
        assert!(ended && sent == 12000 && !g.is_open(), "{sent}");
        assert_eq!(g.push(&block(4.0), Mode::PushToTalk(false)), Gated::default());
        // held again within the tail: one stretch goes on
        g.push(&block(1.0), Mode::PushToTalk(true));
        g.push(&block(1.0), Mode::PushToTalk(false));
        let r = g.push(&block(1.0), Mode::PushToTalk(true));
        assert!(!r.end && r.audio.len() == B && g.is_open(), "(no second pre-roll: the stretch goes on)");
        // a fresh press after quiet: 0.15 s from before it
        g.push(&block(1.0), Mode::Off);
        for _ in 0..6 {
            g.push(&block(5.0), Mode::PushToTalk(false));
        }
        let f = g.push(&block(6.0), Mode::PushToTalk(true));
        assert!(f.audio.len() == 7200 + B && f.audio[0] == 5.0 && f.audio[7200] == 6.0);
        g.push(&block(1.0), Mode::PushToTalk(true));
        // the microphone closed mid-stretch: it ends
        assert!(g.push(&block(1.0), Mode::Off).end && !g.is_open());
    }

    #[test]
    fn the_detector_with_its_pre_roll() {
        let mut g = Gate::new();
        for i in 0..10 {
            assert_eq!(g.push(&block(i as f32), Mode::Detector(false)), Gated::default(), "quiet: nothing");
        }
        let r = g.push(&block(10.0), Mode::Detector(true));
        // 0.3 s before it (14400 samples: the last six blocks, 4..9), then the block itself
        assert_eq!(r.audio.len(), 14400 + B);
        assert!(r.audio[0] == 4.0 && r.audio[14399] == 9.0 && r.audio[14400] == 10.0 && !r.end);
        assert_eq!(g.push(&block(11.0), Mode::Detector(true)).audio, block(11.0));
        let e = g.push(&block(12.0), Mode::Detector(false));
        assert!(e.end && e.audio.is_empty() && !g.is_open());
        assert_eq!(g.push(&block(13.0), Mode::Detector(false)), Gated::default());
        // a short start: the pre-roll is what there is
        let mut h = Gate::new();
        assert_eq!(h.push(&block(1.0), Mode::Detector(true)).audio.len(), B);
    }
}
