//! The Link: Koetama's files for the game. The scripted run of make_fixtures.py link_cases (app/fixtures/link.json:
//! after each step the exact files in both folders and their contents, a hello's version aside as Python has its own;
//! the result, mic/lang/live), then test_helper.py's link checks (the hello, pings wrapping at 1000, acks, sessions,
//! word times, the voice state, the Workshop folder).
use kd_common::feed::Feed;
use kd_games::teardown::{Link, PREFIX};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("kd-games-link-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// {name: contents} of a folder, read as Python's text mode does ("\r\n" -> "\n")
fn files(d: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for e in std::fs::read_dir(d).unwrap().flatten() {
        let raw = std::fs::read(e.path()).unwrap();
        out.insert(e.file_name().to_string_lossy().into_owned(), String::from_utf8(raw).unwrap().replace("\r\n", "\n"));
    }
    out
}

/// A prefab's object (its tag j, unhexed, parsed); None for another file.
fn object_of(content: &str) -> Option<Value> {
    let h = content.split(" j=").nth(1)?.split('"').next()?;
    serde_json::from_str(&unhex(h)).ok()
}

/// A file as compared with Python's: a hello's version blanked (each program has its own).
fn norm(content: &str) -> String {
    match object_of(content) {
        Some(mut o) if o["type"] == "hello" => {
            o["version"] = Value::from("*");
            o.to_string()
        }
        _ => content.to_string(),
    }
}

fn names(d: &Path) -> Vec<String> {
    files(d).into_keys().collect()
}

fn feed(sid: i64, ack: i64, ping: i64, mic: bool, lang: &str, live: bool) -> Feed {
    Feed { seq: 1, vol: 1.0, sid, ack, ping, mic, lang: lang.into(), live, speakers: BTreeMap::new(), ptt: None, ..Default::default() }
}

#[test]
fn scripted_run_as_python() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/link.json");
    let fx: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let (a, b) = (tmp("fx-local"), tmp("fx-workshop"));
    std::fs::write(a.join("pcvx_t9.xml"), "old").unwrap();
    std::fs::write(a.join("other.txt"), "keep").unwrap();
    let link = Link::new(vec![a.clone(), b.clone()], kd_common::null_log());
    let steps = fx["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 14);
    for (i, st) in steps.iter().enumerate() {
        let args = &st["args"];
        let s = |k: &str| args[k].as_str().unwrap().to_string();
        let r: Option<bool> = match st["op"].as_str().unwrap() {
            "start" => {
                link.start();
                None
            }
            "stop" => {
                link.stop();
                None
            }
            "feed" => {
                let v = args["args"].as_array().unwrap();
                let f = feed(
                    v[0].as_i64().unwrap(),
                    v[1].as_i64().unwrap(),
                    v[2].as_i64().unwrap(),
                    v.get(3).is_none_or(|x| x.as_bool().unwrap()),
                    v.get(4).map_or("en", |x| x.as_str().unwrap()),
                    v.get(5).is_none_or(|x| x.as_bool().unwrap()),
                );
                link.on_feed(&f, &s("tag"));
                None
            }
            "send" => Some(link.send_msg(
                s("kind").chars().next().unwrap(),
                args["utt"].as_u64().unwrap() as u32,
                &s("text"),
                None,
                None,
            )),
            "send_text" => Some(link.send_text(&s("text"))),
            op => panic!("unknown op {op}"),
        };
        let what = format!("step {i}: {} {}", st["op"], args);
        assert_eq!(r, st["result"].as_bool(), "{what}");
        for (name, d) in [("local", &a), ("workshop", &b)] {
            let want: BTreeMap<String, String> = st["files"][name]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), norm(v.as_str().unwrap())))
                .collect();
            let got: BTreeMap<String, String> = files(d).into_iter().map(|(k, v)| (k, norm(&v))).collect();
            assert_eq!(got, want, "{what}: {name}");
        }
        assert_eq!(link.mic(), st["mic"].as_bool().unwrap(), "{what}");
        assert_eq!(link.lang(), st["lang"].as_str().unwrap(), "{what}");
        assert_eq!(link.live(), st["live"].as_bool().unwrap(), "{what}");
    }
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}

