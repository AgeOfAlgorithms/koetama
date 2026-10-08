//! The Rust engine on the machine-translation benchmark (bench/mt): every job of jobs.json with Mozilla's route,
//! the same 100 NTREX sentences, one sentence at a time on one thread - the hypotheses, the time each sentence takes,
//! the load time and the memory. bench/mt/score_rust.py scores them next to Mozilla's own (WASM) build.
//!     cargo run --release -p kd-translate --example mt_bench -- <bench/mt> [src-tgt ...]
use std::path::{Path, PathBuf};
use std::time::Instant;

use kd_translate::Model;
use serde_json::{json, Value};

/// (the process's working set and private bytes, Windows)
#[cfg(windows)]
fn memory() -> (usize, usize) {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        faults: u32,
        peak_working_set: usize,
        working_set: usize,
        quota: [usize; 4],
        private: usize,
        peak_private: usize,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
    }
    let mut c = Counters { cb: std::mem::size_of::<Counters>() as u32, ..Default::default() };
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    (c.working_set, c.private)
}

#[cfg(not(windows))]
fn memory() -> (usize, usize) {
    (0, 0)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bench = PathBuf::from(args.first().map(String::as_str).unwrap_or("../bench/mt"));
    let only: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    let jobs: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(bench.join("jobs.json")).unwrap()).unwrap();
    eprintln!("arithmetic: {}", kd_translate::engine::kernel_name());
    let mut out = Vec::new();
    for job in &jobs {
        let (src, tgt) = (job["src"].as_str().unwrap(), job["tgt"].as_str().unwrap());
        if !only.is_empty() && !only.contains(&format!("{src}-{tgt}").as_str()) {
            continue;
        }
        let Some(route) = job["mozilla"].as_array() else {
            continue;
        };
        let route: Vec<String> =
            route.iter().map(|r| format!("{}-{}", r[0].as_str().unwrap(), r[1].as_str().unwrap())).collect();
        let lines: Vec<String> = std::fs::read_to_string(bench.join("data").join(format!("{src}.txt")))
            .unwrap()
            .lines()
            .map(String::from)
            .collect();
        let before = memory();
        let t = Instant::now();
        let models: Vec<Model> = match route.iter().map(|r| Model::load(&bench.join("models").join(r))).collect() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("{src}-{tgt}: {e}");
                continue;
            }
        };
        let load_ms = t.elapsed().as_secs_f64() * 1000.0;
        let loaded = memory();
        let mut hyps = Vec::new();
        let mut ms = Vec::new();
        for line in &lines {
            let t = Instant::now();
            let mut text = line.clone();
            for m in &models {
                text = m.translate(&text).unwrap();
            }
            ms.push(t.elapsed().as_secs_f64() * 1000.0);
            hyps.push(text);
        }
        let after = memory();
        let mut sorted = ms.clone();
        sorted.sort_by(f64::total_cmp);
        let mean = ms.iter().sum::<f64>() / ms.len() as f64;
        let p90 = sorted[(sorted.len() * 9 / 10).min(sorted.len() - 1)];
        let bytes: usize = models.iter().map(Model::bytes).sum();
        eprintln!(
            "{src}->{tgt} {:<22} load {load_ms:5.0} ms  mean {mean:5.1} ms  p90 {p90:5.1} ms  model {:.1} MB  working set +{:.1} MB (+{:.1} while translating)",
            route.join("+"),
            bytes as f64 / 1e6,
            (loaded.0 as f64 - before.0 as f64) / 1e6,
            (after.0 as f64 - loaded.0 as f64) / 1e6
        );
        out.push(json!({
            "src": src, "tgt": tgt, "route": route, "hyps": hyps, "ms": ms, "mean_ms": mean, "p90_ms": p90,
            "load_ms": load_ms, "model_mb": bytes as f64 / 1e6,
            "ws_load_mb": (loaded.0 as f64 - before.0 as f64) / 1e6,
            "private_load_mb": (loaded.1 as f64 - before.1 as f64) / 1e6,
            "ws_after_mb": after.0 as f64 / 1e6,
        }));
        drop(models);
    }
    let path = bench.join("results_rust.json");
    std::fs::write(&path, serde_json::to_string_pretty(&out).unwrap()).unwrap();
    eprintln!("wrote {}", Path::new(&path).display());
}
