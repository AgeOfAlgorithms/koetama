//! The game mods Koetama works with (engine/games/). A game mod is a PROFILE (profile.rs: a small JSON file), never
//! code: Koetama's built-in CONNECTORS do the talking (files.rs: Teardown's link, through a file the game saves and
//! files next to the mod; socket.rs: a TCP connection on 127.0.0.1) and a profile only names one and gives its
//! settings. Built-in profiles (Teardown) are compiled in; others are *.json files in [`profiles_dir`]. [`games`]
//! lists them in the window's game picker.
//!
//! The engine (speech to text, the voice mixer) knows no game; a connector is only the game's LINK. The game's state,
//! as the connector reads it, is a FEED (kd_common::feed::Feed), handed to the mixer (a FeedSink) and kept by the game.
pub mod api;
pub mod files;
pub mod profile;
pub mod socket;
pub mod steam;
pub mod teardown;
pub mod voices;

use kd_common::feed::{Feed, FeedSink, RuleState};
use kd_common::{paths, Log};
use profile::{Connector, Profile};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Instant, SystemTime};

/// What every game module gives the program (engine/games/base.py).
pub trait Game: Send + Sync {
    /// short, for settings: "<game>-<mod>", e.g. "teardown-proximity-babble-chat"
    fn id(&self) -> &'static str;
    /// shown in the game picker: "Teardown"
    fn name(&self) -> &'static str;
    /// what the player needs in the game, shown while waiting: "the Proximity Babble Chat mod"
    fn needs(&self) -> &'static str;

    // ---- the program calls these
    /// (found, where): is the game installed here, and where its files are (shown in the app)
    fn locate(&self) -> (bool, String) {
        (false, String::new())
    }

    /// start listening to the game (a thread of its own); tell it Koetama runs
    fn start(&mut self) {}

    /// stop; tell the game Koetama is gone
    fn stop(&mut self) {}

    /// hand the game what the player said (api::speech): kind 's' (they started talking), 'l' (the words so far,
    /// only ever growing), 'f' (the finished line; "" = nothing made out), or 'r' the session's voice room
    /// ("<room>:<key>"). times: each unit's start in s after t0 (when the line's audio began). False if no game is
    /// listening
    fn send(&self, _kind: char, _utt: u32, _text: &str, _times: Option<&[f64]>, _t0: Option<Instant>) -> bool {
        false
    }

    /// a typed line (--type, --auto): handed to the game as a finished line
    fn send_text(&self, _text: &str) -> bool {
        false
    }

    /// the translation of the feed's line `id` (PROTOCOL.md "Translation"; "" = nothing to show - exactly one per
    /// id). False if no game is listening
    fn send_translation(&self, _id: i64, _text: &str) -> bool {
        false
    }

    /// the translations' states, sent when they change. False if no game is listening
    fn send_translations_state(&self, _rules: &[RuleState]) -> bool {
        false
    }

    /// the voice chat's link to the relay, for the game to show: "off" (no room), "connecting", "connected",
    /// "unreachable" (the last tries failed: still trying). Called with each change.
    fn set_voice_state(&self, _state: &str) {}

    /// {test voice id: its wav file}: the recorded voices the game's test speakers play (the program loads
    /// them with kd_audio::load_wav). Blocking: the first call makes them (PowerShell, a few seconds each)
    fn test_voices(&self) -> HashMap<i64, PathBuf> {
        HashMap::new()
    }

    fn speaker_name(&self, src: i64) -> String {
        src.to_string()
    }

    /// the latest feed from the game, if any
    fn feed(&self) -> Option<Feed>;

    /// the mixer's feed is fresh (the game is running the mod)
    fn connected(&self) -> bool;

    // ---- what the game wants (from its feed)
    fn wants_mic(&self) -> bool {
        self.feed().is_some_and(|f| f.mic)
    }

    /// push to talk: Some(the key is held); None - always on (the speech detector decides)
    fn push_to_talk(&self) -> Option<bool> {
        self.feed().filter(|f| f.mic).and_then(|f| f.ptt)
    }

