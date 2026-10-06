//! Shared by the tests: a fake clock, test clips, a feed with one speaker (as engine/test_helper.py).
#![allow(dead_code)]
use kd_audio::{Clip, Mixer, RATE};
use kd_common::feed::{Feed, Speaker};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A clock the test moves by hand (seconds).
#[derive(Clone, Default)]
pub struct Clock(pub Arc<Mutex<f64>>);

impl Clock {
    pub fn get(&self) -> f64 {
        *self.0.lock().unwrap()
    }
    pub fn add(&self, s: f64) {
        *self.0.lock().unwrap() += s;
    }
    pub fn boxed(&self) -> Box<dyn Fn() -> f64 + Send> {
        let c = self.clone();
        Box::new(move || c.get())
    }
}

/// A small deterministic random source (xorshift + Box-Muller): normal samples.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    pub fn normal(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }
}

pub fn clips(x: Vec<f32>) -> HashMap<i64, Clip> {
    HashMap::from([(1, Arc::new(x))])
}

/// 1 s of noise at 0.1 (NOISE)
pub fn noise() -> HashMap<i64, Clip> {
    let mut r = Rng::new(1);
    clips((0..RATE).map(|_| (r.normal() * 0.1) as f32).collect())
}

/// 1 s of 0.1 (ONES)
pub fn ones() -> HashMap<i64, Clip> {
    clips(vec![0.1; RATE as usize])
}

/// A speaker's settings: (src, talk, gain, az, el, muffle) start as test_helper.py's feed() and are changed by `f`.
pub fn feed_with(vol: f64, f: impl FnOnce(&mut Speaker)) -> Feed {
    let mut s = Speaker { src: 1, talk: true, gain: 1.0, az: 0.0, el: 0.0, muffle: 0.0 };
    f(&mut s);
    Feed { seq: 1, vol, sid: 1, ack: 0, ping: 1, speakers: [(7, s)].into_iter().collect(), ..Default::default() }
}

pub fn feed() -> Feed {
    feed_with(1.0, |_| {})
}

/// Render `seconds` of audio in blocks, the clock moving with it. -> interleaved stereo
pub fn run(m: &mut Mixer, clock: &Clock, seconds: f64, block: usize) -> Vec<f32> {
    let mut out = Vec::new();
    for _ in 0..(seconds * RATE as f64 / block as f64) as usize {
        let mut b = vec![0.0f32; block * 2];
        m.render_into(&mut b);
        out.extend_from_slice(&b);
        clock.add(block as f64 / RATE as f64);
    }
    out
}

/// The output once the gains have settled on feed f (the feed kept fresh).
pub fn settled(clips: &HashMap<i64, Clip>, f: &Feed) -> Vec<f32> {
    let clock = Clock::default();
    let mut m = Mixer::with_clock(clips.clone(), clock.boxed());
    let mut out = Vec::new();
    for i in 0..(0.5f64 / 0.05) as usize + 8 {
        m.set_feed(f.clone());
        let o = run(&mut m, &clock, 0.05, 480);
        if i >= 8 {
            out.extend(o);
        }
    }
    out
}

pub fn chan(x: &[f32], c: usize) -> Vec<f32> {
    x.iter().skip(c).step_by(2).copied().collect()
}

pub fn rms(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|&v| v as f64 * v as f64).sum::<f64>() / x.len() as f64).sqrt()
}

pub fn db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}
