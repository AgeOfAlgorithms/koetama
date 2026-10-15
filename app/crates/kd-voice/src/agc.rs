//! Automatic gain for the voice this player sends, as other games' voice chat does: a quiet microphone is raised toward
//! a normal speaking level, a loud one held back, so every player comes through about as loud. It follows the level of
//! the speech (its power over the 10 ms pieces with speech, over a second or two: fast only at the first speech) and sets the gain from that,
//! so it aims at the voice's average, not at its loudest syllables. Only what the gate lets through is seen (speech,
//! not the room's quiet); a piece far quieter than the speech (a pause's noise) is not followed; the gain is never
//! raised past MAX_GAIN, and a soft limiter keeps peaks under full scale. The level is kept from one line to the next,
//! and the first 100 ms of speech set it (no quiet start). `boost` (dB, the player's "Mic boost") comes first.
//!
//! (2026-10-15, measured on recorded speech: a microphone whose speech is at -40 dBFS came out at -26 dBFS with the
//! first version - a target of -20 dBFS, at most +18 dB, a gain that fell fast and rose slowly, so it sat on the loud
//! syllables - and quiet microphones came through ~10 dB under normal ones, hot ones ~9 dB over (kd-voice's voice_zoo).
//! The target stays -20 dBFS: a normal microphone sounds as before.)

/// the speaking level aimed at (RMS of the speech): -20 dBFS (the level players have heard since 0.5.1)
pub const TARGET: f32 = 0.1;
/// the most it raises a voice: +30 dB
pub const MAX_GAIN: f32 = 31.6;
/// the most it lowers one: -20 dB (a microphone clipping at full scale)
pub const MIN_GAIN: f32 = 0.1;
/// a 10 ms piece quieter than this is never speech: -54 dBFS
pub const SPEECH: f32 = 0.002;
/// nor one this much quieter than the speech's level (a power ratio: 15 dB)
pub const BELOW: f32 = 0.0316;
/// how fast the level follows the speech, per 10 ms piece: toward a louder piece / a quieter one (slow: syllables keep
/// their rise and fall - a fast one flattened speech)
pub const UP: f32 = 0.05;
pub const DOWN: f32 = 0.005;
/// for the first this many pieces of speech the level follows fast (UP_FIRST, DOWN_FIRST): no quiet start
pub const FIRST: u32 = 40;
pub const UP_FIRST: f32 = 0.3;
pub const DOWN_FIRST: f32 = 0.1;
/// how fast the gain follows the level, per 10 ms piece
pub const SMOOTH: f32 = 0.1;
/// the first level is the average of this many pieces of speech (100 ms: one piece is often a quiet onset, and a gain
/// set from it overshot the first syllable)
pub const WAIT: u32 = 10;
const PIECE: usize = 480;

pub struct Agc {
    gain: f32,
    boost: f32,
    /// the speech's level (power) so far; None until the first speech
    level: Option<f32>,
    /// pieces of speech so far (up to FIRST)
    pieces: u32,
    /// before the first level: the power of the pieces of speech so far, summed, and how many
    first: (f32, u32),
}

impl Default for Agc {
    fn default() -> Self {
        Agc { gain: 1.0, boost: 1.0, level: None, pieces: 0, first: (0.0, 0) }
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
            let power = rms * rms;
            let from = self.gain;
            if rms > SPEECH && self.level.is_none_or(|l| power > l * BELOW) {
                let level = match self.level {
                    None if self.first.1 + 1 < WAIT => {
                        self.first = (self.first.0 + power, self.first.1 + 1);
                        // (meanwhile only down: a hot microphone's first syllable is not let through at full scale)
                        let so_far = self.first.0 / self.first.1 as f32;
                        let want = (TARGET / so_far.sqrt()).clamp(MIN_GAIN, 1.0);
                        if want < self.gain {
                            self.gain += (want - self.gain) * 0.5;
                        }
                        None
                    }
                    None => Some((self.first.0 + power) / WAIT as f32),
                    Some(l) => Some({
                        let (up, down) = if self.pieces < FIRST { (UP_FIRST, DOWN_FIRST) } else { (UP, DOWN) };
                        l + (power - l) * if power > l { up } else { down }
                    }),
                };
                if let Some(level) = level {
                    self.pieces = (self.pieces + 1).min(FIRST);
                    self.level = Some(level);
                    let want = (TARGET / level.sqrt()).clamp(MIN_GAIN, MAX_GAIN);
                    self.gain += (want - self.gain) * SMOOTH;
                }
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
    fn a_quiet_voice_is_raised_at_once() {
        // (a voice at -32 dBFS: at the target within half a second - the first 100 ms of speech set the level)
        let mut a = Agc::default();
        let mut x = tone(48000, 0.035);
        a.process(&mut x);
        assert!((rms(&x[24000..]) / TARGET - 1.0).abs() < 0.05, "raised to {}", rms(&x[24000..]));
        // (a very quiet one, -51 dBFS: raised as far as MAX_GAIN allows)
        let mut b = Agc::default();
        let mut y = tone(48000, 0.004);
        b.process(&mut y);
        assert!((b.gain() / MAX_GAIN - 1.0).abs() < 0.01, "raised by {}", b.gain());
    }

    #[test]
    fn a_shout_is_held_back_fast_and_never_clips() {
        // (talking at -32 dBFS, then a shout at -5 dBFS: down to its gain within 0.6 s, no sample past full scale)
        let mut a = Agc::default();
        let mut x = tone(48000, 0.035);
        a.process(&mut x);
        let mut y = tone(48000, 0.8);
        a.process(&mut y);
        let want = (TARGET / (0.8 / 2f32.sqrt())).max(MIN_GAIN);
        assert!(y.iter().all(|v| v.abs() <= 1.0));
        assert!((rms(&y[48000 * 6 / 10..]) / (0.8 / 2f32.sqrt() * want) - 1.0).abs() < 0.1, "held back to {}", a.gain());
    }

    #[test]
    fn noise_and_pauses_are_not_followed() {
        // (a quiet room: not speech, the gain stays)
        let mut a = Agc::default();
        let mut x = tone(48000, 0.0015);
        a.process(&mut x);
        assert!((a.gain() - 1.0).abs() < 1e-6, "quiet room: the gain stays");
        // (talking, then a pause's noise 20 dB under the speech: the gain does not climb in the pause)
        let mut b = Agc::default();
        let mut s = tone(48000, 0.1);
        b.process(&mut s);
        let g = b.gain();
        let mut n = tone(48000, 0.01);
        b.process(&mut n);
        assert!((b.gain() - g).abs() < 1e-6, "the pause left the gain alone: {g} -> {}", b.gain());
    }

    #[test]
    fn the_level_carries_to_the_next_line_and_the_boost_counts() {
        let mut a = Agc::default();
        let mut x = tone(48000, 0.035);
        a.process(&mut x);
        let g = a.gain();
        let mut y = tone(4800, 0.035);
        a.process(&mut y);
        assert!((rms(&y) / TARGET - 1.0).abs() < 0.05, "the next line starts at the level: {}", rms(&y));
        assert!((a.gain() - g).abs() / g < 0.01);
        let mut b = Agc::default();
        b.set_boost_db(12.0);
        assert!((b.gain() - 3.98).abs() < 0.01);
        // (never past MAX_GAIN times the boost)
        let mut c = Agc::default();
        c.set_boost_db(6.0);
        let mut z = tone(48000 * 2, 0.003);
        c.process(&mut z);
        assert!(c.gain() <= MAX_GAIN * 2.0 + 1e-2);
    }
}
