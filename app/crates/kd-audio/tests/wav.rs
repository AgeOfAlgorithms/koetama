//! Wav files, resampling, load_wav (engine/audio.py's read_wav, resample, load_wav).
mod common;

use kd_audio::*;
use std::f64::consts::PI;

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("kd-audio-test-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn sine(f: f64, rate: u32, seconds: f64, amp: f64) -> Vec<f32> {
    (0..(seconds * rate as f64) as usize).map(|i| (amp * (2.0 * PI * f * i as f64 / rate as f64).sin()) as f32).collect()
}

/// the amplitude of frequency f in x (a single-bin DFT)
fn tone(x: &[f32], rate: u32, f: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &v) in x.iter().enumerate() {
        let p = 2.0 * PI * f * i as f64 / rate as f64;
        re += v as f64 * p.cos();
        im += v as f64 * p.sin();
    }
    2.0 * (re * re + im * im).sqrt() / x.len() as f64
}

#[test]
fn wav16_round_trip() {
    let p = tmp("a.wav");
    let x = vec![0.0f32, 0.5, -0.5, 1.0, -1.0, 2.0, -2.0, 0.25];
    write_wav16(&p, &x, 16000).unwrap();
    let (y, sr) = read_wav(&p).unwrap();
    assert_eq!(sr, 16000);
    assert_eq!(y.len(), x.len());
    for (a, b) in x.iter().zip(&y) {
        assert!((a.clamp(-1.0, 1.0) - b).abs() < 1e-4, "{a} -> {b}");
    }
    assert_eq!(y[3], 32767.0 / 32768.0); // (as Python: int16 / 32768)
}

#[test]
fn float_stereo_wav_is_mixed_to_mono() {
    let p = tmp("stereo.wav");
    let spec = hound::WavSpec { channels: 2, sample_rate: 44100, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut w = hound::WavWriter::create(&p, spec).unwrap();
    for (l, r) in [(0.5f32, 0.1f32), (-1.0, 1.0), (0.2, 0.2)] {
        w.write_sample(l).unwrap();
        w.write_sample(r).unwrap();
    }
    w.finalize().unwrap();
    let (y, sr) = read_wav(&p).unwrap();
    assert_eq!(sr, 44100);
    assert_eq!(y, vec![0.3, 0.0, 0.2]);
}

#[test]
fn bad_files_are_readable_errors() {
    let e = read_wav(tmp("missing.wav")).unwrap_err();
    assert!(e.to_string().contains("missing.wav"), "{e}");
    let p = tmp("junk.wav");
    std::fs::write(&p, b"not a wav file at all").unwrap();
    let e = read_wav(&p).unwrap_err();
    assert!(e.to_string().contains("junk.wav"), "{e}");
    assert!(load_wav(&p).is_err());
}

#[test]
fn resample_keeps_tones_and_cuts_above_nyquist() {
    let x = sine(1000.0, 44100, 1.0, 0.5);
    let y = resample(&x, 44100, 48000);
    assert!((y.len() as i64 - 48000).abs() <= 1, "{}", y.len());
    let a = tone(&y[4800..43200], 48000, 1000.0);
    assert!((a - 0.5).abs() < 0.01, "1 kHz kept: {a}");
    // the start lines up (rubato's delay trimmed)
    let first = y.iter().position(|v| v.abs() > 0.1).unwrap();
    assert!(first < 10, "{first}");
    // 48k -> 16k: a 10 kHz tone is above the new Nyquist: gone, not folded down to 6 kHz
    let z = resample(&sine(10000.0, 48000, 1.0, 0.5), 48000, 16000);
    assert!((z.len() as i64 - 16000).abs() <= 1);
    assert!(common::rms(&z[1600..14400]) < 0.01, "band-limited: {}", common::rms(&z));
    // 16k -> 48k keeps a 3 kHz tone
    let u = resample(&sine(3000.0, 16000, 0.5, 0.3), 16000, 48000);
    assert!((tone(&u[2400..21600], 48000, 3000.0) - 0.3).abs() < 0.01);
    // the same rate, empty, an odd rate pair (linear, as Python's fallback)
    assert_eq!(resample(&x[..10], 44100, 44100), x[..10].to_vec());
    assert!(resample(&[], 16000, 48000).is_empty());
    let s = sine(500.0, 44101, 0.2, 0.5);
    let odd = resample(&s, 44101, 48000);
    assert_eq!(odd.len(), (s.len() as f64 * 48000.0 / 44101.0).round() as usize);
    assert!((tone(&odd, 48000, 500.0) - 0.5).abs() < 0.02);
}

#[test]
fn load_wav_as_python() {
    let p = tmp("voice.wav");
    let x = sine(440.0, 16000, 1.0, 0.2);
    write_wav16(&p, &x, 16000).unwrap();
    let clip = load_wav(&p).unwrap();
    assert_eq!(clip.len(), 48000 + 24000, "at RATE, then 0.5 s of quiet");
    assert!(clip[48000..].iter().all(|&v| v == 0.0));
    let abs: Vec<f32> = clip[..48000].iter().map(|v| v.abs()).collect();
    let p999 = percentile(&abs, 99.9);
    assert!((p999 - 0.5).abs() < 0.01, "peaks at half scale: {p999}");
    // all quiet: kept quiet (no division by zero)
    let q = tmp("quiet.wav");
    write_wav16(&q, &[0.0; 1600], 16000).unwrap();
    let c = load_wav(&q).unwrap();
    assert_eq!(c.len(), 4800 + 24000);
    assert!(c.iter().all(|&v| v == 0.0));
}
