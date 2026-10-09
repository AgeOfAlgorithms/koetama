//! The savegame reader: what is in the file at the start is not live; each change is (once); a half-written file is
//! skipped; stop() ends the thread. And the Teardown module end to end over temp folders (SAVEPROBE_DIR-like).
use kd_common::feed::{Feed, FeedSink};
use kd_games::teardown::{FeedReader, FeedScan, Teardown};
use kd_games::Game;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("kd-games-reader-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// a feed object as the mod writes it: its hex
fn hx(feed: &str) -> String {
    feed.bytes().map(|b| format!("{b:02x}")).collect()
}

/// a savegame.xml as Teardown writes it (engine/test_e2e.py), with this feed (an object: hexed) under the local copy
/// of the mod
fn xml(feed: &str) -> String {
    let feed = hx(feed);
    format!(
        "<registry version=\"2.1.0\">\n<savegame>\n<mod>\n<local-proximity-chat>\n<pcmode value=\"s\"/>\n<pcvx>\n\t\
         <f value=\"{feed}\"/>\n</pcvx>\n</local-proximity-chat>\n</mod>\n</savegame>\n</registry>\n"
    )
}

/// write the whole file at once (as the game replaces it)
fn put(path: &Path, text: &str) {
    let t = path.with_extension("tmp");
    std::fs::write(&t, text).unwrap();
    std::fs::rename(&t, path).unwrap();
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < Duration::from_secs(5), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn scan_once_as_python() {
    let d = tmp("scan");
    let path = d.join("savegame.xml");
    let a = xml(r#"{"seq":5,"session":7,"speakers":[{"id":2000,"test_voice":1,"talking":true}]}"#);
    std::fs::write(&path, &a).unwrap();
    let mut scan = FeedScan::default();
    let mut got: Vec<(Feed, String)> = Vec::new();
    scan.once(&path, &mut |f, t| got.push((f, t.to_string())));
    assert!(got.is_empty() && scan.updates == 0, "a feed already in the file when the helper starts is not played");
    std::fs::write(&path, xml(r#"{"seq":6,"session":7}"#)).unwrap();
    scan.once(&path, &mut |f, t| got.push((f, t.to_string())));
    assert!(got.len() == 1 && got[0].0.seq == 6 && got[0].1 == "local-proximity-chat" && scan.updates == 1);
    std::fs::write(&path, format!("<registry><pcvx><f value=\"{}\"/></pcvx>", hx(r#"{"seq":7}"#))).unwrap();
    scan.once(&path, &mut |f, t| got.push((f, t.to_string())));
    assert_eq!(got.len(), 1, "a half-written file is skipped");
    scan.once(&d.join("none.xml"), &mut |f, t| got.push((f, t.to_string())));
    assert_eq!(got.len(), 1, "no file: nothing");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn reader_thread_fires_once_per_change() {
    let d = tmp("thread");
    let path = d.join("savegame.xml");
    put(&path, &xml(r#"{"seq":1,"session":5,"listen":"always"}"#));
    let seen: Arc<Mutex<Vec<i64>>> = Arc::default();
    let s2 = seen.clone();
    let mut r = FeedReader::start(path.clone(), FeedReader::POLL, move |f: Feed, _tag: &str| s2.lock().unwrap().push(f.seq));
    std::thread::sleep(Duration::from_millis(100));
    assert!(seen.lock().unwrap().is_empty() && r.updates() == 0, "the initial content is not live");
    for seq in 2..=4 {
        put(&path, &xml(&format!(r#"{{"seq":{seq},"session":5,"listen":"always","speakers":[{{"id":1,"test_voice":1,"talking":true}}]}}"#)));
        wait_until("the change is read", || seen.lock().unwrap().len() == (seq - 1) as usize);
        std::thread::sleep(Duration::from_millis(60)); // (several polls: still once)
    }
    assert_eq!(*seen.lock().unwrap(), vec![2, 3, 4]);
    assert_eq!(r.updates(), 3);
    put(&path, &format!("<registry><pcvx><f value=\"{}\"/></pcvx>\n", hx(r#"{"seq":9}"#)));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(seen.lock().unwrap().len(), 3, "a half-written file (no </registry>) is ignored");
    let t0 = Instant::now();
    r.stop();
    assert!(!r.running() && t0.elapsed() < Duration::from_secs(1));
    put(&path, &xml(r#"{"seq":10}"#));
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(seen.lock().unwrap().len(), 3, "stopped: nothing more");
    let _ = std::fs::remove_dir_all(&d);
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

#[test]
fn teardown_module_end_to_end() {
    let d = tmp("module");
    let mods = tmp("module-mods");
    let save = d.join("savegame.xml");
    put(&save, &xml(r#"{"seq":1,"session":5}"#));
    let sink = Arc::new(Sink::default());
    let mut g = Teardown::new(sink.clone(), kd_common::null_log(), Some(save.clone()), Some(vec![mods.clone()]));
    assert_eq!((g.id(), g.name()), ("teardown-proximity-babble-chat", "Teardown"));
    assert!(g.feed().is_none() && !g.wants_mic() && g.language() == "en" && g.live_words() && !g.connected());
    let lines = g.describe();
    assert_eq!(lines[0], format!("reading {}", save.display()));
    assert_eq!(lines[1], format!("my files for the game go to: {}", mods.display()));
    let (found, _) = g.locate();
    assert!(found, "the savegame is there: found");
    g.start();
    assert!(mods.join("pcvx_on").exists());
    // (as the game does: a new feed every few frames - the reader's first look, whenever its thread gets to it, takes
    //  what is there as old, so one write right after start() may be that "old" content on a slow machine)
    let t0 = Instant::now();
    let mut seq = 2;
    while g.feed().is_none() {
        assert!(t0.elapsed() < Duration::from_secs(10), "timed out: the feed arrives");
        put(&save, &xml(&format!(
            r#"{{"seq":{seq},"volume":0.5,"session":5,"ping":7,"listen":"always","lang":"ru","live":false,"speakers":[{{"id":3,"test_voice":2,"talking":true,"azimuth":90,"muffle":0.5}}]}}"#
        )));
        seq += 1;
        std::thread::sleep(Duration::from_millis(50));
    }
    let n = g.updates();
    assert!(g.connected() && g.wants_mic() && g.language() == "ru" && !g.live_words() && n >= 1);
    assert!(mods.join("pcvx_p7").exists(), "the ping answered");
    let rid = kd_common::feed::relay_id("", &kd_common::feed::PlayerId::number(3));
    assert_eq!(sink.0.lock().unwrap()[0].speakers[&rid].az, 90.0);
    assert!(mods.join("pcvx_t1.xml").exists(), "the session's hello");
    assert!(g.send('f', 1, "hello", None, None) && mods.join("pcvx_t2.xml").exists());
    assert!(g.send_text("typed") && mods.join("pcvx_t3.xml").exists());
    assert_eq!((g.speaker_name(1), g.speaker_name(3), g.speaker_name(9)), ("whisperer".into(), "yeller".into(), "9".into()));
    g.stop();
    assert!(std::fs::read_dir(&mods).unwrap().next().is_none(), "stop: my files gone");
    assert!(g.updates() >= n);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&mods);
}
