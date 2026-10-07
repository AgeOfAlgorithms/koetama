//! The socket connector end to end, with a TcpStream as the game's mod: hello both ways, a feed reaches the sink,
//! send() delivers a msg line, malformed lines are skipped (logged once), a second client replaces the first, a line
//! over 64 KB drops the connection, stop() closes everything; a busy port is retried.
use kd_common::feed::{Feed, FeedSink};
use kd_common::Log;
use kd_games::profile::Profile;
use kd_games::socket::SocketGame;
use kd_games::{Game, GameKind};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
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

fn profile(port: u16, uses: Value) -> Profile {
    Profile::parse(
        &json!({"format": 1, "id": "example-game", "game": "Example Game", "mod": "Example Voice", "url": "https://example.com",
                "author": "me", "uses": uses, "speaker_names": {"1": "tester"}, "connector": {"type": "socket", "port": port}})
        .to_string(),
    )
    .unwrap()
}

fn logger() -> (Log, Arc<Mutex<Vec<String>>>) {
    let lines: Arc<Mutex<Vec<String>>> = Arc::default();
    let l2 = lines.clone();
    (Arc::new(move |s: &str| l2.lock().unwrap().push(s.to_string())), lines)
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < Duration::from_secs(8), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A mod: connects (retrying while Kotodama starts listening), reads lines.
struct Client {
    w: TcpStream,
    r: BufReader<TcpStream>,
}

impl Client {
    fn connect(port: u16) -> Client {
        let t0 = Instant::now();
        loop {
            if let Ok(s) = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(500)) {
                s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                return Client { r: BufReader::new(s.try_clone().unwrap()), w: s };
            }
            assert!(t0.elapsed() < Duration::from_secs(8), "could not connect");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn line(&mut self) -> Value {
        let mut s = String::new();
        self.r.read_line(&mut s).unwrap();
        serde_json::from_str(&s).unwrap_or_else(|e| panic!("{s:?}: {e}"))
    }

    fn raw_line(&mut self) -> String {
        let mut s = String::new();
        self.r.read_line(&mut s).unwrap();
        s
    }

    fn send(&mut self, v: &str) {
        self.w.write_all(v.as_bytes()).unwrap();
        self.w.write_all(b"\n").unwrap();
    }

    /// the connection was closed by Kotodama (EOF or reset)
    fn closed(&mut self) -> bool {
        let mut b = [0u8; 256];
        loop {
            match self.r.read(&mut b) {
                Ok(0) | Err(_) => return true,
                Ok(_) => continue, // (lines before the close)
            }
        }
    }
}

#[test]
fn socket_end_to_end() {
    let port = free_port();
    let sink = Arc::new(Sink::default());
    let (log, lines) = logger();
    let mut g = SocketGame::new(Arc::new(profile(port, json!(["voices", "speech"]))), false, sink.clone(), log);
    assert!(!g.send_text("nobody there") && !g.send('f', 1, "x", None, None), "no client: false");
    assert_eq!((g.id(), g.name(), g.needs(), g.port()), ("example-game", "Example Game", "the Example Voice mod", port));
    g.start();
    let mut c = Client::connect(port);
    let hello = c.line();
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["app"], "Kotodama");
    assert_eq!(hello["protocol"], 1);
    assert_eq!(hello["version"], kd_common::paths::VERSION);
    wait_until("the client is taken", || g.has_client());
    c.send(r#"{"type":"hello","protocol":1,"game":"Example Game","mod":"Example Voice"}"#);
    c.send(
        r#"{"type":"feed","vol":0.5,"mic":true,"lang":"ru","live":false,"speakers":[
            {"id":7,"src":1,"talk":true,"gain":0.8,"az":90,"el":-5.5,"muffle":0.25},{"id":8,"src":2}]}"#
            .replace('\n', "")
            .as_str(),
    );
    wait_until("the feed arrives", || g.feed().is_some());
    let f = g.feed().unwrap();
    assert!(f.vol == 0.5 && f.mic && f.lang == "ru" && !f.live && f.seq == 1);
    let s7 = &f.speakers[&7];
    assert!(s7.src == 1 && s7.talk && s7.gain == 0.8 && s7.az == 90.0 && s7.el == -5.5 && s7.muffle == 0.25);
    let s8 = &f.speakers[&8];
    assert!(s8.src == 2 && !s8.talk && s8.gain == 1.0 && s8.az == 0.0 && s8.muffle == 0.0, "defaults");
    assert_eq!(sink.0.lock().unwrap().len(), 1);
    assert!(g.connected() && g.wants_mic() && g.language() == "ru" && !g.live_words() && g.updates() == 1);
    assert_eq!(g.speaker_name(1), "tester");
    assert_eq!(g.describe(), vec![format!("listening on 127.0.0.1:{port} (this computer only)"), "connected: Example Game (Example Voice)".into()]);
    // malformed lines: skipped, logged once; unknown types ignored; blank lines nothing
    c.send("this is not json");
    c.send(r#"{"type":"feed","vol":"loud"}"#);
    c.send(r#"{"no":"type"}"#);
    c.send(r#"{"type":"feed","speakers":[{"src":1}]}"#);
    c.send(r#"{"type":"future-thing","x":1}"#);
    c.send("");
    c.send(r#"{"type":"feed","mic":false}"#);
    wait_until("the good feed after the bad ones", || g.updates() == 2);
    let f = g.feed().unwrap();
    assert!(!f.mic && f.vol == 1.0 && f.lang == "en" && f.live && f.speakers.is_empty() && f.seq == 2, "{f:?}");
    let bad = lines.lock().unwrap().iter().filter(|l| l.contains("malformed")).count();
    assert_eq!(bad, 1, "{:?}", lines.lock().unwrap());
    // Kotodama -> mod
    let a_second_ago = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
    assert!(g.send('l', 3, " hello there ", Some(&[0.1, 0.654]), Some(a_second_ago)));
    let m = c.line();
    assert_eq!((m["type"].as_str(), m["kind"].as_str(), m["utt"].as_u64(), m["text"].as_str()), (Some("msg"), Some("l"), Some(3), Some("hello there")));
    assert_eq!(m["times"], json!([0.1, 0.65]));
    assert!((0.95..1.6).contains(&m["ago"].as_f64().unwrap()), "{m}");
    assert!(g.send_text("typed \"line\""));
    assert_eq!(c.raw_line(), "{\"type\":\"msg\",\"kind\":\"f\",\"utt\":0,\"text\":\"typed \\\"line\\\"\"}\n");
    assert!(g.send('s', 4, "", None, None));
    assert_eq!(c.raw_line(), "{\"type\":\"msg\",\"kind\":\"s\",\"utt\":4,\"text\":\"\"}\n");
    assert!(!g.send('l', 4, "  ", None, None), "empty live words are not sent");
    assert!(g.send('f', 4, "a b c", Some(&[0.1]), Some(Instant::now())));
    assert!(c.line().get("times").is_none(), "fewer times than units: none sent");
    // a second client replaces the first
    let mut c2 = Client::connect(port);
    assert_eq!(c2.line()["type"], "hello");
    assert!(c.closed(), "the first connection is closed");
    assert!(g.send_text("to the new one"));
    assert_eq!(c2.line()["text"], "to the new one");
    assert!(g.peer().is_none(), "the new client has not said hello");
    // a line over 64 KB drops the connection
    let _ = c2.w.write_all(&vec![b'x'; 70 * 1024]); // (Kotodama may close before the last bytes)
    assert!(c2.closed(), "dropped");
    wait_until("the client is gone", || !g.has_client());
    assert!(!g.send_text("nobody"));
    assert!(lines.lock().unwrap().iter().any(|l| l.contains("over 64 KB")));
    // stop: the connection and the port closed
    let mut c3 = Client::connect(port);
    c3.line();
    let t0 = Instant::now();
    g.stop();
    assert!(t0.elapsed() < Duration::from_secs(1), "stop() is quick");
    assert!(c3.closed() && !g.has_client() && !g.listening());
    assert!(TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(500)).is_err(), "the port is closed");
    // started again: listens again
    g.start();
    let mut c4 = Client::connect(port);
    assert_eq!(c4.line()["type"], "hello");
    drop(g);
    assert!(c4.closed(), "dropping the game stops it");
}

#[test]
fn feeds_as_the_profile_uses_them() {
    let port = free_port();
    let sink = Arc::new(Sink::default());
    let kind = GameKind::from_profile(profile(port, json!(["voices"])), false, None);
    let mut g = kind.make(sink.clone(), kd_common::null_log(), None);
    g.start();
    let mut c = Client::connect(port);
    c.line();
    c.send(r#"{"type":"feed","mic":true,"speakers":[{"id":1,"src":1,"talk":true}]}"#);
    wait_until("the feed", || g.feed().is_some());
    assert!(!g.wants_mic() && g.feed().unwrap().speakers.len() == 1, "voices only: the mic is never asked for");
    g.stop();
    let port = free_port();
    let mut g = GameKind::from_profile(profile(port, json!(["speech"])), false, None).make(sink, kd_common::null_log(), None);
    g.start();
    let mut c = Client::connect(port);
    c.line();
    c.send(r#"{"type":"feed","mic":true,"speakers":[{"id":1,"src":1,"talk":true}]}"#);
    wait_until("the feed", || g.feed().is_some());
    assert!(g.wants_mic() && g.feed().unwrap().speakers.is_empty(), "speech only: no voices");
    g.stop();
}

/// By hand (needs python on PATH): examples/socket_client.py against the connector - its hello and feeds arrive, it
/// prints what it is sent. cargo test -p kd-games --test socket -- --ignored --nocapture
#[test]
#[ignore]
fn the_example_client() {
    let port = free_port();
    let (log, lines) = logger();
    let mut g = SocketGame::new(Arc::new(profile(port, json!(["voices", "speech"]))), false, Arc::new(Sink::default()), log);
    g.start();
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../examples/socket_client.py");
    let child = std::process::Command::new("python")
        .args(["-u", script, &port.to_string()])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("python");
    wait_until("feeds from the script", || g.updates() >= 3);
    assert!(g.wants_mic() && g.feed().unwrap().speakers.len() == 1);
    assert_eq!(g.peer().as_deref(), Some("Example Game (Example Voice Link)"));
    assert!(g.send('f', 1, "hello from Kotodama", Some(&[0.0, 0.3, 0.6]), Some(Instant::now())));
    std::thread::sleep(Duration::from_millis(300));
    g.stop();
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    println!("{text}\n{:?}", lines.lock().unwrap());
    assert!(text.contains("from Kotodama: {'type': 'hello'") && text.contains("[f] utterance 1: 'hello from Kotodama'"), "{text}");
    assert!(text.contains("Kotodama closed the connection"));
}

#[test]
fn a_busy_port_is_retried() {
    let blocker = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = blocker.local_addr().unwrap().port();
    let (log, lines) = logger();
    let mut g = SocketGame::new(Arc::new(profile(port, json!(["speech"]))), false, Arc::new(Sink::default()), log);
    g.start();
    wait_until("the busy port is reported", || lines.lock().unwrap().iter().any(|l| l.contains("busy")));
    let msg = lines.lock().unwrap().iter().find(|l| l.contains("busy")).cloned().unwrap();
    assert!(msg.contains(&format!("port {port} on 127.0.0.1 is busy")) && msg.contains("trying again"), "{msg}");
    assert!(!g.listening());
    drop(blocker);
    wait_until("listening once the port is free", || g.listening());
    let mut c = Client::connect(port);
    assert_eq!(c.line()["type"], "hello");
    assert_eq!(lines.lock().unwrap().iter().filter(|l| l.contains("busy")).count(), 1, "said once, not every retry");
    g.stop();
}
