//! The mixer's and the low-pass's behaviour, ported from engine/test_helper.py and engine/test_app.py.
mod common;

use common::*;
use kd_audio::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// the low-pass: the same as two one-pole filters in a row, block after block (test_app.py)
#[test]
fn lowpass_matches_two_one_pole_filters() {
    let mut r = Rng::new(3);
    let x: Vec<f32> = (0..1440).map(|_| r.normal() as f32).collect();
    for fc in [400.0f64, 4000.0] {
        let a = 1.0 - (-2.0 * std::f64::consts::PI * fc / RATE as f64).exp();
        let (mut y1, mut y2) = (0.0f64, 0.0f64);
        let reference: Vec<f64> = x
            .iter()
            .map(|&s| {
                y1 = a * s as f64 + (1.0 - a) * y1;
                y2 = a * y1 + (1.0 - a) * y2;
                y2
            })
            .collect();
        let mut hist = vec![0.0f32; LP_TAPS];
        let got: Vec<f32> = x.chunks(480).flat_map(|b| lowpass(b, &mut hist, a)).collect();
        let err = got.iter().zip(&reference).map(|(g, w)| (*g as f64 - w).abs()).fold(0.0, f64::max);
        assert!(err < 1e-3, "the low-pass at {fc} Hz matches two one-pole filters (largest difference {err})");
    }
}

#[test]
fn direction() {
    let n = noise();
    let o = settled(&n, &feed());
    assert!((db(rms(&chan(&o, 0))) - db(rms(&chan(&o, 1)))).abs() < 0.1 && rms(&o) > 0.01, "ahead: both ears the same");
    let o = settled(&n, &feed_with(1.0, |s| s.az = 90.0));
    let right = db(rms(&chan(&o, 1))) - db(rms(&chan(&o, 0)));
    assert!(right > 15.0, "to the right: the right ear much louder ({right:.0} dB)");
    let o = settled(&n, &feed_with(1.0, |s| s.az = -90.0));
    assert!(db(rms(&chan(&o, 0))) - db(rms(&chan(&o, 1))) > 15.0, "to the left: mirrored");
    let o = settled(&n, &feed_with(1.0, |s| s.az = 30.0));
    let d30 = db(rms(&chan(&o, 1))) - db(rms(&chan(&o, 0)));
    assert!(3.0 < d30 && d30 < right, "a little to the right: a little louder there ({d30:.1} dB)");
    let front = settled(&n, &feed());
    let back = settled(&n, &feed_with(1.0, |s| s.az = 180.0));
    assert!(
        (db(rms(&chan(&back, 0))) - db(rms(&chan(&back, 1)))).abs() < 0.1 && db(rms(&front)) - db(rms(&back)) > 1.5,
        "behind: centred, quieter and duller than ahead"
    );
    let up = settled(&n, &feed_with(1.0, |s| {
        s.az = 90.0;
        s.el = 80.0
    }));
    assert!(db(rms(&chan(&up, 1))) - db(rms(&chan(&up, 0))) < 4.0, "almost overhead: hardly to one side");
    let p_r = settled(&n, &feed_with(1.0, |s| s.az = 90.0));
    assert!((db(rms(&front)) - db(rms(&p_r))).abs() < 0.5, "the same loudness in every direction (constant power)");
}

/// the share of the left channel's level above 3 kHz
fn high(x: &[f32]) -> f64 {
    let left: Vec<f64> = chan(x, 0).into_iter().map(|v| v as f64).collect();
    let mut planner = realfft::RealFftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(left.len());
    let mut input = left.clone();
    let mut spec = fft.make_output_vec();
    fft.process(&mut input, &mut spec).unwrap();
    let df = RATE as f64 / left.len() as f64;
    spec.iter().enumerate().filter(|(i, _)| *i as f64 * df > 3000.0).map(|(_, c)| c.norm_sqr()).sum::<f64>().sqrt()
}

