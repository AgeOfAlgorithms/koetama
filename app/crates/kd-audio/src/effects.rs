//! A voice's sound effects (PROTOCOL.md "Devices", kd_common::feed::Effects): blocks a game turns on one by one,
//! and the devices' presets (a walkie-talkie, a loudspeaker, a PA) made of them. In this order:
//!
//!   pitch -> robot -> band (+ horn) -> drive -> compress -> lofi -> wobble
//!     -> + hiss / crackle / squelch / hum (the receiver's noise, band-limited like the voice)
//!     -> echo -> reverb -> a soft limiter
//!
//! The band also shapes what follows it like a small speaker would: the noise goes through the same band, and the
//! voice through a gentle copy of it after drive and lofi (so their harmonics stay inside, as in a real radio).
//!
//! Real time (the audio callback, 48 kHz mono, any block size): Effect::new allocates everything; process allocates
//! nothing, takes no locks and works sample by sample - every glide, every random event and every `on` edge - so
//! the output is the same whatever the block size. A block at 0 costs about nothing. All off: the voice as it is.
use crate::mixer::RATE;
use kd_common::feed::Effects;
use std::f64::consts::PI;

const FS: f64 = RATE as f64;
/// s: set's values glide to each new value
const TAU_PARAM: f64 = 0.05;
/// s: a block fading in or out when it is turned on or off
const TAU_EN: f64 = 0.02;
/// filter coefficients follow a glide every this many samples (counted from the start: block-size independent)
const COEF_EVERY: u64 = 16;
/// dBFS: quieter than this after the voice and its tails: done (process returns false)
const DEAD_DB: f64 = -70.0;
/// the output's peak follower falls this fast (dB / s)
const ENV_FALL_DB: f64 = 600.0;
/// s after the voice: whatever still sounds (a long echo) fades out from here to TAIL_MAX
const TAIL_FADE: f64 = 2.4;
const TAIL_MAX: f64 = 2.9;

// ---------------------------------------------------------------- small parts

/// one-pole coefficient for a time constant (s)
fn coef(tau: f64) -> f64 {
    1.0 - (-1.0 / (tau * FS)).exp()
}

fn db(x: f64) -> f64 {
    10f64.powf(x / 20.0)
}

fn ms(t: f64) -> f64 {
    t * 0.001 * FS
}

/// A value gliding to its target (snaps when within 1e-9).
#[derive(Clone, Copy, Default, Debug)]
struct Glide {
    v: f64,
    t: f64,
}

impl Glide {
    #[inline]
    fn step(&mut self, k: f64) -> f64 {
        if self.v != self.t {
            self.v += (self.t - self.v) * k;
            if (self.v - self.t).abs() < 1e-9 {
                self.v = self.t;
            }
        }
        self.v
    }
    fn snap(&mut self) {
        self.v = self.t;
    }
    /// on, or gliding to or from off
    #[inline]
    fn on(&self) -> bool {
        self.v != 0.0 || self.t != 0.0
    }
    #[inline]
    fn moving(&self) -> bool {
        self.v != self.t
    }
}

/// xorshift64*, seeded through splitmix64
#[derive(Clone)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Rng(if z == 0 { 1 } else { z })
    }
    #[inline]
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// 0..1
    #[inline]
    fn uniform(&mut self) -> f64 {
        (self.next() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }
    /// -1..1 (variance 1/3)
    #[inline]
    fn white(&mut self) -> f64 {
        2.0 * self.uniform() - 1.0
    }
    fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.uniform()
    }
}

