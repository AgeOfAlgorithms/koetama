//! The updater (test_app.py): versions compare as numbers, a release's installer and checksums found in GitHub's
//! answer, drafts / prereleases / the same version skipped, SHA256SUMS.txt read, and a download from a local web
//! server: a matching checksum accepted, a wrong one refused. No GitHub in these tests (check_github_by_hand: by hand).
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
            {{"name": "SHA256SUMS.txt", "browser_download_url": "{DL}SHA256SUMS.txt"}}]}}"#
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
            page: REL_PAGE.into(),
        }
    );
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
    let sums = format!("{good}  Koetama-Setup-9.9.9.exe\n{}  other.tar.gz\n", "0".repeat(64));
    let (base, files) = serve(vec![("/Koetama-Setup-9.9.9.exe", blob.clone()), ("/SHA256SUMS.txt", sums.into_bytes())]);
    let r = Release {
        version: "9.9.9".into(),
        installer: Some("Koetama-Setup-9.9.9.exe".into()),
        installer_url: Some(format!("{base}/Koetama-Setup-9.9.9.exe")),
        sums_url: Some(format!("{base}/SHA256SUMS.txt")),
        ..Default::default()
    };
    let seen = Mutex::new(Vec::new());
    let path: PathBuf = download(&r, &|p| seen.lock().unwrap().push(p)).expect("the installer downloads");
    assert_eq!(std::fs::read(&path).unwrap(), blob, "... and matches its checksum");
    assert_eq!(path.file_name().unwrap(), "Koetama-Setup-9.9.9.exe");
    let seen = seen.into_inner().unwrap();
    assert!(!seen.is_empty() && seen.iter().all(|p| (0.0..=1.0).contains(p)) && *seen.last().unwrap() == 1.0, "{seen:?}");
    let path2 = download(&r, &|_| {}).unwrap();
    assert_ne!(path.parent(), path2.parent(), "each download in a fresh folder");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
    let _ = std::fs::remove_dir_all(path2.parent().unwrap());

    files.lock().unwrap()[1].1 = format!("{}  Koetama-Setup-9.9.9.exe\n", "f".repeat(64)).into_bytes();
    let err = download(&r, &|_| {}).unwrap_err();
    assert_eq!(err, "the download does not match its checksum");
    files.lock().unwrap()[1].1 = b"nothing here\n".to_vec();
    assert_eq!(download(&r, &|_| {}).unwrap_err(), "the installer is not in SHA256SUMS.txt");
    let none = Release { sums_url: None, ..r.clone() };
    assert_eq!(download(&none, &|_| {}).unwrap_err(), "this release has no installer for this system");
    let sneaky = Release { installer: Some("..\\Koetama-Setup.exe".into()), ..r.clone() };
    assert!(download(&sneaky, &|_| {}).is_err(), "a name that is a path is refused");
    let gone = Release { installer_url: Some(format!("{base}/none.exe")), ..r };
    files.lock().unwrap()[1].1 = format!("{good}  Koetama-Setup-9.9.9.exe\n").into_bytes();
    assert!(download(&gone, &|_| {}).is_err(), "a missing file is an error");
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