#[test]
fn volume_talking_muffle() {
    let n = noise();
    let full = settled(&n, &feed());
    let half = settled(&n, &feed_with(1.0, |s| s.gain = 0.5));
    assert!((db(rms(&full)) - db(rms(&half)) - 6.02).abs() < 0.3, "half the gain: 6 dB quieter");
    let halfvol = settled(&n, &feed_with(0.5, |_| {}));
    assert!((db(rms(&full)) - db(rms(&halfvol)) - 6.02).abs() < 0.3, "half the voice volume: 6 dB quieter");
    assert!(
        rms(&settled(&n, &feed_with(1.0, |s| s.gain = 0.0))) < 1e-6
            && rms(&settled(&n, &feed_with(1.0, |s| s.talk = false))) < 1e-6,
        "gain 0 or not talking: silence"
    );
    assert!(rms(&settled(&n, &feed_with(1.0, |s| s.src = 9))) < 1e-6, "a speaker without audio: silence, no crash");
    let muf = settled(&n, &feed_with(1.0, |s| s.muffle = 1.0));
    let drop = db(high(&full)) - db(high(&muf));
    assert!(drop > 30.0, "fully muffled: above 3 kHz down {drop:.0} dB");
    let mid = settled(&n, &feed_with(1.0, |s| s.muffle = 0.5));
    assert!(
        db(high(&full)) - db(high(&mid)) > 6.0 && db(high(&mid)) - db(high(&muf)) > 6.0,
        "half muffled: in between"
    );
}

fn max_step(x: &[f32]) -> f64 {
    let l = chan(x, 0);
    l.windows(2).map(|w| (w[1] - w[0]).abs() as f64).fold(0.0, f64::max)
}

#[test]
fn a_jump_in_the_feed_glides() {
    let clock = Clock::default();
    let mut m = Mixer::with_clock(ones(), clock.boxed());
    m.set_feed(feed_with(1.0, |s| s.gain = 0.0));
    run(&mut m, &clock, 0.1, 480);
    m.set_feed(feed());
    let o = run(&mut m, &clock, 0.5, 480);
    let step = max_step(&o);
    assert!(step < 0.002 && o[o.len() - 2] > 0.05, "gain 0 -> 1 in one feed: a glide (largest step {step:.5})");
    m.set_feed(feed_with(1.0, |s| s.az = 90.0));
    let o2 = run(&mut m, &clock, 0.5, 480);
    assert!(max_step(&o2) < 0.002, "ahead -> right in one feed: a glide");
}

#[test]
fn stale_feed_fades_out_and_comes_back() {
    let clock = Clock::default();
    let mut m = Mixer::with_clock(noise(), clock.boxed());
    for _ in 0..10 {
        m.set_feed(feed());
        run(&mut m, &clock, 0.05, 480);
    }
    let loud = rms(&run(&mut m, &clock, 0.05, 480));
    let o = run(&mut m, &clock, STALE + 0.5, 480);
    let quiet = rms(&o[o.len() - 9600..]);
    assert!(loud > 0.01 && quiet < 1e-4 && !m.fresh(), "no new feed for {STALE} s: the voices fade out");
    assert!(m.feed_age() > STALE && m.feed().is_some());
    for _ in 0..10 {
        m.set_feed(feed());
        run(&mut m, &clock, 0.05, 480);
    }
    assert!(rms(&run(&mut m, &clock, 0.05, 480)) > 0.01, "... and come back with the feed");
    assert!(m.fresh() && m.level(7) > 0.1);
    m.set_feed(kd_common::feed::Feed { seq: 99, vol: 1.0, ..Default::default() });
    run(&mut m, &clock, 0.6, 480);
    assert!(
        rms(&run(&mut m, &clock, 0.05, 480)) < 1e-6 && m.voice_ids().is_empty() && m.level(7) == 0.0,
        "an empty feed (voice off): silence, the speaker forgotten"
    );
}

#[test]
fn no_feed_yet_is_silence() {
    let m = Mixer::new(noise());
    assert!(!m.fresh() && m.feed().is_none() && m.feed_age() > 1e8);
    let mut m = m;
    let o = m.render(480);
    assert!(o.iter().all(|f| f[0] == 0.0 && f[1] == 0.0));
    assert!(m.render(0).is_empty());
}

