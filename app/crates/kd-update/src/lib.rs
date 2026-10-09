//! Updates from GitHub Releases (engine/updater.py): is there a newer Koetama, and (Windows) download its installer,
//! check it, run it.
//!
//! A release is tagged v<version> and has the assets Koetama-Setup-<version>.exe (Windows installer), the Linux
//! build, SHA256SUMS.txt (first line "# koetama <version>", then "<sha256>  <file name>" per line) and
//! SHA256SUMS.txt.sig: the maintainer's Ed25519 signature over SHA256SUMS.txt's exact bytes, made offline with
//! examples/release_key.rs (one line of hex), checked with the public key built in here (RELEASE_KEY). The checksums
//! are used only when that signature is good and their first line names this release's version (an older signed
//! file under a new tag does not pass); then the installer must match them, and - when this copy of Koetama is
//! code-signed - must carry a valid Authenticode signature from the same publisher. Then it runs silently (it
//! replaces the files and starts the new version) and this copy closes. A release that is not signed is not
//! installed from here: check() says why (Release::refused) and the releases page is still offered.
use ed25519_dalek::{Signature, VerifyingKey};
use kd_common::{fetch, paths};
use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// The releases page (the newest).
pub const PAGE: &str = "https://github.com/AgeOfAlgorithms/koetama/releases/latest";
/// where this repo's pages and its releases' files are (anything else a release names is ignored)
pub const REPO_PAGES: &str = "https://github.com/AgeOfAlgorithms/koetama/";
pub const RELEASE_FILES: &str = "https://github.com/AgeOfAlgorithms/koetama/releases/download/";
/// What GitHub's API answers in.
pub const ACCEPT: &str = "application/vnd.github+json";

/// The release key: the Ed25519 public key (hex) whose signature SHA256SUMS.txt must carry. Its private key is the
/// maintainer's, kept offline (PROJECT.md "Release plan"); `cargo run -p kd-update --example release_key -- new-key`
/// printed this line. "": no key - no update is installed from here.
pub const RELEASE_KEY: &str = "d51775904e6ebd0e8d873f8a104bb4e4c76a03fd213727426a51390a19568221";
/// The release's checksums and their signature (asset names).
pub const SUMS: &str = "SHA256SUMS.txt";
pub const SIG: &str = "SHA256SUMS.txt.sig";
/// How every refusal over the signature begins (the window and the selftest show the whole reason).
pub const NOT_SIGNED: &str = "this release is not signed";

static NUMBERS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9]+").unwrap());

/// The latest release, through GitHub's API.
pub fn api_url() -> String {
    format!("https://api.github.com/repos/{}/releases/latest", paths::REPO)
}

/// 'v1.2.10' -> [1, 2, 10] (the first three numbers; none: [0])
pub fn version_tuple(v: &str) -> Vec<u32> {
    let out: Vec<u32> =
        NUMBERS.find_iter(v).take(3).map(|m| m.as_str().parse::<u32>().unwrap_or(u32::MAX)).collect();
    if out.is_empty() {
        vec![0]
    } else {
        out
    }
}

/// latest is a later version than current (as numbers: 0.10 after 0.9; a tag's v ignored)
pub fn newer(latest: &str, current: &str) -> bool {
    version_tuple(latest) > version_tuple(current)
}

/// A release newer than this copy.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Release {
    /// "1.2.3" (the tag without its v)
    pub version: String,
    pub notes: String,
    /// the installer's file name (Koetama-Setup-<version>.exe), if the release has one
    pub installer: Option<String>,
    pub installer_url: Option<String>,
    /// SHA256SUMS.txt
    pub sums_url: Option<String>,
    /// SHA256SUMS.txt.sig (the signature over SHA256SUMS.txt)
    pub sig_url: Option<String>,
    /// the release's page
    pub page: String,
    /// why this release's installer is not offered (set by check(): its signature or checksums do not hold -
    /// NOT_SIGNED ...); the page still is
    pub refused: Option<String>,
}

impl Release {
    /// The installer can be downloaded and run from here: there is one and check() found it in signed checksums.
    pub fn installable(&self) -> bool {
        self.installer_url.is_some() && self.refused.is_none()
    }
}

fn text_of(v: &Value) -> Option<&str> {
    v.as_str()
}

