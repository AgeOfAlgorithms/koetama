//! Model downloads (engine/fetch.py): the pinned files of a Hugging Face repo, over plain HTTPS. A file already in
//! this machine's Hugging Face cache (a developer's) is used where it is; else it is downloaded once into Koetama's
//! models folder: `<models>/<owner>__<repo>/<revision>/<file>`. A download goes to `<file>.part` first, so a half
//! file is never used, and a broken one goes on where it stopped (Range).
use crate::paths;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Where models come from: Hugging Face, or a mirror with the same layout (KOETAMA_MODELS_URL).
pub fn base_url() -> String {
    std::env::var("KOETAMA_MODELS_URL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "https://huggingface.co".into())
}

/// The snapshot folder of repo@revision in the Hugging Face cache, if it is there.
pub fn hf_cache_dir(repo: &str, revision: &str) -> Option<PathBuf> {
    if revision.is_empty() {
        return None;
    }
    let cache = match std::env::var_os("HF_HUB_CACHE").filter(|v| !v.is_empty()) {
        Some(c) => PathBuf::from(c),
        None => std::env::var_os("HF_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| paths::home().join(".cache").join("huggingface"))
            .join("hub"),
    };
    let d = cache.join(format!("models--{}", repo.replace('/', "--"))).join("snapshots").join(revision);
    d.is_dir().then_some(d)
}

/// The folder holding these files of repo@revision, downloading the missing ones into `models`.
/// progress(file, bytes done, bytes total - 0 when unknown).
pub fn repo_files(
    repo: &str,
    revision: &str,
    files: &[&str],
    models: &Path,
    log: &dyn Fn(&str),
    progress: &dyn Fn(&str, u64, u64),
) -> io::Result<PathBuf> {
    if let Some(hf) = hf_cache_dir(repo, revision) {
        if files.iter().all(|f| hf.join(f).exists()) {
            return Ok(hf);
        }
    }
    let rev = if revision.is_empty() { "main" } else { revision };
    let d = models.join(repo.replace('/', "__")).join(rev);
    fs::create_dir_all(&d)?;
    for f in files {
        let dest = d.join(f);
        if dest.exists() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let url = format!("{}/{}/resolve/{}/{}", base_url(), repo, rev, f);
        let t0 = Instant::now();
        download(&url, &dest, &|done, total| progress(f, done, total), 3)?;
        let mb = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0) as f64 / 1e6;
        let name = repo.rsplit('/').next().unwrap_or(repo);
        log(&format!("downloaded {name} / {f} ({mb:.0} MB, {:.0} s)", t0.elapsed().as_secs_f64()));
    }
    Ok(d)
}

fn agent() -> String {
    format!("{}/{}", paths::APP_NAME, paths::VERSION)
}

/// One URL to `dest`, through `dest.part`; up to `tries` attempts, each going on where the last stopped when the
/// server allows it. progress(bytes done, bytes total - 0 when unknown).
pub fn download(url: &str, dest: &Path, progress: &dyn Fn(u64, u64), tries: u32) -> io::Result<PathBuf> {
    let part = PathBuf::from(format!("{}.part", dest.display()));
    let mut last = String::new();
    for attempt in 0..tries.max(1) {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(2));
        }
        match try_once(url, &part, progress) {
            Ok(()) => {
                fs::rename(&part, dest)?;
                return Ok(dest.to_path_buf());
            }
            Err(e) => last = e.to_string(),
        }
    }
    Err(io::Error::other(format!("could not download {url}: {last}")))
}

fn try_once(url: &str, part: &Path, progress: &dyn Fn(u64, u64)) -> io::Result<()> {
    let mut have = fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    // (https only - a redirect to plain http is refused - except from this PC itself: the tests' stand-in servers)
    let agent = ureq::Agent::config_builder()
        .https_only(!on_this_pc(url))
        .timeout_global(None)
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_recv_body(Some(Duration::from_secs(60)))
        .build()
        .new_agent();
    let mut req = agent.get(url).header("User-Agent", &agent_name());
    if have > 0 {
        req = req.header("Range", &format!("bytes={have}-"));
    }
    let resp = req.call().map_err(io::Error::other)?;
    if have > 0 && resp.status().as_u16() != 206 {
        have = 0; // (no resuming there: from the start)
    }
    let len: u64 = resp
        .headers()
        .get("Content-Length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let total = if len > 0 { have + len } else { 0 };
    let mut out = fs::OpenOptions::new().create(true).write(true).append(have > 0).truncate(have == 0).open(part)?;
    let mut body = resp.into_body();
    let mut reader = body.as_reader();
    let mut buf = vec![0u8; 1 << 20];
    let mut done = have;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        done += n as u64;
        progress(done, total);
    }
    out.flush()?;
    if total > 0 && done < total {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("got {done} of {total} bytes")));
    }
    Ok(())
}

fn agent_name() -> String {
    agent()
}

/// A small text from the web (the updater's release list, SHA256SUMS.txt).
pub fn get_text(url: &str, accept: Option<&str>, timeout: Duration) -> io::Result<String> {
    let agent = ureq::Agent::config_builder().https_only(!on_this_pc(url)).timeout_global(Some(timeout)).build().new_agent();
    let mut req = agent.get(url).header("User-Agent", &agent_name());
    if let Some(a) = accept {
        req = req.header("Accept", a);
    }
    let resp = req.call().map_err(io::Error::other)?;
    resp.into_body().read_to_string().map_err(io::Error::other)
}

/// A URL on this PC (http://127.0.0.1 or localhost: the tests' stand-in servers), where plain http is allowed.
fn on_this_pc(url: &str) -> bool {
    ["http://127.0.0.1:", "http://127.0.0.1/", "http://localhost:", "http://localhost/"].iter().any(|p| url.starts_with(p))
}
