//! The voice mixer (each voice placed by the game's gain, direction and muffle) and the low-pass that muffles
//! (engine/audio.py: Mixer, lowpass, pan_gains, behind).
use crate::effects::Effect;
use kd_common::feed::{Feed, FeedSink, Via, MAX_DELAY};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

pub const RATE: u32 = 48000;
/// s: gains, direction and muffle glide to each new feed value
pub const SMOOTH: f64 = 0.05;
/// s without a new feed (the game paused or gone): the voices fade out
pub const STALE: f64 = 1.5;
/// low-pass cutoff (Hz) at muffle 0
pub const CUT_CLEAR: f64 = 16000.0;
/// low-pass cutoff (Hz) at muffle 1
pub const CUT_MUFFLED: f64 = 400.0;
/// how far to one side a voice goes at 90 degrees (1 = that ear only)
pub const PAN: f64 = 0.9;
/// a voice straight behind: this much duller
pub const BEHIND_MUFFLE: f64 = 0.25;
/// a voice straight behind: this much quieter
pub const BEHIND_QUIET: f64 = 0.2;
/// gain before the soft limiter
pub const HEADROOM: f64 = 1.3;
/// longest low-pass impulse response kept (samples; the most muffled ~ 350)
pub const LP_TAPS: usize = 400;

// ---------------------------------------------------------------- the muffle: two one-pole low-passes in a row

/// The impulse response of two one-pole low-passes in a row (y = a x + (1 - a) y'), cut where it is spent.
pub fn lowpass_ir(a: f64) -> Vec<f64> {
    let mut h = Vec::with_capacity(LP_TAPS);
    lowpass_ir_into(a, &mut h);
    h
}

/// lowpass_ir into a reused buffer (the audio callback: no allocation once it has grown).
fn lowpass_ir_into(a: f64, h: &mut Vec<f64>) {
    // (one pole's response is a (1 - a)^n, so two in a row: the convolution a^2 (n + 1) (1 - a)^n)
    h.clear();
    let mut max = 0.0f64;
    for n in 0..LP_TAPS {
        let v = a * a * (n as f64 + 1.0) * (1.0 - a).powi(n as i32);
        max = max.max(v);
        h.push(v);
    }
    let keep = h.iter().rposition(|&v| v > max * 1e-4).map_or(1, |i| i + 1);
    h.truncate(keep);
}

/// Filter block x with the cutoff a (the one-pole coefficient); hist: the last LP_TAPS input samples before x (the
/// filter's memory, as an FIR over the input - the cutoff may change from block to block). hist is updated.
pub fn lowpass(x: &[f32], hist: &mut Vec<f32>, a: f64) -> Vec<f32> {
    let mut h = lowpass_ir(a);
    h.reverse();
    let (mut full, mut y) = (Vec::new(), Vec::new());
    lowpass_into(x, hist, &h, &mut full, &mut y);
    y
}

/// lowpass with the impulse response given back to front (hr: a plain dot product with the input then, which the
/// compiler vectorises) and reused buffers (full: scratch, y: the output).
fn lowpass_into(x: &[f32], hist: &mut Vec<f32>, hr: &[f64], full: &mut Vec<f32>, y: &mut Vec<f32>) {
    full.clear();
    full.extend_from_slice(hist);
    full.extend_from_slice(x);
    let off = hist.len();
    y.clear();
    for i in 0..x.len() {
        let n = off + i;
        let taps = hr.len().min(n + 1); // (before the start of hist: zeros, as np.convolve)
        y.push(dot(&hr[hr.len() - taps..], &full[n + 1 - taps..=n]) as f32);
    }
    let start = full.len().saturating_sub(LP_TAPS);
    hist.clear();
    hist.extend_from_slice(&full[start..]);
}

/// sum of h[i] x[i] (four running sums: vectorised, and fast enough unoptimised)
fn dot(h: &[f64], x: &[f32]) -> f64 {
    let (hc, hr) = h.as_chunks::<4>();
    let (xc, xr) = x.as_chunks::<4>();
    let mut s = [0.0f64; 4];
    for (a, b) in hc.iter().zip(xc) {
        s[0] += a[0] * b[0] as f64;
        s[1] += a[1] * b[1] as f64;
        s[2] += a[2] * b[2] as f64;
        s[3] += a[3] * b[3] as f64;
    }
    let tail: f64 = hr.iter().zip(xr).map(|(&a, &b)| a * b as f64).sum();
    (s[0] + s[1]) + (s[2] + s[3]) + tail
}

