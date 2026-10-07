//! The program's build extras: on Windows its version information (what Explorer's Properties and antivirus programs
//! read); on Linux it finds the speech engine's libraries next to itself ($ORIGIN), as they ship.
fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "linux" {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    }
    if target_os == "windows" {
        let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
        let icon = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default())
            .join("../../assets/kotodama.ico");
        println!("cargo:rerun-if-changed={}", icon.display());
        let mut res = winresource::WindowsResource::new();
        res.set_icon(&icon.to_string_lossy());
        res.set("ProductName", "Kotodama")
            .set(
                "FileDescription",
                "Kotodama: proximity voice chat with live speech to text",
            )
            .set("CompanyName", "Kotodama")
            .set("LegalCopyright", "Copyright (c) 2026 AgeOfAlgorithms (MIT)")
            .set("OriginalFilename", "Kotodama.exe")
            .set("ProductVersion", &version)
            .set("FileVersion", &version);
        if let Err(e) = res.compile() {
            println!("cargo:warning=no version information in the exe: {e}");
        }
    }
}
