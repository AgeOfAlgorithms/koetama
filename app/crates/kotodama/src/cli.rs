//! Kotodama on the command line, for Teardown (engine/teardown_helper.py: the same flags): the same runtime, a status
//! line, and the test modes that need no microphone.
//!
//!     kotodama --cli               start it, then play (a level with Proximity Babble Chat)
//!     kotodama --cli --demo        no game needed: one voice walks a circle around you
//!     kotodama --cli --list        sound devices;  --device NAME / --mic-device NAME pick one
//!     kotodama --cli --transcribe some.wav --lang ru   a recording through the pipeline
//!     kotodama --cli --auto-speech recorded lines as if spoken (needs the benchmark's export/)
//!
//! Ctrl+C stops it.
use crate::runtime::{MicSource, Options, Runtime, Status};
use clap::Parser;
use kd_common::{paths, Log};
use kd_speech::{Callbacks, Listener, Mic, Models, PlaylistMicrophone, WavMicrophone};
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(name = "kotodama --cli", about = "Kotodama on the command line, for Teardown", version = paths::VERSION)]
struct Args {
    /// list the sound devices
    #[arg(long)]
    list: bool,
    /// output device (a name from --list, or its number there; default: the system default)
    #[arg(long)]
    device: Option<String>,
    /// microphone device (a name from --list, or its number there; default: the system default)
    #[arg(long = "mic-device")]
    mic_device: Option<String>,
    /// play this recording as the microphone (real time; tests without one)
    #[arg(long = "mic-wav", value_name = "WAV")]
    mic_wav: Option<String>,
    /// no microphone: recorded lines (English, one-word callouts, Russian, Chinese, Spanish, German, mixed) played
    /// through the REAL speech-to-text as if spoken, each in its own language (needs export/asrbench from the benchmark)
    #[arg(long = "auto-speech")]
    auto_speech: bool,
    /// Kotodama's own volume, 0..1
    #[arg(long, default_value_t = 1.0)]
    volume: f64,
    /// never open the microphone
    #[arg(long = "no-mic")]
    no_mic: bool,
    /// the languages spoken: one (en, ru, zh, es, de, ...) or several, comma-separated (en,ru: "auto" among them;
    /// auto: among the 10 default ones) - default: the game's setting "Language I speak"
    #[arg(long)]
    lang: Option<String>,
    /// CPU threads for the speech models
    #[arg(long, default_value_t = 4)]
    threads: usize,
    /// where the mod looks for Kotodama's files (default: the mods folder / the Workshop folder)
    #[arg(long = "io-dir")]
    io_dir: Option<String>,
    /// run a recording through the speech pipeline as if it came from the microphone: print the live words and the
    /// lines, and stop
    #[arg(long, value_name = "WAV")]
    transcribe: Option<String>,
    /// no game: one voice walks a circle around you
    #[arg(long)]
    demo: bool,
    /// stop after this long (0 = until Ctrl+C)
    #[arg(long, default_value_t = 0.0)]
    seconds: f64,
    /// no microphone: each line piped in goes to the game as if said
    #[arg(long = "type")]
    type_: bool,
    /// no microphone: once the game is running, the test lines are sent one by one, 8 s apart, as if you had said
    /// them (their words live first)
    #[arg(long)]
    auto: bool,
}

/// A device given as a name or as its number in --list.
fn device(arg: &Option<String>, names: Vec<String>) -> Option<String> {
    let a = arg.as_ref()?;
    match a.parse::<usize>() {
        Ok(i) if i < names.len() => Some(names[i].clone()),
        _ => Some(a.clone()),
    }
}

