//! Which language each stretch of a line is in (detect.rs), for checking a line by hand:
//!     cargo run --release -p kd-translate --example detect_line -- "Vamos a jugar otra vez" "..."
///     ... -- --prefer es,en "Vamos a jugar"      (as with a translation es -> en)
fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut prefer = Vec::new();
    if args.first().is_some_and(|a| a == "--prefer") && args.len() > 1 {
        prefer = args[1].split(',').map(String::from).collect();
        args.drain(..2);
    }
    let prefer: Vec<&str> = prefer.iter().map(String::as_str).collect();
    for line in args {
        let parts: Vec<String> = kd_translate::detect::stretches_preferring(&line, &prefer)
            .iter()
            .map(|s| format!("[{}] {:?}", s.lang.unwrap_or("?"), &line[s.start..s.end]))
            .collect();
        println!("{line:?}\n    {}", parts.join("  "));
    }
}
