//! Mozilla's translation models (Firefox Translations): which exist, which files a rule needs, and the downloads.
//!
//! The list is Mozilla's Remote Settings collection `translations-models` (RECORDS_URL): one record per file, with its
//! direction (fromLang, toLang - Mozilla's codes), version, fileType (model, lex, vocab or srcvocab + trgvocab), name
//! and attachment (location on CDN_URL, size, sha256). A direction is used at its NEWEST purely numeric version
//! ("2.1"); a pre-release ("1.0a1") only for a direction with no release at all (Maltese -> English). The list is kept in `<root>/models.json` and fetched again at most
//! once a day (LIST_MAX_AGE); offline, the kept one is used.
//!
//! Koetama's language codes are Mozilla's except zh -> zh-Hans, and yue (Cantonese has no model) -> zh-Hant, as a
//! SOURCE only: written Cantonese is translated with the Traditional Chinese model; nothing is translated into it.
//! Mozilla's models all have English on one side: a rule A -> B uses one direction when A or B is English, else two
//! (A -> en, en -> B); it is unavailable when a direction it needs does not exist.
//!
//! Files go to `<root>/<from>-<to>/<version>/<the file's own name>` (root: Koetama's data folder/translate), each
//! downloaded to `<name>.dl` (fetch::download: `.part` while it comes, resumed after a break), checked against its
//! sha256, then renamed: a file under its own name is whole and checked. Older versions of a direction are deleted
//! once the new one is there.
use kd_common::{fetch, paths};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Mozilla's list of translation model files (Remote Settings).
pub const RECORDS_URL: &str =
    "https://firefox.settings.services.mozilla.com/v1/buckets/main/collections/translations-models/records";
/// Where the files are: CDN_URL + attachment.location.
pub const CDN_URL: &str = "https://firefox-settings-attachments.cdn.mozilla.net/";
/// the list is fetched again when the kept one is older than this
pub const LIST_MAX_AGE: Duration = Duration::from_secs(24 * 3600);
/// the kept list, in the root folder
pub const LIST_FILE: &str = "models.json";

/// Koetama's translation folder: its data folder/translate.
pub fn root() -> PathBuf {
    paths::data_dir().join("translate")
}

/// Mozilla's code for one of Koetama's languages, as the source (`source`) or the target of a translation; None:
/// Mozilla has none (yue as a target, a language Koetama does not have).
pub fn mozilla_code(lang: &str, source: bool) -> Option<&'static str> {
    match lang {
        "zh" => Some("zh-Hans"),
        "yue" => source.then_some("zh-Hant"),
        l => crate::detect::LANGS.iter().find(|&&x| x == l && x != "yue").copied(),
    }
}

/// A version as numbers ("2.1" -> [2, 1]); None for anything else (a pre-release "1.0a1", "", "1..2").
pub fn numeric_version(v: &str) -> Option<Vec<u64>> {
    if v.is_empty() {
        return None;
    }
    v.split('.')
        .map(
            |p| {
                if !p.is_empty() && p.len() <= 9 && p.bytes().all(|c| c.is_ascii_digit()) {
                    p.parse().ok()
                } else {
                    None
                }
            },
        )
        .collect()
}

/// A name that is safe as a file or folder name here (no separators, no "..", nothing hidden).
fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && !s.starts_with('.')
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_'))
}

/// One file of a direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelFile {
    /// its own name: model.jaen.intgemm.alphas.bin
    pub name: String,
    /// model, lex, vocab, srcvocab, trgvocab
    pub kind: String,
    pub size: u64,
    /// lower-case hex
    pub sha256: String,
    /// where on the CDN
    pub location: String,
}

impl ModelFile {
    pub fn url(&self) -> String {
        format!("{CDN_URL}{}", self.location)
    }
}

/// One translation direction at one version: Mozilla's codes, the files it is made of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Direction {
    pub from: String,
    pub to: String,
    pub version: String,
    pub files: Vec<ModelFile>,
}

