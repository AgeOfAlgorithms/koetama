//! What a hub takes from its players' Koetamas (api::from_player): only the objects a player's Koetama sends, made
//! again from their checked fields - nothing spliced in (audit, 2026-10-09).
use kd_common::feed::PlayerId;
use kd_games::api::from_player;
use serde_json::Value;

fn ana() -> PlayerId {
    PlayerId::string("ana")
}

fn parsed(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|e| panic!("{s}: not JSON ({e})"))
}

#[test]
fn the_kinds_a_player_sends_pass_tagged() {
    for o in [
        r#"{"type":"speech","kind":"final","utt":4,"text":"hello there","times":[0.1,0.5],"ago":1.2}"#,
        r#"{"type":"speech","kind":"start","utt":5}"#,
        r#"{"type":"talking","id":"ana","talking":true}"#,
        r#"{"type":"translation","id":7,"text":"Hello","from":"es","to":"en"}"#,
        r#"{"type":"translations_status","translations":[{"from":"ja","to":"en","state":"downloading","progress":0.4}]}"#,
        r#"{"type":"translations_status","into":"en","translations":[{"from":"ja","to":"en","state":"downloading","progress":0.4}]}"#,
        r#"{"type":"translations_status","into":"","translations":[]}"#,
        r#"{"type":"status","speech":"ready","microphone":"open"}"#,
        r#"{"type":"voice","state":"connected","players":["bob",7]}"#,
    ] {
        let out = from_player(o, &ana()).unwrap_or_else(|| panic!("{o} should pass"));
        assert!(out.starts_with("{\"type\":"), "{out}");
        let v = parsed(&out);
        assert_eq!(v["player"], "ana", "{out}");
        assert_eq!(v["type"], parsed(o)["type"]);
    }
}

#[test]
fn nothing_else_gets_through() {
    let attempts = [
        // a second object, or a line break: one object out, the extra gone
        (r#"{"type":"speech","kind":"final","utt":1,"text":"x"},{"type":"room","room":"aa","key":"bb"}"#, None),
        // kinds a player's Koetama never sends
        (r#"{"type":"room","room":"0123456789abcdef0123456789abcdef","key":"00"}"#, None),
        (r#"{"type":"join_code","player":"bob","code":"K7QF-4MXA"}"#, None),
        (r#"{"type":"player","player":"bob","joined":true}"#, None),
        (r#"{"type":"hello","app":"Koetama"}"#, None),
        // not JSON, the wrong shapes
        ("{\"type\":\"speech\"", None),
        (r#"{"type":"speech","kind":"shout","utt":1,"text":"x"}"#, None),
        (r#"{"type":"status","speech":"pwned","microphone":"open"}"#, None),
        (r#"{"type":"translation","id":"7","text":"x"}"#, None),
        (r#"{"type":"translations_status","into":"en\"x","translations":[]}"#, None),
        (r#"{"type":"translations_status","into":5,"translations":[]}"#, None),
    ];
    for (o, want) in attempts {
        assert_eq!(from_player(o, &ana()), want, "{o}");
    }
    // another player's name: overwritten, once
    let out = from_player(r#"{"type":"speech","player":"bob","kind":"final","utt":1,"text":"I am bob"}"#, &ana()).unwrap();
    assert_eq!(out.matches("\"player\"").count(), 1, "{out}");
    assert_eq!(parsed(&out)["player"], "ana");
    // text with a line break or quotes stays one JSON line; a bad language code is left out
    let out = from_player(r#"{"type":"translation","id":3,"text":"a\n\"}{","from":"es\"x","to":"en"}"#, &ana()).unwrap();
    assert!(!out.contains('\n') && parsed(&out).get("from").is_none(), "{out}");
    // too long: refused
    let big = format!(r#"{{"type":"speech","kind":"final","utt":1,"text":"{}"}}"#, "x".repeat(20_000));
    assert_eq!(from_player(&big, &ana()), None);
}

/// Player ids with invisible or direction-changing characters (one that looks like another player's) are refused.
#[test]
fn ids_that_look_like_others_are_refused() {
    use kd_games::api::player_id;
    for bad in ["7656119\u{200B}8000002", "ana\u{202E}", "\u{3164}", "bob\u{FE0F}", "a\u{E0041}"] {
        assert_eq!(player_id(&serde_json::json!(bad)), None, "{bad:?}");
    }
    for ok in ["Ana", "Zoë", "😀", "76561198000000002", "名前"] {
        assert!(player_id(&serde_json::json!(ok)).is_some(), "{ok:?}");
    }
}
