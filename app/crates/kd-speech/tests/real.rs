//! The real models on the benchmark clips against the Python answers (app/fixtures/real.json: make_fixtures.py --real).
//! Needs the models (the Hugging Face cache / Koetama's models folder) and export/ (the clips, the language
//! detector): `cargo test -p kd-speech -- --include-ignored`.
use kd_speech::*;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn fixture() -> Value {
    let p = root().join("app/fixtures/real.json");
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap()
}

/// a 16-bit wav as Python's audio.read_wav: mono float32 /32768
fn read_wav(rel: &str) -> Vec<f32> {
    let mut r = hound::WavReader::open(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    let spec = r.spec();
    assert_eq!(spec.sample_rate, RATE, "{rel}");
    let s: Vec<f32> = r.samples::<i16>().map(|v| v.unwrap() as f32 / 32768.0).collect();
    let ch = spec.channels as usize;
    s.chunks(ch).map(|c| c.iter().sum::<f32>() / ch as f32).collect()
}

fn models() -> Arc<Models> {
    static M: OnceLock<Arc<Models>> = OnceLock::new();
    M.get_or_init(|| {
        let m = Models::new(4, kd_common::stdout_log());
        for name in ["parakeet", "gigaam", "sensevoice", "langid"] {
            m.load(name).unwrap();
        }
        m
    })
    .clone()
}

fn f64s(v: &Value) -> Vec<f64> {
    v.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect()
}

fn close(a: &[f64], b: &[f64], tol: f64) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
}

#[test]
#[ignore]
fn offline_transcripts_as_python() {
    let m = models();
    let f = fixture();
    for c in f["offline"].as_array().unwrap() {
        let (id, model) = (c["id"].as_str().unwrap(), c["model"].as_str().unwrap());
        let x = read_wav(c["path"].as_str().unwrap());
        let (raw, tokens, stamps, took) = m.offline_full(model, &x).unwrap();
        println!("{id} [{model}] {:.2} s: {raw}", took);
        assert_eq!(raw, c["raw"].as_str().unwrap(), "{id}: raw text");
        let want_tokens: Vec<&str> = c["tokens"].as_array().unwrap().iter().map(|t| t.as_str().unwrap()).collect();
        assert_eq!(tokens, want_tokens, "{id}: tokens");
        let stamps: Vec<f64> = stamps.iter().map(|&s| s as f64).collect();
        assert!(close(&stamps, &f64s(&c["stamps"]), 0.011), "{id}: timestamps {stamps:?}");
        let (text, times, _) = m.offline_timed(model, &x, 0.25).unwrap();
        assert_eq!(text, c["text"].as_str().unwrap(), "{id}: tidy text");
        assert!(close(&times, &f64s(&c["times"]), 0.011), "{id}: unit times {times:?} != {:?}", c["times"]);
    }
}

#[test]
#[ignore]
fn lid_probs_as_python() {
    let m = models();
    let f = fixture();
    for c in f["lid"].as_array().unwrap() {
        let x = read_wav(c["path"].as_str().unwrap());
        let (a, b) = (c["a"].as_f64().unwrap(), c["b"].as_f64().unwrap());
        let seg = &x[(a * RATE as f64) as usize..((b * RATE as f64) as usize).min(x.len())];
        let p = m.lid_probs(seg).unwrap();
        let want = f64s(&c["p"]);
        assert!(close(&p, &want, 1e-4), "{} {a}-{b}: {p:?} != {want:?}", c["id"]);
    }
    let x = read_wav("export/asrbench/lid/line_en01_room.wav");
    let t0 = Instant::now();
    for k in 0..10 {
        m.lid_probs(&x[k * 1600..k * 1600 + 16000]).unwrap();
    }
    println!("language detector: {:.4} s a 1 s window", t0.elapsed().as_secs_f64() / 10.0);
}

#[test]
#[ignore]
fn mixed_lines_as_python() {
    let m = models();
    let f = fixture();
    for c in f["mixed"].as_array().unwrap() {
        let id = c["id"].as_str().unwrap();
        let x = read_wav(c["path"].as_str().unwrap());
        let r = transcribe_mixed(&m, &x, "en", None).unwrap();
        println!("{id} {:.2} s: {:?} {}", r.took, r.segs, r.text);
        let want = c["segs"].as_array().unwrap();
        assert_eq!(r.segs.len(), want.len(), "{id}: {:?} != {want:?}", r.segs);
        for (s, w) in r.segs.iter().zip(want) {
            assert_eq!(s.0, w[0].as_str().unwrap(), "{id}: {:?} != {want:?}", r.segs);
            assert!((s.1 - w[1].as_f64().unwrap()).abs() <= 0.02 && (s.2 - w[2].as_f64().unwrap()).abs() <= 0.02,
                "{id}: {:?} != {want:?}", r.segs);
        }
        assert_eq!(r.text, c["text"].as_str().unwrap(), "{id}: text");
        assert!(close(&r.times, &f64s(&c["times"]), 0.02), "{id}: times {:?} != {:?}", r.times, c["times"]);
        assert_eq!(r.langs, r.segs.iter().map(|s| s.0.clone()).collect::<Vec<_>>());
    }
}

/// Speeds next to Python's (app/fixtures/speech.json "timing", make_speech_fixtures.py): a pass over the 7.65 s
/// en04+en02 line through Parakeet, a 1 s window through the language detector.
#[test]
#[ignore]
fn timings() {
    let m = models();
    let p = root().join("app/fixtures/speech.json");
    let f: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    let a = read_wav("export/asrbench/clips/en04_room.wav");
    let b = read_wav("export/asrbench/clips/en02_room.wav");
    let mut x = a[..((4.22 + 0.1) * RATE as f64) as usize].to_vec();
    x.extend_from_slice(&b[((0.5 - 0.1) * RATE as f64) as usize..]);
    m.offline_full("parakeet", &x).unwrap();
    let t0 = Instant::now();
    for _ in 0..3 {
        m.offline_full("parakeet", &x).unwrap();
    }
    let pass = t0.elapsed().as_secs_f64() / 3.0;
    for k in 0..5 {
        m.lid_probs(&x[k * 1600..k * 1600 + 16000]).unwrap();
    }
    let t0 = Instant::now();
    for k in 0..10 {
        m.lid_probs(&x[k * 1600..k * 1600 + 16000]).unwrap();
    }
    let lid = t0.elapsed().as_secs_f64() / 10.0;
    let t = &f["timing"];
    println!("Parakeet pass over {:.2} s: {pass:.3} s (Python {}); language detector 1 s window: {lid:.4} s (Python {})",
        x.len() as f64 / RATE as f64, t["parakeet_pass_6s"], t["lid_window"]);
}