impl Direction {
    /// "ja-en", "zh-Hans-en"
    pub fn key(&self) -> String {
        format!("{}-{}", self.from, self.to)
    }

    /// "ja-en/2.1": a loaded model's name (a new version is another model)
    pub fn id(&self) -> String {
        format!("{}/{}", self.key(), self.version)
    }

    /// Where its files go: <root>/<from>-<to>/<version>
    pub fn dir(&self, root: &Path) -> PathBuf {
        root.join(self.key()).join(&self.version)
    }

    /// all its files' bytes
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// Its files are all there (a reused download: the right size under its own name - only a checked file gets it).
    pub fn present(&self, root: &Path) -> bool {
        let d = self.dir(root);
        self.files.iter().all(|f| std::fs::metadata(d.join(&f.name)).is_ok_and(|m| m.is_file() && m.len() == f.size))
    }
}

#[derive(Clone, Debug)]
struct Record {
    from: String,
    to: String,
    version: String,
    file: ModelFile,
}

/// Mozilla's list of model files.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    records: Vec<Record>,
}

impl Catalog {
    /// The records JSON ({"data": [...]}); records it cannot use (missing fields, unsafe names) are left out.
    pub fn parse(json: &str) -> Result<Catalog, String> {
        let v: Value =
            serde_json::from_str(json).map_err(|e| format!("the list of translation models is not JSON: {e}"))?;
        let data = v.get("data").and_then(Value::as_array).ok_or("the list of translation models has no \"data\"")?;
        let mut records = Vec::new();
        for r in data {
            let s = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let a = r.get("attachment").unwrap_or(&Value::Null);
            let file = ModelFile {
                name: s("name"),
                kind: s("fileType"),
                size: a.get("size").and_then(Value::as_u64).unwrap_or(0),
                sha256: a.get("hash").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase(),
                location: a.get("location").and_then(Value::as_str).unwrap_or("").to_string(),
            };
            let (from, to, version) = (s("fromLang"), s("toLang"), s("version"));
            let good = safe_name(&from)
                && safe_name(&to)
                && safe_name(&file.name)
                && !file.name.ends_with(".dl")
                && !file.name.ends_with(".part")
                && file.size > 0
                && file.sha256.len() == 64
                && file.sha256.bytes().all(|c| c.is_ascii_hexdigit())
                && !file.location.is_empty()
                && !file.location.contains("..");
            if good {
                records.push(Record { from, to, version, file });
            }
        }
        Ok(Catalog { records })
    }

    /// How many usable records.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// A direction (Mozilla's codes) at its newest numeric version that has a whole model (a model file and a vocab,
    /// or a source and a target vocab; the lexical shortlist when there is one) - or, when no release has one, its
    /// newest pre-release (Maltese -> English is only "1.0a1"; chrF 63.6 on the benchmark); None if there is none.
    pub fn direction(&self, from: &str, to: &str) -> Option<Direction> {
        // (releases first, newest first; then the pre-releases, newest first)
        let mut versions: Vec<(bool, Vec<u64>, &str)> = Vec::new();
        for r in self.records.iter().filter(|r| r.from == from && r.to == to) {
            if !versions.iter().any(|(_, _, v)| *v == r.version) && safe_name(&r.version) {
                let n = numeric_version(&r.version);
                versions.push((n.is_some(), n.unwrap_or_default(), &r.version));
            }
        }
        versions.sort();
        for (_, _, version) in versions.iter().rev() {
            let mut files: Vec<ModelFile> = Vec::new();
            for r in self.records.iter().filter(|r| r.from == from && r.to == to && r.version == *version) {
                // (one file of each kind, and no two with one name)
                if !files.iter().any(|f| f.kind == r.file.kind || f.name == r.file.name) {
                    files.push(r.file.clone());
                }
            }
            let has = |k: &str| files.iter().any(|f| f.kind == k);
            if has("model") && (has("vocab") || (has("srcvocab") && has("trgvocab"))) {
                files.retain(|f| matches!(f.kind.as_str(), "model" | "lex" | "vocab" | "srcvocab" | "trgvocab"));
                files.sort_by(|a, b| a.name.cmp(&b.name));
                return Some(Direction { from: from.into(), to: to.into(), version: version.to_string(), files });
            }
        }
        None
    }

