//! What every part of Koetama shares: its name and folders (`paths`), a line's word units and their times (`text`,
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

/// A read that ran out of time (nothing came yet) rather than a broken connection. Windows sometimes answers a socket
/// read past its timeout with ERROR_IO_PENDING (997, "Overlapped I/O operation is in progress") instead of
/// WSAETIMEDOUT - taken for a broken connection, it dropped a voice connection for a second.
pub fn timed_out(e: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) || e.raw_os_error() == Some(997)
}