/// --lang: "en", "en,ru", "auto" (the 10 default languages); none: [] (the game's setting).
fn langs_arg(arg: &Option<String>) -> Vec<String> {
    let Some(a) = arg else { return Vec::new() };
    if a.trim() == "auto" {
        return kd_speech::MIXED_LANGS
            .iter()
            .map(|l| l.to_string())
            .collect();
    }
    a.split(',')
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// One line: the game, the microphone, each voice, what is being heard / was said.
fn status_line(st: &Status, needs: &str) -> String {
    if st.state == "waiting" {
        return format!("waiting for the game (a level with {needs})");
    }
    if st.state == "paused" {
        return "the game stopped sending (paused, or the level ended)".into();
    }
    let mut parts = vec![if st.mic == "listening" || st.mic == "talking" {
        format!("mic {} {:4.0} dB", st.mic, st.level)
    } else {
        format!("mic {}", st.mic)
    }];
    for sp in &st.speakers {
        parts.push(format!(
            "{}{} vol {:3}% dir {:4.0} muffle {:3}%",
            sp.name,
            if sp.talk { "*" } else { " " },
            (sp.gain * 100.0).round(),
            sp.az,
            (sp.muffle * 100.0).round()
        ));
    }
    if !st.live.is_empty() {
        let n = st.live.chars().count();
        parts.push(format!(
            "hearing: {}",
            st.live
                .chars()
                .skip(n.saturating_sub(40))
                .collect::<String>()
        ));
    } else if !st.last.is_empty() {
        parts.push(format!(
            "said: {}",
            st.last.chars().take(50).collect::<String>()
        ));
    }
    parts.join(" | ")
}

const AUTO_GAP: f64 = 8.0; // s between two --auto lines
const AUTO_LINES: [&str; 7] = [
    "open sesame, it's me",
    "To test the chat modes, change the chat's mode now: Enter, click Whisper or Yell on the input line, then Enter on the empty line.",
    "This line is said in whatever mode the chat is in right now: whispered, spoken or yelled.",
    "Привет, как дела? Это русский текст.",
    "你好，有人能听到我吗？",
    "This is a long line to see how the chat cuts it into pieces between words: the secret door is behind the painting in the great hall, the key is under the third stone of the fireplace, and the monster only comes out when the lights are off, so keep your flashlight charged and stay together.",
    "Last test line. Now stop the helper with Ctrl+C: about five seconds later the chat should say it disconnected.",
];

/// --auto-speech: [(lang, audio, what is said)] from the benchmark's recordings (computer voices, the webcam-in-a-room
/// versions): English lines, one-word callouts, Russian, Chinese, Spanish, German, and mixed-language lines.
fn auto_speech_items() -> Result<Vec<(String, Vec<f32>, String)>, String> {
    let root = paths::repo_root()
        .ok_or("--auto-speech needs the benchmark recordings (a developer's copy of the repo)")?;
    let lid = root
        .join("export")
        .join("asrbench")
        .join("lid")
        .join("items.json");
    let text = std::fs::read_to_string(&lid).map_err(|_| "--auto-speech needs the benchmark recordings: run bench/make_clips.py and lid.py prep first".to_string())?;
    let items: Vec<serde_json::Value> = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let pick = [
        "en01_room",
        "en04_room",
        "en08_room",
        "en13_room",
        "w000_room",
        "w005_room",
        "w010_room",
        "w015_room",
        "ru02_room",
        "ru05_room",
        "ru10_room",
        "zh03_room",
        "zh07_room",
        "es02_room",
        "de03_room",
        "m08",
        "m10",
        "m12",
        "m14",
        "m15",
        "m16",
        "m19",
    ];
    let mut out = Vec::new();
    for k in pick {
        let Some(it) = items.iter().find(|i| i["id"] == k) else {
            continue;
        };
        let path = it["path"].as_str().unwrap_or_default();
        let (x, sr) =
            kd_audio::read_wav(std::path::Path::new(path)).map_err(|e| format!("{path}: {e}"))?;
        let x = kd_audio::resample(&x, sr, kd_speech::RATE);
        let mixed = it["segs"].as_array().filter(|s| !s.is_empty());
        let said = match mixed {
            Some(s) => format!(
                "{}  (MIXED: {})",
                it["text"].as_str().unwrap_or_default(),
                s.iter()
                    .map(|v| v["lang"].as_str().unwrap_or("?"))
                    .collect::<Vec<_>>()
                    .join("+")
            ),
            None => it["text"].as_str().unwrap_or_default().to_string(),
        };
        let lang = if mixed.is_some() {
            "auto".to_string()
        } else {
            it["lang"].as_str().unwrap_or("en").to_string()
        };
        out.push((lang, x, said));
    }
    Ok(out)
}

fn transcribe(args: &Args, path: &str) -> i32 {
    let (audio, sr) = match kd_audio::read_wav(std::path::Path::new(path)) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{path}: {e}");
            return 1;
        }
    };
    let mut audio = kd_audio::resample(&audio, sr, kd_speech::RATE);
    let log = kd_common::stdout_log();
    let cb = Callbacks {
        on_start: Box::new(|_| {}),
        on_live: Box::new(|u, t, _| println!("  live {u}: {t}")),
        on_final: Box::new(|u, t, i| {
            println!(
                "LINE {u}: {}   [{}; {:.1} s of speech; final pass {:.2} s]",
                if t.is_empty() { "(nothing)" } else { t },
                i.used,
                i.speech,
                i.second_s
            )
        }),
    };
    let l = match Listener::new(cb, Models::new(args.threads, log), true) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    l.set_languages(&langs_arg(&args.lang));
    let t0 = Instant::now();
    if let Err(e) = l.warm(None) {
        eprintln!("{e}");
        return 1;
    }
    println!("models ready in {:.1} s", t0.elapsed().as_secs_f64());
    let t0 = Instant::now();
    audio.extend(std::iter::repeat_n(0.0, kd_speech::RATE as usize));
    for blk in audio.chunks(800) {
        l.feed(blk);
    }
    l.flush();
    println!(
        "{:.1} s of audio in {:.1} s",
        audio.len() as f64 / kd_speech::RATE as f64,
        t0.elapsed().as_secs_f64()
    );
    0
}

