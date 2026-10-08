//! The translator (service.rs) with a fake engine and a fake model source: rules, pivots through English, mixed
//! lines put back together, exactly one reply per id, the rules' states and their changes, models let go.
//! (The fake "translation" of a piece into `xx` is `xx(<the piece>)`: what went through which model shows.)
use kd_common::null_log;
use kd_translate::service::{Engine, Event, Prepared, Provider, RuleStatus, State, Translator};
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

/// The fake model source: routes by rule; a rule's preparation can be held (gate) to see it downloading.
#[derive(Default)]
struct Source {
    /// "ja>en" -> Prepared (default: one direction "<from>-<to>/1", or two through English)
    special: Mutex<HashMap<String, Prepared>>,
    /// rules held until released
    held: Mutex<Vec<String>>,
    released: Condvar,
    /// directions loaded (how many times), and the engines handed out
    loads: Mutex<Vec<String>>,
    engines: Mutex<Vec<(String, Weak<dyn Engine>)>>,
    prepared: Mutex<Vec<String>>,
    /// progress steps sent while preparing
    steps: usize,
}

impl Source {
    fn hold(&self, rule: &str) {
        self.held.lock().unwrap().push(rule.into());
    }

    fn release(&self, rule: &str) {
        self.held.lock().unwrap().retain(|r| r != rule);
        self.released.notify_all();
    }

    fn alive(&self, id: &str) -> bool {
        self.engines.lock().unwrap().iter().any(|(i, w)| i == id && w.upgrade().is_some())
    }
}

