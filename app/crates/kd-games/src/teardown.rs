//! Teardown, through the mod Proximity Babble Chat (its voice.lua) - engine/games/teardown.py. The link
//! (PROTOCOL.md has the formats):
//!   game -> Kotodama   savegame.xml: savegame.mod.pcvx.f, ~20 times a second - whom the player hears and how
//!                      (volume, direction, muffle), and what the game wants (the microphone, the language, a ping)
//!   Kotodama -> game   small files next to the mod's folder: pcvx_on (running), pcvx_p<n> (the answer to ping n),
//!                      pcvx_t<n>.xml (message n: what the player said)
//! Teardown runs on Windows; on Linux (Steam Deck) through Proton - its files are then inside its Proton prefix.
use crate::{steam, Game};
use kd_common::feed::{Feed, FeedSink, Speaker};
use kd_common::{paths, text, Log};
use regex::bytes::Regex;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const APPID: u32 = 1167630;
pub const TEXT_MAX: usize = 400; // characters of one text file
pub const FEED: &str = r#"(?-u)<pcvx>\s*<f\s+value="([^"]*)"\s*/>\s*</pcvx>"#;
pub const MODTAG: &str = r#"(?-u)<((?:local|steam)-[^\s/>]+)>"#;

pub const ID: &str = "teardown";
pub const NAME: &str = "Teardown";
pub const NEEDS: &str = "the Proximity Babble Chat mod (Steam Workshop)";

/// the three test voices of the mod's voice dummies (/dummy voice): (Windows voice, speaking rate -10..10, what it says)
pub const VOICES: [(&str, i32, &str); 3] = [
    (
        "Microsoft Zira Desktop",
        -1,
        "I am the whisperer. Stay close, or you will not hear me at all. \
         One, two, three, four, five, six, seven, eight, nine, ten.",
    ),
    (
        "Microsoft David Desktop",
        0,
        "I am the speaker. This is my normal voice, and it carries a fair distance. \
         Monday, Tuesday, Wednesday, Thursday, Friday, Saturday, Sunday.",
    ),
    (
        "Microsoft Zira Desktop",
        2,
        "I am the yeller! You can hear me from far away! \
         Red, orange, yellow, green, blue, purple, black and white!",
    ),
];
pub const NAMES: [(i64, &str); 3] = [(1, "whisperer"), (2, "speaker"), (3, "yeller")];

static FEED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(FEED).unwrap());
static MODTAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(MODTAG).unwrap());

/// Where the test voices are made once: Kotodama's data folder; a developer's build: export/voicehelper in the repo.
pub fn work_dir() -> PathBuf {
    match paths::repo_root() {
        Some(repo) => repo.join("export").join("voicehelper"),
        None => paths::data_dir().join("voices"),
    }
}

// ---------------------------------------------------------------- where Teardown's files are
/// The Windows user folder Teardown sees: this one on Windows (None); inside its Proton prefix on Linux.
pub fn teardown_user() -> Option<PathBuf> {
    if cfg!(windows) {
        return None;
    }
    steam::proton_user(APPID, None)
}

