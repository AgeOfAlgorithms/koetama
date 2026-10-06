//! What every part of Kotodama shares: its name and folders (`paths`), a line's word units and their times (`text`,
//! the same split as the game's voice.lua), the model downloads (`fetch`), the game's state as the mixer and the
//! runtime see it (`feed`), and the log every part writes to (`Log`).
pub mod feed;
pub mod fetch;
pub mod paths;
pub mod text;

use std::sync::Arc;

/// Where a part writes what it is doing: the window's log box, or the command line.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// A log that prints each line (the command line, tests).
pub fn stdout_log() -> Log {
    Arc::new(|s: &str| println!("{s}"))
}

/// A log that drops everything (tests).
pub fn null_log() -> Log {
    Arc::new(|_: &str| {})
}
