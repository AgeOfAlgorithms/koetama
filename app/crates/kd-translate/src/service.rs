//! The translator (PROTOCOL.md "Translation (version 6)"): the game's rules and the lines it wants translated, on a
//! thread of its own.
//!
//! Up to MAX_RULES rules "from A into B". A new rule's models are got ready at once on a helper thread (Mozilla's
//! list, the downloads: Provider::prepare), then loaded on the translator's thread; a model no rule uses any more is
//! let go. Each rule has a state (ready, downloading N %, loading, unavailable, error) - told on each change through
//! Event::Status (a download's progress at most every STATUS_EVERY).
//!
//! A request (id, line): the line's stretches (detect.rs); each stretch in a rule's source language is translated
//! through the rule's model(s) (two through English), the others are kept as they are, and the result keeps their
//! order. Exactly one Event::Reply per id: "" when nothing in the line matched a ready rule (or the translation is the
//! line itself). Ids are the game's, unique in its session: an id already queued or answered is ignored, and a new
//! session (new_session) forgets them and drops what is still queued from the old one.
use crate::catalog::{self, Catalog};
use crate::detect;
use kd_common::Log;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// the most rules a player has (PROTOCOL.md)
pub const MAX_RULES: usize = 2;
/// a download's progress is told at most this often
pub const STATUS_EVERY: Duration = Duration::from_millis(500);
/// a rule whose models failed (no internet, a broken file) is tried again after this
pub const RETRY_AFTER: Duration = Duration::from_secs(60);
/// the ids remembered per session (a game keeps a line until its reply arrives: answered ones come back for a while)
const SEEN_MAX: usize = 4096;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One translation direction loaded (kd_translate::Model; tests: a fake).
pub trait Engine: Send + Sync {
    /// One piece of a chat line into the direction's target language.
    fn translate(&self, text: &str) -> Result<String, String>;
}

/// A Model behind a lock (Engine needs Sync; the translator's one thread uses it anyway).
struct Loaded(Mutex<crate::Model>);

impl Engine for Loaded {
    fn translate(&self, text: &str) -> Result<String, String> {
        lock(&self.0).translate(text)
    }
}

/// What getting a rule's models ready came to.
#[derive(Clone, Debug, PartialEq)]
pub enum Prepared {
    /// [(the direction's id - "ja-en/2.1", its folder)], in the order a line goes through them
    Ready(Vec<(String, PathBuf)>),
    /// no model for it (why)
    Unavailable(String),
    /// it failed (why): tried again after RETRY_AFTER
    Error(String),
}

/// Where a rule's models come from (Mozilla; tests: fakes).
pub trait Provider: Send + Sync {
    /// The directions the rule from -> to (Koetama's codes) needs, their files there (downloaded when missing:
    /// progress(bytes done, bytes total)). Blocking - a helper thread runs it.
    fn prepare(&self, from: &str, to: &str, progress: &dyn Fn(u64, u64)) -> Prepared;
    /// One direction's model from its folder (blocking).
    fn load(&self, id: &str, dir: &Path) -> Result<Arc<dyn Engine>, String>;
}

/// Mozilla's models (catalog.rs) in Koetama's translation folder, run by kd_translate::Model.
pub struct Mozilla {
    root: PathBuf,
    log: Log,
    /// the list, and when it was read
    list: Mutex<Option<(Instant, Arc<Catalog>)>>,
    /// one rule's downloads at a time (two rules may share a direction)
    busy: Mutex<()>,
}

impl Mozilla {
    pub fn new(root: PathBuf, log: Log) -> Mozilla {
        Mozilla { root, log, list: Mutex::new(None), busy: Mutex::new(()) }
    }

    fn catalog(&self) -> Result<Arc<Catalog>, String> {
        let mut list = lock(&self.list);
        if let Some((t, c)) = list.as_ref() {
            if t.elapsed() < catalog::LIST_MAX_AGE {
                return Ok(c.clone());
            }
        }
        let c = Arc::new(catalog::load_list(&self.root, catalog::LIST_MAX_AGE, &catalog::get_list, &*self.log)?);
        *list = Some((Instant::now(), c.clone()));
        Ok(c)
    }
}

