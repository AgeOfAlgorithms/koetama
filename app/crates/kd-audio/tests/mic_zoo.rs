//! Every simulated microphone (simmic.rs) through Koetama's own capture: the voice must come out at the level a right
//! capture gives (the channel that carries it, or channels alike averaged), with its shape - never cancelled, halved
//! or lost. A microphone that came through wrongly in the field becomes an entry in sim_mics(), and this test keeps it
//! from coming back.
//!     cargo test -p kd-audio --test mic_zoo -- --nocapture      (the table)
use kd_audio::simmic::{sim_mics, test_voice};
use kd_audio::Choice;

fn level_db(x: &[f32]) -> f64 {
    let n = x.len().max(1) as f64;
    let mean = x.iter().map(|v| *v as f64).sum::<f64>() / n;
    let ms = x.iter().map(|v| (*v as f64 - mean).powi(2)).sum::<f64>() / n;
    10.0 * ms.max(1e-24).log10()
}

/// How alike the loudness over time is (20 ms levels), allowing a few ms of delay (the resampler's).
fn shape(a: &[f32], b: &[f32]) -> f64 {
    let env = |x: &[f32]| x.chunks(960).map(|c| level_db(c).max(-80.0)).collect::<Vec<f64>>();
    let (a, b) = (env(a), env(b));
    let n = a.len().min(b.len());
    let (a, b) = (&a[..n], &b[..n]);
    let (ma, mb) = (a.iter().sum::<f64>() / n as f64, b.iter().sum::<f64>() / n as f64);
    let cov: f64 = a.iter().zip(b).map(|(x, y)| (x - ma) * (y - mb)).sum();
    let va: f64 = a.iter().map(|x| (x - ma).powi(2)).sum();
    let vb: f64 = b.iter().map(|y| (y - mb).powi(2)).sum();
    cov / (va * vb).sqrt().max(1e-12)
}

#[test]
fn every_microphone_comes_through_at_its_level() {
    let voice = test_voice(4.0);
    let skip = 48000 / 4; // (the first quarter second: the channel choice settles, the resampler fills)
    let want = level_db(&voice[skip..]);
    let mut bad = Vec::new();
    println!("{:<24} {:>8} {:>8} {:>6}  {}", "microphone", "level", "want", "shape", "channels");
    for m in sim_mics() {
        let (out, choice) = m.capture(&voice, 48000).unwrap();
        assert!((out.len() as f64 - voice.len() as f64).abs() < 48000.0 * 0.05, "{}: {} samples of {}", m.name, out.len(), voice.len());
        let got = level_db(&out[skip..out.len().min(voice.len())]) - want;
        let s = shape(&voice[skip..], &out[skip..]);
        let exp = m.expect_db.map_or("-".into(), |e| format!("{e:+.1}"));
        println!("{:<24} {:>+8.1} {:>8} {:>6.2}  {:?}", m.name, got, exp, s, choice);
        match m.expect_db {
            Some(e) if (got - e as f64).abs() > 1.5 => bad.push(format!("{}: {got:+.1} dB, a right capture gives {e:+.1}", m.name)),
            None if got < -10.0 => bad.push(format!("{}: {got:+.1} dB - lost", m.name)),
            _ => {}
        }
        if s < 0.9 {
            bad.push(format!("{}: its loudness over time matches only {s:.2}", m.name));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn what_is_done_with_the_channels() {
    let voice = test_voice(1.0);
    let choice = |name: &str| kd_audio::simmic::sim_mic(name).unwrap().capture(&voice, 48000).unwrap().1;
    assert_eq!(choice("stereo-inverted"), Choice::OutOfPhase(0));
    assert_eq!(choice("stereo-silent-right"), Choice::Louder(0));
    assert_eq!(choice("stereo-silent-left"), Choice::Louder(1));
    assert_eq!(choice("quad-voice-in-one"), Choice::Louder(2));
    assert_eq!(choice("stereo-copy"), Choice::Average);
    assert_eq!(choice("array-4"), Choice::Average);
    assert!(matches!(choice("array-4-one-inverted"), Choice::OutOfPhase(_)));
}