    fn language(&self) -> String {
        self.feed().map(|f| f.lang).filter(|l| !l.is_empty()).unwrap_or_else(|| "en".into())
    }

    fn live_words(&self) -> bool {
        self.feed().map(|f| f.live).unwrap_or(true)
    }

    /// live feeds read so far
    fn updates(&self) -> u64 {
        0
    }

    /// what the command line prints at the start: "reading <save>", "my files for the game go to: <dirs>"
    fn describe(&self) -> Vec<String> {
        Vec::new()
    }
}

/// A &'static str for a profile's text (the Game trait's id/name/needs): each distinct string kept once.
pub fn intern(s: &str) -> &'static str {
    static KEPT: LazyLock<Mutex<HashSet<&'static str>>> = LazyLock::new(Mutex::default);
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(k) = kept.get(s) {
        return k;
    }
    let k: &'static str = Box::leak(s.to_string().into_boxed_str());
    kept.insert(k);
    k
}

/// A game mod the program works with (the window's picker lists these): the game, the mod that links it to
/// Koetama and where to get that mod, what it does on this PC, and how to make its module.
#[derive(Clone)]
pub struct GameKind {
    pub id: String,
    /// the game: "Teardown"
    pub name: String,
    /// the mod: "Proximity Babble Chat"
    pub mod_name: String,
    /// the mod's page (where players get it; http/https only)
    pub mod_url: String,
    pub author: String,
    /// "the Proximity Babble Chat mod" (the waiting line)
    pub needs: String,
    /// compiled in (else a profile file)
    pub builtin: bool,
    /// the profile file
    pub source: Option<PathBuf>,
    /// what it does, for the import preview: "reads <file>", "writes its message files (pcvx_*) into <dir>",
    /// "listens on 127.0.0.1:<port> (this computer only)" (placeholders resolved when it was listed)
    pub summary: Vec<String>,
    /// Koetama plays the speakers from the feed (the audio output is needed)
    pub voices: bool,
    /// Koetama listens to the microphone and sends what the player said (speech to text is needed)
    pub speech: bool,
    /// Koetama translates the chat lines the game sends (PROTOCOL.md "Translation": the translation models are needed)
    pub translate: bool,
    /// the profile itself
    pub profile: Arc<Profile>,
}

impl std::fmt::Debug for GameKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GameKind")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("builtin", &self.builtin)
            .field("source", &self.source)
            .finish()
    }
}

impl GameKind {
    /// A game kind from a valid profile (summary: resolved on this PC now).
    pub fn from_profile(profile: Profile, builtin: bool, source: Option<PathBuf>) -> GameKind {
        GameKind {
            id: profile.id.clone(),
            name: profile.game.clone(),
            mod_name: profile.mod_name.clone(),
            mod_url: profile.url.clone(),
            author: profile.author.clone(),
            needs: profile.needs.clone(),
            builtin,
            source,
            summary: profile.summary(),
            voices: profile.voices,
            speech: profile.speech,
            translate: profile.translate,
            profile: Arc::new(profile),
        }
    }

    /// The game's module: (the feed's sink: the mixer; the log; a folder for the game's files instead of the usual
    /// ones - --io-dir, files connector only). Cheap: nothing runs until Game::start.
    pub fn make(&self, sink: Arc<dyn FeedSink>, log: Log, io_dir: Option<PathBuf>) -> Box<dyn Game> {
        match &self.profile.connector {
            Connector::Files(_) => Box::new(files::FilesGame::with_paths(
                self.profile.clone(),
                self.builtin,
                sink,
                log,
                None,
                io_dir.map(|d| vec![d]),
            )),
            Connector::Socket(_) => Box::new(socket::SocketGame::new(self.profile.clone(), self.builtin, sink, log)),
        }
    }
}

/// The built-in game mods (compiled in).
fn builtins() -> Vec<GameKind> {
    vec![GameKind::from_profile((*teardown::profile()).clone(), true, None)]
}