    /// The directions a rule `from` -> `to` (Koetama's codes) needs, in order: one when either side is English, else
    /// two through English. Err: why it is unavailable (a sentence for the log).
    pub fn route(&self, from: &str, to: &str) -> Result<Vec<Direction>, String> {
        if from == to {
            return Err(format!("{from} > {to}: the same language"));
        }
        let a = mozilla_code(from, true).ok_or(format!("{from} > {to}: no translation model from {from}"))?;
        let b = mozilla_code(to, false).ok_or(format!("{from} > {to}: no translation model into {to}"))?;
        if a == b {
            return Err(format!("{from} > {to}: one model language"));
        }
        let hops: Vec<(&str, &str)> = if a == "en" || b == "en" { vec![(a, b)] } else { vec![(a, "en"), ("en", b)] };
        hops.iter()
            .map(|&(x, y)| self.direction(x, y).ok_or(format!("{from} > {to}: Mozilla has no {x} -> {y} model")))
            .collect()
    }
}

/// The list: the kept one (<root>/models.json) while it is younger than max_age, else fetched with `get` (and kept);
/// a fetch that fails falls back on the kept one, however old.
pub fn load_list(
    root: &Path,
    max_age: Duration,
    get: &dyn Fn(&str) -> io::Result<String>,
    log: &dyn Fn(&str),
) -> Result<Catalog, String> {
    let kept = root.join(LIST_FILE);
    let age = std::fs::metadata(&kept)
        .and_then(|m| m.modified())
        .ok()
        .map(|t| SystemTime::now().duration_since(t).unwrap_or_default());
    let read_kept =
        || std::fs::read_to_string(&kept).ok().and_then(|t| Catalog::parse(&t).ok()).filter(|c| !c.is_empty());
    if age.is_some_and(|a| a < max_age) {
        if let Some(c) = read_kept() {
            return Ok(c);
        }
    }
    match get(RECORDS_URL).map_err(|e| e.to_string()).and_then(|text| Catalog::parse(&text).map(|c| (c, text))) {
        Ok((c, text)) if !c.is_empty() => {
            let tmp = root.join(format!("{LIST_FILE}.tmp"));
            let saved = std::fs::create_dir_all(root)
                .and_then(|_| std::fs::write(&tmp, text))
                .and_then(|_| std::fs::rename(&tmp, &kept));
            if let Err(e) = saved {
                log(&format!("translation: could not keep the model list in {}: {e}", kept.display()));
            }
            Ok(c)
        }
        got => {
            let why = match got {
                Ok(_) => "it is empty".to_string(),
                Err(e) => e,
            };
            match read_kept() {
                Some(c) => {
                    log(&format!("translation: could not get Mozilla's model list ({why}); using the one from before"));
                    Ok(c)
                }
                None => Err(format!("could not get Mozilla's list of translation models: {why}")),
            }
        }
    }
}

/// The list over the internet (fetch::get_text).
pub fn get_list(url: &str) -> io::Result<String> {
    fetch::get_text(url, Some("application/json"), Duration::from_secs(30))
}

/// A file over the internet (fetch::download: resumed after a break, three tries).
pub fn download_file(url: &str, dest: &Path, progress: &dyn Fn(u64, u64)) -> io::Result<()> {
    fetch::download(url, dest, progress, 3).map(|_| ())
}

/// A file's sha256, lower-case hex.
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

/// How a file is downloaded: (url, where to, progress(done, total of that file)).
pub type Download<'a> = &'a dyn Fn(&str, &Path, &dyn Fn(u64, u64)) -> io::Result<()>;

