//! The translator end to end with Mozilla's real models: the player's setting (a target, the languages they speak,
//! downloads on), then chat lines - mixed-language ones too: each foreign language's pair made the first time it is
//! seen (the list, the downloads into a folder of your choosing, the loading; its line held meanwhile), the pairs'
//! states, and how long each reply takes.
//!     cargo run --release -p kd-translate --example translate_live -- <folder> [into] [spoken,languages] [line ...]
//! (default: into en, speaking en; the lines below)
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kd_translate::service::{Event, Mozilla, Translator};

const LINES: &[&str] = &[
    "¿Alguien tiene una linterna? No veo nada.",
    "地下室のドアが閉まっている。鍵を探そう！",
    "This line is English only and stays as it is.",
    "Hola! the door is locked, ¿dónde está la llave?",
    "ok いまから行く、wait for me",
    "gg",
    "Je suis là, derrière toi.",
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = PathBuf::from(args.first().expect("a folder for the models"));
    let into = args.get(1).cloned().unwrap_or_else(|| "en".into());
    let known: Vec<String> =
        args.get(2).map_or_else(|| vec!["en".into()], |k| k.split(',').map(|s| s.trim().to_string()).collect());
    let lines: Vec<String> =
        if args.len() > 3 { args[3..].to_vec() } else { LINES.iter().map(|s| s.to_string()).collect() };
    let (tx, rx) = mpsc::channel();
    let log: kd_common::Log = Arc::new(|s: &str| eprintln!("  log: {s}"));
    let on_event: kd_translate::service::OnEvent = Arc::new(move |e| {
        let _ = tx.send(e);
    });
    let t = Translator::start(Arc::new(Mozilla::new(root, log.clone())), on_event, log);
    let t0 = Instant::now();
    println!("into {into}, speaking {}", known.join(", "));
    t.set_target(Some(into), known, true);
    for (i, line) in lines.iter().enumerate() {
        let sent = Instant::now();
        t.request(i as i64 + 1, line);
        loop {
            match rx.recv_timeout(Duration::from_secs(300)).expect("a reply") {
                Event::Reply { id, text, rule } if id == i as i64 + 1 => {
                    let used = rule.map(|(f, t)| format!("  [{f} > {t}]")).unwrap_or_default();
                    println!(
                        "{:6.0} ms  {line}\n           -> {}{used}",
                        sent.elapsed().as_secs_f64() * 1000.0,
                        if text.is_empty() { "(left as it is)".to_string() } else { text }
                    );
                    break;
                }
                Event::Status(s) => eprintln!("{:6.2} s  status: {}", t0.elapsed().as_secs_f64(), s.wire()),
                e => eprintln!("  ({e:?})"),
            }
        }
    }
    println!("pairs in use: {}", t.status().wire());
    t.stop();
}
