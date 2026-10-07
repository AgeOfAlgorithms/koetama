//! The languages a player speaks -> what the listener runs and which models it needs.
use kd_speech::*;

fn v(l: &[&str]) -> Vec<String> {
    l.iter().map(|s| s.to_string()).collect()
}

#[test]
fn one_language_is_that_language() {
    assert_eq!(plan(&v(&["ru"])), ("ru".to_string(), v(&["ru"])));
    assert_eq!(models_for(&v(&["ru"])), vec!["gigaam"]);
    assert_eq!(models_for(&v(&["de"])), vec!["parakeet"]);
}

#[test]
fn several_are_auto_among_exactly_them() {
    assert_eq!(plan(&v(&["en", "ru"])), ("auto".to_string(), v(&["en", "ru"])));
    assert_eq!(models_for(&v(&["en", "ru"])), vec!["langid", "parakeet", "gigaam"]);
    // (two languages of one model: still the detector - Parakeet decides one language per clip)
    assert_eq!(models_for(&v(&["en", "de"])), vec!["langid", "parakeet"]);
    assert_eq!(models_for(&v(&["ja", "ko", "zh"])), vec!["langid", "sensevoice"]);
}

#[test]
fn none_is_english_and_auto_or_repeats_are_dropped() {
    assert_eq!(plan(&[]), ("en".to_string(), v(&["en"])));
    assert_eq!(plan(&v(&["auto", "", "ko", "ko"])), ("ko".to_string(), v(&["ko"])));
}

#[test]
fn the_language_list_and_its_tiers() {
    let n = |t: Tier| LANGS.iter().filter(|l| l.tier == t).count();
    assert_eq!((n(Tier::Full), n(Tier::Soft), n(Tier::Weak)), (14, 8, 7), "as the game's Voice page");
    for l in MIXED_LANGS {
        assert!(lang_info(l).is_some_and(|i| i.tier == Tier::Full), "{l}: the default \"auto\" ones are fully supported");
    }
    assert_eq!(detector_label("yue"), "zh");
    assert_eq!(roll_model("yue"), "sensevoice");
    let mut codes: Vec<&str> = LANGS.iter().map(|l| l.code).collect();
    codes.sort();
    codes.dedup();
    assert_eq!(codes.len(), LANGS.len(), "each language once");
    for m in MODEL_INFO {
        assert!(m.name == "langid" || MODELS.iter().any(|s| s.name == m.name), "{}", m.name);
    }
}

/// (the real detector: every language on the list is one it knows; Cantonese as Chinese)
#[test]
#[ignore]
fn the_detector_knows_every_language() {
    let m = Models::new(2, kd_common::null_log());
    let all: Vec<String> = LANGS.iter().map(|l| l.code.to_string()).collect();
    let p = m.lid_probs_in(&vec![0.01f32; 16000], &all).expect("the detector");
    assert_eq!(p.len(), all.len());
    assert!((p.iter().sum::<f64>() - 1.0).abs() < 1e-4);
}