/// The folders of these directions with all their files there: the missing ones downloaded (`download`: url, dest,
/// progress(done, total of that file)), each checked against its sha256; then the other versions of each direction
/// deleted. progress(done, total): bytes across all the directions' files (those already there count as done).
pub fn fetch_route(
    root: &Path,
    dirs: &[Direction],
    progress: &dyn Fn(u64, u64),
    download: Download,
    log: &dyn Fn(&str),
) -> Result<Vec<PathBuf>, String> {
    let total: u64 = dirs.iter().map(Direction::bytes).sum();
    let mut done: u64 = 0;
    let mut out = Vec::new();
    for d in dirs {
        let folder = d.dir(root);
        std::fs::create_dir_all(&folder).map_err(|e| format!("cannot make {}: {e}", folder.display()))?;
        for f in &d.files {
            let dest = folder.join(&f.name);
            if std::fs::metadata(&dest).is_ok_and(|m| m.is_file() && m.len() == f.size) {
                done += f.size;
                progress(done, total);
                continue;
            }
            let tmp = folder.join(format!("{}.dl", f.name));
            let before = done;
            let t0 = std::time::Instant::now();
            download(&f.url(), &tmp, &|got, _| progress(before + got.min(f.size), total))
                .map_err(|e| format!("could not download {} ({}): {e}", f.name, d.key()))?;
            let sum = sha256_file(&tmp).map_err(|e| format!("cannot read {}: {e}", tmp.display()))?;
            if sum != f.sha256 {
                let _ = std::fs::remove_file(&tmp);
                return Err(format!(
                    "{} ({}) came broken (its checksum is wrong): deleted, it is downloaded again next time",
                    f.name,
                    d.key()
                ));
            }
            std::fs::rename(&tmp, &dest).map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
            done = before + f.size;
            progress(done, total);
            log(&format!(
                "translation: downloaded {} / {} ({:.0} MB, {:.0} s)",
                d.key(),
                f.name,
                f.size as f64 / 1e6,
                t0.elapsed().as_secs_f64()
            ));
        }
        // (the direction's other versions: replaced by this one)
        if let Ok(rd) = std::fs::read_dir(root.join(d.key())) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let older = name != d.version
                    && name.bytes().next().is_some_and(|c| c.is_ascii_digit())
                    && safe_name(&name)
                    && e.file_type().is_ok_and(|t| t.is_dir());
                if older && std::fs::remove_dir_all(e.path()).is_ok() {
                    log(&format!("translation: deleted the older {} model ({name})", d.key()));
                }
            }
        }
        out.push(folder);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(numeric_version("2.1"), Some(vec![2, 1]));
        assert_eq!(numeric_version("10"), Some(vec![10]));
        for v in ["1.0a1", "", "1..2", "2.", ".1", "v2", "1.0a", "1,0"] {
            assert_eq!(numeric_version(v), None, "{v}");
        }
        assert!(numeric_version("2.10") > numeric_version("2.9"));
    }

    #[test]
    fn codes() {
        assert_eq!(mozilla_code("zh", true), Some("zh-Hans"));
        assert_eq!(mozilla_code("zh", false), Some("zh-Hans"));
        assert_eq!(mozilla_code("yue", true), Some("zh-Hant"));
        assert_eq!(mozilla_code("yue", false), None);
        assert_eq!(mozilla_code("ja", false), Some("ja"));
        assert_eq!(mozilla_code("xx", true), None);
        assert_eq!(mozilla_code("auto", true), None);
    }

    #[test]
    fn names() {
        assert!(safe_name("model.jaen.intgemm.alphas.bin") && safe_name("zh-Hans") && safe_name("2.1"));
        for n in ["", "..", ".hidden", "a/b", "a\\b", "c:x", "x y"] {
            assert!(!safe_name(n), "{n}");
        }
    }
}
