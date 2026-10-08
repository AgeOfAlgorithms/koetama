//! Teardown, through the mod Proximity Babble Chat (its voice.lua) - engine/games/teardown.py. A BUILT-IN profile
//! (profiles/teardown-proximity-babble-chat.json) on the files connector (files.rs; PROTOCOL.md has the formats):
//!   game -> Kotodama   savegame.xml: savegame.mod.pcvx.f, ~20 times a second - whom the player hears and how
//!                      (volume, direction, muffle), and what the game wants (the microphone, the language, a ping)
//!   Kotodama -> game   small files next to the mod's folder: pcvx_on (running), pcvx_p<n> (the answer to ping n),
//!                      pcvx_t<n>.xml (message n: what the player said)
//! Teardown runs on Windows; on Linux (Steam Deck) through Proton - its files are then inside its Proton prefix.
use crate::files::{self, FilesGame};
use crate::profile::{self, Connector, Profile, TestVoice};
use crate::{steam, Game};
use kd_common::feed::FeedSink;
use kd_common::{paths, Log};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

pub use crate::files::{
    find_feeds, parse_feed, read_shared, text_prefab, times_hex, FeedReader, FeedScan, Link, PREFIX, TEXT_MAX,
};

pub const APPID: u32 = 1167630;
pub const FEED: &str = r#"(?-u)<pcvx>\s*<f\s+value="([^"]*)"\s*/>\s*</pcvx>"#;
pub const MODTAG: &str = r#"(?-u)<((?:local|steam)-[^\s/>]+)>"#;

/// the game AND the mod: another Teardown mod made for Kotodama has its own id
pub const ID: &str = "teardown-proximity-babble-chat";
pub const NAME: &str = "Teardown";
pub const NEEDS: &str = "the Proximity Babble Chat mod";
pub const MOD_NAME: &str = "Proximity Babble Chat";
/// the mod on the Steam Workshop (its id.txt)
pub const MOD_URL: &str = "https://steamcommunity.com/sharedfiles/filedetails/?id=3812301496";

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

/// The built-in profile's JSON (compiled in).
pub const PROFILE_JSON: &str = include_str!("profiles/teardown-proximity-babble-chat.json");

static PROFILE: LazyLock<Arc<Profile>> =
    LazyLock::new(|| Arc::new(Profile::parse(PROFILE_JSON).expect("the built-in Teardown profile is valid (tested)")));

/// Teardown's profile.
pub fn profile() -> Arc<Profile> {
    PROFILE.clone()
}

fn files_config() -> &'static profile::FilesConfig {
    match &PROFILE.connector {
        Connector::Files(c) => c,
        Connector::Socket(_) => unreachable!("the built-in Teardown profile uses the files connector"),
    }
}

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

/// Teardown's savegame.xml (empty when it cannot be known: Linux without the Proton prefix). SAVEPROBE_DIR: tests.
pub fn savegame_path() -> PathBuf {
    files::feed_file(files_config())
}

/// The folders a running copy of the mod looks in (MOD/../) that are on this PC: the local mods folder, then the
/// Workshop's. HFP_MODS: tests.
pub fn io_dirs() -> Vec<PathBuf> {
    files::out_slots(files_config()).into_iter().flatten().collect()
}

// ---------------------------------------------------------------- the test voices
fn test_voices() -> Vec<TestVoice> {
    VOICES
        .iter()
        .enumerate()
        .map(|(i, (voice, rate, said))| TestVoice { src: i as i64 + 1, voice: voice.to_string(), rate: *rate, text: said.to_string() })
        .collect()
}

/// The three test voices as wav files: made once with the Windows computer voices (none on other systems - the
/// dummies are then silent). {src: its wav} for those that are there.
pub fn make_voices() -> HashMap<i64, PathBuf> {
    make_voices_in(&work_dir())
}

/// make_voices into this folder (tests: a temp one).
pub fn make_voices_in(work: &Path) -> HashMap<i64, PathBuf> {
    crate::voices::make_in(work, &test_voices(), &|v| format!("voice{}.wav", v.src))
}

// ---------------------------------------------------------------- the game module
/// Teardown's module: the files connector with the built-in profile.
pub type Teardown = FilesGame;

impl FilesGame {
    /// Teardown's module. save: the savegame to read (None: Teardown's own); dirs: the folders for my files (None or
    /// empty: io_dirs()).
    pub fn new(sink: Arc<dyn FeedSink>, log: Log, save: Option<PathBuf>, dirs: Option<Vec<PathBuf>>) -> Teardown {
        FilesGame::with_paths(profile(), true, sink, log, save, dirs)
    }
}

/// Teardown's module made as GameKind::make does (io_dir: --io-dir, the one folder for my files).
pub fn make(sink: Arc<dyn FeedSink>, log: Log, io_dir: Option<PathBuf>) -> Box<dyn Game> {
    Box::new(Teardown::new(sink, log, None, io_dir.map(|d| vec![d])))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The built-in profile is the constants above.
    #[test]
    fn profile_is_teardown() {
        let p = profile();
        assert_eq!((p.id.as_str(), p.game.as_str(), p.mod_name.as_str()), (ID, NAME, MOD_NAME));
        assert_eq!((p.url.as_str(), p.needs.as_str(), p.steam_app), (MOD_URL, NEEDS, Some(APPID)));
        assert!(p.voices && p.speech);
        assert_eq!(p.test_voices, test_voices());
        for (src, name) in NAMES {
            assert_eq!(p.speaker_name(src), name);
        }
        let c = files_config();
        assert_eq!((c.pattern.as_str(), c.tag_pattern.as_str(), c.complete.as_str()), (FEED, MODTAG, "</registry>"));
        assert_eq!((c.prefix.as_str(), c.message), (PREFIX, profile::MessageFormat::TeardownPrefab));
        assert_eq!(c.tag_dirs, vec![("steam-".to_string(), 1)]);
        assert_eq!(c.dirs.len(), 2);
    }
}
