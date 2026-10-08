//! kd_common::fetch against a local web server laid out like Hugging Face (<repo>/resolve/<rev>/<file>): a whole
//! download, resuming a broken one (with and without Range support), progress, nothing downloaded twice, errors.
use kd_common::fetch;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// A tiny HTTP server for one file: honours Range when `ranges`; counts the requests.
fn serve(blob: Vec<u8>, ranges: bool) -> (String, Arc<Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let mut s = s.unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut first = String::new();
            r.read_line(&mut first).unwrap();
            let mut range = None;
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("range: bytes=") {
                    range = v.trim().trim_end_matches('-').parse::<usize>().ok();
                }
            }
            let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
            seen2.lock().unwrap().push(format!("{path} {range:?}"));
            if !path.ends_with("/resolve/abc/m.onnx") {
                let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                continue;
            }
            let (status, body) = match range {
                Some(a) if ranges => ("206 Partial Content", &blob[a..]),
                _ => ("200 OK", &blob[..]),
            };
            let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(body);
        }
    });
    (base, seen)
}

fn blob() -> Vec<u8> {
    (0..3_000_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect()
}

fn with_base<T>(base: &str, f: impl FnOnce() -> T) -> T {
    // (the tests in this file share the process environment: one at a time)
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("KOETAMA_MODELS_URL", base);
    std::env::set_var("HF_HUB_CACHE", std::env::temp_dir().join("kd-no-hf-cache"));
    f()
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("kd-fetch-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn downloads_whole_over_a_broken_try_without_ranges() {
    let b = blob();
    let (base, _) = serve(b.clone(), false);
    let home = tmp("noranges");
    let want = home.join("own__model").join("abc").join("m.onnx");
    std::fs::create_dir_all(want.parent().unwrap()).unwrap();
    std::fs::write(format!("{}.part", want.display()), b"half a file from before").unwrap();
    let seen = Mutex::new(Vec::new());
    let d = with_base(&base, || {
        fetch::repo_files("own/model", "abc", &["m.onnx"], &home, &|_| {}, &|f, a, t| seen.lock().unwrap().push((f.to_string(), a, t)))
    })
    .unwrap();
    assert_eq!(std::fs::read(d.join("m.onnx")).unwrap(), b);
    assert!(!std::path::Path::new(&format!("{}.part", want.display())).exists());
    let last = seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last, ("m.onnx".to_string(), b.len() as u64, b.len() as u64));
}

#[test]
fn resumes_where_it_stopped_with_ranges() {
    let b = blob();
    let (base, reqs) = serve(b.clone(), true);
    let home = tmp("ranges");
    let want = home.join("own__model").join("abc").join("m.onnx");
    std::fs::create_dir_all(want.parent().unwrap()).unwrap();
    std::fs::write(format!("{}.part", want.display()), &b[..1_000_000]).unwrap();
    let d = with_base(&base, || fetch::repo_files("own/model", "abc", &["m.onnx"], &home, &|_| {}, &|_, _, _| {})).unwrap();
    assert_eq!(std::fs::read(d.join("m.onnx")).unwrap(), b);
    assert_eq!(reqs.lock().unwrap().as_slice(), ["/own/model/resolve/abc/m.onnx Some(1000000)"]);
    // a second time: nothing downloaded
    let again = with_base(&base, || fetch::repo_files("own/model", "abc", &["m.onnx"], &home, &|_| panic!("no log"), &|_, _, _| {})).unwrap();
    assert_eq!(again, d);
    assert_eq!(reqs.lock().unwrap().len(), 1);
}

#[test]
fn a_missing_file_is_an_error_and_leaves_nothing() {
    let (base, _) = serve(blob(), true);
    let home = tmp("missing");
    let dest = home.join("none.onnx");
    let r = fetch::download(&format!("{base}/own/model/resolve/abc/none.onnx"), &dest, &|_, _| {}, 1);
    assert!(r.is_err());
    assert!(!dest.exists());
    let mut leftovers = String::new();
    for e in std::fs::read_dir(&home).unwrap() {
        leftovers += &e.unwrap().file_name().to_string_lossy();
    }
    assert!(leftovers.is_empty() || !leftovers.contains("none.onnx"), "{leftovers}");
}