/// The release in GitHub's answer (JSON) if it is newer than current; None: not newer, a draft, a prerelease, or
/// not a release at all.
pub fn parse_release(json: &str, current: &str) -> Option<Release> {
    let rel: Value = serde_json::from_str(json).ok()?;
    release_from(&rel, current)
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn release_from(rel: &Value, current: &str) -> Option<Release> {
    let tag = text_of(&rel["tag_name"]).unwrap_or("");
    if truthy(&rel["draft"]) || truthy(&rel["prerelease"]) || !newer(tag, current) {
        return None;
    }
    // (name -> url; a name seen again keeps its first place and takes the later url, as a Python dict)
    let mut assets: Vec<(String, String)> = Vec::new();
    for a in rel["assets"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let (Some(name), Some(url)) = (text_of(&a["name"]), text_of(&a["browser_download_url"])) else {
            continue;
        };
        match assets.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = url.to_string(),
            None => assets.push((name.to_string(), url.to_string())),
        }
    }
    // (only this release's own installer, from this repo's releases: not an older one under a new tag, nor a file
    // somewhere else)
    let version = tag.trim_start_matches('v').to_string();
    let want = format!("{}-setup-{}.exe", paths::APP_ID, version).to_lowercase();
    let ours = |u: &String| u.starts_with(RELEASE_FILES);
    let assets: Vec<(String, String)> = assets.into_iter().filter(|(_, u)| ours(u)).collect();
    let inst = assets.iter().find(|(n, _)| n.to_lowercase() == want);
    let page = text_of(&rel["html_url"]).filter(|p| p.starts_with(REPO_PAGES)).unwrap_or(PAGE).to_string();
    Some(Release {
        version,
        notes: text_of(&rel["body"]).unwrap_or("").to_string(),
        installer: inst.map(|(n, _)| n.clone()),
        installer_url: inst.map(|(_, u)| u.clone()),
        sums_url: assets.iter().find(|(n, _)| n == SUMS).map(|(_, u)| u.clone()),
        sig_url: assets.iter().find(|(n, _)| n == SIG).map(|(_, u)| u.clone()),
        page,
        refused: None,
    })
}

/// what check() says when GitHub has no release to give (none published, or the repository is gone or private) - not
/// "the latest version": a copy pointed at the wrong place must not look up to date (0.2.0 builds asked the old
/// repository, and said so)
pub const NO_RELEASES: &str = "no releases found";

/// The latest release if it is newer than this copy; else None. Err on network trouble ("could not check: ...") and
/// when there is no release at all (NO_RELEASES ...). A release whose installer would not pass download()'s checks
/// of the signed checksums comes back with `refused` (why): not offered for install, its page still is.
pub fn check(timeout: Duration) -> Result<Option<Release>, String> {
    let text = match fetch::get_text(&api_url(), Some(ACCEPT), timeout) {
        Ok(t) => t,
        // (GitHub's answer while there is no published release - or the repository is not public, or gone)
        Err(e) if e.to_string().contains("404") => return Err(format!("{NO_RELEASES} at github.com/{}", paths::REPO)),
        Err(e) => return Err(format!("could not check: {e}")),
    };
    let rel: Value = serde_json::from_str(&text).map_err(|e| format!("could not check: {e}"))?;
    let mut r = release_from(&rel, paths::VERSION);
    if let Some(r) = r.as_mut() {
        vet(r, RELEASE_KEY, timeout);
    }
    Ok(r)
}

/// Before the installer is offered: the release's signed checksums (signed_sums, with this public key) list it;
/// else `refused` says why. (A release with no installer for this system: nothing to vet - its page is offered.)
pub fn vet(r: &mut Release, key: &str, timeout: Duration) {
    let Some(name) = r.installer.clone().filter(|_| r.installer_url.is_some()) else {
        return;
    };
    r.refused = signed_sums(r, key, timeout).and_then(|sums| listed(&sums, &name)).err();
}

/// The checksum the (signed) SHA256SUMS.txt gives the installer.
fn listed(sums: &str, name: &str) -> Result<String, String> {
    expected_sha(sums, name).ok_or_else(|| format!("the installer is not in {SUMS}"))
}

/// A whole hex string's bytes (either case), or None.
fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.as_bytes();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let digit = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    s.chunks(2).map(|p| Some((digit(p[0])? << 4) | digit(p[1])?)).collect()
}

/// The first line a release's SHA256SUMS.txt must have: "# koetama <version>" (signed with the rest: the checksums
/// are this release's, not an older release's under a new tag).
pub fn sums_header(version: &str) -> String {
    format!("# {} {version}", paths::APP_ID)
}