fn env_nonempty(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Teardown's savegame.xml (empty when it cannot be known: Linux without the Proton prefix).
pub fn savegame_path() -> PathBuf {
    if let Some(d) = env_nonempty("SAVEPROBE_DIR") {
        // (tests: a fake game's folder)
        return d.join("savegame.xml");
    }
    if cfg!(windows) {
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default();
        return base.join("Teardown").join("savegame.xml");
    }
    match teardown_user() {
        Some(user) => user.join("AppData").join("Local").join("Teardown").join("savegame.xml"),
        None => PathBuf::new(),
    }
}

/// The folders a running copy of the mod looks in (MOD/../): the local mods folder, then the Workshop's.
pub fn io_dirs() -> Vec<PathBuf> {
    if let Some(d) = env_nonempty("HFP_MODS") {
        // (tests)
        return vec![d];
    }
    let docs = if cfg!(windows) {
        steam::documents_dir()
    } else {
        match teardown_user() {
            Some(user) => user.join("Documents"),
            None => paths::home().join("Documents"),
        }
    };
    let mut dirs = vec![docs.join("Teardown").join("mods")];
    if let Some(ws) = steam::workshop_dir(APPID, None) {
        dirs.push(ws);
    }
    dirs
}

// ---------------------------------------------------------------- Python's number and space rules
/// Python's str.isspace (for strip() and int()/float()): Unicode white space and the ASCII separators 0x1c..0x1f.
fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

fn py_strip(s: &str) -> &str {
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

// ---------------------------------------------------------------- the feed (game -> Kotodama)
/// '4|seq|volume|session|ack|ping|mic|lang|live|id,src,talk,gain,az,el,muffle;...' (versions 2 and 3 too) -> a Feed,
/// or None
pub fn parse_feed(text: &str) -> Option<Feed> {
    let p: Vec<&str> = text.split('|').collect();
    let (seq, vol, sid, ack, ping, mic, lang, live, rest) = match (p[0], p.len()) {
        ("4", 10) => (p[1], p[2], p[3], p[4], p[5], p[6], p[7], p[8], p[9]),
        ("3", 9) => (p[1], p[2], p[3], p[4], p[5], p[6], p[7], "1", p[8]),
        ("2", 8) => (p[1], p[2], p[3], p[4], p[5], p[6], "en", "1", p[7]),
        _ => return None,
    };
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
        mic: mic == "1",
        lang: if lang.is_empty() { "en".into() } else { lang.into() },
        live: live != "0",
        speakers,
    })
}

/// Bytes as Python's .decode('ascii', 'replace'): each byte past ASCII is U+FFFD.
fn ascii_replace(b: &[u8]) -> String {
    b.iter().map(|&c| if c < 0x80 { c as char } else { '\u{FFFD}' }).collect()
}

