//! The updater (test_app.py): versions compare as numbers, a release's installer and checksums found in GitHub's
//! answer, drafts / prereleases / the same version skipped, SHA256SUMS.txt read, its signature (a test key: good
//! accepted; tampered, another key, none, another version's or release's file refused), and a download from a local
//! web server: signed checksums and a matching installer accepted, anything else refused. No GitHub in these tests
//! (check_github_by_hand: by hand).
use ed25519_dalek::{Signer, SigningKey};
use kd_update::*;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn versions() {
    assert_eq!(version_tuple("v1.2.10"), vec![1, 2, 10]);
    assert_eq!(version_tuple(""), vec![0]);
    assert_eq!(version_tuple("v2"), vec![2]);
    assert_eq!(version_tuple("1.2.3.4"), vec![1, 2, 3]);
    assert!(newer("v0.2.0", "0.1.9") && !newer("v0.1.0", "0.1.0") && newer("v0.10.0", "0.9.9"));
    assert!(newer("v1.0", "0.9.9") && !newer("v1.0", "1.0.0") && newer("v1.0.0", "1.0"));
    assert!(!newer("garbage", "0.0.1"));
    assert_eq!(PAGE, format!("https://github.com/{}/releases/latest", kd_common::paths::REPO));
    assert_eq!(api_url(), format!("https://api.github.com/repos/{}/releases/latest", kd_common::paths::REPO));
}

const DL: &str = "https://github.com/AgeOfAlgorithms/koetama/releases/download/v9.9.9/";
const REL_PAGE: &str = "https://github.com/AgeOfAlgorithms/koetama/releases/tag/v9.9.9";

fn rel(tag: &str, extra: &str) -> String {
    format!(
        r#"{{"tag_name": "{tag}", "body": "notes", "html_url": "{REL_PAGE}", {extra} "assets": [
            {{"name": "Koetama-Setup-9.9.9.exe", "browser_download_url": "{DL}Koetama-Setup-9.9.9.exe"}},
            {{"name": "Koetama-9.9.9-linux.tar.gz", "browser_download_url": "{DL}Koetama-9.9.9-linux.tar.gz"}},
            {{"name": "SHA256SUMS.txt", "browser_download_url": "{DL}SHA256SUMS.txt"}},
            {{"name": "SHA256SUMS.txt.sig", "browser_download_url": "{DL}SHA256SUMS.txt.sig"}}]}}"#
    )
}

