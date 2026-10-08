//! Koetama: proximity voice chat with live speech-to-text for games (engine/koetama.py's window, teardown_helper.py's
//! command line). The work is runtime::Runtime with the chosen game's module (kd_games).
//!
//!     koetama                 the window
//!     koetama --cli [...]     the command line, with its test modes (teardown_helper.py's flags)
//!     koetama --selftest      the build's native parts load: the window, sound, ONNX Runtime + the detector, HTTPS
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cli;
mod console;
mod dialog;
mod fonts;
mod gui;
mod instance;
mod mic;
mod runtime;
mod selftest;
mod settings;
mod theme;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let code = if args.iter().any(|a| a == "--selftest") {
        console::attach();
        selftest::run()
    } else if args.iter().any(|a| a == "--cli") {
        console::attach();
        let rest: Vec<String> = args.iter().filter(|a| *a != "--cli").cloned().collect();
        cli::main(rest)
    } else {
        gui::main()
    };
    std::process::exit(code);
}
