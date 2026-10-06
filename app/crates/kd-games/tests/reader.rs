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

/// a savegame.xml as Teardown writes it (engine/test_e2e.py), with this feed under the local copy of the mod
fn xml(feed: &str) -> String {
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
    let a = xml("2|5|1.00|7|0|1|0|2000,1,1,1.000,0.0,0.0,0.00");
    std::fs::write(&path, &a).unwrap();
    let mut scan = FeedScan::default();
    let mut got: Vec<(Feed, String)> = Vec::new();
    scan.once(&path, &mut |f, t| got.push((f, t.to_string())));
    assert!(got.is_empty() && scan.updates == 0, "a feed already in the file when the helper starts is not played");
    std::fs::write(&path, a.replace("2|5|", "2|6|")).unwrap();
    scan.once(&path, &mut |f, t| got.push((f, t.to_string())));
    assert!(got.len() == 1 && got[0].0.seq == 6 && got[0].1 == "local-proximity-chat" && scan.updates == 1);
    std::fs::write(&path, "<registry><pcvx><f value=\"2|7|1.00|7|0|1|0|\"/></pcvx>").unwrap();
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
    put(&path, &xml("4|1|1.00|5|0|1|1|en|1|"));
    let seen: Arc<Mutex<Vec<i64>>> = Arc::default();
    let s2 = seen.clone();
    let mut r = FeedReader::start(path.clone(), FeedReader::POLL, move |f: Feed, _tag: &str| s2.lock().unwrap().push(f.seq));
    std::thread::sleep(Duration::from_millis(100));
    assert!(seen.lock().unwrap().is_empty() && r.updates() == 0, "the initial content is not live");
    for seq in 2..=4 {
        put(&path, &xml(&format!("4|{seq}|1.00|5|0|1|1|en|1|1,1,1,1,0,0,0")));
        wait_until("the change is read", || seen.lock().unwrap().len() == (seq - 1) as usize);
        std::thread::sleep(Duration::from_millis(60)); // (several polls: still once)
    }
    assert_eq!(*seen.lock().unwrap(), vec![2, 3, 4]);
    assert_eq!(r.updates(), 3);
    put(&path, "<registry><pcvx><f value=\"4|9|1.00|5|0|1|1|en|1|\"/></pcvx>\n");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(seen.lock().unwrap().len(), 3, "a half-written file (no </registry>) is ignored");
    let t0 = Instant::now();
    r.stop();
    assert!(!r.running() && t0.elapsed() < Duration::from_secs(1));
    put(&path, &xml("4|10|1.00|5|0|1|1|en|1|"));
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
    put(&save, &xml("4|1|1.00|5|0|1|0|en|1|"));
    let sink = Arc::new(Sink::default());
    let mut g = Teardown::new(sink.clone(), kd_common::null_log(), Some(save.clone()), Some(vec![mods.clone()]));
    assert_eq!((g.id(), g.name()), ("teardown", "Teardown"));
    assert!(g.feed().is_none() && !g.wants_mic() && g.language() == "en" && g.live_words() && !g.connected());
    let lines = g.describe();
    assert_eq!(lines[0], format!("reading {}", save.display()));
    assert_eq!(lines[1], format!("my files for the game go to: {}", mods.display()));
    let (found, _) = g.locate();
    assert!(found, "the savegame is there: found");
    g.start();
    assert!(mods.join("pcvx_on").exists());
    put(&save, &xml("4|2|0.50|5|0|7|1|ru|0|3,2,1,1,90,0,0.5"));
    wait_until("the feed arrives", || g.feed().is_some());
    assert!(g.connected() && g.wants_mic() && g.language() == "ru" && !g.live_words() && g.updates() == 1);
    assert!(mods.join("pcvx_p7").exists(), "the ping answered");
    assert_eq!(sink.0.lock().unwrap()[0].speakers[&3].az, 90.0);
    assert!(g.send('f', 1, "hello", None, None) && mods.join("pcvx_t1.xml").exists());
    assert!(g.send_text("typed") && mods.join("pcvx_t2.xml").exists());
    assert_eq!((g.speaker_name(1), g.speaker_name(3), g.speaker_name(9)), ("whisperer".into(), "yeller".into(), "9".into()));
    g.stop();
    assert!(std::fs::read_dir(&mods).unwrap().next().is_none(), "stop: my files gone");
    assert_eq!(g.updates(), 1);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&mods);
}
