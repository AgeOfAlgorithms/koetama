//! Every kind of microphone through the voice chat, as two players on one PC: the simulated microphone (kd-audio's
//! simmic: Koetama's own capture), A's sender (the gate, the automatic gain, Opus), a stand-in relay, B's decoder and
//! jitter buffer. What B hears from each microphone is compared with what it hears from the reference one: the
//! automatic gain evens levels out, so every one must come through within 3 dB of it. (2026-10-15: a headset whose second channel was inverted came through ~30 dB too quiet -
//! this is the test that would have caught it, with mic_zoo.)
//!     cargo test -p kd-voice --test voice_zoo -- --nocapture      (the table)
#[path = "common/standin.rs"]
mod standin;

use kd_audio::simmic::{sim_mic, test_voice, SimMic};
use kd_audio::Streams;
use kd_common::feed::{Feed, Speaker};
use kd_voice::{crypto, Voice};
use std::time::{Duration, Instant};

/// A player in the room as `me`, sending to `to` (push to talk held), hearing player 1 (src 0).
fn feed(room: &str, key: &str, me: i64, to: &[i64]) -> Feed {
    let mut f = Feed { vol: 1.0, mic: true, ptt: Some(true), room: room.into(), key: key.into(), me, to: to.to_vec(), ..Default::default() };
    f.speakers.insert(1, Speaker { src: 0, gain: 1.0, ..Default::default() });
    f
}

/// The speech level of what was heard (dBFS): the power of the 20 ms windows with sound in them.
fn speech_db(x: &[f32]) -> f64 {
    let w: Vec<f64> = x.chunks(960).map(|c| c.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / c.len() as f64).collect();
    let top = w.iter().copied().fold(0.0, f64::max);
    let on: Vec<f64> = w.into_iter().filter(|p| *p > top * 1e-4).collect();
    10.0 * (on.iter().sum::<f64>() / on.len().max(1) as f64).max(1e-24).log10()
}

/// What B hears when A says `voice` into `mic`, in real time (50 ms blocks).
fn heard(relay: &str, mic: &SimMic, voice: &[f32]) -> Vec<f32> {
    let (captured, _) = mic.capture(voice, kd_voice::RATE).unwrap();
    let text = crypto::new_room().unwrap();
    let (room, key) = text.split_once(':').unwrap();
    let log = kd_common::null_log();
    let (a, b) = (Voice::start(relay.into(), log.clone()), Voice::start(relay.into(), log));
    let feeds = [feed(room, key, 1, &[2]), feed(room, key, 2, &[])];
    let t0 = Instant::now();
    while !(a.status().state == "connected" && b.status().state == "connected") {
        a.set_feed(&feeds[0]);
        b.set_feed(&feeds[1]);
        assert!(t0.elapsed() < Duration::from_secs(20), "not connected: {:?} {:?}", a.status(), b.status());
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut pb = b.playback();
    let block = 2400;
    let mut out = vec![0.0f32; block];
    let mut got = Vec::new();
    let t0 = Instant::now();
    let total = captured.len() / block + 20; // (+1 s: the last packets arrive, the voice plays out)
    for i in 0..total {
        b.set_feed(&feeds[1]);
        if (i + 1) * block <= captured.len() {
            a.set_feed(&feeds[0]);
            a.push_mic(&captured[i * block..(i + 1) * block], false);
        } else {
            let mut fa = feeds[0].clone();
            fa.ptt = Some(false);
            a.set_feed(&fa);
            a.push_mic(&vec![0.0; block], false);
        }
        pb.pull(1, &mut out);
        got.extend_from_slice(&out);
        let due = t0 + Duration::from_millis(50 * (i as u64 + 1));
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
    }
    got
}

#[test]
fn every_kind_of_microphone_through_the_voice_chat() {
    let relay = standin::mock_relay();
    let voice = test_voice(2.0);
    let reference = speech_db(&heard(&relay.url, &sim_mic("mono").unwrap(), &voice));
    assert!(reference > -40.0, "the reference microphone was not heard: {reference:.1} dBFS");
    println!("reference (mono): {reference:.1} dBFS heard");
    let mut bad = Vec::new();
    // (one of each kind of capture; kd-audio's mic_zoo runs all of them through the capture alone)
    for name in [
        "stereo-inverted",
        "stereo-inverted-uneven",
        "stereo-silent-right",
        "array-4-one-inverted",
        "quad-voice-in-one",
        "quiet",
        "hot",
        "noisy",
        "16k-i16",
        "44k1-i16",
    ] {
        let m = sim_mic(name).unwrap();
        let got = speech_db(&heard(&relay.url, &m, &voice)) - reference;
        // (the automatic gain evens out a quiet microphone and a clipping one too: before 2026-10-15 they came through
        //  10.5 dB under and 9.3 dB over)
        let (lo, hi) = (-3.0, 3.0);
        println!("{name:<24} {got:+6.1} dB against the reference (want {lo:+.0}..{hi:+.0})");
        if !(lo..=hi).contains(&got) {
            bad.push(format!("{name} ({}): {got:+.1} dB against the reference microphone", m.about));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
