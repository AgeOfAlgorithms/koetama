//! The translator (PROTOCOL.md "Translation"): the player's setting and the lines the game wants translated, on a
//! thread of its own.
//!
//! The setting (set_target) is the player's, from Koetama's window: the language chat is translated INTO (None: off),
//! the languages the player speaks (left alone), and whether models may be downloaded. The game only sends lines.
//!
//! A request (id, line): the line's stretches (detect.rs, short or unsure ones told among the player's languages and
//! the target first: "ok" and "lol" stay the player's). Each stretch in a language the player does not speak, and that
//! is not the target, is translated into the target through that language's PAIR (one model, two through English);
//! the others are kept as they are, and the result keeps their order. A pair is made the first time its language
//! appears: its models got ready on a helper thread (Mozilla's list, the downloads: Provider::prepare; nothing
//! downloaded when downloads are off), then loaded on the translator's thread; the line waits for them (HOLD_MAX).
//! At most MAX_PAIRS pairs keep models: a new one lets go of the least recently used. Each pair has a state (ready,
//! downloading N %, loading, unavailable, error) - told with the target on each change through Event::Status (a
//! download's progress at most every STATUS_EVERY).
//!
//! Exactly one Event::Reply per id: "" when nothing in the line was translated (or the translation is the line
//! itself). Ids are the game's, unique in its session: an id already queued or answered is ignored, and a new session
//! (new_session) forgets them and drops what is still queued from the old one.
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

/// the most pairs whose models are kept (each ~20-55 MB in memory, a pair without English two): a new language past
/// these lets go of the least recently used pair
pub const MAX_PAIRS: usize = 4;
/// a download's progress is told at most this often
pub const STATUS_EVERY: Duration = Duration::from_millis(500);
/// a pair whose models failed (no internet, a broken file) is tried again after this
pub const RETRY_AFTER: Duration = Duration::from_secs(60);
/// the longest a line waits for its pairs' models (a first download on a slow connection)
pub const HOLD_MAX: Duration = Duration::from_secs(120);
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

/// What getting a pair's models ready came to.
#[derive(Clone, Debug, PartialEq)]
pub enum Prepared {
    /// [(the direction's id - "ja-en/2.1", its folder)], in the order a line goes through them
    Ready(Vec<(String, PathBuf)>),
    /// no model for it (why)
    Unavailable(String),
    /// its models are not on this PC and downloads are off (why)
    NotDownloaded(String),
    /// it failed (why): tried again after RETRY_AFTER
    Error(String),
}

/// Where a pair's models come from (Mozilla; tests: fakes).
pub trait Provider: Send + Sync {
    /// The directions the pair from -> to (Koetama's codes) needs, their files there - downloaded when missing if
    /// `download` (progress(bytes done, bytes total)), else NotDownloaded. Blocking - a helper thread runs it.
    fn prepare(&self, from: &str, to: &str, download: bool, progress: &dyn Fn(u64, u64)) -> Prepared;
    /// One direction's model from its folder (blocking).
    fn load(&self, id: &str, dir: &Path) -> Result<Arc<dyn Engine>, String>;
}

/// Mozilla's models (catalog.rs) in Koetama's translation folder, run by kd_translate::Model.
pub struct Mozilla {
    root: PathBuf,
    log: Log,
    /// the list, and when it was read
    list: Mutex<Option<(Instant, Arc<Catalog>)>>,
    /// one pair's downloads at a time (two pairs may share a direction)
    busy: Mutex<()>,
}

impl Mozilla {
    pub fn new(root: PathBuf, log: Log) -> Mozilla {
        Mozilla { root, log, list: Mutex::new(None), busy: Mutex::new(()) }
    }

    /// The list: fetched again once a day when downloads are on; else the one kept on this PC, however old.
    fn catalog(&self, download: bool) -> Result<Arc<Catalog>, String> {
        let mut list = lock(&self.list);
        if let Some((t, c)) = list.as_ref() {
            if !download || t.elapsed() < catalog::LIST_MAX_AGE {
                return Ok(c.clone());
            }
        }
        let c = if download {
            catalog::load_list(&self.root, catalog::LIST_MAX_AGE, &catalog::get_list, &*self.log)?
        } else {
            let offline = |_: &str| -> std::io::Result<String> { Err(std::io::Error::other("downloads are off")) };
            catalog::load_list(&self.root, Duration::MAX, &offline, &|_| {})?
        };
        let c = Arc::new(c);
        *list = Some((Instant::now(), c.clone()));
        Ok(c)
    }
}

