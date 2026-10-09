//! Voices end to end through a relay: a stand-in relay here (tungstenite, the rules of relay/src/frames.js: each
//! packet to the players it names only, never back to the sender), and the live one (#[ignore]d: it needs the
//! internet). Four Koetamas in one room: A talks (push to talk held) to B and D; B hears A's voice back (decoded,
//! its level and pitch kept); C is not in A's `to` and gets nothing; D has the wrong key and plays nothing.
//! Then presence (who is in the room, their ids and ranges), talking events, and a player id taken in the room (the
//! relay replaces a connection with the same id: close 4000, or refuses it: 409).
//!     cargo test -p kd-voice --test relay -- --include-ignored     (the live relay too)
use kd_audio::Streams;
use kd_common::feed::{relay_id, Feed, PlayerId, Speaker};
use kd_voice::{crypto, frames, Voice, VoiceEvent};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::CloseFrame;
use tungstenite::Message;

/// What a player's connection is told by the others' threads.
enum Out {
    Frame(Vec<u8>),
    /// another connection with the same id came: closed with 4000 (relay/src/index.js)
    Replaced,
}

type Rooms = Arc<Mutex<HashMap<String, HashMap<u16, Sender<Out>>>>>;
type Counts = Arc<Mutex<HashMap<u16, u64>>>;

/// A presence packet's payload from a player without an id: nonce, the 16-byte plaintext, tag (a voice packet's is
/// longer: it has frames)
const BARE_PRESENCE: usize = 12 + 16 + 16;

/// A stand-in relay on 127.0.0.1.
#[derive(Clone)]
struct Mock {
    /// its ws:// address
    url: String,
    /// voice frames delivered to each player id (not presence from players without ids: BARE_PRESENCE)
    delivered: Counts,
    /// connections made / tried, per player id
    opened: Counts,
    tried: Counts,
    /// player ids it refuses (409, as a relay that keeps the first connection with an id would)
    refuse: Arc<Mutex<Vec<u16>>>,
}

fn count(c: &Counts, id: u16) -> u64 {
    c.lock().unwrap().get(&id).copied().unwrap_or(0)
}

fn mock_relay() -> Mock {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let m = Mock {
        url: format!("ws://{}", l.local_addr().unwrap()),
        delivered: Arc::default(),
        opened: Arc::default(),
        tried: Arc::default(),
        refuse: Arc::default(),
    };
    let rooms: Rooms = Arc::default();
    let m2 = m.clone();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let (rooms, m) = (rooms.clone(), m2.clone());
            std::thread::spawn(move || serve(s, rooms, m));
        }
    });
    m
}

/// /v1/room/<32 hex>?me=<id> -> (room, id)
fn room_and_id(path: &str) -> Option<(String, u16)> {
    let (room, me) = path.strip_prefix("/v1/room/")?.split_once("?me=")?;
    let ok = room.len() == 32 && room.bytes().all(|c| c.is_ascii_hexdigit());
    ok.then(|| Some((room.to_string(), frames::parse_id(me)?)))?
}

