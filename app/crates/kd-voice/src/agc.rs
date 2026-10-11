//! Automatic gain for the voice this player sends, as other games' voice chat does: a quiet microphone is raised toward
//! a normal speaking level, a loud one held back, so every player comes through about as loud (a player Koetama sent at
//! half the level other games did, 2026-10-10). Only what the gate lets through is seen (speech, not the room's quiet),
//! the gain rises slowly (no pumping between words) and falls quickly (a shout does not blast), it is never raised past
//! MAX_GAIN (a whisper's noise stays noise), and a soft limiter keeps peaks under full scale. `boost` (dB, the player's
//! "Mic boost") comes first.

/// the speaking level aimed at (RMS of 10 ms pieces with speech): -20 dBFS
pub const TARGET: f32 = 0.1;
/// the most it raises a voice: +18 dB
pub const MAX_GAIN: f32 = 8.0;
/// the most it lowers one: -6 dB
pub const MIN_GAIN: f32 = 0.5;
/// a 10 ms piece quieter than this is not speech (its level is not followed)
pub const SPEECH: f32 = 0.004;
/// how fast the gain follows: per 10 ms piece, toward a lower gain / a higher one
pub const FALL: f32 = 0.25;
pub const RISE: f32 = 0.015;
const PIECE: usize = 480;

pub struct Agc {
    gain: f32,
    boost: f32,
}

impl Default for Agc {
    fn default() -> Self {
        Agc { gain: 1.0, boost: 1.0 }
    }
}

impl Agc {
    /// The player's own boost, in dB (0..=20).
    pub fn set_boost_db(&mut self, db: f32) {
        self.boost = 10f32.powf(db.clamp(0.0, 20.0) / 20.0);
    }

    pub fn gain(&self) -> f32 {
        self.gain * self.boost
    }

    /// The gated audio (48 kHz mono), in place.
    pub fn process(&mut self, x: &mut [f32]) {
        for piece in x.chunks_mut(PIECE) {
            let rms = (piece.iter().map(|v| v * v).sum::<f32>() / piece.len().max(1) as f32).sqrt() * self.boost;
            let from = self.gain;
            if rms > SPEECH {
                let want = (TARGET / rms).clamp(MIN_GAIN, MAX_GAIN);
                self.gain += (want - self.gain) * if want < self.gain { FALL } else { RISE };
            }
            let n = piece.len().max(1) as f32;
            for (i, v) in piece.iter_mut().enumerate() {
                let g = (from + (self.gain - from) * (i + 1) as f32 / n) * self.boost;
                *v = limit(*v * g);
            }
        }
    }
}

/// A soft limiter: untouched below 0.8, then bent smoothly toward 1.
fn limit(v: f32) -> f32 {
    let a = v.abs();
    if a <= 0.8 {
        v
    } else {
        v.signum() * (0.8 + 0.2 * ((a - 0.8) / 0.2).tanh())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| amp * (i as f32 * 0.06).sin()).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn a_quiet_voice_is_raised_a_loud_one_held_back() {
        // (a voice at -32 dBFS: raised to about the target within a few seconds)
        let mut a = Agc::default();
        let mut x = tone(48000 * 4, 0.035);
        a.process(&mut x);
        let late = rms(&x[48000 * 3..]);
        assert!((late / TARGET - 1.0).abs() < 0.15, "raised to {late}");
        // (a shout at -6 dBFS RMS: brought down to MIN_GAIN within a tenth of a second, peaks under 1)
        let mut b = Agc::default();
        let mut y = tone(48000, 0.7);
        b.process(&mut y);
        assert!((b.gain() - MIN_GAIN).abs() < 0.01, "held back: {}", b.gain());
        assert!((rms(&y[4800..]) - 0.7 / 2f32.sqrt() * MIN_GAIN).abs() < 0.02 && y.iter().all(|v| v.abs() < 1.0));
    }

    #[test]
    fn noise_is_not_raised_and_the_boost_counts() {
        let mut a = Agc::default();
        let mut x = tone(48000, 0.002);
        a.process(&mut x);
        assert!((a.gain() - 1.0).abs() < 1e-6, "quiet room: the gain stays");
        let mut b = Agc::default();
        b.set_boost_db(12.0);
        assert!((b.gain() - 3.98).abs() < 0.01);
        // (never past MAX_GAIN times the boost)
        let mut c = Agc::default();
        let mut z = tone(48000 * 6, 0.005);
        c.process(&mut z);
        assert!(c.gain() <= MAX_GAIN + 1e-3);
    }
}
