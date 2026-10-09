//! The HTTP connector end to end, with plain TCP requests as the game's mod: the hello first, objects until acked
//! (a lost answer loses nothing), "wait" bringing an object at once, sessions, the browser rules (only the profile's
//! origins; CORS and Private Network Access headers for them), bad requests. And the files connector's whole-file feed.
use kd_common::feed::{Feed, FeedSink};
use kd_games::http::HttpGame;
use kd_games::profile::Profile;
use kd_games::{Game, GameKind};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn profile(port: u16, extra: Value) -> Arc<Profile> {
    let mut connector = json!({"type": "http", "port": port});
    if let Value::Object(m) = extra {
        connector.as_object_mut().unwrap().extend(m);
    }
    Arc::new(
        Profile::parse(
            &json!({"format": 1, "id": "web-game", "game": "Web Game", "mod": "Webby", "url": "https://example.com",
                    "author": "me", "uses": ["voices", "speech", "translate"], "connector": connector})
            .to_string(),
        )
        .unwrap(),
    )
}

/// One request: (status, headers lower-cased, body).
fn request(port: u16, method: &str, headers: &[(&str, &str)], body: &str) -> (u16, Vec<(String, String)>, String) {
    let t0 = Instant::now();
    let mut s = loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => break s,
            Err(_) if t0.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => panic!("connect: {e}"),
        }
    };
    let mut req = format!("{method} / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let mut lines = head.split("\r\n");
    let status: u16 = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    (status, headers, body.to_string())
}

fn post(port: u16, feed: Value) -> Value {
    let (status, _, body) = request(port, "POST", &[("Content-Type", "application/json")], &feed.to_string());
    assert_eq!(status, 200, "{body}");
    serde_json::from_str(&body).unwrap()
}

fn header<'a>(h: &'a [(String, String)], k: &str) -> Option<&'a str> {
    h.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

#[test]
fn feeds_and_objects() {
    let port = free_port();
    let sink = Arc::new(Sink::default());
    let mut g = HttpGame::new(profile(port, json!({})), false, sink.clone(), kd_common::null_log());
    assert!(!g.send_text("nobody yet"), "no game: nothing queued");
    g.start();
    let (status, _, info) = request(port, "GET", &[], "");
    let info: Value = serde_json::from_str(&info).unwrap();
    assert!(status == 200 && info["app"] == "Koetama" && info["protocol"] == 2 && info["game"] == "web-game", "{info}");
    // the first feed: the hello
    let r = post(port, json!({"type": "feed", "session": 7, "listen": "always", "lang": "ja", "speakers": [{"id": 2, "gain": 0.5}]}));
    assert_eq!((r["first"].as_i64(), r["last"].as_i64()), (Some(1), Some(1)));
    assert_eq!(r["objects"][0]["type"], "hello");
    assert_eq!(r["objects"][0]["features"], json!(["speech", "voices", "rooms", "translate"]));
    let f = g.feed().unwrap();
    assert!(f.mic && f.lang == "ja" && f.sid == 7 && f.speakers.len() == 1 && g.connected() && g.updates() == 1);
    // acked: gone; nothing new: an empty answer at once
    let t = Instant::now();
    let r = post(port, json!({"session": 7, "ack": 1}));
    assert!(r["objects"].as_array().unwrap().is_empty() && r["last"] == 1 && t.elapsed() < Duration::from_millis(500));
    // wait: an object sent meanwhile comes at once
    let gref = Arc::new(Mutex::new(g));
    let g2 = gref.clone();
    let sender = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        assert!(g2.lock().unwrap().send('f', 3, "hello there", None, None));
    });
    let t = Instant::now();
    let r = post(port, json!({"session": 7, "ack": 1, "wait": 1}));
    let took = t.elapsed();
    sender.join().unwrap();
    assert_eq!(r["objects"], json!([{"type": "speech", "kind": "final", "utt": 3, "text": "hello there"}]));
    assert!(took >= Duration::from_millis(150) && took < Duration::from_millis(900), "came when sent: {took:?}");
    assert_eq!(r["last"], 2);
    // a lost answer: the same objects again until acked
    let r = post(port, json!({"session": 7, "ack": 1}));
    assert_eq!(r["last"], 2);
    // wait with nothing: at most 1 s
    let t = Instant::now();
    let r = post(port, json!({"session": 7, "ack": 2, "wait": 5}));
    assert!(r["objects"].as_array().unwrap().is_empty() && t.elapsed() < Duration::from_millis(1500), "capped at 1 s");
    // a newer feed ends a waiting answer at once (a mod sends a change while its poll waits)
    let waiting = std::thread::spawn(move || {
        let t = Instant::now();
        let r = post(port, json!({"session": 7, "ack": 2, "wait": 1}));
        (t.elapsed(), r)
    });
    std::thread::sleep(Duration::from_millis(150));
    let t = Instant::now();
    post(port, json!({"session": 7, "ack": 2, "talk_key": true, "listen": "push_to_talk"}));
    assert!(t.elapsed() < Duration::from_millis(100), "a quick request is quick: {:?}", t.elapsed());
    let (took, r) = waiting.join().unwrap();
    assert!(took < Duration::from_millis(600) && r["objects"].as_array().unwrap().is_empty(), "ended by the newer feed: {took:?}");
    assert_eq!((r["first"].as_i64(), r["last"].as_i64()), (Some(3), Some(2)), "nothing: first = last + 1");
    // the voice chat's state, translations
    {
        let g = gref.lock().unwrap();
        g.set_standing("voice", kd_games::api::voice("connected", &[]));
        g.set_standing("voice", kd_games::api::voice("connected", &[]));
        assert!(g.send_translation(9, " Hi ", None));
    }
    let r = post(port, json!({"session": 7, "ack": 2}));
    assert_eq!(r["objects"], json!([{"type": "voice", "state": "connected", "players": []}, {"type": "translation", "id": 9, "text": "Hi"}]));
    // a new session: its hello, then the voice state, numbered after what the game has
    let r = post(port, json!({"session": 8, "ack": 0}));
    assert_eq!(r["objects"][0]["type"], "hello");
    assert_eq!(r["objects"][1], json!({"type": "voice", "state": "connected", "players": []}));
    assert_eq!(r["last"], 2);
    // bad requests
    let (s, _, _) = request(port, "POST", &[], "not json");
    assert_eq!(s, 400);
    let (s, _, _) = request(port, "POST", &[], r#"{"listen":"sometimes"}"#);
    assert_eq!(s, 400);
    let (s, _, _) = request(port, "PUT", &[], "{}");
    assert_eq!(s, 405);
    let (s, _, _) = request(port, "POST", &[("Transfer-Encoding", "chunked")], "");
    assert_eq!(s, 411);
    let big = format!("{{\"x\":\"{}\"}}", "a".repeat(70 * 1024));
    let (s, _, _) = request(port, "POST", &[], &big);
    assert_eq!(s, 413);
    gref.lock().unwrap().stop();
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err(), "stopped: the port is closed");
}

