//! The Link: Kotodama's files for the game. The scripted run of make_fixtures.py link_cases (app/fixtures/link.json:
//! after each step the exact files in both folders and their contents, the result, mic/lang/live), then
//! test_helper.py's link checks (pings wrapping at 1000, acks, sessions, word times, the Workshop folder).
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
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                .collect();
            assert_eq!(files(d), want, "{what}: {name}");
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

fn tag(t: &str, key: &str) -> String {
    t.split(&format!(" {key}=")).nth(1).unwrap().split([' ', '"']).next().unwrap().to_string()
}

fn unhex(h: &str) -> String {
    let b: Vec<u8> = (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect();
    String::from_utf8(b).unwrap()
}

/// test_helper.py's link checks
#[test]
fn helper_link_checks() {
    let (local, shop) = (tmp("local"), tmp("shop"));
    std::fs::write(local.join("pcvx_p77"), "1").unwrap(); // (left by a crash)
    std::fs::write(local.join("other.txt"), "1").unwrap();
    let link = Link::new(vec![local.clone(), shop.clone()], kd_common::null_log());
    link.start();
    assert_eq!(names(&local), ["other.txt", "pcvx_on", "pcvx_v5"]);
    assert_eq!(names(&shop), ["pcvx_on", "pcvx_v5"]);
    assert!(!link.send_text("too early") && names(&local) == ["other.txt", "pcvx_on", "pcvx_v5"], "no game yet: a text is not written");
    let fd = |sid, ack, ping, mic, lang: &str| feed(sid, ack, ping, mic, lang, true);
    link.on_feed(&fd(5, 0, 1, false, "en"), "local-proximity-chat");
    assert!(names(&local).contains(&"pcvx_p1".into()) && link.dir().as_deref() == Some(local.as_path()));
    link.on_feed(&fd(5, 0, 2, true, "en"), "local-proximity-chat");
    assert!(names(&local).contains(&"pcvx_p2".into()) && !names(&local).contains(&"pcvx_p1".into()) && link.mic());
    link.on_feed(&fd(5, 0, 1002, false, "en"), "local-proximity-chat");
    assert!(names(&local).contains(&"pcvx_p2".into()), "ping 1002 is answered as p2 (numbers wrap at 1000)");
    assert!(link.send_text("open sesame, \"quoted\" & <ok>") && link.send_text("Привет, 你好"));
    let t1 = read(&local, "pcvx_t1.xml");
    let t2 = read(&local, "pcvx_t2.xml");
    assert!(t1.starts_with("<prefab") && t1.contains("<body tags=\"pcvx k=f u=0 t="));
    assert_eq!(unhex(&tag(&t1, "t")), "open sesame, \"quoted\" & <ok>");
    assert_eq!(unhex(&tag(&t2, "t")), "Привет, 你好");
    assert!(!names(&local).iter().any(|n| n.ends_with(".tmp")));
    if cfg!(windows) {
        // (as Python's text mode writes it there)
        assert_eq!(t2.matches("\r\n").count(), 3);
    }
    link.on_feed(&fd(5, 1, 1002, false, "zh"), "local-proximity-chat");
    assert_eq!(link.lang(), "zh");
    assert!(link.send_msg('l', 7, "the words so", None, None) && link.send_msg('f', 7, "", None, None));
    assert!(!link.send_msg('l', 7, "  ", None, None), "empty live words are not sent");
    let t3 = read(&local, "pcvx_t3.xml");
    let t4 = read(&local, "pcvx_t4.xml");
    assert!(t3.contains("tags=\"pcvx k=l u=7 t=") && unhex(&tag(&t3, "t")) == "the words so");
    assert!(t4.contains("tags=\"pcvx k=f u=7 t=\""));
    let n = names(&local);
    assert!(!n.contains(&"pcvx_t1.xml".into()) && n.contains(&"pcvx_t2.xml".into()) && n.contains(&"pcvx_t4.xml".into()));
    // word times: w = each unit's start (4 hex digits, 1/100 s), a = how long ago the line's audio began
    let two_s_ago = Instant::now().checked_sub(Duration::from_secs(2)).unwrap();
    assert!(link.send_msg('l', 8, "one two three", Some(&[0.1, 0.5, 0.9]), Some(two_s_ago)));
    let t5 = read(&local, "pcvx_t5.xml");
    let a5: i64 = tag(&t5, "a").parse().unwrap();
    assert!(t5.contains(" w=000a0032005a a=") && (195..=260).contains(&a5), "{t5}");
    let long = vec!["word"; 150].join(" ");
    let times: Vec<f64> = (0..150).map(|k| k as f64 * 0.1).collect();
    link.send_msg('f', 8, &long, Some(&times), Some(Instant::now()));
    let t6 = read(&local, "pcvx_t6.xml");
    let n6 = unhex(&tag(&t6, "t")).split_whitespace().count();
    assert!(tag(&t6, "w").len() == 4 * n6 && n6 < 150, "a line cut at TEXT_MAX keeps one time per unit left ({n6})");
    // (fewer times than units: none sent)
    link.send_msg('f', 8, "a b c", Some(&[0.1, 0.2]), Some(Instant::now()));
    assert!(!read(&local, "pcvx_t7.xml").contains(" w="));
    assert!(link.send_msg('s', 9, "", None, None));
    assert!(read(&local, "pcvx_t8.xml").contains("tags=\"pcvx k=s u=9 t=\""));
    link.send_msg('l', 8, "no times here", None, None);
    let t9 = read(&local, "pcvx_t9.xml");
    assert!(!t9.contains(" w=") && !t9.contains(" a="));
    link.on_feed(&fd(6, 0, 1, false, "en"), "local-proximity-chat");
    let n = names(&local);
    assert!(!n.contains(&"pcvx_t2.xml".into()) && n.contains(&"pcvx_p1".into()) && link.n() == 0, "a new session: {n:?}");
    link.send_text("first of the new level");
    assert!(names(&local).contains(&"pcvx_t1.xml".into()));
    let link2 = Link::new(vec![local.clone(), shop.clone()], kd_common::null_log());
    link2.on_feed(&fd(6, 4, 9, false, "en"), "local-proximity-chat");
    link2.send_text("helper restarted");
    assert!(names(&local).contains(&"pcvx_t5.xml".into()), "a helper started mid-session continues after the game's ack");
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
