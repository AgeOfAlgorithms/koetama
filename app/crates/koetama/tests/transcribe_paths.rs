//! Does the voice chat's audio path hurt the speech to text? The benchmark clips (export/asrbench, both the clean and
//! the room recordings) through the real listener three ways, each scored against the clip's own text:
//!   as is        16 kHz, straight in (what the listener got before the voice chat)
//!   via 48 kHz   16 -> 48 kHz -> 16 kHz with the microphone's own converter (kd_audio::Rechunk, as mic.rs: with the
//!                voice chat the microphone runs at 48 kHz and the listener gets a 16 kHz copy)
//!   via Opus     the same, with an Opus round trip at 48 kHz in between (kd_voice's codec, 24 kbit/s): what another
//!                player's game would hear (Koetama never transcribes it today)
//! Word error rate per language (characters for Chinese). Needs the models, a few minutes (a report, no assertion:
//! measured 2026-10-07 - as is 4.3 %, via 48 kHz 5.3 %, via Opus 5.4 %; German room clips suffer most under Opus):
//!   cargo test -p koetama --release --test transcribe_paths -- --ignored --nocapture
use kd_audio::{read_wav, Rechunk};
use kd_speech::{Callbacks, Listener, Models, RATE};
use kd_voice::codec::{Decoder, Encoder, FrameDecoder};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// x through a Rechunk (from -> to Hz), with its tail flushed
fn convert(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    let chunk = from as usize / 100;
    let mut r = Rechunk::new(from, to, chunk, 1).unwrap();
    let mut out = Vec::new();
    r.push(x, &mut |y| out.extend_from_slice(y));
    r.push(&vec![0.0; 2 * chunk], &mut |y| out.extend_from_slice(y));
    out
}

/// 48 kHz audio through Opus and back, frame by frame
fn opus(x: &[f32]) -> Vec<f32> {
    let (mut e, mut d) = (Encoder::new().unwrap(), Decoder::new().unwrap());
    let f = kd_voice::FRAME;
    let mut out = vec![0.0; f];
    let mut y = Vec::with_capacity(x.len());
    for chunk in x.chunks(f) {
        let mut frame = chunk.to_vec();
        frame.resize(f, 0.0);
        let bytes = e.encode(&frame).unwrap();
        d.decode(Some(&bytes), &mut out);
        y.extend_from_slice(&out);
    }
    y
}

/// the text's units, lower case, without punctuation (CJK: one per character)
fn units(t: &str) -> Vec<String> {
    kd_common::text::units(t)
        .into_iter()
        .map(|(_, u)| u.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>())
        .filter(|u| !u.is_empty())
        .collect()
}

fn edits(a: &[String], b: &[String]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, y) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(x != y)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

fn transcribe(models: &Arc<Models>, lang: &str, x: &[f32]) -> String {
    let said = Arc::new(Mutex::new(Vec::<String>::new()));
    let s = said.clone();
    let cb = Callbacks {
        on_start: Box::new(|_| {}),
        on_live: Box::new(|_, _, _| {}),
        on_final: Box::new(move |_, t, _| s.lock().unwrap().push(t.to_string())),
    };
    let l = Listener::new(cb, models.clone(), false).unwrap();
    l.set_language(lang);
    l.warm(None).unwrap();
    let mut x = x.to_vec();
    x.resize(x.len() + RATE as usize, 0.0);
    for b in x.chunks(800) {
        l.feed(b);
    }
    l.flush();
    let t = said.lock().unwrap().join(" ");
    t
}

