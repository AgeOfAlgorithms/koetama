//! The FILES connector (PROTOCOL.md "Transport: files"): for a game whose mod can write a file the game saves and
//! read files next to itself (Teardown). Parameterised by a profile (profile::FilesConfig):
//!   game -> Koetama   a file the game writes (Teardown: savegame.xml), polled: a regex finds each copy of the mod's
//!                      feed string (the feed object, or its hex: api::feed_from_text), a second one the tag of the copy
//!   Koetama -> game   small files in the folder the mod looks in: <prefix>on (running), <prefix>p<n> (the answer to
//!                      ping n), <prefix>t<n>.<ext> (object n of the session - 1 is the hello; json, or a Teardown prefab
//!                      holding the object's hex: api::object_prefab)
//! SAFETY: it writes only into folders that exist (never creates one), and deletes only files whose names are exactly
//! its own patterns (Link::owns) - the prefix must be 3+ letters, digits or _ ending in _ (profile::safe_prefix).
use crate::profile::{self, FilesConfig, MessageFormat, Profile};
use crate::{intern, voices, Game};
use kd_common::feed::{self, Feed, FeedSink};
use kd_common::{text, Log};
use regex::bytes::Regex;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const TEXT_MAX: usize = 400; // characters of one text file
/// characters of one translation (kind 'x': a 400-byte line can come back longer)
pub const TRANSLATION_MAX: usize = 1000;

/// Teardown's file prefix (the files connector's default)
pub const PREFIX: &str = profile::DEFAULT_PREFIX;

static FEED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(crate::teardown::FEED).unwrap());
static MODTAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(crate::teardown::MODTAG).unwrap());

// ---------------------------------------------------------------- Python's space rules
/// Python's str.isspace (for strip() and int()/float()): Unicode white space and the ASCII separators 0x1c..0x1f.
fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

pub(crate) fn py_strip(s: &str) -> &str {
    s.trim_matches(py_isspace)
}

// ---------------------------------------------------------------- the feed (game -> Koetama)
/// A feed string from the game's file (the feed object, or its hex) -> a Feed, or None (api::feed_from_text).
pub fn parse_feed(text: &str) -> Option<Feed> {
    crate::api::feed_from_text(text).ok()
}

/// Bytes as Python's .decode('ascii', 'replace'): each byte past ASCII is U+FFFD.
fn ascii_replace(b: &[u8]) -> String {
    b.iter().map(|&c| if c < 0x80 { c as char } else { '\u{FFFD}' }).collect()
}

/// How the feed is found in the game's file (a profile's feed.pattern, feed.tag_pattern, feed.complete).
#[derive(Clone, Debug)]
pub struct FeedRules {
    /// one group: the feed string
    pub feed: Regex,
    /// one group: the tag of the mod copy that wrote the feed after it (None: no tags)
    pub tag: Option<Regex>,
    /// the file is read only when it ends with this, white space aside (empty: always)
    pub complete: Vec<u8>,
}

impl Default for FeedRules {
    /// Teardown's: savegame.xml
    fn default() -> FeedRules {
        FeedRules { feed: FEED_RE.clone(), tag: Some(MODTAG_RE.clone()), complete: profile::DEFAULT_COMPLETE.into() }
    }
}

impl FeedRules {
    pub fn from_config(c: &FilesConfig) -> FeedRules {
        FeedRules { feed: c.feed_re.clone(), tag: c.tag_re.clone(), complete: c.complete.as_bytes().to_vec() }
    }

    /// [(the mod's tag, the feed string)] for every feed in the file
    pub fn find(&self, data: &[u8]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for c in self.feed.captures_iter(data) {
            let start = c.get(0).map_or(0, |m| m.start());
            let tag = self
                .tag
                .as_ref()
                .and_then(|re| re.captures_iter(&data[..start]).last())
                .and_then(|t| t.get(1))
                .map(|t| ascii_replace(t.as_bytes()));
            out.push((tag.unwrap_or_default(), c.get(1).map(|m| String::from_utf8_lossy(m.as_bytes()).into_owned()).unwrap_or_default()));
        }
        out
    }
}

