//! The translator (service.rs) with a fake engine and a fake model source: the player's target and own languages,
//! a pair made the first time a language is seen (its line held while the models come), downloads off, pivots
//! through English, mixed lines put back together, exactly one reply per id, the pairs' states and their changes, the
//! least recently used pair let go.
//! (The fake "translation" of a piece into `xx` is `xx(<the piece>)`: what went through which model shows.)
use kd_common::null_log;
use kd_translate::service::{Engine, Event, PairStatus, Prepared, Provider, State, Status, Translator, MAX_PAIRS};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

struct Fake {
    /// "<to>": what it translates into; identity: gives the piece back as it is
    to: String,
    identity: bool,
}

impl Engine for Fake {
    fn translate(&self, text: &str) -> Result<String, String> {
        if text.contains("SLOW") {
            std::thread::sleep(Duration::from_millis(150));
        }
        if text.contains("FAIL") {
            return Err("the engine failed".into());
        }
        Ok(if self.identity { text.to_string() } else { format!("{}({text})", self.to) })
    }
}

/// The fake model source: routes by pair; a pair's preparation can be held (gate) to see it downloading.
#[derive(Default)]
struct Source {
    /// "ja>en" -> Prepared (default: one direction "<from>-<to>/1", or two through English)
    special: Mutex<HashMap<String, Prepared>>,
    /// pairs held until released
    held: Mutex<Vec<String>>,
    released: Condvar,
    /// directions loaded (how many times), and the engines handed out
    loads: Mutex<Vec<String>>,
    engines: Mutex<Vec<(String, Weak<dyn Engine>)>>,
    /// every pair asked for, and those whose models were downloaded
    prepared: Mutex<Vec<String>>,
    downloaded: Mutex<Vec<String>>,
    /// pairs whose models are on this PC already (the others need a download)
    on_pc: Mutex<Vec<String>>,
    /// progress steps sent while preparing
    steps: usize,
}

impl Source {
    fn hold(&self, pair: &str) {
        self.held.lock().unwrap().push(pair.into());
    }

    fn release(&self, pair: &str) {
        self.held.lock().unwrap().retain(|r| r != pair);
        self.released.notify_all();
    }

    fn alive(&self, id: &str) -> bool {
        self.engines.lock().unwrap().iter().any(|(i, w)| i == id && w.upgrade().is_some())
    }

    fn prepared(&self) -> Vec<String> {
        self.prepared.lock().unwrap().clone()
    }
}

impl Provider for Source {
    fn prepare(&self, from: &str, to: &str, download: bool, progress: &dyn Fn(u64, u64)) -> Prepared {
        let pair = format!("{from}>{to}");
        self.prepared.lock().unwrap().push(pair.clone());
        if let Some(p @ Prepared::Unavailable(_)) = self.special.lock().unwrap().get(&pair) {
            return p.clone();
        }
        if !self.on_pc.lock().unwrap().contains(&pair) {
            if !download {
                return Prepared::NotDownloaded(format!("{pair}: not on this PC"));
            }
            self.downloaded.lock().unwrap().push(pair.clone());
            for i in 0..=self.steps {
                progress(i as u64 * 10, self.steps.max(1) as u64 * 10);
            }
        }
        let mut held = self.held.lock().unwrap();
        while held.contains(&pair) {
            held = self.released.wait(held).unwrap();
        }
        drop(held);
        if let Some(p) = self.special.lock().unwrap().get(&pair) {
            return p.clone();
        }
        let ids: Vec<String> = if from == "en" || to == "en" {
            vec![format!("{from}-{to}/1")]
        } else {
            vec![format!("{from}-en/1"), format!("en-{to}/1")]
        };
        Prepared::Ready(ids.into_iter().map(|i| (i.clone(), PathBuf::from(i))).collect())
    }