#[test]
fn a_quiet_voice_keeps_its_place_and_the_limiter() {
    let saw: Vec<f32> = (0..RATE).map(|i| i as f32 / RATE as f32 * 0.1).collect();
    let clock = Clock::default();
    let mut m = Mixer::with_clock(clips(saw), clock.boxed());
    m.set_feed(feed());
    run(&mut m, &clock, 0.3, 480);
    m.set_feed(feed_with(1.0, |s| s.talk = false));
    run(&mut m, &clock, 1.0, 480);
    let pos = m.voice_pos(7).unwrap();
    m.set_feed(feed_with(1.0, |s| s.talk = false));
    run(&mut m, &clock, 0.5, 480);
    assert!(
        m.voice_pos(7) == Some(pos) && 0.3 * RATE as f64 <= pos as f64 && (pos as f64) < 0.9 * RATE as f64,
        "a voice that stops talking keeps its place in its recording ({pos})"
    );
    let loud = clips(vec![5.0; RATE as usize]);
    let o = settled(&loud, &feed());
    assert!(o.iter().all(|v| v.abs() <= 1.0), "too loud: limited to full scale, never beyond");
}

#[test]
fn talking_again_starts_the_clip_over() {
    let clock = Clock::default();
    let mut m = Mixer::with_clock(noise(), clock.boxed());
    m.set_feed(feed());
    run(&mut m, &clock, 0.3, 480);
    let p1 = m.voice_pos(7).unwrap();
    m.set_feed(feed_with(1.0, |s| s.talk = false));
    run(&mut m, &clock, 0.4, 480);
    m.set_feed(feed());
    run(&mut m, &clock, 0.1, 480);
    let want = |s: f64| (s * RATE as f64 / 480.0) as usize * 480;
    assert!(
        p1 == want(0.3) && m.voice_pos(7) == Some(want(0.1)),
        "a voice dummy talking again: its clip starts over (at {:?} samples after 0.1 s)",
        m.voice_pos(7)
    );
}

#[test]
fn a_voice_that_left_the_feed_fades_on_its_last_clip() {
    let clock = Clock::default();
    let mut m = Mixer::with_clock(ones(), clock.boxed());
    for _ in 0..10 {
        m.set_feed(feed());
        run(&mut m, &clock, 0.05, 480);
    }
    m.set_feed(kd_common::feed::Feed { vol: 1.0, ..Default::default() });
    let o = run(&mut m, &clock, 0.01, 480);
    assert!(rms(&o) > 0.05, "the first block after it left: still sounding (fading)");
    assert_eq!(m.voice_ids(), vec![7]);
    run(&mut m, &clock, 1.0, 480);
    assert!(m.voice_ids().is_empty(), "faded out: forgotten");
}

#[test]
fn render_and_render_into_agree() {
    let (c1, c2) = (Clock::default(), Clock::default());
    let mut a = Mixer::with_clock(noise(), c1.boxed());
    let mut b = Mixer::with_clock(noise(), c2.boxed());
    a.set_feed(feed_with(0.7, |s| s.az = 40.0));
    b.set_feed(feed_with(0.7, |s| s.az = 40.0));
    for frames in [480usize, 512, 100, 1024] {
        let x = a.render(frames);
        let mut y = vec![0.0f32; frames * 2];
        b.render_into(&mut y);
        assert!(x.iter().flatten().zip(&y).all(|(p, q)| p == q));
    }
}

#[test]
fn the_sink_feeds_the_shared_mixer() {
    use kd_common::feed::FeedSink;
    let clock = Clock::default();
    let shared: SharedMixer = Arc::new(Mutex::new(Mixer::with_clock(HashMap::new(), clock.boxed())));
    let sink: Arc<dyn FeedSink> = Arc::new(MixerSink(shared.clone()));
    assert!(!sink.fresh());
    sink.set_feed(feed());
    assert!(sink.fresh() && lock(&shared).feed().is_some_and(|f| f.seq == 1));
    clock.add(STALE + 0.1);
    assert!(!sink.fresh());
}