impl Provider for Mozilla {
    fn prepare(&self, from: &str, to: &str, download: bool, progress: &dyn Fn(u64, u64)) -> Prepared {
        let _one = lock(&self.busy);
        let cat = match self.catalog(download) {
            Ok(c) => c,
            Err(_) if !download => return Prepared::NotDownloaded("no list of Mozilla's models on this PC".into()),
            Err(e) => return Prepared::Error(e),
        };
        let dirs = match cat.route(from, to) {
            Ok(d) => d,
            Err(why) => return Prepared::Unavailable(why),
        };
        if !download && !dirs.iter().all(|d| d.present(&self.root)) {
            let keys: Vec<String> = dirs.iter().map(|d| d.key()).collect();
            return Prepared::NotDownloaded(format!("{from} > {to}: the {} model is not on this PC", keys.join(" + ")));
        }
        match catalog::fetch_route(&self.root, &dirs, progress, &catalog::download_file, &*self.log) {
            Ok(paths) => Prepared::Ready(dirs.iter().map(|d| d.id()).zip(paths).collect()),
            Err(e) => Prepared::Error(e),
        }
    }

    fn load(&self, _id: &str, dir: &Path) -> Result<Arc<dyn Engine>, String> {
        crate::Model::load(dir).map(|m| Arc::new(Loaded(Mutex::new(m))) as Arc<dyn Engine>)
    }
}

/// A pair's state.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Ready,
    /// 0..1 of the pair's files
    Downloading(f64),
    Loading,
    /// Mozilla has no model for it (or it cannot be one: a language Koetama does not have, Cantonese as the target)
    Unavailable,
    /// its models are not on this PC and downloads are off (the game reads "unavailable")
    NotDownloaded,
    Error,
}

impl State {
    /// As the game reads it: "ready", "downloading 42", "loading", "unavailable", "error".
    pub fn wire(&self) -> String {
        match self {
            State::Downloading(f) => format!("downloading {}", percent(*f)),
            s => s.word().into(),
        }
    }

    /// The state's name without the progress ("downloading"), as the game reads it.
    pub fn word(&self) -> &'static str {
        match self {
            State::Ready => "ready",
            State::Downloading(_) => "downloading",
            State::Loading => "loading",
            State::Unavailable | State::NotDownloaded => "unavailable",
            State::Error => "error",
        }
    }

    /// Its models are on their way (a line in its language waits for them).
    pub fn getting_ready(&self) -> bool {
        matches!(self, State::Downloading(_) | State::Loading)
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

/// One pair in use and its state.
#[derive(Clone, Debug, PartialEq)]
pub struct PairStatus {
    pub from: String,
    pub to: String,
    pub state: State,
}

impl PairStatus {
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

/// The pairs' states in one line (message kind 'd', the status line): each pair's wire(), comma-separated ("" for
/// none).
pub fn status_text(pairs: &[PairStatus]) -> String {
    pairs.iter().map(PairStatus::wire).collect::<Vec<_>>().join(",")
}

/// The translation as the game and the window see it: the target and the pairs in use this session.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// the language chat is translated into; "": off
    pub into: String,
    pub pairs: Vec<PairStatus>,
}

impl Status {
    /// "en:ja>en=ready,ko>en=downloading 42" ("" off): what tells a change
    pub fn wire(&self) -> String {
        if self.into.is_empty() && self.pairs.is_empty() {
            String::new()
        } else {
            format!("{}:{}", self.into, status_text(&self.pairs))
        }
    }
}

/// What the translator tells the program.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// the reply to request `id` ("" = nothing to show), and the pair used (the first, for a line with stretches in
    /// two languages)
    Reply { id: i64, text: String, rule: Option<(String, String)> },
    /// the target or a pair's state changed
    Status(Status),
}

pub type OnEvent = Arc<dyn Fn(Event) + Send + Sync>;

/// The player's setting.
#[derive(Clone, Debug, Default, PartialEq)]
struct Target {
    into: Option<String>,
    known: Vec<String>,
    downloads: bool,
}

