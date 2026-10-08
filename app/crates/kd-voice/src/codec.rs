//! Opus (the crate opus-rs: libopus 1.6 ported to Rust): 48 kHz mono, 20 ms frames, 24 kbit/s, VOIP.
use crate::{BITRATE, FRAME, RATE};
use opus_rs::{Application, OpusDecoder, OpusEncoder};

/// the largest Opus packet (RFC 6716)
const MAX_BYTES: usize = 1276;

/// One player's voice into Opus frames.
pub struct Encoder {
    enc: OpusEncoder,
    buf: Vec<u8>,
}

impl Encoder {
    pub fn new() -> Result<Encoder, String> {
        let mut enc = OpusEncoder::new(RATE as i32, 1, Application::Voip).map_err(|e| format!("Opus: {e}"))?;
        enc.bitrate_bps = BITRATE;
        Ok(Encoder { enc, buf: vec![0; MAX_BYTES] })
    }

    /// One frame (FRAME samples, -1..1) -> its Opus bytes.
    pub fn encode(&mut self, pcm: &[f32]) -> Result<Vec<u8>, String> {
        if pcm.len() != FRAME {
            return Err(format!("Opus: a frame is {FRAME} samples, not {}", pcm.len()));
        }
        let n = self.enc.encode(pcm, FRAME, &mut self.buf).map_err(|e| format!("Opus: {e}"))?;
        Ok(self.buf[..n].to_vec())
    }
}

/// What the jitter buffer needs of a decoder (a stand-in in its tests).
pub trait FrameDecoder: Send {
    /// One frame's Opus bytes (None: the frame is missing - loss concealment) -> FRAME samples into out.
    fn decode(&mut self, frame: Option<&[u8]>, out: &mut [f32]);
}

/// One sender's Opus frames back into audio.
pub struct Decoder(OpusDecoder);

impl Decoder {
    pub fn new() -> Result<Decoder, String> {
        Ok(Decoder(OpusDecoder::new(RATE as i32, 1).map_err(|e| format!("Opus: {e}"))?))
    }
}

impl FrameDecoder for Decoder {
    fn decode(&mut self, frame: Option<&[u8]>, out: &mut [f32]) {
        if out.len() < FRAME {
            out.fill(0.0);
            return;
        }
        let out = &mut out[..FRAME];
        // (an empty packet: concealment - Opus fills in from what came before)
        let ok = match frame {
            Some(f) if f.len() > 1 && f.len() <= MAX_BYTES => self.0.decode(f, FRAME, out).is_ok(),
            _ => false,
        };
        if !ok && self.0.decode(&[], FRAME, out).is_err() {
            // (a frame that does not decode is concealed; if even that fails: silence)
            out.fill(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize, hz: f64) -> Vec<f32> {
        (0..n).map(|i| (0.4 * (2.0 * std::f64::consts::PI * hz * i as f64 / RATE as f64).sin()) as f32).collect()
    }

    fn rms(v: &[f32]) -> f64 {
        (v.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / v.len().max(1) as f64).sqrt()
    }

    #[test]
    fn a_tone_survives_the_codec() {
        let x = sine(FRAME * 50, 440.0);
        let (mut e, mut d) = (Encoder::new().unwrap(), Decoder::new().unwrap());
        let mut y = Vec::new();
        let mut bytes = 0;
        for f in x.chunks(FRAME) {
            let p = e.encode(f).unwrap();
            bytes += p.len();
            let mut out = vec![0.0; FRAME];
            d.decode(Some(&p), &mut out);
            y.extend(out);
        }
        // ~24 kbit/s (VBR: a pure tone takes a little more)
        let kbps = bytes as f64 * 8.0 / (x.len() as f64 / RATE as f64) / 1000.0;
        assert!((12.0..40.0).contains(&kbps), "{kbps} kbit/s");
        // the tone comes back at its level (after the codec's delay)
        let (a, b) = (rms(&x[FRAME * 10..]), rms(&y[FRAME * 10..]));
        assert!((b / a - 1.0).abs() < 0.2, "rms {a} -> {b}");
        // concealment fades out (never louder than the voice); junk is concealed too; no panic
        let mut out = vec![1.0; FRAME];
        let mut levels = Vec::new();
        for _ in 0..10 {
            d.decode(None, &mut out);
            levels.push(rms(&out));
        }
        assert!(levels.iter().all(|&l| l <= a * 1.2) && levels[9] < 0.01, "{levels:?}");
        d.decode(Some(&[0xff, 0xff, 0xff]), &mut out);
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        assert!(e.encode(&x[..10]).is_err());
    }
}
