//! Windows: the speech engine's DLLs next to the TEST binaries too.
//!
//! sherpa-onnx-sys (feature "shared") copies sherpa-onnx-c-api.dll and onnxruntime.dll into target/<profile>/ - next
//! to a built program (koetama.exe), where Windows looks first. Test binaries live in target/<profile>/deps/: cargo
//! puts target/<profile>/ on their PATH, but Windows resolves a DLL's own imports (sherpa-onnx-c-api.dll ->
//! onnxruntime.dll) through the system folder BEFORE PATH, and Windows 10/11 ship an older onnxruntime.dll in
//! System32 - the wrong one would load. So the DLLs are copied into deps/ as well: every test binary of the
//! workspace that links kd-speech (this crate's, the program's) then finds the right ones next to itself, on any
//! machine (CI too) with no PATH settings. (This build script runs after sherpa-onnx-sys's: a direct dependency
//! with `links` - see Cargo.toml.)
use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let Some(out) = env::var_os("OUT_DIR").map(PathBuf::from) else { return };
    // (OUT_DIR = target/[<triple>/]<profile>/build/kd-speech-<hash>/out)
    let Some(profile) = out.ancestors().nth(3).map(Path::to_path_buf) else { return };
    let deps = profile.join("deps");
    let Ok(entries) = fs::read_dir(&profile) else { return };
    for e in entries.flatten() {
        let src = e.path();
        let name = e.file_name().to_string_lossy().to_lowercase();
        if !(name.ends_with(".dll") && (name.starts_with("sherpa-onnx") || name.starts_with("onnxruntime"))) {
            continue;
        }
        println!("cargo:rerun-if-changed={}", src.display());
        let dest = deps.join(e.file_name());
        let same = match (fs::metadata(&src), fs::metadata(&dest)) {
            (Ok(a), Ok(b)) => a.len() == b.len() && a.modified().ok() <= b.modified().ok(),
            _ => false,
        };
        if same {
            continue;
        }
        let _ = fs::create_dir_all(&deps);
        if let Err(err) = fs::copy(&src, &dest) {
            // (a test still running holds it: the copy there is already the right one, or the next build retries)
            println!("cargo:warning=could not copy {} to {}: {err}", src.display(), deps.display());
        }
    }
}