impl Provider for Mozilla {
    fn prepare(&self, from: &str, to: &str, progress: &dyn Fn(u64, u64)) -> Prepared {
        let _one = lock(&self.busy);
        let cat = match self.catalog() {
            Ok(c) => c,
            Err(e) => return Prepared::Error(e),
        };
        let dirs = match cat.route(from, to) {
            Ok(d) => d,
            Err(why) => return Prepared::Unavailable(why),
        };
        match catalog::fetch_route(&self.root, &dirs, progress, &catalog::download_file, &*self.log) {
            Ok(paths) => Prepared::Ready(dirs.iter().map(|d| d.id()).zip(paths).collect()),
            Err(e) => Prepared::Error(e),
        }
    }

    fn load(&self, _id: &str, dir: &Path) -> Result<Arc<dyn Engine>, String> {
        crate::Model::load(dir).map(|m| Arc::new(Loaded(Mutex::new(m))) as Arc<dyn Engine>)
    }
}

/// A rule's state.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Ready,
    /// 0..1 of the rule's files
    Downloading(f64),
    Loading,
    /// Mozilla has no model for it (or it is not a rule: the same language twice, a language Koetama does not have)
    Unavailable,
    Error,
}

impl State {
    /// As the game reads it: "ready", "downloading 42", "loading", "unavailable", "error".
    pub fn wire(&self) -> String {
        match self {
            State::Ready => "ready".into(),
            State::Downloading(f) => format!("downloading {}", percent(*f)),
            State::Loading => "loading".into(),
            State::Unavailable => "unavailable".into(),
            State::Error => "error".into(),
        }
    }

    /// The state's name without the progress ("downloading").
    pub fn word(&self) -> &'static str {
        match self {
            State::Ready => "ready",
            State::Downloading(_) => "downloading",
            State::Loading => "loading",
            State::Unavailable => "unavailable",
            State::Error => "error",
        }
    }
}

/// 0..1 as a whole percent, 0..=100 (rounded down: 100 only when all is there).
pub fn percent(f: f64) -> u32 {
    if f.is_finite() {
        (f.clamp(0.0, 1.0) * 100.0).floor() as u32
    } else {
        0
    }
}

/// One rule and its state.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleStatus {
    pub from: String,
    pub to: String,
    pub state: State,
}

impl RuleStatus {
    /// "ja>en=downloading 42"
    pub fn wire(&self) -> String {
        format!("{}>{}={}", self.from, self.to, self.state.wire())
    }

    /// As the game connectors take it.
    pub fn common(&self) -> kd_common::feed::RuleState {
        let progress = match self.state {
            State::Downloading(f) => f,
            State::Ready => 1.0,
            _ => 0.0,
        };
        kd_common::feed::RuleState {
            from: self.from.clone(),
            to: self.to.clone(),
            state: self.state.word().into(),
            progress,
        }
    }
}

/// The rules' states as the game reads them (message kind 'd'): each rule's wire(), comma-separated ("" for none).
pub fn status_text(rules: &[RuleStatus]) -> String {
    rules.iter().map(RuleStatus::wire).collect::<Vec<_>>().join(",")
}

/// What the translator tells the program.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// the reply to request `id` ("" = nothing to show)
    Reply { id: i64, text: String },
    /// the rules' states changed
    Status(Vec<RuleStatus>),
}

pub type OnEvent = Arc<dyn Fn(Event) + Send + Sync>;

enum Msg {
    Rules(Vec<(String, String)>),
    Request { session: u64, id: i64, text: String },
    Progress { job: u64, frac: f64 },
    Prepared { job: u64, result: Prepared },
    Stop,
}

/// The ids of this session queued or answered (the newest SEEN_MAX).
#[derive(Default)]
struct Seen {
    set: HashSet<i64>,
    order: VecDeque<i64>,
}