fn demo(args: &Args) -> i32 {
    use kd_common::feed::{Feed, Speaker};
    let mut clips = std::collections::HashMap::new();
    for (src, path) in kd_games::teardown::make_voices() {
        if let Ok(x) = kd_audio::load_wav(&path) {
            clips.insert(src, std::sync::Arc::new(x));
        }
    }
    let mut mixer = kd_audio::Mixer::new(clips);
    mixer.volume = args.volume.clamp(0.0, 1.0);
    let mixer: kd_audio::SharedMixer = Arc::new(Mutex::new(mixer));
    let out = device(&args.device, kd_audio::output_devices());
    let _stream =
        match kd_audio::Output::open(mixer.clone(), out.as_deref(), kd_common::stdout_log()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("no sound output: {e}");
                return 1;
            }
        };
    println!("demo: the speaker walks a circle around you, 4 m away (ahead, right, behind, left)");
    let t0 = Instant::now();
    let mut seq = 0;
    while args.seconds <= 0.0 || t0.elapsed().as_secs_f64() < args.seconds {
        seq += 1;
        let az = (t0.elapsed().as_secs_f64() * 45.0 + 180.0).rem_euclid(360.0) - 180.0; // (a turn in 8 s)
        let mut f = Feed {
            seq,
            vol: 1.0,
            live: true,
            lang: "en".into(),
            ..Default::default()
        };
        f.speakers.insert(
            1,
            Speaker {
                src: 2,
                talk: true,
                gain: 1.0,
                az,
                el: 0.0,
                muffle: 0.0,
            },
        );
        kd_audio::lock(&mixer).set_feed(f);
        std::thread::sleep(Duration::from_millis(50));
        if seq % 10 == 0 {
            print!("\r  direction {az:4.0}   ");
            let _ = std::io::stdout().flush();
        }
    }
    println!();
    0
}