impl Provider for Source {
    fn prepare(&self, from: &str, to: &str, progress: &dyn Fn(u64, u64)) -> Prepared {
        let rule = format!("{from}>{to}");
        self.prepared.lock().unwrap().push(rule.clone());
        for i in 0..=self.steps {
            progress(i as u64 * 10, self.steps.max(1) as u64 * 10);
        }
        let mut held = self.held.lock().unwrap();
        while held.contains(&rule) {
            held = self.released.wait(held).unwrap();
        }
        drop(held);
        if let Some(p) = self.special.lock().unwrap().get(&rule) {
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

fn rules(r: &[(&str, &str)]) -> Vec<(String, String)> {
    r.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

impl Rig {
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

    fn status_is(&self, wire: &str) {
        let wire = wire.to_string();
        self.wait(&format!("status {wire}"), |ev| {
            ev.iter().rev().find_map(|e| match e {
                Event::Status(s) => Some(kd_translate::service::status_text(s)),
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
                Event::Reply { id: i, text } if *i == id => Some(text.clone()),
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

    fn statuses(&self) -> Vec<Vec<RuleStatus>> {
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

#[test]
fn rules_and_lines() {
    let r = rig();
    r.t.set_rules(&rules(&[("ja", "en"), ("ko", "en")]));
    r.status_is("ja>en=ready,ko>en=ready");
    assert_eq!(r.ask(1, "こんにちは、元気ですか？"), "en(こんにちは、元気ですか？)");
    assert_eq!(r.ask(2, "오늘 같이 게임할 사람?"), "en(오늘 같이 게임할 사람?)");
    assert_eq!(r.ask(3, "Does anyone know where the key is?"), "", "nothing in a rule's language: empty");
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
    assert_eq!(r.t.status().len(), 2);
    r.t.stop();
}

#[test]
fn pivots_and_targets() {
    let r = rig();
    r.t.set_rules(&rules(&[("ja", "ko"), ("en", "ja")]));
    r.status_is("ja>ko=ready,en>ja=ready");
    assert_eq!(r.ask(1, "ありがとう"), "ko(en(ありがとう))", "two models through English");
    // (into Japanese; two translated pieces: a space between them unless one side is Chinese or Japanese)
    assert_eq!(r.ask(2, "Thank you very much for the help"), "ja(Thank you very much for the help)");
    assert_eq!(
        r.ask(3, "This is my favourite song. 「さくら」が好きです"),
        "ja(This is my favourite song.) ko(en(「さくら」が好きです))"
    );
    let loads = r.src.loads.lock().unwrap().clone();
    assert_eq!(loads.iter().filter(|l| l.as_str() == "en-ja/1").count(), 1);
    r.t.stop();
}

#[test]
fn shared_direction_loaded_once_and_unused_let_go() {
    let r = rig();
    r.t.set_rules(&rules(&[("ja", "en"), ("ko", "ja")]));
    r.status_is("ja>en=ready,ko>ja=ready");
    assert!(r.src.alive("ja-en/1") && r.src.alive("ko-en/1") && r.src.alive("en-ja/1"));
    // (ja > en no more: its model goes, the others stay)
    r.t.set_rules(&rules(&[("ko", "ja")]));
    r.status_is("ko>ja=ready");
    assert_eq!(r.ask(1, "안녕하세요 여러분"), "ja(en(안녕하세요 여러분))");
    assert!(!r.src.alive("ja-en/1"), "a model no rule uses is let go");
    assert!(r.src.alive("ko-en/1") && r.src.alive("en-ja/1"));
    // (the same rules again: nothing happens - no new preparation)
    let n = r.src.prepared.lock().unwrap().len();
    r.t.set_rules(&rules(&[("ko", "ja")]));
    r.t.set_rules(&rules(&[("ko", "ja")]));
    assert_eq!(r.ask(2, "감사합니다"), "ja(en(감사합니다))");
    assert_eq!(r.src.prepared.lock().unwrap().len(), n);
    // (no rules: off - everything let go, every line empty)
    r.t.set_rules(&[]);
    r.status_is("");
    assert_eq!(r.ask(3, "감사합니다"), "");
    assert!(!r.src.alive("ko-en/1") && !r.src.alive("en-ja/1"));
    r.t.stop();
}

#[test]
fn one_reply_per_id() {
    let r = rig();
    r.t.set_rules(&rules(&[("es", "en")]));
    r.status_is("es>en=ready");
    assert_eq!(r.ask(7, "¿Alguien sabe dónde está la llave?"), "en(¿Alguien sabe dónde está la llave?)");
    // (the game keeps the line in its feed until it reads the reply: asked again, ignored)
    assert!(!r.t.request(7, "¿Alguien sabe dónde está la llave?"));
    assert!(!r.t.request(7, "something else"));
    assert_eq!(r.ask(8, "Hola a todos, ¿qué tal la partida?"), "en(Hola a todos, ¿qué tal la partida?)");
    assert_eq!(r.replies(7).len(), 1);
    // (a new session: the ids start over)
    r.t.new_session();
    assert_eq!(r.ask(7, "Buenas noches a todos los jugadores"), "en(Buenas noches a todos los jugadores)");
    r.t.stop();
}

#[test]
fn a_new_session_drops_what_is_queued() {
    let r = rig();
    r.t.set_rules(&rules(&[("es", "en")]));
    r.status_is("es>en=ready");
    // (the translator busy with a slow line; more queued behind it; then a new session: the queued ones are never
    // answered - their ids may be new lines' now - and the new session's line is)
    assert!(r.t.request(1, "Hola SLOW a todos, ¿qué tal la partida?"));
    for id in 2..=20 {
        assert!(r.t.request(id, "Hola a todos, ¿qué tal la partida?"));
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
fn states_downloading_loading_ready() {
    let src = Source { steps: 50, ..Default::default() };
    src.hold("ja>en");
    let r = rig_with(src);
    r.t.set_rules(&rules(&[("ja", "en")]));
    r.status_is("ja>en=downloading 100");
    // (not ready: an empty reply at once, not a wait)
    assert_eq!(r.ask(1, "こんにちは"), "");
    r.src.release("ja>en");
    r.status_is("ja>en=ready");
    assert_eq!(r.ask(2, "こんにちは"), "en(こんにちは)");
    let seen: Vec<String> = r.statuses().iter().map(|s| kd_translate::service::status_text(s)).collect();
    assert_eq!(seen.first().map(String::as_str), Some("ja>en=downloading 0"));
    assert!(seen.contains(&"ja>en=loading".to_string()), "{seen:?}");
    assert_eq!(seen.last().map(String::as_str), Some("ja>en=ready"));
    // (fifty progress steps in a moment: told at most every 0.5 s, plus the last one)
    let downloading = seen.iter().filter(|s| s.contains("downloading")).count();
    assert!(downloading <= 3, "{seen:?}");
    // (each status told is a change)
    assert!(seen.windows(2).all(|w| w[0] != w[1]), "{seen:?}");
    r.t.stop();
}

#[test]
fn unavailable_and_errors() {
    let src = Source::default();
    src.special.lock().unwrap().insert("mt>en".into(), Prepared::Unavailable("no model".into()));
    src.special.lock().unwrap().insert("fi>en".into(), Prepared::Error("offline".into()));
    src.special.lock().unwrap().insert("sv>en".into(), Prepared::Ready(vec![("broken/1".into(), PathBuf::from("x"))]));
    let r = rig_with(src);
    r.t.set_rules(&rules(&[("mt", "en"), ("fi", "en")]));
    r.status_is("mt>en=unavailable,fi>en=error");
    assert_eq!(r.ask(1, "Xi ħadd jaf fejn hu ċ-ċavetta tal-bieb?"), "");
    assert_eq!(r.ask(2, "Tietääkö kukaan, missä punaisen oven avain on?"), "");
    // (rules Koetama can never have: unavailable without asking for models)
    r.t.set_rules(&rules(&[("ja", "ja"), ("en", "yue")]));
    r.status_is("ja>ja=unavailable,en>yue=unavailable");
    assert!(!r.src.prepared.lock().unwrap().iter().any(|p| p == "ja>ja" || p == "en>yue"));
    r.t.set_rules(&rules(&[("xx", "en"), ("sv", "en")]));
    r.status_is("xx>en=unavailable,sv>en=error");
    // (more than two rules: the first two)
    r.t.set_rules(&rules(&[("ja", "en"), ("ko", "en"), ("zh", "en")]));
    r.status_is("ja>en=ready,ko>en=ready");
    r.t.stop();
}

#[test]
fn chinese_and_cantonese_stand_in() {
    let r = rig();
    r.t.set_rules(&rules(&[("yue", "en")]));
    r.status_is("yue>en=ready");
    assert_eq!(r.ask(1, "你哋今晚去邊度玩？"), "en(你哋今晚去邊度玩？)");
    assert_eq!(
        r.ask(2, "我们今天去哪里玩？"),
        "en(我们今天去哪里玩？)",
        "Chinese through the Cantonese rule when there is no Chinese one"
    );
    r.t.set_rules(&rules(&[("zh", "en"), ("yue", "ja")]));
    r.status_is("zh>en=ready,yue>ja=ready");
    assert_eq!(r.ask(3, "我们今天去哪里玩？"), "en(我们今天去哪里玩？)");
    assert_eq!(r.ask(4, "你哋今晚去邊度玩？"), "ja(en(你哋今晚去邊度玩？))", "each to its own rule");
    r.t.stop();
}

#[test]
fn unchanged_translation_is_empty() {
    let r = rig();
    // (the fake de -> en model gives the text back: nothing to show)
    r.t.set_rules(&rules(&[("de", "en")]));
    r.status_is("de>en=ready");
    assert_eq!(r.ask(1, "Ich habe den Schlüssel gefunden."), "");
    r.t.stop();
}

#[test]
fn state_words() {
    assert_eq!(State::Downloading(0.5).wire(), "downloading 50");
    assert_eq!(State::Downloading(0.5).word(), "downloading");
    assert_eq!(State::Ready.wire(), "ready");
}
