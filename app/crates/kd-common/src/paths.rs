//! Where Kotodama keeps its things, and its name and version (engine/paths.py).
use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "Kotodama";
pub const APP_ID: &str = "kotodama";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// GitHub: releases, updates
pub const REPO: &str = "AgeOfAlgorithms/proximity-voice-chat-STT-engine";

/// This user's Kotodama folder: settings, downloaded models, the test voices.
pub fn data_dir() -> PathBuf {
    if cfg!(windows) {
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(home);
        return base.join(APP_NAME);
    }
    if cfg!(target_os = "macos") {
        return home().join("Library").join("Application Support").join(APP_NAME);
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local").join("share"));
    base.join(APP_ID)
}

pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

/// The user's home folder.
pub fn home() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The folder the program is in (the install folder: its models/, THIRD_PARTY_NOTICES.txt).
pub fn app_root() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// A developer's build: the repository it was built in (the folder holding engine/asr.py), found from the program's
/// folder or the working folder - for export/ (the language detector export_lid.py writes, the benchmark clips).
/// None in an installed copy.
pub fn repo_root() -> Option<PathBuf> {
    let starts = [Some(app_root()), std::env::current_dir().ok()];
    for start in starts.into_iter().flatten() {
        for dir in start.ancestors() {
            if dir.join("engine").join("asr.py").exists() {
                return Some(dir.to_path_buf());
            }
        }
    }
    None
}