/// [(the mod's tag, the feed string)] for every copy of the mod in a savegame.xml: 'local-proximity-chat'
/// (the mods folder) and 'steam-<id>' (the Workshop) each have their own
pub fn find_feeds(data: &[u8]) -> Vec<(String, String)> {
    FeedRules::default().find(data)
}

/// The file's bytes, opened so that the game can write, replace or delete it meanwhile; None if not there.
pub fn read_shared(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut opts = fs::OpenOptions::new();
    opts.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
        opts.share_mode(0x1 | 0x2 | 0x4);
    }
    let mut f = opts.open(path).ok()?;
    let mut data = Vec::new();
    f.read_to_end(&mut data).ok()?;
    Some(data)
}

/// Python's bytes.rstrip(): no ASCII white space at the end (space, \t, \n, \r, \x0b, \x0c).
fn rstrip_bytes(b: &[u8]) -> &[u8] {
    let n = b.iter().rposition(|c| !matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)).map_or(0, |i| i + 1);
    &b[..n]
}

/// What the reader knows between looks at the game's file: each copy of the mod's last string.
#[derive(Debug, Default)]
pub struct FeedScan {
    /// the mod's tag -> its last string
    pub last: HashMap<String, String>,
    /// live feeds handed on
    pub updates: u64,
    /// how the feed is found (default: Teardown's)
    pub rules: FeedRules,
    /// a feed that does not read: (its text, since when, why) - told once it has stayed so for BAD_FOR (a
    /// half-written file is only briefly "not JSON")
    bad: Option<(String, Instant, String)>,
    /// why the game's feed cannot be read, once it is sure (None: it reads)
    pub problem: Option<String>,
}

/// how long a feed must stay unreadable before it is a problem worth telling
const BAD_FOR: Duration = Duration::from_millis(500);

impl FeedScan {
    pub fn with_rules(rules: FeedRules) -> FeedScan {
        FeedScan { rules, ..FeedScan::default() }
    }

    /// One look at the file: each feed that changed (and parses) goes to on_feed(feed, tag). A half-written file
    /// (Teardown: no </registry> at its end) is skipped.
    pub fn once(&mut self, path: &Path, on_feed: &mut dyn FnMut(Feed, &str)) {
        let Some(data) = read_shared(path) else {
            return;
        };
        if data.is_empty() || !rstrip_bytes(&data).ends_with(&self.rules.complete) {
            return;
        }
        for (tag, text) in self.rules.find(&data) {
            if self.last.get(&tag) != Some(&text) {
                let first = !self.last.contains_key(&tag);
                let feed = crate::api::feed_from_text(&text);
                match &feed {
                    Ok(_) => {
                        self.bad = None;
                        self.problem = None;
                    }
                    Err(why) => self.bad = Some((text.clone(), Instant::now(), why.clone())),
                }
                self.last.insert(tag.clone(), text);
                if let Ok(feed) = feed {
                    if !first {
                        // (what was in the file before we started is not live)
                        self.updates += 1;
                        on_feed(feed, &tag);
                    }
                }
            }
        }
        if let Some((text, since, why)) = &self.bad {
            let still = self.last.values().any(|t| t == text);
            if still && since.elapsed() >= BAD_FOR && self.problem.is_none() {
                self.problem = Some(why.clone());
            }
        }
    }
}

