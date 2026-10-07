//! A game mod PROFILE: a small JSON file naming one of Kotodama's built-in connectors and giving its settings - never
//! code (game mods are added without a code review). PROTOCOL.md "Adding a game mod: profiles" has the format; this
//! is its parser and validator (readable errors, never a panic on a profile's content), the path placeholders and the
//! summary the import preview shows.
use crate::steam;
use kd_common::paths;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The profile format this Kotodama reads.
pub const FORMAT: u64 = 1;
/// A profile file bigger than this is refused (a profile is a few hundred bytes).
pub const MAX_SIZE: u64 = 64 * 1024;
/// The files connector's defaults (Teardown's link).
pub const DEFAULT_PREFIX: &str = "pcvx_";
pub const DEFAULT_COMPLETE: &str = "</registry>";
/// How big a profile's regex may compile (the regex crate runs in linear time; this bounds its memory).
const REGEX_SIZE: usize = 1 << 20;

// ---------------------------------------------------------------- placeholders and paths
/// A placeholder a path may start with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// {documents}: this user's Documents (Windows: where it really is - OneDrive moves it)
    Documents,
    /// {localappdata}: %LOCALAPPDATA% (Windows)
    LocalAppData,
    /// {home}: the user's home folder
    Home,
    /// {steam_app:ID}: the Steam app's install folder
    SteamApp(u32),
    /// {steam_workshop:ID}: the app's Workshop content folder
    SteamWorkshop(u32),
    /// {proton_user:ID}: Linux: the Windows user folder in the app's Proton prefix
    ProtonUser(u32),
    /// {env:NAME}: an environment variable (set and not empty)
    Env(String),
}

impl Place {
    fn parse(inner: &str) -> Result<Place, String> {
        let (name, arg) = match inner.split_once(':') {
            Some((n, a)) => (n, Some(a)),
            None => (inner, None),
        };
        let id = |a: Option<&str>| -> Result<u32, String> {
            match a.map(|a| (a, a.parse::<u32>())) {
                Some((a, Ok(n))) if n > 0 && a.bytes().all(|c| c.is_ascii_digit()) => Ok(n),
                _ => Err(format!("{{{inner}}}: {name} needs a Steam app id, as {{{name}:1167630}}")),
            }
        };
        Ok(match (name, arg) {
            ("documents", None) => Place::Documents,
            ("localappdata", None) => Place::LocalAppData,
            ("home", None) => Place::Home,
            ("steam_app", a) => Place::SteamApp(id(a)?),
            ("steam_workshop", a) => Place::SteamWorkshop(id(a)?),
            ("proton_user", a) => Place::ProtonUser(id(a)?),
            ("env", Some(v)) if !v.is_empty() && v.len() <= 64 && v.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') => {
                Place::Env(v.to_string())
            }
            ("env", _) => return Err(format!("{{{inner}}}: env needs a variable name (letters, digits, _), as {{env:NAME}}")),
            _ => {
                return Err(format!(
                    "unknown placeholder {{{inner}}} (known: {{documents}} {{localappdata}} {{home}} {{steam_app:ID}} \
                     {{steam_workshop:ID}} {{proton_user:ID}} {{env:NAME}})"
                ))
            }
        })
    }

    /// Where it is on this PC, or None.
    pub fn lookup(&self) -> Option<PathBuf> {
        let env = |n: &str| std::env::var_os(n).filter(|v| !v.is_empty()).map(PathBuf::from);
        match self {
            Place::Documents => Some(steam::documents_dir()),
            Place::LocalAppData => env("LOCALAPPDATA"),
            Place::Home => Some(paths::home()),
            Place::SteamApp(id) => steam::install_dir(*id, None),
            Place::SteamWorkshop(id) => steam::workshop_dir(*id, None),
            // (Windows runs the game itself: no Proton prefix)
            Place::ProtonUser(id) => {
                if cfg!(windows) {
                    None
                } else {
                    steam::proton_user(*id, None)
                }
            }
            Place::Env(n) => env(n),
        }
    }
}

/// Resolves placeholders (Place::lookup; tests: a stand-in).
pub type Lookup<'a> = &'a dyn Fn(&Place) -> Option<PathBuf>;

/// The real lookup.
pub fn this_pc(p: &Place) -> Option<PathBuf> {
    p.lookup()
}

