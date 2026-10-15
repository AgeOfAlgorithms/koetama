//! A microphone's channels to one. Many headset and USB microphones are a single capsule that the driver offers as a
//! 2-channel device - some copy the voice into both channels, some leave the second one silent, and some send the
//! second one with its polarity inverted. Averaging the channels halved a voice in a silent second channel (−6 dB:
//! seen 2026-10-10) and cancels one in an inverted channel almost entirely (a voice yelled into the microphone came
//! through barely audible: 2026-10-15, while other games heard the same microphone fine). So the channels are averaged
//! only when they carry the same signal; otherwise the channel that carries the voice is used alone. Each channel's
//! level, and how alike it is to the loudest one (their correlation), is followed over about a second: a channel
//! clearly louder than the rest (by LONE: 10 dB) is taken alone, and so is the loudest when any other channel of a
//! comparable level is not in phase with it (correlation under OUT_OF_PHASE). Channels alike in level and in phase (a
//! driver that copies, a real stereo or array microphone) are averaged as before.

/// a channel this many times (in energy) above the quietest one carries the voice alone (10 dB)
pub const LONE: f64 = 10.0;
/// s the channels' levels are followed over
pub const FOLLOW: f64 = 1.0;
/// a new loudest channel takes over only this many times (in energy) above the one in use (no flicker between two)
pub const SWITCH: f64 = 2.0;
/// channels whose correlation with the loudest is under this are not in phase with it: not averaged
pub const OUT_OF_PHASE: f64 = 0.5;
/// ... and averaged again only once every one is over this (no flicker)
pub const IN_PHASE: f64 = 0.7;

/// What the picker does with the channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// the channels' average (one channel, or channels alike)
    Average,
    /// this channel alone: the others are much quieter
    Louder(usize),
    /// this channel alone: another one of a like level is not in phase with it (inverted, or another signal)
    OutOfPhase(usize),
}

pub struct ChannelPicker {
    ch: usize,
    energy: Vec<f64>,
    /// the followed products of each pair of channels (ch x ch, row-major; the diagonal is `energy`)
    cross: Vec<f64>,
    choice: Choice,
}

impl ChannelPicker {
    pub fn new(channels: usize) -> ChannelPicker {
        let ch = channels.max(1);
        ChannelPicker { ch, energy: vec![0.0; ch], cross: vec![0.0; ch * ch], choice: Choice::Average }
    }

    /// The channel taken alone now (None: the channels' average).
    pub fn pick(&self) -> Option<usize> {
        match self.choice {
            Choice::Average => None,
            Choice::Louder(c) | Choice::OutOfPhase(c) => Some(c),
        }
    }

    /// What is done with the channels now, and why.
    pub fn choice(&self) -> Choice {
        self.choice
    }

    /// The correlation of channels a and b over the last second or so (-1 opposite .. 1 the same; 0 when one is silent).
    pub fn correlation(&self, a: usize, b: usize) -> f64 {
        let d = (self.energy[a] * self.energy[b]).sqrt();
        if d > 1e-12 {
            self.cross[a * self.ch + b] / d
        } else {
            0.0
        }
    }