/// Where profile files are: Koetama's data folder/games (KOETAMA_PROFILES_DIR: tests).
pub fn profiles_dir() -> PathBuf {
    match std::env::var_os("KOETAMA_PROFILES_DIR").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => paths::data_dir().join("games"),
    }
}

/// A profile file's name ends in .json (any case).
fn is_json(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json"))
}

/// Parse and validate a profile file (the import preview). Errors start with the file's name.
pub fn load_profile(path: &Path) -> Result<GameKind, String> {
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let read = || -> Result<Profile, String> {
        let meta = std::fs::metadata(path).map_err(|e| format!("cannot read it: {e}"))?;
        if !meta.is_file() {
            return Err("not a file".into());
        }
        if meta.len() > profile::MAX_SIZE {
            return Err(format!("too big for a profile ({} KB; at most {} KB)", meta.len() / 1024, profile::MAX_SIZE / 1024));
        }
        let bytes = std::fs::read(path).map_err(|e| format!("cannot read it: {e}"))?;
        let text = String::from_utf8(bytes).map_err(|_| "not UTF-8 text".to_string())?;
        Profile::parse(&text)
    };
    let p = read().map_err(|e| format!("{name}: {e}"))?;
    Ok(GameKind::from_profile(p, false, Some(path.to_path_buf())))
}

/// The profiles folder, read: (the games, the files that do not load and why).
type Listing = (Vec<GameKind>, Vec<(PathBuf, String)>);

/// What the folder looked like when it was last read: (folder, [(file, size, modified)]).
type FolderKey = (PathBuf, Vec<(PathBuf, u64, Option<SystemTime>)>);

/// The last listing, and what the folder looked like then.
type Cached = Option<(FolderKey, Arc<Listing>)>;

static CACHE: LazyLock<Mutex<Cached>> = LazyLock::new(Mutex::default);

fn folder_key(dir: &Path) -> FolderKey {
    let mut files = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if is_json(&p) {
                let meta = e.metadata().ok();
                files.push((p, meta.as_ref().map_or(0, |m| m.len()), meta.and_then(|m| m.modified().ok())));
            }
        }
    }
    files.sort();
    (dir.to_path_buf(), files)
}

/// Built-ins, then the folder's valid profiles (sorted by file name); a clash with an id listed before: skipped
/// (and in the bad list). Cached until the folder's files change (games() is called every frame by the window).
fn listing() -> Arc<Listing> {
    let key = folder_key(&profiles_dir());
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((k, l)) = cache.as_ref() {
        if *k == key {
            return l.clone();
        }
    }
    let mut all = builtins();
    let mut bad = Vec::new();
    for (path, _, _) in &key.1 {
        match load_profile(path) {
            Ok(g) if prefix_taken(&g, &all).is_some() => bad.push((
                path.clone(),
                format!("its message files' prefix is already used by {}: skipped", prefix_taken(&g, &all).unwrap_or_default()),
            )),
            Ok(g) => match all.iter().find(|o| o.id == canonical(&g.id)) {
                Some(o) => {
                    let first = if o.builtin {
                        format!("Koetama's built-in {}", o.name)
                    } else {
                        o.source.as_ref().and_then(|s| s.file_name()).map_or(String::new(), |n| n.to_string_lossy().into_owned())
                    };
                    bad.push((path.clone(), format!("the id \"{}\" is already used by {first}: skipped", g.id)));
                }
                None => all.push(g),
            },
            Err(e) => bad.push((path.clone(), e)),
        }
    }
    let l = Arc::new((all, bad));
    *cache = Some((key, l.clone()));
    l
}

/// The games, in the picker's order: built-ins, then the profiles folder's valid ones.
pub fn games() -> Vec<GameKind> {
    listing().0.clone()
}

/// Files in the profiles folder that do not load, and why (a clash of ids too).
pub fn bad_profiles() -> Vec<(PathBuf, String)> {
    listing().1.clone()
}

