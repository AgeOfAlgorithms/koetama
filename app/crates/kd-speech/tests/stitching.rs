//! The language stitching and LocalAgreement against the Python answers (app/fixtures/segments.json, text.json:
//! make_fixtures.py). segments() runs with the same made-up detector as the Python did (fake_probs).
use kd_speech::*;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

fn fixture(name: &str) -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").join(name);
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

fn f32s(v: &Value) -> Vec<f32> {
    v.as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect()
}

fn f64s(v: &Value) -> Vec<f64> {
    v.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect()
}

/// make_fixtures.py fake_probs: language k is how much of the window's energy sits on every 10th sample from k.
fn fake_probs(x: &[f32]) -> Vec<f64> {
    fake_probs_in(x, &MIXED_LANGS.iter().map(|l| l.to_string()).collect::<Vec<_>>())
}

/// ... over some of MIXED_LANGS (the real detector's softmax over just their logits)
fn fake_probs_in(x: &[f32], langs: &[String]) -> Vec<f64> {
    let mut x: Vec<f64> = x.iter().map(|&v| v as f64).collect();
    if x.len() < 1600 {
        x.resize(1600, 0.0);
    }
    let e: Vec<f64> = (0..10)
        .map(|k| {
            let s: Vec<f64> = x.iter().skip(k).step_by(10).map(|v| v * v).collect();
            s.iter().sum::<f64>() / s.len() as f64 + 1e-12
        })
        .collect();
    let all: Vec<f64> = e.iter().map(|v| v.ln() * 3.0).collect();
    let z: Vec<f64> = langs.iter().map(|l| all[MIXED_LANGS.iter().position(|m| m == l).unwrap()]).collect();
    let top = z.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let p: Vec<f64> = z.iter().map(|v| (v - top).exp()).collect();
    let sum: f64 = p.iter().sum();
    p.iter().map(|v| v / sum).collect()
}

#[test]
fn constants_as_python() {
    let f = fixture("segments.json");
    let c = &f["consts"];
    assert_eq!(c["RATE"].as_u64().unwrap() as u32, RATE);
    for (k, v) in [("LID_WIN", LID_WIN), ("LID_MIN", LID_MIN), ("LID_HOP", LID_HOP), ("LID_QUIET", LID_QUIET),
        ("LID_SWITCH", LID_SWITCH), ("LID_SURE", LID_SURE)]
    {
        assert_eq!(c[k].as_f64().unwrap(), v, "{k}");
    }
    let langs: Vec<&str> = c["MIXED_LANGS"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(langs, MIXED_LANGS);
    assert_eq!(roll_model("ru"), "gigaam");
    assert_eq!(roll_model("yue"), "sensevoice");
    assert_eq!(roll_model("de"), "parakeet");
}

#[test]
fn fake_probs_as_python() {
    let f = fixture("segments.json");
    for probe in f["fake_probs"].as_array().unwrap() {
        let p = fake_probs(&f32s(&probe["x"]));
        let want = f64s(&probe["p"]);
        for (a, b) in p.iter().zip(&want) {
            assert!((a - b).abs() < 1e-4, "fake_probs {p:?} != {want:?}");
        }
    }
}

#[test]
fn segments_as_python() {
    let f = fixture("segments.json");
    let cases = f["cases"].as_array().unwrap();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        // (a player's own languages: the same audio as the case it names, only these candidates)
        let x = match case["x_from"].as_str() {
            Some(from) => f32s(&cases.iter().find(|c| c["name"] == from && c["x"].is_array()).unwrap()["x"]),
            None => f32s(&case["x"]),
        };
        let langs: Vec<String> = match case["langs"].as_array() {
            Some(a) => a.iter().map(|l| l.as_str().unwrap().to_string()).collect(),
            None => MIXED_LANGS.iter().map(|l| l.to_string()).collect(),
        };
        let name = &format!("{name} {langs:?}");
        let mut cache = HashMap::new();
        let segs =
            segments_in(&langs, &x, case["fallback"].as_str().unwrap(), Some(&mut cache), &mut |w: &[f32]| Ok(fake_probs_in(w, &langs)))
                .unwrap();
        let want = case["segs"].as_array().unwrap();
        assert_eq!(segs.len(), want.len(), "{name}: {segs:?} != {want:?}");
        for (s, w) in segs.iter().zip(want) {
            assert_eq!(s.0, w[0].as_str().unwrap(), "{name}: {segs:?} != {want:?}");
            assert!((s.1 - w[1].as_f64().unwrap()).abs() <= 0.011, "{name}: {segs:?} != {want:?}");
            assert!((s.2 - w[2].as_f64().unwrap()).abs() <= 0.011, "{name}: {segs:?} != {want:?}");
        }
        let mut keys: Vec<i64> = cache.keys().cloned().collect();
        keys.sort();
        let want_keys: Vec<i64> = case["cache_keys"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
        assert_eq!(keys, want_keys, "{name}: cache keys");
        // (a second run on the cache: the same stretches, the detector not asked again)
        let mut asked = 0;
        let again = segments_in(&langs, &x, case["fallback"].as_str().unwrap(), Some(&mut cache), &mut |w: &[f32]| {
            asked += 1;
            Ok(fake_probs_in(w, &langs))
        })
        .unwrap();
        assert_eq!(again, segs, "{name}: from the cache");
        if !want_keys.is_empty() {
            assert_eq!(asked, 0, "{name}: windows in the cache are not run again");
        }
    }
}

#[test]
fn quiet_point_as_python() {
    let f = fixture("segments.json");
    let q = &f["quiet_point"];
    let x = f32s(&q["x"]);
    for c in q["cases"].as_array().unwrap() {
        let t = c["t"].as_f64().unwrap();
        let got = quiet_point(&x, t, 0.3);
        let want = c["out"].as_f64().unwrap();
        assert!((got - want).abs() <= 0.011, "quiet_point({t}) = {got}, Python {want}");
    }
}

#[test]
fn detector_errors_come_back() {
    let x = vec![0.1f32; 16000 * 3];
    let r = segments(&x, "en", None, &mut |_: &[f32]| Err("no detector".to_string()));
    assert_eq!(r, Err("no detector".to_string()));
}

#[test]
fn commit_as_python() {
    let f = fixture("text.json");
    for seq in f["commit"].as_array().unwrap() {
        let mut line = RollingLine::for_test();
        for step in seq.as_array().unwrap() {
            let text = step["text"].as_str().unwrap();
            let grew = line.commit(text, &f64s(&step["times"]));
            assert_eq!(grew, step["grew"].as_bool().unwrap(), "{text}: grew");
            let want: Vec<(String, String, f64)> = step["committed"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| (c[0].as_str().unwrap().to_string(), c[1].as_str().unwrap().to_string(), c[2].as_f64().unwrap()))
                .collect();
            assert_eq!(line.committed, want, "{text}: committed");
        }
    }
}
