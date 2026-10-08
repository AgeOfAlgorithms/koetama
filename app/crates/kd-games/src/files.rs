//! The FILES connector: Teardown's link (PROTOCOL.md), for any game whose mod can write a file the game saves and
//! read files next to itself. Parameterised by a profile (profile::FilesConfig):
//!   game -> Koetama   a file the game writes (Teardown: savegame.xml), polled: a regex finds each copy of the mod's
//!                      feed string (parse_feed's format), a second one the tag of the copy that wrote it
//!   Koetama -> game   small files in the folder the mod looks in: <prefix>on (running), <prefix>v<n> (it reads feed
//!                      version n: one for each, FEED_VERSIONS), <prefix>p<n> (the answer to ping n), <prefix>t<n>.<ext>
//!                      (message n: what the player said, a translation, the translation rules' states; a Teardown
//!                      prefab or JSON), <prefix>vc / <prefix>vx (the voice chat is in its room / can't reach the relay)
//! SAFETY: it writes only into folders that exist (never creates one), and deletes only files whose names are exactly
//! its own patterns (Link::owns) - the prefix must be 3+ letters, digits or _ ending in _ (profile::safe_prefix).
use crate::profile::{self, FilesConfig, MessageFormat, Profile};
use crate::{intern, voices, Game};
use kd_common::feed::{self, Feed, FeedSink, Speaker};
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
/// the feed versions Koetama reads, each announced by a <prefix>v<n> file next to <prefix>on (a mod sees whether
/// this Koetama knows its feed: older versions are read too; version 5 has the voice room, 6 also translation)
pub const FEED_VERSIONS: [u32; 2] = [5, 6];

/// Teardown's file prefix (the files connector's default)
pub const PREFIX: &str = profile::DEFAULT_PREFIX;

static FEED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(crate::teardown::FEED).unwrap());
static MODTAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(crate::teardown::MODTAG).unwrap());

// ---------------------------------------------------------------- Python's number and space rules
/// Python's str.isspace (for strip() and int()/float()): Unicode white space and the ASCII separators 0x1c..0x1f.
fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

pub(crate) fn py_strip(s: &str) -> &str {
    s.trim_matches(py_isspace)
}

/// The white space Python's int()/float() ignore around a number (unlike str.strip(): not 0x1c..0x1f).
fn num_strip(s: &str) -> &str {
    s.trim_matches(char::is_whitespace)
}

/// Underscores as Python's int()/float() take them: only between two digits (then dropped).
fn py_underscores(s: &str) -> Option<String> {
    if !s.contains('_') {
        return Some(s.to_string());
    }
    let b = s.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c == b'_' && !(i > 0 && b[i - 1].is_ascii_digit() && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            return None;
        }
    }
    Some(s.replace('_', ""))
}

