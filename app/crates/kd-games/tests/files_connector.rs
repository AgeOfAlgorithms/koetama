//! The files connector driven by a profile that is not Teardown's: another prefix, the json message format, its own
//! feed pattern and folders; what a sweep may delete (only its exact name patterns - decoys stay); an unsafe prefix
//! refused; a speech-only game's feed.
use kd_common::feed::{Feed, FeedSink};
use kd_games::files::{Link, LinkRules};
use kd_games::profile::{MessageFormat, Profile};
use kd_games::{Game, GameKind};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("kd-games-files-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn names(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

fn feed(sid: i64, ack: i64, ping: i64) -> Feed {
    Feed { seq: 1, vol: 1.0, sid, ack, ping, mic: true, lang: "en".into(), live: true, speakers: BTreeMap::new(), ptt: None, ..Default::default() }
}

/// an object as the json format writes it (api::speech, one line)
fn json_message(text: &str, kind: char, utt: u32, times: Option<&[f64]>, ago: Option<f64>) -> String {
    kd_games::api::speech(kind, utt, text, times, ago) + "\n"
}

fn rules(prefix: &str) -> LinkRules {
    LinkRules { prefix: prefix.into(), message: MessageFormat::Json, tag_dirs: vec![("ws-".into(), 1)] }
}

/// files a sweep must never touch
const DECOYS: [&str; 16] = [
    "other.txt",
    "talky_notes.txt",
    "talky_t1.xml",
    "talky_tx.json",
    "talky_t.json",
    "talky_on.bak",
    "talky_p",
    "talky_p1x",
    "talky_w1.tmp.keep",
    "xtalky_on",
    "pcvx_on",
    "talky_t\u{0663}.json",
    "talky_v",
    "talky_vcx",
    "talky_v5x",
    "talky_vx.bak",
];

#[test]
fn json_messages_and_a_safe_sweep() {
    let (a, b) = (tmp("a"), tmp("b"));
    for d in DECOYS {
        std::fs::write(a.join(d), "keep").unwrap();
    }
    std::fs::create_dir(a.join("talky_p5")).unwrap(); // (a folder named like a ping answer: not a file of mine)
    std::fs::write(a.join("talky_t9.json"), "old").unwrap();
    std::fs::write(a.join("talky_w9.tmp"), "old").unwrap();
    std::fs::write(a.join("talky_p77"), "old").unwrap();
    let gone = a.join("not-there");
    let link = Link::with_rules(vec![Some(a.clone()), Some(b.clone()), Some(gone.clone()), None], rules("talky_"), kd_common::null_log()).unwrap();
    assert_eq!(link.dirs(), [a.clone(), b.clone(), gone.clone()]);
    std::fs::write(a.join("talky_v6"), "old").unwrap(); // (an older Koetama's)
    std::fs::write(a.join("talky_vc"), "old").unwrap();
    link.start();
    let mut want: Vec<String> = DECOYS.iter().map(|s| s.to_string()).chain(["talky_on".into(), "talky_p5".into()]).collect();
    want.sort();
    assert_eq!(names(&a), want, "old files of mine (an older Koetama's v6, vc too) swept, the decoys kept; on written");
    assert_eq!(names(&b), ["talky_on"]);
    // the voice chat's state before any game: kept for the session's start (no file now)
    link.set_voice("connected");
    assert_eq!(names(&b), ["talky_on"]);
    assert!(!gone.exists(), "a missing folder is never made");
    // the folders by tag: ws-* the second, anything else the first
    assert_eq!(link.dir_for("ws-123"), Some(b.clone()));
    assert_eq!(link.dir_for("local-x"), Some(a.clone()));
    assert_eq!(link.dir_for(""), Some(a.clone()));
    link.on_feed(&feed(1, 0, 3), "");
    assert!(names(&a).contains(&"talky_p3".into()));
    let obj = |d: &Path, n: u32| -> Value {
        serde_json::from_str(&std::fs::read_to_string(d.join(format!("talky_t{n}.json"))).unwrap()).unwrap()
    };
    assert_eq!(obj(&a, 1)["type"], "hello", "a session starts with the hello");
    assert_eq!(obj(&a, 2), json!({"type": "voice", "state": "connected"}), "... then the voice state");
    let two_s_ago = Instant::now().checked_sub(Duration::from_secs(2)).unwrap();
    assert!(link.send_msg('l', 7, "  héllo wörld 你好 ", Some(&[0.1, 0.554, 1.0, 1.2]), Some(two_s_ago)));
    assert!(link.send_text("typed"));
    assert!(link.send_msg('s', 8, "", None, None));
    let t1 = obj(&a, 3);
    assert_eq!((t1["kind"].as_str(), t1["utt"].as_u64(), t1["text"].as_str()), (Some("live"), Some(7), Some("héllo wörld 你好")));
    let w: Vec<f64> = t1["times"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
    assert_eq!(w, [0.1, 0.55, 1.0, 1.2], "unit times in s, to 1/100 s (four units: 你 and 好 each one)");
    let age = t1["ago"].as_f64().unwrap();
    assert!((1.95..2.6).contains(&age), "{age}");
    assert_eq!(
        std::fs::read_to_string(a.join("talky_t4.json")).unwrap(),
        "{\"type\":\"speech\",\"kind\":\"final\",\"utt\":0,\"text\":\"typed\"}\n",
        "no times: no times or ago"
    );
    assert_eq!(std::fs::read_to_string(a.join("talky_t5.json")).unwrap(), "{\"type\":\"speech\",\"kind\":\"start\",\"utt\":8}\n");
    assert!(!names(&a).iter().any(|n| n.ends_with(".tmp") && n.starts_with("talky_w")), "written whole (a .tmp renamed)");
    // acks drop what the game has read
    link.on_feed(&feed(1, 4, 3), "");
    let n = names(&a);
    assert!(!n.contains(&"talky_t1.json".into()) && !n.contains(&"talky_t4.json".into()) && n.contains(&"talky_t5.json".into()));
    // the Workshop copy: its own session (hello, voice), then the line
    link.on_feed(&feed(1, 0, 4), "ws-55");
    assert!(link.send_text("to the second folder") && obj(&b, 1)["type"] == "hello" && obj(&b, 3)["text"] == "to the second folder");
    link.stop();
    assert_eq!(names(&a), {
        let mut w: Vec<String> = DECOYS.iter().map(|s| s.to_string()).chain(["talky_p5".into()]).collect();
        w.sort();
        w
    });
    assert!(names(&b).is_empty());
    assert!(a.join("talky_p5").is_dir());
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}

#[test]
fn owned_names_exactly() {
    let link = Link::with_rules(vec![], rules("talky_"), kd_common::null_log()).unwrap();
    for n in ["talky_on", "talky_p0", "talky_p999", "talky_t12.json", "talky_w3.tmp"] {
        assert!(link.owns(n), "{n}");
    }
    for n in DECOYS.iter().chain(&["talky_", "talky_t1.json.bak", "talky_w.tmp", "talky_p-1", "talky_t1.xml", "talky_on/x"]) {
        assert!(!link.owns(n), "{n}");
    }
    if cfg!(windows) {
        assert!(link.owns("TALKY_ON") && link.owns("Talky_T3.JSON"), "Windows: file names in any case are the same file");
    }
    let teardown = Link::new(vec![], kd_common::null_log());
    assert!(teardown.owns("pcvx_t1.xml") && !teardown.owns("pcvx_t1.json") && !teardown.owns("pcvx_notes.txt"));
}

#[test]
fn unsafe_prefixes_refused() {
    for p in ["", "_", "a_", "pcvx", "../x_", "a/b_", "a.b_", "a b_", "x*_", "ab\u{e9}_", "a\\_"] {
        assert!(Link::with_rules(vec![], rules(p), kd_common::null_log()).is_err(), "{p:?}");
    }
    for p in ["ab_", "pcvx_", "My_Game_2_"] {
        assert!(Link::with_rules(vec![], rules(p), kd_common::null_log()).is_ok(), "{p:?}");
    }
}

#[test]
fn json_message_format() {
    // (the same objects the socket connector sends: api.rs)
    assert_eq!(json_message("a \"q\"\n", 'f', 3, None, None), "{\"type\":\"speech\",\"kind\":\"final\",\"utt\":3,\"text\":\"a \\\"q\\\"\\n\"}\n");
    assert_eq!(
        json_message("x", 'l', 1, Some(&[0.004, 2.5]), Some(-1.0)),
        "{\"type\":\"speech\",\"kind\":\"live\",\"utt\":1,\"text\":\"x\",\"times\":[0,2.5],\"ago\":0}\n"
    );
    assert_eq!(
        json_message("x", 'l', 1, Some(&[f64::NAN]), Some(1.234)),
        "{\"type\":\"speech\",\"kind\":\"live\",\"utt\":1,\"text\":\"x\",\"times\":[0],\"ago\":1.23}\n"
    );
    assert_eq!(
        json_message(&format!("{}:{}", "a".repeat(32), "b".repeat(64)), 'r', 0, None, None),
        format!("{{\"type\":\"room\",\"room\":\"{}\",\"key\":\"{}\"}}\n", "a".repeat(32), "b".repeat(64)),
        "the voice room: its own object"
    );
}

/// A stand-in mixer: keeps the feeds.
#[derive(Default)]
struct Sink(Mutex<Vec<Feed>>);

impl FeedSink for Sink {
    fn set_feed(&self, feed: Feed) {
        self.0.lock().unwrap().push(feed);
    }
    fn fresh(&self) -> bool {
        !self.0.lock().unwrap().is_empty()
    }
}

fn put(path: &Path, text: &str) {
    let t = path.with_extension("part");
    std::fs::write(&t, text).unwrap();
    std::fs::rename(&t, path).unwrap();
}

/// A whole game on the files connector from a profile: its own file and pattern (no tags, no end marker), its own
/// prefix and json messages, a folder that is not there, speech only (the speakers are not played).
#[test]
fn a_profile_game_end_to_end() {
    let (game_dir, out) = (tmp("game"), tmp("out"));
    std::env::set_var("KD_FILES_TEST_GAME", &game_dir);
    std::env::set_var("KD_FILES_TEST_OUT", &out);
    let profile = json!({
        "format": 1, "id": "talky-game", "game": "Talky Game", "mod": "Talky", "url": "https://example.com", "author": "me",
        "uses": ["speech"],
        "connector": {"type": "files",
            "feed": {"file": ["{env:KD_FILES_TEST_UNSET}/state.txt", "{env:KD_FILES_TEST_GAME}/state.txt"],
                     "pattern": "FEED=\\[([^\\]]*)\\]", "tag_pattern": "", "complete": ""},
            "out": {"dirs": [["{env:KD_FILES_TEST_UNSET}", "{env:KD_FILES_TEST_OUT}"], "{env:KD_FILES_TEST_GAME}/missing"],
                    "prefix": "talky_", "message": "json"}}
    });
    let kind = GameKind::from_profile(Profile::parse(&profile.to_string()).unwrap(), false, None);
    assert!(kind.speech && !kind.voices && !kind.builtin && kind.id == "talky-game");
    let save = game_dir.join("state.txt");
    let hex = |s: String| s.bytes().map(|b| format!("{b:02x}")).collect::<String>();
    put(&save, &format!("junk FEED=[{}] junk", hex(r#"{"seq":1,"session":5}"#.into())));
    let sink = Arc::new(Sink::default());
    let mut g = kind.make(sink.clone(), kd_common::null_log(), None);
    assert_eq!((g.id(), g.name(), g.needs()), ("talky-game", "Talky Game", "the Talky mod"));
    assert_eq!(g.describe(), vec![format!("reading {}", save.display()), format!("my files for the game go to: {}", out.display())]);
    assert_eq!(g.locate(), (true, game_dir.display().to_string()), "found: the file it reads is there");
    g.start();
    assert!(out.join("talky_on").exists());
    let t0 = Instant::now();
    let mut seq = 2;
    while g.feed().is_none() {
        assert!(t0.elapsed() < Duration::from_secs(10), "timed out: the feed arrives");
        let f = format!(
            r#"{{"seq":{seq},"volume":0.5,"session":5,"ping":7,"listen":"always","lang":"ru","live":false,"speakers":[{{"id":3,"test_voice":2,"talking":true,"azimuth":90,"muffle":0.5}}]}}"#
        );
        put(&save, &format!("FEED=[{}]", hex(f)));
        seq += 1;
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(g.connected() && g.wants_mic() && g.language() == "ru" && !g.live_words());
    assert!(g.feed().unwrap().speakers.is_empty(), "speech only: no voices played");
    assert!(sink.0.lock().unwrap().iter().all(|f| f.speakers.is_empty()));
    assert!(out.join("talky_p7").exists(), "the ping answered");
    let hello: Value = serde_json::from_str(&std::fs::read_to_string(out.join("talky_t1.json")).unwrap()).unwrap();
    assert_eq!(hello["features"], json!(["speech"]), "the hello names what the profile uses");
    assert!(g.send('f', 1, "hello", None, None));
    assert_eq!(
        std::fs::read_to_string(out.join("talky_t2.json")).unwrap(),
        "{\"type\":\"speech\",\"kind\":\"final\",\"utt\":1,\"text\":\"hello\"}\n"
    );
    std::fs::write(out.join("talky_notes.txt"), "mine, not Koetama's").unwrap();
    g.stop();
    assert_eq!(names(&out), ["talky_notes.txt"], "stop: its files gone, nothing else");
    assert!(!game_dir.join("missing").exists());
    let _ = std::fs::remove_dir_all(&game_dir);
    let _ = std::fs::remove_dir_all(&out);
}

/// A voices-only game never gets the microphone, whatever its feed says.
#[test]
fn voices_only_ignores_mic() {
    let d = tmp("voices-only");
    let profile = json!({
        "format": 1, "id": "quiet-game", "game": "Quiet", "mod": "Q", "url": "https://example.com", "author": "me",
        "uses": ["voices"],
        "connector": {"type": "files", "feed": {"file": "C:/x/y.txt", "complete": ""}, "out": {"dirs": ["C:/x"]}}
    });
    let p = Arc::new(Profile::parse(&profile.to_string()).unwrap());
    let sink = Arc::new(Sink::default());
    let save = d.join("y.txt");
    let hex = |s: String| s.bytes().map(|b| format!("{b:02x}")).collect::<String>();
    let xml = |seq: i32| {
        let f = format!(r#"{{"seq":{seq},"session":5,"listen":"always","speakers":[{{"id":1,"test_voice":1,"talking":true}}]}}"#);
        format!("<pcvx><f value=\"{}\"/></pcvx>", hex(f))
    };
    put(&save, &xml(1));
    let mut g = kd_games::files::FilesGame::with_paths(p, false, sink.clone(), kd_common::null_log(), Some(save.clone()), Some(vec![d.clone()]));
    g.start();
    let t0 = Instant::now();
    let mut seq = 2;
    while g.feed().is_none() {
        assert!(t0.elapsed() < Duration::from_secs(10), "timed out");
        put(&save, &xml(seq));
        seq += 1;
        std::thread::sleep(Duration::from_millis(50));
    }
    let f = g.feed().unwrap();
    assert!(!f.mic && !g.wants_mic() && f.speakers.len() == 1);
    g.stop();
    let _ = std::fs::remove_dir_all(&d);
}