/// [(the mod's tag, the feed string)] for every copy of the mod in a savegame.xml: 'local-proximity-chat'
/// (the mods folder) and 'steam-<id>' (the Workshop) each have their own
pub fn find_feeds(data: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for c in FEED_RE.captures_iter(data) {
        let start = c.get(0).map_or(0, |m| m.start());
        let tag = MODTAG_RE.captures_iter(&data[..start]).last().and_then(|t| t.get(1)).map(|t| ascii_replace(t.as_bytes()));
        out.push((tag.unwrap_or_default(), ascii_replace(&c[1])));
    }
    out
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

/// What the reader knows between looks at savegame.xml: each copy of the mod's last string.
#[derive(Debug, Default)]
pub struct FeedScan {
    /// the mod's tag -> its last string
    pub last: HashMap<String, String>,
    /// live feeds handed on
    pub updates: u64,
}

impl FeedScan {
    /// One look at the savegame: each feed that changed (and parses) goes to on_feed(feed, tag). A half-written file
    /// (no </registry> at its end) is skipped.
    pub fn once(&mut self, path: &Path, on_feed: &mut dyn FnMut(Feed, &str)) {
        let Some(data) = read_shared(path) else {
            return;
        };
        if data.is_empty() || !rstrip_bytes(&data).ends_with(b"</registry>") {
            return;
        }
        for (tag, text) in find_feeds(&data) {
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

/// Polls savegame.xml in a thread of its own; a feed that changes is live: on_feed(feed, tag).
pub struct FeedReader {
    running: Arc<AtomicBool>,
    updates: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl FeedReader {
    /// how often savegame.xml is read
    pub const POLL: Duration = Duration::from_millis(10);

    /// Starts the thread.
    pub fn start(path: PathBuf, poll: Duration, mut on_feed: impl FnMut(Feed, &str) + Send + 'static) -> FeedReader {
        let running = Arc::new(AtomicBool::new(true));
        let updates = Arc::new(AtomicU64::new(0));
        let (run, ups) = (running.clone(), updates.clone());
        let thread = std::thread::Builder::new()
            .name("teardown feed".into())
            .spawn(move || {
                let mut scan = FeedScan::default();
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

// ---------------------------------------------------------------- the link (Kotodama -> game)
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

/// Kotodama's files for the game: pcvx_on, the answer to each ping, numbered text files. Shared by the feed's thread
/// (on_feed) and the speech's (send_msg): Send + Sync.
pub struct Link {
    dirs: Vec<PathBuf>,
    log: Log,
    state: Mutex<LinkState>,
}

pub const PREFIX: &str = "pcvx_";

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

impl Link {
    pub fn new(dirs: Vec<PathBuf>, log: Log) -> Link {
        Link {
            dirs,
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

    fn path(d: &Path, name: &str) -> PathBuf {
        d.join(format!("{PREFIX}{name}"))
    }

    /// every pcvx_* file in d removed (Windows: any case, as glob there)
    fn sweep(d: &Path) {
        let Ok(rd) = fs::read_dir(d) else {
            return;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let name = if cfg!(windows) { name.to_lowercase() } else { name };
            if name.starts_with(PREFIX) {
                remove(&e.path());
            }
        }
    }

    /// The folders the game's copies of the mod look in.
    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
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

    /// Old files of mine swept, pcvx_on written in every folder there is.
    pub fn start(&self) {
        for d in &self.dirs {
            if d.is_dir() {
                Self::sweep(d);
                if let Err(e) = fs::write(Self::path(d, "on"), "1") {
                    (self.log)(&format!("could not write {}: {e}", Self::path(d, "on").display()));
                }
            }
        }
    }

    /// All my files gone.
    pub fn stop(&self) {
        for d in &self.dirs {
            if d.is_dir() {
                Self::sweep(d);
            }
        }
    }

    /// The folder a copy of the mod looks in, by its tag (the Workshop's: the second folder).
    pub fn dir_for(&self, tag: &str) -> Option<PathBuf> {
        if tag.starts_with("steam-") && self.dirs.len() > 1 {
            return Some(self.dirs[1].clone());
        }
        self.dirs.first().cloned()
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
            let answer = Self::path(&d, &format!("p{}", feed.ping.rem_euclid(1000)));
            if let Err(e) = fs::write(&answer, "1") {
                (self.log)(&format!("could not write {}: {e}", answer.display()));
                return;
            }
            if let Some(old) = s.ping {
                if old.rem_euclid(1000) != feed.ping.rem_euclid(1000) {
                    remove(&Self::path(&d, &format!("p{}", old.rem_euclid(1000))));
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

    /// Hand a message to the game: kind 's' (the player started talking: no text yet), 'l' (the live words so far)
    /// or 'f' (the finished line; "" = nothing made out: the live words go); times: each unit's start (s after t0,
    /// when the line's audio began) - written with how long ago t0 is now. False if no game is listening.
    pub fn send_msg(&self, kind: char, utt: u32, text: &str, times: Option<&[f64]>, t0: Option<Instant>) -> bool {
        let text: String = py_strip(text).chars().take(TEXT_MAX).collect();
        // (cut with the text: the first n units keep their times)
        let times = times.and_then(|t| {
            let n = text::units(&text).len();
            (t.len() >= n).then(|| t[..n].to_vec())
        });
        let mut s = self.lock();
        let Some(dir) = s.dir.clone() else {
            return false;
        };
        if kind == 'l' && text.is_empty() {
            return false;
        }
        s.n += 1;
        let path = Self::path(&dir, &format!("t{}.xml", s.n));
        let tmp = Self::path(&dir, &format!("w{}.tmp", s.n));
        let ago = match (&times, t0) {
            (Some(_), Some(t0)) => Some(t0.elapsed().as_secs_f64()),
            _ => None,
        };
        let body = text_prefab(&text, kind, utt, if ago.is_some() { times.as_deref() } else { None }, ago);
        // (appears complete, never half-written)
        if let Err(e) = write_text(&tmp, &body).and_then(|_| fs::rename(&tmp, &path)) {
            (self.log)(&format!("could not write {}: {e}", path.display()));
            remove(&tmp);
            return false;
        }
        let n = s.n;
        s.pending.insert(n, path);
        true
    }
}

// ---------------------------------------------------------------- the test voices
/// The three test voices as wav files: made once with the Windows computer voices (none on other systems - the
/// dummies are then silent). {src: its wav} for those that are there.
pub fn make_voices() -> HashMap<i64, PathBuf> {
    make_voices_in(&work_dir())
}

/// make_voices into this folder (tests: a temp one).
pub fn make_voices_in(work: &Path) -> HashMap<i64, PathBuf> {
    let mut out = HashMap::new();
    if !cfg!(windows) {
        return out;
    }
    let _ = fs::create_dir_all(work);
    for (i, (voice, rate, said)) in VOICES.iter().enumerate() {
        let i = i as i64 + 1;
        let path = work.join(format!("voice{i}.wav"));
        if !path.exists() {
            let ps = format!(
                "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
                 try {{ $s.SelectVoice('{voice}') }} catch {{}}; $s.Rate = {rate}; $s.SetOutputToWaveFile('{}'); \
                 $s.Speak('{}'); $s.Dispose()",
                path.display().to_string().replace('\'', "''"),
                said.replace('\'', "''")
            );
            let mut cmd = std::process::Command::new("powershell");
            cmd.args(["-NoProfile", "-Command", &ps]).stdin(std::process::Stdio::null());
            cmd.env_remove("PSModulePath"); // (PowerShell 7's module path breaks Windows PowerShell's own modules)
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000); // (CREATE_NO_WINDOW)
            }
            match cmd.status() {
                Ok(st) if st.success() => {}
                _ => continue,
            }
        }
        if fs::metadata(&path).is_ok_and(|m| m.len() > 44) {
            out.insert(i, path);
        }
    }
    out
}

// ---------------------------------------------------------------- the game module
/// Teardown's module: the feed from savegame.xml (a FeedReader thread), the files for the game (a Link).
pub struct Teardown {
    sink: Arc<dyn FeedSink>,
    log: Log,
    save: PathBuf,
    link: Arc<Link>,
    reader: Option<FeedReader>,
    feed: Arc<Mutex<Option<Feed>>>,
}

impl Teardown {
    /// save: the savegame to read (None: Teardown's own); dirs: the folders for my files (None or empty: io_dirs()).
    pub fn new(sink: Arc<dyn FeedSink>, log: Log, save: Option<PathBuf>, dirs: Option<Vec<PathBuf>>) -> Teardown {
        let save = save.filter(|s| !s.as_os_str().is_empty()).unwrap_or_else(savegame_path);
        let dirs = dirs.filter(|d| !d.is_empty()).unwrap_or_else(io_dirs);
        let link = Arc::new(Link::new(dirs, log.clone()));
        Teardown { sink, log, save, link, reader: None, feed: Arc::new(Mutex::new(None)) }
    }

    /// The savegame.xml read.
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

    fn stop_reader(&mut self) {
        if let Some(r) = self.reader.as_mut() {
            r.stop();
        }
    }
}

/// GameKind::make for Teardown (io_dir: --io-dir, the one folder for my files).
pub fn make(sink: Arc<dyn FeedSink>, log: Log, io_dir: Option<PathBuf>) -> Box<dyn Game> {
    Box::new(Teardown::new(sink, log, None, io_dir.map(|d| vec![d])))
}

impl Game for Teardown {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        NAME
    }

    fn needs(&self) -> &'static str {
        NEEDS
    }

    fn locate(&self) -> (bool, String) {
        if let Some(inst) = steam::install_dir(APPID, None) {
            return (true, inst.display().to_string());
        }
        if !self.save.as_os_str().is_empty() && self.save.exists() {
            let dir = self.save.parent().map(Path::to_path_buf).unwrap_or_default();
            return (true, dir.display().to_string());
        }
        (false, "Teardown was not found in your Steam libraries".into())
    }

    fn start(&mut self) {
        self.stop_reader();
        self.link.start();
        let (sink, link, keep) = (self.sink.clone(), self.link.clone(), self.feed.clone());
        self.reader = Some(FeedReader::start(self.save.clone(), FeedReader::POLL, move |feed: Feed, tag: &str| {
            *keep.lock().unwrap_or_else(|e| e.into_inner()) = Some(feed.clone());
            sink.set_feed(feed.clone());
            link.on_feed(&feed, tag);
        }));
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

    fn test_voices(&self) -> HashMap<i64, PathBuf> {
        make_voices()
    }

    fn speaker_name(&self, src: i64) -> String {
        NAMES.iter().find(|(k, _)| *k == src).map_or_else(|| src.to_string(), |(_, n)| n.to_string())
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
}
