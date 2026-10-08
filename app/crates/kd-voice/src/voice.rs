//! The voice chat as the program uses it: a Voice follows the game's feed (the room, whom to send to, whom to hear),
//! takes the microphone's audio and gives the mixer the voices that arrive.
use crate::codec::{Decoder, Encoder};
use crate::gate::{Gate, Mode};
use crate::jitter::Jitter;
use crate::packet::Packet;
use crate::relay::Conn;
use crate::{crypto, frames, FRAME, PER_PACKET};
use kd_common::feed::Feed;
use kd_common::Log;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// s: a voice heard this recently counts as heard (the status)
pub const HEARD: f64 = 5.0;
/// the keep-alive's interval
pub const PING_EVERY: Duration = Duration::from_secs(20);
/// the longest wait before connecting again (1, 2, 4 ... s)
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// a connection that lasted this long starts the waits over at 1 s when it drops
const STEADY: Duration = Duration::from_secs(30);
/// s without a feed: the game is gone (kd_audio::STALE): no connection
const STALE: f64 = kd_audio::STALE;
/// s without a packet: a sender's buffer is let go
const FORGET: f64 = 30.0;
/// microphone blocks queued for the voice thread before more are dropped (~3 s)
const QUEUE: usize = 64;

const OFF: u8 = 0;
const CONNECTING: u8 = 1;
const CONNECTED: u8 = 2;
/// connecting, and the last UNREACHABLE_AFTER tries failed (the game tells its player: the network may block it)
const UNREACHABLE: u8 = 3;
const UNREACHABLE_AFTER: u32 = 2;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the window shows.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceStatus {
    /// "off" (no room, or the game is not running), "connecting", "connected", "unreachable" (still trying: the
    /// last tries failed)
    pub state: &'static str,
    /// players whose voice arrived in the last HEARD s
    pub heard: usize,
}

/// What the feed says (the voice thread and the playback read it).
#[derive(Default)]
struct Cfg {
    room: String,
    key: Option<[u8; crypto::KEY_LEN]>,
    me: u16,
    to: Vec<u16>,
    /// the microphone is wanted, and push to talk (Some(held)) or the speech detector (None)
    mic: bool,
    ptt: Option<bool>,
    /// the real players this one hears (src 0, gain above 0)
    hears: Vec<u16>,
    fed: Option<Instant>,
}

impl Cfg {
    /// a room to be in: a good room, key and id, and a game feeding
    fn wanted(&self) -> bool {
        !self.room.is_empty() && self.key.is_some() && self.me > 0 && self.fed.is_some_and(|t| t.elapsed().as_secs_f64() <= STALE)
    }
}

struct Peer {
    jitter: Jitter<Decoder>,
}

struct Shared {
    cfg: Mutex<Cfg>,
    peers: Mutex<HashMap<u16, Peer>>,
    state: AtomicU8,
    running: AtomicBool,
    epoch: Instant,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Shared {
    fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }
}

/// A block of microphone audio and whether the speech detector heard speech then.
struct Block {
    x: Vec<f32>,
    talking: bool,
}

/// The voice chat. A handle (clones share it); the voice thread runs from start() until stop() (or the last handle
/// is dropped... stop() it: the thread holds one).
#[derive(Clone)]
pub struct Voice {
    sh: Arc<Shared>,
    tx: SyncSender<Block>,
}

impl Voice {
    /// Starts the voice thread for the relay at `relay` (crate::relay_url()). Nothing connects until a feed names a
    /// room.
    pub fn start(relay: String, log: Log) -> Voice {
        let sh = Arc::new(Shared {
            cfg: Mutex::new(Cfg::default()),
            peers: Mutex::new(HashMap::new()),
            state: AtomicU8::new(OFF),
            running: AtomicBool::new(true),
            epoch: Instant::now(),
            thread: Mutex::new(None),
        });
        let (tx, rx) = sync_channel(QUEUE);
        let sh2 = sh.clone();
        let t = std::thread::Builder::new().name("voice".into()).spawn(move || run(sh2, rx, relay, log)).ok();
        *lock(&sh.thread) = t;
        Voice { sh, tx }
    }

    /// The game's latest feed (as each is read: whom to send to follows the game at once).
    pub fn set_feed(&self, feed: &Feed) {
        let mut c = lock(&self.sh.cfg);
        let key = crypto::key_from_hex(&feed.key);
        let me = u16::try_from(feed.me).unwrap_or(0);
        if feed.room.is_empty() || key.is_none() || me == 0 {
            c.room.clear();
            c.key = None;
            c.me = 0;
        } else {
            // (a region is part of the room: another region, another room - relay.rs room_url)
            c.room = if feed.region.is_empty() { feed.room.clone() } else { format!("{}@{}", feed.room, feed.region) };
            c.key = key;
            c.me = me;
        }
        c.to = feed.to.iter().filter_map(|&i| u16::try_from(i).ok()).filter(|&i| i > 0 && i != me).collect();
        c.mic = feed.mic;
        c.ptt = feed.ptt;
        c.hears = feed
            .speakers
            .iter()
            .filter(|(_, s)| s.src == 0 && s.gain > 0.0)
            .filter_map(|(&id, _)| u16::try_from(id).ok())
            .collect();
        c.fed = Some(Instant::now());
    }

