//! kd_common::text against the Python reference (app/fixtures/text.json, from engine/asr.py).
use kd_common::text::*;
use serde_json::Value;

fn fixture() -> Value {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/text.json");
    serde_json::from_str(&std::fs::read_to_string(p).expect("run app/fixtures/make_fixtures.py")).unwrap()
}

#[test]
fn wide_ranges_match_python() {
    let f = fixture();
    let py: Vec<(u32, u32)> = f["wide"].as_array().unwrap().iter().map(|r| (r[0].as_u64().unwrap() as u32, r[1].as_u64().unwrap() as u32)).collect();
    assert_eq!(py, WIDE.to_vec());
}

#[test]
fn units_match_python() {
    for c in fixture()["units"].as_array().unwrap() {
        let text = c["text"].as_str().unwrap();
        let want: Vec<(usize, String)> = c["units"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| (u[0].as_u64().unwrap() as usize, u[1].as_str().unwrap().to_string()))
            .collect();
        assert_eq!(units(text), want, "units of {text:?}");
    }
}

#[test]
fn unit_key_matches_python() {
    for c in fixture()["unit_key"].as_array().unwrap() {
        assert_eq!(unit_key(c["u"].as_str().unwrap()), c["key"].as_str().unwrap(), "{c}");
    }
}

#[test]
fn tidy_matches_python() {
    for c in fixture()["tidy"].as_array().unwrap() {
        assert_eq!(tidy(c["text"].as_str().unwrap()), c["out"].as_str().unwrap(), "{c}");
    }
}

#[test]
fn unit_times_match_python() {
    for c in fixture()["unit_times"].as_array().unwrap() {
        let tokens: Vec<String> = c["tokens"].as_array().unwrap().iter().map(|t| t.as_str().unwrap().to_string()).collect();
        let stamps: Vec<f32> = c["stamps"].as_array().unwrap().iter().map(|t| t.as_f64().unwrap() as f32).collect();
        let got = unit_times(c["text"].as_str().unwrap(), &tokens, &stamps, c["offset"].as_f64().unwrap(), c["dur"].as_f64());
        let want: Vec<f64> = c["out"].as_array().unwrap().iter().map(|t| t.as_f64().unwrap()).collect();
        assert_eq!(got.len(), want.len(), "{c}");
        for (g, w) in got.iter().zip(&want) {
            // (Python got the stamps as float64 from float32 too; the rounding to 1/100 s is the same)
            assert!((g - w).abs() < 1e-9, "{c}: got {got:?}");
        }
    }
}

#[test]
fn round2_is_pythons_round() {
    assert_eq!(round2(0.125), 0.12);
    assert_eq!(round2(0.375), 0.38);
    assert_eq!(round2(1.005), 1.0);
    assert_eq!(round2(2.675), 2.67);
    assert_eq!(round2(-0.004), -0.0);
}