fn read(d: &Path, name: &str) -> String {
    String::from_utf8(std::fs::read(d.join(name)).unwrap()).unwrap()
}

fn unhex(h: &str) -> String {
    let b: Vec<u8> = (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect();
    String::from_utf8(b).unwrap()
}

/// object n of the session, as the game reads it
fn obj(d: &Path, n: u32) -> Value {
    let t = read(d, &format!("pcvx_t{n}.xml"));
    assert!(t.starts_with("<prefab") && t.contains("<body tags=\"pcvx j="), "{t}");
    object_of(&t).unwrap()
}

/// test_helper.py's link checks
#[test]
fn helper_link_checks() {
    let (local, shop) = (tmp("local"), tmp("shop"));
    for old in ["pcvx_p77", "pcvx_v5", "pcvx_v6", "pcvx_vc"] {
        std::fs::write(local.join(old), "1").unwrap(); // (left by a crash, an older Koetama)
    }
    std::fs::write(local.join("other.txt"), "1").unwrap();
    let link = Link::new(vec![local.clone(), shop.clone()], kd_common::null_log());
    link.start();
    assert_eq!(names(&local), ["other.txt", "pcvx_on"], "old files of mine (an older Koetama's too) swept");
    assert_eq!(names(&shop), ["pcvx_on"]);
    assert!(!link.send_text("too early") && names(&local) == ["other.txt", "pcvx_on"], "no game yet: nothing is written");
    let fd = |sid, ack, ping, mic, lang: &str| feed(sid, ack, ping, mic, lang, true);
    link.on_feed(&fd(5, 0, 1, false, "en"), "local-proximity-chat");
    assert!(names(&local).contains(&"pcvx_p1".into()) && link.dir().as_deref() == Some(local.as_path()));
    let h = obj(&local, 1);
    assert!(h["type"] == "hello" && h["protocol"] == 2 && h["version"] == kd_common::paths::VERSION, "{h}");
    assert_eq!(h["features"], serde_json::json!(["speech", "voices", "rooms", "translate"]));
    link.on_feed(&fd(5, 0, 2, true, "en"), "local-proximity-chat");
    assert!(names(&local).contains(&"pcvx_p2".into()) && !names(&local).contains(&"pcvx_p1".into()) && link.mic());
    link.on_feed(&fd(5, 0, 1002, false, "en"), "local-proximity-chat");
    assert!(names(&local).contains(&"pcvx_p2".into()), "ping 1002 is answered as p2 (numbers wrap at 1000)");
    assert!(link.send_text("open sesame, \"quoted\" & <ok>") && link.send_text("Привет, 你好"));
    assert_eq!(obj(&local, 2), serde_json::json!({"type": "speech", "kind": "final", "utt": 0, "text": "open sesame, \"quoted\" & <ok>"}));
    assert_eq!(obj(&local, 3)["text"], "Привет, 你好");
    assert!(!names(&local).iter().any(|n| n.ends_with(".tmp")));
    if cfg!(windows) {
        // (as Python's text mode writes it there)
        assert_eq!(read(&local, "pcvx_t3.xml").matches("\r\n").count(), 3);
    }
    link.on_feed(&fd(5, 1, 1002, false, "zh"), "local-proximity-chat");
    assert_eq!(link.lang(), "zh");
    assert!(link.send_msg('l', 7, "the words so", None, None) && link.send_msg('f', 7, "", None, None));
    assert!(!link.send_msg('l', 7, "  ", None, None), "empty live words are not sent");
    assert_eq!(obj(&local, 4), serde_json::json!({"type": "speech", "kind": "live", "utt": 7, "text": "the words so"}));
    assert_eq!(obj(&local, 5), serde_json::json!({"type": "speech", "kind": "final", "utt": 7, "text": ""}));
    let n = names(&local);
    assert!(!n.contains(&"pcvx_t1.xml".into()) && n.contains(&"pcvx_t2.xml".into()) && n.contains(&"pcvx_t5.xml".into()));
    // word times: each unit's start (s), and how long ago the line's audio began
    let two_s_ago = Instant::now().checked_sub(Duration::from_secs(2)).unwrap();
    assert!(link.send_msg('l', 8, "one two three", Some(&[0.1, 0.5, 0.9]), Some(two_s_ago)));
    let o6 = obj(&local, 6);
    let ago = o6["ago"].as_f64().unwrap();
    assert!(o6["times"] == serde_json::json!([0.1, 0.5, 0.9]) && (1.95..=2.6).contains(&ago), "{o6}");
    let long = vec!["word"; 150].join(" ");
    let times: Vec<f64> = (0..150).map(|k| k as f64 * 0.1).collect();
    link.send_msg('f', 8, &long, Some(&times), Some(Instant::now()));
    let o7 = obj(&local, 7);
    let n7 = o7["text"].as_str().unwrap().split_whitespace().count();
    assert!(o7["times"].as_array().unwrap().len() == n7 && n7 < 150, "a line cut at TEXT_MAX keeps one time per unit left ({n7})");
    // (fewer times than units: none sent)
    link.send_msg('f', 8, "a b c", Some(&[0.1, 0.2]), Some(Instant::now()));
    assert!(obj(&local, 8).get("times").is_none());
    assert!(link.send_msg('s', 9, "", None, None));
    assert_eq!(obj(&local, 9), serde_json::json!({"type": "speech", "kind": "start", "utt": 9}));
    link.send_msg('l', 8, "no times here", None, None);
    let o10 = obj(&local, 10);
    assert!(o10.get("times").is_none() && o10.get("ago").is_none());
    let (room, key) = ("ab".repeat(16), "cd".repeat(32));
    assert!(link.send_msg('r', 0, &format!("{room}:{key}"), None, None));
    assert_eq!(obj(&local, 11), serde_json::json!({"type": "room", "room": room, "key": key}));
    link.set_voice("connected");
    assert_eq!(obj(&local, 12), serde_json::json!({"type": "voice", "state": "connected"}));
    link.set_voice("connected");
    assert!(!names(&local).contains(&"pcvx_t13.xml".into()), "the same state again: nothing");
    link.on_feed(&fd(6, 0, 1, false, "en"), "local-proximity-chat");
    let n = names(&local);
    assert!((3..=12).all(|k| !n.contains(&format!("pcvx_t{k}.xml"))) && n.contains(&"pcvx_p1".into()), "a new session: {n:?}");
    assert!(obj(&local, 1)["type"] == "hello" && obj(&local, 2) == serde_json::json!({"type": "voice", "state": "connected"}) && link.n() == 2,
        "the new session: the hello, then the voice state");
    link.send_text("first of the new level");
    assert!(names(&local).contains(&"pcvx_t3.xml".into()));
    let link2 = Link::new(vec![local.clone(), shop.clone()], kd_common::null_log());
    link2.on_feed(&fd(6, 4, 9, false, "en"), "local-proximity-chat");
    assert!(obj(&local, 5)["type"] == "hello", "a helper started mid-session continues after the game's ack (t5: its hello)");
    link.on_feed(&fd(6, 0, 3, false, "en"), "steam-3812301496");
    assert!(link.dir().as_deref() == Some(shop.as_path()) && names(&shop).contains(&"pcvx_p3".into()));
    link.stop();
    assert_eq!(names(&local), ["other.txt"]);
    assert!(names(&shop).is_empty());
    assert_eq!(PREFIX, "pcvx_");
    let _ = std::fs::remove_dir_all(&local);
    let _ = std::fs::remove_dir_all(&shop);
}

/// A link with no folders, a feed for a folder that is gone: nothing panics, nothing is sent.
#[test]
fn link_without_folders() {
    let link = Link::new(vec![], kd_common::null_log());
    link.start();
    link.on_feed(&feed(1, 0, 1, true, "en", true), "local-x");
    assert!(!link.send_text("hello"));
    let gone = std::env::temp_dir().join(format!("kd-games-link-gone-{}", std::process::id()));
    let link = Link::new(vec![gone], kd_common::null_log());
    link.on_feed(&feed(1, 0, 1, true, "en", true), "local-x");
    // (the ping answer could not be written: the feed is not taken - as Python's OSError)
    assert!(!link.mic());
}