/// A biquad (transposed direct form II, f64).
#[derive(Clone, Copy, Default, Debug)]
struct Bq {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

#[derive(Clone, Copy)]
enum Kind {
    Low,
    High,
    Peak(f64),
}

impl Bq {
    /// RBJ cookbook coefficients (the state is kept: a filter may glide)
    fn design(&mut self, kind: Kind, f: f64, q: f64) {
        let w = 2.0 * PI * f.clamp(10.0, 0.45 * FS) / FS;
        let (s, c) = w.sin_cos();
        let al = s / (2.0 * q);
        let (b, a) = match kind {
            Kind::Low => (
                [(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0],
                [1.0 + al, -2.0 * c, 1.0 - al],
            ),
            Kind::High => (
                [(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0],
                [1.0 + al, -2.0 * c, 1.0 - al],
            ),
            Kind::Peak(g) => {
                let a = 10f64.powf(g / 40.0);
                (
                    [1.0 + al * a, -2.0 * c, 1.0 - al * a],
                    [1.0 + al / a, -2.0 * c, 1.0 - al / a],
                )
            }
        };
        self.b0 = b[0] / a[0];
        self.b1 = b[1] / a[0];
        self.b2 = b[2] / a[0];
        self.a1 = a[1] / a[0];
        self.a2 = a[2] / a[0];
    }
    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
    fn clear(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// Butterworth sections' Qs
const BUTTER2: [f64; 1] = [std::f64::consts::FRAC_1_SQRT_2];
const BUTTER4: [f64; 2] = [0.541_196, 1.306_563];
const BUTTER6: [f64; 3] = [0.517_638, std::f64::consts::FRAC_1_SQRT_2, 1.931_852];

fn design(f: &mut [Bq], kind: Kind, hz: f64, qs: &[f64]) {
    for (b, &q) in f.iter_mut().zip(qs) {
        b.design(kind, hz, q);
    }
}

#[inline]
fn run(f: &mut [Bq], mut x: f64) -> f64 {
    for b in f.iter_mut() {
        x = b.run(x);
    }
    x
}

fn clear(f: &mut [Bq]) {
    f.iter_mut().for_each(Bq::clear);
}

/// tanh, rationally (within 2%, exact at 0 and in the limits)
#[inline]
fn soft(u: f64) -> f64 {
    if u.abs() >= 3.0 {
        u.signum()
    } else {
        u * (27.0 + u * u) / (27.0 + 9.0 * u * u)
    }
}

/// a cubic clip: linear-ish to ~0.7, flat from 1.5 (a hard knee)
#[inline]
fn hard(u: f64) -> f64 {
    let u = u.clamp(-1.5, 1.5);
    u - (4.0 / 27.0) * u * u * u
}

/// a delay line read d samples back from the newest sample, cubic (Hermite)
#[inline]
fn read_cubic(buf: &[f32], mask: usize, newest: usize, d: f64) -> f64 {
    let p = newest as f64 - d;
    let i = p.floor();
    let t = p - i;
    let i = i as isize as usize;
    let xm = buf[i.wrapping_sub(1) & mask] as f64;
    let x0 = buf[i & mask] as f64;
    let x1 = buf[i.wrapping_add(1) & mask] as f64;
    let x2 = buf[i.wrapping_add(2) & mask] as f64;
    let c1 = 0.5 * (x1 - xm);
    let c2 = xm - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm) + 1.5 * (x0 - x1);
    ((c3 * t + c2) * t + c1) * t + x0
}

#[inline]
fn read_linear(buf: &[f32], mask: usize, newest: usize, d: f64) -> f64 {
    let p = newest as f64 - d;
    let i = p.floor();
    let t = p - i;
    let i = i as isize as usize;
    let a = buf[i & mask] as f64;
    let b = buf[i.wrapping_add(1) & mask] as f64;
    a + (b - a) * t
}

/// Smooth random motion (white noise through two one-pole low-passes), about unit variance.
#[derive(Clone, Copy, Default)]
struct Lfo {
    a: f64,
    y1: f64,
    y2: f64,
    norm: f64,
}

impl Lfo {
    fn new(hz: f64) -> Lfo {
        let a = 1.0 - (-2.0 * PI * hz / FS).exp();
        // (variance of two one-poles over white noise of variance 1/3: a^4 (1 + r) / (1 - r)^3 / 3, r = (1 - a)^2)
        let r = (1.0 - a) * (1.0 - a);
        let var = a.powi(4) * (1.0 + r) / (1.0 - r).powi(3) / 3.0;
        Lfo {
            a,
            y1: 0.0,
            y2: 0.0,
            norm: 1.0 / var.sqrt(),
        }
    }
    #[inline]
    fn step(&mut self, w: f64) -> f64 {
        self.y1 += (w - self.y1) * self.a;
        self.y2 += (self.y1 - self.y2) * self.a;
        self.y2 * self.norm
    }
}

// ---------------------------------------------------------------- the blocks

/// pitch: a delay line read at the shifted rate by one tap; when it runs out of room it crossfades to a second tap
/// placed where the waveform matches best (WSOLA-style splices: few of the comb-filter artefacts of a plain two-tap
/// shifter). Latency 5..40 ms.
struct Pitch {
    st: Glide,
    en: Glide,
    ratio: f64,
    ratio_st: f64,
    buf: Vec<f32>,
    w: usize,
    da: f64,
    db: f64,
    xf: usize,
    seg: [f32; PITCH_SEG],
}

const PITCH_BUF: usize = 8192;
/// crossfade (samples, 10 ms)
const PITCH_XF: usize = 480;
/// the tap's delay range (samples)
const PITCH_DMIN: f64 = 300.0;
const PITCH_DMAX: f64 = 1900.0;
/// how far the splice search looks either side (samples, ~ one 75 Hz period over both sides)
const PITCH_SEARCH: usize = 320;
/// the compared stretch: 256 samples behind the tap and 128 ahead, every other one
const PITCH_SEG: usize = 192;

impl Pitch {
    fn new() -> Pitch {
        Pitch {
            st: Glide::default(),
            en: Glide::default(),
            ratio: 1.0,
            ratio_st: 0.0,
            buf: vec![0.0; PITCH_BUF],
            w: 0,
            da: (PITCH_DMIN + PITCH_DMAX) / 2.0,
            db: 0.0,
            xf: 0,
            seg: [0.0; PITCH_SEG],
        }
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
        self.da = (PITCH_DMIN + PITCH_DMAX) / 2.0;
        self.xf = 0;
    }

    /// the delay (around `centre`, within PITCH_SEARCH) whose surroundings match tap A's best
    fn best(&mut self, centre: f64) -> f64 {
        let mask = PITCH_BUF - 1;
        let newest = self.w.wrapping_sub(1);
        let ia = newest.wrapping_sub(self.da.round() as usize);
        for (j, s) in self.seg.iter_mut().enumerate() {
            *s = self.buf[ia.wrapping_add(2 * j).wrapping_sub(256) & mask];
        }
        let score = |buf: &[f32], seg: &[f32; PITCH_SEG], d: usize| -> f64 {
            let ib = newest.wrapping_sub(d).wrapping_sub(256);
            let (mut c, mut e) = (0.0f32, 1e-9f32);
            for (j, &a) in seg.iter().enumerate() {
                let b = buf[ib.wrapping_add(2 * j) & mask];
                c += a * b;
                e += b * b;
            }
            c as f64 / (e as f64).sqrt()
        };
        let c = centre.round() as usize;
        let (lo, hi) = (c - PITCH_SEARCH, c + PITCH_SEARCH);
        let (mut bd, mut bs) = (c, f64::MIN);
        for d in (lo..=hi).step_by(2) {
            let s = score(&self.buf, &self.seg, d);
            if s > bs {
                (bd, bs) = (d, s);
            }
        }
        for d in [bd - 1, bd + 1] {
            let s = score(&self.buf, &self.seg, d);
            if s > bs {
                (bd, bs) = (d, s);
            }
        }
        // (the tap's fractional part kept: the splice lands on the same sub-sample phase)
        bd as f64 + (self.da - self.da.round())
    }

    #[inline]
    fn run(&mut self, x: f64, k: f64, ke: f64) -> f64 {
        let mask = PITCH_BUF - 1;
        self.buf[self.w & mask] = x as f32;
        self.w = self.w.wrapping_add(1);
        let e = self.en.step(ke);
        if e == 0.0 {
            return x;
        }
        let st = self.st.step(k);
        if st != self.ratio_st {
            self.ratio = (st / 12.0).exp2();
            self.ratio_st = st;
        }
        let step = 1.0 - self.ratio;
        let newest = self.w - 1;
        self.da += step;
        let ya = read_cubic(&self.buf, mask, newest, self.da);
        let y = if self.xf > 0 {
            self.db += step;
            let yb = read_cubic(&self.buf, mask, newest, self.db);
            let t = 1.0 - self.xf as f64 / PITCH_XF as f64;
            let w = 0.5 - 0.5 * (PI * t).cos();
            self.xf -= 1;
            if self.xf == 0 {
                self.da = self.db;
            }
            ya * (1.0 - w) + yb * w
        } else {
            let rise = PITCH_XF as f64 * (self.ratio - 1.0).abs() + 8.0;
            if self.ratio > 1.0 && self.da < (rise).max(140.0) {
                self.db = self.best(PITCH_DMAX - PITCH_SEARCH as f64);
                self.xf = PITCH_XF;
            } else if self.ratio < 1.0 && self.da > PITCH_DMAX - rise {
                self.db = self.best(PITCH_DMIN + PITCH_SEARCH as f64);
                self.xf = PITCH_XF;
            }
            ya
        };
        x + (y - x) * e
    }
}

/// band (+ horn): a steep band-pass (4th-order high-pass, 6th-order low-pass), a horn's resonances and metallic
/// ring, and a makeup gain that gives back the energy the band took (measured on the voice: a band does not make
/// the voice quieter). `spk`: a gentle copy of the band for after drive and lofi.
struct Band {
    en: Glide,
    lo: Glide,
    hi: Glide,
    hp: [Bq; 2],
    lp: [Bq; 3],
    spk: [Bq; 3],
    horn: Glide,
    peaks: [Bq; 3],
    ring: [[f64; RING_LEN]; 2],
    ring_i: usize,
    e_in: f64,
    e_out: f64,
    mk: f64,
}

const RING_LEN: usize = 64;
/// the horn's two short tubes (samples: about 2.2 and 1.4 kHz and their overtones) and their feedback
const RING_D: [usize; 2] = [22, 35];
const RING_FB: f64 = 0.45;
/// the horn's resonances: (Hz, Q, dB at horn 1)
const HORN_PEAKS: [(f64, f64, f64); 3] =
    [(1100.0, 3.0, 8.0), (2400.0, 4.0, 7.0), (3400.0, 5.0, 4.0)];
/// s: the band's makeup follows the voice's spectrum this slowly
const TAU_MAKEUP: f64 = 0.4;

impl Band {
    fn new() -> Band {
        Band {
            en: Glide::default(),
            lo: Glide {
                v: 300f64.ln(),
                t: 300f64.ln(),
            },
            hi: Glide {
                v: 3000f64.ln(),
                t: 3000f64.ln(),
            },
            hp: [Bq::default(); 2],
            lp: [Bq::default(); 3],
            spk: [Bq::default(); 3],
            horn: Glide::default(),
            peaks: [Bq::default(); 3],
            ring: [[0.0; RING_LEN]; 2],
            ring_i: 0,
            e_in: 1.0,
            e_out: 0.5,
            mk: 2f64.sqrt(),
        }
    }
    fn design_band(&mut self) {
        let (lo, hi) = (self.lo.v.exp(), self.hi.v.exp());
        design(&mut self.hp, Kind::High, lo, &BUTTER4);
        design(&mut self.lp, Kind::Low, hi, &BUTTER6);
        design(&mut self.spk[..2], Kind::High, lo * 0.85, &BUTTER4);
        self.spk[2].design(Kind::Low, hi * 1.15, BUTTER2[0]);
    }
    fn design_horn(&mut self) {
        for (b, &(f, q, g)) in self.peaks.iter_mut().zip(&HORN_PEAKS) {
            b.design(Kind::Peak(g * self.horn.v), f, q);
        }
    }
    fn clear(&mut self) {
        clear(&mut self.hp);
        clear(&mut self.lp);
        clear(&mut self.spk);
        clear(&mut self.peaks);
        self.ring = [[0.0; RING_LEN]; 2];
    }

    #[inline]
    fn run(&mut self, x: f64, k: f64, ke: f64, recoef: bool) -> f64 {
        let e = self.en.step(ke);
        let h = self.horn.step(k);
        if e == 0.0 && h == 0.0 {
            return x;
        }
        if recoef && (self.lo.moving() || self.hi.moving()) {
            self.lo.step(k * COEF_EVERY as f64);
            self.hi.step(k * COEF_EVERY as f64);
            self.design_band();
        }
        if recoef && self.horn.moving() {
            self.design_horn();
        }
        let mut y = x;
        if e > 0.0 {
            y += (run(&mut self.hp, run(&mut self.lp, x)) - x) * e;
        }
        if h > 0.0 {
            y = run(&mut self.peaks, y);
            let i = self.ring_i;
            let mut r = 0.0;
            for (c, &d) in self.ring.iter_mut().zip(&RING_D) {
                let fb = c[(i + RING_LEN - d) % RING_LEN];
                let v = y + RING_FB * fb;
                c[i] = v;
                r += v - y;
            }
            self.ring_i = (i + 1) % RING_LEN;
            y += 0.5 * h * r;
        }
        // (the makeup: the energy in over the energy out, both while the voice is there)
        let kk = 1.0 / (TAU_MAKEUP * FS);
        if x * x > 1e-7 {
            self.e_in += (x * x - self.e_in) * kk;
            self.e_out += (y * y - self.e_out) * kk;
        }
        if recoef {
            self.mk = (self.e_in / self.e_out.max(1e-12)).sqrt().clamp(0.5, 4.0);
        }
        y * (1.0 + (self.mk - 1.0) * e.max(h))
    }

    /// the gentle copy of the band (after drive and lofi), as much as the band is on
    #[inline]
    fn speaker(&mut self, x: f64) -> f64 {
        let e = self.en.v;
        if e == 0.0 {
            return x;
        }
        x + (run(&mut self.spk, x) - x) * e
    }
}

/// echo: a feedback delay, darkened a little each time round, its delay gliding (tape-like) when it changes
struct Echo {
    en: Glide,
    d: Glide,
    fb: Glide,
    buf: Vec<f32>,
    w: usize,
    lp: f64,
    dirty: bool,
    norm: f64,
    norm_e: f64,
    norm_fb: f64,
}

const ECHO_BUF: usize = 65536;
/// the first repeat's loudness
const ECHO_MIX: f64 = 0.55;

impl Echo {
    fn new() -> Echo {
        Echo {
            en: Glide::default(),
            d: Glide {
                v: 0.25 * FS,
                t: 0.25 * FS,
            },
            fb: Glide::default(),
            buf: vec![0.0; ECHO_BUF],
            w: 0,
            lp: 0.0,
            dirty: false,
            norm: 1.0,
            norm_e: -1.0,
            norm_fb: -1.0,
        }
    }
    fn clear(&mut self) {
        if self.dirty {
            self.buf.fill(0.0);
            self.lp = 0.0;
            self.dirty = false;
        }
    }
    #[inline]
    fn run(&mut self, x: f64, k: f64, ke: f64) -> f64 {
        let e = self.en.step(ke);
        if e == 0.0 {
            self.clear();
            return x;
        }
        self.dirty = true;
        let d = self.d.step(k * 0.5);
        let fb = self.fb.step(k);
        let mask = ECHO_BUF - 1;
        let r = read_linear(&self.buf, mask, self.w.wrapping_sub(1), d - 1.0);
        self.lp += (r - self.lp) * 0.45; // (~4.5 kHz)
        self.buf[self.w & mask] = (x + fb * self.lp) as f32;
        self.w = self.w.wrapping_add(1);
        if e != self.norm_e || fb != self.norm_fb {
            let m = ECHO_MIX * e;
            self.norm = 1.0 / (1.0 + m * m / (1.0 - fb * fb)).sqrt();
            (self.norm_e, self.norm_fb) = (e, fb);
        }
        (x + ECHO_MIX * e * r) * self.norm
    }
}

/// reverb: Freeverb's eight damped combs and four all-passes (mono), after a pre-delay; decay 0.4 s at 0 .. 3 s at 1,
/// the wet part normalised to the dry's loudness and crossfaded in at equal power.
struct Reverb {
    amt: Glide,
    combs: Vec<Vec<f32>>,
    ci: [usize; 8],
    filt: [f64; 8],
    fb: [f64; 8],
    aps: Vec<Vec<f32>>,
    ai: [usize; 4],
    pre: Vec<f32>,
    pi: usize,
    dirty: bool,
    /// (the amount the mix below is for, dry and wet gains)
    mix_at: f64,
    dry: f64,
    wet: f64,
}

/// Freeverb's lengths (at 44.1 kHz)
const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
const REV_DAMP: f64 = 0.3;
const REV_PRE_MS: f64 = 18.0;
/// the wet part's level (measured: wet RMS = dry RMS for speech)
const REV_CAL: f64 = 0.29;
const REV_RT: f64 = 1.25;

fn scaled(n: usize) -> usize {
    (n as f64 * FS / 44100.0).round() as usize
}

impl Reverb {
    fn new() -> Reverb {
        Reverb {
            amt: Glide::default(),
            combs: COMBS.iter().map(|&n| vec![0.0; scaled(n)]).collect(),
            ci: [0; 8],
            filt: [0.0; 8],
            fb: [0.0; 8],
            aps: ALLPASSES.iter().map(|&n| vec![0.0; scaled(n)]).collect(),
            ai: [0; 4],
            pre: vec![0.0; ms(REV_PRE_MS) as usize],
            pi: 0,
            dirty: false,
            mix_at: -1.0,
            dry: 1.0,
            wet: 0.0,
        }
    }
    fn clear(&mut self) {
        if self.dirty {
            self.combs.iter_mut().for_each(|c| c.fill(0.0));
            self.aps.iter_mut().for_each(|c| c.fill(0.0));
            self.pre.fill(0.0);
            self.filt = [0.0; 8];
            self.dirty = false;
        }
        self.mix_at = -1.0;
    }
    /// the combs' feedback for the decay that amount `a` gives
    fn decay(&mut self, a: f64) {
        // (x REV_RT: the damping shortens the decay the combs' feedback alone would give)
        let rt = 0.4 * (3.0f64 / 0.4).powf(a.clamp(0.0, 1.0)) * REV_RT;
        for (f, c) in self.fb.iter_mut().zip(&self.combs) {
            *f = 10f64.powf(-3.0 * c.len() as f64 / (rt * FS));
        }
        self.mix_at = -1.0;
    }
    #[inline]
    fn run(&mut self, x: f64, k: f64, recoef: bool) -> f64 {
        let a = self.amt.step(k);
        if a == 0.0 {
            self.clear();
            return x;
        }
        self.dirty = true;
        if recoef && self.amt.moving() {
            self.decay(a);
        }
        let d = self.pre[self.pi] as f64;
        self.pre[self.pi] = x as f32;
        self.pi = (self.pi + 1) % self.pre.len();
        let mut s = 0.0;
        for i in 0..8 {
            let c = &mut self.combs[i];
            let o = c[self.ci[i]] as f64;
            self.filt[i] = o * (1.0 - REV_DAMP) + self.filt[i] * REV_DAMP;
            c[self.ci[i]] = (d + self.filt[i] * self.fb[i]) as f32;
            self.ci[i] += 1;
            if self.ci[i] == c.len() {
                self.ci[i] = 0;
            }
            s += o;
        }
        for i in 0..4 {
            let b = &mut self.aps[i];
            let bo = b[self.ai[i]] as f64;
            b[self.ai[i]] = (s + bo * 0.5) as f32;
            s = bo - s;
            self.ai[i] += 1;
            if self.ai[i] == b.len() {
                self.ai[i] = 0;
            }
        }
        // (eight combs of feedback g hold 8 / (1 - g^2) times the input's energy)
        if a != self.mix_at {
            let g = self.fb[0];
            let th = a.powf(0.8) * 0.6 * PI / 2.0;
            self.dry = th.cos();
            self.wet = th.sin() * REV_CAL * ((1.0 - g * g) / 8.0).sqrt();
            self.mix_at = a;
        }
        x * self.dry + s * self.wet
    }
}

// ---------------------------------------------------------------- the effect

/// One voice's chain of effects (one per device the voice comes out of; one on a voice's own sound).
pub struct Effect {
    rng: Rng,
    primed: bool,
    dead: bool,
    on: bool,
    /// samples since new (filters follow glides on multiples of COEF_EVERY)
    t: u64,
    env: f64,
    /// samples since the voice stopped
    off_t: u32,
    k: f64,
    ke: f64,

    pitch: Pitch,
    robot_en: Glide,
    robot_hz: Glide,
    robot_ph: f64,
    band: Band,
    drive: Glide,
    drive_mk: f64,
    drive_pre: f64,
    drive_at: f64,
    comp: Glide,
    comp_env: f64,
    comp_g: f64,
    lofi: Glide,
    lofi_ph: f64,
    lofi_hold: f64,
    wobble: Glide,
    wob_buf: [f32; WOB_BUF],
    wob_w: usize,
    wob_lfo: [Lfo; 3],
    wob_ph: f64,

    // the noise: hiss, crackle, squelch, hum (levels as output RMS)
    hiss: Glide,
    hum: Glide,
    hum_lvl: f64,
    ngate: f64,
    flutter: Glide,
    pop_rate: f64,
    burst_rate: f64,
    drop_rate: f64,
    squelch: f64,
    nlo: Glide,
    nhi: Glide,
    nfilt: [Bq; 3],
    nnorm: f64,
    wprev: f64,
    nlfo: Lfo,
    burst_left: u32,
    drop_left: u32,
    drop_chop: bool,
    fade_v: f64,
    fade_n: f64,
    chop: f64,
    pop: f64,
    click: f64,
    gate: f64,
    ku_t: u32,
    ku_len: u32,
    tail_t: u32,
    tail_len: u32,
    hum_tab: [f32; HUM_LEN],
    hum_i: usize,
    hum_pop: u32,

    echo: Echo,
    reverb: Reverb,
}

const WOB_BUF: usize = 512;
/// wobble's delay swing at 1 (samples; about +-30 cents)
const WOB_D: f64 = 60.0;
/// one cycle of the mains buzz (60 Hz)
const HUM_LEN: usize = 800;
/// the squelch tail and key-up burst at squelch 1 (dBFS RMS)
const TAIL_DB: f64 = -17.0;
/// a dropout's noise (dBFS RMS)
const DROP_DB: f64 = -27.0;
/// a pop's size before the band (peak)
const POP_AMP: f64 = 1.2;
/// noise when the band is off
const NOISE_BAND: (f64, f64) = (150.0, 7000.0);
/// the compressor's makeup reference: the level speech at -20 dBFS RMS sits at in its detector (dB)
const COMP_REF: f64 = -12.0;

impl Effect {
    /// All allocation for every block here; `seed` makes the noise differ per device.
    pub fn new(seed: u64) -> Effect {
        let mut hum_tab = [0.0f32; HUM_LEN];
        for (i, h) in hum_tab.iter_mut().enumerate() {
            let p = 2.0 * PI * i as f64 / HUM_LEN as f64;
            // (a transformer's buzz: 60 Hz and its harmonics, odd ones stronger; the band keeps the upper ones)
            let v: f64 = (1..=12)
                .map(|n| (n as f64 * p).sin() / (n as f64).powf(if n % 2 == 1 { 0.7 } else { 1.2 }))
                .sum();
            *h = (v * 0.42) as f32;
        }
        let mut e = Effect {
            rng: Rng::new(seed),
            primed: false,
            dead: true,
            on: false,
            t: 0,
            env: 0.0,
            off_t: 0,
            k: coef(TAU_PARAM),
            ke: coef(TAU_EN),
            pitch: Pitch::new(),
            robot_en: Glide::default(),
            robot_hz: Glide::default(),
            robot_ph: 0.0,
            band: Band::new(),
            drive: Glide::default(),
            drive_mk: 1.0,
            drive_pre: 1.0,
            drive_at: -1.0,
            comp: Glide::default(),
            comp_env: 0.0,
            comp_g: 1.0,
            lofi: Glide::default(),
            lofi_ph: 0.0,
            lofi_hold: 0.0,
            wobble: Glide::default(),
            wob_buf: [0.0; WOB_BUF],
            wob_w: 0,
            wob_lfo: [Lfo::new(0.8), Lfo::new(3.5), Lfo::new(2.0)],
            wob_ph: 0.0,
            hiss: Glide::default(),
            hum: Glide::default(),
            hum_lvl: 0.0,
            ngate: 0.0,
            flutter: Glide::default(),
            pop_rate: 0.0,
            burst_rate: 0.0,
            drop_rate: 0.0,
            squelch: 0.0,
            nlo: Glide {
                v: NOISE_BAND.0.ln(),
                t: NOISE_BAND.0.ln(),
            },
            nhi: Glide {
                v: NOISE_BAND.1.ln(),
                t: NOISE_BAND.1.ln(),
            },
            nfilt: [Bq::default(); 3],
            nnorm: 1.0,
            wprev: 0.0,
            nlfo: Lfo::new(5.0),
            burst_left: 0,
            drop_left: 0,
            drop_chop: false,
            fade_v: 1.0,
            fade_n: 0.0,
            chop: 1.0,
            pop: 0.0,
            click: 0.0,
            gate: 1.0,
            ku_t: 0,
            ku_len: 0,
            tail_t: 0,
            tail_len: 0,
            hum_tab,
            hum_i: 0,
            hum_pop: u32::MAX,
            echo: Echo::new(),
            reverb: Reverb::new(),
        };
        e.band.design_band();
        e.band.design_horn();
        e.design_noise();
        e.reverb.decay(0.5);
        e
    }

    /// The effects and a radio's reception (signal 0..1, 1 clear: lower adds hiss and crackle, dropouts below ~0.3).
    /// May be called every block: the values glide (no clicks).
    pub fn set(&mut self, fx: &Effects, signal: f64) {
        let fin = |v: f64, lo: f64, hi: f64| if v.is_finite() { v.clamp(lo, hi) } else { 0.0 };
        let bad = 1.0
            - if signal.is_finite() {
                signal.clamp(0.0, 1.0)
            } else {
                1.0
            };

        // (a block that was off takes its new values at once, and fades in)
        let pitch = fin(fx.pitch, -12.0, 12.0);
        self.pitch.st.t = pitch;
        self.pitch.en.t = if pitch != 0.0 { 1.0 } else { 0.0 };
        if self.pitch.en.v == 0.0 {
            self.pitch.st.snap();
        }
        let robot = fin(fx.robot, 0.0, 2000.0);
        self.robot_en.t = if robot > 0.0 { 1.0 } else { 0.0 };
        if robot > 0.0 {
            self.robot_hz.t = robot;
        }
        if self.robot_en.v == 0.0 {
            self.robot_hz.snap();
        }
        match fx.band {
            Some((lo, hi)) if lo.is_finite() && hi.is_finite() => {
                let lo = lo.clamp(20.0, 16000.0);
                let hi = hi.clamp(lo * 1.2, 20000.0);
                self.band.lo.t = lo.ln();
                self.band.hi.t = hi.ln();
                self.band.en.t = 1.0;
            }
            _ => self.band.en.t = 0.0,
        }
        if self.band.en.v == 0.0 && (self.band.lo.moving() || self.band.hi.moving()) {
            self.band.lo.snap();
            self.band.hi.snap();
            self.band.design_band();
        }
        self.band.horn.t = fin(fx.horn, 0.0, 1.0);
        self.drive.t = fin(fx.drive, 0.0, 1.0);
        self.comp.t = fin(fx.compress, 0.0, 1.0);
        self.lofi.t = fin(fx.lofi, 0.0, 1.0);
        self.wobble.t = fin(fx.wobble, 0.0, 1.0);
        match fx.echo {
            Some((d, fb)) if d.is_finite() && fb.is_finite() && d > 0.0 => {
                self.echo.d.t = d.clamp(0.02, 1.0) * FS;
                self.echo.fb.t = fb.clamp(0.0, 0.9);
                self.echo.en.t = 1.0;
            }
            _ => self.echo.en.t = 0.0,
        }
        if self.echo.en.v == 0.0 {
            self.echo.d.snap();
            self.echo.fb.snap();
        }
        let rev = fin(fx.reverb, 0.0, 1.0);
        if self.reverb.amt.v == 0.0 && rev > 0.0 {
            self.reverb.decay(rev);
        }
        self.reverb.amt.t = rev;

        // the noise: levels from the effects and the reception
        let hiss = fin(fx.hiss, 0.0, 1.0) + 0.8 * bad.powf(1.2);
        self.hiss.t = if hiss > 0.0 {
            db(-62.0 + 42.0 * hiss.min(1.0))
        } else {
            0.0
        };
        let crackle = (fin(fx.crackle, 0.0, 1.0) + bad.powf(1.5)).min(1.0);
        self.pop_rate = 30.0 * crackle * crackle;
        self.burst_rate = if crackle > 0.2 {
            3.0 * (crackle - 0.2) / 0.8
        } else {
            0.0
        };
        self.drop_rate = if crackle > 0.45 {
            5.0 * ((crackle - 0.45) / 0.55).powf(1.2)
        } else {
            0.0
        };
        self.flutter.t = 0.5 * bad + 0.3 * crackle;
        self.squelch = fin(fx.squelch, 0.0, 1.0);
        let hum = fin(fx.hum, 0.0, 1.0);
        self.hum_lvl = if hum > 0.0 {
            db(-62.0 + 32.0 * hum)
        } else {
            0.0
        };
        self.hum.t = if self.on { self.hum_lvl } else { 0.0 };
        let (lo, hi) = match fx.band {
            Some(_) => (self.band.lo.t, self.band.hi.t),
            None => (NOISE_BAND.0.ln(), NOISE_BAND.1.ln()),
        };
        self.nlo.t = lo;
        self.nhi.t = hi;

        if !self.primed {
            self.primed = true;
            self.snap();
        }
    }

    /// every glide to its target (the first set, and waking up)
    fn snap(&mut self) {
        for g in [
            &mut self.pitch.st,
            &mut self.pitch.en,
            &mut self.robot_en,
            &mut self.robot_hz,
            &mut self.band.en,
            &mut self.band.lo,
            &mut self.band.hi,
            &mut self.band.horn,
            &mut self.drive,
            &mut self.comp,
            &mut self.lofi,
            &mut self.wobble,
            &mut self.hiss,
            &mut self.hum,
            &mut self.flutter,
            &mut self.nlo,
            &mut self.nhi,
            &mut self.echo.en,
            &mut self.echo.d,
            &mut self.echo.fb,
            &mut self.reverb.amt,
        ] {
            g.snap();
        }
        self.band.design_band();
        self.band.design_horn();
        self.design_noise();
        self.reverb.decay(self.reverb.amt.v);
    }

    fn design_noise(&mut self) {
        let (lo, hi) = (self.nlo.v.exp(), self.nhi.v.exp());
        self.nfilt[0].design(Kind::High, lo, BUTTER2[0]);
        design(&mut self.nfilt[1..], Kind::Low, hi, &BUTTER4);
        // (white noise of variance 1/3, pre-emphasised (w - 0.5 w'), through the band: its RMS, to make it 1)
        let (w1, w2) = (2.0 * PI * lo / FS, 2.0 * PI * hi.max(lo * 1.1) / FS);
        let emph = 1.25 - (w2.sin() - w1.sin()) / (w2 - w1);
        let frac = (hi - lo).max(10.0) / (FS / 2.0);
        self.nnorm = 1.0 / (emph * frac / 3.0).sqrt();
    }

    /// everything back to silence (the voice and its tails are done)
    fn reset(&mut self) {
        self.pitch.clear();
        self.band.clear();
        self.comp_env = 0.0;
        self.wob_buf = [0.0; WOB_BUF];
        clear(&mut self.nfilt);
        self.echo.clear();
        self.reverb.clear();
        self.pop = 0.0;
        self.click = 0.0;
        self.burst_left = 0;
        self.drop_left = 0;
        self.fade_v = 1.0;
        self.fade_n = 0.0;
        self.chop = 1.0;
        self.ku_len = 0;
        self.tail_len = 0;
        self.hum_pop = u32::MAX;
        self.env = 0.0;
        self.ngate = 0.0;
        self.gate = 1.0;
    }

    /// the talk button went down
    fn key_up(&mut self) {
        if self.dead {
            self.snap();
            self.dead = false;
        }
        self.tail_len = 0;
        self.off_t = 0;
        if self.squelch > 0.0 {
            self.ku_t = 0;
            self.ku_len = ms(self.rng.range(30.0, 60.0)) as u32;
            self.click += 0.6 * self.squelch;
            self.gate = 0.0;
        }
        if self.hum.t > 0.0 {
            self.hum_pop = 0;
        }
    }

    /// the talk button came up
    fn key_down(&mut self) {
        self.ku_len = 0;
        self.drop_left = 0;
        self.burst_left = 0;
        if self.squelch > 0.0 {
            self.tail_t = 0;
            self.tail_len = ms(self.rng.range(150.0, 250.0)) as u32;
            self.click -= 0.35 * self.squelch;
        }
    }

    /// One block, mono at 48 kHz: `x` the voice (zeros when nothing comes), `on` whether a voice is coming through
    /// in this block. Writes out[..x.len()]. Returns true while it makes sound (the voice, a squelch tail, a reverb
    /// dying away); once false the caller may stop calling until `on` again.
    pub fn process(&mut self, x: &[f32], on: bool, out: &mut [f32]) -> bool {
        let n = x.len().min(out.len());
        if !self.primed {
            self.set(&Effects::default(), 1.0);
        }
        if on && !self.on {
            self.key_up();
        } else if !on && self.on {
            self.key_down();
        }
        self.on = on;
        if self.dead {
            out[..n].fill(0.0);
            return false;
        }
        let fall = db(-ENV_FALL_DB / FS);
        let dead_at = db(DEAD_DB);
        let mut sound = false;
        for i in 0..n {
            if self.dead {
                out[i] = 0.0;
                continue;
            }
            // (t counts only samples processed: a dead effect restarts its count where it stopped, as the mixer
            // stops calling it)
            let (y, clean) = self.sample(x[i] as f64);
            let y = if clean {
                y
            } else if y.abs() > 0.9 {
                // (a soft limiter: peaks stay under 1)
                y.signum() * (0.9 + 0.1 * soft((y.abs() - 0.9) / 0.1))
            } else {
                y
            };
            let mut y = if y.is_finite() { y } else { 0.0 };
            if !self.on {
                self.off_t = self.off_t.saturating_add(1);
                let t = self.off_t as f64 / FS;
                if t > TAIL_FADE {
                    y *= ((TAIL_MAX - t) / (TAIL_MAX - TAIL_FADE)).max(0.0);
                }
            }
            out[i] = y as f32;
            sound |= y != 0.0;
            self.t += 1;
            self.env = y.abs().max(self.env * fall);
            if !self.on
                && self.tail_len == 0
                && (self.env < dead_at || self.off_t as f64 > TAIL_MAX * FS)
            {
                self.reset();
                self.dead = true;
            }
        }
        !self.dead || sound
    }

    /// one sample through the chain; (y, whether every block was off)
    #[inline]
    fn sample(&mut self, x: f64) -> (f64, bool) {
        let (k, ke) = (self.k, self.ke);
        let recoef = self.t.is_multiple_of(COEF_EVERY);
        let mut clean = true;

        // pitch
        let mut v = self.pitch.run(x, k, ke);
        clean &= !self.pitch.en.on();

        // robot: ring modulation
        let re = self.robot_en.step(ke);
        if re > 0.0 {
            clean = false;
            let hz = self.robot_hz.step(k);
            self.robot_ph += hz / FS;
            self.robot_ph -= self.robot_ph.floor();
            let m = std::f64::consts::SQRT_2 * (2.0 * PI * self.robot_ph).sin();
            v *= 1.0 - re + re * m;
        }

        // band + horn
        if self.band.en.on() || self.band.horn.on() {
            clean = false;
            v = self.band.run(v, k, ke, recoef);
        }

        // drive: saturation going hard with more drive; the level kept for speech around -20 dBFS
        let d = self.drive.step(k);
        if d > 0.0 {
            clean = false;
            if d != self.drive_at {
                let pre = db(26.0 * d);
                let r = 0.2;
                let sat = |u: f64| soft(u) + (hard(u) - soft(u)) * d * d;
                self.drive_mk = r / sat(pre * r) * (1.0 + 0.3 * d);
                self.drive_pre = pre;
                self.drive_at = d;
            }
            let u = self.drive_pre * v;
            let s = (soft(u) + (hard(u) - soft(u)) * d * d) * self.drive_mk;
            let w = (d / 0.1).min(1.0);
            v += (s - v) * w;
        }

        // compress
        let c = self.comp.step(k);
        if c > 0.0 {
            clean = false;
            let p = v * v;
            let a = if p > self.comp_env { 0.0069 } else { 0.000_21 }; // (attack 3 ms, release 100 ms)
            self.comp_env += (p - self.comp_env) * a;
            if self.t.is_multiple_of(4) {
                let thr = -16.0 - 16.0 * c;
                let slope = 1.0 - 1.0 / (1.0 + 7.0 * c);
                let lvl = 10.0 * (self.comp_env + 1e-12).log10();
                let gr = if lvl > thr { (lvl - thr) * slope } else { 0.0 };
                let mk = (COMP_REF - thr).max(0.0) * slope;
                self.comp_g = db(mk - gr);
            }
            v *= self.comp_g;
        }

        // lofi: a lower sample rate (held samples) and fewer bits (mu-law)
        let l = self.lofi.step(k);
        if l > 0.0 {
            clean = false;
            let hold = 1.0 + 7.0 * l;
            self.lofi_ph += 1.0;
            if self.lofi_ph >= hold {
                self.lofi_ph -= hold;
                let levels = (13.0 - 9.0 * l).exp2();
                let u = v.clamp(-1.0, 1.0);
                let m = (1.0 + 255.0 * u.abs()).ln() / 256f64.ln();
                let q = (m * levels).round() / levels;
                self.lofi_hold = u.signum() * (256f64.powf(q) - 1.0) / 255.0;
            }
            let w = (l / 0.05).min(1.0);
            v += (self.lofi_hold - v) * w;
        }

        // wobble: a slowly wandering delay (pitch) and level
        self.wob_buf[self.wob_w & (WOB_BUF - 1)] = v as f32;
        self.wob_w = self.wob_w.wrapping_add(1);
        let wb = self.wobble.step(k);
        if wb > 0.0 {
            clean = false;
            self.wob_ph += 0.9 / FS;
            self.wob_ph -= self.wob_ph.floor();
            let (r1, r2, r3) = (self.rng.white(), self.rng.white(), self.rng.white());
            let slow = 0.5 * self.wob_lfo[0].step(r1) + 0.3 * (2.0 * PI * self.wob_ph).sin();
            let fast = 0.35 * self.wob_lfo[1].step(r2);
            let lvl = self.wob_lfo[2].step(r3);
            let dl = (wb * WOB_D * (1.0 + slow + fast)).clamp(0.0, WOB_BUF as f64 - 8.0);
            v = read_cubic(&self.wob_buf, WOB_BUF - 1, self.wob_w - 1, dl);
            v *= (1.0 + 0.12 * wb * lvl).max(0.3);
        }

        // the gentle speaker copy of the band
        v = self.band.speaker(v);

        // the noise and the squelch
        let hiss = self.hiss.step(k);
        self.hum.t = if self.on { self.hum_lvl } else { 0.0 };
        let hum = self.hum.step(k);
        let noisy = hiss > 0.0
            || hum > 0.0
            || self.pop_rate > 0.0
            || self.squelch > 0.0
            || self.tail_len > 0
            || self.click != 0.0
            || self.pop != 0.0;
        if noisy {
            clean = false;
            v = self.noise(v, hiss, hum, k, recoef);
        }

        // echo, reverb
        if self.echo.en.on() || self.echo.dirty {
            clean = false;
            v = self.echo.run(v, k, ke);
        }
        if self.reverb.amt.on() || self.reverb.dirty {
            clean = false;
            v = self.reverb.run(v, k, recoef);
        }
        (v, clean)
    }

    /// The receiver: the voice v (gated by the squelch and dropouts) plus hiss, crackle, the squelch's click, key-up
    /// burst and tail, and the hum, all through the band.
    #[inline]
    fn noise(&mut self, v: f64, hiss: f64, hum: f64, k: f64, recoef: bool) -> f64 {
        if recoef && (self.nlo.moving() || self.nhi.moving()) {
            self.nlo.step(k * COEF_EVERY as f64);
            self.nhi.step(k * COEF_EVERY as f64);
            self.design_noise();
        }
        let on = self.on;
        let kf = 0.01; // (~2 ms)
        let fl = self.flutter.step(k);
        let w = self.rng.white();
        let e = w - 0.5 * self.wprev;
        self.wprev = w;
        let lf = self.nlfo.step(self.rng.white());
        self.ngate += (if on { 1.0 } else { 0.0 } - self.ngate) * 0.002;
        if self.ngate < 1e-6 {
            self.ngate = 0.0;
        }
        let mut lvl = hiss * self.ngate * (1.0 + fl * lf).max(0.0);

        // crackle: pops, bursts of them, dropouts (only while a voice comes through)
        let mut pop_rate = self.pop_rate;
        if on && self.pop_rate > 0.0 {
            if self.burst_left > 0 {
                self.burst_left -= 1;
                pop_rate *= 15.0;
                lvl *= 2.5;
            } else if self.rng.uniform() < self.burst_rate / FS {
                self.burst_left = ms(self.rng.range(20.0, 80.0)) as u32;
            }
            if self.drop_left > 0 {
                self.drop_left -= 1;
            } else if self.drop_rate > 0.0 && self.rng.uniform() < self.drop_rate / FS {
                self.drop_left = ms(self.rng.range(40.0, 160.0)) as u32;
                self.drop_chop = self.squelch > 0.0 && self.rng.uniform() < 0.5;
            }
            if self.rng.uniform() < pop_rate / FS {
                let s = if self.rng.uniform() < 0.5 { -1.0 } else { 1.0 };
                self.pop += s * POP_AMP * self.rng.range(0.3, 1.0);
            }
        }
        let dropping = on && self.drop_left > 0;
        let (vt, nt, ct) = match (dropping, self.drop_chop) {
            (true, true) => (0.0, 0.0, 0.0),
            (true, false) => (0.03, db(DROP_DB), 1.0),
            _ => (1.0, 0.0, 1.0),
        };
        self.fade_v += (vt - self.fade_v) * kf;
        self.fade_n += (nt - self.fade_n) * kf;
        self.chop += (ct - self.chop) * kf;
        lvl = lvl.max(self.fade_n);

        // the squelch: key-up burst, tail, and the voice cut while it is closed
        if self.squelch > 0.0 {
            let tail = db(TAIL_DB) * self.squelch;
            if self.ku_t < self.ku_len {
                lvl += tail * (-(self.ku_t as f64) / ms(14.0)).exp();
                self.ku_t += 1;
            }
            if self.tail_t < self.tail_len {
                let t = self.tail_t as f64;
                let len = self.tail_len as f64;
                let env =
                    (t / ms(1.5)).min(1.0) * (1.0 - 0.3 * t / len) * ((len - t) / ms(4.0)).min(1.0);
                lvl += tail * env;
                self.tail_t += 1;
                if self.tail_t == self.tail_len {
                    self.tail_len = 0;
                }
            }
            let gt = if on && self.ku_t as f64 >= ms(8.0).min(self.ku_len as f64) {
                1.0
            } else {
                0.0
            };
            self.gate += (gt - self.gate) * if gt > self.gate { 0.0035 } else { 0.015 };
        } else {
            self.gate = 1.0;
        }

        let mut n = e * lvl * self.nnorm + self.pop + self.click;
        self.pop *= 0.55;
        self.click *= 0.75;
        if self.pop.abs() < 1e-9 {
            self.pop = 0.0;
        }
        if self.click.abs() < 1e-9 {
            self.click = 0.0;
        }
        // the hum, while a voice comes through; a soft pop when it starts
        if hum > 0.0 {
            let h = self.hum_tab[self.hum_i] as f64;
            self.hum_i = (self.hum_i + 1) % HUM_LEN;
            n += h * hum * 1.6;
        }
        if self.hum_pop < ms(8.0) as u32 {
            let t = self.hum_pop as f64 / ms(8.0);
            n += 0.25 * (PI * t).sin().powi(2) * (1.0 - t);
            self.hum_pop += 1;
        }
        let n = run(&mut self.nfilt, n);
        (v * self.fade_v * self.gate + n) * self.chop
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kd_common::feed::Device;
    use realfft::RealFftPlanner;

    const B: usize = 480;

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|&v| v as f64 * v as f64).sum::<f64>() / x.len().max(1) as f64).sqrt()
    }

    fn dbfs(v: f64) -> f64 {
        20.0 * v.max(1e-12).log10()
    }

    /// RMS of x and of y over the 20 ms frames where x is louder than -45 dBFS
    fn active(x: &[f32], y: &[f32]) -> (f64, f64) {
        let f = 960;
        let (mut ex, mut ey, mut n) = (0.0, 0.0, 0);
        for (a, b) in x.chunks(f).zip(y.chunks(f)) {
            if dbfs(rms(a)) > -45.0 {
                ex += a.iter().map(|&v| v as f64 * v as f64).sum::<f64>();
                ey += b.iter().map(|&v| v as f64 * v as f64).sum::<f64>();
                n += a.len();
            }
        }
        ((ex / n as f64).sqrt(), (ey / n as f64).sqrt())
    }

    /// A speech-like voice: a glottal pulse train with intonation through three moving formants, syllables and
    /// pauses, some hissing consonants; -20 dBFS RMS over its voiced frames.
    fn speech(secs: f64, seed: u64) -> Vec<f32> {
        let mut rng = Rng::new(seed);
        let n = (secs * FS) as usize;
        let vowels = [
            (730.0, 1090.0, 2440.0),
            (270.0, 2290.0, 3010.0),
            (300.0, 870.0, 2240.0),
            (530.0, 1840.0, 2480.0),
            (570.0, 840.0, 2410.0),
        ];
        let (mut ph, mut g1, mut g2) = (0.0f64, 0.0f64, 0.0f64);
        let mut res = [[0.0f64; 2]; 3];
        let mut fm = [500.0f64, 1500.0, 2500.0];
        let mut target = vowels[0];
        let mut fric = false;
        let mut hp = Bq::default();
        hp.design(Kind::High, 3000.0, 0.7);
        let syl = (0.22 * FS) as usize;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f64 / FS;
            if i % syl == 0 {
                target = vowels[(rng.uniform() * 5.0) as usize % 5];
                fric = rng.uniform() < 0.2;
            }
            let f0 = 115.0 + 25.0 * (2.0 * PI * 0.35 * t).sin() + 3.0 * rng.white();
            ph += f0 / FS;
            let mut src = 0.0;
            if ph >= 1.0 {
                ph -= 1.0;
                src = 1.0;
            }
            // (glottal tilt: two one-poles)
            g1 += (src - g1) * 0.05;
            g2 += (g1 - g2) * 0.05;
            let tt = [target.0, target.1, target.2];
            let mut v = 0.0;
            for k in 0..3 {
                fm[k] += (tt[k] - fm[k]) * 0.002;
                let (r, w) = (
                    1.0 - PI * (60.0 + 30.0 * k as f64) / FS,
                    2.0 * PI * fm[k] / FS,
                );
                let y = g2 * 40.0 + 2.0 * r * w.cos() * res[k][0] - r * r * res[k][1];
                res[k][1] = res[k][0];
                res[k][0] = y;
                v += y / (1.0 + k as f64);
            }
            let p = (i % syl) as f64 / syl as f64;
            let env = (PI * p).sin().powi(2);
            let pause = (t % 1.7) > 1.45;
            let s = if pause {
                0.0
            } else if fric {
                hp.run(rng.white()) * env * 0.5
            } else {
                v * env
            };
            out.push(s as f32);
        }
        for _ in 0..3 {
            let (r, _) = active(&out, &out);
            out.iter_mut().for_each(|v| *v *= (0.1 / r) as f32);
        }
        out
    }