#[test]
#[ignore]
fn the_voice_chats_audio_path_keeps_the_words() {
    let clips: Value =
        serde_json::from_str(&std::fs::read_to_string(root().join("export/asrbench/clips.json")).unwrap()).unwrap();
    let models = Models::new(4, kd_common::null_log());
    const WAYS: [&str; 3] = ["as is", "via 48 kHz", "via Opus"];
    // lang -> per way (errors, reference units)
    let mut score: BTreeMap<String, [(usize, usize); 3]> = BTreeMap::new();
    let mut worse = Vec::new();
    for c in clips.as_array().unwrap() {
        let (id, lang, text) = (c["id"].as_str().unwrap(), c["lang"].as_str().unwrap(), c["text"].as_str().unwrap());
        let want = units(text);
        for kind in ["clean", "room"] {
            let path = root().join(format!("export/asrbench/clips/{id}_{kind}.wav"));
            let Ok((x, sr)) = read_wav(&path) else { continue };
            assert_eq!(sr, RATE, "{id}_{kind}: 16 kHz clips");
            let up = convert(&x, RATE, kd_voice::RATE);
            let ways = [x.clone(), convert(&up, kd_voice::RATE, RATE), convert(&opus(&up), kd_voice::RATE, RATE)];
            let mut errs = [0usize; 3];
            let mut heard = Vec::new();
            for (k, w) in ways.iter().enumerate() {
                let got = transcribe(&models, lang, w);
                errs[k] = edits(&want, &units(&got));
                let s = score.entry(lang.to_string()).or_default();
                s[k].0 += errs[k];
                s[k].1 += want.len();
                heard.push(got);
            }
            if errs[1] > errs[0] || errs[2] > errs[0] {
                worse.push(format!("{id}_{kind}: \"{}\" | 48k \"{}\" | opus \"{}\"", heard[0], heard[1], heard[2]));
            }
        }
    }
    println!("\nword error rate (Chinese: characters)    {:>10} {:>12} {:>10}", WAYS[0], WAYS[1], WAYS[2]);
    let mut all = [(0usize, 0usize); 3];
    for (lang, s) in &score {
        let pct = |k: usize| 100.0 * s[k].0 as f64 / s[k].1.max(1) as f64;
        println!("  {lang:<38} {:>9.1}% {:>11.1}% {:>9.1}%", pct(0), pct(1), pct(2));
        for k in 0..3 {
            all[k].0 += s[k].0;
            all[k].1 += s[k].1;
        }
    }
    let pct = |k: usize| 100.0 * all[k].0 as f64 / all[k].1.max(1) as f64;
    println!("  {:<38} {:>9.1}% {:>11.1}% {:>9.1}%", "all", pct(0), pct(1), pct(2));
    println!("\nclips where a way did worse than as is ({}):", worse.len());
    for w in &worse {
        println!("  {w}");
    }
    // (no assertion: "via 48 kHz" converts twice - the clips are 16 kHz, a microphone isn't - so it costs a little
    //  more than the real path, which converts once and is the same as before: the test below)
}

/// What the listener gets from a 48 kHz microphone is the same as before the voice chat, sample for sample. Before:
/// the microphone opened at 16 kHz, its 48 kHz converted in kd_audio's input stream (Rechunk 48000 -> 16000, 10 ms
/// chunks). Now: opened at 48 kHz (no conversion there), then mic.rs's tap converts (the same Rechunk, the same chunks).
#[test]
fn a_48_khz_microphone_gives_the_listener_the_same_audio_as_before() {
    // (a 48 kHz microphone's audio, made here - export/ is not in the repository, so CI has no recordings: 3 s of
    //  tones up to 18 kHz, swelling and fading like syllables, with a little noise)
    let mut seed = 7u64;
    let mic: Vec<f32> = (0..3 * kd_voice::RATE as usize)
        .map(|i| {
            let t = i as f64 / kd_voice::RATE as f64;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let noise = ((seed >> 40) as f64 / (1u64 << 24) as f64 - 0.5) * 0.02;
            let tones: f64 = [180.0, 720.0, 2400.0, 9000.0, 18000.0].iter().map(|f| (2.0 * std::f64::consts::PI * f * t).sin()).sum();
            (0.06 * tones * (0.5 + 0.5 * (2.0 * std::f64::consts::PI * 4.0 * t).sin()) + noise) as f32
        })
        .collect();
    let blocks = |x: &[f32], n: usize| -> Vec<f32> {
        // (in the microphone's 50 ms blocks, as the device hands them over)
        let mut r = Rechunk::new(kd_voice::RATE, RATE, kd_voice::RATE as usize / 100, 1).unwrap();
        let mut out = Vec::new();
        for b in x.chunks(n) {
            r.push(b, &mut |y| out.extend_from_slice(y));
        }
        out
    };
    let before = blocks(&mic, 2400);
    // now: the input stream at 48 kHz passes blocks through untouched (Rechunk 48000 -> 48000), then the tap converts
    let mut pass = Rechunk::new(kd_voice::RATE, kd_voice::RATE, kd_voice::RATE as usize / 100, 1).unwrap();
    let mut at48 = Vec::new();
    for b in mic.chunks(2400) {
        pass.push(b, &mut |y| at48.extend_from_slice(y));
    }
    assert_eq!(at48, mic, "48 kHz in, 48 kHz out: untouched");
    let now = blocks(&at48, 2400);
    assert!(!before.is_empty() && before == now, "the listener's 16 kHz audio is the same as before");
}
