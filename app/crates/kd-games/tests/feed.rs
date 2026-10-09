//! The feed and the objects against the Python answers (app/fixtures/feed.json, make_fixtures.py feed_cases):
//! parse_feed (the object or its hex; defaults, the voice room, the translations and the lines to translate, what is
//! refused), find_feeds (each feed with its copy of the mod), every object Koetama sends and the prefab that carries
//! one, byte for byte.
use kd_games::api;
use kd_games::files::TRANSLATION_MAX;
use kd_games::teardown::{find_feeds, parse_feed, TEXT_MAX};
use kd_common::feed::{Feed, PlayerId, RuleState};
use serde_json::Value;

fn fixture() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/feed.json");
    let text = std::fs::read_to_string(path).unwrap();
    // (Python's json writes NaN, serde_json reads no such thing: null stands for it here)
    serde_json::from_str(&text.replace("NaN", "null")).unwrap()
}

fn same_f64(want: &Value, got: f64) -> bool {
    match want.as_f64() {
        Some(w) => w == got,
        None => got.is_nan(),
    }
}

/// a float as Python wrote it, to the last bits (NaN as null)
fn close(want: &Value, got: f64) -> bool {
    match want.as_f64() {
        Some(w) => (w - got).abs() <= 1e-9 * w.abs().max(1.0),
        None => got.is_nan(),
    }
}

fn pid(v: &Value) -> Option<PlayerId> {
    v.as_array().map(|a| PlayerId { text: a[0].as_str().unwrap().into(), number: a[1].as_bool().unwrap() })
}

fn range_of(v: &Value) -> Option<(f64, f64)> {
    v.as_array().map(|a| (a[0].as_f64().unwrap(), a[1].as_f64().unwrap()))
}

/// One feed against Python's (a hub's players too).
fn same_effects(e: &kd_common::feed::Effects, w: &Value, text: &str) {
    let pair = |v: &Value| v.as_array().map(|a| (a[0].as_f64().unwrap(), a[1].as_f64().unwrap()));
    assert_eq!(e.band, pair(&w["band"]), "{text}: band");
    assert_eq!(e.echo, pair(&w["echo"]), "{text}: echo");
    let fields = [
        ("drive", e.drive),
        ("compress", e.compress),
        ("hiss", e.hiss),
        ("crackle", e.crackle),
        ("squelch", e.squelch),
        ("horn", e.horn),
        ("lofi", e.lofi),
        ("wobble", e.wobble),
        ("pitch", e.pitch),
        ("robot", e.robot),
        ("reverb", e.reverb),
        ("hum", e.hum),
    ];
    for (k, x) in fields {
        assert!(same_f64(&w[k], x), "{text}: effect {k} {x} vs {}", w[k]);
    }
}

