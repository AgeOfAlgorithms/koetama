//! Devices in the mixer (PROTOCOL.md "Devices"): a voice heard through a device's places, each placed, delayed and
//! faded like a voice, its effects still sounding after the voice stops.
mod common;

use common::*;
use kd_audio::*;
use kd_common::feed::{Device, Effects, Out, Via};

fn plain(outs: Vec<Out>) -> Via {
    Via { device: Device::Plain, outs, muffle: 0.0, signal: 1.0, effects: Effects::default() }
}

fn out(az: f64, gain: f64, delay: f64) -> Out {
    Out { az, el: 0.0, gain, delay }
}

fn rms(x: impl Iterator<Item = f32>) -> f64 {
    let (mut s, mut n) = (0.0f64, 0usize);
    for v in x {
        s += (v as f64) * (v as f64);
        n += 1;
    }
    (s / n.max(1) as f64).sqrt()
}

/// A device with no effects sounds exactly as the voice would there: through it or directly, the same samples.
#[test]
fn a_plain_device_is_the_voice_placed_there() {
    let render = |feed| {
        let clock = Clock::default();
        let mut m = Mixer::with_clock(noise(), clock.boxed());
        m.set_feed(feed);
        run(&mut m, &clock, 0.5, 480)
    };
    let direct = render(feed_with(1.0, |s| {
        s.gain = 0.7;
        s.az = 30.0;
    }));
    let device = render(feed_with(1.0, |s| {
        s.gain = 0.0;
        s.via = vec![plain(vec![out(30.0, 0.7, 0.0)])];
    }));
    let worst = direct.iter().zip(&device).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(worst < 1e-5, "differs by {worst}");
    assert!(rms(device.iter().copied()) > 0.03);
}

/// Heard only through a device to the right: the sound is on the right; with the device gone it fades away.
#[test]
fn heard_only_through_a_device() {
    let clock = Clock::default();
    let mut m = Mixer::with_clock(noise(), clock.boxed());
    m.set_feed(feed_with(1.0, |s| {
        s.gain = 0.0;
        s.via = vec![plain(vec![out(90.0, 1.0, 0.0)])];
    }));
    let o = run(&mut m, &clock, 0.5, 480);
    let tail = &o[o.len() / 2..];
    let (l, r) = (rms(tail.iter().step_by(2).copied()), rms(tail.iter().skip(1).step_by(2).copied()));
    assert!(r > 4.0 * l && r > 0.03, "left {l} right {r}");
    m.set_feed(feed_with(1.0, |s| s.gain = 0.0));
    let o = run(&mut m, &clock, 0.5, 480);
    assert!(rms(o[o.len() / 2..].iter().copied()) < 1e-4, "gone");
}

/// A PA's farther speaker comes later by its delay: a click through two places 0.1 s apart is heard twice.
#[test]
fn farther_places_come_later() {
    let mut click = vec![0.0f32; RATE as usize];
    click[0] = 0.8;
    let clock = Clock::default();
    let mut m = Mixer::with_clock(clips(click), clock.boxed());
    m.set_feed(feed_with(1.0, |s| {
        s.gain = 0.0;
        s.via = vec![plain(vec![out(0.0, 1.0, 0.0), out(0.0, 0.5, 0.1)])];
    }));
    let o = run(&mut m, &clock, 2.0, 480);
    // (the second second: gains settled; the clip loops, its click at the start of each second)
    let mono: Vec<f32> = o.chunks(2).map(|f| f[0] + f[1]).skip(RATE as usize).take(RATE as usize).collect();
    let peak = |from: usize, to: usize| (from..to).max_by(|&a, &b| mono[a].abs().total_cmp(&mono[b].abs())).unwrap();
    let first = peak(0, 2400);
    let second = peak(2400, 9600);
    assert!(first < 50, "the near place at once: {first}");
    assert!((second as i64 - 4800).abs() < 50, "the far one 0.1 s later: {second}");
    assert!(mono[second].abs() < mono[first].abs() && mono[second].abs() > 0.3 * mono[first].abs());
}

/// A walkie-talkie keeps sounding after the voice stops (its squelch tail), then goes quiet.
#[test]
fn a_radio_has_a_tail() {
    let clock = Clock::default();
    let mut m = Mixer::with_clock(noise(), clock.boxed());
    let radio = Via { device: Device::Radio, effects: Effects::preset(Device::Radio), ..plain(vec![out(0.0, 1.0, 0.0)]) };
    m.set_feed(feed_with(1.0, |s| {
        s.gain = 0.0;
        s.via = vec![radio.clone()];
    }));
    run(&mut m, &clock, 0.5, 480);
    m.set_feed(feed_with(1.0, |s| {
        s.gain = 0.0;
        s.talk = false;
        s.via = vec![radio.clone()];
    }));
    let o = run(&mut m, &clock, 1.5, 480);
    let at = |a: f64, b: f64| rms(o[(a * RATE as f64) as usize * 2..(b * RATE as f64) as usize * 2].iter().copied());
    assert!(at(0.03, 0.12) > 1e-3, "the squelch tail: {}", at(0.03, 0.12));
    assert!(at(1.0, 1.5) < 1e-4, "then quiet: {}", at(1.0, 1.5));
}

/// The direct voice's own effects (a speaker's `effects`): a band-limited voice loses its highs.
#[test]
fn direct_voice_effects() {
    let render = |fx: Effects| {
        let clock = Clock::default();
        let mut m = Mixer::with_clock(noise(), clock.boxed());
        m.set_feed(feed_with(1.0, |s| s.effects = fx));
        run(&mut m, &clock, 0.5, 480)
    };
    let clean = render(Effects::default());
    let narrow = render(Effects { band: Some((300.0, 3000.0)), ..Effects::default() });
    // (the highs: a sample minus the one before)
    let highs = |x: &[f32]| rms(x.chunks(2).collect::<Vec<_>>().windows(2).map(|w| w[1][0] - w[0][0]));
    assert!(highs(&narrow) < 0.5 * highs(&clean), "{} vs {}", highs(&narrow), highs(&clean));
}

/// A player's own volume for a speaker (`volume`): the voice and its devices, scaled; 0: not played at all.
#[test]
fn a_speakers_own_volume() {
    let render = |vol: Option<f64>, via: bool| {
        let clock = Clock::default();
        let mut m = Mixer::with_clock(noise(), clock.boxed());
        m.set_feed(feed_with(1.0, |s| {
            s.volume = vol;
            s.gain = 0.4;
            if via {
                s.via = vec![plain(vec![out(0.0, 0.4, 0.0)])];
            }
        }));
        let o = run(&mut m, &clock, 0.5, 480);
        rms(o[o.len() / 2..].iter().copied())
    };
    for via in [false, true] {
        let (full, half) = (render(None, via), render(Some(0.5), via));
        assert!((half / full - 0.5).abs() < 0.02, "half: {half} of {full}");
        assert!(render(Some(0.0), via) < 1e-6, "0: silent");
    }
}