/// SHA256SUMS.txt's text, if `sig` (SHA256SUMS.txt.sig: the signature's 64 bytes as hex; whitespace around it
/// ignored) is the Ed25519 signature of `key` (the public key as hex: RELEASE_KEY) over the exact bytes of `sums`,
/// and their first line is sums_header(version). Else why not (NOT_SIGNED ... when the signature does not hold).
pub fn verify_sums(sums: &[u8], sig: &[u8], key: &str, version: &str) -> Result<String, String> {
    if key.is_empty() {
        return Err(format!("{NOT_SIGNED} (this build has no release key to check it with)"));
    }
    let key = unhex(key)
        .and_then(|k| <[u8; 32]>::try_from(k).ok())
        .and_then(|k| VerifyingKey::from_bytes(&k).ok())
        .ok_or_else(|| format!("{NOT_SIGNED} (this build's release key is not a valid key)"))?;
    let sig = std::str::from_utf8(sig)
        .ok()
        .and_then(|s| unhex(s.trim()))
        .and_then(|b| <[u8; 64]>::try_from(b).ok())
        .map(|b| Signature::from_bytes(&b))
        .ok_or_else(|| format!("{NOT_SIGNED} ({SIG} is not a signature)"))?;
    // (strict: no weak keys, no second form of the same signature)
    key.verify_strict(sums, &sig)
        .map_err(|_| format!("{NOT_SIGNED} ({SIG} does not match {SUMS} and the release key)"))?;
    let text = std::str::from_utf8(sums).map_err(|_| format!("{SUMS} is not text"))?;
    let first = text.split('\n').next().unwrap_or("").trim_end_matches('\r');
    if version.is_empty() || first != sums_header(version) {
        return Err(format!("the signed {SUMS} is not this release's ({version}): its first line is {first:?}"));
    }
    Ok(text.to_string())
}

/// The release's SHA256SUMS.txt, downloaded with its signature and verified (verify_sums, with this public key) -
/// the only checksums an installer is checked against. Err: why not (no signature: NOT_SIGNED ...).
pub fn signed_sums(r: &Release, key: &str, timeout: Duration) -> Result<String, String> {
    if key.is_empty() {
        return verify_sums(b"", b"", key, &r.version);
    }
    let Some(sums_url) = &r.sums_url else {
        return Err(format!("this release has no {SUMS}"));
    };
    let Some(sig_url) = &r.sig_url else {
        return Err(format!("{NOT_SIGNED} (it has no {SIG})"));
    };
    let get = |url: &str, what: &str| {
        fetch::get_text(url, Some(ACCEPT), timeout).map_err(|e| format!("could not download {what}: {e}"))
    };
    let sums = get(sums_url, SUMS)?;
    let sig = get(sig_url, SIG)?;
    verify_sums(sums.as_bytes(), sig.as_bytes(), key, &r.version)
}

/// A file's SHA-256, in lower-case hex.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Python's str.splitlines(): every kind of line break.
fn split_lines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}') {
            out.push(&s[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r' && it.peek().is_some_and(|&(_, n)| n == '\n') {
                it.next();
                end += 1;
            }
            start = end;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// The checksum SHA256SUMS.txt gives for this file name (lower case), or None.
pub fn expected_sha(sums: &str, name: &str) -> Option<String> {
    for line in split_lines(sums) {
        let parts: Vec<&str> = line.split(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)).filter(|p| !p.is_empty()).collect();
        if parts.len() == 2 && parts[1].trim_start_matches('*') == name {
            return Some(parts[0].to_lowercase());
        }
    }
    None
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Windows: (status, publisher) of a file's Authenticode signature (status "Valid" when it is good), via PowerShell;
/// (None, None) elsewhere or when it cannot be asked.
pub fn signer(path: &Path) -> (Option<String>, Option<String>) {
    if !cfg!(windows) {
        return (None, None);
    }
    let ps = format!(
        "$s = Get-AuthenticodeSignature -LiteralPath '{}'; Write-Output $s.Status; \
         Write-Output $s.SignerCertificate.Subject",
        path.display().to_string().replace('\'', "''")
    );
    let mut cmd = Command::new(paths::powershell());
    cmd.args(["-NoProfile", "-Command", &ps]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    // (Windows PowerShell with its own modules: a PSModulePath inherited from PowerShell 7 - Koetama started from
    //  a pwsh window, or GitHub's runners - points it at modules it cannot load, and Get-AuthenticodeSignature fails)
    cmd.env_remove("PSModulePath");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let Ok(mut child) = cmd.spawn() else {
        return (None, None);
    };
    // (its output is read in a thread of its own; the process gets 30 s)
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        if let Some(s) = stdout.as_mut() {
            let _ = s.read_to_end(&mut out);
        }
        out
    });
    let t0 = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if t0.elapsed() < Duration::from_secs(30) => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return (None, None);
            }
        }
    }
    let out = String::from_utf8_lossy(&reader.join().unwrap_or_default()).into_owned();
    let lines = split_lines(&out);
    let field = |i: usize| lines.get(i).map(|l| l.trim().to_string());
    (field(0), field(1))
}

