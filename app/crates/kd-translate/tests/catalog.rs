//! catalog.rs without the network: a canned records list (the shape of Mozilla's), a fake download.
//! Which version a direction uses, the routes (one model with English, two through it, unavailable), the URLs and
//! folders, the downloads (checked, reused, older versions deleted, a broken file refused) and the kept list.
use kd_translate::catalog::{
    fetch_route, load_list, mozilla_code, Catalog, Direction, CDN_URL, LIST_FILE, RECORDS_URL,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn sha(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// The bytes of a fake file (its name and version: every file differs).
fn content(name: &str, version: &str) -> Vec<u8> {
    format!("{name}@{version}").repeat(7).into_bytes()
}

fn location(name: &str, version: &str) -> String {
    format!("main-workspace/translations-models/{version}-{name}")
}

/// A record as Mozilla's list has it.
fn record(from: &str, to: &str, version: &str, kind: &str, name: &str) -> Value {
    let b = content(name, version);
    json!({
        "name": name, "fromLang": from, "toLang": to, "version": version, "fileType": kind, "schema": 1,
        "filter_expression": "", "id": format!("{from}{to}{version}{kind}"), "last_modified": 1,
        "attachment": {"hash": sha(&b), "size": b.len(), "filename": name, "location": location(name, version),
                       "mimetype": "application/octet-stream"}
    })
}

/// A whole direction: model, lex, vocab (or src + trg vocab).
fn direction(from: &str, to: &str, version: &str, split_vocab: bool) -> Vec<Value> {
    let pair = format!("{}{}", from.replace('-', ""), to.replace('-', ""));
    let mut v = vec![
        record(from, to, version, "model", &format!("model.{pair}.intgemm.alphas.bin")),
        record(from, to, version, "lex", &format!("lex.50.50.{pair}.s2t.bin")),
    ];
    if split_vocab {
        v.push(record(from, to, version, "srcvocab", &format!("srcvocab.{pair}.spm")));
        v.push(record(from, to, version, "trgvocab", &format!("trgvocab.{pair}.spm")));
    } else {
        v.push(record(from, to, version, "vocab", &format!("vocab.{pair}.spm")));
    }
    v
}

fn canned() -> String {
    let mut data = Vec::new();
    for (f, t, v, split) in [
        ("ja", "en", "1.0", false),
        ("ja", "en", "2.0", false),
        ("ja", "en", "2.1", false),
        ("ja", "en", "3.0a1", false), // (a pre-release: never picked)
        ("en", "ja", "2.3", true),
        ("ko", "en", "2.1", false),
        ("en", "ko", "2.1", true),
        ("zh-Hans", "en", "2.1", false),
        ("en", "zh-Hans", "2.2", true),
        ("zh-Hant", "en", "2.0", true),
        ("en", "zh-Hant", "2.0", true),
        ("mt", "en", "1.0a1", false), // (Maltese: only a pre-release)
        ("de", "en", "0.9", false),
        ("en", "de", "2.10", false),
        ("en", "de", "2.9", false),
    ] {
        data.extend(direction(f, t, v, split));
    }
    // (de -> en 1.0: incomplete - no vocabulary; 0.9 is used)
    data.push(record("de", "en", "1.0", "model", "model.deen.intgemm.alphas.bin"));
    // (unusable records are left out: an unsafe name, no hash)
    data.push(record("fr", "en", "1.0", "model", "../evil.bin"));
    let mut nohash = record("fr", "en", "1.0", "vocab", "vocab.fren.spm");
    nohash["attachment"]["hash"] = json!("");
    data.push(nohash);
    json!({ "data": data }).to_string()
}

fn cat() -> Catalog {
    Catalog::parse(&canned()).unwrap()
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("kd-translate-catalog-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn newest_numeric_version() {
    let c = cat();
    let d = c.direction("ja", "en").unwrap();
    assert_eq!(d.version, "2.1", "pre-releases are never picked");
    assert_eq!(d.files.len(), 3);
    assert_eq!(c.direction("en", "de").unwrap().version, "2.10", "versions compare as numbers");
    assert_eq!(c.direction("de", "en").unwrap().version, "0.9", "an incomplete version is skipped");
    assert_eq!(c.direction("mt", "en").unwrap().version, "1.0a1", "no release at all: the pre-release");
    let ej = c.direction("en", "ja").unwrap();
    let kinds: Vec<&str> = ej.files.iter().map(|f| f.kind.as_str()).collect();
    assert!(
        kinds.contains(&"srcvocab")
            && kinds.contains(&"trgvocab")
            && kinds.contains(&"model")
            && kinds.contains(&"lex")
    );
    assert!(c.direction("fr", "en").is_none(), "unusable records are left out");
}

#[test]
fn routes() {
    let c = cat();
    let ids = |f: &str, t: &str| c.route(f, t).map(|r| r.iter().map(Direction::id).collect::<Vec<_>>());
    assert_eq!(ids("ja", "en").unwrap(), ["ja-en/2.1"]);
    assert_eq!(ids("en", "ja").unwrap(), ["en-ja/2.3"]);
    assert_eq!(ids("ja", "ko").unwrap(), ["ja-en/2.1", "en-ko/2.1"], "no English side: two models through it");
    assert_eq!(ids("zh", "en").unwrap(), ["zh-Hans-en/2.1"]);
    assert_eq!(ids("en", "zh").unwrap(), ["en-zh-Hans/2.2"]);
    assert_eq!(ids("yue", "en").unwrap(), ["zh-Hant-en/2.0"], "Cantonese: the Traditional Chinese model");
    assert_eq!(ids("yue", "ja").unwrap(), ["zh-Hant-en/2.0", "en-ja/2.3"]);
    assert!(ids("en", "yue").is_err(), "nothing is translated into Cantonese");
    assert_eq!(ids("mt", "en").unwrap(), ["mt-en/1.0a1"], "Maltese: its pre-release");
    assert!(ids("ja", "mt").is_err(), "no en -> mt");
    assert!(ids("fr", "ja").is_err(), "a hop that does not exist");
    assert!(ids("ja", "ja").is_err() && ids("xx", "en").is_err() && ids("en", "auto").is_err());
    assert!(ids("zh", "yue").is_err());
    assert_eq!(mozilla_code("yue", true), Some("zh-Hant"));
}

#[test]
fn urls_and_folders() {
    let c = cat();
    let d = c.direction("zh-Hans", "en").unwrap();
    assert_eq!(d.key(), "zh-Hans-en");
    let root = Path::new("R");
    assert_eq!(d.dir(root), root.join("zh-Hans-en").join("2.1"));
    let m = d.files.iter().find(|f| f.kind == "model").unwrap();
    assert_eq!(m.name, "model.zhHansen.intgemm.alphas.bin");
    assert_eq!(m.url(), format!("{CDN_URL}{}", location(&m.name, "2.1")));
    assert!(m.url().starts_with("https://firefox-settings-attachments.cdn.mozilla.net/main-workspace/"));
    assert_eq!(d.bytes(), d.files.iter().map(|f| f.size).sum::<u64>());
    assert!(RECORDS_URL.ends_with("/collections/translations-models/records"));
}

/// A fake CDN: the canned files by URL; counts the downloads; optionally breaks one file.
struct Cdn {
    files: HashMap<String, Vec<u8>>,
    got: RefCell<Vec<String>>,
    broken: Option<String>,
}

impl Cdn {
    fn new(c: &Catalog, dirs: &[(&str, &str)]) -> Cdn {
        let mut files = HashMap::new();
        for (f, t) in dirs {
            let d = c.direction(f, t).unwrap();
            for x in &d.files {
                files.insert(x.url(), content(&x.name, &d.version));
            }
        }
        Cdn { files, got: RefCell::new(Vec::new()), broken: None }
    }

    fn download(&self, url: &str, dest: &Path, progress: &dyn Fn(u64, u64)) -> io::Result<()> {
        let mut b = self.files.get(url).cloned().ok_or_else(|| io::Error::other("404"))?;
        if self.broken.as_deref().is_some_and(|x| url.ends_with(x)) {
            b[0] ^= 1;
        }
        self.got.borrow_mut().push(url.to_string());
        let half = b.len() / 2;
        progress(half as u64, b.len() as u64);
        std::fs::write(dest, &b)?;
        progress(b.len() as u64, b.len() as u64);
        Ok(())
    }
}

#[test]
fn downloads() {
    let c = cat();
    let root = tmp("dl");
    let route = c.route("ja", "ko").unwrap();
    let cdn = Cdn::new(&c, &[("ja", "en"), ("en", "ko")]);
    let total: u64 = route.iter().map(Direction::bytes).sum();
    // (an older version of ja-en, and a file that is not a version folder: only the first goes)
    let old = root.join("ja-en").join("2.0");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("model.jaen.intgemm.alphas.bin"), b"old").unwrap();
    std::fs::write(root.join("ja-en").join("notes.txt"), b"keep").unwrap();
    let seen = RefCell::new(Vec::new());
    let dirs =
        fetch_route(&root, &route, &|d, t| seen.borrow_mut().push((d, t)), &|u, p, f| cdn.download(u, p, f), &|_| {})
            .unwrap();
    assert_eq!(dirs, [root.join("ja-en").join("2.1"), root.join("en-ko").join("2.1")]);
    for (d, dir) in route.iter().zip(&dirs) {
        for f in &d.files {
            let b = std::fs::read(dir.join(&f.name)).unwrap();
            assert_eq!(b, content(&f.name, &d.version), "the file under its own name");
            assert!(!dir.join(format!("{}.dl", f.name)).exists());
        }
        assert!(d.present(&root));
    }
    assert_eq!(cdn.got.borrow().len(), 7, "3 files of ja-en, 4 of en-ko");
    let seen = seen.into_inner();
    assert!(seen.iter().all(|&(_, t)| t == total), "progress over the rule's files together");
    assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0), "only growing");
    assert_eq!(seen.last().unwrap().0, total);
    assert!(!old.exists(), "the older version is deleted");
    assert!(root.join("ja-en").join("notes.txt").exists(), "only version folders are");

    // (again: everything is there - nothing downloaded, progress at once complete)
    let again = RefCell::new(0u64);
    fetch_route(&root, &route, &|d, _| *again.borrow_mut() = d, &|u, p, f| cdn.download(u, p, f), &|_| {}).unwrap();
    assert_eq!(cdn.got.borrow().len(), 7, "reused");
    assert_eq!(*again.borrow(), total);
}