/// One path in a profile: a placeholder or a full path ("C:/..." or "/..."), then folder names ("/" or "\"
/// between them; no "." or "..").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathTemplate {
    /// as the profile wrote it
    pub text: String,
    /// the placeholder it starts with (None: a full path)
    pub start: Option<Place>,
    /// a full path's root: "" (/) or a drive "C:"
    root: String,
    /// the names after it
    pub parts: Vec<String>,
}

impl PathTemplate {
    pub fn parse(text: &str) -> Result<PathTemplate, String> {
        if text.is_empty() || text.len() > 400 {
            return Err("a path must have 1 to 400 characters".into());
        }
        let mut pieces = text.split(['/', '\\']);
        let first = pieces.next().unwrap_or("");
        let (start, root) = if let Some(inner) = first.strip_prefix('{') {
            let Some(inner) = inner.strip_suffix('}') else {
                return Err(format!("{text:?}: a placeholder must be a whole folder name, as {{documents}}/Teardown"));
            };
            (Some(Place::parse(inner)?), String::new())
        } else if first.is_empty() && text.len() > 1 && !text.starts_with("\\\\") && !text.starts_with("//") {
            (None, String::new())
        } else if first.len() == 2 && first.as_bytes()[0].is_ascii_alphabetic() && first.as_bytes()[1] == b':' && text.len() > 2 {
            (None, first.to_string())
        } else {
            return Err(format!(
                "{text:?}: a path must start with a placeholder ({{documents}}, {{steam_app:ID}}, ...) or be a full path \
                 (C:/... or /...)"
            ));
        };
        let mut parts = Vec::new();
        for p in pieces {
            if p.is_empty() {
                continue;
            }
            if p == "." || p == ".." {
                return Err(format!("{text:?}: no \".\" or \"..\" in a path"));
            }
            if p.contains(['{', '}']) {
                return Err(format!("{text:?}: a placeholder can only start a path"));
            }
            if p.chars().any(|c| c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|')) {
                return Err(format!("{text:?}: a folder or file name may not hold : * ? \" < > | or control characters"));
            }
            parts.push(p.to_string());
        }
        Ok(PathTemplate { text: text.to_string(), start, root, parts })
    }

    /// The path on this PC; None when its placeholder does not resolve (or it is not a full path here).
    pub fn resolve_with(&self, look: Lookup) -> Option<PathBuf> {
        let mut p = match &self.start {
            Some(place) => look(place)?,
            None if self.root.is_empty() => PathBuf::from("/"),
            None => PathBuf::from(format!("{}\\", self.root)),
        };
        for part in &self.parts {
            p.push(part);
        }
        p.is_absolute().then_some(p)
    }

    /// The last name (a file's name).
    pub fn file_name(&self) -> Option<&str> {
        self.parts.last().map(String::as_str)
    }
}

/// A path entry: one path, or candidates (the first that resolves - and, for a folder, exists - is used).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathSpec(pub Vec<PathTemplate>);

impl PathSpec {
    fn parse(v: &Value, at: &str) -> Result<PathSpec, String> {
        let texts: Vec<&str> = match v {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) if !a.is_empty() && a.len() <= 8 => {
                a.iter().map(|x| x.as_str().ok_or(())).collect::<Result<_, _>>().map_err(|_| {
                    format!("\"{at}\": candidates must all be paths (strings)")
                })?
            }
            _ => return Err(format!("\"{at}\": must be a path, or a list of 1 to 8 candidate paths")),
        };
        texts.iter().map(|t| PathTemplate::parse(t).map_err(|e| format!("\"{at}\": {e}"))).collect::<Result<_, _>>().map(PathSpec)
    }

    /// The first candidate that resolves (a file: it need not exist yet).
    pub fn resolve_file(&self, look: Lookup) -> Option<PathBuf> {
        self.0.iter().find_map(|t| t.resolve_with(look))
    }

    /// The first candidate that resolves to a folder that exists.
    pub fn resolve_dir(&self, look: Lookup) -> Option<PathBuf> {
        self.0.iter().filter_map(|t| t.resolve_with(look)).find(|p| p.is_dir())
    }

    /// As the profile wrote it: "a or b".
    pub fn text(&self) -> String {
        self.0.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" or ")
    }
}

// ---------------------------------------------------------------- the profile
/// How the files connector writes a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageFormat {
    /// <prefix>t<n>.xml: teardown::text_prefab exactly (a prefab whose tags hold the message)
    TeardownPrefab,
    /// <prefix>t<n>.json: one JSON object {"k","u","t","w","a"}
    Json,
}