    fn tone(hz: f64, amp: f64, secs: f64) -> Vec<f32> {
        (0..(secs * FS) as usize)
            .map(|i| (amp * (2.0 * PI * hz * i as f64 / FS).sin()) as f32)
            .collect()
    }

    fn noise(amp: f64, secs: f64, seed: u64) -> Vec<f32> {
        let mut r = Rng::new(seed);
        (0..(secs * FS) as usize)
            .map(|_| (amp * r.white() * 3f64.sqrt()) as f32)
            .collect()
    }

    /// x through an effect in blocks of `b`, `on` for the first `on_len` samples (by block), always called
    fn run_fx(
        fx: &Effects,
        signal: f64,
        x: &[f32],
        on_len: usize,
        b: usize,
        seed: u64,
    ) -> Vec<f32> {
        let mut e = Effect::new(seed);
        let mut y = vec![0.0f32; x.len()];
        for (i, (xb, yb)) in x.chunks(b).zip(y.chunks_mut(b)).enumerate() {
            e.set(fx, signal);
            e.process(xb, i * b < on_len, yb);
        }
        y
    }

    /// power spectrum (Hann, summed over 8192-blocks), bins of FS / 8192 Hz
    fn spectrum(y: &[f32]) -> Vec<f64> {
        let n = 8192;
        let mut planner = RealFftPlanner::<f64>::new();
        let fft = planner.plan_fft_forward(n);
        let mut spec = vec![0.0f64; n / 2 + 1];
        let mut buf = fft.make_input_vec();
        let mut out = fft.make_output_vec();
        for c in y.chunks_exact(n) {
            for (i, b) in buf.iter_mut().enumerate() {
                *b = c[i] as f64 * (0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos());
            }
            fft.process(&mut buf, &mut out).unwrap();
            for (s, o) in spec.iter_mut().zip(&out) {
                *s += o.norm_sqr();
            }
        }
        spec
    }