/// Polls the game's file in a thread of its own; a feed that changes is live: on_feed(feed, tag).
pub struct FeedReader {
    running: Arc<AtomicBool>,
    updates: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl FeedReader {
    /// how often the file is read
    pub const POLL: Duration = Duration::from_millis(10);

    /// Starts the thread (Teardown's rules).
    pub fn start(path: PathBuf, poll: Duration, on_feed: impl FnMut(Feed, &str) + Send + 'static) -> FeedReader {
        FeedReader::start_with(path, poll, FeedRules::default(), on_feed)
    }

    /// Starts the thread with these rules.
    pub fn start_with(path: PathBuf, poll: Duration, rules: FeedRules, on_feed: impl FnMut(Feed, &str) + Send + 'static) -> FeedReader {
        FeedReader::start_logged(path, poll, rules, on_feed, kd_common::null_log())
    }

    /// start_with, telling the log when the game's feed cannot be read (once per problem).
    pub fn start_logged(
        path: PathBuf,
        poll: Duration,
        rules: FeedRules,
        mut on_feed: impl FnMut(Feed, &str) + Send + 'static,
        log: Log,
    ) -> FeedReader {
        let running = Arc::new(AtomicBool::new(true));
        let updates = Arc::new(AtomicU64::new(0));
        let (run, ups) = (running.clone(), updates.clone());
        let thread = std::thread::Builder::new()
            .name("game feed".into())
            .spawn(move || {
                let mut scan = FeedScan::with_rules(rules);
                let mut told: Option<String> = None;
                while run.load(Ordering::SeqCst) {
                    scan.once(&path, &mut on_feed);
                    ups.store(scan.updates, Ordering::SeqCst);
                    if scan.problem != told {
                        if let Some(p) = &scan.problem {
                            log(&format!("the game's feed cannot be read: {p}"));
                        }
                        told = scan.problem.clone();
                    }
                    std::thread::sleep(poll);
                }
            })
            .ok();
        FeedReader { running, updates, thread }
    }

    /// live feeds read so far
    pub fn updates(&self) -> u64 {
        self.updates.load(Ordering::SeqCst)
    }

    pub fn running(&self) -> bool {
        self.thread.is_some() && self.running.load(Ordering::SeqCst)
    }

    /// Stops the thread and waits for it (at most one poll and one on_feed).
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for FeedReader {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------- the messages (Koetama -> game)
/// A translation as it is sent: stripped, at most TRANSLATION_MAX characters.
pub(crate) fn cut_translation(text: &str) -> String {
    py_strip(text).chars().take(TRANSLATION_MAX).collect()
}

/// A line as it is sent: stripped, at most TEXT_MAX characters, and its times cut with it (the first n units keep
/// theirs; fewer times than units: none).
pub(crate) fn cut_line(text: &str, times: Option<&[f64]>) -> (String, Option<Vec<f64>>) {
    let text: String = py_strip(text).chars().take(TEXT_MAX).collect();
    let times = times.and_then(|t| {
        let n = text::units(&text).len();
        (t.len() >= n).then(|| t[..n].to_vec())
    });
    (text, times)
}

/// How a Link names and writes its files (a profile's out.prefix, out.message, out.tag_dirs).
#[derive(Clone, Debug)]
pub struct LinkRules {
    /// the start of every file name (profile::safe_prefix)
    pub prefix: String,
    pub message: MessageFormat,
    /// (tag prefix, folder index): the folder a mod copy with that tag looks in (else the first)
    pub tag_dirs: Vec<(String, usize)>,
}

impl Default for LinkRules {
    /// Teardown's: pcvx_, prefabs, the Workshop's copies (steam-<id>) in the second folder
    fn default() -> LinkRules {
        LinkRules { prefix: PREFIX.into(), message: MessageFormat::TeardownPrefab, tag_dirs: vec![("steam-".into(), 1)] }
    }
}

#[derive(Debug)]
struct LinkState {
    /// where the live copy of the mod looks (known from its first feed)
    dir: Option<PathBuf>,
    sid: Option<i64>,
    ping: Option<i64>,
    /// the last text number written
    n: i64,
    /// number -> path, until the game acks it
    pending: BTreeMap<i64, PathBuf>,
    mic: bool,
    lang: String,
    live: bool,
    /// the standing objects as last told (kind -> object: a new session hears them after its hello)
    standing: BTreeMap<&'static str, String>,
}

/// Koetama's files for the game: <prefix>on, the answer to each ping, numbered message files. Shared by the feed's
/// thread (on_feed) and the speech's (send_msg): Send + Sync.
pub struct Link {
    /// a folder per profile entry (None: not on this PC) - tag_dirs index these; and the folders there are. A folder
    /// made after Koetama started (a mod that makes its own on first run) is found later: refresh()
    places: std::sync::RwLock<(Vec<Option<PathBuf>>, Vec<PathBuf>)>,
    rules: LinkRules,
    log: Log,
    state: Mutex<LinkState>,
    /// the hello's features (api::features of the profile; Teardown's: all of them)
    features: Mutex<Vec<&'static str>>,
}

fn remove(path: &Path) {
    let _ = fs::remove_file(path);
}

/// A text file as Python writes it (text mode: "\n" is "\r\n" on Windows).
fn write_text(path: &Path, s: &str) -> std::io::Result<()> {
    if cfg!(windows) {
        fs::write(path, s.replace('\n', "\r\n"))
    } else {
        fs::write(path, s)
    }
}

/// s is head + digits + tail (at least one ASCII digit)
fn numbered(s: &str, head: &str, tail: &str) -> bool {
    s.strip_prefix(head)
        .and_then(|r| r.strip_suffix(tail))
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit()))
}

impl Link {
    /// Teardown's link over these folders (the second: the Workshop's).
    pub fn new(dirs: Vec<PathBuf>, log: Log) -> Link {
        Link::build(dirs.into_iter().map(Some).collect(), LinkRules::default(), log)
    }