/// Left, right gain for a direction (degrees; az 0 ahead, 90 right; el up): constant power.
pub fn pan_gains(az: f64, el: f64) -> (f64, f64) {
    let p = (az.to_radians().sin() * el.to_radians().cos() * PAN).clamp(-PAN, PAN);
    let th = (p + 1.0) * std::f64::consts::PI / 4.0;
    (th.cos(), th.sin())
}

/// 0 in front .. 1 straight behind
pub fn behind(az: f64) -> f64 {
    ((az.abs() - 90.0) / 90.0).max(0.0)
}

// ---------------------------------------------------------------- the mixer

/// A voice clip: mono at RATE.
pub type Clip = Arc<Vec<f32>>;

/// Real players' voices as they arrive (kd-voice's receiver): the mixer pulls each one's audio while it plays, for a
/// speaker with src 0 (id = their player id) and gain above 0. Called from the audio callback: must not block long.
pub trait Streams: Send {
    /// The next out.len() samples (mono, RATE) of player `id`'s voice into out (silence where there is none);
    /// false: nothing playing for them now.
    fn pull(&mut self, id: i64, out: &mut [f32]) -> bool;
}

/// samples a device's echo line holds: the longest delay and a long block
const RING: usize = (MAX_DELAY * RATE as f64) as usize + 8192;

/// A device a voice comes out of (PROTOCOL.md "Devices"): its sound (effects.rs), the walls' muffle, then each of its
/// places - delayed by how much farther it is than the nearest, panned and faded like a voice.
struct DevOut {
    effect: Effect,
    /// it still makes sound (the voice, a squelch tail, the reverb)
    sounding: bool,
    muffle: f64,
    hist: Vec<f32>,
    ir: Vec<f64>,
    ir_a: f64,
    full: Vec<f32>,
    y: Vec<f32>,
    lp: Vec<f32>,
    /// the echo line and where the next sample goes
    ring: Vec<f32>,
    w: usize,
    /// per place: the gains (left, right) and the delay (samples) last used
    outs: Vec<(f64, f64, f64)>,
}

impl DevOut {
    fn new(seed: u64) -> DevOut {
        DevOut {
            effect: Effect::new(seed),
            sounding: false,
            muffle: 0.0,
            hist: vec![0.0; LP_TAPS],
            ir: Vec::with_capacity(LP_TAPS),
            ir_a: f64::NAN,
            full: Vec::new(),
            y: Vec::new(),
            lp: Vec::new(),
            ring: vec![0.0; RING],
            w: 0,
            outs: Vec::with_capacity(kd_common::feed::MAX_OUTS),
        }
    }

    fn quiet(&self) -> bool {
        !self.sounding && self.outs.iter().all(|o| o.0 < 1e-4 && o.1 < 1e-4)
    }

    /// One block into `out` (interleaved stereo): `x` the voice, `on` whether it comes through this device now,
    /// `via` as the feed has it (None: gone - fading out). -> the louder gain of its places.
    #[allow(clippy::too_many_arguments)]
    fn render(&mut self, x: &[f32], on: bool, via: Option<&Via>, master: f64, k: f64, ramp: &[f32], out: &mut [f32]) -> f64 {
        let frames = x.len();
        if let Some(v) = via {
            self.effect.set(&v.effects, v.signal);
        }
        self.y.clear();
        self.y.resize(frames, 0.0);
        self.sounding = self.effect.process(x, on, &mut self.y);
        let tm = via.map_or(self.muffle, |v| v.muffle);
        self.muffle += (tm - self.muffle) * k;
        let fc = CUT_CLEAR * (CUT_MUFFLED / CUT_CLEAR).powf(self.muffle);
        let a = 1.0 - (-2.0 * std::f64::consts::PI * fc / RATE as f64).exp();
        if a != self.ir_a {
            lowpass_ir_into(a, &mut self.ir);
            self.ir.reverse(); // (back to front: see lowpass_into)
            self.ir_a = a;
        }
        lowpass_into(&self.y, &mut self.hist, &self.ir, &mut self.full, &mut self.lp);
        for (i, &s) in self.lp.iter().enumerate() {
            self.ring[(self.w + i) % RING] = s;
        }
        let places = via.map_or(0, |v| v.outs.len());
        if self.outs.len() < places {
            self.outs.resize(places, (0.0, 0.0, 0.0));
        }
        let max_delay = (RING - frames.min(RING)) as f64;
        let mut level = 0.0f64;
        for (j, o) in self.outs.iter_mut().enumerate() {
            let place = via.and_then(|v| v.outs.get(j));
            let (tl, tr, td) = match place {
                Some(p) => {
                    let b = behind(p.az);
                    let g = p.gain * master * (1.0 - BEHIND_QUIET * b);
                    let (l, r) = pan_gains(p.az, p.el);
                    (g * l, g * r, (p.delay * RATE as f64).min(max_delay))
                }
                None => (0.0, 0.0, o.2),
            };
            let nl = o.0 + (tl - o.0) * k;
            let nr = o.1 + (tr - o.1) * k;
            if nl > 1e-6 || nr > 1e-6 || o.0 > 1e-6 || o.1 > 1e-6 {
                let (gl0, dl) = (o.0 as f32, (nl - o.0) as f32);
                let (gr0, dr) = (o.1 as f32, (nr - o.1) as f32);
                let start = self.w + RING - td.round() as usize;
                for (i, f) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                    let s = self.ring[(start + i) % RING];
                    f[0] += s * (gl0 + dl * ramp[i]);
                    f[1] += s * (gr0 + dr * ramp[i]);
                }
            }
            *o = (nl, nr, td);
            level = level.max(nl).max(nr);
        }
        self.w = (self.w + frames) % RING;
        // (places gone from the feed and faded: let go)
        while self.outs.len() > places && self.outs.last().is_some_and(|o| o.0 < 1e-4 && o.1 < 1e-4) {
            self.outs.pop();
        }
        level
    }
}