    fn energy(spec: &[f64], lo: f64, hi: f64) -> f64 {
        let hz = FS / 8192.0;
        spec.iter()
            .enumerate()
            .filter(|(i, _)| *i as f64 * hz >= lo && (*i as f64 * hz) < hi)
            .map(|(_, v)| v)
            .sum()
    }

    fn clean() -> Effects {
        Effects::default()
    }

    // ------------------------------------------------ the whole

    #[test]
    fn all_off_is_passthrough() {
        let x = speech(3.0, 1);
        let y = run_fx(&clean(), 1.0, &x, x.len(), B, 3);
        assert!(x.iter().zip(&y).all(|(a, b)| (a - b).abs() <= 1e-6));
    }

    #[test]
    fn presets_keep_the_loudness() {
        let x = speech(6.0, 2);
        for (d, sig) in [
            (Device::Radio, 1.0),
            (Device::Radio, 0.6),
            (Device::Loudspeaker, 1.0),
            (Device::Pa, 1.0),
        ] {
            let y = run_fx(&Effects::preset(d), sig, &x, x.len(), B, 5);
            let (ri, ro) = active(&x, &y);
            let dd = dbfs(ro) - dbfs(ri);
            println!(
                "{d:?} signal {sig}: in {:.1} dBFS, out {:.1} dBFS ({dd:+.1} dB)",
                dbfs(ri),
                dbfs(ro)
            );
            assert!(dd.abs() < 3.0, "{d:?}: {dd:+.1} dB");
            let peak = y.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak <= 1.0);
        }
    }

