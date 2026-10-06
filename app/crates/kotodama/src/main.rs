//! Kotodama: proximity voice chat with live speech-to-text for games (engine/kotodama.py's window, teardown_helper.py's
//! command line). The work is runtime::Runtime with the chosen game's module (kd_games).
//!
//!     kotodama                 the window
//!     kotodama --cli [...]     the command line, with its test modes (teardown_helper.py's flags)
//!     kotodama --selftest      the build's native parts load: the window, sound, ONNX Runtime + the detector, HTTPS
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cli;
mod console;
mod fonts;
mod gui;
mod instance;
mod mic;
mod runtime;
mod selftest;
mod settings;

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
