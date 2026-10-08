//! The engine against its references (app/fixtures/mt.json, from make_mt_fixtures.py): Python's SentencePiece for the
//! vocabularies, engine/mt.py for the translations. Needs Mozilla's model folders in bench/mt/models; skipped without.
use std::path::PathBuf;

use kd_translate::engine::{self, Model};
use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn models() -> Option<PathBuf> {
    let p = root().join("bench/mt/models");
    if p.join("es-en").is_dir() {
        Some(p)
    } else {
        eprintln!("skipped: no models in bench/mt/models");
        None
    }
}

fn fixtures() -> Value {
    serde_json::from_str(&std::fs::read_to_string(root().join("app/fixtures/mt.json")).unwrap()).unwrap()
}

fn ids(v: &Value) -> Vec<u32> {
    v.as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u32).collect()
}

#[test]
fn vocabularies_match_sentencepiece() {
    let Some(dir) = models() else { return };
    let fx = fixtures();
    for case in fx["spm"].as_array().unwrap() {
        let path = dir.join(case["vocab"].as_str().unwrap());
        let vocab = engine::spm_for_tests(&std::fs::read(&path).unwrap()).unwrap();
        let texts = case["texts"].as_array().unwrap();
        let mut bad = 0;
        for ((t, want), dec) in
            texts.iter().zip(case["ids"].as_array().unwrap()).zip(case["decoded"].as_array().unwrap())
        {
            let t = t.as_str().unwrap();
            let got = vocab.encode(t);
            if got != ids(want) {
                bad += 1;
                eprintln!("{}: encode {t:?}\n  got  {got:?}\n  want {:?}", path.display(), ids(want));
            }
            let back = vocab.decode(&ids(want));
            if back != dec.as_str().unwrap() {
                bad += 1;
                eprintln!("{}: decode {t:?}\n  got  {back:?}\n  want {:?}", path.display(), dec.as_str().unwrap());
            }
        }
        assert_eq!(bad, 0, "{} mismatches in {}", bad, path.display());
    }
}

#[test]
fn translations_match_the_reference() {
    let Some(dir) = models() else { return };
    let fx = fixtures();
    let (mut same, mut first, mut total) = (0, 0, 0);
    for pair in fx["translate"].as_array().unwrap() {
        let name = pair["pair"].as_str().unwrap();
        let model = Model::load(&dir.join(name)).unwrap();
        for case in pair["cases"].as_array().unwrap() {
            let src = ids(&case["src_ids"]);
            let want = ids(&case["out_ids"]);
            assert_eq!(model.encode_for_tests(case["text"].as_str().unwrap()), src, "{name}: source ids");
            assert_eq!(model.shortlist_for_tests(&src), ids(&case["shortlist"]), "{name}: shortlist");
            let got = model.translate_ids(&src);
            total += 1;
            // (8-bit maths: a value a hair from a rounding boundary - 17.499996 - may round the other way than numpy's
            //  float order gives, and the sentence can go on differently; Mozilla's own build agrees with mt.py on
            //  about half the sentences. So: the same first word, and most sentences the same throughout)
            let agree = got.iter().zip(&want).take_while(|(a, b)| a == b).count();
            first += (agree >= 1.min(want.len())) as usize;
            if got == want {
                same += 1;
            } else {
                eprintln!(
                    "{name}: differs after {agree} of {} tokens\n  got  {}\n  want {}",
                    want.len(),
                    model.translate(case["text"].as_str().unwrap()).unwrap(),
                    case["out"].as_str().unwrap()
                );
            }
        }
    }
    eprintln!("{same}/{total} translations token for token the same as mt.py");
    assert!(same * 2 >= total, "only {same}/{total} the same as mt.py");
    assert!(first * 10 >= total * 9, "only {first}/{total} start with mt.py's first word");
}