    #[test]
    fn whisper_quieter_shout_louder() {
        let x = speech(6.0, 3);
        let at = |g: f64| -> f64 {
            let xs: Vec<f32> = x
                .iter()
                .map(|v| (v * db(g) as f32).clamp(-1.0, 1.0))
                .collect();
            let y = run_fx(&Effects::preset(Device::Radio), 1.0, &xs, xs.len(), B, 5);
            dbfs(active(&x, &y).1)
        };
        let (w, n, s) = (at(-18.0), at(0.0), at(10.0));
        println!("radio: whisper {w:.1}, normal {n:.1}, shout {s:.1} dBFS");
        assert!(
            n - w > 6.0 && s - n > 2.0,
            "whisper {w:.1} normal {n:.1} shout {s:.1}"
        );
    }

    #[test]
    fn same_whatever_the_block_size() {
        let x = speech(6.0, 4);
        let mut all = Effects::preset(Device::Radio);
        all.pitch = 4.0;
        all.robot = 30.0;
        all.wobble = 0.5;
        all.echo = Some((0.2, 0.5));
        all.reverb = 0.6;
        all.hum = 0.4;
        all.horn = 0.5;
        for (fx, sig) in [
            (Effects::preset(Device::Radio), 0.25),
            (Effects::preset(Device::Pa), 1.0),
            (all, 0.3),
        ] {
            let on = 4800 * 30;
            let a = run_fx(&fx, sig, &x, on, 4800, 9);
            let b = run_fx(&fx, sig, &x, on, 480, 9);
            let worst = a
                .iter()
                .zip(&b)
                .map(|(p, q)| (p - q).abs())
                .fold(0.0f32, f32::max);
            assert!(worst < 1e-5, "{worst}");
            assert!(rms(&a) > 0.01);
        }
    }