#[test]
fn a_broken_download_is_refused() {
    let c = cat();
    let root = tmp("broken");
    let route = c.route("ko", "en").unwrap();
    let mut cdn = Cdn::new(&c, &[("ko", "en")]);
    cdn.broken = Some("vocab.koen.spm".into());
    let err = fetch_route(&root, &route, &|_, _| {}, &|u, p, f| cdn.download(u, p, f), &|_| {}).unwrap_err();
    assert!(err.contains("checksum"), "{err}");
    let dir = route[0].dir(&root);
    assert!(!dir.join("vocab.koen.spm").exists() && !dir.join("vocab.koen.spm.dl").exists(), "nothing broken is kept");
    assert!(!route[0].present(&root));
    // (a failed download: the error says which file)
    let empty = Cdn { files: HashMap::new(), got: RefCell::new(Vec::new()), broken: None };
    let err = fetch_route(&tmp("404"), &route, &|_, _| {}, &|u, p, f| empty.download(u, p, f), &|_| {}).unwrap_err();
    assert!(err.contains("could not download") && err.contains("ko-en"), "{err}");
}

#[test]
fn the_list_is_kept_and_fetched_daily() {
    let root = tmp("list");
    let calls = RefCell::new(0);
    let good = |url: &str| -> io::Result<String> {
        assert_eq!(url, RECORDS_URL);
        *calls.borrow_mut() += 1;
        Ok(canned())
    };
    let offline = |_: &str| -> io::Result<String> { Err(io::Error::other("offline")) };
    let day = Duration::from_secs(86400);
    // no list kept, offline: an error
    assert!(load_list(&root, day, &offline, &|_| {}).is_err());
    // fetched and kept
    let c = load_list(&root, day, &good, &|_| {}).unwrap();
    assert!(!c.is_empty() && root.join(LIST_FILE).exists() && *calls.borrow() == 1);
    // fresh: the kept one, no fetch
    load_list(&root, day, &good, &|_| {}).unwrap();
    assert_eq!(*calls.borrow(), 1);
    // old (max age 0): fetched again
    load_list(&root, Duration::ZERO, &good, &|_| {}).unwrap();
    assert_eq!(*calls.borrow(), 2);
    // old and offline: the kept one, said in the log
    let said = RefCell::new(String::new());
    let c = load_list(&root, Duration::ZERO, &offline, &|s| said.borrow_mut().push_str(s)).unwrap();
    assert!(c.direction("ja", "en").is_some() && said.borrow().contains("from before"));
    // a broken answer: the kept one
    let garbage = |_: &str| -> io::Result<String> { Ok("<html>".into()) };
    assert!(load_list(&root, Duration::ZERO, &garbage, &|_| {}).is_ok());
    assert!(Catalog::parse("{}").is_err() && Catalog::parse("nope").is_err());
}