#[test]
fn browsers_only_from_the_profiles_origins() {
    let port = free_port();
    let mut g = HttpGame::new(profile(port, json!({"allow_origins": ["https://game.example"]})), false, Arc::new(Sink::default()), kd_common::null_log());
    g.start();
    // (a page from anywhere else: refused, before its feed is read - it can neither read nor send)
    let (s, h, _) = request(port, "POST", &[("Origin", "https://evil.example")], r#"{"listen":"always"}"#);
    assert!(s == 403 && header(&h, "access-control-allow-origin").is_none());
    assert!(g.feed().is_none(), "the refused feed was not taken");
    let (s, h, _) = request(port, "OPTIONS", &[("Origin", "https://evil.example"), ("Access-Control-Request-Method", "POST")], "");
    assert!(s == 403 && header(&h, "access-control-allow-origin").is_none());
    // the game's own page: the preflight and the request carry the CORS headers
    let (s, h, _) = request(port, "OPTIONS", &[("Origin", "https://game.example"), ("Access-Control-Request-Method", "POST")], "");
    assert_eq!(s, 204);
    assert_eq!(header(&h, "access-control-allow-origin"), Some("https://game.example"));
    assert_eq!(header(&h, "access-control-allow-private-network"), Some("true"));
    let (s, h, body) = request(port, "POST", &[("Origin", "https://game.example"), ("Content-Type", "application/json")], r#"{"session":1}"#);
    assert!(s == 200 && header(&h, "access-control-allow-origin") == Some("https://game.example"), "{body}");
    // (no origins listed: no browser at all; a game - no Origin header - is fine)
    let port2 = free_port();
    let mut g2 = HttpGame::new(profile(port2, json!({})), false, Arc::new(Sink::default()), kd_common::null_log());
    g2.start();
    let (s, _, _) = request(port2, "POST", &[("Origin", "http://localhost:8080")], "{}");
    assert_eq!(s, 403);
    assert_eq!(request(port2, "POST", &[], "{}").0, 200);
    g.stop();
    g2.stop();
}

#[test]
fn http_profiles() {
    let p = |c: Value| {
        Profile::parse(
            &json!({"format": 1, "id": "web-game", "game": "W", "mod": "M", "url": "https://x.org", "author": "me", "connector": c}).to_string(),
        )
    };
    assert!(p(json!({"type": "http", "port": 47140})).is_ok());
    assert!(p(json!({"type": "http", "port": 47140, "allow_origins": ["https://a.example", "http://localhost:8080"]})).is_ok());
    for bad in [json!(["*"]), json!(["https://a.example/"]), json!(["a.example"]), json!(["https://a.example/game"]), json!("https://a.example")] {
        assert!(p(json!({"type": "http", "port": 47140, "allow_origins": bad})).is_err(), "{bad}");
    }
    assert!(p(json!({"type": "http"})).is_err() && p(json!({"type": "http", "port": 80})).is_err());
    let kind = GameKind::from_profile(p(json!({"type": "http", "port": 47140, "allow_origins": ["https://a.example"]})).unwrap(), false, None);
    assert!(kind.summary.iter().any(|l| l.contains("answers HTTP on 127.0.0.1:47140")), "{:?}", kind.summary);
    assert!(kind.summary.iter().any(|l| l.contains("https://a.example")));
}

/// The files connector with feed.pattern "": the whole file is the feed (a game that writes its feed object as a
/// JSON file of its own - REFramework's json.dump_file, ...).
#[test]
fn whole_file_feeds() {
    let d = std::env::temp_dir().join(format!("kd-games-wholefile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let profile = Arc::new(
        Profile::parse(
            &json!({"format": 1, "id": "lua-game", "game": "Lua Game", "mod": "L", "url": "https://x.org", "author": "me",
                    "connector": {"type": "files", "feed": {"file": "C:/x/feed.json", "pattern": "", "complete": ""},
                                  "out": {"dirs": ["C:/x"], "prefix": "kt_", "message": "json"}}})
            .to_string(),
        )
        .unwrap(),
    );
    let save = d.join("feed.json");
    std::fs::write(&save, r#"{"seq": 1, "session": 3}"#).unwrap();
    let sink = Arc::new(Sink::default());
    let mut g = kd_games::files::FilesGame::with_paths(profile, false, sink, kd_common::null_log(), Some(save.clone()), Some(vec![d.clone()]));
    g.start();
    let t0 = Instant::now();
    let mut seq = 2;
    while g.feed().is_none() {
        assert!(t0.elapsed() < Duration::from_secs(10), "timed out");
        // (pretty-printed, as a Lua JSON library writes it; a half-written file is skipped: not JSON)
        let f = format!("{{\n  \"seq\": {seq},\n  \"session\": 3,\n  \"listen\": \"always\",\n  \"lang\": \"de\"\n}}\n");
        std::fs::write(&save, &f[..f.len() / 2]).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        std::fs::write(&save, &f).unwrap();
        seq += 1;
        std::thread::sleep(Duration::from_millis(50));
    }
    let f = g.feed().unwrap();
    assert!(f.mic && f.lang == "de" && f.sid == 3);
    let hello: Value = serde_json::from_str(&std::fs::read_to_string(d.join("kt_t1.json")).unwrap()).unwrap();
    assert_eq!(hello["type"], "hello");
    g.stop();
    let _ = std::fs::remove_dir_all(&d);
}

/// (audit, 2026-10-09) A page reached by DNS rebinding names its own host: refused. Senders that trickle a byte at a
/// time cannot hold the request slots: each is cut off, and the game's own requests still come through.
#[test]
fn other_hosts_and_slow_senders() {
    let port = free_port();
    let mut g = HttpGame::new(profile(port, json!({})), false, Arc::new(Sink::default()), kd_common::null_log());
    g.start();
    assert_eq!(request(port, "GET", &[], "").0, 200, "the server is up");
    let raw = |host: &str| {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(format!("GET / HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out.split(' ').nth(1).unwrap_or("").to_string()
    };
    assert_eq!(raw("evil.example"), "421");
    assert_eq!(raw("evil.example:80"), "421");
    for ok in [format!("127.0.0.1:{port}"), format!("localhost:{port}"), "localhost".into(), format!("[::1]:{port}")] {
        assert_eq!(raw(&ok), "200", "{ok}");
    }
    // sixteen slow senders (a byte every 2 s, never done), then the game
    let slow: Vec<_> = (0..16)
        .map(|_| {
            std::thread::spawn(move || {
                let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
                let t0 = Instant::now();
                for b in b"POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Slow: aaaaaaaaaaaaaaaa" {
                    if s.write_all(&[*b]).is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_secs(2));
                    if t0.elapsed() > Duration::from_secs(12) {
                        break;
                    }
                }
                t0.elapsed()
            })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(500));
    let t0 = Instant::now();
    let mut served = false;
    while t0.elapsed() < Duration::from_secs(8) {
        // (busy while the slots are held: a 503, or the connection reset - tried again)
        let status = (|| {
            let mut s = TcpStream::connect(("127.0.0.1", port)).ok()?;
            s.write_all(b"POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\n\r\n{}").ok()?;
            let mut out = String::new();
            s.read_to_string(&mut out).ok()?;
            out.split(' ').nth(1).map(str::to_string)
        })();
        if status.as_deref() == Some("200") {
            served = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    assert!(served && t0.elapsed() < Duration::from_secs(7), "the game got in after {:?}", t0.elapsed());
    for t in slow {
        assert!(t.join().unwrap() < Duration::from_secs(10), "a slow sender was cut off");
    }
    g.stop();
}