    #[test]
    fn nothing_breaks() {
        let wild = Effects {
            band: Some((50.0, 18000.0)),
            drive: 1.0,
            compress: 1.0,
            hiss: 1.0,
            crackle: 1.0,
            squelch: 1.0,
            horn: 1.0,
            lofi: 1.0,
            wobble: 1.0,
            pitch: 12.0,
            robot: 1500.0,
            echo: Some((0.02, 0.9)),
            reverb: 1.0,
            hum: 1.0,
        };
        let x: Vec<f32> = noise(2.0, 4.0, 3)
            .iter()
            .map(|v| v.clamp(-1.0, 1.0))
            .collect();
        let odd = Effects {
            band: Some((f64::NAN, 3.0)),
            pitch: f64::INFINITY,
            echo: Some((-1.0, 5.0)),
            ..wild
        };
        for fx in [
            wild,
            Effects {
                pitch: -12.0,
                ..wild
            },
            odd,
        ] {
            for sig in [0.0, 0.5, f64::NAN] {
                let y = run_fx(&fx, sig, &x, 3 * 48000, B, 1);
                assert!(y.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
            }
        }
    }

    /// after the voice: done (process false) within 3 s, and only zeros from then on
    #[test]
    fn silence_after_the_tails() {
        let x = speech(2.0, 5);
        for fx in [
            Effects::preset(Device::Radio),
            Effects::preset(Device::Loudspeaker),
            Effects {
                reverb: 1.0,
                ..Effects::preset(Device::Pa)
            },
            Effects {
                echo: Some((1.0, 0.9)),
                reverb: 1.0,
                ..clean()
            },
        ] {
            let mut e = Effect::new(2);
            let mut y = vec![0.0f32; B];
            for c in x.chunks(B) {
                e.set(&fx, 0.5);
                assert!(e.process(c, true, &mut y));
            }
            let z = vec![0.0f32; B];
            let mut n = 0;
            while e.process(&z, false, &mut y) {
                n += 1;
                assert!(n * B < 3 * 48000, "{fx:?} still sounding after 3 s");
            }
            println!("done {:.2} s after the voice", (n * B) as f64 / FS);
            for _ in 0..20 {
                assert!(!e.process(&z, false, &mut y));
                assert!(y.iter().all(|&v| v == 0.0));
            }
        }
    }

    #[test]
    fn the_radio_squelch_tail() {
        let x = speech(2.0, 6);
        let mut x2 = x.clone();
        x2.resize(x.len() + 48000, 0.0);
        let on = x.len() / B * B;
        let y = run_fx(&Effects::preset(Device::Radio), 1.0, &x2, on, B, 4);
        let at = |a: f64, b: f64| dbfs(rms(&y[on + ms(a) as usize..on + ms(b) as usize]));
        let (tail, after) = (at(0.0, 140.0), at(300.0, 600.0));
        let len = y[on..].iter().rposition(|&v| v.abs() > 1e-3).unwrap_or(0) as f64 / FS * 1000.0;
        println!("squelch tail {tail:.1} dBFS, {len:.0} ms; after 300 ms {after:.1} dBFS");
        assert!(tail > -30.0 && (140.0..=260.0).contains(&len));
        assert!(y[on + ms(300.0) as usize..].iter().all(|&v| v.abs() < 1e-3));
        assert!(y[on + ms(400.0) as usize..].iter().all(|&v| v == 0.0));
    }

    // ------------------------------------------------ each block alone

    #[test]
    fn block_band() {
        let x = noise(0.1, 3.0, 1);
        let y = run_fx(
            &Effects {
                band: Some((300.0, 3000.0)),
                ..clean()
            },
            1.0,
            &x,
            x.len(),
            B,
            1,
        );
        let (sx, sy) = (spectrum(&x[48000..]), spectrum(&y[48000..]));
        let out = |s: &[f64]| {
            (energy(s, 0.0, 240.0) + energy(s, 3750.0, 24000.0)) / energy(s, 0.0, 24000.0)
        };
        println!(
            "band: outside 240..3750 Hz {:.1}% in, {:.3}% out",
            out(&sx) * 100.0,
            out(&sy) * 100.0
        );
        assert!(out(&sx) > 0.8 && out(&sy) < 0.01);
    }

    #[test]
    fn block_horn() {
        let x = noise(0.1, 3.0, 2);
        let y = run_fx(
            &Effects {
                horn: 1.0,
                ..clean()
            },
            1.0,
            &x,
            x.len(),
            B,
            1,
        );
        let (sx, sy) = (spectrum(&x[48000..]), spectrum(&y[48000..]));
        let peak = |s: &[f64]| energy(s, 2300.0, 2500.0) / energy(s, 600.0, 800.0);
        let gain = 10.0 * (peak(&sy) / peak(&sx)).log10();
        println!("horn: 2.4 kHz over 700 Hz {gain:+.1} dB");
        assert!(gain > 4.0);
    }

    #[test]
    fn block_drive() {
        let x = tone(220.0, 0.15, 2.0);
        let y = run_fx(
            &Effects {
                drive: 0.7,
                ..clean()
            },
            1.0,
            &x,
            x.len(),
            B,
            1,
        );
        let (sx, sy) = (spectrum(&x[24000..]), spectrum(&y[24000..]));
        let thd = |s: &[f64]| energy(s, 600.0, 700.0) / energy(s, 200.0, 240.0);
        let (a, b) = (10.0 * thd(&sx).log10(), 10.0 * thd(&sy).log10());
        println!("drive: 3rd harmonic {a:.0} dB -> {b:.0} dB");
        assert!(b > -30.0 && b - a > 30.0);
    }

    #[test]
    fn block_compress() {
        let x = speech(4.0, 7);
        let lvl = |g: f64, c: f64| {
            let xs: Vec<f32> = x.iter().map(|v| v * db(g) as f32).collect();
            let y = run_fx(
                &Effects {
                    compress: c,
                    ..clean()
                },
                1.0,
                &xs,
                xs.len(),
                B,
                1,
            );
            dbfs(active(&x, &y).1)
        };
        for c in [0.2, 0.5, 1.0] {
            println!("compress {c}: 0 dB in -> {:.1} dBFS", lvl(0.0, c));
        }
        let (q, m, l) = (lvl(-18.0, 0.8), lvl(0.0, 0.8), lvl(8.0, 0.8));
        println!("compress 0.8: -18 / 0 / +8 dB in -> {q:.1} / {m:.1} / {l:.1} dBFS (26 dB apart before)");
        assert!(l - q < 15.0 && l > q && (m + 20.0).abs() < 2.0);
    }

    #[test]
    fn block_hiss_and_signal() {
        let z = vec![0.0f32; 96000];
        let lvl = |h: f64, s: f64| {
            dbfs(rms(&run_fx(
                &Effects { hiss: h, ..clean() },
                s,
                &z,
                z.len(),
                B,
                1,
            )[9600..]))
        };
        let (a, b, c) = (lvl(0.2, 1.0), lvl(0.8, 1.0), lvl(0.2, 0.3));
        println!("hiss 0.2: {a:.1}, 0.8: {b:.1}, 0.2 at signal 0.3: {c:.1} dBFS");
        assert!(a > -70.0 && b > a + 15.0 && c > a + 10.0);
        assert!(run_fx(
            &Effects {
                hiss: 0.5,
                ..clean()
            },
            1.0,
            &z,
            0,
            B,
            1
        )
        .iter()
        .all(|&v| v == 0.0));
    }

    #[test]
    fn block_crackle() {
        // pops: a spiky noise; dropouts: the voice gone for a while
        let z = vec![0.0f32; 4 * 48000];
        let y = run_fx(
            &Effects {
                crackle: 0.8,
                ..clean()
            },
            1.0,
            &z,
            z.len(),
            B,
            1,
        );
        let crest = y.iter().fold(0.0f32, |m, v| m.max(v.abs())) as f64 / rms(&y);
        let x = tone(1000.0, 0.1, 6.0);
        let y = run_fx(
            &Effects {
                crackle: 1.0,
                ..clean()
            },
            1.0,
            &x,
            x.len(),
            B,
            1,
        );
        // (the tone's own level per 20 ms: 20 whole cycles)
        let level = |c: &[f32]| {
            let (mut s, mut co) = (0.0, 0.0);
            for (i, &v) in c.iter().enumerate() {
                let w = 2.0 * PI * 1000.0 * i as f64 / FS;
                s += v as f64 * w.sin();
                co += v as f64 * w.cos();
            }
            2.0 * (s * s + co * co).sqrt() / c.len() as f64
        };
        let gone = y
            .as_chunks::<960>()
            .0
            .iter()
            .filter(|c| level(&c[..]) < 0.1 * db(-15.0))
            .count();
        println!("crackle: crest factor {crest:.1}, 20 ms frames dropped {gone} of 300");
        assert!(crest > 8.0 && gone >= 5);
    }

    #[test]
    fn block_squelch() {
        let z = vec![0.0f32; 48000];
        let on = 24000;
        let y = run_fx(
            &Effects {
                squelch: 1.0,
                ..clean()
            },
            1.0,
            &z,
            on,
            B,
            1,
        );
        let keyup = dbfs(rms(&y[..ms(30.0) as usize]));
        let tail = dbfs(rms(&y[on..on + ms(140.0) as usize]));
        let len = y[on..].iter().rposition(|&v| v.abs() > 1e-3).unwrap_or(0) as f64 / FS * 1000.0;
        println!("squelch: key-up burst {keyup:.1} dBFS, tail {tail:.1} dBFS, {len:.0} ms long");
        assert!(keyup > -35.0 && tail > -25.0 && (140.0..=260.0).contains(&len));
        assert!(y[ms(100.0) as usize..on].iter().all(|&v| v.abs() < 1e-3));
    }

    #[test]
    fn block_lofi() {
        let x = tone(3000.0, 0.3, 2.0);
        let y = run_fx(
            &Effects {
                lofi: 0.7,
                ..clean()
            },
            1.0,
            &x,
            x.len(),
            B,
            1,
        );
        let (sx, sy) = (spectrum(&x[24000..]), spectrum(&y[24000..]));
        let junk = |s: &[f64]| 1.0 - energy(s, 2900.0, 3100.0) / energy(s, 0.0, 24000.0);
        println!(
            "lofi: energy off the tone {:.2e} -> {:.2e}",
            junk(&sx),
            junk(&sy)
        );
        assert!(junk(&sx) < 1e-4 && junk(&sy) > 0.01);
    }

    #[test]
    fn block_wobble() {
        let x = tone(1000.0, 0.2, 6.0);
        let y = run_fx(
            &Effects {
                wobble: 1.0,
                ..clean()
            },
            1.0,
            &x,
            x.len(),
            B,
            1,
        );
        // (frequency per 50 ms from its zero crossings, level per 50 ms)
        let f = 2400;
        let hz: Vec<f64> = y[f..]
            .chunks_exact(f)
            .map(|c| {
                let z: Vec<f64> = c
                    .windows(2)
                    .enumerate()
                    .filter(|(_, w)| w[0] <= 0.0 && w[1] > 0.0)
                    .map(|(i, w)| i as f64 + w[0] as f64 / (w[0] - w[1]) as f64)
                    .collect();
                (z.len() - 1) as f64 / (z[z.len() - 1] - z[0]) * FS
            })
            .collect();
        let (lo, hi) = hz
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        let lv: Vec<f64> = y[f..].chunks_exact(f).map(|c| dbfs(rms(c))).collect();
        let (llo, lhi) = lv
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        println!("wobble: {lo:.1} .. {hi:.1} Hz, level {llo:.1} .. {lhi:.1} dBFS");
        assert!(hi - lo > 10.0 && hi - lo < 60.0 && lhi - llo > 2.0);
    }

    /// the period of a voiced sound (samples): the first autocorrelation peak near the highest
    fn period(y: &[f32]) -> f64 {
        let ac = |l: usize| -> f64 {
            y.iter()
                .zip(&y[l..])
                .map(|(a, b)| *a as f64 * *b as f64)
                .sum()
        };
        let lags: Vec<f64> = (100..700).map(ac).collect();
        let max = lags.iter().cloned().fold(f64::MIN, f64::max);
        let i = (1..lags.len() - 1)
            .find(|&i| lags[i] > 0.85 * max && lags[i] >= lags[i - 1] && lags[i] >= lags[i + 1])
            .unwrap();
        // (parabolic: the peak between samples)
        let (a, b, c) = (lags[i - 1], lags[i], lags[i + 1]);
        100.0 + i as f64 + 0.5 * (a - c) / (a - 2.0 * b + c)
    }

    #[test]
    fn block_pitch() {
        // a steady vowel at 150 Hz
        let mut x = vec![0.0f32; 2 * 48000];
        let mut res = [[0.0f64; 2]; 2];
        for (i, v) in x.iter_mut().enumerate() {
            let src = if i % 320 == 0 { 1.0 } else { 0.0 };
            let mut s = 0.0;
            for (k, &(f, bw)) in [(700.0, 80.0), (1200.0, 100.0)].iter().enumerate() {
                let (r, w) = (1.0 - PI * bw / FS, 2.0 * PI * f / FS);
                let y = src + 2.0 * r * w.cos() * res[k][0] - r * r * res[k][1];
                res[k][1] = res[k][0];
                res[k][0] = y;
                s += y;
            }
            *v = (s * 0.05) as f32;
        }
        let p0 = period(&x[48000..60000]);
        for st in [5.0, -5.0, 12.0] {
            let y = run_fx(
                &Effects {
                    pitch: st,
                    ..clean()
                },
                1.0,
                &x,
                x.len(),
                B,
                1,
            );
            let p = period(&y[48000..60000]);
            let got = 12.0 * (p0 / p).log2();
            let dd = dbfs(rms(&y[24000..])) - dbfs(rms(&x[24000..]));
            println!("pitch {st:+}: period {p0:.1} -> {p:.1} samples ({got:+.2} semitones), level {dd:+.1} dB");
            assert!((got - st).abs() < 0.2, "{got}");
            assert!(dd.abs() < 1.5);
        }
    }

    #[test]
    fn block_robot() {
        let x = tone(1000.0, 0.1, 2.0);
        let y = run_fx(
            &Effects {
                robot: 50.0,
                ..clean()
            },
            1.0,
            &x,
            x.len(),
            B,
            1,
        );
        let s = spectrum(&y[24000..]);
        let (side, carrier) = (
            energy(&s, 940.0, 960.0) + energy(&s, 1040.0, 1060.0),
            energy(&s, 990.0, 1010.0),
        );
        println!(
            "robot: sidebands over the carrier {:+.0} dB",
            10.0 * (side / carrier).log10()
        );
        assert!(side > carrier * 100.0);
    }

    #[test]
    fn block_echo() {
        // a click; the echo comes back at the delay, then again, quieter
        let mut x = vec![0.0f32; 2 * 48000];
        x[4800] = 0.5;
        let y = run_fx(
            &Effects {
                echo: Some((0.25, 0.5)),
                ..clean()
            },
            1.0,
            &x,
            x.len(),
            B,
            1,
        );
        let at = |from: usize| {
            (from..from + 9600)
                .max_by(|&a, &b| y[a].abs().total_cmp(&y[b].abs()))
                .unwrap()
        };
        let (e1, e2) = (at(4800 + 7200), at(4800 + 19200));
        println!(
            "echo: repeats at {:.1} ms ({:.2}) and {:.1} ms ({:.2})",
            (e1 - 4800) as f64 / 48.0,
            y[e1],
            (e2 - 4800) as f64 / 48.0,
            y[e2]
        );
        assert!((e1 as i64 - 4800 - 12000).abs() <= 3 && (e2 as i64 - 4800 - 24000).abs() <= 6);
        assert!(y[e2].abs() < y[e1].abs() && y[e1].abs() > 0.1);
    }

    /// RT60 (s) of the reverb from its impulse response (Schroeder's backward integral, -5 .. -25 dB)
    fn rt60(amount: f64) -> f64 {
        let mut x = vec![0.0f32; 5 * 48000];
        x[480] = 0.5;
        let y = run_fx(
            &Effects {
                reverb: amount,
                ..clean()
            },
            1.0,
            &x,
            x.len(),
            B,
            1,
        );
        let mut sch: Vec<f64> = y[960..].iter().map(|&v| v as f64 * v as f64).collect();
        for i in (0..sch.len() - 1).rev() {
            sch[i] += sch[i + 1];
        }
        let t = |d: f64| {
            sch.iter()
                .position(|&v| 10.0 * (v / sch[0]).log10() < d)
                .unwrap() as f64
                / FS
        };
        (t(-25.0) - t(-5.0)) * 3.0
    }

    #[test]
    fn block_reverb() {
        for (a, want) in [(0.0001, 0.4), (0.5, 1.1), (1.0, 3.0)] {
            let rt = rt60(a);
            println!("reverb {a}: RT60 {rt:.2} s (aimed {want})");
            assert!((rt / want - 1.0).abs() < 0.35, "{rt}");
        }
        // and about as loud as dry
        let x = speech(4.0, 8);
        let dd: Vec<f64> = [0.2, 0.5, 0.8, 1.0]
            .iter()
            .map(|&a| {
                let y = run_fx(
                    &Effects {
                        reverb: a,
                        ..clean()
                    },
                    1.0,
                    &x,
                    x.len(),
                    B,
                    1,
                );
                let (ri, ro) = active(&x, &y);
                println!("reverb {a}: {:+.1} dB", dbfs(ro) - dbfs(ri));
                dbfs(ro) - dbfs(ri)
            })
            .collect();
        assert!(dd.iter().all(|d| d.abs() < 2.5));
    }

    #[test]
    fn block_hum() {
        let z = vec![0.0f32; 2 * 48000];
        let y = run_fx(
            &Effects {
                hum: 0.6,
                ..clean()
            },
            1.0,
            &z,
            z.len(),
            B,
            1,
        );
        let s = spectrum(&y[24000..]);
        let (buzz, between) = (energy(&s, 175.0, 185.0), energy(&s, 200.0, 220.0));
        let lvl = dbfs(rms(&y[24000..]));
        println!(
            "hum: {lvl:.1} dBFS, 180 Hz over 210 Hz {:+.0} dB",
            10.0 * (buzz / between).log10()
        );
        assert!(buzz > between * 100.0 && lvl > -60.0);
    }

    /// CPU per effect at 48 kHz (run with --release for the real number)
    #[test]
    fn cpu() {
        let x = speech(10.0, 9);
        let all = Effects {
            band: Some((300.0, 3000.0)),
            drive: 0.5,
            compress: 0.5,
            hiss: 0.3,
            crackle: 0.5,
            squelch: 0.8,
            horn: 0.5,
            lofi: 0.4,
            wobble: 0.5,
            pitch: 5.0,
            robot: 30.0,
            echo: Some((0.3, 0.4)),
            reverb: 0.6,
            hum: 0.3,
        };
        for (name, fx) in [
            ("radio", Effects::preset(Device::Radio)),
            ("loudspeaker", Effects::preset(Device::Loudspeaker)),
            ("pa", Effects::preset(Device::Pa)),
            ("all blocks", all),
        ] {
            let t0 = std::time::Instant::now();
            let y = run_fx(&fx, 0.5, &x, x.len(), B, 1);
            let pct = t0.elapsed().as_secs_f64() / 10.0 * 100.0;
            println!("cpu {name}: {pct:.2} % of a core");
            assert!(rms(&y) > 0.0 && pct < 5.0);
        }
    }
}
