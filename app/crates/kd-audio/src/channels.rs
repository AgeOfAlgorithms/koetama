//! A microphone's channels to one. Many headset and USB microphones are a single capsule that the driver offers as a
//! 2-channel device - some copy the voice into both channels, some leave the second one silent. Averaging the channels
//! halved such a voice (−6 dB, whispered or yelled: seen 2026-10-10, a player Koetama heard at half the level other games
//! did). So the channel that carries the voice is used: each channel's level is followed over about a second, and one
//! clearly louder than the rest (by LONE: 10 dB) is taken alone; channels alike (a real stereo microphone, or a driver
//! that copies) are averaged as before.

/// a channel this many times (in energy) above the quietest one carries the voice alone (10 dB)
pub const LONE: f64 = 10.0;
/// s the channels' levels are followed over
pub const FOLLOW: f64 = 1.0;
/// a new loudest channel takes over only this many times (in energy) above the one in use (no flicker between two)
pub const SWITCH: f64 = 2.0;

pub struct ChannelPicker {
    ch: usize,
    energy: Vec<f64>,
    /// the channel taken alone (None: the average)
    pick: Option<usize>,
}

impl ChannelPicker {
    pub fn new(channels: usize) -> ChannelPicker {
        let ch = channels.max(1);
        ChannelPicker { ch, energy: vec![0.0; ch], pick: None }
    }

    /// The channel taken alone now (None: the channels' average).
    pub fn pick(&self) -> Option<usize> {
        self.pick
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
        for c in 0..ch {
            let e = x.chunks_exact(ch).map(|f| (f[c] as f64) * (f[c] as f64)).sum::<f64>() / frames as f64;
            self.energy[c] = a * self.energy[c] + (1.0 - a) * e;
        }
        let (best, top) = self.energy.iter().copied().enumerate().fold((0, f64::MIN), |b, (i, e)| if e > b.1 { (i, e) } else { b });
        let quietest = self.energy.iter().copied().fold(f64::INFINITY, f64::min);
        if top > 1e-10 && top > LONE * quietest {
            match self.pick {
                Some(p) if self.energy[p] * SWITCH >= top => {}
                _ => self.pick = Some(best),
            }
        } else if top <= 1e-10 || top < 0.5 * LONE * quietest {
            // (alike again - or silence: the average; between the two thresholds the choice stays, no flicker)
            self.pick = None;
        }
        match self.pick {
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
        assert_eq!(p.pick(), Some(0));
        let late = &out[24000..];
        assert!((rms(late) / rms(&voice[24000..]) - 1.0).abs() < 0.01, "full level: {} vs {}", rms(late), rms(&voice[24000..]));
        // (the voice in the right channel instead: found too)
        let mut p = ChannelPicker::new(2);
        run(&mut p, &interleave(&[vec![0.0; 48000], voice.clone()]), 2);
        assert_eq!(p.pick(), Some(1));
    }

    #[test]
    fn alike_channels_and_mono_are_as_before() {
        let voice = tone(48000, 0.2);
        let mut p = ChannelPicker::new(2);
        let out = run(&mut p, &interleave(&[voice.clone(), voice.clone()]), 2);
        assert_eq!(p.pick(), None, "a driver that copies: the average, the same level");
        assert!((rms(&out[24000..]) / rms(&voice[24000..]) - 1.0).abs() < 0.01);
        let mut m = ChannelPicker::new(1);
        let mut out = Vec::new();
        m.mix(&voice, 48000, &mut out);
        assert_eq!(out, voice, "mono: untouched");
        // (silence: the average, nothing picked)
        let mut s = ChannelPicker::new(2);
        run(&mut s, &vec![0.0; 9600], 2);
        assert_eq!(s.pick(), None);
    }
}