impl MessageFormat {
    /// the message files' extension
    pub fn ext(self) -> &'static str {
        match self {
            MessageFormat::TeardownPrefab => "xml",
            MessageFormat::Json => "json",
        }
    }
}

/// The files connector's settings (Teardown's link, PROTOCOL.md).
#[derive(Clone, Debug)]
pub struct FilesConfig {
    /// the file the mod's feed is read from (the game writes it)
    pub feed_file: PathSpec,
    /// a regex with one group: the feed string (bytes; Teardown's: teardown::FEED)
    pub pattern: String,
    /// a regex with one group: the tag of the mod copy that wrote a feed (the last match before it); "" = none
    pub tag_pattern: String,
    /// the file is read only when it ends with this (after white space); "" = always
    pub complete: String,
    /// the folders the message files go to (missing ones skipped)
    pub dirs: Vec<PathSpec>,
    /// (tag prefix, index into dirs): which folder a mod copy's tag maps to (else dirs[0])
    pub tag_dirs: Vec<(String, usize)>,
    /// the start of every file it writes (and the only files it deletes)
    pub prefix: String,
    pub message: MessageFormat,
    /// pattern and tag_pattern, compiled (validated when the profile was read)
    pub(crate) feed_re: regex::bytes::Regex,
    pub(crate) tag_re: Option<regex::bytes::Regex>,
}

/// The socket connector's settings.
#[derive(Clone, Debug)]
pub struct SocketConfig {
    /// on 127.0.0.1 only
    pub port: u16,
}

#[derive(Clone, Debug)]
pub enum Connector {
    Files(Box<FilesConfig>),
    Socket(SocketConfig),
}

/// A test speaker's recorded voice (a Windows SAPI voice reading a text), as teardown::VOICES.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestVoice {
    /// the feed's src that plays it
    pub src: i64,
    /// the Windows voice: "Microsoft Zira Desktop"
    pub voice: String,
    /// speaking rate -10..10
    pub rate: i32,
    pub text: String,
}

/// A parsed, valid profile.
#[derive(Clone, Debug)]
pub struct Profile {
    /// [a-z0-9-]{3,40}: the settings key
    pub id: String,
    /// the game: "Teardown"
    pub game: String,
    /// the mod: "Proximity Babble Chat"
    pub mod_name: String,
    /// the mod's page (http/https)
    pub url: String,
    pub author: String,
    /// what players need, shown while waiting (default "the <mod> mod")
    pub needs: String,
    /// the Steam app that is the game (locate)
    pub steam_app: Option<u32>,
    /// Kotodama plays the speakers from the feed ("uses": "voices")
    pub voices: bool,
    /// Kotodama listens to the microphone and sends what the player said ("uses": "speech")
    pub speech: bool,
    pub test_voices: Vec<TestVoice>,
    pub speaker_names: BTreeMap<i64, String>,
    pub connector: Connector,
}

/// A JSON object's fields, with readable errors naming where ("connector.out.prefix").
struct Fields<'a> {
    map: &'a Map<String, Value>,
    at: String,
}

impl<'a> Fields<'a> {
    fn new(v: &'a Value, at: &str) -> Result<Fields<'a>, String> {
        match v {
            Value::Object(map) => Ok(Fields { map, at: at.to_string() }),
            _ => Err(if at.is_empty() { "a profile must be a JSON object { ... }".into() } else { format!("\"{at}\": must be an object {{ ... }}") }),
        }
    }

    fn name(&self, k: &str) -> String {
        if self.at.is_empty() {
            k.to_string()
        } else {
            format!("{}.{k}", self.at)
        }
    }

    /// (no fields it does not know: a typo is an error, not silently ignored)
    fn only(&self, known: &[&str]) -> Result<(), String> {
        for k in self.map.keys() {
            if !known.contains(&k.as_str()) {
                return Err(format!("unknown field \"{}\" (known here: {})", self.name(k), known.join(", ")));
            }
        }
        Ok(())
    }

    fn get(&self, k: &str) -> Option<&'a Value> {
        self.map.get(k).filter(|v| !v.is_null())
    }