enum Msg {
    Target(Target),
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
    /// the setting as last handed over (set_target is called a few times a second: only a change goes to the thread)
    target: Mutex<Target>,
    status: Mutex<Status>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

/// The translator: a handle (clones share it); its thread runs until stop() (or the last handle is dropped).
#[derive(Clone)]
pub struct Translator {
    inner: Arc<Inner>,
}

impl Translator {
    /// Starts the thread, translation off until set_target. on_event: each reply and status change (called on the
    /// translator's threads: it must not wait on them).
    pub fn start(provider: Arc<dyn Provider>, on_event: OnEvent, log: Log) -> Translator {
        let (tx, rx) = mpsc::channel();
        let inner = Arc::new(Inner {
            tx: Mutex::new(tx.clone()),
            seen: Mutex::new(Seen::default()),
            session: AtomicU64::new(0),
            target: Mutex::new(Target::default()),
            status: Mutex::new(Status::default()),
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
                    target: Target::default(),
                    slots: Vec::new(),
                    models: HashMap::new(),
                    next_job: 0,
                    clock: 0,
                    told: Status::default(),
                    told_at: None,
                    last_error: String::new(),
                    held: VecDeque::new(),
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

    /// The player's setting: the language chat is translated into (None or "": off), the languages the player speaks
    /// (stretches in them are left alone), whether missing models may be downloaded. Another target lets go of every
    /// pair; a language now spoken lets go of its pair. Cheap when nothing changed (call it as often as you like).
    pub fn set_target(&self, into: Option<String>, known: Vec<String>, allow_downloads: bool) {
        let mut known = known;
        known.sort();
        known.dedup();
        let t = Target { into: into.filter(|s| !s.is_empty()), known, downloads: allow_downloads };
        let mut kept = lock(&self.inner.target);
        if *kept != t {
            kept.clone_from(&t);
            self.send(Msg::Target(t));
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

    /// The target and the pairs' states as last told.
    pub fn status(&self) -> Status {
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

/// A pair on the thread: a language the player does not speak, into the target.
struct Slot {
    from: String,
    to: String,
    state: State,
    /// the helper getting its models ready (its messages carry this; a dropped pair's are ignored)
    job: u64,
    /// the direction ids it goes through, once ready
    route: Vec<String>,
    failed_at: Option<Instant>,
    /// when it was last needed (the worker's clock): the least recently used goes first
    used: u64,
}

impl Slot {
    fn status(&self) -> PairStatus {
        PairStatus { from: self.from.clone(), to: self.to.clone(), state: self.state.clone() }
    }

}

/// A line waiting for its pairs' models.
struct Held {
    session: u64,
    id: i64,
    text: String,
    since: Instant,
    /// the languages whose pairs it needs
    langs: Vec<&'static str>,
}

struct Worker {
    provider: Arc<dyn Provider>,
    on_event: OnEvent,
    log: Log,
    tx: Sender<Msg>,
    inner: std::sync::Weak<Inner>,
    target: Target,
    /// the pairs in use, in the order they came
    slots: Vec<Slot>,
    /// direction id -> its model (shared by the pairs through it)
    models: HashMap<String, Arc<dyn Engine>>,
    next_job: u64,
    /// counts each use of a pair (Slot::used)
    clock: u64,
    /// the status as last told, and when
    told: Status,
    told_at: Option<Instant>,
    /// the last translation error logged (each new one once)
    last_error: String,
    /// the lines in the order they came: answered once their pairs are ready (or after HOLD_MAX)
    held: VecDeque<Held>,
}

/// Chinese and Cantonese are one for the player (one script; detect.rs tells them apart by a few characters only).
fn same_language(a: &str, b: &str) -> bool {
    a == b || matches!((a, b), ("zh", "yue") | ("yue", "zh"))
}

/// A pair Koetama can never have a model for (no need to ask Mozilla's list).
fn never(from: &str, to: &str) -> bool {
    from == to || catalog::mozilla_code(from, true).is_none() || catalog::mozilla_code(to, false).is_none()
}

impl Worker {
    fn run(&mut self, rx: mpsc::Receiver<Msg>) {
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(Msg::Target(t)) => self.set_target(t),
                Ok(Msg::Request { session, id, text }) => self.arrived(session, id, text),
                Ok(Msg::Progress { job, frac }) => {
                    if let Some(s) = self.slots.iter_mut().find(|s| s.job == job && s.state.getting_ready()) {
                        s.state = State::Downloading(frac);
                    }
                }
                Ok(Msg::Prepared { job, result }) => self.prepared(job, result),
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.retry_failed();
            self.tell();
            if !self.answer_held() {
                break;
            }
        }
    }

    /// A stretch in this language is left as it is: translation off, the target, or a language the player speaks.
    fn left_alone(&self, lang: &str) -> bool {
        match &self.target.into {
            None => true,
            Some(into) => same_language(lang, into) || self.target.known.iter().any(|k| same_language(k, lang)),
        }
    }

    /// The languages a line is most likely in: the player's own and the target - and English when none of those is
    /// written in Latin letters ("ok", "lol" in a Japanese speaker's chat: English, not a language picked at random
    /// from three letters).
    fn likely(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.target.known.iter().map(String::as_str).collect();
        out.extend(self.target.into.as_deref());
        if !out.iter().any(|l| detect::latin(l)) {
            out.push("en");
        }
        out
    }

    /// The line's stretches with the languages to translate (those not left alone).
    fn stretches(&self, line: &str) -> Vec<(detect::Stretch, bool)> {
        detect::stretches_preferring(line, &self.likely())
            .into_iter()
            .map(|st| {
                let wanted = st.lang.is_some_and(|l| !self.left_alone(l)) && !st.text(line).trim().is_empty();
                (st, wanted)
            })
            .collect()
    }

    fn set_target(&mut self, t: Target) {
        let old = std::mem::replace(&mut self.target, t);
        if old.into != self.target.into {
            if !self.slots.is_empty() {
                (self.log)("translation: another target language - every pair let go");
            }
            self.slots.clear();
        } else {
            // (a language the player speaks now: its pair goes)
            let gone: Vec<usize> = (0..self.slots.len()).filter(|&i| self.left_alone(&self.slots[i].from)).collect();
            for i in gone.into_iter().rev() {
                self.slots.remove(i);
            }
            if self.target.downloads && !old.downloads {
                // (downloads allowed now: the pairs that lacked models are made again the next time they are needed)
                self.slots.retain(|s| s.state != State::NotDownloaded);
            }
        }
        match &self.target.into {
            Some(into) => (self.log)(&format!(
                "translation: into {into}, leaving alone {} (downloads {})",
                if self.target.known.is_empty() { "nothing else".to_string() } else { self.target.known.join(", ") },
                if self.target.downloads { "on" } else { "off" }
            )),
            None if old.into.is_some() => (self.log)("translation: off"),
            None => {}
        }
        self.unload_unused();
    }

    /// A line came: the pairs its languages need are made (their models start to come) and it waits its turn.
    fn arrived(&mut self, session: u64, id: i64, text: String) {
        let mut langs: Vec<&'static str> = Vec::new();
        for (st, wanted) in self.stretches(&text) {
            if let (true, Some(l)) = (wanted, st.lang) {
                if !langs.contains(&l) {
                    langs.push(l);
                }
            }
        }
        for l in &langs {
            self.pair_for(l);
        }
        self.held.push_back(Held { session, id, text, since: Instant::now(), langs });
    }

    /// The pair from `lang` into the target, made when it is not there yet (its models fetched). Marked used.
    fn pair_for(&mut self, lang: &str) {
        let Some(into) = self.target.into.clone() else { return };
        self.clock += 1;
        if let Some(s) = self.slots.iter_mut().find(|s| s.from == lang) {
            s.used = self.clock;
            return;
        }
        let unavailable = never(lang, &into);
        self.slots.push(Slot {
            from: lang.to_string(),
            to: into.clone(),
            state: if unavailable { State::Unavailable } else { State::Loading },
            job: 0,
            route: Vec::new(),
            failed_at: None,
            used: self.clock,
        });
        if unavailable {
            (self.log)(&format!("translation: {lang} > {into} is not available (no model for it)"));
        } else {
            let i = self.slots.len() - 1;
            self.fetch(i);
        }
    }

    /// The held lines answered, in order, unless the front one's pairs are still getting their models (a line held
    /// HOLD_MAX is answered anyway). False: the translator is gone.
    fn answer_held(&mut self) -> bool {
        while let Some(h) = self.held.front() {
            let waiting = h.langs.iter().any(|l| self.slots.iter().any(|s| s.from == *l && s.state.getting_ready()));
            if waiting && h.since.elapsed() < HOLD_MAX {
                break;
            }
            let Some(inner) = self.inner.upgrade() else {
                return false;
            };
            let Some(h) = self.held.pop_front() else { break };
            // (a line from a session gone: its id may be a new line's now - never answered)
            if h.session == inner.session.load(Ordering::SeqCst) {
                let (text, rule) = self.translate_line(&h.text);
                // (the session may have ended while it was translated)
                if h.session == inner.session.load(Ordering::SeqCst) {
                    (self.on_event)(Event::Reply { id: h.id, text, rule });
                }
            }
        }
        true
    }

    /// Starts a helper getting a pair's models ready.
    fn fetch(&mut self, i: usize) {
        self.next_job += 1;
        let job = self.next_job;
        let download = self.target.downloads;
        let s = &mut self.slots[i];
        s.job = job;
        // (loading until bytes actually come down: models already here never show "downloading")
        s.state = State::Loading;
        s.failed_at = None;
        let (provider, tx, from, to) = (self.provider.clone(), self.tx.clone(), s.from.clone(), s.to.clone());
        let tx2 = tx.clone();
        let spawned = std::thread::Builder::new().name("translation models".into()).spawn(move || {
            let progress = move |done: u64, total: u64| {
                let frac = if total > 0 { done as f64 / total as f64 } else { 0.0 };
                let _ = tx2.send(Msg::Progress { job, frac });
            };
            let result = provider.prepare(&from, &to, download, &progress);
            let _ = tx.send(Msg::Prepared { job, result });
        });
        if let Err(e) = spawned {
            self.slots[i].state = State::Error;
            self.slots[i].failed_at = Some(Instant::now());
            (self.log)(&format!("translation: cannot start a download: {e}"));
        }
    }

    /// Lets go of the models no pair goes through.
    fn unload_unused(&mut self) {
        let used: HashSet<&String> = self.slots.iter().flat_map(|s| s.route.iter()).collect();
        let gone: Vec<String> = self.models.keys().filter(|k| !used.contains(k)).cloned().collect();
        for k in gone {
            self.models.remove(&k);
            (self.log)(&format!("translation: let go of the {k} model"));
        }
    }

    /// Before a pair's models load: the least recently used ready pairs (not job's) let go of until fewer than
    /// MAX_PAIRS are left (a pair without models - unavailable, failed, still coming - counts for nothing).
    fn make_room(&mut self, job: u64) {
        loop {
            let ready: Vec<usize> =
                (0..self.slots.len()).filter(|&i| self.slots[i].state == State::Ready && self.slots[i].job != job).collect();
            if ready.len() < MAX_PAIRS {
                break;
            }
            let Some(&i) = ready.iter().min_by_key(|&&i| self.slots[i].used) else { break };
            let s = self.slots.remove(i);
            (self.log)(&format!("translation: let go of {} > {} (the least recently used)", s.from, s.to));
        }
        self.unload_unused();
    }

    fn prepared(&mut self, job: u64, result: Prepared) {
        if !self.slots.iter().any(|s| s.job == job) {
            return; // (the pair is gone)
        }
        if matches!(result, Prepared::Ready(_)) {
            self.make_room(job);
        }
        let Some(i) = self.slots.iter().position(|s| s.job == job) else { return };
        let pair = format!("{} > {}", self.slots[i].from, self.slots[i].to);
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
                                (self.log)(&format!("translation: {pair}: the {id} model could not be loaded: {e}"));
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
                (self.log)(&format!("translation: {pair} ready"));
                self.unload_unused();
            }
            Prepared::Unavailable(why) => {
                self.slots[i].state = State::Unavailable;
                (self.log)(&format!("translation: not available: {why}"));
            }
            Prepared::NotDownloaded(why) => {
                self.slots[i].state = State::NotDownloaded;
                (self.log)(&format!("translation: {why}, and downloads are off"));
            }
            Prepared::Error(e) => {
                self.slots[i].state = State::Error;
                self.slots[i].failed_at = Some(Instant::now());
                (self.log)(&format!("translation: {pair}: {e} (tried again in {} s)", RETRY_AFTER.as_secs()));
            }
        }
    }

    fn retry_failed(&mut self) {
        for i in 0..self.slots.len() {
            if self.slots[i].state == State::Error && self.slots[i].failed_at.is_some_and(|t| t.elapsed() >= RETRY_AFTER)
            {
                self.fetch(i);
            }
        }
    }

    /// Tells the status when it changed (a download's progress alone: at most every STATUS_EVERY).
    fn tell(&mut self) {
        let now = Status {
            into: self.target.into.clone().unwrap_or_default(),
            pairs: self.slots.iter().map(Slot::status).collect(),
        };
        // (compared whole, not on the wire: NotDownloaded and Unavailable read the same there, the window tells them apart)
        if now == self.told {
            return;
        }
        let shape = |s: &Status| {
            let pairs: Vec<_> =
                s.pairs.iter().map(|p| (p.from.clone(), p.to.clone(), std::mem::discriminant(&p.state))).collect();
            (s.into.clone(), pairs)
        };
        let progress_only = shape(&now) == shape(&self.told);
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

    /// One piece through a pair's model(s).
    fn through(&self, s: &Slot, text: &str) -> Result<String, String> {
        let mut t = text.to_string();
        for id in &s.route {
            let m = self.models.get(id).ok_or(format!("the {id} model is not loaded"))?;
            t = m.translate(&t)?;
        }
        Ok(t)
    }

    /// A line's translation ("" when nothing in it was translated), and the pair used (the first, for a line with
    /// stretches in two languages).
    fn translate_line(&mut self, line: &str) -> (String, Option<(String, String)>) {
        if self.target.into.is_none() {
            return (String::new(), None);
        }
        let mut rule: Option<(String, String)> = None;
        // (piece, translated)
        let mut pieces: Vec<(String, bool)> = Vec::new();
        let mut used: Vec<usize> = Vec::new();
        for (st, wanted) in self.stretches(line) {
            let text = st.text(line);
            let slot = st
                .lang
                .filter(|_| wanted)
                .and_then(|l| self.slots.iter().position(|s| s.from == l && s.state == State::Ready));
            let done = match slot {
                Some(i) => match self.through(&self.slots[i], text.trim()) {
                    Ok(t) => {
                        let s = &self.slots[i];
                        rule.get_or_insert_with(|| (s.from.clone(), s.to.clone()));
                        used.push(i);
                        Some(t)
                    }
                    Err(e) => {
                        let msg = format!("translation: {} > {}: {e}", self.slots[i].from, self.slots[i].to);
                        if msg != self.last_error {
                            (self.log)(&msg);
                            self.last_error = msg;
                        }
                        None
                    }
                },
                None => None,
            };
            match done {
                Some(t) => pieces.push((t.trim().to_string(), true)),
                None => pieces.push((text.to_string(), false)),
            }
        }
        for i in used {
            self.clock += 1;
            self.slots[i].used = self.clock;
        }
        if !pieces.iter().any(|p| p.1) {
            return (String::new(), None);
        }
        let out = join_pieces(&pieces);
        if out == line.trim() {
            (String::new(), None) // (nothing changed: nothing to show)
        } else {
            (out, rule)
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
        let r = |f: &str, t: &str, s: State| PairStatus { from: f.into(), to: t.into(), state: s };
        assert_eq!(
            status_text(&[r("ja", "en", State::Ready), r("ko", "en", State::Downloading(0.429))]),
            "ja>en=ready,ko>en=downloading 42"
        );
        assert_eq!(
            status_text(&[r("mt", "en", State::Unavailable), r("ja", "ko", State::Loading)]),
            "mt>en=unavailable,ja>ko=loading"
        );
        assert_eq!(status_text(&[r("de", "en", State::Error)]), "de>en=error");
        assert_eq!(status_text(&[r("de", "en", State::NotDownloaded)]), "de>en=unavailable", "the game reads unavailable");
        assert_eq!(status_text(&[]), "");
        let all =
            [r("ja", "en", State::Ready), r("ko", "en", State::Downloading(0.429)), r("mt", "en", State::Unavailable)];
        let common: Vec<_> = all.iter().map(PairStatus::common).collect();
        assert_eq!(kd_common::feed::translations_wire(&common), status_text(&all), "the connectors write the same");
        assert_eq!((percent(1.0), percent(0.999), percent(-1.0), percent(f64::NAN)), (100, 99, 0, 0));
        assert_eq!(Status::default().wire(), "");
        assert_eq!(Status { into: "en".into(), pairs: vec![] }.wire(), "en:");
        assert_eq!(Status { into: "en".into(), pairs: all[..1].to_vec() }.wire(), "en:ja>en=ready");
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

    #[test]
    fn languages() {
        assert!(same_language("zh", "yue") && same_language("ja", "ja") && !same_language("ja", "ko"));
        assert!(never("ja", "ja") && never("en", "yue") && never("xx", "en") && !never("yue", "en"));
    }
}