    /// A link with these rules; slots: a folder per profile entry (None: not on this PC). An unsafe prefix is refused.
    pub fn with_rules(slots: Vec<Option<PathBuf>>, rules: LinkRules, log: Log) -> Result<Link, String> {
        if !profile::safe_prefix(&rules.prefix) {
            return Err(format!("the file prefix {:?} is not safe (3+ letters, digits or _, ending in _)", rules.prefix));
        }
        Ok(Link::build(slots, rules, log))
    }

    fn build(slots: Vec<Option<PathBuf>>, rules: LinkRules, log: Log) -> Link {
        let dirs = slots.iter().flatten().cloned().collect();
        Link {
            places: std::sync::RwLock::new((slots, dirs)),
            rules,
            log,
            state: Mutex::new(LinkState {
                dir: None,
                sid: None,
                ping: None,
                n: 0,
                pending: BTreeMap::new(),
                mic: false,
                lang: "en".into(),
                live: true,
                standing: BTreeMap::new(),
            }),
            features: Mutex::new(vec!["speech", "voices", "rooms", "translate"]),
        }
    }

    /// The features the hello names (api::features).
    pub fn set_features(&self, features: Vec<&'static str>) {
        *self.features.lock().unwrap_or_else(|e| e.into_inner()) = features;
    }

    fn lock(&self) -> MutexGuard<'_, LinkState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn path(&self, d: &Path, name: &str) -> PathBuf {
        d.join(format!("{}{name}", self.rules.prefix))
    }

    /// Is this file name one of mine - exactly <prefix>on, <prefix>p<n>, <prefix>t<n>.<ext>, <prefix>w<n>.tmp, and an
    /// older Koetama's <prefix>v<n>, <prefix>vc, <prefix>vx (Windows: any case, as its file names). The only files a
    /// link ever deletes.
    pub fn owns(&self, name: &str) -> bool {
        let fold = |s: &str| if cfg!(windows) { s.to_lowercase() } else { s.to_string() };
        let (name, prefix) = (fold(name), fold(&self.rules.prefix));
        let Some(rest) = name.strip_prefix(&prefix) else {
            return false;
        };
        rest == "on"
            || rest == "vc"
            || rest == "vx"
            || numbered(rest, "v", "")
            || numbered(rest, "p", "")
            || numbered(rest, "t", &format!(".{}", self.rules.message.ext()))
            || numbered(rest, "w", ".tmp")
    }

    /// my files in d removed (only those: owns())
    fn sweep(&self, d: &Path) {
        let Ok(rd) = fs::read_dir(d) else {
            return;
        };
        for e in rd.flatten() {
            if e.file_type().is_ok_and(|t| !t.is_dir()) && self.owns(&e.file_name().to_string_lossy()) {
                remove(&e.path());
            }
        }
    }