    /// Interleaved frames `x` (whole frames) at `rate` Hz -> their mono samples, appended to `out`.
    pub fn mix(&mut self, x: &[f32], rate: u32, out: &mut Vec<f32>) {
        let ch = self.ch;
        if ch == 1 {
            out.extend_from_slice(x);
            return;
        }
        let frames = x.len() / ch;
        if frames == 0 {
            return;
        }
        let a = (-(frames as f64) / (rate.max(1) as f64 * FOLLOW)).exp();
        for i in 0..ch {
            for j in i..ch {
                let p = x.chunks_exact(ch).map(|f| f[i] as f64 * f[j] as f64).sum::<f64>() / frames as f64;
                let v = a * self.cross[i * ch + j] + (1.0 - a) * p;
                self.cross[i * ch + j] = v;
                self.cross[j * ch + i] = v;
            }
            self.energy[i] = self.cross[i * ch + i];
        }
        let (best, top) = self.energy.iter().copied().enumerate().fold((0, f64::MIN), |b, (i, e)| if e > b.1 { (i, e) } else { b });
        let quietest = self.energy.iter().copied().fold(f64::INFINITY, f64::min);
        // (the one in use stays unless a new loudest is clearly louder: no flicker between two)
        let keep = |p: usize| if self.energy[p] * SWITCH >= top { p } else { best };
        if top <= 1e-10 {
            self.choice = Choice::Average; // (silence)
        } else if top > LONE * quietest {
            self.choice = Choice::Louder(match self.choice {
                Choice::Louder(p) | Choice::OutOfPhase(p) => keep(p),
                Choice::Average => best,
            });
        } else if top < 0.5 * LONE * quietest {
            // (alike in level: in phase too? the lowest correlation of a comparable channel with the loudest)
            let lowest = (0..ch).filter(|&c| c != best).map(|c| self.correlation(best, c)).fold(f64::INFINITY, f64::min);
            self.choice = match self.choice {
                _ if lowest < OUT_OF_PHASE => Choice::OutOfPhase(match self.choice {
                    Choice::OutOfPhase(p) => keep(p),
                    _ => best,
                }),
                Choice::OutOfPhase(p) if lowest < IN_PHASE => Choice::OutOfPhase(keep(p)),
                _ => Choice::Average,
            };
        }
        // (between the level thresholds the choice stays: no flicker)
        match self.pick() {
            Some(p) => out.extend(x.chunks_exact(ch).map(|f| f[p])),
            None => out.extend(x.chunks_exact(ch).map(|f| f.iter().sum::<f32>() / ch as f32)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| amp * (i as f32 * 0.07).sin()).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    fn interleave(chs: &[Vec<f32>]) -> Vec<f32> {
        (0..chs[0].len()).flat_map(|i| chs.iter().map(move |c| c[i])).collect()
    }

    /// in blocks of 10 ms, as a device delivers
    fn run(p: &mut ChannelPicker, x: &[f32], ch: usize) -> Vec<f32> {
        let mut out = Vec::new();
        for b in x.chunks(480 * ch) {
            p.mix(b, 48000, &mut out);
        }
        out
    }

    #[test]
    fn a_silent_second_channel_no_longer_halves_the_voice() {
        let voice = tone(48000, 0.2);
        let x = interleave(&[voice.clone(), vec![0.0; 48000]]);
        let mut p = ChannelPicker::new(2);
        let out = run(&mut p, &x, 2);
        assert_eq!(p.choice(), Choice::Louder(0));
        let late = &out[24000..];
        assert!((rms(late) / rms(&voice[24000..]) - 1.0).abs() < 0.01, "full level: {} vs {}", rms(late), rms(&voice[24000..]));
        // (the voice in the right channel instead: found too)
        let mut p = ChannelPicker::new(2);
        run(&mut p, &interleave(&[vec![0.0; 48000], voice.clone()]), 2);
        assert_eq!(p.pick(), Some(1));
    }

    #[test]
    fn an_inverted_second_channel_does_not_cancel_the_voice() {
        // (the same voice, the second channel's polarity inverted: averaged, it would be silence)
        let voice = tone(48000, 0.2);
        let inverted: Vec<f32> = voice.iter().map(|v| -v * 0.95).collect();
        let mut p = ChannelPicker::new(2);
        let out = run(&mut p, &interleave(&[voice.clone(), inverted]), 2);
        assert_eq!(p.choice(), Choice::OutOfPhase(0));
        assert!(p.correlation(0, 1) < -0.9);
        let late = &out[24000..];
        assert!((rms(late) / rms(&voice[24000..]) - 1.0).abs() < 0.01, "full level: {}", rms(late));
        // (from the first block on: no quiet start)
        assert!(rms(&out[..4800]) > 0.5 * rms(&voice[..4800]), "first 0.1 s: {}", rms(&out[..4800]));
        // (a 4-channel array with one capsule inverted: the loudest alone too)
        let mut q = ChannelPicker::new(4);
        let neg: Vec<f32> = voice.iter().map(|v| -v).collect();
        run(&mut q, &interleave(&[voice.clone(), voice.clone(), neg, voice.clone()]), 4);
        assert!(matches!(q.choice(), Choice::OutOfPhase(_)));
    }

    #[test]
    fn alike_channels_and_mono_are_as_before() {
        let voice = tone(48000, 0.2);
        let mut p = ChannelPicker::new(2);
        let out = run(&mut p, &interleave(&[voice.clone(), voice.clone()]), 2);
        assert_eq!(p.choice(), Choice::Average, "a driver that copies: the average, the same level");
        assert!((rms(&out[24000..]) / rms(&voice[24000..]) - 1.0).abs() < 0.01);
        // (a stereo microphone: two capsules, the voice a little different in each - in phase: averaged)
        let other = tone(48000, 0.17);
        let near: Vec<f32> = other.iter().enumerate().map(|(i, v)| v + 0.02 * ((i as f32) * 0.31).sin()).collect();
        let mut s = ChannelPicker::new(2);
        run(&mut s, &interleave(&[voice.clone(), near]), 2);
        assert_eq!(s.choice(), Choice::Average);
        let mut m = ChannelPicker::new(1);
        let mut out = Vec::new();
        m.mix(&voice, 48000, &mut out);
        assert_eq!(out, voice, "mono: untouched");
        // (silence: the average, nothing picked)
        let mut z = ChannelPicker::new(2);
        run(&mut z, &vec![0.0; 9600], 2);
        assert_eq!(z.choice(), Choice::Average);
    }
}