/// A new empty folder in the temp folder: koetama-<...>.
fn fresh_temp_dir() -> io::Result<PathBuf> {
    let base = std::env::temp_dir();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    for k in 0..100u32 {
        let d = base.join(format!("{}-{}-{:x}{k}", paths::APP_ID, std::process::id(), nanos));
        match std::fs::create_dir(&d) {
            Ok(()) => return Ok(d),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("no fresh temp folder"))
}

fn show(v: &Option<String>) -> &str {
    v.as_deref().unwrap_or("None")
}

/// The release's installer, downloaded (to a fresh temp folder) and checked: its path. progress(0..1) as it comes.
/// Err when a check fails: the signature over SHA256SUMS.txt (RELEASE_KEY) or its version line, the checksum, or -
/// when this program is validly signed - the installer's publisher.
pub fn download(r: &Release, progress: &dyn Fn(f64)) -> Result<PathBuf, String> {
    download_with(r, RELEASE_KEY, progress)
}

/// download(), the checksums' signature checked with this public key (hex) - the tests' own key.
pub fn download_with(r: &Release, key: &str, progress: &dyn Fn(f64)) -> Result<PathBuf, String> {
    let (Some(name), Some(url), Some(_)) = (&r.installer, &r.installer_url, &r.sums_url) else {
        return Err("this release has no installer for this system".into());
    };
    if name.is_empty() || name.contains(['/', '\\', ':']) || name.contains("..") {
        // (a file name from the network: never a path)
        return Err(format!("the installer's name is not a file name: {name}"));
    }
    if name.to_lowercase() != format!("{}-setup-{}.exe", paths::APP_ID, r.version).to_lowercase() {
        return Err(format!("the installer is not this release's ({}): {name}", r.version));
    }
    // (checksums only from a file whose signature and version line hold - never from an unsigned one)
    let sums = signed_sums(r, key, Duration::from_secs(10))?;
    let want = listed(&sums, name)?;
    let dir = fresh_temp_dir().map_err(|e| format!("could not make a temp folder: {e}"))?;
    let path = dir.join(name);
    fetch::download(url, &path, &|done, total| {
        if total > 0 {
            progress(done as f64 / total as f64)
        }
    }, 1)
    .map_err(|e| e.to_string())?;
    let got = sha256_file(&path).map_err(|e| format!("could not read the download: {e}"))?;
    if got != want {
        let _ = std::fs::remove_file(&path); // (never left to be run)
        return Err("the download does not match its checksum".into());
    }
    if cfg!(windows) {
        // (signed builds: the update must be signed by the same publisher)
        let me = std::env::current_exe().map(|p| signer(&p)).unwrap_or((None, None));
        if me.0.as_deref() == Some("Valid") {
            let (st, who) = signer(&path);
            if st.as_deref() != Some("Valid") || who != me.1 {
                let _ = std::fs::remove_file(&path);
                return Err(format!(
                    "the installer is not signed by the same publisher ({}, {})",
                    show(&st),
                    show(&who)
                ));
            }
        }
    }
    Ok(path)
}

/// Run the installer silently, detached (it closes Koetama, replaces it and starts the new version); the caller
/// exits.
pub fn install(path: &Path) -> io::Result<()> {
    let mut cmd = Command::new(path);
    cmd.args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS", "/RELAUNCH=1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(DETACHED_PROCESS); // (no console of its own; the installer shows nothing: /VERYSILENT)
    }
    cmd.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex() {
        assert_eq!(unhex("00fFa1"), Some(vec![0, 255, 0xa1]));
        assert_eq!(unhex(""), Some(vec![]));
        assert!(unhex("abc").is_none() && unhex("zz").is_none() && unhex("+1").is_none());
    }

    #[test]
    fn the_release_key_is_a_key() {
        // (a typo in the built-in key would refuse every update)
        let k = unhex(RELEASE_KEY).and_then(|k| <[u8; 32]>::try_from(k).ok()).expect("64 hex digits");
        assert!(VerifyingKey::from_bytes(&k).is_ok_and(|k| !k.is_weak()));
    }

    #[test]
    fn lines_as_python() {
        assert_eq!(split_lines("a\r\nb\rc\nd\x0be\u{2028}"), ["a", "b", "c", "d", "e"]);
        assert_eq!(split_lines("a\n\nb\n"), ["a", "", "b"]);
        assert!(split_lines("").is_empty());
    }
}