fn assert_send<T: Send>() {}
fn assert_sync<T: Sync>() {}

#[test]
fn thread_safety() {
    assert_send::<Mixer>();
    assert_send::<SharedMixer>();
    assert_sync::<SharedMixer>();
    assert_send::<MixerSink>();
    assert_send::<Output>();
    assert_send::<Input>();
}

/// How long a 10 ms block takes with 8 muffled voices (printed; the audio callback has 10 ms in all).
#[test]
#[ignore]
fn render_speed() {
    let clock = Clock::default();
    let mut m = Mixer::with_clock(noise(), clock.boxed());
    let mut f = feed_with(1.0, |s| s.muffle = 1.0);
    let s = f.speakers[&7].clone();
    for id in 0..8 {
        f.speakers.insert(id, s.clone());
    }
    m.set_feed(f);
    let mut out = vec![0.0f32; 960];
    let t = std::time::Instant::now();
    for _ in 0..200 {
        m.render_into(&mut out);
    }
    let us = t.elapsed().as_secs_f64() * 1e6 / 200.0;
    println!("8 muffled voices: {us:.0} us per 480-frame block");
}

/// A stand-in for kd-voice's receiver: each player id's voice is 0.1; who was asked is noted.
struct Voices {
    playing: Vec<i64>,
    asked: Arc<Mutex<Vec<i64>>>,
}

impl Streams for Voices {
    fn pull(&mut self, id: i64, out: &mut [f32]) -> bool {
        self.asked.lock().unwrap().push(id);
        let on = self.playing.contains(&id);
        out.fill(if on { 0.1 } else { 0.0 });
        on
    }
}

/// Real players (src 0): their voices stream in (talk ignored), placed like the test voices; gain 0 or a player not
/// in the feed: not played (never even asked for); the test voices go on as before.
#[test]
fn real_players_stream_in() {
    let clock = Clock::default();
    let asked: Arc<Mutex<Vec<i64>>> = Arc::default();
    let mut m = Mixer::with_clock(ones(), clock.boxed());
    m.streams = Some(Box::new(Voices { playing: vec![7, 8, 9], asked: asked.clone() }));
    // player 7 to the right, talk 0 (ignored for a real player)
    m.set_feed(feed_with(1.0, |s| {
        s.src = 0;
        s.talk = false;
        s.az = 90.0
    }));
    let o = run(&mut m, &clock, 0.5, 480);
    let tail = &o[o.len() / 2..];
    assert!(rms(&chan(tail, 1)) > 0.05 && db(rms(&chan(tail, 1))) - db(rms(&chan(tail, 0))) > 15.0, "heard, to the right");
    assert!(asked.lock().unwrap().iter().all(|&id| id == 7), "only the player in the feed is asked for");
    // gain 0: not played, not asked
    asked.lock().unwrap().clear();
    m.set_feed(feed_with(1.0, |s| {
        s.src = 0;
        s.gain = 0.0
    }));
    let o = run(&mut m, &clock, 0.3, 480);
    assert!(rms(&o[o.len() / 2..]) < 1e-4 && asked.lock().unwrap().is_empty());
    // a real player with nothing arriving: silence; a test voice beside it plays its clip
    let mut f = feed_with(1.0, |s| s.src = 0);
    f.speakers.get_mut(&7).unwrap().src = 0;
    f.speakers.insert(5, kd_common::feed::Speaker { src: 1, talk: true, gain: 1.0, ..Default::default() });
    let mut quiet = Mixer::with_clock(ones(), clock.boxed());
    quiet.streams = Some(Box::new(Voices { playing: vec![], asked: Arc::default() }));
    quiet.set_feed(f);
    let o = run(&mut quiet, &clock, 0.3, 480);
    assert!(quiet.level(7) == 0.0 && quiet.level(5) > 0.5, "{} {}", quiet.level(7), quiet.level(5));
    assert!(rms(&o[o.len() / 2..]) > 0.05, "the test voice plays");
}
