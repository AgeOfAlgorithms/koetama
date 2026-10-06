//! The Python engine's answers (app/fixtures/audio.json, written by make_fixtures.py from engine/audio.py): the
//! constants, the low-pass, panning, a scripted mixer run on a fake clock, the percentile.
mod common;

use common::Clock;
use kd_audio::*;
use kd_common::feed::{Feed, Speaker};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

fn fixture() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/audio.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("app/fixtures/audio.json")).unwrap()
}

fn floats(v: &Value) -> Vec<f64> {
    v.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect()
}

fn f32s(v: &Value) -> Vec<f32> {
    floats(v).into_iter().map(|x| x as f32).collect()
}

#[test]
fn constants() {
    let c = &fixture()["consts"];
    assert_eq!(c["RATE"].as_u64().unwrap(), RATE as u64);
    assert_eq!(c["LP_TAPS"].as_u64().unwrap(), LP_TAPS as u64);
    for (name, v) in [
        ("SMOOTH", SMOOTH),
        ("STALE", STALE),
        ("CUT_CLEAR", CUT_CLEAR),
        ("CUT_MUFFLED", CUT_MUFFLED),
        ("PAN", PAN),
        ("BEHIND_MUFFLE", BEHIND_MUFFLE),
        ("BEHIND_QUIET", BEHIND_QUIET),
        ("HEADROOM", HEADROOM),
    ] {
        assert_eq!(c[name].as_f64().unwrap(), v, "{name}");
    }
}

#[test]
fn lowpass_ir_as_python() {
    for case in fixture()["lowpass_ir"].as_array().unwrap() {
        let a = case["a"].as_f64().unwrap();
        let want = floats(&case["h"]);
        let got = lowpass_ir(a);
        assert_eq!(got.len(), want.len(), "a={a}: length");
        for (i, (g, w)) in got.iter().zip(&want).enumerate() {
            assert!((g - w).abs() < 2e-9, "a={a} h[{i}]: {g} vs {w}");
        }
    }
}

#[test]
fn lowpass_blocks_as_python() {
    let mut hist = vec![0.0f32; LP_TAPS];
    for (k, case) in fixture()["lowpass"].as_array().unwrap().iter().enumerate() {
        let a = case["a"].as_f64().unwrap();
        let x = f32s(&case["x"]);
        let want = floats(&case["y"]);
        let y = lowpass(&x, &mut hist, a);
        assert_eq!(y.len(), want.len());
        assert_eq!(hist.len(), LP_TAPS);
        let err = y.iter().zip(&want).map(|(g, w)| (*g as f64 - w).abs()).fold(0.0, f64::max);
        assert!(err < 1e-5, "block {k} (a={a}): largest difference {err}");
    }
}

#[test]
fn pan_and_behind_as_python() {
    for case in fixture()["pan"].as_array().unwrap() {
        let (az, el) = (case["az"].as_f64().unwrap(), case["el"].as_f64().unwrap());
        let lr = floats(&case["lr"]);
        let (l, r) = pan_gains(az, el);
        assert!((l - lr[0]).abs() < 1e-12 && (r - lr[1]).abs() < 1e-12, "az={az} el={el}: ({l}, {r}) vs {lr:?}");
        assert!((behind(az) - case["behind"].as_f64().unwrap()).abs() < 1e-12, "behind({az})");
    }
}

#[test]
fn percentile_as_numpy() {
    let p = &fixture()["percentile"];
    let x: Vec<f32> = f32s(&p["x"]).into_iter().map(f32::abs).collect();
    let want = p["p999"].as_f64().unwrap();
    let got = percentile(&x, 99.9);
    assert!((got - want).abs() < 1e-6, "{got} vs {want}");
    assert_eq!(percentile(&[3.0, 1.0, 2.0], 50.0), 2.0);
    assert_eq!(percentile(&[1.0, 2.0], 25.0), 1.25);
    assert_eq!(percentile(&[], 50.0), 0.0);
}

fn feed_of(v: &Value) -> Feed {
    let mut speakers = std::collections::BTreeMap::new();
    for (k, s) in v["speakers"].as_object().unwrap() {
        speakers.insert(
            k.parse::<i64>().unwrap(),
            Speaker {
                src: s["src"].as_i64().unwrap(),
                talk: s["talk"].as_bool().unwrap(),
                gain: s["gain"].as_f64().unwrap(),
                az: s["az"].as_f64().unwrap(),
                el: s["el"].as_f64().unwrap(),
                muffle: s["muffle"].as_f64().unwrap(),
            },
        );
    }
    Feed { vol: v["vol"].as_f64().unwrap(), speakers, ..Default::default() }
}

#[test]
fn mixer_script_as_python() {
    let fx = fixture();
    let mx = &fx["mixer"];
    let clips: HashMap<i64, Clip> = mx["clips"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.parse().unwrap(), Arc::new(f32s(v))))
        .collect();
    let clock = Clock::default();
    let mut m = Mixer::with_clock(clips, clock.boxed());
    m.volume = mx["volume"].as_f64().unwrap();
    let (mut renders, mut worst) = (0, 0.0f64);
    for (n, step) in mx["steps"].as_array().unwrap().iter().enumerate() {
        match step["op"].as_str().unwrap() {
            "feed" => m.set_feed(feed_of(&step["feed"])),
            "wait" => clock.add(step["seconds"].as_f64().unwrap()),
            "render" => {
                let frames = step["frames"].as_u64().unwrap() as usize;
                let out = m.render(frames);
                clock.add(frames as f64 / RATE as f64);
                let want = floats(&step["out"]);
                assert_eq!(out.len() * 2, want.len(), "step {n}: frames");
                let err = out.iter().flatten().zip(&want).map(|(g, w)| (*g as f64 - w).abs()).fold(0.0, f64::max);
                assert!(err < 1e-4, "step {n} (render {renders}): largest difference {err}");
                worst = worst.max(err);
                let pos = step["pos"].as_object().unwrap();
                let mut want_ids: Vec<i64> = pos.keys().map(|k| k.parse().unwrap()).collect();
                want_ids.sort();
                assert_eq!(m.voice_ids(), want_ids, "step {n}: the voices kept");
                for (k, p) in pos {
                    let id: i64 = k.parse().unwrap();
                    assert_eq!(m.voice_pos(id), Some(p.as_u64().unwrap() as usize), "step {n}: voice {id}'s place");
                }
                renders += 1;
            }
            op => panic!("unknown op {op}"),
        }
    }
    assert!(renders >= 10);
    println!("{renders} renders, largest difference from Python {worst:.2e}");
}
