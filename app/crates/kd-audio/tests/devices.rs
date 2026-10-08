//! The sound devices (need the hardware: #[ignore]d, run with `cargo test -p kd-audio -- --include-ignored`).
//! Nothing loud: the output plays a mixer without a feed (silence).
use kd_audio::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn log() -> kd_common::Log {
    Arc::new(|s: &str| println!("log: {s}"))
}

#[test]
#[ignore]
fn list_devices() {
    let outs = output_devices();
    let ins = input_devices();
    println!("outputs: {outs:?}\ninputs: {ins:?}");
}

#[test]
#[ignore]
fn default_output_plays_a_silent_mixer() {
    let mixer: SharedMixer = Arc::new(Mutex::new(Mixer::new(HashMap::new())));
    let out = match Output::open(mixer.clone(), None, log()) {
        Ok(o) => o,
        Err(e) => {
            // (no output device on this machine: a readable error, no panic)
            assert!(!e.is_empty());
            println!("no output: {e}");
            return;
        }
    };
    std::thread::sleep(Duration::from_millis(1000));
    println!(
        "output: {} ({}), {} callbacks, {} frames, latency {:.1} ms, priority {}, errors {}",
        out.device_name(),
        out.format(),
        out.callbacks(),
        out.frames(),
        out.latency_ms(),
        out.priority(),
        out.errors()
    );
    assert!(out.callbacks() > 10, "the callback ran");
    assert!(out.frames() > 30000, "about a second of frames rendered: {}", out.frames());
    assert!(out.latency_ms() > 0.0);
    assert!(!lock(&mixer).fresh());
    drop(out);
}

/// A device that will not take 48 kHz stereo float (forced here: 44.1 kHz mono 16-bit): rendered at 48 kHz, converted.
#[test]
#[ignore]
fn converted_output_plays() {
    let mixer: SharedMixer = Arc::new(Mutex::new(Mixer::new(HashMap::new())));
    let out = match Output::open_converted(mixer, None, 44100, 1, SampleFormat::I16, log()) {
        Ok(o) => o,
        Err(e) => {
            println!("no output: {e}");
            assert!(output_devices().is_empty());
            return;
        }
    };
    std::thread::sleep(Duration::from_millis(1000));
    println!(
        "converted: {} ({}), {} callbacks, {} frames at 48 kHz, latency {:.1} ms, priority {}",
        out.device_name(),
        out.format(),
        out.callbacks(),
        out.frames(),
        out.latency_ms(),
        out.priority()
    );
    assert!(out.callbacks() > 10 && out.frames() > 30000 && out.errors() == 0);
}

#[test]
#[ignore]
fn a_missing_output_device_falls_back_to_the_default() {
    let mixer: SharedMixer = Arc::new(Mutex::new(Mixer::new(HashMap::new())));
    let said = Arc::new(Mutex::new(Vec::<String>::new()));
    let s2 = said.clone();
    let lg: kd_common::Log = Arc::new(move |s: &str| s2.lock().unwrap().push(s.to_string()));
    let r = Output::open(mixer, Some("No Such Speakers 123"), lg);
    let said = said.lock().unwrap();
    println!("{:?} {said:?}", r.as_ref().map(|o| o.device_name()));
    assert!(said.iter().any(|s| s.contains("output device failed") && s.contains("using the default output")));
    if output_devices().is_empty() {
        assert!(r.is_err());
    } else {
        assert!(r.is_ok());
    }
}

#[test]
#[ignore]
fn default_input_gives_16k_blocks_or_a_readable_error() {
    let blocks = Arc::new(AtomicUsize::new(0));
    let sizes = Arc::new(Mutex::new(Vec::new()));
    let (b2, s2) = (blocks.clone(), sizes.clone());
    let on_block: BlockFn = Box::new(move |x: &[f32]| {
        b2.fetch_add(1, Ordering::Relaxed);
        s2.lock().unwrap().push(x.len());
    });
    match Input::open(None, 16000, on_block, log()) {
        Ok(inp) => {
            std::thread::sleep(Duration::from_millis(1200));
            println!(
                "input: {} ({}), {} callbacks, {} blocks, {} samples, priority {}",
                inp.device_name(),
                inp.format(),
                inp.callbacks(),
                blocks.load(Ordering::Relaxed),
                inp.samples(),
                inp.priority()
            );
            assert!(blocks.load(Ordering::Relaxed) >= 10, "about 20 blocks of 50 ms in a second");
            assert!(sizes.lock().unwrap().iter().all(|&n| n == 320), "20 ms at 16 kHz each");
        }
        Err(e) => {
            println!("no microphone: {e}");
            assert!(!e.is_empty());
        }
    }
    // a named microphone that is not there: the default (or the same readable error)
    let r = Input::open(Some("No Such Microphone 123"), 16000, Box::new(|_| {}), log());
    if let Err(e) = r {
        println!("named, missing: {e}");
        assert!(!e.is_empty());
    }
}
