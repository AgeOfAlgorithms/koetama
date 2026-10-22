//! A real microphone through what the voice chat does to it, each second's levels printed: what Koetama's capture
//! gets (kd_audio::Input: the device's frames -> the channel with the voice -> 48 kHz mono) and what the automatic gain
//! makes of it (kd_voice::agc, Mic boost 0) - the level a "Hear yourself" test plays and other players hear. Nothing is
//! saved or sent.
//!
//!     cargo run -p kd-voice --example mic_check -- [SECONDS] [DEVICE NAME]     (default 30 s, the default microphone)
use kd_voice::agc::Agc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn db(power: f64) -> f64 {
    10.0 * power.max(1e-12).log10()
}

/// (the power of the loudest 10 % of its 10 ms pieces: the speech, not the pauses; and the whole second's)
fn levels(x: &[f32]) -> (f64, f64) {
    let mut p: Vec<f64> = x.chunks(480).map(|c| c.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / c.len() as f64).collect();
    let all = p.iter().sum::<f64>() / p.len().max(1) as f64;
    p.sort_by(|a, b| b.total_cmp(a));
    let top = &p[..(p.len() / 10).max(1)];
    (top.iter().sum::<f64>() / top.len() as f64, all)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let secs: u64 = args.first().and_then(|s| s.parse().ok()).unwrap_or(30);
    let device = args.get(1).cloned();
    let log: kd_common::Log = Arc::new(|s: &str| println!("  ({s})"));
    let got: Arc<Mutex<(Vec<f32>, Vec<f32>, Agc)>> = Arc::default();
    let g = got.clone();
    let input = kd_audio::Input::open(
        device.as_deref(),
        kd_voice::RATE,
        Box::new(move |x: &[f32]| {
            let mut s = g.lock().unwrap();
            s.0.extend_from_slice(x);
            let mut y = x.to_vec();
            s.2.process(&mut y);
            s.1.extend_from_slice(&y);
        }),
        log,
    )
    .expect("the microphone");
    println!("{} ({}) for {secs} s - talk now", input.device_name(), input.format());
    println!("second   captured: speech  whole    after the gain: speech  whole   gain");
    let t0 = Instant::now();
    for n in 1..=secs {
        std::thread::sleep((t0 + Duration::from_secs(n)).saturating_duration_since(Instant::now()));
        let mut s = got.lock().unwrap();
        let (raw, out) = (std::mem::take(&mut s.0), std::mem::take(&mut s.1));
        let gain = s.2.gain();
        drop(s);
        let ((rs, ra), (os, oa)) = (levels(&raw), levels(&out));
        println!(
            "{n:6}   {:16.1} {:6.1}    {:21.1} {:6.1}   {:+.1} dB",
            db(rs),
            db(ra),
            db(os),
            db(oa),
            20.0 * (gain as f64).log10()
        );
    }
}