    fn load(&self, id: &str, _dir: &Path) -> Result<Arc<dyn Engine>, String> {
        if id.starts_with("broken") {
            return Err("cannot load".into());
        }
        self.loads.lock().unwrap().push(id.to_string());
        let to = id.split('/').next().unwrap().rsplit('-').next().unwrap().to_string();
        let e: Arc<dyn Engine> = Arc::new(Fake { to, identity: id.starts_with("de-en") });
        self.engines.lock().unwrap().push((id.to_string(), Arc::downgrade(&e)));
        Ok(e)
    }
}

/// The translator and what it said.
struct Rig {
    t: Translator,
    src: Arc<Source>,
    events: Arc<(Mutex<Vec<Event>>, Condvar)>,
}

fn rig_with(src: Source) -> Rig {
    let src = Arc::new(src);
    let events: Arc<(Mutex<Vec<Event>>, Condvar)> = Arc::default();
    let ev = events.clone();
    let t = Translator::start(
        src.clone(),
        Arc::new(move |e| {
            ev.0.lock().unwrap().push(e);
            ev.1.notify_all();
        }),
        null_log(),
    );
    Rig { t, src, events }
}

fn rig() -> Rig {
    rig_with(Source::default())
}

fn langs(l: &[&str]) -> Vec<String> {
    l.iter().map(|s| s.to_string()).collect()
}

impl Rig {
    /// The player's setting: translate into `into` ("" off), speaking `known`, downloads on.
    fn set(&self, into: &str, known: &[&str]) {
        self.t.set_target(Some(into.to_string()), langs(known), true);
    }