struct Voice {
    /// the devices it comes out of (the feed's `via`, in order)
    devs: Vec<DevOut>,
    /// effects on the direct voice (made when the feed first asks for some), and whether they still sound
    fx: Option<Effect>,
    fx_sounding: bool,
    fx_out: Vec<f32>,
    pos: usize,
    /// (a new turn starts the clip from its beginning: the game times the words to it)
    talking: bool,
    gl: f64,
    gr: f64,
    muffle: f64,
    hist: Vec<f32>,
    /// the clip it last played (to fade out on after it left the feed)
    src_clip: Option<Clip>,
    // (reused buffers: the audio callback does not allocate once they have grown)
    /// the impulse response, back to front (for cutoff ir_a)
    ir: Vec<f64>,
    ir_a: f64,
    x: Vec<f32>,
    y: Vec<f32>,
    full: Vec<f32>,
}

impl Voice {
    fn new() -> Voice {
        Voice {
            devs: Vec::new(),
            fx: None,
            fx_sounding: false,
            fx_out: Vec::new(),
            pos: 0,
            talking: false,
            gl: 0.0,
            gr: 0.0,
            muffle: 0.0,
            hist: vec![0.0; LP_TAPS],
            src_clip: None,
            ir: Vec::with_capacity(LP_TAPS),
            ir_a: f64::NAN,
            x: Vec::new(),
            y: Vec::new(),
            full: Vec::new(),
        }
    }
}

/// clips: {src: mono clip at RATE}. A feed (the game's): vol and speakers {id: src, talk, gain, az, el, muffle}.
/// render(frames) -> stereo frames.
pub struct Mixer {
    pub clips: HashMap<i64, Clip>,
    /// real players' voices (speakers with src 0); None: none play
    pub streams: Option<Box<dyn Streams>>,
    /// the helper's own volume, on top of the game's
    pub volume: f64,
    clock: Box<dyn Fn() -> f64 + Send>,
    feed: Option<Feed>,
    feed_t: f64,
    voices: HashMap<i64, Voice>,
    /// id -> the level last rendered (for the status)
    levels: HashMap<i64, f64>,
    ids: Vec<i64>,
    ramp: Vec<f32>,
    stereo: Vec<f32>,
    /// a block of silence (a device's input while no voice comes through it)
    zeros: Vec<f32>,
}

impl Mixer {
    /// A mixer on the real clock.
    pub fn new(clips: HashMap<i64, Clip>) -> Mixer {
        let t0 = Instant::now();
        Mixer::with_clock(clips, Box::new(move || t0.elapsed().as_secs_f64()))
    }

    /// A mixer on the given clock (seconds; tests: a fake clock).
    pub fn with_clock(clips: HashMap<i64, Clip>, clock: Box<dyn Fn() -> f64 + Send>) -> Mixer {
        Mixer {
            clips,
            streams: None,
            volume: 1.0,
            clock,
            feed: None,
            feed_t: -1e9,
            voices: HashMap::new(),
            levels: HashMap::new(),
            ids: Vec::new(),
            ramp: Vec::new(),
            stereo: Vec::new(),
            zeros: Vec::new(),
        }
    }