    fn req(&self, k: &str) -> Result<&'a Value, String> {
        self.get(k).ok_or_else(|| format!("\"{}\" is missing", self.name(k)))
    }

    fn opt_str(&self, k: &str) -> Result<Option<&'a str>, String> {
        match self.get(k) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            Some(_) => Err(format!("\"{}\": must be a string", self.name(k))),
        }
    }

    /// a short line of text: 1..=max characters, no control characters
    fn text(&self, k: &str, max: usize) -> Result<String, String> {
        let s = self.opt_str(k)?.ok_or_else(|| format!("\"{}\" is missing", self.name(k)))?;
        check_text(s, max).map_err(|e| format!("\"{}\": {e}", self.name(k)))?;
        Ok(s.to_string())
    }

    fn int(&self, k: &str, lo: i64, hi: i64, what: &str) -> Result<Option<i64>, String> {
        match self.get(k) {
            None => Ok(None),
            Some(v) => match v.as_i64() {
                Some(n) if (lo..=hi).contains(&n) => Ok(Some(n)),
                _ => Err(format!("\"{}\": must be {what} (got {v})", self.name(k))),
            },
        }
    }
}

fn check_text(s: &str, max: usize) -> Result<(), String> {
    let n = s.chars().count();
    if n == 0 || n > max {
        return Err(format!("must have 1 to {max} characters"));
    }
    if s.chars().any(char::is_control) {
        return Err("may not hold control characters (line breaks, tabs, ...)".into());
    }
    Ok(())
}

fn compile(pattern: &str, at: &str) -> Result<regex::bytes::Regex, String> {
    let re = regex::bytes::RegexBuilder::new(pattern)
        .size_limit(REGEX_SIZE)
        .build()
        .map_err(|e| format!("\"{at}\": not a valid regex: {}", e.to_string().lines().last().unwrap_or("")))?;
    if re.captures_len() != 2 {
        return Err(format!("\"{at}\": the regex must have exactly one group ( ... ), it has {}", re.captures_len() - 1));
    }
    Ok(re)
}