impl Seen {
    /// true when it is new (and now remembered)
    fn add(&mut self, id: i64) -> bool {
        if !self.set.insert(id) {
            return false;
        }
        self.order.push_back(id);
        if self.order.len() > SEEN_MAX {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        true
    }
}

struct Inner {
    tx: Mutex<Sender<Msg>>,
    seen: Mutex<Seen>,
    session: AtomicU64,
    /// the rules as last handed over (set_rules is called with every feed: only a change goes to the thread)
    rules: Mutex<Vec<(String, String)>>,
    status: Mutex<Vec<RuleStatus>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

/// The translator: a handle (clones share it); its thread runs until stop() (or the last handle is dropped).
#[derive(Clone)]
pub struct Translator {
    inner: Arc<Inner>,
}

impl Translator {
    /// Starts the thread. on_event: each reply and status change (called on the translator's threads: it must not
    /// wait on them).
    pub fn start(provider: Arc<dyn Provider>, on_event: OnEvent, log: Log) -> Translator {
        let (tx, rx) = mpsc::channel();
        let inner = Arc::new(Inner {
            tx: Mutex::new(tx.clone()),
            seen: Mutex::new(Seen::default()),
            session: AtomicU64::new(0),
            rules: Mutex::new(Vec::new()),
            status: Mutex::new(Vec::new()),
            thread: Mutex::new(None),
        });
        let weak = Arc::downgrade(&inner);
        let thread = std::thread::Builder::new()
            .name("translator".into())
            .spawn(move || {
                let mut w = Worker {
                    provider,
                    on_event,
                    log,
                    tx,
                    inner: weak,
                    slots: Vec::new(),
                    models: HashMap::new(),
                    next_job: 0,
                    told: Vec::new(),
                    told_at: None,
                    last_error: String::new(),
                };
                w.run(rx);
            })
            .ok();
        *lock(&inner.thread) = thread;
        Translator { inner }
    }

    /// The translator with Mozilla's models in Koetama's translation folder.
    pub fn mozilla(on_event: OnEvent, log: Log) -> Translator {
        Translator::start(Arc::new(Mozilla::new(catalog::root(), log.clone())), on_event, log)
    }

    fn send(&self, m: Msg) {
        let _ = lock(&self.inner.tx).send(m);
    }

    /// The game's rules (the first MAX_RULES): a change gets the new ones' models ready and lets go of the old ones'.
    /// Cheap when nothing changed (every feed calls it).
    pub fn set_rules(&self, rules: &[(String, String)]) {
        let rules: Vec<(String, String)> = rules.iter().take(MAX_RULES).cloned().collect();
        let mut kept = lock(&self.inner.rules);
        if *kept != rules {
            kept.clone_from(&rules);
            self.send(Msg::Rules(rules));
        }
    }

    /// A line to translate; false when its id is already queued or answered in this session (ignored).
    pub fn request(&self, id: i64, text: &str) -> bool {
        if !lock(&self.inner.seen).add(id) {
            return false;
        }
        let session = self.inner.session.load(Ordering::SeqCst);
        self.send(Msg::Request { session, id, text: text.to_string() });
        true
    }

    /// The game started a new session (its ids start over): the ids are forgotten, the old session's queued lines
    /// dropped (never answered).
    pub fn new_session(&self) {
        let mut seen = lock(&self.inner.seen);
        *seen = Seen::default();
        self.inner.session.fetch_add(1, Ordering::SeqCst);
    }

    /// The rules' states as last told.
    pub fn status(&self) -> Vec<RuleStatus> {
        lock(&self.inner.status).clone()
    }