    pub fn set_feed(&mut self, feed: Feed) {
        self.feed = Some(feed);
        self.feed_t = (self.clock)();
    }

    /// A feed, at most STALE s old.
    pub fn fresh(&self) -> bool {
        self.feed.is_some() && self.feed_age() <= STALE
    }

    pub fn feed(&self) -> Option<&Feed> {
        self.feed.as_ref()
    }

    /// s since the last feed (huge if none)
    pub fn feed_age(&self) -> f64 {
        (self.clock)() - self.feed_t
    }

    /// Where voice `id` is in its clip (samples), None if the mixer has no such voice.
    pub fn voice_pos(&self, id: i64) -> Option<usize> {
        self.voices.get(&id).map(|v| v.pos)
    }

    /// The voices the mixer keeps (talking, fading out, or quiet but still in the feed), ascending.
    pub fn voice_ids(&self) -> Vec<i64> {
        let mut ids: Vec<i64> = self.voices.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// The gain voice `id` was last rendered at (the louder ear; 0 if quiet or unknown).
    pub fn level(&self, id: i64) -> f64 {
        self.levels.get(&id).copied().unwrap_or(0.0)
    }

    /// `frames` stereo frames.
    pub fn render(&mut self, frames: usize) -> Vec<[f32; 2]> {
        let mut out = vec![0.0f32; frames * 2];
        self.render_into(&mut out);
        out.as_chunks::<2>().0.to_vec()
    }

    /// Render into interleaved stereo `out` (out.len() / 2 frames): the audio callback. It allocates only when a
    /// new voice appears or a block is longer than any before (its buffers are kept).
    pub fn render_into(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        out.fill(0.0);
        if frames == 0 {
            return;
        }
        let age = (self.clock)() - self.feed_t;
        let live = self.feed.is_some() && age <= STALE;
        let feed = if live { self.feed.as_ref() } else { None };
        let master = feed.map_or(0.0, |f| f.vol) * self.volume * HEADROOM;
        let k = 1.0 - (-(frames as f64) / RATE as f64 / SMOOTH).exp();
        self.ramp.clear();
        self.ramp.extend((1..=frames).map(|i| i as f32 / frames as f32));
        let ramp = &self.ramp;
        self.zeros.clear();
        self.zeros.resize(frames, 0.0);
        let zeros = &self.zeros;

        let ids = &mut self.ids;
        ids.clear();
        ids.extend(self.voices.keys().copied());
        if let Some(f) = feed {
            ids.extend(f.speakers.keys().copied().filter(|id| !self.voices.contains_key(id)));
        }
        ids.sort_unstable();

        for &sid in ids.iter() {
            let sp = feed.and_then(|f| f.speakers.get(&sid));
            let v = self.voices.entry(sid).or_insert_with(Voice::new);
            // (a real player: their voice streams in; talk is ignored - what arrives plays. Gain 0: not played)
            let real = sp.is_some_and(|s| s.src == 0);
            if real {
                v.x.clear();
                v.x.resize(frames, 0.0);
            }
            let streaming = real
                && sp.is_some_and(|s| s.audible())
                && self.streams.as_mut().is_some_and(|st| st.pull(sid, &mut v.x));
            let clip = if real { None } else { sp.and_then(|s| self.clips.get(&s.src)) };
            let talking = streaming || (sp.is_some_and(|s| s.talk) && clip.is_some());
            if talking && !v.talking {
                v.pos = 0;
            }
            v.talking = talking;
            let (tl, tr, tm) = match sp {
                Some(s) if talking || v.fx_sounding => {
                    let b = behind(s.az);
                    let g = s.gain * master * (1.0 - BEHIND_QUIET * b);
                    let (l, r) = pan_gains(s.az, s.el);
                    (g * l, g * r, (s.muffle + BEHIND_MUFFLE * b).min(1.0))
                }
                _ => (0.0, 0.0, v.muffle),
            };
            if !talking && !v.fx_sounding && v.gl < 1e-4 && v.gr < 1e-4 && v.devs.iter().all(DevOut::quiet) {
                v.devs.clear();
                v.gl = 0.0;
                v.gr = 0.0;
                self.levels.insert(sid, 0.0);
                if sp.is_none() {
                    self.voices.remove(&sid);
                }
                continue;
            }
            if !real {
                if let Some(c) = clip {
                    // (fading out after it left the feed: its last clip)
                    if !v.src_clip.as_ref().is_some_and(|s| Arc::ptr_eq(s, c)) {
                        v.src_clip = Some(c.clone());
                    }
                }
                let src = match v.src_clip.as_ref() {
                    Some(s) if !s.is_empty() => s,
                    _ => {
                        // (an empty clip, or a real player gone from the feed: nothing to play)
                        v.gl = 0.0;
                        v.gr = 0.0;
                        v.devs.clear();
                        self.levels.insert(sid, 0.0);
                        continue;
                    }
                };
                let n = src.len();
                v.pos %= n;
                v.x.clear();
                v.x.extend((0..frames).map(|i| src[(v.pos + i) % n]));
                v.pos = (v.pos + frames) % n;
            }
            v.muffle += (tm - v.muffle) * k;
            let fc = CUT_CLEAR * (CUT_MUFFLED / CUT_CLEAR).powf(v.muffle);
            let a = 1.0 - (-2.0 * std::f64::consts::PI * fc / RATE as f64).exp();
            if a != v.ir_a {
                lowpass_ir_into(a, &mut v.ir);
                v.ir.reverse(); // (back to front: see lowpass_into)
                v.ir_a = a;
            }
            // (the direct voice's own effects - a helmet, a robot; the devices take the voice as it came)
            let fx = sp.map(|s| s.effects).filter(|e| !e.is_clean());
            let direct: &[f32] = if fx.is_some() || v.fx_sounding {
                let e = v.fx.get_or_insert_with(|| Effect::new((sid as u64).wrapping_mul(31)));
                e.set(&fx.unwrap_or_default(), 1.0);
                v.fx_out.clear();
                v.fx_out.resize(frames, 0.0);
                let on = talking && fx.is_some();
                let x = if on { &v.x[..frames] } else { &zeros[..] };
                v.fx_sounding = e.process(x, on, &mut v.fx_out);
                &v.fx_out
            } else {
                &v.x
            };
            lowpass_into(direct, &mut v.hist, &v.ir, &mut v.full, &mut v.y);
            let nl = v.gl + (tl - v.gl) * k;
            let nr = v.gr + (tr - v.gr) * k;
            // (the gains ramp across the block, in float32 as numpy does)
            let (gl0, dl) = (v.gl as f32, (nl - v.gl) as f32);
            let (gr0, dr) = (v.gr as f32, (nr - v.gr) as f32);
            for (i, (o, &x)) in out.as_chunks_mut::<2>().0.iter_mut().zip(v.y.iter()).enumerate() {
                o[0] += x * (gl0 + dl * ramp[i]);
                o[1] += x * (gr0 + dr * ramp[i]);
            }
            v.gl = nl;
            v.gr = nr;
            let mut level = nl.max(nr);
            // the devices it comes out of
            let via: &[Via] = sp.map_or(&[], |s| &s.via);
            while v.devs.len() < via.len() {
                let i = v.devs.len() as u64 + 1;
                v.devs.push(DevOut::new((sid as u64).wrapping_mul(31).wrapping_add(i)));
            }
            for (i, d) in v.devs.iter_mut().enumerate() {
                let dv = via.get(i);
                let on = talking && dv.is_some_and(Via::audible);
                if !on && d.quiet() {
                    continue;
                }
                let x = if on { &v.x[..frames] } else { &zeros[..] };
                level = level.max(d.render(x, on, dv, master, k, ramp, out));
            }
            while v.devs.len() > via.len() && v.devs.last().is_some_and(DevOut::quiet) {
                v.devs.pop();
            }
            self.levels.insert(sid, level);
        }
        for s in out.iter_mut() {
            *s = s.tanh();
        }
    }

    /// render_into a reused stereo buffer of `frames` frames, returned as a slice (for converting callbacks).
    pub(crate) fn render_scratch(&mut self, frames: usize) -> &[f32] {
        let mut buf = std::mem::take(&mut self.stereo);
        buf.resize(frames * 2, 0.0);
        self.render_into(&mut buf);
        self.stereo = buf;
        &self.stereo
    }
}

/// The mixer as the audio callback and the game share it.
pub type SharedMixer = Arc<Mutex<Mixer>>;

/// Lock a shared mixer (a panic elsewhere while it was held does not silence the audio).
pub fn lock(m: &SharedMixer) -> MutexGuard<'_, Mixer> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a game feeds: each new feed goes to the mixer.
pub struct MixerSink(pub SharedMixer);

impl FeedSink for MixerSink {
    fn set_feed(&self, feed: Feed) {
        lock(&self.0).set_feed(feed);
    }

    fn fresh(&self) -> bool {
        lock(&self.0).fresh()
    }
}
