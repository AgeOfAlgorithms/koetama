//! The Listener end to end on benchmark clips against the Python Listener (app/fixtures/speech.json:
//! make_speech_fixtures.py): fed as teardown_helper.py --transcribe does (1 s of quiet after, 800-sample blocks).
//! Needs the models: `cargo test -p kd-speech -- --include-ignored`.
use kd_speech::*;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn read_wav(rel: &str) -> Vec<f32> {
    let mut r = hound::WavReader::open(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    assert_eq!(r.spec().sample_rate, RATE);
    r.samples::<i16>().map(|v| v.unwrap() as f32 / 32768.0).collect()
}

fn models() -> Arc<Models> {
    static M: OnceLock<Arc<Models>> = OnceLock::new();
    M.get_or_init(|| Models::new(4, kd_common::stdout_log())).clone()
}

/// (one test at a time: they share the models and Models::every)
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static S: Mutex<()> = Mutex::new(());
    S.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Default)]
struct Heard {
    starts: Vec<u32>,
    lives: Vec<(u32, String, Vec<f64>)>,
    finals: Vec<(u32, String, FinalInfo)>,
}

fn listener(live: bool) -> (Listener, Arc<Mutex<Heard>>) {
    let heard = Arc::new(Mutex::new(Heard::default()));
    let (a, b, c) = (heard.clone(), heard.clone(), heard.clone());
    let cb = Callbacks {
        on_start: Box::new(move |u| a.lock().unwrap().starts.push(u)),
        on_live: Box::new(move |u, t, i| b.lock().unwrap().lives.push((u, t.to_string(), i.times.clone()))),
        on_final: Box::new(move |u, t, i| c.lock().unwrap().finals.push((u, t.to_string(), i.clone()))),
    };
    (Listener::new(cb, models(), live).unwrap(), heard)
}

/// a case's audio: its clips' pieces [path, from s, to s|null] glued
fn audio(case: &Value) -> Vec<f32> {
    let mut x = Vec::new();
    for p in case["parts"].as_array().unwrap() {
        let a = read_wav(&format!("export/asrbench/{}", p[0].as_str().unwrap()));
        let s0 = (p[1].as_f64().unwrap() * RATE as f64) as usize;
        let s1 = p[2].as_f64().map(|t| (t * RATE as f64) as usize).unwrap_or(a.len()).min(a.len());
        x.extend_from_slice(&a[s0..s1]);
    }
    x
}

fn close(a: &[f64], b: &[f64], tol: f64) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
}

fn f64s(v: &Value) -> Vec<f64> {
    v.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect()
}

#[test]
#[ignore]
fn lines_as_python() {
    let _one = serial();
    let p = root().join("app/fixtures/speech.json");
    let f: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    for case in f["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let (lst, heard) = listener(true);
        lst.models().set_every(ROLL_EVERY);
        lst.set_language(case["lang"].as_str().unwrap());
        let t0 = Instant::now();
        lst.warm(None).unwrap();
        let warm_s = t0.elapsed().as_secs_f64();
        let mut x = audio(case);
        x.resize(x.len() + RATE as usize, 0.0);
        let t0 = Instant::now();
        for blk in x.chunks(800) {
            lst.feed(blk);
        }
        lst.flush();
        let took = t0.elapsed().as_secs_f64();
        assert!(!lst.talking(), "{name}: the line ended");
        let h = heard.lock().unwrap();
        println!("{name}: {:.1} s of audio in {took:.2} s (Python {:.2} s; warm {warm_s:.1} s), every {} s",
            x.len() as f64 / RATE as f64, case["took"].as_f64().unwrap(), lst.models().every());
        for (u, t, _) in &h.lives {
            println!("  live {u}: {t}");
        }
        for (u, t, i) in &h.finals {
            println!("  LINE {u}: {t}   [{}; {}; {:.2} s of speech; {} passes; final pass {:.2} s]", i.used, i.lang, i.speech, i.passes, i.second_s);
        }
        let want = case["finals"].as_array().unwrap();
        assert_eq!(h.finals.len(), want.len(), "{name}: finals");
        let starts: Vec<u32> = case["starts"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u32).collect();
        assert_eq!(h.starts, starts, "{name}: on_start");
        for ((u, t, i), w) in h.finals.iter().zip(want) {
            assert_eq!(*u as u64, w["utt"].as_u64().unwrap(), "{name}");
            assert_eq!(t, w["text"].as_str().unwrap(), "{name}: final text");
            assert_eq!(i.used, w["used"].as_str().unwrap(), "{name}: used");
            assert_eq!(i.lang, w["lang"].as_str().unwrap(), "{name}: lang");
            assert!((i.speech - w["speech"].as_f64().unwrap()).abs() < 1e-6, "{name}: speech {}", i.speech);
            assert!(close(&i.times, &f64s(&w["times"]), 0.011), "{name}: times {:?}", i.times);
            assert_eq!(i.times.len(), kd_common::text::units(t).len(), "{name}: one time per unit");
        }
        // live words: only ever growing, before the final; the same as Python's when both kept a pass a second
        for w in h.lives.windows(2) {
            assert!(w[1].0 != w[0].0 || w[1].1.starts_with(&w[0].1), "{name}: live words grow: {:?}", h.lives);
        }
        let speech = h.finals.iter().map(|f| f.2.speech).fold(0.0, f64::max);
        if speech >= 2.5 {
            assert!(!h.lives.is_empty(), "{name}: live words before a long line's final");
        }
        let want_lives = case["lives"].as_array().unwrap();
        if lst.models().every() == ROLL_EVERY && case["every"].as_f64() == Some(ROLL_EVERY) {
            let lives: Vec<&str> = h.lives.iter().map(|l| l.1.as_str()).collect();
            let wl: Vec<&str> = want_lives.iter().map(|l| l["text"].as_str().unwrap()).collect();
            assert_eq!(lives, wl, "{name}: live words as Python's");
            assert_eq!(h.finals[0].2.live, want[0]["live"].as_str().unwrap(), "{name}: info.live");
            assert_eq!(h.finals[0].2.passes as u64, want[0]["passes"].as_u64().unwrap(), "{name}: passes");
        }
    }
}