    /// Stops the thread (after the line it is on) and waits for it. Helper threads still downloading finish on
    /// their own; what they bring is not used.
    pub fn stop(&self) {
        self.send(Msg::Stop);
        let t = lock(&self.inner.thread).take();
        if let Some(t) = t {
            if t.thread().id() != std::thread::current().id() {
                let _ = t.join();
            }
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = lock(&self.tx).send(Msg::Stop);
    }
}

/// A rule on the thread.
struct Slot {
    from: String,
    to: String,
    state: State,
    /// the helper getting its models ready (its messages carry this; a dropped rule's are ignored)
    job: u64,
    /// the direction ids it goes through, once ready
    route: Vec<String>,
    failed_at: Option<Instant>,
}

impl Slot {
    fn status(&self) -> RuleStatus {
        RuleStatus { from: self.from.clone(), to: self.to.clone(), state: self.state.clone() }
    }
}

struct Worker {
    provider: Arc<dyn Provider>,
    on_event: OnEvent,
    log: Log,
    tx: Sender<Msg>,
    inner: std::sync::Weak<Inner>,
    slots: Vec<Slot>,
    /// direction id -> its model (shared by the rules through it)
    models: HashMap<String, Arc<dyn Engine>>,
    next_job: u64,
    /// the states as last told, and when
    told: Vec<RuleStatus>,
    told_at: Option<Instant>,
    /// the last translation error logged (each new one once)
    last_error: String,
}

/// A rule Koetama can never have a model for (no need to ask Mozilla's list).
fn never(from: &str, to: &str) -> bool {
    from == to || catalog::mozilla_code(from, true).is_none() || catalog::mozilla_code(to, false).is_none()
}

impl Worker {
    fn run(&mut self, rx: mpsc::Receiver<Msg>) {
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(Msg::Rules(r)) => self.set_rules(r),
                Ok(Msg::Request { session, id, text }) => {
                    let Some(inner) = self.inner.upgrade() else {
                        break;
                    };
                    // (a line from a session gone: its id may be a new line's now - never answered)
                    if session == inner.session.load(Ordering::SeqCst) {
                        let text = self.translate_line(&text);
                        // (the session may have ended while it was translated)
                        if session == inner.session.load(Ordering::SeqCst) {
                            (self.on_event)(Event::Reply { id, text });
                        }
                    }
                }
                Ok(Msg::Progress { job, frac }) => {
                    if let Some(s) =
                        self.slots.iter_mut().find(|s| s.job == job && matches!(s.state, State::Downloading(_)))
                    {
                        s.state = State::Downloading(frac);
                    }
                }
                Ok(Msg::Prepared { job, result }) => self.prepared(job, result),
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.retry_failed();
            self.tell();
        }
    }

    /// Starts a helper getting a rule's models ready.
    fn fetch(&mut self, i: usize) {
        self.next_job += 1;
        let job = self.next_job;
        let s = &mut self.slots[i];
        s.job = job;
        s.state = State::Downloading(0.0);
        s.failed_at = None;
        let (provider, tx, from, to) = (self.provider.clone(), self.tx.clone(), s.from.clone(), s.to.clone());
        let tx2 = tx.clone();
        let spawned = std::thread::Builder::new().name("translation models".into()).spawn(move || {
            let progress = move |done: u64, total: u64| {
                let frac = if total > 0 { done as f64 / total as f64 } else { 0.0 };
                let _ = tx2.send(Msg::Progress { job, frac });
            };
            let result = provider.prepare(&from, &to, &progress);
            let _ = tx.send(Msg::Prepared { job, result });
        });
        if let Err(e) = spawned {
            self.slots[i].state = State::Error;
            self.slots[i].failed_at = Some(Instant::now());
            (self.log)(&format!("translation: cannot start a download: {e}"));
        }
    }

    fn set_rules(&mut self, rules: Vec<(String, String)>) {
        let mut old = std::mem::take(&mut self.slots);
        for (from, to) in rules {
            if self.slots.iter().any(|s| s.from == from && s.to == to) {
                continue;
            }
            if let Some(i) = old.iter().position(|s| s.from == from && s.to == to) {
                self.slots.push(old.remove(i));
                continue;
            }
            let unavailable = never(&from, &to);
            self.slots.push(Slot {
                from: from.clone(),
                to: to.clone(),
                state: if unavailable { State::Unavailable } else { State::Downloading(0.0) },
                job: 0,
                route: Vec::new(),
                failed_at: None,
            });
            if unavailable {
                (self.log)(&format!("translation: {from} > {to} is not available (no model for it)"));
            } else {
                let i = self.slots.len() - 1;
                self.fetch(i);
            }
        }
        self.unload_unused();
    }