    /// The folders the game's copies of the mod look in (those on this PC).
    pub fn dirs(&self) -> Vec<PathBuf> {
        self.places.read().unwrap_or_else(|e| e.into_inner()).1.clone()
    }

    /// The folders again (the profile's resolved anew): a folder that is there now and was not is taken, and gets
    /// <prefix>on. Called while a game's feed comes and its folder is not known.
    pub fn refresh(&self, slots: Vec<Option<PathBuf>>) {
        let old = self.dirs();
        let dirs: Vec<PathBuf> = slots.iter().flatten().cloned().collect();
        if dirs == old {
            return;
        }
        for d in dirs.iter().filter(|d| !old.contains(d) && d.is_dir()) {
            let f = self.path(d, "on");
            if let Err(e) = fs::write(&f, "1") {
                (self.log)(&format!("could not write {}: {e}", f.display()));
            }
        }
        *self.places.write().unwrap_or_else(|e| e.into_inner()) = (slots, dirs);
    }

    pub fn rules(&self) -> &LinkRules {
        &self.rules
    }

    /// Where the live copy of the mod looks (known from its first feed).
    pub fn dir(&self) -> Option<PathBuf> {
        self.lock().dir.clone()
    }

    /// The last text number written.
    pub fn n(&self) -> i64 {
        self.lock().n
    }

    pub fn mic(&self) -> bool {
        self.lock().mic
    }

    pub fn lang(&self) -> String {
        self.lock().lang.clone()
    }

    pub fn live(&self) -> bool {
        self.lock().live
    }

    /// Old files of mine swept (an older Koetama's too), <prefix>on written in every folder there is.
    pub fn start(&self) {
        for d in &self.dirs() {
            if d.is_dir() {
                self.sweep(d);
                let f = self.path(d, "on");
                if let Err(e) = fs::write(&f, "1") {
                    (self.log)(&format!("could not write {}: {e}", f.display()));
                }
            }
        }
    }

    /// A standing object (Game::set_standing): written when it changed; a new session gets it after its hello.
    pub fn set_standing(&self, kind: &'static str, object: String) {
        let changed = {
            let mut s = self.lock();
            let changed = s.standing.get(kind) != Some(&object);
            s.standing.insert(kind, object.clone());
            changed
        };
        if changed {
            self.write_obj(object);
        }
    }

    /// Any other object for the game. False if no game is listening.
    pub fn send_object(&self, object: String) -> bool {
        self.write_obj(object)
    }

    /// All my files gone.
    pub fn stop(&self) {
        for d in &self.dirs() {
            if d.is_dir() {
                self.sweep(d);
            }
        }
    }

    /// The folder a copy of the mod looks in, by its tag (tag_dirs; Teardown's Workshop copies: the second folder),
    /// else the first.
    pub fn dir_for(&self, tag: &str) -> Option<PathBuf> {
        let i = self.rules.tag_dirs.iter().find(|(p, _)| tag.starts_with(p.as_str())).map_or(0, |(_, i)| *i);
        let places = self.places.read().unwrap_or_else(|e| e.into_inner());
        places.0.get(i).cloned().flatten().or_else(|| places.0.first().cloned().flatten())
    }