    /// The microphone's audio (mono, RATE: 48 kHz), from its callback - never waits (far behind: dropped). talking:
    /// the speech detector hears speech now (kd_speech::Listener::talking).
    pub fn push_mic(&self, x: &[f32], talking: bool) {
        match self.tx.try_send(Block { x: x.to_vec(), talking }) {
            Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// The voices that arrive, for the mixer (kd_audio::Mixer::streams).
    pub fn playback(&self) -> Playback {
        Playback(self.sh.clone())
    }

    pub fn status(&self) -> VoiceStatus {
        let state = match self.sh.state.load(Ordering::SeqCst) {
            CONNECTED => "connected",
            CONNECTING => "connecting",
            UNREACHABLE => "unreachable",
            _ => "off",
        };
        let now = self.sh.now();
        let heard = lock(&self.sh.peers).values().filter(|p| now - p.jitter.last_arrival() <= HEARD).count();
        VoiceStatus { state, heard }
    }

    /// Stops the voice thread (it leaves the room) and waits for it.
    pub fn stop(&self) {
        self.sh.running.store(false, Ordering::SeqCst);
        let t = lock(&self.sh.thread).take();
        if let Some(t) = t {
            let _ = t.join();
        }
        lock(&self.sh.peers).clear();
    }
}

/// The voices that arrive, as the mixer pulls them (kd_audio::Streams): a speaker with src 0 and that player id.
pub struct Playback(Arc<Shared>);

impl kd_audio::Streams for Playback {
    fn pull(&mut self, id: i64, out: &mut [f32]) -> bool {
        let Ok(id) = u16::try_from(id) else {
            out.fill(0.0);
            return false;
        };
        let now = self.0.now();
        match lock(&self.0.peers).get_mut(&id) {
            Some(p) => p.jitter.pull(out, now),
            None => {
                out.fill(0.0);
                false
            }
        }
    }
}

/// This player's voice into packets: the gate, Opus, the packet layout (seq counts up across stretches).
pub struct Sender {
    gate: Gate,
    enc: Encoder,
    pcm: Vec<f32>,
    frames: Vec<Vec<u8>>,
    seq: u32,
}

impl Sender {
    pub fn new() -> Result<Sender, String> {
        Ok(Sender { gate: Gate::new(), enc: Encoder::new()?, pcm: Vec::new(), frames: Vec::new(), seq: 0 })
    }

    fn encode_pending(&mut self) {
        while self.pcm.len() >= FRAME {
            let f: Vec<f32> = self.pcm.drain(..FRAME).collect();
            if let Ok(b) = self.enc.encode(&f) {
                self.frames.push(b);
            }
        }
    }

    fn packet(&mut self, last: bool) -> Packet {
        let p = Packet { seq: self.seq, last, frames: std::mem::take(&mut self.frames) };
        self.seq = self.seq.wrapping_add(1);
        p
    }

    /// A block of microphone audio and what the player was doing -> the packets ready to send (a stretch's last one
    /// padded with silence to whole frames, flagged last).
    pub fn push(&mut self, x: &[f32], mode: Mode) -> Vec<Packet> {
        let g = self.gate.push(x, mode);
        self.pcm.extend_from_slice(&g.audio);
        let mut out = Vec::new();
        loop {
            self.encode_pending();
            if self.frames.len() < PER_PACKET {
                break;
            }
            let rest = self.frames.split_off(PER_PACKET);
            let p = self.packet(false);
            self.frames = rest;
            out.push(p);
        }
        if g.end {
            if !self.pcm.is_empty() {
                self.pcm.resize(FRAME, 0.0);
                self.encode_pending();
            }
            if self.frames.is_empty() {
                // (the stretch ended on a packet's edge: one frame of silence carries the end)
                self.pcm.resize(FRAME, 0.0);
                self.encode_pending();
            }
            out.push(self.packet(true));
        }
        out
    }
}

/// The voice thread: the connection (made and dropped as the feed says; again after a drop: 1, 2, 4 ... 30 s), the
/// voices arriving, the keep-alive, the microphone's blocks into packets out.
fn run(sh: Arc<Shared>, rx: Receiver<Block>, relay: String, log: Log) {
    let mut conn: Option<(Conn, String, u16, Instant)> = None;
    let mut backoff = Duration::from_secs(1);
    let mut retry_at = Instant::now();
    let mut told = String::new();
    let mut last_ping = Instant::now();
    let mut last_room = String::new();
    let mut sender = match Sender::new() {
        Ok(s) => Some(s),
        Err(e) => {
            log(&format!("voice: {e} - your voice is not sent"));
            None
        }
    };
    let mut got = Vec::new();
    let mut last_forget = Instant::now();
    let mut failures = 0u32; // (failed tries in a row)
    while sh.running.load(Ordering::SeqCst) {
        let (wanted, room, me, key, to, mode) = {
            let c = lock(&sh.cfg);
            let mode = match (c.mic, c.ptt) {
                (false, _) => None,
                (true, Some(held)) => Some(Mode::PushToTalk(held)),
                (true, None) => Some(Mode::Detector(false)), // (talking: per block)
            };
            (c.wanted(), c.room.clone(), c.me, c.key, c.to.clone(), mode)
        };
        if room != last_room {
            // (a new room: a new key - what was buffered goes; connect at once)
            lock(&sh.peers).clear();
            last_room = room.clone();
            backoff = Duration::from_secs(1);
            retry_at = Instant::now();
        }
        // the connection
        if conn.as_ref().is_some_and(|(_, r, m, _)| !wanted || *r != room || *m != me) {
            if let Some((c, ..)) = conn.take() {
                c.close();
                log("voice: left the room");
            }
        }
        if !wanted {
            sh.state.store(OFF, Ordering::SeqCst);
            failures = 0;
            told.clear();
        } else if conn.is_none() && Instant::now() >= retry_at {
            if failures < UNREACHABLE_AFTER {
                sh.state.store(CONNECTING, Ordering::SeqCst);
            }
            match Conn::open(&relay, &room, me) {
                Ok(c) => {
                    log(&format!("voice: in the room (player {me})"));
                    conn = Some((c, room.clone(), me, Instant::now()));
                    sh.state.store(CONNECTED, Ordering::SeqCst);
                    failures = 0;
                    last_ping = Instant::now();
                    told.clear();
                }
                Err(e) => {
                    let msg = format!("voice: cannot reach the relay: {e}");
                    if msg != told {
                        log(&format!("{msg} - trying again (every {} s at most)", BACKOFF_MAX.as_secs()));
                        told = msg;
                    }
                    retry_at = Instant::now() + backoff;
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                    failures += 1;
                    if failures >= UNREACHABLE_AFTER {
                        sh.state.store(UNREACHABLE, Ordering::SeqCst);
                    }
                }
            }
        }
        // what arrives (poll waits a few ms: the loop's pace while connected)
        let mut dropped = None;
        if let Some((c, _, _, since)) = conn.as_mut() {
            got.clear();
            if let Err(e) = c.poll(&mut got) {
                dropped = Some((e, since.elapsed()));
            } else if last_ping.elapsed() >= PING_EVERY {
                last_ping = Instant::now();
                if let Err(e) = c.ping() {
                    dropped = Some((e, since.elapsed()));
                }
            }
            if let Some(key) = key {
                for f in &got {
                    receive(&sh, f, &key);
                }
            }
        }
        // the microphone's blocks (not connected: wait for them a little instead)
        let first = if conn.is_some() {
            rx.try_recv().ok()
        } else {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(b) => Some(b),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        };
        let mut block = first;
        while let Some(b) = block {
            if let Some(s) = sender.as_mut() {
                let m = match mode {
                    None => Mode::Off,
                    Some(Mode::Detector(_)) => Mode::Detector(b.talking),
                    Some(m) => m,
                };
                // (no room: nothing is sent, the gate still keeps its pre-roll)
                let m = if wanted { m } else { Mode::Off };
                for p in s.push(&b.x, m) {
                    if let (Some((c, ..)), Some(key), false) = (conn.as_mut(), key.as_ref(), to.is_empty()) {
                        if let Some(err) = send(c, key, me, &to, &p) {
                            dropped = Some((err, Duration::ZERO));
                            break;
                        }
                    }
                }
            }
            block = rx.try_recv().ok();
        }
        if let Some((e, lasted)) = dropped {
            if let Some((c, ..)) = conn.take() {
                c.close();
            }
            log(&format!("voice: the connection to the relay dropped ({e}) - connecting again in {} s", backoff.as_secs()));
            if lasted >= STEADY {
                backoff = Duration::from_secs(1);
            }
            retry_at = Instant::now() + backoff;
            backoff = (backoff * 2).min(BACKOFF_MAX);
            sh.state.store(CONNECTING, Ordering::SeqCst);
        }
        if last_forget.elapsed() >= Duration::from_secs(5) {
            // (senders long quiet: their buffers and decoders go)
            last_forget = Instant::now();
            let now = sh.now();
            lock(&sh.peers).retain(|_, p| now - p.jitter.last_arrival() <= FORGET);
        }
    }
    if let Some((c, ..)) = conn.take() {
        c.close();
    }
    sh.state.store(OFF, Ordering::SeqCst);
}

/// A packet out to players `to`; Some(error): the connection is broken.
fn send(c: &mut Conn, key: &[u8; crypto::KEY_LEN], me: u16, to: &[u16], p: &Packet) -> Option<String> {
    let payload = crypto::seal(key, me, &p.encode()).ok()?;
    let frame = frames::voice_frame(&to[..to.len().min(frames::MAX_TO)], &payload)?;
    c.send(frame).err()
}

/// A frame from the relay: from a player this one hears, decrypted with the room's key, into their jitter buffer.
fn receive(sh: &Shared, f: &[u8], key: &[u8; crypto::KEY_LEN]) {
    let Some((from, payload)) = frames::parse_out(f) else { return };
    if !lock(&sh.cfg).hears.contains(&from) {
        return; // (not in the speakers, or gain 0: not played)
    }
    let Some(plain) = crypto::open(key, from, payload) else { return };
    let Some(p) = Packet::decode(&plain) else { return };
    let now = sh.now();
    let mut peers = lock(&sh.peers);
    let peer = match peers.entry(from) {
        std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
        std::collections::hash_map::Entry::Vacant(e) => match Decoder::new() {
            Ok(d) => e.insert(Peer { jitter: Jitter::new(d) }),
            Err(_) => return,
        },
    };
    peer.jitter.insert(p, now);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_packets() {
        let mut s = Sender::new().unwrap();
        let tone: Vec<f32> = (0..2400).map(|i| 0.3 * (i as f32 * 0.06).sin()).collect();
        // quiet: nothing
        assert!(s.push(&tone, Mode::PushToTalk(false)).is_empty());
        // held for 3 blocks, after the one quiet block as its pre-roll (2400 + 7200 samples = 10 frames): three
        // packets of three frames
        let mut ps = Vec::new();
        for _ in 0..3 {
            ps.extend(s.push(&tone, Mode::PushToTalk(true)));
        }
        assert_eq!(ps.len(), 3);
        assert!(ps.iter().all(|p| p.frames.len() == 3 && !p.last));
        assert_eq!((ps[0].seq, ps[1].seq, ps[2].seq), (0, 1, 2));
        // let go: the tail (12000 samples) and the end: the last packet flagged, padded to whole frames
        let mut end = Vec::new();
        for _ in 0..6 {
            end.extend(s.push(&tone, Mode::PushToTalk(false)));
        }
        let last = end.last().unwrap();
        assert!(last.last && end[..end.len() - 1].iter().all(|p| !p.last));
        // 1 frame left + 12000 = 12960 samples: 13.5 -> 14 frames = 4 full packets + a last one of 2
        let frames: usize = end.iter().map(|p| p.frames.len()).sum();
        assert_eq!(frames, 14);
        assert_eq!(end.iter().map(|p| p.seq).collect::<Vec<_>>(), vec![3, 4, 5, 6, 7]);
        // the end on a packet's edge: a frame of silence carries it
        let mut t = Sender::new().unwrap();
        let p = t.push(&vec![0.1; 2880], Mode::Detector(true));
        assert_eq!(p.len(), 1);
        let e = t.push(&[], Mode::Detector(false));
        assert!(e.len() == 1 && e[0].last && e[0].frames.len() == 1);
    }

    #[test]
    fn the_feed_sets_what_is_sent_and_heard() {
        let v = Voice::start("ws://127.0.0.1:1".into(), kd_common::null_log());
        let mut f = Feed { room: "a".repeat(32), key: "b".repeat(64), me: 7, to: vec![2, 7, 70000, 3], mic: true, ..Default::default() };
        f.speakers.insert(2, kd_common::feed::Speaker { src: 0, gain: 0.5, ..Default::default() });
        f.speakers.insert(3, kd_common::feed::Speaker { src: 0, gain: 0.0, ..Default::default() });
        f.speakers.insert(4, kd_common::feed::Speaker { src: 1, gain: 1.0, ..Default::default() });
        v.set_feed(&f);
        {
            let c = lock(&v.sh.cfg);
            assert!(c.wanted() && c.me == 7 && c.key == Some([0xbb; 32]));
            assert_eq!(c.to, vec![2, 3], "never to myself");
            assert_eq!(c.hears, vec![2], "src 0 and gain above 0 only");
        }
        v.set_feed(&Feed { key: "b".repeat(64), me: 7, ..Default::default() });
        assert!(!lock(&v.sh.cfg).wanted(), "no room");
        v.stop();
        assert_eq!(v.status(), VoiceStatus { state: "off", heard: 0 });
    }
}
