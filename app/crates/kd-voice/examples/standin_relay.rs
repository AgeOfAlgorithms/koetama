//! A stand-in relay on 127.0.0.1 for local tests: prints its ws:// address, then serves until it is stopped.
//! tools/voice_e2e.py runs it and points every Koetama there (KOETAMA_RELAY).
//!     cargo run -p kd-voice --example standin_relay
#[path = "../tests/common/standin.rs"]
mod standin;

fn main() {
    let m = standin::mock_relay();
    println!("{}", m.url);
    use std::io::Write;
    let _ = std::io::stdout().flush();
    loop {
        std::thread::park();
    }
}