pub fn main(argv: Vec<String>) -> i32 {
    let args = match Args::try_parse_from(argv) {
        Ok(a) => a,
        Err(e) => {
            let _ = e.print();
            return if e.use_stderr() { 2 } else { 0 };
        }
    };
    if let Some(p) = args.transcribe.clone() {
        return transcribe(&args, &p);
    }
    if args.list {
        println!("outputs:");
        for (i, n) in kd_audio::output_devices().iter().enumerate() {
            println!("  {i:2}  {n}");
        }
        println!("microphones:");
        for (i, n) in kd_audio::input_devices().iter().enumerate() {
            println!("  {i:2}  {n}");
        }
        return 0;
    }
    if args.demo {
        return demo(&args);
    }

    let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new())); // (what to print above the status line)
    let l2 = lines.clone();
    let log: Log = Arc::new(move |s: &str| l2.lock().unwrap().push(s.to_string()));
    let mut mic_source: Option<MicSource> = None;
    if args.auto_speech {
        // (recorded lines in several languages, the real pipeline)
        let items = match auto_speech_items() {
            Ok(i) => i,
            Err(e) => {
                eprintln!("{e}");
                return 1;
            }
        };
        mic_source = Some(Box::new(move |l: Listener, log: Log| {
            Box::new(PlaylistMicrophone::new(l, items, 2.5, log)) as Box<dyn Mic>
        }));
    } else if let Some(p) = &args.mic_wav {
        // (a recording instead of the microphone)
        let (audio, sr) = match kd_audio::read_wav(std::path::Path::new(p)) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("{p}: {e}");
                return 1;
            }
        };
        let audio = kd_audio::resample(&audio, sr, kd_speech::RATE);
        mic_source = Some(Box::new(move |l: Listener, log: Log| {
            Box::new(WavMicrophone::new(l, audio, log)) as Box<dyn Mic>
        }));
    }
    let opts = Options {
        threads: args.threads,
        out_device: device(&args.device, kd_audio::output_devices()),
        mic_device: device(&args.mic_device, kd_audio::input_devices()),
        volume: args.volume,
        langs: if args.lang.is_none() && args.auto_speech {
            vec!["en".to_string()]
        } else {
            langs_arg(&args.lang)
        },
        no_mic: args.no_mic || args.auto || args.type_,
        io_dir: args.io_dir.clone().map(Into::into),
    };
    println!("preparing...");
    let mut rt = Runtime::start(kd_games::by_id("teardown"), log.clone(), opts, mic_source);
    let game = rt.game.clone();
    if args.type_ {
        // (lines piped in: a test)
        let (game, log) = (game.clone(), log.clone());
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                let line = line.trim().to_string();
                if !line.is_empty() {
                    let sent = game.lock().unwrap().send_text(&line);
                    log(&format!(
                        "typed{}: {line}",
                        if sent {
                            ""
                        } else {
                            " [no game to tell - is a level running?]"
                        }
                    ));
                }
            }
        });
    }
    if let Some((name, ms)) = rt.output_info() {
        println!("playing on: {name} (output delay {ms:.0} ms)");
    }
    for line in game.lock().unwrap().describe() {
        println!("{line}");
    }
    if args.auto {
        println!("auto test: {} lines, {AUTO_GAP:.0} s apart, once a level is running. Stay in the game and watch the chat.", AUTO_LINES.len());
    }
    let needs = rt.kind.needs;
    let t0 = Instant::now();
    let mut auto_sent = 0usize;
    let mut auto_next: Option<Instant> = None;
    // (--auto: the line being "said" - utterance, words, how many sent, next time)
    let mut auto_live: Option<(u32, Vec<String>, usize, Instant)> = None;
    while args.seconds <= 0.0 || t0.elapsed().as_secs_f64() < args.seconds {
        std::thread::sleep(Duration::from_millis(250));
        if args.auto {
            let g = game.lock().unwrap();
            if g.wants_mic() && g.connected() {
                if let Some((utt, words, k, nxt)) = auto_live.take() {
                    if Instant::now() >= nxt {
                        let k = (k + 2).min(words.len());
                        if k < words.len() {
                            g.send('l', utt, &words[..k].join(" "), None, None);
                            auto_live =
                                Some((utt, words, k, Instant::now() + Duration::from_millis(350)));
                        } else {
                            let line = words.join(" ");
                            let sent = g.send_text(&line); // (the finished line)
                            log(&format!(
                                "typed{}: {line}",
                                if sent {
                                    ""
                                } else {
                                    " [no game to tell - is a level running?]"
                                }
                            ));
                        }
                    } else {
                        auto_live = Some((utt, words, k, nxt));
                    }
                } else if auto_sent < AUTO_LINES.len() {
                    match auto_next {
                        None => auto_next = Some(Instant::now() + Duration::from_secs(3)), // (the first line 3 s after it is wanted)
                        Some(t) if Instant::now() >= t => {
                            auto_sent += 1;
                            auto_next = Some(Instant::now() + Duration::from_secs_f64(AUTO_GAP));
                            log(&format!("  (auto line {auto_sent} of {}: its words arrive live, then the line)", AUTO_LINES.len()));
                            let words = AUTO_LINES[auto_sent - 1]
                                .split_whitespace()
                                .map(String::from)
                                .collect();
                            auto_live = Some((auto_sent as u32, words, 0, Instant::now()));
                        }
                        _ => {}
                    }
                }
            }
        }
        rt.tick();
        let pending: Vec<String> = std::mem::take(&mut *lines.lock().unwrap());
        for line in pending {
            println!("\r{line:<118}");
        }
        let st = status_line(&rt.status(), needs);
        print!("\r{:<118.118}", st);
        let _ = std::io::stdout().flush();
    }
    println!();
    rt.stop();
    0
}