/// The game with this id; an unknown id: the first.
pub fn by_id(id: &str) -> GameKind {
    let id = canonical(id);
    let l = listing();
    l.0.iter().find(|g| g.id == id).unwrap_or(&l.0[0]).clone()
}

/// An id as the list knows it: old ids of built-ins (from before ids named the mod too: "teardown" was Proximity
/// Babble Chat's) are theirs - settings that hold one still find it, and no profile can take it.
fn canonical(id: &str) -> &str {
    if id == "teardown" {
        teardown::ID
    } else {
        id
    }
}

/// The message-file prefix of a files profile (two mods using one prefix in a folder would delete each other's files).
fn files_prefix(g: &GameKind) -> Option<String> {
    match &g.profile.connector {
        profile::Connector::Files(f) => Some(f.prefix.to_ascii_lowercase()),
        _ => None,
    }
}

/// Who already uses this profile's message-file prefix, if anyone (another game mod's name).
fn prefix_taken(g: &GameKind, others: &[GameKind]) -> Option<String> {
    let p = files_prefix(g)?;
    others
        .iter()
        .find(|o| o.id != g.id && files_prefix(o).as_deref() == Some(p.as_str()))
        .map(|o| format!("{} ({} mod)", o.name, o.mod_name))
}

/// Validate a profile file and copy it into profiles_dir() as <id>.json (replacing that id's profile: any other file
/// there with the same id goes). A built-in's id cannot be replaced.
pub fn install_profile(path: &Path) -> Result<GameKind, String> {
    let g = load_profile(path)?;
    if let Some(b) = builtins().iter().find(|b| b.id == canonical(&g.id)) {
        return Err(format!("the id \"{}\" is Koetama's built-in {}: a profile cannot replace it", g.id, b.name));
    }
    if let Some(who) = prefix_taken(&g, &games()) {
        return Err(format!(
            "its message files' prefix is already used by {who}: the two would remove each other's files (the mod's \
             author must pick another prefix)"
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let dir = profiles_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let dest = dir.join(format!("{}.json", g.id));
    let same = |a: &Path, b: &Path| std::fs::canonicalize(a).ok().zip(std::fs::canonicalize(b).ok()).is_some_and(|(a, b)| a == b);
    for other in same_id_files(&dir, &g.id) {
        if !same(&other, &dest) && !same(&other, path) {
            let _ = std::fs::remove_file(&other);
        }
    }
    let tmp = dir.join(format!("{}.json.tmp", g.id));
    std::fs::write(&tmp, &bytes)
        .and_then(|_| std::fs::rename(&tmp, &dest))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("cannot write {}: {e}", dest.display())
        })?;
    if !same(path, &dest) {
        // (the copy outside the folder stays the user's; one with the same id inside it would clash)
        if path.parent().is_some_and(|p| same(p, &dir)) {
            let _ = std::fs::remove_file(path);
        }
    }
    load_profile(&dest)
}

/// The *.json files in dir whose profile has this id (valid ones), and <id>.json.
fn same_id_files(dir: &Path, id: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !is_json(&p) {
                continue;
            }
            let named = p.file_stem().is_some_and(|s| s.eq_ignore_ascii_case(id));
            if named || load_profile(&p).is_ok_and(|g| g.id == id) {
                out.push(p);
            }
        }
    }
    out
}

/// Remove a profile file's game (its file(s) in profiles_dir(); never a built-in).
pub fn remove_profile(id: &str) -> Result<(), String> {
    if let Some(b) = builtins().iter().find(|b| b.id == id) {
        return Err(format!("{} is built into Koetama: it cannot be removed", b.name));
    }
    let files = same_id_files(&profiles_dir(), id);
    if files.is_empty() {
        return Err(format!("no game mod profile with the id \"{id}\""));
    }
    for f in files {
        std::fs::remove_file(&f).map_err(|e| format!("cannot remove {}: {e}", f.display()))?;
    }
    Ok(())
}