    /// Waits (at most 5 s) until the events satisfy `cond`.
    fn wait(&self, what: &str, cond: impl Fn(&[Event]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut ev = self.events.0.lock().unwrap();
        while !cond(&ev) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for {what}: {ev:#?}");
            ev = self.events.1.wait_timeout(ev, left).unwrap().0;
        }
    }

    /// Waits until the last status told is `wire` (Status::wire: "en:ja>en=ready").
    fn status_is(&self, wire: &str) {
        let wire = wire.to_string();
        self.wait(&format!("status {wire}"), |ev| {
            ev.iter().rev().find_map(|e| match e {
                Event::Status(s) => Some(s.wire()),
                _ => None,
            }) == Some(wire.clone())
        });
    }

    fn replies(&self, id: i64) -> Vec<String> {
        self.events
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                Event::Reply { id: i, text, .. } if *i == id => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// Asks for a line and waits for its reply.
    fn ask(&self, id: i64, line: &str) -> String {
        let before = self.replies(id).len();
        assert!(self.t.request(id, line), "id {id} is new");
        let count = |ev: &[Event]| ev.iter().filter(|e| matches!(e, Event::Reply { id: i, .. } if *i == id)).count();
        self.wait(&format!("the reply to {id}"), |ev| count(ev) > before);
        // (and no second one)
        std::thread::sleep(Duration::from_millis(20));
        let r = self.replies(id);
        assert_eq!(r.len(), before + 1, "one reply to {id}");
        r[before].clone()
    }

    fn statuses(&self) -> Vec<Status> {
        self.events
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                Event::Status(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }
}

const JA: &str = "こんにちは、元気ですか？";
const KO: &str = "오늘 같이 게임할 사람?";
const ZH: &str = "我们今天去哪里玩？";
const ES: &str = "¿Alguien sabe dónde está la llave del sótano?";
const RU: &str = "Привет, как дела? Это русский текст.";

#[test]
fn lines_into_the_target() {
    let r = rig();
    r.set("en", &["en"]);
    r.status_is("en:");
    assert_eq!(r.ask(1, JA), format!("en({JA})"), "a language seen the first time: its pair made, the line waits");
    assert_eq!(r.ask(2, KO), format!("en({KO})"));
    r.status_is("en:ja>en=ready,ko>en=ready");
    assert_eq!(r.ask(3, "Does anyone know where the key is?"), "", "the player's own language: empty");
    assert_eq!(r.ask(4, "   "), "");
    assert_eq!(r.ask(5, ""), "");
    // (mixed: the Japanese translated, the English kept, the order kept)
    assert_eq!(
        r.ask(6, "I think we should go upstairs now. 上の階に宝物があるはずです。"),
        "I think we should go upstairs now. en(上の階に宝物があるはずです。)"
    );
    assert_eq!(
        r.ask(7, "こんにちは！ How is everyone doing today? 오늘 같이 게임할 사람?"),
        "en(こんにちは！) How is everyone doing today? en(오늘 같이 게임할 사람?)"
    );
    // (a failing engine: that piece kept; nothing translated - empty)
    assert_eq!(r.ask(8, "FAILです"), "");
    assert_eq!(r.t.status().pairs.len(), 2);
    assert_eq!(r.src.prepared(), ["ja>en", "ko>en"], "each pair made once");
    r.t.stop();
}

#[test]
fn own_languages_left_alone() {
    let r = rig();
    r.set("en", &["en", "ja"]);
    assert_eq!(r.ask(1, JA), "", "Japanese is the player's");
    assert_eq!(r.ask(2, KO), format!("en({KO})"));
    assert_eq!(
        r.ask(3, "上の階に宝物があるはずです。 오늘 같이 게임할 사람?"),
        "上の階に宝物があるはずです。en(오늘 같이 게임할 사람?)",
        "in a mixed line, only the stretch the player does not speak"
    );
    assert!(!r.src.prepared().contains(&"ja>en".to_string()), "no pair for a language the player speaks");
    // (the target is left alone too, spoken or not)
    r.set("ko", &["en"]);
    r.status_is("ko:");
    assert_eq!(r.ask(4, KO), "", "a line already in the target");
    assert_eq!(r.ask(5, "Does anyone know where the key is?"), "");
    assert_eq!(r.ask(6, JA), format!("ko(en({JA}))"));
    // (Chinese and Cantonese are one for the player)
    r.set("ja", &["zh"]);
    assert_eq!(r.ask(7, "你哋今晚去邊度玩？"), "", "a Chinese speaker reads Cantonese");
    assert_eq!(r.ask(8, "Where are we going tonight?"), "ja(Where are we going tonight?)", "English is not theirs");
    r.t.stop();
}

#[test]
fn a_language_now_spoken_lets_go_of_its_pair() {
    let r = rig();
    r.set("en", &["en"]);
    assert_eq!(r.ask(1, JA), format!("en({JA})"));
    r.status_is("en:ja>en=ready");
    assert!(r.src.alive("ja-en/1"));
    r.set("en", &["en", "ja"]);
    r.status_is("en:");
    assert_eq!(r.ask(2, JA), "");
    assert!(!r.src.alive("ja-en/1"), "its model let go");
    // (another target: every pair goes)
    r.set("en", &["en"]);
    assert_eq!(r.ask(3, KO), format!("en({KO})"));
    r.status_is("en:ko>en=ready");
    r.set("ja", &["en"]);
    r.status_is("ja:");
    assert!(!r.src.alive("ko-en/1"));
    assert_eq!(r.ask(4, KO), format!("ja(en({KO}))"));
    r.status_is("ja:ko>ja=ready");
    // (the same setting again: nothing happens - no new preparation)
    let n = r.src.prepared().len();
    r.set("ja", &["en"]);
    r.set("ja", &["en"]);
    assert_eq!(r.ask(5, "감사합니다 여러분"), "ja(en(감사합니다 여러분))");
    assert_eq!(r.src.prepared().len(), n);
    r.t.stop();
}

#[test]
fn pivots_through_english() {
    let r = rig();
    r.set("ko", &["en"]);
    assert_eq!(r.ask(1, "ありがとう"), "ko(en(ありがとう))", "two models through English");
    assert_eq!(r.ask(2, ES), format!("ko(en({ES}))"));
    assert_eq!(
        r.ask(3, "This is my favourite song. 「さくら」が好きです"),
        "This is my favourite song. ko(en(「さくら」が好きです))"
    );
    let loads = r.src.loads.lock().unwrap().clone();
    assert_eq!(loads.iter().filter(|l| l.as_str() == "en-ko/1").count(), 1, "the shared direction loaded once");
    // (into Japanese, the player speaking Korean: English pieces translated; two translated pieces with a space
    //  between them unless one side is Chinese or Japanese)
    r.set("ja", &["ko"]);
    assert_eq!(r.ask(4, "Thank you very much for the help"), "ja(Thank you very much for the help)");
    r.t.stop();
}

#[test]
fn a_new_language_is_fetched_and_its_line_held() {
    let src = Source { steps: 50, ..Default::default() };
    src.hold("ja>en");
    let r = rig_with(src);
    r.set("en", &["en"]);
    r.status_is("en:");
    assert!(r.src.prepared().is_empty(), "nothing fetched before a language shows up");
    // (not ready: the line waits for the models - no "" that would lose it)
    assert!(r.t.request(1, JA));
    r.status_is("en:ja>en=downloading 100");
    std::thread::sleep(Duration::from_millis(300));
    assert!(r.replies(1).is_empty(), "held while the models come");
    // (a line in the player's own language behind it waits its turn: the replies keep the lines' order)
    assert!(r.t.request(2, "Does anyone know where the key is?"));
    std::thread::sleep(Duration::from_millis(100));
    assert!(r.replies(2).is_empty());
    r.src.release("ja>en");
    r.status_is("en:ja>en=ready");
    let t0 = Instant::now();
    while r.replies(2).is_empty() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(r.replies(1), vec![format!("en({JA})")], "answered once they are ready");
    assert_eq!(r.replies(2), vec![String::new()]);
    assert_eq!(r.ask(3, JA), format!("en({JA})"));
    let seen: Vec<String> = r.statuses().iter().map(Status::wire).collect();
    // (loading until the first bytes come: models already here never show "downloading")
    assert_eq!(seen.first().map(String::as_str), Some("en:"));
    assert_eq!(seen.get(1).map(String::as_str), Some("en:ja>en=loading"));
    assert!(seen.iter().any(|s| s.starts_with("en:ja>en=downloading")), "{seen:?}");
    assert_eq!(seen.last().map(String::as_str), Some("en:ja>en=ready"));
    // (fifty progress steps in a moment: told at most every 0.5 s, plus the last one)
    let downloading = seen.iter().filter(|s| s.contains("downloading")).count();
    assert!(downloading <= 3, "{seen:?}");
    // (each status told is a change)
    assert!(seen.windows(2).all(|w| w[0] != w[1]), "{seen:?}");
    r.t.stop();
}

#[test]
fn downloads_off() {
    let src = Source::default();
    src.on_pc.lock().unwrap().push("es>en".into());
    let r = rig_with(src);
    r.t.set_target(Some("en".into()), langs(&["en"]), false);
    assert_eq!(r.ask(1, JA), "", "its models are not here: nothing downloaded, the reply empty");
    r.status_is("en:ja>en=unavailable");
    assert_eq!(r.t.status().pairs[0].state, State::NotDownloaded, "the window tells it apart");
    assert!(r.src.downloaded.lock().unwrap().is_empty(), "no download");
    assert_eq!(r.ask(2, ES), format!("en({ES})"), "models already on this PC are used");
    assert_eq!(r.ask(3, JA), "", "not asked again while downloads are off");
    assert_eq!(r.src.prepared().iter().filter(|p| *p == "ja>en").count(), 1);
    // (downloads on: the pair is made again the next time its language shows up)
    r.t.set_target(Some("en".into()), langs(&["en"]), true);
    r.status_is("en:es>en=ready");
    assert_eq!(r.ask(4, JA), format!("en({JA})"));
    assert_eq!(*r.src.downloaded.lock().unwrap(), ["ja>en"]);
    r.status_is("en:es>en=ready,ja>en=ready");
    r.t.stop();
}

#[test]
fn off_means_nothing() {
    let r = rig();
    // (off until the player chooses a language)
    assert_eq!(r.ask(1, JA), "");
    assert_eq!(r.ask(2, ES), "");
    assert!(r.src.prepared().is_empty(), "nothing fetched while off");
    assert_eq!(r.t.status(), Status::default());
    r.set("en", &["en"]);
    assert_eq!(r.ask(3, JA), format!("en({JA})"));
    // (off again: every pair let go, every line empty)
    r.t.set_target(None, langs(&["en"]), true);
    r.status_is("");
    assert_eq!(r.ask(4, JA), "");
    assert!(!r.src.alive("ja-en/1"));
    r.set("en", &["en"]);
    r.t.set_target(Some(String::new()), langs(&["en"]), true);
    r.status_is("");
    assert_eq!(r.ask(5, JA), "", "\"\" is off too");
    assert_eq!(r.src.prepared(), ["ja>en"]);
    r.t.stop();
}

#[test]
fn the_least_recently_used_pair_goes() {
    assert_eq!(MAX_PAIRS, 4);
    let r = rig();
    r.set("en", &["en"]);
    assert_eq!(r.ask(1, JA), format!("en({JA})"));
    assert_eq!(r.ask(2, KO), format!("en({KO})"));
    assert_eq!(r.ask(3, ZH), format!("en({ZH})"));
    assert_eq!(r.ask(4, ES), format!("en({ES})"));
    r.status_is("en:ja>en=ready,ko>en=ready,zh>en=ready,es>en=ready");
    // (Japanese used again: Korean is now the least recently used)
    assert_eq!(r.ask(5, JA), format!("en({JA})"));
    assert_eq!(r.ask(6, RU), format!("en({RU})"));
    r.status_is("en:ja>en=ready,zh>en=ready,es>en=ready,ru>en=ready");
    assert!(!r.src.alive("ko-en/1"), "Korean's model let go");
    assert!(r.src.alive("ja-en/1") && r.src.alive("ru-en/1"));
    // (Korean again: made again, and Chinese - now the least recently used - goes)
    assert_eq!(r.ask(7, KO), format!("en({KO})"));
    r.status_is("en:ja>en=ready,es>en=ready,ru>en=ready,ko>en=ready");
    assert!(!r.src.alive("zh-en/1"));
    // (pairs without models do not count: an unavailable one is listed, nothing else goes)
    r.src.special.lock().unwrap().insert("mt>en".into(), Prepared::Unavailable("no model".into()));
    assert_eq!(r.ask(8, "Xi ħadd jaf fejn hu ċ-ċavetta tal-bieb?"), "");
    r.status_is("en:ja>en=ready,es>en=ready,ru>en=ready,ko>en=ready,mt>en=unavailable");
    r.t.stop();
}

#[test]
fn short_lines_stay_the_players() {
    // (a Spanish speaker reading Spanish: "ok", "lol", "gg" are too short to tell - their own language first)
    let r = rig();
    r.set("es", &["es"]);
    for (i, line) in ["ok", "lol", "gg", "ok ok", "jajaja"].iter().enumerate() {
        assert_eq!(r.ask(i as i64 + 1, line), "", "{line}");
    }
    // (an English speaker reading English)
    r.set("en", &["en"]);
    for (i, line) in ["ok", "lol", "gg wp", "brb", "np"].iter().enumerate() {
        assert_eq!(r.ask(i as i64 + 10, line), "", "{line}");
    }
    assert!(r.src.prepared().is_empty(), "no pair made for a short line: {:?}", r.src.prepared());
    // (longer lines are translated as before)
    assert_eq!(r.ask(20, ES), format!("en({ES})"));
    r.t.stop();
}

#[test]
fn one_reply_per_id() {
    let r = rig();
    r.set("en", &["en"]);
    assert_eq!(r.ask(7, ES), format!("en({ES})"));
    // (the game keeps the line in its feed until it reads the reply: asked again, ignored)
    assert!(!r.t.request(7, ES));
    assert!(!r.t.request(7, "something else"));
    assert_eq!(r.ask(8, "Hola a todos, ¿qué tal la partida de hoy?"), "en(Hola a todos, ¿qué tal la partida de hoy?)");
    assert_eq!(r.replies(7).len(), 1);
    // (a new session: the ids start over)
    r.t.new_session();
    assert_eq!(r.ask(7, "Buenas noches a todos los jugadores"), "en(Buenas noches a todos los jugadores)");
    r.t.stop();
}

#[test]
fn a_new_session_drops_what_is_queued() {
    let r = rig();
    r.set("en", &["en"]);
    assert_eq!(r.ask(100, ES), format!("en({ES})"));
    // (the translator busy with a slow line; more queued behind it; then a new session: the queued ones are never
    // answered - their ids may be new lines' now - and the new session's line is)
    assert!(r.t.request(1, "Hola SLOW a todos, ¿qué tal la partida de hoy?"));
    for id in 2..=20 {
        assert!(r.t.request(id, "Hola a todos, ¿qué tal la partida de hoy?"));
    }
    r.t.new_session();
    assert_eq!(r.ask(2, "Buenas noches a todos los jugadores"), "en(Buenas noches a todos los jugadores)");
    std::thread::sleep(Duration::from_millis(100));
    for id in [1].into_iter().chain(3..=20) {
        assert!(r.replies(id).is_empty(), "{id}: the old session's line is not answered");
    }
    r.t.stop();
}

#[test]
fn unavailable_and_errors() {
    let src = Source::default();
    src.special.lock().unwrap().insert("mt>en".into(), Prepared::Unavailable("no model".into()));
    src.special.lock().unwrap().insert("fi>en".into(), Prepared::Error("offline".into()));
    src.special.lock().unwrap().insert("sv>en".into(), Prepared::Ready(vec![("broken/1".into(), PathBuf::from("x"))]));
    let r = rig_with(src);
    r.set("en", &["en"]);
    assert_eq!(r.ask(1, "Xi ħadd jaf fejn hu ċ-ċavetta tal-bieb?"), "");
    assert_eq!(r.ask(2, "Tietääkö kukaan, missä punaisen oven avain on?"), "");
    assert_eq!(r.ask(3, "Vet någon var nyckeln till den röda dörren är?"), "");
    r.status_is("en:mt>en=unavailable,fi>en=error,sv>en=error");
    // (a target Koetama can never have models for - Cantonese: every pair unavailable without asking for models)
    let n = r.src.prepared().len();
    r.set("yue", &["en"]);
    assert_eq!(r.ask(4, JA), "");
    r.status_is("yue:ja>yue=unavailable");
    assert_eq!(r.src.prepared().len(), n);
    r.t.stop();
}

#[test]
fn cantonese_is_its_own_pair() {
    let r = rig();
    r.set("en", &["en"]);
    assert_eq!(r.ask(1, "你哋今晚去邊度玩？"), "en(你哋今晚去邊度玩？)");
    assert_eq!(r.ask(2, ZH), format!("en({ZH})"));
    r.status_is("en:yue>en=ready,zh>en=ready");
    // (into Chinese: Cantonese is left alone - the same written language for the player)
    r.set("zh", &["en"]);
    assert_eq!(r.ask(3, "你哋今晚去邊度玩？"), "");
    r.t.stop();
}

#[test]
fn unchanged_translation_is_empty() {
    let r = rig();
    // (the fake de -> en model gives the text back: nothing to show)
    r.set("en", &["en"]);
    assert_eq!(r.ask(1, "Ich habe den Schlüssel im Keller gefunden."), "");
    r.status_is("en:de>en=ready");
    r.t.stop();
}

#[test]
fn state_words() {
    assert_eq!(State::Downloading(0.5).wire(), "downloading 50");
    assert_eq!(State::Downloading(0.5).word(), "downloading");
    assert_eq!(State::Ready.wire(), "ready");
    assert_eq!(State::NotDownloaded.wire(), "unavailable");
    let p = PairStatus { from: "ja".into(), to: "en".into(), state: State::NotDownloaded };
    assert_eq!(p.common().state, "unavailable");
}
