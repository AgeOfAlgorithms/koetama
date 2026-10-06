//! Resampling and wav files (engine/audio.py: resample, read_wav, load_wav).
use crate::mixer::RATE;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use std::io;
use std::path::Path;

/// The largest rate step (rate / gcd of the two rates) the FFT resampler is given: odd rates (no common factor)
/// would need a huge FFT, so those go linear, as Python's fallback.
const FFT_MAX_STEP: usize = 4096;

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// True when rubato's FFT resampler can take this rate pair (see FFT_MAX_STEP).
pub(crate) fn fft_ok(from: u32, to: u32) -> bool {
    let g = gcd(from as usize, to as usize).max(1);
    from > 0 && to > 0 && from as usize / g <= FFT_MAX_STEP && to as usize / g <= FFT_MAX_STEP
}

/// x (mono) from one sample rate to another: band-limited (rubato's FFT resampler), else linear.
/// (len(x) * to / from samples out, rounded up)
pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || x.is_empty() || from == 0 || to == 0 {
        return x.to_vec();
    }
    if fft_ok(from, to) {
        if let Ok(mut rs) = Fft::<f32>::new(from as usize, to as usize, 1024, 1, FixedSync::Input) {
            if let Ok(input) = InterleavedSlice::new(x, 1, x.len()) {
                if let Ok(y) = rs.process_all(&input, x.len(), None) {
                    return y.take_data();
                }
            }
        }
    }
    linear(x, from, to)
}

/// np.interp's resampling (Python's fallback)
fn linear(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    let n = (x.len() as f64 * to as f64 / from as f64).round() as usize;
    let step = from as f64 / to as f64;
    let last = x.len() - 1;
    (0..n)
        .map(|i| {
            let t = i as f64 * step;
            let j = (t.floor() as usize).min(last);
            if j >= last {
                return x[last];
            }
            let f = t - j as f64;
            (x[j] as f64 + (x[j + 1] as f64 - x[j] as f64) * f) as f32
        })
        .collect()
}

/// The q-th percentile (0..100) of x: numpy's default (linear between the two nearest ranks). Empty: 0.
pub fn percentile(x: &[f32], q: f64) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    let mut s: Vec<f32> = x.to_vec();
    s.sort_unstable_by(|a, b| a.total_cmp(b));
    let at = (q / 100.0).clamp(0.0, 1.0) * (s.len() - 1) as f64;
    let i = at.floor() as usize;
    let t = at - i as f64;
    let a = s[i] as f64;
    let b = s[(i + 1).min(s.len() - 1)] as f64;
    // (numpy's _lerp: from the nearer end)
    if t >= 0.5 {
        b - (b - a) * (1.0 - t)
    } else {
        a + (b - a) * t
    }
}

fn bad(path: &Path, e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display()))
}

/// (mono samples, rate) of a wav: 8/16/24/32-bit PCM or 32-bit float, the channels averaged.
pub fn read_wav(path: impl AsRef<Path>) -> io::Result<(Vec<f32>, u32)> {
    let path = path.as_ref();
    let r = hound::WavReader::open(path).map_err(|e| match e {
        hound::Error::IoError(e) => io::Error::new(e.kind(), format!("{}: {e}", path.display())),
        e => bad(path, e),
    })?;
    let spec = r.spec();
    let ch = spec.channels.max(1) as usize;
    let raw: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => r.into_samples::<f32>().collect::<Result<_, _>>().map_err(|e| bad(path, e))?,
        (hound::SampleFormat::Int, bits @ (8 | 16 | 24 | 32)) => {
            let scale = 1.0 / (1u64 << (bits - 1)) as f32; // (16-bit: / 32768, as Python)
            r.into_samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()
                .map_err(|e| bad(path, e))?
        }
        (f, b) => return Err(bad(path, format!("{b}-bit {f:?} wav: 16-bit PCM (or 32-bit float) expected"))),
    };
    let x = if ch == 1 {
        raw
    } else {
        raw.chunks_exact(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect()
    };
    Ok((x, spec.sample_rate))
}

/// Write x (mono, -1..1) as a 16-bit wav.
pub fn write_wav16(path: impl AsRef<Path>, x: &[f32], rate: u32) -> io::Result<()> {
    let path = path.as_ref();
    let spec = hound::WavSpec { channels: 1, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let err = |e: hound::Error| match e {
        hound::Error::IoError(e) => io::Error::new(e.kind(), format!("{}: {e}", path.display())),
        e => bad(path, e),
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(err)?;
    for &s in x {
        w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16).map_err(err)?;
    }
    w.finalize().map_err(err)
}

/// A voice clip for the mixer: at RATE, every clip equally loud (peaks at half scale), 0.5 s of quiet after.
pub fn load_wav(path: impl AsRef<Path>) -> io::Result<Vec<f32>> {
    let (x, sr) = read_wav(path)?;
    let mut x = resample(&x, sr, RATE);
    let mut loud = percentile(&x.iter().map(|v| v.abs()).collect::<Vec<_>>(), 99.9);
    if loud == 0.0 || !loud.is_finite() {
        loud = 1.0;
    }
    let g = (0.5 / loud) as f32;
    x.iter_mut().for_each(|v| *v *= g);
    x.resize(x.len() + (RATE as f64 * 0.5) as usize, 0.0);
    Ok(x)
}