    /// A feed from the game (its copy of the mod: tag): answer its ping, drop what it has read, remember what it wants.
    pub fn on_feed(&self, feed: &Feed, tag: &str) {
        let Some(d) = self.dir_for(tag) else {
            return;
        };
        let mut s = self.lock();
        let new_session = Some(feed.sid) != s.sid || s.dir.as_ref() != Some(&d);
        if new_session {
            // (a new level, or another copy of the mod)
            for path in s.pending.values() {
                remove(path);
            }
            s.pending.clear();
            s.sid = Some(feed.sid);
            s.dir = Some(d.clone());
            s.ping = None;
            s.n = feed.ack;
        }
        if Some(feed.ping) != s.ping {
            // (alive: the answer to this ping, the last one gone)
            let answer = self.path(&d, &format!("p{}", feed.ping.rem_euclid(1000)));
            if let Err(e) = fs::write(&answer, "1") {
                (self.log)(&format!("could not write {}: {e}", answer.display()));
                return;
            }
            if let Some(old) = s.ping {
                if old.rem_euclid(1000) != feed.ping.rem_euclid(1000) {
                    remove(&self.path(&d, &format!("p{}", old.rem_euclid(1000))));
                }
            }
            s.ping = Some(feed.ping);
        }
        let acked: Vec<i64> = s.pending.range(..=feed.ack).map(|(&n, _)| n).collect();
        for n in acked {
            // (read by the game)
            if let Some(p) = s.pending.remove(&n) {
                remove(&p);
            }
        }
        s.mic = feed.mic;
        s.lang = feed.lang.clone();
        s.live = feed.live;
        let standing: Vec<String> = s.standing.values().cloned().collect();
        drop(s);
        if new_session {
            // (the session's first object: what this Koetama does; then the standing ones - the voice chat, the status)
            let features = self.features.lock().unwrap_or_else(|e| e.into_inner()).clone();
            self.write_obj(crate::api::hello(&features));
            for o in standing {
                self.write_obj(o);
            }
        }
    }

    /// Hand a finished line to the game; false if no game is listening.
    pub fn send_text(&self, text: &str) -> bool {
        let text = py_strip(text);
        !text.is_empty() && self.send_msg('f', 0, text, None, None)
    }

    /// Hand what the player said to the game (api::speech): kind 's' (they started talking), 'l' (the live words so
    /// far), 'f' (the finished line; "" = nothing made out) or 'r' (a new voice room, "<room>:<key>"); times: each
    /// unit's start (s after t0, when the line's audio began) - sent with how long ago t0 is now. False if no game is
    /// listening.
    pub fn send_msg(&self, kind: char, utt: u32, text: &str, times: Option<&[f64]>, t0: Option<Instant>) -> bool {
        let (text, times) = cut_line(text, times);
        if kind == 'l' && text.is_empty() {
            return false;
        }
        let ago = t0.filter(|_| times.is_some()).map(|t| t.elapsed().as_secs_f64());
        self.write_obj(crate::api::speech(kind, utt, &text, times.as_deref(), ago))
    }

    /// The translation of line `id` ("" = nothing to show). False if no game is listening.
    pub fn send_translation(&self, id: i64, text: &str, rule: Option<(&str, &str)>) -> bool {
        self.write_obj(crate::api::translation(id, &cut_translation(text), rule))
    }

    /// The translation's state (the target, "" off; the pairs in use). False if no game is listening.
    pub fn send_translations_state(&self, into: &str, states: &[feed::RuleState]) -> bool {
        self.write_obj(crate::api::translations_status(into, states))
    }

    /// Object n of the session (written whole: through <prefix>w<n>.tmp), kept until the game acks it: json (one
    /// line) or a Teardown prefab (api::object_prefab). False if no game is listening.
    fn write_obj(&self, object: String) -> bool {
        let mut s = self.lock();
        let Some(dir) = s.dir.clone() else {
            return false;
        };
        s.n += 1;
        let path = self.path(&dir, &format!("t{}.{}", s.n, self.rules.message.ext()));
        let tmp = self.path(&dir, &format!("w{}.tmp", s.n));
        let written = match self.rules.message {
            MessageFormat::Json => fs::write(&tmp, object + "\n"),
            MessageFormat::TeardownPrefab => write_text(&tmp, &crate::api::object_prefab(&object)),
        };
        // (appears complete, never half-written)
        if let Err(e) = written.and_then(|_| fs::rename(&tmp, &path)) {
            (self.log)(&format!("could not write {}: {e}", path.display()));
            remove(&tmp);
            return false;
        }
        let n = s.n;
        s.pending.insert(n, path);
        true
    }
}