/// Python's int(s) for a base-10 string; None where Python raises ValueError (or the number is beyond i64).
fn py_int(s: &str) -> Option<i64> {
    let s = py_underscores(num_strip(s))?;
    let digits = s.strip_prefix(['+', '-']).unwrap_or(&s);
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Python's float(s); None where Python raises ValueError.
fn py_float(s: &str) -> Option<f64> {
    let s = py_underscores(num_strip(s))?;
    if s.is_empty() || !s.is_ascii() {
        return None;
    }
    s.parse().ok()
}

// ---------------------------------------------------------------- the feed (game -> Koetama)
/// '6|seq|volume|session|ack|ping|mic|lang|live|room|key|me|to|region|translations|requests|id,src,talk,gain,az,el,muffle;...'
/// (version 5: no translations or requests; 4, 3 and 2: no room either) -> a Feed, or None. A bad room, key or id: no room;
/// bad ids in `to`: skipped (feed::voice_room, voice_to); malformed translations and requests: skipped (feed::parse_translations,
/// parse_requests)
pub fn parse_feed(text: &str) -> Option<Feed> {
    let p: Vec<&str> = text.split('|').collect();
    let (seq, vol, sid, ack, ping, mic, lang, live, rest) = match (p[0], p.len()) {
        ("6", 17) => (p[1], p[2], p[3], p[4], p[5], p[6], p[7], p[8], p[16]),
        ("5", 15) => (p[1], p[2], p[3], p[4], p[5], p[6], p[7], p[8], p[14]),
        ("4", 10) => (p[1], p[2], p[3], p[4], p[5], p[6], p[7], p[8], p[9]),
        ("3", 9) => (p[1], p[2], p[3], p[4], p[5], p[6], p[7], "1", p[8]),
        ("2", 8) => (p[1], p[2], p[3], p[4], p[5], p[6], "en", "1", p[7]),
        _ => return None,
    };
    let ((room, key, me), to, region) = if p[0] == "5" || p[0] == "6" {
        let to = if p[12].is_empty() { Vec::new() } else { feed::voice_to(p[12].split(',').map(feed::player_id)) };
        let rkm = feed::voice_room(p[9], p[10], feed::player_id(p[11]));
        let region = feed::voice_region(p[13], &rkm.0);
        (rkm, to, region)
    } else {
        ((String::new(), String::new(), 0), Vec::new(), String::new())
    };
    let (translations, to_translate) =
        if p[0] == "6" { (feed::parse_translations(p[14]), feed::parse_requests(p[15])) } else { (Vec::new(), Vec::new()) };
    let mut speakers = BTreeMap::new();
    for item in rest.split(';') {
        if item.is_empty() {
            continue;
        }
        let f: Vec<&str> = item.split(',').collect();
        let [spid, src, talk, gain, az, el, muffle] = f[..] else {
            return None;
        };
        speakers.insert(
            py_int(spid)?,
            Speaker {
                src: py_int(src)?,
                talk: talk == "1",
                gain: py_float(gain)?,
                az: py_float(az)?,
                el: py_float(el)?,
                muffle: py_float(muffle)?,
            },
        );
    }
    Some(Feed {
        seq: py_int(seq)?,
        vol: py_float(vol)?,
        sid: py_int(sid)?,
        ack: py_int(ack)?,
        ping: py_int(ping)?,
        // (0 off, 1 listen, 2 / 3 push to talk with the key up / held)
        mic: matches!(mic, "1" | "2" | "3"),
        ptt: match mic {
            "2" => Some(false),
            "3" => Some(true),
            _ => None,
        },
        lang: if lang.is_empty() { "en".into() } else { lang.into() },
        live: live != "0",
        speakers,
        room,
        key,
        me,
        to,
        region,
        translations,
        to_translate,
    })
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
            out.push((tag.unwrap_or_default(), c.get(1).map(|m| ascii_replace(m.as_bytes())).unwrap_or_default()));
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
}

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
                let feed = parse_feed(&text);
                self.last.insert(tag.clone(), text);
                if let Some(feed) = feed {
                    if !first {
                        // (what was in the file before we started is not live)
                        self.updates += 1;
                        on_feed(feed, &tag);
                    }
                }
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
    pub fn start_with(
        path: PathBuf,
        poll: Duration,
        rules: FeedRules,
        mut on_feed: impl FnMut(Feed, &str) + Send + 'static,
    ) -> FeedReader {
        let running = Arc::new(AtomicBool::new(true));
        let updates = Arc::new(AtomicU64::new(0));
        let (run, ups) = (running.clone(), updates.clone());
        let thread = std::thread::Builder::new()
            .name("game feed".into())
            .spawn(move || {
                let mut scan = FeedScan::with_rules(rules);
                while run.load(Ordering::SeqCst) {
                    scan.once(&path, &mut on_feed);
                    ups.store(scan.updates, Ordering::SeqCst);
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
/// Python's int(round(x)): halves to even.
fn round_even(x: f64) -> i64 {
    x.round_ties_even() as i64 // (NaN: 0; Python would raise)
}

/// unit start times (s) as the tag w: 4 hex digits each, in 1/100 s (up to 655 s)
pub fn times_hex(times: &[f64]) -> String {
    times.iter().map(|t| format!("{:04x}", round_even(t * 100.0).clamp(0, 0xFFFF))).collect()
}

/// The file the game Spawns to read a message: a body whose tags are its kind ('s' started talking, 'l' the live
/// words so far, 'f' the finished line), the utterance it belongs to, t = the hex of the UTF-8 text, and when there
/// are word times: w = each unit's start (times_hex; s after the line's audio began) and a = how long ago that was,
/// in 1/100 s, when the file was written (the game turns it into a moment on its own clock)
pub fn text_prefab(text: &str, kind: char, utt: u32, times: Option<&[f64]>, ago: Option<f64>) -> String {
    let extra = match (times, ago) {
        (Some(times), Some(ago)) => format!(" w={} a={}", times_hex(times), round_even(ago * 100.0).max(0)),
        _ => String::new(),
    };
    let hex: String = text.bytes().map(|b| format!("{b:02x}")).collect();
    format!("<prefab version=\"1.5.2\">\n\t<body tags=\"pcvx k={kind} u={utt} t={hex}{extra}\"/>\n</prefab>\n")
}

/// A message with an id and a text only (PROTOCOL.md version 6: 'x' the translation of request u, 'd' the rules'
/// states; any id, not only a u32 utterance): Python's text_prefab(text, kind, id).
pub fn id_prefab(text: &str, kind: char, id: i64) -> String {
    let hex: String = text.bytes().map(|b| format!("{b:02x}")).collect();
    format!("<prefab version=\"1.5.2\">\n\t<body tags=\"pcvx k={kind} u={id} t={hex}\"/>\n</prefab>\n")
}

/// A translation as it is sent: stripped, at most TRANSLATION_MAX characters.
pub(crate) fn cut_translation(text: &str) -> String {
    py_strip(text).chars().take(TRANSLATION_MAX).collect()
}

/// Seconds as JSON, to 1/100 s (never NaN or infinite: 0).
pub fn json_secs(x: f64) -> String {
    let r = (x * 100.0).round() / 100.0;
    if r.is_finite() {
        format!("{r}")
    } else {
        "0".into()
    }
}

/// A message as the json format writes it: one line, the same object the socket connector sends (lines::message).
pub fn json_message(text: &str, kind: char, utt: u32, times: Option<&[f64]>, ago: Option<f64>) -> String {
    crate::lines::message(kind, utt, text, times, ago) + "\n"
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
}

/// Koetama's files for the game: <prefix>on, the answer to each ping, numbered message files. Shared by the feed's
/// thread (on_feed) and the speech's (send_msg): Send + Sync.
pub struct Link {
    /// a folder per profile entry (None: not on this PC) - tag_dirs index these
    slots: Vec<Option<PathBuf>>,
    /// the folders there are
    dirs: Vec<PathBuf>,
    rules: LinkRules,
    log: Log,
    state: Mutex<LinkState>,
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
            slots,
            dirs,
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
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, LinkState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn path(&self, d: &Path, name: &str) -> PathBuf {
        d.join(format!("{}{name}", self.rules.prefix))
    }

    /// Is this file name one of mine - exactly <prefix>on, <prefix>p<n>, <prefix>t<n>.<ext>, <prefix>w<n>.tmp (Windows:
    /// any case, as its file names). The only files a link ever deletes.
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
    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
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

    /// Old files of mine swept, <prefix>on and the feed versions (<prefix>v<n>) written in every folder there is.
    pub fn start(&self) {
        for d in &self.dirs {
            if d.is_dir() {
                self.sweep(d);
                let names = std::iter::once("on".to_string()).chain(FEED_VERSIONS.iter().map(|v| format!("v{v}")));
                for name in names {
                    let f = self.path(d, &name);
                    if let Err(e) = fs::write(&f, "1") {
                        (self.log)(&format!("could not write {}: {e}", f.display()));
                    }
                }
            }
        }
    }

    /// The voice chat's state for the game: <prefix>vc while in the room, <prefix>vx while the relay can't be
    /// reached, neither otherwise - in every folder (the mod looks in one; which is known only from a feed).
    pub fn set_voice(&self, state: &str) {
        let want = match state {
            "connected" => Some("vc"),
            "unreachable" => Some("vx"),
            _ => None,
        };
        for d in &self.dirs {
            if !d.is_dir() {
                continue;
            }
            for name in ["vc", "vx"] {
                let f = self.path(d, name);
                if Some(name) == want {
                    if let Err(e) = fs::write(&f, "1") {
                        (self.log)(&format!("could not write {}: {e}", f.display()));
                    }
                } else if f.exists() {
                    remove(&f);
                }
            }
        }
    }

    /// All my files gone.
    pub fn stop(&self) {
        for d in &self.dirs {
            if d.is_dir() {
                self.sweep(d);
            }
        }
    }

    /// The folder a copy of the mod looks in, by its tag (tag_dirs; Teardown's Workshop copies: the second folder),
    /// else the first.
    pub fn dir_for(&self, tag: &str) -> Option<PathBuf> {
        let i = self.rules.tag_dirs.iter().find(|(p, _)| tag.starts_with(p.as_str())).map_or(0, |(_, i)| *i);
        self.slots.get(i).cloned().flatten().or_else(|| self.slots.first().cloned().flatten())
    }

    /// A feed from the game (its copy of the mod: tag): answer its ping, drop what it has read, remember what it wants.
    pub fn on_feed(&self, feed: &Feed, tag: &str) {
        let Some(d) = self.dir_for(tag) else {
            return;
        };
        let mut s = self.lock();
        if Some(feed.sid) != s.sid || s.dir.as_ref() != Some(&d) {
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
    }

    /// Hand a finished line to the game; false if no game is listening.
    pub fn send_text(&self, text: &str) -> bool {
        let text = py_strip(text);
        !text.is_empty() && self.send_msg('f', 0, text, None, None)
    }

    /// Hand a message to the game: kind 's' (the player started talking: no text yet), 'l' (the live words so far),
    /// 'f' (the finished line; "" = nothing made out: the live words go) or 'r' (a new voice room, "<room>:<key>";
    /// PROTOCOL.md version 5); times: each unit's start (s after t0,
    /// when the line's audio began) - written with how long ago t0 is now. False if no game is listening.
    pub fn send_msg(&self, kind: char, utt: u32, text: &str, times: Option<&[f64]>, t0: Option<Instant>) -> bool {
        let (text, times) = cut_line(text, times);
        if kind == 'l' && text.is_empty() {
            return false;
        }
        self.write_msg(kind, utt as i64, &text, times.as_deref(), t0, None)
    }

    /// The translation of request `id` (PROTOCOL.md version 6, kind 'x'; "" = nothing to show). False if no game is
    /// listening.
    pub fn send_translation(&self, id: i64, text: &str) -> bool {
        let text = cut_translation(text);
        self.write_msg('x', id, &text, None, None, Some(crate::lines::translation(id, &text)))
    }

    /// The translations' states (kind 'd': feed::translations_wire; json: lines::translations_status). False if no game
    /// is listening.
    pub fn send_translations_state(&self, states: &[feed::RuleState]) -> bool {
        let json = crate::lines::translations_status(states);
        self.write_msg('d', 0, &feed::translations_wire(states), None, None, Some(json))
    }

    /// One numbered message file (written whole: through <prefix>w<n>.tmp), kept until the game acks it. json: the
    /// json format's object when it is not what the player said (that one is lines::message).
    fn write_msg(
        &self,
        kind: char,
        u: i64,
        text: &str,
        times: Option<&[f64]>,
        t0: Option<Instant>,
        json: Option<String>,
    ) -> bool {
        let mut s = self.lock();
        let Some(dir) = s.dir.clone() else {
            return false;
        };
        s.n += 1;
        let path = self.path(&dir, &format!("t{}.{}", s.n, self.rules.message.ext()));
        let tmp = self.path(&dir, &format!("w{}.tmp", s.n));
        let ago = match (times, t0) {
            (Some(_), Some(t0)) => Some(t0.elapsed().as_secs_f64()),
            _ => None,
        };
        let times = if ago.is_some() { times } else { None };
        // (a speech message's utterance is a u32; a translation's id is the game's: any positive number)
        let written = match (self.rules.message, json, u32::try_from(u)) {
            (MessageFormat::Json, Some(line), _) => fs::write(&tmp, line + "\n"),
            (MessageFormat::Json, None, Ok(utt)) => fs::write(&tmp, json_message(text, kind, utt, times, ago)),
            (MessageFormat::Json, None, Err(_)) => fs::write(&tmp, crate::lines::translation(u, text) + "\n"),
            (MessageFormat::TeardownPrefab, _, Ok(utt)) => write_text(&tmp, &text_prefab(text, kind, utt, times, ago)),
            (MessageFormat::TeardownPrefab, _, Err(_)) => write_text(&tmp, &id_prefab(text, kind, u)),
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
        let (rules, save, link) = match &profile.connector {
            profile::Connector::Files(c) => {
                let save = save.filter(|s| !s.as_os_str().is_empty()).unwrap_or_else(|| feed_file(c));
                let slots = match dirs.filter(|d| !d.is_empty()) {
                    Some(d) => d.into_iter().map(Some).collect(),
                    None => out_slots(c),
                };
                let rules = LinkRules { prefix: c.prefix.clone(), message: c.message, tag_dirs: c.tag_dirs.clone() };
                let link = Link::with_rules(slots, rules.clone(), log.clone()).unwrap_or_else(|e| {
                    log(&format!("{}: {e}", profile.id));
                    Link::build(Vec::new(), rules, log.clone())
                });
                (FeedRules::from_config(c), save, link)
            }
            profile::Connector::Socket(_) => {
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
        feed.translations.clear();
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
        let on_feed = move |feed: Feed, tag: &str| {
            let feed = as_used(feed, &p);
            *keep.lock().unwrap_or_else(|e| e.into_inner()) = Some(feed.clone());
            // (the link first: a new session's files are set up before anything the sink starts - a translation
            // of this feed's lines - can answer into it)
            link.on_feed(&feed, tag);
            sink.set_feed(feed);
        };
        self.reader = Some(FeedReader::start_with(self.save.clone(), FeedReader::POLL, self.rules.clone(), on_feed));
    }

    fn set_voice_state(&self, state: &str) {
        self.link.set_voice(state);
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

    fn send_translation(&self, id: i64, text: &str) -> bool {
        self.link.send_translation(id, text)
    }

    fn send_translations_state(&self, rules: &[feed::RuleState]) -> bool {
        self.link.send_translations_state(rules)
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
    fn numbers_as_python() {
        assert_eq!(py_int(" 0_7 "), Some(7));
        assert_eq!(py_int("+5"), Some(5));
        assert_eq!(py_int("-0"), Some(0));
        assert_eq!(py_int("1__0"), None);
        assert_eq!(py_int("_1"), None);
        assert_eq!(py_int("1.0"), None);
        assert_eq!(py_int(""), None);
        assert_eq!(py_int("\x1c3\t"), None);
        assert_eq!(py_int("\u{3000}3\x0b"), Some(3));
        assert_eq!(py_strip("\x1ca\u{2028}"), "a");
        assert_eq!(py_float("  1_0.5 "), Some(10.5));
        assert!(py_float("-nan").unwrap().is_nan());
        assert!(py_float("NaN").unwrap().is_nan());
        assert_eq!(py_float("INFINITY"), Some(f64::INFINITY));
        assert_eq!(py_float("-inf"), Some(f64::NEG_INFINITY));
        assert_eq!(py_float("1.e5"), Some(1e5));
        assert_eq!(py_float(".5"), Some(0.5));
        assert_eq!(py_float("1e"), None);
        assert_eq!(py_float("."), None);
        assert_eq!(py_float("0x10"), None);
        assert_eq!(py_float("1,5"), None);
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