    /// Lets go of the models no rule goes through.
    fn unload_unused(&mut self) {
        let used: HashSet<&String> = self.slots.iter().flat_map(|s| s.route.iter()).collect();
        let gone: Vec<String> = self.models.keys().filter(|k| !used.contains(k)).cloned().collect();
        for k in gone {
            self.models.remove(&k);
            (self.log)(&format!("translation: let go of the {k} model"));
        }
    }

    fn prepared(&mut self, job: u64, result: Prepared) {
        let Some(i) = self.slots.iter().position(|s| s.job == job) else {
            return; // (the rule is gone)
        };
        let rule = format!("{} > {}", self.slots[i].from, self.slots[i].to);
        match result {
            Prepared::Ready(dirs) => {
                self.slots[i].state = State::Loading;
                self.tell();
                let mut ids = Vec::new();
                for (id, dir) in dirs {
                    if !self.models.contains_key(&id) {
                        let t0 = Instant::now();
                        match self.provider.load(&id, &dir) {
                            Ok(m) => {
                                (self.log)(&format!(
                                    "translation: loaded the {id} model ({:.2} s)",
                                    t0.elapsed().as_secs_f64()
                                ));
                                self.models.insert(id.clone(), m);
                            }
                            Err(e) => {
                                (self.log)(&format!("translation: {rule}: the {id} model could not be loaded: {e}"));
                                self.slots[i].state = State::Error;
                                self.slots[i].failed_at = Some(Instant::now());
                                self.unload_unused();
                                return;
                            }
                        }
                    }
                    ids.push(id);
                }
                self.slots[i].route = ids;
                self.slots[i].state = State::Ready;
                (self.log)(&format!("translation: {rule} ready"));
                self.unload_unused();
            }
            Prepared::Unavailable(why) => {
                self.slots[i].state = State::Unavailable;
                (self.log)(&format!("translation: not available: {why}"));
            }
            Prepared::Error(e) => {
                self.slots[i].state = State::Error;
                self.slots[i].failed_at = Some(Instant::now());
                (self.log)(&format!("translation: {rule}: {e} (tried again in {} s)", RETRY_AFTER.as_secs()));
            }
        }
    }

    fn retry_failed(&mut self) {
        for i in 0..self.slots.len() {
            if self.slots[i].state == State::Error
                && self.slots[i].failed_at.is_some_and(|t| t.elapsed() >= RETRY_AFTER)
            {
                self.fetch(i);
            }
        }
    }

    /// Tells the states when they changed (a download's progress alone: at most every STATUS_EVERY).
    fn tell(&mut self) {
        let now: Vec<RuleStatus> = self.slots.iter().map(Slot::status).collect();
        let wire = |v: &[RuleStatus]| status_text(v);
        if wire(&now) == wire(&self.told) {
            return;
        }
        let words =
            |v: &[RuleStatus]| v.iter().map(|r| (r.from.clone(), r.to.clone(), r.state.word())).collect::<Vec<_>>();
        let progress_only = words(&now) == words(&self.told);
        if progress_only && self.told_at.is_some_and(|t| t.elapsed() < STATUS_EVERY) {
            return;
        }
        self.told = now.clone();
        self.told_at = Some(Instant::now());
        if let Some(inner) = self.inner.upgrade() {
            *lock(&inner.status) = now.clone();
        }
        (self.on_event)(Event::Status(now));
    }

    /// The ready rule a stretch in `lang` goes through: the first rule from that language; Chinese and Cantonese
    /// stand in for each other when only the other has a rule (the same script: detect.rs tells them apart only by
    /// a few characters).
    fn rule_for(&self, lang: &str) -> Option<&Slot> {
        let exact = self.slots.iter().find(|s| s.from == lang);
        let near = || match lang {
            "zh" => self.slots.iter().find(|s| s.from == "yue"),
            "yue" => self.slots.iter().find(|s| s.from == "zh"),
            _ => None,
        };
        exact.or_else(near).filter(|s| s.state == State::Ready)
    }

    /// One piece through a rule's model(s).
    fn through(&self, s: &Slot, text: &str) -> Result<String, String> {
        let mut t = text.to_string();
        for id in &s.route {
            let m = self.models.get(id).ok_or(format!("the {id} model is not loaded"))?;
            t = m.translate(&t)?;
        }
        Ok(t)
    }

