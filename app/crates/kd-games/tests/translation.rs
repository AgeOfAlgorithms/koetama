//! Translation through the connectors (PROTOCOL.md "Translation"): the profile's "translate", the feed's "translate" and
//! the lines to translate in a feed (files and socket), and Koetama's answers - the "translation" and
//! "translations_status" objects, in Teardown prefabs, json files and socket lines.
use kd_common::feed::{Feed, FeedSink, RuleState};
use kd_games::api;
use kd_games::files::{FilesGame, Link, LinkRules, TRANSLATION_MAX};
use kd_games::profile::{MessageFormat, Profile};
use kd_games::socket::{parse_socket_feed, SocketGame};
use kd_games::{teardown, Game, GameKind};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("kd-games-translation-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn feed(sid: i64, ack: i64, ping: i64) -> Feed {
    Feed { seq: 1, vol: 1.0, sid, ack, ping, lang: "en".into(), live: true, speakers: BTreeMap::new(), ..Default::default() }
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn states() -> Vec<RuleState> {
    vec![
        RuleState { from: "ja".into(), to: "en".into(), state: "ready".into(), progress: 1.0 },
        RuleState { from: "ko".into(), to: "en".into(), state: "downloading".into(), progress: 0.4271 },
    ]
}

/// A file as the game reads it (prefabs are written in text mode: \r\n on Windows).
fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap().replace("\r\n", "\n")
}