fn same_feed(f: &Feed, want: &Value, text: &str) {
    assert_eq!(f.seq, want["seq"].as_i64().unwrap(), "{text}");
    assert!(same_f64(&want["vol"], f.vol), "{text}");
    assert_eq!(f.sid, want["sid"].as_i64().unwrap(), "{text}");
    assert_eq!(f.ack, want["ack"].as_i64().unwrap(), "{text}");
    assert_eq!(f.ping, want["ping"].as_i64().unwrap(), "{text}");
    assert_eq!(f.mic, want["mic"].as_bool().unwrap(), "{text}");
    assert_eq!(f.ptt, want["ptt"].as_bool(), "{text}");
    assert_eq!(f.lang, want["lang"].as_str().unwrap(), "{text}");
    assert_eq!(f.live, want["live"].as_bool().unwrap(), "{text}");
    assert_eq!(f.room, want["room"].as_str().unwrap(), "{text}");
    assert_eq!(f.key, want["key"].as_str().unwrap(), "{text}");
    assert_eq!(f.me, want["me"].as_i64().unwrap(), "{text}");
    assert_eq!(f.me_id, pid(&want["me_id"]), "{text}");
    assert_eq!(f.name, want["name"].as_str().unwrap(), "{text}");
    assert_eq!(f.range, range_of(&want["range"]), "{text}");
    let clashes: Vec<PlayerId> = want["clashes"].as_array().unwrap().iter().map(|c| pid(c).unwrap()).collect();
    assert_eq!(f.clashes, clashes, "{text}");
    assert_eq!(f.region, want["region"].as_str().unwrap(), "{text}");
    let to: Vec<i64> = want["to"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
    assert_eq!(f.to, to, "{text}");
    let translations: Vec<(String, String)> = want["translations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r[0].as_str().unwrap().to_string(), r[1].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(f.translations, translations, "{text}");
    let requests: Vec<(i64, String)> = want["to_translate"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r[0].as_i64().unwrap(), r[1].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(f.to_translate, requests, "{text}");
    let sp = want["speakers"].as_object().unwrap();
    assert_eq!(f.speakers.len(), sp.len(), "{text}");
    for (k, v) in sp {
        let s = &f.speakers[&k.parse::<i64>().unwrap()];
        assert_eq!(s.src, v["src"].as_i64().unwrap());
        assert_eq!(s.talk, v["talk"].as_bool().unwrap());
        assert!(close(&v["gain"], s.gain), "{text}: gain {}", s.gain);
        assert!(close(&v["az"], s.az), "{text}: az {}", s.az);
        assert!(close(&v["el"], s.el), "{text}: el {}", s.el);
        assert!(same_f64(&v["muffle"], s.muffle));
        assert_eq!(Some(s.id.clone()), pid(&v["id"]), "{text}");
        assert_eq!(s.name, v["name"].as_str().unwrap(), "{text}");
        assert_eq!(s.gain_given, v["gain_given"].as_bool().unwrap(), "{text}");
        assert_eq!(s.range, range_of(&v["range"]), "{text}");
        match (s.distance, v["distance"].as_f64()) {
            (Some(d), Some(w)) => assert!((d - w).abs() <= 1e-9 * w.max(1.0), "{text}: distance {d}"),
            (None, None) => {}
            (d, w) => panic!("{text}: distance {d:?} vs {w:?}"),
        }
        same_effects(&s.effects, &v["effects"], text);
        let via = v["via"].as_array().unwrap();
        assert_eq!(s.via.len(), via.len(), "{text}: via");
        for (d, w) in s.via.iter().zip(via) {
            assert_eq!(d.device.name(), w["device"].as_str().unwrap(), "{text}");
            assert!(same_f64(&w["muffle"], d.muffle) && same_f64(&w["signal"], d.signal), "{text}");
            same_effects(&d.effects, &w["effects"], text);
            let outs = w["outs"].as_array().unwrap();
            assert_eq!(d.outs.len(), outs.len(), "{text}: outs");
            for (o, wo) in d.outs.iter().zip(outs) {
                assert!(close(&wo["az"], o.az) && close(&wo["el"], o.el), "{text}: {o:?}");
                assert!(close(&wo["gain"], o.gain) && close(&wo["delay"], o.delay), "{text}: {o:?}");
            }
        }
    }
    assert_eq!(f.transmit_all, want["transmit_all"].as_bool().unwrap(), "{text}");
    let players = want["players"].as_array().unwrap();
    assert_eq!(f.players.len(), players.len(), "{text}: players");
    for (p, w) in f.players.iter().zip(players) {
        same_feed(p, w, text);
    }
}

#[test]
fn parse_as_python() {
    let fx = fixture();
    let cases = fx["parse"].as_array().unwrap();
    assert_eq!(cases.len(), 69);
    for c in cases {
        let text = c["text"].as_str().unwrap();
        let got = parse_feed(text);
        let want = &c["feed"];
        if want.is_null() {
            assert!(got.is_none(), "{text:?} should be refused: {got:?}");
            continue;
        }
        let f = got.unwrap_or_else(|| panic!("{text:?} should parse"));
        same_feed(&f, want, text);
    }
}

#[test]
fn find_as_python() {
    let fx = fixture();
    for c in fx["find"].as_array().unwrap() {
        let got = find_feeds(c["xml"].as_str().unwrap().as_bytes());
        let want: Vec<(String, String)> = c["feeds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p[0].as_str().unwrap().to_string(), p[1].as_str().unwrap().to_string()))
            .collect();
        assert_eq!(got, want);
    }
}

#[test]
fn objects_byte_identical() {
    let fx = fixture();
    let f64s = |v: &Value| -> Option<Vec<f64>> { v.as_array().map(|a| a.iter().map(|x| x.as_f64().unwrap_or(f64::NAN)).collect()) };
    let cases = fx["objects"].as_array().unwrap();
    assert!(cases.len() >= 12);
    for c in cases {
        let a = &c["args"];
        let got = match c["fn"].as_str().unwrap() {
            "speech" => api::speech(
                a[0].as_str().unwrap().chars().next().unwrap(),
                a[1].as_u64().unwrap() as u32,
                a[2].as_str().unwrap(),
                f64s(&a[3]).as_deref(),
                a[4].as_f64(),
            ),
            "translation" => {
                let rule = a.get(2).and_then(Value::as_array).map(|r| (r[0].as_str().unwrap(), r[1].as_str().unwrap()));
                api::translation(a[0].as_i64().unwrap(), a[1].as_str().unwrap(), rule)
            }
            "translations_status" => api::translations_status(
                &a[0]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| RuleState {
                        from: s[0].as_str().unwrap().into(),
                        to: s[1].as_str().unwrap().into(),
                        state: s[2].as_str().unwrap().into(),
                        progress: s[3].as_f64().unwrap(),
                    })
                    .collect::<Vec<_>>(),
            ),
            "voice" => api::voice(a[0].as_str().unwrap(), &a[1].as_array().unwrap().iter().map(|p| pid(p).unwrap()).collect::<Vec<_>>()),
            "talking" => api::talking(&pid(&a[0]).unwrap(), a[1].as_bool().unwrap()),
            "status" => api::status(a[0].as_str().unwrap(), a[1].as_str().unwrap()),
            "join_code" => api::join_code(&pid(&a[0]).unwrap(), a[1].as_str().unwrap()),
            "player" => api::player(&pid(&a[0]).unwrap(), a[1].as_bool().unwrap()),
            f => panic!("unknown {f}"),
        };
        assert_eq!(got, c["out"].as_str().unwrap(), "{c}");
        assert!(got.starts_with("{\"type\":"), "\"type\" first");
    }
    for c in fx["prefab"].as_array().unwrap() {
        assert_eq!(api::object_prefab(c["object"].as_str().unwrap()), c["out"].as_str().unwrap());
    }
    assert_eq!(fx["TEXT_MAX"].as_u64().unwrap() as usize, TEXT_MAX);
    assert_eq!(fx["TRANSLATION_MAX"].as_u64().unwrap() as usize, TRANSLATION_MAX);
    assert_eq!(fx["PROTOCOL"].as_u64().unwrap(), api::PROTOCOL);
}

/// test_helper.py's feed checks
#[test]
fn helper_feed_checks() {
    let hex = |s: &str| s.bytes().map(|b| format!("{b:02x}")).collect::<String>();
    let f = parse_feed(&hex(r#"{"type":"feed","seq":42,"volume":0.5,"session":7,"ack":3,"ping":12,"listen":"always","speakers":[
        {"id":2000,"test_voice":1,"talking":false,"gain":0.55,"azimuth":-39.8},
        {"id":2001,"test_voice":2,"talking":true,"gain":1.0,"elevation":-3.5,"muffle":0.25}]}"#))
    .unwrap();
    assert!(f.seq == 42 && f.vol == 0.5 && f.sid == 7 && f.ack == 3 && f.ping == 12 && f.mic && f.ptt.is_none());
    let rid = |n: i64| kd_common::feed::relay_id("", &PlayerId::number(n));
    let s = &f.speakers[&rid(2001)];
    assert!(s.src == 2 && s.talk && s.gain == 1.0 && s.az == 0.0 && s.el == -3.5 && s.muffle == 0.25);
    assert!(!f.speakers[&rid(2000)].talk);
    let f = parse_feed(r#"{"type":"feed"}"#).unwrap();
    assert!(f.speakers.is_empty() && !f.mic && f.lang == "en" && f.live && f.vol == 1.0, "plain JSON; the defaults");
    let f = parse_feed(&hex(r#"{"listen":"push_to_talk","talk_key":true,"lang":"zh","live":false,"speakers":[{"id":3,"gain":0.5}]}"#)).unwrap();
    assert!(f.mic && f.ptt == Some(true) && f.lang == "zh" && !f.live && f.speakers[&rid(3)].src == 0);
    for bad in ["garbage", "7b", "[1,2]", r#"{"listen":"sometimes"}"#, r#"{"volume":"loud"}"#] {
        assert!(parse_feed(bad).is_none(), "{bad}");
    }
    let (a, b) = (hex(r#"{"seq":5,"session":7}"#), hex(r#"{"seq":9,"session":3}"#));
    let xml = format!(
        "<registry version=\"2.1.0\">\n<savegame><mod>\n<local-proximity-chat>\n<pcmode value=\"s\"/>\n<pcvx>\n\t<f value=\"{a}\"/>\n</pcvx>\n\
         </local-proximity-chat>\n<steam-123>\n<pcvx>\n<f value=\"{b}\"/>\n</pcvx>\n</steam-123>\n</mod></savegame>\n</registry>\n"
    );
    assert_eq!(
        find_feeds(xml.as_bytes()),
        vec![("local-proximity-chat".to_string(), a.clone()), ("steam-123".to_string(), b.clone())]
    );
    // (a tag's bytes past ASCII read as U+FFFD; a feed's as UTF-8 (lossy); never a panic)
    let odd = b"<steam-\xff1><pcvx><f value=\"{}\xe4\"/></pcvx>";
    let got = find_feeds(odd);
    assert_eq!(got, vec![("steam-\u{FFFD}1".to_string(), "{}\u{FFFD}".to_string())]);
    assert!(parse_feed(&got[0].1).is_none());
}