#[allow(clippy::result_large_err)] // (tungstenite's handshake callback type)
fn serve(s: TcpStream, rooms: Rooms, m: Mock) {
    let mut path = String::new();
    let Ok(mut ws) = tungstenite::accept_hdr(s, |req: &Request, resp: Response| {
        path = req.uri().to_string();
        if let Some((_, me)) = room_and_id(&path) {
            *m.tried.lock().unwrap().entry(me).or_default() += 1;
            if m.refuse.lock().unwrap().contains(&me) {
                let no: ErrorResponse = tungstenite::http::Response::builder().status(409).body(Some("taken".into())).unwrap();
                return Err(no);
            }
        }
        Ok(resp)
    }) else {
        return;
    };
    let Some((room, me)) = room_and_id(&path) else { return };
    *m.opened.lock().unwrap().entry(me).or_default() += 1;
    let (tx, rx) = channel::<Out>();
    // (the same player again: the new connection replaces the old one)
    if let Some(old) = rooms.lock().unwrap().entry(room.clone()).or_default().insert(me, tx) {
        let _ = old.send(Out::Replaced);
    }
    ws.get_ref().set_read_timeout(Some(Duration::from_millis(2))).unwrap();
    // (no Nagle: small voice packets go at once, as a real relay sends them - with it, the delayed-ACK stall held a
    //  packet ~200 ms and the listener concealed the gap)
    ws.get_ref().set_nodelay(true).unwrap();
    loop {
        match ws.read() {
            Ok(Message::Binary(b)) => {
                if let Some((to, out)) = frames::route(&b, me) {
                    let r = rooms.lock().unwrap();
                    for id in to {
                        if let Some(t) = r.get(&room).and_then(|m| m.get(&id)) {
                            let _ = t.send(Out::Frame(out.clone()));
                        }
                    }
                }
            }
            Ok(Message::Text(t)) if t.as_str() == "ping" => {
                let _ = ws.send(Message::text("pong"));
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
        while let Ok(out) = rx.try_recv() {
            let out = match out {
                Out::Frame(f) => f,
                Out::Replaced => {
                    let _ = ws.close(Some(CloseFrame { code: CloseCode::Library(4000), reason: "replaced".into() }));
                    // (until the client answers: it reads the code)
                    let t0 = Instant::now();
                    while ws.read().is_ok() || t0.elapsed() < Duration::from_millis(500) && ws.can_read() {}
                    return;
                }
            };
            if out.len() > 3 + BARE_PRESENCE {
                *m.delivered.lock().unwrap().entry(me).or_default() += 1;
            }
            if ws.send(Message::binary(out)).is_err() {
                return;
            }
        }
    }
}

/// A player's feed: in the room as `me`, sending to `to` (push to talk held), hearing player 1 (src 0).
fn feed(room: &str, key: &str, me: i64, to: &[i64]) -> Feed {
    let mut f = Feed { vol: 1.0, mic: true, ptt: Some(true), room: room.into(), key: key.into(), me, to: to.to_vec(), ..Default::default() };
    f.speakers.insert(1, Speaker { src: 0, gain: 1.0, ..Default::default() });
    f
}

/// A player's feed with game ids: in the room as `me`, sending to `to` (push to talk held), hearing `hears` (gain 1)
/// and knowing `knows` (gain 0: in the feed, not heard).
fn feed_ids(room: &str, key: &str, me: &PlayerId, to: &[&PlayerId], hears: &[&PlayerId], knows: &[&PlayerId]) -> Feed {
    let rid = |id: &PlayerId| relay_id(room, id);
    let mut f = Feed {
        vol: 1.0,
        mic: true,
        ptt: Some(true),
        room: room.into(),
        key: key.into(),
        me: rid(me),
        me_id: Some(me.clone()),
        to: to.iter().map(|id| rid(id)).collect(),
        ..Default::default()
    };
    for (ids, gain) in [(hears, 1.0), (knows, 0.0)] {
        for id in ids {
            f.speakers.insert(rid(id), Speaker { src: 0, gain, id: (*id).clone(), ..Default::default() });
        }
    }
    f
}

/// A tone (2 s at 48 kHz).
fn tone() -> Vec<f32> {
    (0..kd_voice::RATE as usize * 2).map(|i| (0.3 * (2.0 * std::f64::consts::PI * 300.0 * i as f64 / 48000.0).sin()) as f32).collect()
}

/// An English line from the benchmark's clips (export/, not in git: None without it), 48 kHz, at most 3 s.
fn speech() -> Option<Vec<f32>> {
    let clip = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../export/asrbench/clips/en01_clean.wav");
    let (x, rate) = kd_audio::read_wav(clip).ok()?;
    let x = kd_audio::resample(&x, rate, kd_voice::RATE);
    Some(x[..x.len().min(kd_voice::RATE as usize * 3)].to_vec())
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

/// zero crossings a second: a tone's pitch
fn crossings(x: &[f32]) -> f64 {
    x.windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count() as f64 / (x.len() as f64 / 48000.0)
}

/// How alike two voices' loudness over time is: the best correlation of their 20 ms levels, got shifted 0..10
/// windows later (it may run a little behind: concealed gaps).
fn envelope_match(want: &[f32], got: &[f32]) -> f64 {
    let env = |x: &[f32]| x.chunks(960).map(rms).collect::<Vec<f64>>();
    let (a, b) = (env(want), env(got));
    let corr = |a: &[f64], b: &[f64]| {
        let n = a.len().min(b.len());
        let (a, b) = (&a[..n], &b[..n]);
        let (ma, mb) = (a.iter().sum::<f64>() / n as f64, b.iter().sum::<f64>() / n as f64);
        let cov: f64 = a.iter().zip(b).map(|(x, y)| (x - ma) * (y - mb)).sum();
        let va: f64 = a.iter().map(|x| (x - ma).powi(2)).sum();
        let vb: f64 = b.iter().map(|y| (y - mb).powi(2)).sum();
        cov / (va * vb).sqrt().max(1e-12)
    };
    (0..=10).filter(|&lag| lag < b.len()).map(|lag| corr(&a, &b[lag..])).fold(f64::MIN, f64::max)
}

/// Feeds the players (as a game does, so they stay in their rooms) until done() holds; at most secs.
fn feed_until(vs: &[&Voice], feeds: &[Feed], secs: f64, done: impl Fn() -> bool) {
    let t0 = Instant::now();
    loop {
        for (v, f) in vs.iter().zip(feeds) {
            v.set_feed(f);
        }
        if done() {
            return;
        }
        assert!(t0.elapsed().as_secs_f64() < secs, "not yet: {:?}", vs.iter().map(|v| v.status()).collect::<Vec<_>>());
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_connected(vs: &[&Voice], feeds: &[Feed], secs: f64) {
    feed_until(vs, feeds, secs, || vs.iter().all(|v| v.status().state == "connected"));
}

/// The four players through `relay`; delivered: the stand-in relay's count per player (None: the live relay).
/// x: what A says (48 kHz); is_speech: compared by its loudness over time (else as a tone, by its pitch).
fn four_players(relay: &str, delivered: Option<Counts>, x: &[f32], is_speech: bool) {
    let text = crypto::new_room().unwrap();
    let (room, key) = text.split_once(':').unwrap();
    let wrong = crypto::new_room().unwrap().split_once(':').unwrap().1.to_string();
    let log = kd_common::null_log();
    let (a, b, c, d) = (
        Voice::start(relay.into(), log.clone()),
        Voice::start(relay.into(), log.clone()),
        Voice::start(relay.into(), log.clone()),
        Voice::start(relay.into(), log.clone()),
    );
    let feeds = [feed(room, key, 1, &[2, 4]), feed(room, key, 2, &[]), feed(room, key, 3, &[]), feed(room, &wrong, 4, &[])];
    wait_connected(&[&a, &b, &c, &d], &feeds, 20.0);
    let (mut pa, mut pb, mut pc, mut pd) = (a.playback(), b.playback(), c.playback(), d.playback());
    // (real time: a 50 ms block from A's microphone, then 50 ms of each one's audio output)
    let block = 2400;
    let mut heard = Vec::new();
    let mut others = 0.0f64;
    let mut out = vec![0.0f32; block];
    let t0 = Instant::now();
    let total = x.len() / block + 30; // (+1.5 s: the last packets arrive, the voice plays out)
    for i in 0..total {
        for (v, f) in [&a, &b, &c, &d].iter().zip(&feeds) {
            v.set_feed(f);
        }
        if (i + 1) * block <= x.len() {
            a.push_mic(&x[i * block..(i + 1) * block], false);
        } else if (i + 1) * block <= x.len() + block * 6 {
            // (the key let go: silence through the tail - 300 ms, past PTT_TAIL, so the last packet goes out flagged)
            let mut fa = feeds[0].clone();
            fa.ptt = Some(false);
            a.set_feed(&fa);
            a.push_mic(&vec![0.0; block], false);
        }
        pb.pull(1, &mut out);
        heard.extend_from_slice(&out);
        pc.pull(1, &mut out);
        others = others.max(rms(&out));
        pd.pull(1, &mut out);
        others = others.max(rms(&out));
        for id in [2, 4] {
            pa.pull(id, &mut out);
            others = others.max(rms(&out));
        }
        let due = t0 + Duration::from_millis(50 * (i as u64 + 1));
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
    }
    // B heard A: about as loud, alike, delayed by the trip and the jitter buffer
    let start = heard.iter().position(|s| s.abs() > 1e-4).expect("B heard nothing");
    let got = &heard[start..(start + x.len()).min(heard.len())];
    let first = x.iter().position(|s| s.abs() > 1e-4).unwrap_or(0);
    let want = &x[first..];
    let (rw, rg) = (rms(want), rms(got));
    assert!(got.len() as f64 > want.len() as f64 * 0.9, "B heard {} of {} samples", got.len(), want.len());
    assert!((rg / rw - 1.0).abs() < 0.35, "level {rw:.4} -> {rg:.4}");
    if is_speech {
        let m = envelope_match(want, got);
        assert!(m > 0.8, "the loudness over time matches only {m:.2}");
    } else {
        let (cw, cg) = (crossings(want), crossings(got));
        // (20 %: a packet late under load - the whole suite runs at once - is concealed for a frame, which moves the
        //  count a little; a wrong rate would be off by 3x)
        assert!((cg / cw - 1.0).abs() < 0.2, "pitch (zero crossings/s) {cw:.0} -> {cg:.0}");
    }
    // C (not in range), D (wrong key) and A (the sender) heard nothing
    assert_eq!(others, 0.0, "only B plays A's voice");
    assert_eq!(b.status().heard, 1);
    assert_eq!((c.status().heard, d.status().heard, a.status().heard), (0, 0, 0));
    if let Some(dl) = delivered {
        // (voice frames: the presence every player sends the ones in its feed is not counted)
        let dl = dl.lock().unwrap();
        let n = |id| dl.get(&id).copied().unwrap_or(0);
        assert!(n(2) > 10 && n(4) == n(2), "B and D each got A's packets: {dl:?}");
        assert_eq!((n(1), n(3)), (0, 0), "nothing back to A, nothing to C");
    }
    for v in [a, b, c, d] {
        v.stop();
    }
}

#[test]
fn voices_through_a_stand_in_relay() {
    let m = mock_relay();
    four_players(&m.url, Some(m.delivered), &tone(), false);
}

/// The same with a spoken line (skipped without export/ from the benchmark).
#[test]
fn speech_through_a_stand_in_relay() {
    let Some(x) = speech() else {
        eprintln!("no export/asrbench/clips: skipped");
        return;
    };
    let m = mock_relay();
    four_players(&m.url, Some(m.delivered), &x, true);
}

/// Reconnects: the relay gone and back; a room change; no room - off.
#[test]
fn connects_and_leaves_as_the_feed_says() {
    let url = mock_relay().url;
    let v = Voice::start(url.clone(), kd_common::null_log());
    assert_eq!(v.status().state, "off");
    let text = crypto::new_room().unwrap();
    let (room, key) = text.split_once(':').unwrap();
    wait_connected(&[&v], &[feed(room, key, 5, &[])], 10.0);
    // no room: off
    v.set_feed(&Feed::default());
    let t0 = Instant::now();
    while v.status().state != "off" {
        assert!(t0.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
    }
    // a relay that is not there: connecting (and trying again)
    let dead = { TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap() };
    let w = Voice::start(format!("ws://{dead}"), kd_common::null_log());
    w.set_feed(&feed(room, key, 6, &[]));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(w.status().state, "connecting");
    // the game stops feeding: off after kd_audio::STALE
    let t0 = Instant::now();
    while w.status().state != "off" {
        assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", w.status());
        std::thread::sleep(Duration::from_millis(20));
    }
    v.stop();
    w.stop();
}

/// A fresh room and its key.
fn new_room() -> (String, String) {
    let text = crypto::new_room().unwrap();
    let (room, key) = text.split_once(':').unwrap();
    (room.into(), key.into())
}

fn said(id: &PlayerId, talking: bool) -> VoiceEvent {
    VoiceEvent::Talking { id: id.clone(), talking }
}

/// Presence: who is in the room (heard from in the last 15 s) by their game ids, and the ranges they announce. A
/// knows B (in its feed, gain 0), B hears A, C sends to A only: A sees B and C, B sees A, C sees nobody.
#[test]
fn presence_tells_who_is_in_the_room() {
    let m = mock_relay();
    let (room, key) = new_room();
    let (ana, bo, cy) = (PlayerId::string("ana"), PlayerId::number(76561198000000002), PlayerId::string("cy"));
    let rid = |id: &PlayerId| relay_id(&room, id);
    assert!(rid(&ana) != rid(&bo) && rid(&bo) != rid(&cy) && rid(&ana) != rid(&cy), "(numbers clash: 1 in 20000)");
    let mut fa = feed_ids(&room, &key, &ana, &[], &[], &[&bo]);
    fa.range = Some((8.0, 25.0));
    let feeds = [fa, feed_ids(&room, &key, &bo, &[], &[&ana], &[]), feed_ids(&room, &key, &cy, &[&ana], &[], &[])];
    let log = kd_common::null_log();
    let (a, b, c) = (Voice::start(m.url.clone(), log.clone()), Voice::start(m.url.clone(), log.clone()), Voice::start(m.url.clone(), log));
    // (presence goes out on connecting, then every 5 s: one that went before the other was in comes again)
    feed_until(&[&a, &b, &c], &feeds, 15.0, || a.players() == vec![bo.clone(), cy.clone()] && b.players() == vec![ana.clone()]);
    assert_eq!(b.range_of(rid(&ana)), Some((8.0, 25.0)));
    assert_eq!((a.range_of(rid(&bo)), a.range_of(rid(&cy))), (None, None), "no range given");
    assert!(c.players().is_empty());
    assert_eq!((a.status().players, b.status().players, c.status().players), (2, 1, 0));
    assert!(a.take_events().is_empty() && b.take_events().is_empty(), "nobody talked");
    for v in [a, b, c] {
        v.stop();
    }
}

/// Talking: A holds push to talk for 1 s, sending to B, who hears A. A's Koetama tells A started and stopped (being
/// sent), B's the same (heard; the stop by A's last packet, not 0.5 s of silence after it).
#[test]
fn talking_is_told_on_both_sides() {
    let m = mock_relay();
    let (room, key) = new_room();
    let (ana, bo) = (PlayerId::string("ana"), PlayerId::string("bo"));
    let fa = feed_ids(&room, &key, &ana, &[&bo], &[], &[&bo]);
    let fb = feed_ids(&room, &key, &bo, &[], &[&ana], &[]);
    let (a, b) = (Voice::start(m.url.clone(), kd_common::null_log()), Voice::start(m.url.clone(), kd_common::null_log()));
    wait_connected(&[&a, &b], &[fa.clone(), fb.clone()], 10.0);
    let (x, block) = (tone(), 2400);
    let (mut ea, mut eb) = (Vec::new(), Vec::new());
    let t0 = Instant::now();
    for i in 0..50 {
        // (1 s held, then let go)
        a.set_feed(&Feed { ptt: Some(i < 20), ..fa.clone() });
        b.set_feed(&fb);
        a.push_mic(&x[(i % 40) * block..(i % 40 + 1) * block], false);
        let now = t0.elapsed().as_secs_f64();
        ea.extend(a.take_events().into_iter().map(|e| (now, e)));
        eb.extend(b.take_events().into_iter().map(|e| (now, e)));
        std::thread::sleep((t0 + Duration::from_millis(50 * (i as u64 + 1))).saturating_duration_since(Instant::now()));
    }
    let events = |e: &[(f64, VoiceEvent)]| e.iter().map(|(_, e)| e.clone()).collect::<Vec<_>>();
    assert_eq!(events(&ea), vec![said(&ana, true), said(&ana, false)], "sent");
    assert_eq!(events(&eb), vec![said(&ana, true), said(&ana, false)], "heard");
    let (sent_end, heard_end) = (ea[1].0, eb[1].0);
    assert!(heard_end - sent_end < 0.4, "B's stop came {:.2} s after A's: by the timeout, not the last packet", heard_end - sent_end);
    a.stop();
    b.stop();
}

/// A player id taken in the room: the relay replaces the first connection with the second (close 4000). The first
/// stays out (connecting again would push the other out, and so on forever) until its room or id changes. A relay
/// that refuses the id (409): the same.
#[test]
fn a_taken_id_stays_out_of_the_room() {
    let m = mock_relay();
    let (room, key) = new_room();
    let log = kd_common::null_log();
    let (a, b) = (Voice::start(m.url.clone(), log.clone()), Voice::start(m.url.clone(), log.clone()));
    let f = feed(&room, &key, 5, &[]);
    wait_connected(&[&a], std::slice::from_ref(&f), 10.0);
    let both = [f.clone(), f.clone()];
    feed_until(&[&a, &b], &both, 10.0, || a.status().state == "id_taken" && b.status().state == "connected");
    // 3 s on: still out, the other still in, nobody connected again
    let t0 = Instant::now();
    feed_until(&[&a, &b], &both, 10.0, || t0.elapsed() > Duration::from_secs(3));
    assert_eq!((a.status().state, b.status().state), ("id_taken", "connected"));
    assert_eq!(count(&m.opened, 5), 2, "no connecting again");
    // another id: in again
    wait_connected(&[&a, &b], &[feed(&room, &key, 6, &[]), f.clone()], 10.0);
    // (back to 5: now A pushes B out) - another room: in again
    feed_until(&[&a, &b], &both, 10.0, || a.status().state == "connected" && b.status().state == "id_taken");
    let (room2, key2) = new_room();
    wait_connected(&[&a, &b], &[f.clone(), feed(&room2, &key2, 5, &[])], 10.0);
    // refused
    m.refuse.lock().unwrap().push(9);
    let c = Voice::start(m.url.clone(), log);
    let f9 = [feed(&room, &key, 9, &[])];
    feed_until(&[&c], &f9, 10.0, || c.status().state == "id_taken");
    let t0 = Instant::now();
    feed_until(&[&c], &f9, 10.0, || t0.elapsed() > Duration::from_secs(2));
    assert_eq!(c.status().state, "id_taken");
    assert_eq!((count(&m.tried, 9), count(&m.opened, 9)), (1, 0), "asked once");
    for v in [a, b, c] {
        v.stop();
    }
}

/// The live relay (KOETAMA_RELAY or the deployed one): needs the internet.
#[test]
#[ignore]
fn voices_through_the_live_relay() {
    match speech() {
        Some(x) => four_players(&kd_voice::relay_url(), None, &x, true),
        None => four_players(&kd_voice::relay_url(), None, &tone(), false),
    }
}