/// A prefab's object, as the game reads it (its tag j, unhexed).
fn object(p: &Path) -> Value {
    let t = read(p);
    let h = t.split(" j=").nth(1).unwrap().split('"').next().unwrap();
    let b: Vec<u8> = (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect();
    serde_json::from_slice(&b).unwrap()
}

#[test]
fn prefab_messages() {
    let d = tmp("prefab");
    let link = Link::new(vec![d.clone()], kd_common::null_log());
    link.start();
    assert!(!link.send_translation(1, "too early", None) && !link.send_translations_state("en", &states()), "no game yet: nothing written");
    link.on_feed(&feed(3, 0, 1), "local-proximity-chat");
    assert_eq!(object(&d.join("pcvx_t1.xml"))["type"], "hello");
    assert!(link.send_translation(7, "  Hello, how are you?  ", None));
    assert_eq!(read(&d.join("pcvx_t2.xml")), api::object_prefab(&api::translation(7, "Hello, how are you?", None)), "stripped");
    assert_eq!(object(&d.join("pcvx_t2.xml")), json!({"type": "translation", "id": 7, "text": "Hello, how are you?"}));
    assert!(link.send_translation(8, "", None), "an empty reply is a reply");
    assert_eq!(object(&d.join("pcvx_t3.xml"))["text"], "");
    assert!(link.send_translations_state("en", &states()));
    assert_eq!(
        object(&d.join("pcvx_t4.xml")),
        json!({"type": "translations_status", "into": "en", "translations": [{"from": "ja", "to": "en", "state": "ready"},
                                                              {"from": "ko", "to": "en", "state": "downloading", "progress": 0.43}]})
    );
    // (an id past a u32: written as it is)
    assert!(link.send_translation(123456789012345, "x", None));
    assert_eq!(object(&d.join("pcvx_t5.xml"))["id"], 123456789012345i64);
    // (a long translation: cut at TRANSLATION_MAX characters)
    assert!(link.send_translation(9, &"é".repeat(TRANSLATION_MAX + 50), None));
    assert_eq!(object(&d.join("pcvx_t6.xml"))["text"], "é".repeat(TRANSLATION_MAX));
    // (acked: gone, like every object)
    link.on_feed(&feed(3, 4, 1), "local-proximity-chat");
    assert!(!d.join("pcvx_t2.xml").exists() && !d.join("pcvx_t4.xml").exists() && d.join("pcvx_t5.xml").exists());
    link.stop();
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn json_messages() {
    let d = tmp("json");
    let rules = LinkRules { prefix: "talky_".into(), message: MessageFormat::Json, tag_dirs: Vec::new() };
    let link = Link::with_rules(vec![Some(d.clone())], rules, kd_common::null_log()).unwrap();
    link.start();
    link.on_feed(&feed(1, 0, 1), "");
    assert!(std::fs::read_to_string(d.join("talky_t1.json")).unwrap().starts_with("{\"type\":\"hello\""));
    assert!(link.send_translation(7, "Hi \"there\"", None));
    assert_eq!(
        std::fs::read_to_string(d.join("talky_t2.json")).unwrap(),
        "{\"type\":\"translation\",\"id\":7,\"text\":\"Hi \\\"there\\\"\"}\n"
    );
    assert!(link.send_translations_state("en", &states()));
    assert_eq!(
        std::fs::read_to_string(d.join("talky_t3.json")).unwrap(),
        "{\"type\":\"translations_status\",\"into\":\"en\",\"translations\":[{\"from\":\"ja\",\"to\":\"en\",\"state\":\"ready\"},\
         {\"from\":\"ko\",\"to\":\"en\",\"state\":\"downloading\",\"progress\":0.43}]}\n"
    );
    assert!(link.send_translation(5_000_000_000, "x", None));
    assert_eq!(
        std::fs::read_to_string(d.join("talky_t4.json")).unwrap(),
        "{\"type\":\"translation\",\"id\":5000000000,\"text\":\"x\"}\n"
    );
    link.stop();
    let _ = std::fs::remove_dir_all(&d);
}

fn files_profile(uses: Value) -> Arc<Profile> {
    Arc::new(
        Profile::parse(
            &json!({
                "format": 1, "id": "chatty-game", "game": "Chatty", "mod": "C", "url": "https://example.com", "author": "me",
                "uses": uses,
                "connector": {"type": "files", "feed": {"file": "C:/x/y.txt", "complete": ""}, "out": {"dirs": ["C:/x"], "prefix": "chat_"}}
            })
            .to_string(),
        )
        .unwrap(),
    )
}

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

/// A files game reading a feed with translations: they reach it only when its profile uses translate.
fn run_files(uses: Value) -> (Feed, Vec<String>) {
    let d = tmp(&format!("files-{}", uses.as_array().unwrap().len()));
    let save = d.join("y.txt");
    let line = |seq: i32| {
        let f = json!({"seq": seq, "session": 5, "listen": "always", "speakers": [{"id": 1, "test_voice": 1, "talking": true}],
                       "translate": true,
                       "to_translate": [{"id": 7, "text": "こんにちは"}]});
        format!("<pcvx><f value=\"{}\"/></pcvx>", hex(&f.to_string()))
    };
    put(&save, &line(1));
    let sink = Arc::new(Sink::default());
    let mut g = FilesGame::with_paths(files_profile(uses), false, sink, kd_common::null_log(), Some(save.clone()), Some(vec![d.clone()]));
    g.start();
    let t0 = Instant::now();
    let mut seq = 2;
    while g.feed().is_none() {
        assert!(t0.elapsed() < Duration::from_secs(10), "timed out");
        put(&save, &line(seq));
        seq += 1;
        std::thread::sleep(Duration::from_millis(50));
    }
    let f = g.feed().unwrap();
    let mut names: Vec<String> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert!(g.send_translation(7, "Hello", None));
    assert!(g.send_translations_state("en", &states()));
    g.stop();
    let _ = std::fs::remove_dir_all(&d);
    (f, names)
}

#[test]
fn files_feed_as_the_profile_uses_it() {
    let (f, names) = run_files(json!(["speech", "translate"]));
    assert!(f.translate);
    assert_eq!(f.to_translate, [(7, "こんにちは".to_string())]);
    assert!(f.speakers.is_empty(), "no voices");
    assert!(names.contains(&"chat_t1.xml".to_string()) && !names.iter().any(|n| n.starts_with("chat_v")), "{names:?}");
    let (f, _) = run_files(json!(["voices", "speech"]));
    assert!(!f.translate && f.to_translate.is_empty(), "translate not listed: none");
    assert_eq!(f.speakers.len(), 1);
}

#[test]
fn profiles() {
    let p = files_profile(json!(["translate"]));
    assert!(p.translate && !p.voices && !p.speech, "translate only: no microphone, no voices");
    let kind = GameKind::from_profile((*p).clone(), false, None);
    assert!(kind.translate);
    assert!(kind.summary.iter().any(|l| l.contains("translates")), "{:?}", kind.summary);
    assert!(!files_profile(json!(["voices", "speech"])).translate, "off unless listed");
    let mut v: Value = serde_json::from_str(teardown::PROFILE_JSON).unwrap();
    assert!(!{
        v.as_object_mut().unwrap().remove("uses");
        Profile::parse(&v.to_string()).unwrap().translate
    }, "no \"uses\": voices and speech, not translate");
    let td = teardown::profile();
    assert!(td.voices && td.speech && td.translate, "the built-in Teardown profile uses all three");
    let e = Profile::parse(&json!({"format": 1, "id": "x-game", "game": "X", "mod": "X", "url": "https://x.org", "author": "me",
        "uses": ["translation"], "connector": {"type": "socket", "port": 5000}}).to_string())
    .unwrap_err();
    assert!(e.contains("\"translate\""), "{e}");
}

#[test]
fn socket_feed_fields() {
    let p = |s: &str| parse_socket_feed(&serde_json::from_str(s).unwrap(), 1);
    // (what is translated into what is the player's setting in Koetama: an old "translations" is ignored)
    let f = p(r#"{"translations":[{"from":"ja","to":"en"}],"to_translate":[{"id":7,"text":"こんにちは"},{"id":8,"text":"안녕"}]}"#).unwrap();
    assert!(f.translate, "translation allowed unless the game says \"translate\": false");
    assert_eq!(f.to_translate, [(7, "こんにちは".to_string()), (8, "안녕".to_string())]);
    assert!(!p(r#"{"translate":false,"to_translate":[{"id":7,"text":"hola"}]}"#).unwrap().translate);
    let f = p(r#"{"to_translate":[{"id":0,"text":"a"},{"id":-3,"text":"b"},{"id":1.5,"text":"c"},{"id":2,"text":"d"},{"id":2,"text":"e"}]}"#).unwrap();
    assert_eq!(f.to_translate, [(2, "d".to_string())], "good ids only, each once");
    let long = "x".repeat(401);
    let f = p(&format!(r#"{{"to_translate":[{{"id":3,"text":"{long}"}}]}}"#)).unwrap();
    assert_eq!(f.to_translate, [(3, String::new())], "too long: kept with \"\" (its reply: \"\")");
    let many: Vec<String> = (1..=20).map(|i| format!(r#"{{"id":{i},"text":"t"}}"#)).collect();
    assert_eq!(p(&format!(r#"{{"to_translate":[{}]}}"#, many.join(","))).unwrap().to_translate.len(), 16);
    assert!(p(r#"{"listen":"always"}"#).unwrap().translate);
    for bad in [
        r#"{"translate":"no"}"#,
        r#"{"translate":1}"#,
        r#"{"to_translate":{"id":1}}"#,
        r#"{"to_translate":[{"id":1}]}"#,
        r#"{"to_translate":[{"id":1,"text":5}]}"#,
    ] {
        assert!(p(bad).is_err(), "{bad}");
    }
}

#[test]
fn socket_messages() {
    assert_eq!(api::translation(7, "Hello", None), r#"{"type":"translation","id":7,"text":"Hello"}"#, "\"type\" first");
    assert_eq!(
        serde_json::from_str::<Value>(&api::translations_status("en", &states())).unwrap(),
        json!({"type": "translations_status", "into": "en", "translations": [{"from": "ja", "to": "en", "state": "ready"},
                                                     {"from": "ko", "to": "en", "state": "downloading", "progress": 0.43}]})
    );
    assert_eq!(
        serde_json::from_str::<Value>(&api::translations_status("", &[])).unwrap(),
        json!({"type": "translations_status", "into": "", "translations": []})
    );
    // (through a connection)
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let profile = Profile::parse(
        &json!({"format": 1, "id": "example-game", "game": "Example Game", "mod": "Example", "url": "https://example.com",
                "author": "me", "uses": ["translate"], "connector": {"type": "socket", "port": port}})
        .to_string(),
    )
    .unwrap();
    let mut g = SocketGame::new(Arc::new(profile), false, Arc::new(Sink::default()), kd_common::null_log());
    assert!(!g.send_translation(1, "x", None), "no client: false");
    g.start();
    let t0 = Instant::now();
    let s = loop {
        if let Ok(s) = TcpStream::connect(("127.0.0.1", port)) {
            break s;
        }
        assert!(t0.elapsed() < Duration::from_secs(8));
        std::thread::sleep(Duration::from_millis(20));
    };
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let hello: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["features"], json!(["translate"]), "what the profile uses");
    let mut w = s;
    w.write_all(format!("{}\n", json!({"type": "feed", "translations": [{"from": "ja", "to": "en"}], "to_translate": [{"id": 4, "text": "はい"}]})).as_bytes()).unwrap();
    let t0 = Instant::now();
    while g.feed().is_none() {
        assert!(t0.elapsed() < Duration::from_secs(8));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(g.feed().unwrap().to_translate, [(4, "はい".to_string())]);
    assert!(g.send_translation(4, " Yes ", None));
    assert!(g.send_translations_state("en", &states()[..1]));
    let mut got = Vec::new();
    for _ in 0..2 {
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        got.push(serde_json::from_str::<Value>(&l).unwrap());
    }
    assert_eq!(got[0], json!({"type": "translation", "id": 4, "text": "Yes"}));
    assert_eq!(got[1], json!({"type": "translations_status", "into": "en", "translations": [{"from": "ja", "to": "en", "state": "ready"}]}));
    g.stop();
}
