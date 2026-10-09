//! The maintainer's release signing tool: the Ed25519 key whose signature over SHA256SUMS.txt the updater checks
//! (kd_update::RELEASE_KEY). Run from app/ (PROJECT.md "Release plan" has the release steps):
//!
//!     cargo run -p kd-update --example release_key -- new-key <private key file>
//!         a new private key (PKCS#8 PEM; refuses to overwrite a file), and the public key as the Rust line to paste
//!         over kd_update::RELEASE_KEY
//!     cargo run -p kd-update --example release_key -- sign <private key file> <SHA256SUMS.txt>
//!         writes <SHA256SUMS.txt>.sig: the signature over the file's exact bytes, 128 hex digits and a newline.
//!         Refuses unless the first line is "# koetama <version>" and the file lists Koetama-Setup-<version>.exe,
//!         and unless the key is the one built into the app (the signature is checked before it is written)
//!     cargo run -p kd-update --example release_key -- verify <SHA256SUMS.txt>
//!         checks <SHA256SUMS.txt>.sig against the built-in key, as the updater will
//!
//! The private key never goes into a repository or CI: it stays on the maintainer's PC, backed up offline.
use ed25519_dalek::pkcs8::{spki::der::pem::LineEnding, DecodePrivateKey, EncodePrivateKey};
use ed25519_dalek::{Signer, SigningKey};
use kd_update::{expected_sha, verify_sums, RELEASE_KEY, SIG};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn read_key(path: &Path) -> Result<SigningKey, String> {
    let pem = std::fs::read_to_string(path).map_err(|e| format!("could not read the key {}: {e}", path.display()))?;
    SigningKey::from_pkcs8_pem(&pem).map_err(|e| format!("{} is not an Ed25519 private key (PKCS#8 PEM): {e}", path.display()))
}

/// The version "# koetama <version>" names (digits and dots), and that the file lists that version's installer.
fn release_version(sums: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(sums).map_err(|_| "the checksums file is not text".to_string())?;
    let first = text.split('\n').next().unwrap_or("").trim_end_matches('\r');
    let version = first.strip_prefix("# koetama ").unwrap_or("");
    let ok = !version.is_empty()
        && version.split('.').all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()));
    if !ok {
        return Err(format!("the first line must be \"# koetama <version>\" (as CI writes it), not {first:?}"));
    }
    let installer = format!("Koetama-Setup-{version}.exe");
    if expected_sha(text, &installer).is_none() {
        return Err(format!("the checksums file does not list {installer}"));
    }
    Ok(version.to_string())
}

fn sig_path(sums: &Path) -> PathBuf {
    let mut s = sums.as_os_str().to_owned();
    s.push(".sig");
    PathBuf::from(s)
}

fn new_key(path: &Path) -> Result<(), String> {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| format!("no randomness from the system: {e}"))?;
    let key = SigningKey::from_bytes(&seed);
    seed.fill(0);
    let pem = key.to_pkcs8_pem(LineEnding::LF).map_err(|e| format!("could not encode the key: {e}"))?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    }
    // (create_new: an existing key is never overwritten - losing it would orphan every installed copy)
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("could not create {} (an existing key is never overwritten): {e}", path.display()))?;
    f.write_all(pem.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| format!("could not write the key: {e}"))?;
    println!("private key: {} (back it up offline; never commit it)", path.display());
    println!("public key - paste over kd_update::RELEASE_KEY (app/crates/kd-update/src/lib.rs):");
    println!("pub const RELEASE_KEY: &str = \"{}\";", hex(key.verifying_key().as_bytes()));
    Ok(())
}

fn sign(key_path: &Path, sums_path: &Path) -> Result<(), String> {
    let key = read_key(key_path)?;
    let sums = std::fs::read(sums_path).map_err(|e| format!("could not read {}: {e}", sums_path.display()))?;
    let version = release_version(&sums)?;
    let sig = format!("{}\n", hex(&key.sign(&sums).to_bytes()));
    // (checked as the updater will, before anything is written: the key must be the built-in one)
    verify_sums(&sums, sig.as_bytes(), RELEASE_KEY, &version).map_err(|e| {
        format!(
            "{e}\n(this key's public key is {}; the app's RELEASE_KEY is {RELEASE_KEY:?})",
            hex(key.verifying_key().as_bytes())
        )
    })?;
    let out = sig_path(sums_path);
    std::fs::write(&out, sig).map_err(|e| format!("could not write {}: {e}", out.display()))?;
    println!("signed {} (koetama {version}): {}", sums_path.display(), out.display());
    println!("upload it as {SIG} (only the .sig - the release's SHA256SUMS.txt must stay byte for byte as signed)");
    Ok(())
}

fn verify(sums_path: &Path) -> Result<(), String> {
    let sums = std::fs::read(sums_path).map_err(|e| format!("could not read {}: {e}", sums_path.display()))?;
    let sp = sig_path(sums_path);
    let sig = std::fs::read(&sp).map_err(|e| format!("could not read {}: {e}", sp.display()))?;
    let version = release_version(&sums)?;
    verify_sums(&sums, &sig, RELEASE_KEY, &version)?;
    println!("good: {} is signed with the built-in release key (koetama {version})", sums_path.display());
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let r = match a.as_slice() {
        ["new-key", key] => new_key(Path::new(key)),
        ["sign", key, sums] => sign(Path::new(key), Path::new(sums)),
        ["verify", sums] => verify(Path::new(sums)),
        _ => Err("usage: release_key new-key <private key file> | sign <private key file> <SHA256SUMS.txt> | \
                  verify <SHA256SUMS.txt>"
            .into()),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