#[test]
fn releases() {
    let cur = kd_common::paths::VERSION;
    let r = parse_release(&rel("v9.9.9", ""), cur).expect("a newer release");
    assert_eq!(
        r,
        Release {
            version: "9.9.9".into(),
            notes: "notes".into(),
            installer: Some("Koetama-Setup-9.9.9.exe".into()),
            installer_url: Some(format!("{DL}Koetama-Setup-9.9.9.exe")),
            sums_url: Some(format!("{DL}SHA256SUMS.txt")),
            sig_url: Some(format!("{DL}SHA256SUMS.txt.sig")),
            page: REL_PAGE.into(),
            refused: None,
        }
    );
    assert!(r.installable());
    assert!(parse_release(&rel(&format!("v{cur}"), ""), cur).is_none(), "the same version: no update");
    assert!(parse_release(&rel("v9.9.9", r#""draft": true,"#), cur).is_none(), "a draft release: no update");
    assert!(parse_release(&rel("v9.9.9", r#""prerelease": true,"#), cur).is_none(), "a prerelease: no update");
    assert!(parse_release(&rel("v9.9.9", r#""draft": false, "prerelease": null,"#), cur).is_some());
    assert!(parse_release("not json", cur).is_none() && parse_release("[]", cur).is_none());
    assert!(parse_release(r#"{"message": "Not Found"}"#, cur).is_none());
    // (odd assets skipped; the installer only under this version's own name (any case), only from this repo's
    // releases; no page, or one elsewhere: the releases page)
    let r = parse_release(
        &format!(
            r#"{{"tag_name": "v9.9.9", "body": null, "assets": [{{"name": 5}}, {{"name": "koetama-setup.zip",
            "browser_download_url": "{DL}z"}}, {{"name": "KOETAMA-SETUP-9.9.9.EXE", "browser_download_url": "{DL}E"}}]}}"#
        ),
        cur,
    )
    .unwrap();
    assert_eq!((r.installer.as_deref(), r.installer_url.as_deref(), r.sums_url), (Some("KOETAMA-SETUP-9.9.9.EXE"), Some(format!("{DL}E").as_str()), None));
    assert_eq!((r.notes.as_str(), r.page.as_str()), ("", PAGE));
    let r = parse_release(
        r#"{"tag_name": "v9.9.9", "html_url": "https://evil.example/x", "assets": [
            {"name": "Koetama-Setup-9.9.9.exe", "browser_download_url": "https://evil.example/Koetama-Setup-9.9.9.exe"},
            {"name": "SHA256SUMS.txt", "browser_download_url": "https://evil.example/SHA256SUMS.txt"}]}"#,
        cur,
    )
    .unwrap();
    assert_eq!((r.installer, r.sums_url, r.page.as_str()), (None, None, PAGE), "files and pages elsewhere: ignored");
    let r = parse_release(
        &format!(r#"{{"tag_name": "v9.9.9", "assets": [{{"name": "Koetama-Setup-9.9.9.exe", "browser_download_url": "{DL}Koetama-Setup-9.9.9.exe"}},
            {{"name": "SHA256SUMS.txt", "browser_download_url": "{DL}SHA256SUMS.txt"}},
            {{"name": "SHA256SUMS.txt.sig", "browser_download_url": "https://evil.example/SHA256SUMS.txt.sig"}}]}}"#),
        cur,
    )
    .unwrap();
    assert_eq!(r.sig_url, None, "a signature elsewhere: ignored");
    let r = parse_release(
        &format!(r#"{{"tag_name": "v9.9.9", "assets": [{{"name": "Koetama-Setup-0.1.0.exe", "browser_download_url": "{DL}Koetama-Setup-0.1.0.exe"}}]}}"#),
        cur,
    )
    .unwrap();
    assert_eq!(r.installer, None, "an older installer under a new tag: not this release's");
}

#[test]
fn sums() {
    let good = "ab".repeat(32);
    let text = format!("{}  Koetama-Setup-9.9.9.exe\r\n{}  other.tar.gz\n{} *bin.exe\n", good.to_uppercase(), "0".repeat(64), "1".repeat(64));
    assert_eq!(expected_sha(&text, "Koetama-Setup-9.9.9.exe"), Some(good));
    assert_eq!(expected_sha(&text, "bin.exe"), Some("1".repeat(64)));
    assert_eq!(expected_sha(&text, "missing.exe"), None);
    assert_eq!(expected_sha("a b c\n", "b"), None);
}

/// The tests' signing key (never the release key) and another one.
fn test_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

fn other_key() -> SigningKey {
    SigningKey::from_bytes(&[8u8; 32])
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn public(k: &SigningKey) -> String {
    hex(k.verifying_key().as_bytes())
}

/// SHA256SUMS.txt.sig for these bytes, as release_key.rs writes it.
fn sign(k: &SigningKey, sums: &[u8]) -> Vec<u8> {
    format!("{}\n", hex(&k.sign(sums).to_bytes())).into_bytes()
}

fn sums_for(version: &str, sha: &str) -> String {
    format!("# koetama {version}\n{sha}  Koetama-Setup-{version}.exe\n{}  Koetama-{version}-linux.tar.gz\n", "0".repeat(64))
}

#[test]
fn signatures() {
    let (k, pk) = (test_key(), public(&test_key()));
    let sums = sums_for("9.9.9", &"ab".repeat(32));
    let sig = sign(&k, sums.as_bytes());
    assert_eq!(verify_sums(sums.as_bytes(), &sig, &pk, "9.9.9"), Ok(sums.clone()), "a good signature: the checksums");
    assert_eq!(sums_header("9.9.9"), "# koetama 9.9.9");
    let crlf = sums.replace('\n', "\r\n");
    assert!(verify_sums(crlf.as_bytes(), &sign(&k, crlf.as_bytes()), &pk, "9.9.9").is_ok(), "CRLF lines too");
    let spaced = format!("  {}  \r\n", String::from_utf8(sig.clone()).unwrap().trim());
    assert!(verify_sums(sums.as_bytes(), spaced.as_bytes(), &pk, "9.9.9").is_ok(), "whitespace around the hex");
    assert!(verify_sums(sums.as_bytes(), String::from_utf8(sig.clone()).unwrap().to_uppercase().as_bytes(), &pk, "9.9.9").is_ok());

    let refused = |sums: &[u8], sig: &[u8], key: &str, version: &str| {
        let e = verify_sums(sums, sig, key, version).unwrap_err();
        assert!(e.starts_with(NOT_SIGNED), "{e}");
    };
    // (tampered: one checksum changed, a line added, a byte flipped)
    let tampered = sums.replace(&"ab".repeat(32), &"cd".repeat(32));
    refused(tampered.as_bytes(), &sig, &pk, "9.9.9");
    refused(format!("{sums}{}  evil.exe\n", "1".repeat(64)).as_bytes(), &sig, &pk, "9.9.9");
    let mut flipped = sums.clone().into_bytes();
    flipped[20] ^= 1;
    refused(&flipped, &sig, &pk, "9.9.9");
    // (another key; no key built in; the built-in release key - not the test key)
    refused(sums.as_bytes(), &sign(&other_key(), sums.as_bytes()), &pk, "9.9.9");
    refused(sums.as_bytes(), &sig, &public(&other_key()), "9.9.9");
    refused(sums.as_bytes(), &sig, "", "9.9.9");
    refused(sums.as_bytes(), &sig, RELEASE_KEY, "9.9.9");
    refused(sums.as_bytes(), &sig, "not hex", "9.9.9");
    // (no signature / not one: empty, garbage, too short, the signature's last byte changed)
    refused(sums.as_bytes(), b"", &pk, "9.9.9");
    refused(sums.as_bytes(), b"hello\n", &pk, "9.9.9");
    refused(sums.as_bytes(), &sig[..100], &pk, "9.9.9");
    let mut bad = sig.clone();
    bad[127] = if bad[127] == b'0' { b'1' } else { b'0' };
    refused(sums.as_bytes(), &bad, &pk, "9.9.9");
    // (signed, but not this release's: another version's first line, no version line, a different version line)
    let e = verify_sums(sums.as_bytes(), &sig, &pk, "9.9.10").unwrap_err();
    assert!(e.contains("not this release's"), "{e}");
    let bare = format!("{}  Koetama-Setup-9.9.9.exe\n", "ab".repeat(32));
    assert!(verify_sums(bare.as_bytes(), &sign(&k, bare.as_bytes()), &pk, "9.9.9").is_err(), "no version line");
    for first in ["# koetama 9.9.9 ", "# koetama 9.9.9.1", "#koetama 9.9.9", "# Koetama 9.9.9", "", "# koetama "] {
        let s = format!("{first}\n{}  Koetama-Setup-9.9.9.exe\n", "ab".repeat(32));
        assert!(verify_sums(s.as_bytes(), &sign(&k, s.as_bytes()), &pk, "9.9.9").is_err(), "{first:?}");
    }
    assert!(verify_sums(sums.as_bytes(), &sig, &pk, "").is_err(), "no version");
}

/// A tiny web server: path -> body (404 otherwise).
type Files = Arc<Mutex<Vec<(&'static str, Vec<u8>)>>>;

fn serve(files: Vec<(&'static str, Vec<u8>)>) -> (String, Files) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let files = Arc::new(Mutex::new(files));
    let f2 = files.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut first = String::new();
            let _ = r.read_line(&mut first);
            loop {
                let mut h = String::new();
                if r.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                    break;
                }
            }
            let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
            let body = f2.lock().unwrap().iter().find(|(p, _)| *p == path).map(|(_, b)| b.clone());
            match body {
                Some(b) => {
                    let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len());
                    let _ = s.write_all(&b);
                }
                None => {
                    let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                }
            }
        }
    });
    (base, files)
}

#[test]
fn downloads_checked() {
    let blob: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2654435761) >> 11) as u8).collect();
    let probe = std::env::temp_dir().join(format!("kd-update-probe-{}", std::process::id()));
    std::fs::write(&probe, &blob).unwrap();
    let good = sha256_file(&probe).unwrap();
    let _ = std::fs::remove_file(&probe);
    assert_eq!(good.len(), 64);
    let (k, pk) = (test_key(), public(&test_key()));
    let sums = sums_for("9.9.9", &good);
    let sig = sign(&k, sums.as_bytes());
    let (base, files) = serve(vec![
        ("/Koetama-Setup-9.9.9.exe", blob.clone()),
        ("/SHA256SUMS.txt", sums.clone().into_bytes()),
        ("/SHA256SUMS.txt.sig", sig.clone()),
    ]);
    // (files[1]: the checksums, files[2]: their signature - set together)
    let put = |sums: &str, sig: Vec<u8>| {
        let mut f = files.lock().unwrap();
        f[1].1 = sums.as_bytes().to_vec();
        f[2].1 = sig;
    };
    let r = Release {
        version: "9.9.9".into(),
        installer: Some("Koetama-Setup-9.9.9.exe".into()),
        installer_url: Some(format!("{base}/Koetama-Setup-9.9.9.exe")),
        sums_url: Some(format!("{base}/SHA256SUMS.txt")),
        sig_url: Some(format!("{base}/SHA256SUMS.txt.sig")),
        ..Default::default()
    };
    let seen = Mutex::new(Vec::new());
    let path: PathBuf = download_with(&r, &pk, &|p| seen.lock().unwrap().push(p)).expect("the installer downloads");
    assert_eq!(std::fs::read(&path).unwrap(), blob, "... and matches its checksum");
    assert_eq!(path.file_name().unwrap(), "Koetama-Setup-9.9.9.exe");
    let seen = seen.into_inner().unwrap();
    assert!(!seen.is_empty() && seen.iter().all(|p| (0.0..=1.0).contains(p)) && *seen.last().unwrap() == 1.0, "{seen:?}");
    let path2 = download_with(&r, &pk, &|_| {}).unwrap();
    assert_ne!(path.parent(), path2.parent(), "each download in a fresh folder");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
    let _ = std::fs::remove_dir_all(path2.parent().unwrap());
    let mut v = r.clone();
    vet(&mut v, &pk, Duration::from_secs(10));
    assert!(v.refused.is_none() && v.installable(), "{:?}", v.refused);

    // (the signature: by the built-in release key (not the test key), none, another key, none at all built in)
    let refused = |r: &Release, key: &str| {
        let e = download_with(r, key, &|_| {}).unwrap_err();
        assert!(e.starts_with(NOT_SIGNED), "{e}");
        let mut v = r.clone();
        vet(&mut v, key, Duration::from_secs(10));
        assert!(v.refused.as_deref() == Some(e.as_str()) && !v.installable(), "{:?}", v.refused);
    };
    let e = download(&r, &|_| {}).unwrap_err();
    assert!(e.starts_with(NOT_SIGNED), "the release key did not sign the test files: {e}");
    refused(&Release { sig_url: None, ..r.clone() }, &pk);
    refused(&r, &public(&other_key()));
    refused(&r, "");
    put(&sums, sign(&other_key(), sums.as_bytes()));
    refused(&r, &pk);
    put(&sums, Vec::new());
    refused(&r, &pk);
    // (a .sig named but not there: an error, never "unsigned is fine")
    let missing = Release { sig_url: Some(format!("{base}/nothing.sig")), ..r.clone() };
    assert!(download_with(&missing, &pk, &|_| {}).unwrap_err().starts_with("could not download SHA256SUMS.txt.sig"));
    // (tampered checksums: the old signature no longer holds)
    let evil = sums_for("9.9.9", &"f".repeat(64));
    put(&evil, sig.clone());
    refused(&r, &pk);
    // (signed, but the installer does not match: a wrong checksum in a signed file)
    put(&evil, sign(&k, evil.as_bytes()));
    assert_eq!(download_with(&r, &pk, &|_| {}).unwrap_err(), "the download does not match its checksum");
    // (another release's signed checksums under this tag - here the same installer bytes: still refused)
    let old = sums_for("9.9.8", &good);
    put(&old, sign(&k, old.as_bytes()));
    let e = download_with(&r, &pk, &|_| {}).unwrap_err();
    assert!(e.contains("not this release's"), "{e}");
    let mut v = r.clone();
    vet(&mut v, &pk, Duration::from_secs(10));
    assert!(!v.installable());
    // (an older installer offered under the new version: not this release's)
    let older = Release { installer: Some("Koetama-Setup-9.9.8.exe".into()), ..r.clone() };
    assert!(download_with(&older, &pk, &|_| {}).unwrap_err().contains("not this release's"));
    // (signed for this version, but without the installer)
    let other = format!("# koetama 9.9.9\n{good}  other.tar.gz\n");
    put(&other, sign(&k, other.as_bytes()));
    assert_eq!(download_with(&r, &pk, &|_| {}).unwrap_err(), "the installer is not in SHA256SUMS.txt");

    let none = Release { sums_url: None, ..r.clone() };
    assert_eq!(download_with(&none, &pk, &|_| {}).unwrap_err(), "this release has no installer for this system");
    let sneaky = Release { installer: Some("..\\Koetama-Setup.exe".into()), ..r.clone() };
    assert!(download_with(&sneaky, &pk, &|_| {}).is_err(), "a name that is a path is refused");
    put(&sums, sig.clone());
    let gone = Release { installer_url: Some(format!("{base}/none.exe")), ..r.clone() };
    assert!(download_with(&gone, &pk, &|_| {}).is_err(), "a missing file is an error");
    // (no installer for this system: nothing to vet, the page is offered)
    let mut page_only = Release { installer: None, installer_url: None, ..r };
    vet(&mut page_only, &pk, Duration::from_secs(10));
    assert!(page_only.refused.is_none() && !page_only.installable());
}

/// (Windows) an unsigned file: not Valid; a file that is not there: no subject.
#[test]
#[cfg(windows)]
fn signer_of_an_unsigned_file() {
    let p = std::env::temp_dir().join(format!("kd-update-it's-{}.txt", std::process::id()));
    std::fs::write(&p, "hello").unwrap();
    let (st, who) = signer(&p);
    assert!(st.is_some() && st.as_deref() != Some("Valid"), "{st:?} {who:?}");
    let _ = std::fs::remove_file(&p);
    let (st, _) = signer(&PathBuf::from(r"C:\Windows\System32\kernel32.dll"));
    println!("kernel32.dll: {st:?}");
}

/// By hand: the real GitHub API (cargo test -p kd-update -- --ignored --nocapture).
#[test]
#[ignore]
fn check_github_by_hand() {
    println!("{:?}", check(Duration::from_secs(10)));
}