// ---------------------------------------------------------------- the game module
fn env_nonempty(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The file the feed is read from: SAVEPROBE_DIR (tests: a fake game's folder, + the profile's file name), else the
/// first candidate that resolves here (empty when none does).
pub fn feed_file(c: &FilesConfig) -> PathBuf {
    if let Some(d) = env_nonempty("SAVEPROBE_DIR") {
        // (tests: a fake game's folder)
        if let Some(name) = c.feed_file.0.first().and_then(|t| t.file_name()) {
            return d.join(name);
        }
    }
    c.feed_file.resolve_file(&profile::this_pc).unwrap_or_default()
}

/// The folders for the message files, one per profile entry (None: not on this PC); HFP_MODS (tests): the only one.
pub fn out_slots(c: &FilesConfig) -> Vec<Option<PathBuf>> {
    if let Some(d) = env_nonempty("HFP_MODS") {
        // (tests)
        return vec![Some(d)];
    }
    c.dirs.iter().map(|d| d.resolve_dir(&profile::this_pc)).collect()
}

/// A game linked by the files connector (Teardown's module is one): the feed from the game's file (a FeedReader
/// thread), the files for the game (a Link).
pub struct FilesGame {
    profile: Arc<Profile>,
    builtin: bool,
    sink: Arc<dyn FeedSink>,
    log: Log,
    save: PathBuf,
    rules: FeedRules,
    link: Arc<Link>,
    reader: Option<FeedReader>,
    feed: Arc<Mutex<Option<Feed>>>,
    /// the profile's folders are looked for again while the game's is not known (none given by the caller)
    refresh: bool,
}

impl FilesGame {
    /// save: the file to read (None: the profile's); dirs: the only folders for my files (None or empty: the
    /// profile's). profile must use the files connector (else: a game that never connects).
    pub fn with_paths(
        profile: Arc<Profile>,
        builtin: bool,
        sink: Arc<dyn FeedSink>,
        log: Log,
        save: Option<PathBuf>,
        dirs: Option<Vec<PathBuf>>,
    ) -> FilesGame {
        let mut refresh = false;
        let (rules, save, link) = match &profile.connector {
            profile::Connector::Files(c) => {
                let save = save.filter(|s| !s.as_os_str().is_empty()).unwrap_or_else(|| feed_file(c));
                let given = dirs.as_ref().is_some_and(|d| !d.is_empty());
                let slots = match dirs.filter(|d| !d.is_empty()) {
                    Some(d) => d.into_iter().map(Some).collect(),
                    None => out_slots(c),
                };
                refresh = !given;
                let rules = LinkRules { prefix: c.prefix.clone(), message: c.message, tag_dirs: c.tag_dirs.clone() };
                let link = Link::with_rules(slots, rules.clone(), log.clone()).unwrap_or_else(|e| {
                    log(&format!("{}: {e}", profile.id));
                    Link::build(Vec::new(), rules, log.clone())
                });
                link.set_features(crate::api::features(&profile));
                (FeedRules::from_config(c), save, link)
            }
            profile::Connector::Socket(_) | profile::Connector::Http(_) => {
                log(&format!("{}: not a files profile", profile.id));
                (FeedRules::default(), PathBuf::new(), Link::build(Vec::new(), LinkRules::default(), log.clone()))
            }
        };
        FilesGame {
            profile,
            builtin,
            sink,
            log,
            save,
            rules,
            link: Arc::new(link),
            reader: None,
            feed: Arc::new(Mutex::new(None)),
            refresh,
        }
    }

    /// The file read (Teardown: savegame.xml).
    pub fn save(&self) -> &Path {
        &self.save
    }

    /// The files for the game (send_text, its folders, what the game wants).
    pub fn link(&self) -> &Arc<Link> {
        &self.link
    }

    pub fn log(&self) -> &Log {
        &self.log
    }

    pub fn profile(&self) -> &Arc<Profile> {
        &self.profile
    }

    fn stop_reader(&mut self) {
        if let Some(r) = self.reader.as_mut() {
            r.stop();
        }
    }
}

/// The feed as the profile uses it: a speech-only game plays no voices, a voices-only game never gets the microphone.
pub(crate) fn as_used(mut feed: Feed, p: &Profile) -> Feed {
    if !p.speech {
        feed.mic = false;
        feed.ptt = None;
    }
    if !p.voices {
        feed.speakers.clear();
    }
    if !p.translate {
        feed.translate = false;
        feed.to_translate.clear();
    }
    feed
}

impl Game for FilesGame {
    fn id(&self) -> &'static str {
        intern(&self.profile.id)
    }

    fn name(&self) -> &'static str {
        intern(&self.profile.game)
    }

    fn needs(&self) -> &'static str {
        intern(&self.profile.needs)
    }

    fn locate(&self) -> (bool, String) {
        self.profile.locate(Some(&self.save))
    }

    fn start(&mut self) {
        self.stop_reader();
        self.link.start();
        let (sink, link, keep, p) = (self.sink.clone(), self.link.clone(), self.feed.clone(), self.profile.clone());
        let refresh = self.refresh;
        let on_feed = move |feed: Feed, tag: &str| {
            let feed = as_used(feed, &p);
            if refresh && link.dir_for(tag).is_none() {
                // (the mod's folder was not there when Koetama started - a mod that makes its own on first run)
                if let profile::Connector::Files(c) = &p.connector {
                    link.refresh(out_slots(c));
                }
            }
            *keep.lock().unwrap_or_else(|e| e.into_inner()) = Some(feed.clone());
            // (the link first: a new session's files are set up before anything the sink starts - a translation
            // of this feed's lines - can answer into it)
            link.on_feed(&feed, tag);
            sink.set_feed(feed);
        };
        self.reader =
            Some(FeedReader::start_logged(self.save.clone(), FeedReader::POLL, self.rules.clone(), on_feed, self.log.clone()));
    }

    fn set_standing(&self, kind: &'static str, object: String) {
        self.link.set_standing(kind, object);
    }

    fn send_object(&self, object: String) -> bool {
        self.link.send_object(object)
    }

    fn stop(&mut self) {
        self.stop_reader();
        self.link.stop();
    }

    fn send(&self, kind: char, utt: u32, text: &str, times: Option<&[f64]>, t0: Option<Instant>) -> bool {
        self.link.send_msg(kind, utt, text, times, t0)
    }

    fn send_text(&self, text: &str) -> bool {
        self.link.send_text(text)
    }

    fn send_translation(&self, id: i64, text: &str, rule: Option<(&str, &str)>) -> bool {
        self.link.send_translation(id, text, rule)
    }

    fn send_translations_state(&self, into: &str, pairs: &[feed::RuleState]) -> bool {
        self.link.send_translations_state(into, pairs)
    }

    fn test_voices(&self) -> HashMap<i64, PathBuf> {
        voices::for_profile(&self.profile, self.builtin)
    }

    fn speaker_name(&self, src: i64) -> String {
        self.profile.speaker_name(src)
    }

    fn feed(&self) -> Option<Feed> {
        self.feed.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn connected(&self) -> bool {
        self.sink.fresh()
    }

    fn updates(&self) -> u64 {
        self.reader.as_ref().map_or(0, FeedReader::updates)
    }

    fn describe(&self) -> Vec<String> {
        let dirs: Vec<String> = self.link.dirs().iter().map(|d| d.display().to_string()).collect();
        vec![format!("reading {}", self.save.display()), format!("my files for the game go to: {}", dirs.join(", "))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_as_python() {
        assert_eq!(py_strip("\x1ca\u{2028}"), "a");
    }

    #[test]
    fn rstrip_as_python() {
        assert_eq!(rstrip_bytes(b"</registry>\n\x0b \r"), b"</registry>");
        assert_eq!(rstrip_bytes(b" \n"), b"");
    }

    #[test]
    fn numbered_names() {
        assert!(numbered("t12.xml", "t", ".xml") && numbered("p0", "p", "") && numbered("w3.tmp", "w", ".tmp"));
        assert!(!numbered("t.xml", "t", ".xml") && !numbered("t1a.xml", "t", ".xml") && !numbered("p", "p", ""));
        assert!(!numbered("t\u{0661}.xml", "t", ".xml"), "only ASCII digits");
    }
}