#[test]
#[ignore]
fn worker_and_wav_microphone() {
    let _one = serial();
    // the worker: blocks pushed (a whole clip at once fits the queue), stop() finishes the line
    let (lst, heard) = listener(true);
    lst.models().set_every(ROLL_EVERY);
    lst.set_language("en");
    lst.warm(None).unwrap();
    let mut x = read_wav("export/asrbench/clips/en01_room.wav");
    x.resize(x.len() + RATE as usize, 0.0);
    lst.start();
    assert!(lst.started());
    for blk in x.chunks(800) {
        lst.push(blk);
    }
    let t0 = Instant::now();
    while heard.lock().unwrap().finals.is_empty() && t0.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(50));
    }
    lst.stop();
    assert!(!lst.started());
    {
        let h = heard.lock().unwrap();
        assert_eq!(h.finals.len(), 1, "worker: one line");
        assert_eq!(h.finals[0].1, "Hello. Can anyone hear me?");
    }
    // a recording as the microphone, in real time, into the worker
    let (lst, heard) = listener(true);
    lst.set_language("en");
    lst.start();
    let mut mic = WavMicrophone::new(lst.clone(), read_wav("export/asrbench/clips/en04_room.wav"), kd_common::stdout_log());
    assert!(!mic.is_open());
    assert!(mic.open());
    assert!(mic.is_open());
    let t0 = Instant::now();
    let mut loudest = -120.0f64;
    let mut talked = false;
    while heard.lock().unwrap().finals.is_empty() && t0.elapsed() < Duration::from_secs(20) {
        loudest = loudest.max(mic.level());
        talked |= lst.talking();
        std::thread::sleep(Duration::from_millis(20));
    }
    let real = t0.elapsed().as_secs_f64();
    mic.close();
    lst.stop();
    let h = heard.lock().unwrap();
    println!("microphone: final after {real:.1} s, loudest block {loudest:.1} dBFS, {} live updates", h.lives.len());
    assert_eq!(h.finals.len(), 1);
    assert_eq!(h.finals[0].1, "There is something in the basement. Do not go down there alone.");
    assert!(talked, "talking() while the line went on");
    assert!(!h.lives.is_empty(), "live words before the final");
    assert!(real > 4.0, "real time: the clip's speech ends after ~4.2 s ({real:.1} s)");
    assert!(loudest > -40.0 && loudest < 0.0, "level in dBFS: {loudest}");
}

#[test]
#[ignore]
fn playlist_microphone() {
    let _one = serial();
    // (two recorded lines, each in its language: the listener switched to it, its model written them)
    let (lst, heard) = listener(true);
    lst.start();
    let items = vec![
        ("en".to_string(), read_wav("export/asrbench/clips/en01_room.wav"), "Hello, can anyone hear me?".to_string()),
        ("ru".to_string(), read_wav("export/asrbench/clips/ru02_room.wav"), "ru02".to_string()),
    ];
    let mut mic = PlaylistMicrophone::new(lst.clone(), items, 0.5, kd_common::stdout_log());
    mic.open();
    let t0 = Instant::now();
    while !(mic.done() && heard.lock().unwrap().finals.len() >= 2) && t0.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(50));
    }
    mic.close();
    lst.stop();
    let h = heard.lock().unwrap();
    let lines: Vec<(&str, &str)> = h.finals.iter().map(|f| (f.1.as_str(), f.2.used.as_str())).collect();
    println!("playlist: {lines:?} in {:.1} s", t0.elapsed().as_secs_f64());
    assert!(mic.done());
    assert_eq!(lines, [("Hello. Can anyone hear me?", "parakeet"), ("Я нашел ключ за картиной в большом зале", "gigaam")]);
    assert_eq!(lst.language(), "ru");
}

#[test]
#[ignore]
fn hiss_is_no_line() {
    let _one = serial();
    // (test_asr.py: six seconds of hiss - no line; only the speech detector runs)
    let (lst, heard) = listener(true);
    let mut s = 9u64;
    let noise: Vec<f32> = (0..RATE as usize * 6)
        .map(|_| {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((s >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.035 // (uniform, the rms of 0.01 normal noise)
        })
        .collect();
    for blk in noise.chunks(800) {
        lst.feed(blk);
    }
    lst.flush();
    let h = heard.lock().unwrap();
    assert!(h.finals.iter().all(|f| f.1.trim().is_empty()), "{:?}", h.finals.iter().map(|f| &f.1).collect::<Vec<_>>());
}

#[test]
#[ignore]
fn auto_line_without_live_words() {
    let _one = serial();
    // ("auto": a Russian line found as Russian, written by GigaAM; live words off: only the final pass)
    let (lst, heard) = listener(false);
    lst.set_language("auto");
    lst.warm(None).unwrap();
    let mut x = read_wav("export/asrbench/clips/ru02_room.wav");
    x.resize(x.len() + RATE as usize, 0.0);
    for blk in x.chunks(800) {
        lst.feed(blk);
    }
    lst.flush();
    let h = heard.lock().unwrap();
    assert_eq!(h.finals.len(), 1);
    assert_eq!(h.finals[0].2.lang, "ru", "{:?}", h.finals[0]);
    assert!(h.lives.is_empty(), "live words off");
    assert_eq!(h.finals[0].2.passes, 0);
}