    /// A line's translation ("" when nothing in it was translated).
    fn translate_line(&mut self, line: &str) -> String {
        // (piece, translated)
        let mut pieces: Vec<(String, bool)> = Vec::new();
        for st in detect::stretches(line) {
            let text = st.text(line);
            let done = match st.lang.and_then(|l| self.rule_for(l)) {
                Some(s) if !text.trim().is_empty() => match self.through(s, text.trim()) {
                    Ok(t) => Some(t),
                    Err(e) => {
                        let msg = format!("translation: {} > {}: {e}", s.from, s.to);
                        if msg != self.last_error {
                            (self.log)(&msg);
                            self.last_error = msg;
                        }
                        None
                    }
                },
                _ => None,
            };
            match done {
                Some(t) => pieces.push((t.trim().to_string(), true)),
                None => pieces.push((text.to_string(), false)),
            }
        }
        if !pieces.iter().any(|p| p.1) {
            return String::new();
        }
        let out = join_pieces(&pieces);
        if out == line.trim() {
            String::new() // (nothing changed: nothing to show)
        } else {
            out
        }
    }
}

/// The pieces of a line again, in order: two kept pieces as they were; next to a translated one a space, unless
/// either side is Chinese or Japanese (no spaces there).
pub fn join_pieces(pieces: &[(String, bool)]) -> String {
    let mut out = String::new();
    let mut last_translated = false;
    for (i, (text, translated)) in pieces.iter().enumerate() {
        if i == 0 || (!translated && !last_translated) {
            out.push_str(text);
        } else {
            let left = out.trim_end().to_string();
            let right = text.trim_start();
            let tight = left.chars().next_back().is_none_or(detect::no_space)
                || right.chars().next().is_none_or(detect::no_space);
            out = left;
            if !tight {
                out.push(' ');
            }
            out.push_str(right);
        }
        last_translated = *translated;
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire() {
        let r = |f: &str, t: &str, s: State| RuleStatus { from: f.into(), to: t.into(), state: s };
        assert_eq!(
            status_text(&[r("ja", "en", State::Ready), r("ko", "en", State::Downloading(0.429))]),
            "ja>en=ready,ko>en=downloading 42"
        );
        assert_eq!(
            status_text(&[r("mt", "en", State::Unavailable), r("ja", "ko", State::Loading)]),
            "mt>en=unavailable,ja>ko=loading"
        );
        assert_eq!(status_text(&[r("de", "en", State::Error)]), "de>en=error");
        assert_eq!(status_text(&[]), "");
        let all =
            [r("ja", "en", State::Ready), r("ko", "en", State::Downloading(0.429)), r("mt", "en", State::Unavailable)];
        let common: Vec<_> = all.iter().map(RuleStatus::common).collect();
        assert_eq!(kd_common::feed::rules_wire(&common), status_text(&all), "the connectors write the same");
        assert_eq!((percent(1.0), percent(0.999), percent(-1.0), percent(f64::NAN)), (100, 99, 0, 0));
    }

    #[test]
    fn joining() {
        let p = |v: &[(&str, bool)]| join_pieces(&v.iter().map(|(t, b)| (t.to_string(), *b)).collect::<Vec<_>>());
        assert_eq!(p(&[("Hello", false), (" How are you?", true)]), "Hello How are you?");
        assert_eq!(p(&[("Hi.", true), ("こんにちは", false)]), "Hi. こんにちは".replace(' ', ""));
        assert_eq!(p(&[("你好", true), ("世界", true)]), "你好世界");
        assert_eq!(p(&[("Hello", true), ("안녕", false)]), "Hello 안녕");
        assert_eq!(p(&[("a ", false), (" b", false)]), "a  b");
        assert_eq!(p(&[("  x  ", true)]), "x");
    }

    #[test]
    fn seen_ids() {
        let mut s = Seen::default();
        assert!(s.add(1) && !s.add(1) && s.add(2));
        for i in 3..(SEEN_MAX as i64 + 10) {
            s.add(i);
        }
        assert!(s.set.len() <= SEEN_MAX && s.add(1), "the oldest are forgotten");
    }
}