/// Is this a safe file prefix: 3+ letters, digits or _, ending in _ (so its patterns name only its own files).
pub fn safe_prefix(p: &str) -> bool {
    p.len() >= 3 && p.len() <= 32 && p.ends_with('_') && p.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

fn valid_id(id: &str) -> bool {
    (3..=40).contains(&id.len()) && id.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

impl Profile {
    /// Parse and validate a profile's JSON.
    pub fn parse(text: &str) -> Result<Profile, String> {
        if text.len() as u64 > MAX_SIZE {
            return Err(format!("too big for a profile ({} KB; at most {} KB)", text.len() / 1024, MAX_SIZE / 1024));
        }
        let text = text.strip_prefix('\u{FEFF}').unwrap_or(text); // (Notepad's byte order mark)
        let v: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
        let f = Fields::new(&v, "")?;
        // (the version first: a newer file's fields are not "unknown", it is newer)
        match f.req("format")?.as_u64() {
            Some(FORMAT) => {}
            Some(n) if n > FORMAT => {
                return Err(format!("profile format {n} is newer than this Kotodama reads ({FORMAT}): update Kotodama"))
            }
            _ => return Err(format!("\"format\": must be {FORMAT}")),
        }
        f.only(&[
            "format", "id", "game", "mod", "url", "author", "needs", "locate", "uses", "test_voices", "speaker_names",
            "connector",
        ])?;
        let id = f.opt_str("id")?.ok_or("\"id\" is missing")?.to_string();
        if !valid_id(&id) {
            return Err(format!("\"id\": {id:?} must be 3 to 40 of a-z, 0-9 and - (as \"my-game\")"));
        }
        let game = f.text("game", 80)?;
        let mod_name = f.text("mod", 80)?;
        let url = f.text("url", 300)?;
        if !(url.starts_with("https://") || url.starts_with("http://")) || url.contains(char::is_whitespace) {
            return Err(format!("\"url\": must be a web page (https://...), got {url:?}"));
        }
        let author = f.text("author", 80)?;
        let needs = match f.get("needs") {
            Some(_) => f.text("needs", 200)?,
            None => format!("the {mod_name} mod"),
        };
        let steam_app = match f.get("locate") {
            None => None,
            Some(l) => {
                let lf = Fields::new(l, "locate")?;
                lf.only(&["steam_app"])?;
                lf.int("steam_app", 1, u32::MAX as i64, "a Steam app id")?.map(|n| n as u32)
            }
        };
        let (voices, speech) = match f.get("uses") {
            None => (true, true),
            Some(Value::Array(a)) if !a.is_empty() => {
                let (mut v, mut s) = (false, false);
                for x in a {
                    match x.as_str() {
                        Some("voices") => v = true,
                        Some("speech") => s = true,
                        _ => return Err(format!("\"uses\": unknown {x} (known: \"voices\", \"speech\")")),
                    }
                }
                (v, s)
            }
            Some(_) => {
                return Err("\"uses\": must be a list of at least one of \"voices\", \"speech\"".into());
            }
        };
        let mut test_voices: Vec<TestVoice> = Vec::new();
        match f.get("test_voices") {
            None => {}
            Some(Value::Array(a)) if a.len() <= 16 => {
                for (i, x) in a.iter().enumerate() {
                    let at = format!("test_voices[{i}]");
                    let tf = Fields::new(x, &at)?;
                    tf.only(&["src", "voice", "rate", "text"])?;
                    let src = tf.int("src", 1, 999, "a whole number from 1 to 999")?.ok_or(format!("\"{at}.src\" is missing"))?;
                    if test_voices.iter().any(|t| t.src == src) {
                        return Err(format!("\"{at}.src\": {src} is there twice"));
                    }
                    let rate = tf.int("rate", -10, 10, "a whole number from -10 to 10")?.unwrap_or(0) as i32;
                    test_voices.push(TestVoice { src, voice: tf.text("voice", 100)?, rate, text: tf.text("text", 1000)? });
                }
            }
            Some(_) => return Err("\"test_voices\": must be a list of at most 16 voices".into()),
        }
        let mut speaker_names = BTreeMap::new();
        if let Some(n) = f.get("speaker_names") {
            let nf = Fields::new(n, "speaker_names")?;
            for k in nf.map.keys() {
                let src: i64 = k.parse().map_err(|_| format!("\"speaker_names\": the key {k:?} must be a speaker's src number"))?;
                speaker_names.insert(src, nf.text(k, 40)?);
            }
        }
        let connector = parse_connector(f.req("connector")?)?;
        Ok(Profile {
            id,
            game,
            mod_name,
            url,
            author,
            needs,
            steam_app,
            voices,
            speech,
            test_voices,
            speaker_names,
            connector,
        })
    }

    /// The name of a test speaker (its src number when the profile names none).
    pub fn speaker_name(&self, src: i64) -> String {
        self.speaker_names.get(&src).cloned().unwrap_or_else(|| src.to_string())
    }

    /// (found, where): the game's Steam install folder, else the folder of the file it is read from.
    pub fn locate(&self, feed_file: Option<&Path>) -> (bool, String) {
        if let Some(inst) = self.steam_app.and_then(|a| steam::install_dir(a, None)) {
            return (true, inst.display().to_string());
        }
        if let Some(f) = feed_file.filter(|f| !f.as_os_str().is_empty() && f.exists()) {
            return (true, f.parent().map(Path::to_path_buf).unwrap_or_default().display().to_string());
        }
        if self.steam_app.is_some() {
            return (false, format!("{} was not found in your Steam libraries", self.game));
        }
        (false, String::new())
    }

    /// What it does on this PC, for the import preview (placeholders resolved).
    pub fn summary(&self) -> Vec<String> {
        self.summary_with(&this_pc)
    }

    /// summary() with a stand-in for the placeholders (tests).
    pub fn summary_with(&self, look: Lookup) -> Vec<String> {
        const MISSING: &str = "(not found on this PC)";
        let mut out = Vec::new();
        if let Some(app) = self.steam_app {
            out.push(match look(&Place::SteamApp(app)) {
                Some(p) => format!("finds {} through Steam (app {app}): {}", self.game, p.display()),
                None => format!("finds {} through Steam (app {app}) {MISSING}", self.game),
            });
        }
        match &self.connector {
            Connector::Files(c) => {
                out.push(match c.feed_file.resolve_file(look) {
                    Some(p) => format!("reads {}", p.display()),
                    None => format!("reads {} {MISSING}", c.feed_file.text()),
                });
                for d in &c.dirs {
                    out.push(match d.resolve_dir(look) {
                        Some(p) => format!("writes its message files ({}*) into {}", c.prefix, p.display()),
                        None => format!("writes its message files ({}*) into {} {MISSING}", c.prefix, d.text()),
                    });
                }
                let p = &c.prefix;
                out.push(format!(
                    "deletes only its own files there: {p}on, {p}p<n>, {p}t<n>.{}, {p}w<n>.tmp",
                    c.message.ext()
                ));
            }
            Connector::Socket(s) => out.push(format!("listens on 127.0.0.1:{} (this computer only)", s.port)),
        }
        if self.voices {
            out.push("plays other players' voices".into());
        }
        if self.speech {
            out.push("writes what you say (speech to text), while the game asks for the microphone".into());
        }
        if !self.test_voices.is_empty() {
            out.push(format!("makes {} test voices with Windows' speech voices", self.test_voices.len()));
        }
        out
    }
}

fn parse_connector(v: &Value) -> Result<Connector, String> {
    let f = Fields::new(v, "connector")?;
    match f.opt_str("type")? {
        Some("files") => {
            f.only(&["type", "feed", "out"])?;
            let feed = Fields::new(f.req("feed")?, "connector.feed")?;
            feed.only(&["file", "pattern", "tag_pattern", "complete"])?;
            let feed_file = PathSpec::parse(feed.req("file")?, "connector.feed.file")?;
            if feed_file.0.iter().any(|t| t.parts.is_empty()) {
                return Err("\"connector.feed.file\": must end in the file's name, as {localappdata}/Game/save.xml".into());
            }
            let pattern = feed.opt_str("pattern")?.unwrap_or(crate::teardown::FEED).to_string();
            let feed_re = compile(&pattern, "connector.feed.pattern")?;
            let tag_pattern = feed.opt_str("tag_pattern")?.unwrap_or(crate::teardown::MODTAG).to_string();
            let tag_re = match tag_pattern.as_str() {
                "" => None,
                t => Some(compile(t, "connector.feed.tag_pattern")?),
            };
            let complete = feed.opt_str("complete")?.unwrap_or(DEFAULT_COMPLETE).to_string();
            if complete.len() > 100 {
                return Err("\"connector.feed.complete\": at most 100 characters".into());
            }
            let out = Fields::new(f.req("out")?, "connector.out")?;
            out.only(&["dirs", "tag_dirs", "prefix", "message"])?;
            let dirs = match out.req("dirs")? {
                Value::Array(a) if !a.is_empty() && a.len() <= 8 => a
                    .iter()
                    .enumerate()
                    .map(|(i, d)| PathSpec::parse(d, &format!("connector.out.dirs[{i}]")))
                    .collect::<Result<Vec<_>, _>>()?,
                _ => return Err("\"connector.out.dirs\": must be a list of 1 to 8 folders".into()),
            };
            let mut tag_dirs = Vec::new();
            match out.get("tag_dirs") {
                None => {}
                Some(Value::Array(a)) if a.len() <= 8 => {
                    for (i, x) in a.iter().enumerate() {
                        let tf = Fields::new(x, &format!("connector.out.tag_dirs[{i}]"))?;
                        tf.only(&["tag_prefix", "dir"])?;
                        let prefix = tf.text("tag_prefix", 100)?;
                        let max = dirs.len() as i64 - 1;
                        let dir = tf
                            .int("dir", 0, max, &format!("an index into \"connector.out.dirs\" (0 to {max})"))?
                            .ok_or(format!("\"connector.out.tag_dirs[{i}].dir\" is missing"))?;
                        tag_dirs.push((prefix, dir as usize));
                    }
                }
                Some(_) => return Err("\"connector.out.tag_dirs\": must be a list of at most 8 {tag_prefix, dir}".into()),
            }
            let prefix = out.opt_str("prefix")?.unwrap_or(DEFAULT_PREFIX).to_string();
            if !safe_prefix(&prefix) {
                return Err(format!(
                    "\"connector.out.prefix\": {prefix:?} must be 3 to 32 letters, digits or _ and end in _ (as \"pcvx_\"): \
                     it names every file Kotodama writes and the only ones it deletes"
                ));
            }
            let message = match out.opt_str("message")?.unwrap_or("teardown-prefab") {
                "teardown-prefab" => MessageFormat::TeardownPrefab,
                "json" => MessageFormat::Json,
                m => return Err(format!("\"connector.out.message\": {m:?} is not a message format (known: \"teardown-prefab\", \"json\")")),
            };
            Ok(Connector::Files(Box::new(FilesConfig {
                feed_file,
                pattern,
                tag_pattern,
                complete,
                dirs,
                tag_dirs,
                prefix,
                message,
                feed_re,
                tag_re,
            })))
        }
        Some("socket") => {
            f.only(&["type", "port"])?;
            let port = f.int("port", 1024, 65535, "a port from 1024 to 65535")?.ok_or("\"connector.port\" is missing")?;
            Ok(Connector::Socket(SocketConfig { port: port as u16 }))
        }
        Some(t) => Err(format!("\"connector.type\": {t:?} is not a connector (known: \"files\", \"socket\")")),
        None => Err("\"connector.type\" is missing (\"files\" or \"socket\")".into()),
    }
}
