//! The translator end to end with Mozilla's real models: the list, the downloads (into a folder of your choosing),
//! the rules' states, then chat lines - mixed-language ones too - and how long each reply takes.
//!     cargo run --release -p kd-translate --example translate_live -- <folder> [from>to ...]
//! (default rules: ja>en and es>ja; the lines below)
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kd_translate::service::{Event, Mozilla, State, Translator};

const LINES: &[&str] = &[
    "こんにちは、誰か聞こえますか？",
    "地下室のドアが閉まっている。鍵を探そう！",
    "ok いまから行く、wait for me",
    "¿Alguien tiene una linterna? No veo nada.",
    "Hola! the door is locked, ¿dónde está la llave?",
    "This line is English only and stays as it is.",
    "gg",
    "Je suis là, derrière toi.",
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = PathBuf::from(args.first().expect("a folder for the models"));
    let mut rules: Vec<(String, String)> =
        args[1..].iter().filter_map(|r| r.split_once('>').map(|(a, b)| (a.to_string(), b.to_string()))).collect();
    if rules.is_empty() {
        rules = vec![("ja".into(), "en".into()), ("es".into(), "ja".into())];
    }
    let (tx, rx) = mpsc::channel();
    let log: kd_common::Log = Arc::new(|s: &str| eprintln!("  log: {s}"));
    let on_event: kd_translate::service::OnEvent = Arc::new(move |e| {
        let _ = tx.send(e);
    });
    let t = Translator::start(Arc::new(Mozilla::new(root, log.clone())), on_event, log);
    let t0 = Instant::now();
    t.set_rules(&rules);
    // the rules get ready (downloads the first time)
    loop {
        match rx.recv_timeout(Duration::from_secs(300)) {
            Ok(Event::Status(s)) => {
                eprintln!("{:6.2} s  status: {}", t0.elapsed().as_secs_f64(), kd_translate::service::status_text(&s));
                if s.iter().all(|r| !matches!(r.state, State::Downloading(_) | State::Loading)) {
                    break;
                }
            }
            Ok(other) => eprintln!("unexpected {other:?}"),
            Err(_) => panic!("no status for 300 s"),
        }
    }
    for (i, line) in LINES.iter().enumerate() {
        let sent = Instant::now();
        t.request(i as i64 + 1, line);
        loop {
            match rx.recv_timeout(Duration::from_secs(30)).expect("a reply") {
                Event::Reply { id, text, .. } if id == i as i64 + 1 => {
                    println!(
                        "{:5.0} ms  {line}\n          -> {}",
                        sent.elapsed().as_secs_f64() * 1000.0,
                        if text.is_empty() { "(nothing to translate)".to_string() } else { text }
                    );
                    break;
                }
                e => eprintln!("  ({e:?})"),
            }
        }
    }
    t.stop();
}
