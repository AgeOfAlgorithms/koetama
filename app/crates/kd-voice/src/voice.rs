//! The voice chat as the program uses it: a Voice follows the game's feed (the room, whom to send to, whom to hear),
//! takes the microphone's audio and gives the mixer the voices that arrive. It also tells the runtime who is in the
//! room (every packet names its sender; presence packets go out every PRESENCE_EVERY), the range each one announces,
//! and who starts and stops talking.
use crate::codec::{Decoder, Encoder};
use crate::gate::{Gate, Mode};
use crate::jitter::Jitter;
use crate::packet::{self, Packet};
use crate::relay::Conn;
use crate::{crypto, frames, FRAME, PER_PACKET};
use kd_common::feed::{relay_id, Feed, PlayerId};
use kd_common::Log;
use std::collections::{HashMap, VecDeque};
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
/// how often a presence packet goes to the feed's players ("I am in the room")
pub const PRESENCE_EVERY: Duration = Duration::from_secs(5);
/// s: a player heard from (any packet) this recently is in the room (players())
pub const PRESENT: f64 = 15.0;
/// a connection that lasted this long starts the waits over at 1 s when it drops
const STEADY: Duration = Duration::from_secs(30);
/// s without a feed: the game is gone (kd_audio::STALE): no connection
const STALE: f64 = kd_audio::STALE;
/// s without a packet: a sender's buffer is let go
const FORGET: f64 = 30.0;
/// microphone blocks queued for the voice thread before more are dropped (~3 s)
const QUEUE: usize = 64;
/// s without audio from a player talking: they stopped (the jitter buffer ends their stretch then too)
const QUIET: f64 = crate::jitter::END_AFTER;
/// a packet this few seqs before (or at) a stretch's last one belongs to it (it came late): it starts nothing
const STRAGGLER: i32 = 16;
/// events kept for the runtime (take_events) before the oldest go
const MAX_EVENTS: usize = 256;

const OFF: u8 = 0;
const CONNECTING: u8 = 1;
const CONNECTED: u8 = 2;
/// connecting, and the last UNREACHABLE_AFTER tries failed (the game tells its player: the network may block it)
const UNREACHABLE: u8 = 3;
const UNREACHABLE_AFTER: u32 = 2;
/// another connection in the room has this player's number (the relay replaced this one, or refused it): out of
/// that room until the room or the id changes (else the two would push each other out forever)
const ID_TAKEN: u8 = 4;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the window shows.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceStatus {
    /// "off" (no room, or the game is not running), "connecting", "connected", "unreachable" (still trying: the
    /// last tries failed), "id_taken" (another player in the room has this one's number: out of the room)
    pub state: &'static str,
    /// players whose voice arrived in the last HEARD s
    pub heard: usize,
    /// other players in the room (Voice::players)
    pub players: usize,
}

/// What happened in the voice chat, for the game (PROTOCOL.md "Koetama -> game": talking).
#[derive(Clone, Debug, PartialEq)]
pub enum VoiceEvent {
    /// a player's voice started (true) or stopped (false) being heard here - or this player's own being sent
    Talking { id: PlayerId, talking: bool },
}

/// What the feed says (the voice thread and the playback read it).
#[derive(Default)]
struct Cfg {
    room: String,
    key: Option<[u8; crypto::KEY_LEN]>,
    me: u16,
    /// this player's id as the game gives it, and how far their voice reaches now: in every packet
    me_id: Option<PlayerId>,
    range: Option<(f32, f32)>,
    to: Vec<u16>,
    /// the microphone is wanted, and push to talk (Some(held)) or the speech detector (None)
    mic: bool,
    ptt: Option<bool>,
    /// the real players this one hears (src 0, gain above 0)
    hears: Vec<u16>,
    /// the real players' ids (src 0), by relay id
    ids: HashMap<u16, PlayerId>,
    /// who gets presence: `to` and the real players
    present_to: Vec<u16>,
    fed: Option<Instant>,
}

impl Cfg {
    /// a room to be in: a good room, key and id, and a game feeding
    fn wanted(&self) -> bool {
        !self.room.is_empty() && self.key.is_some() && self.me > 0 && self.fed.is_some_and(|t| t.elapsed().as_secs_f64() <= STALE)
    }
}

/// The room as relay ids are made from it: without its region.
fn room_name(room: &str) -> &str {
    room.split('@').next().unwrap_or_default()
}

struct Peer {
    jitter: Jitter<Decoder>,
}

/// A player heard from (any packet): when last, and what their packets say.
struct Seen {
    t: f64,
    /// their id ("": none given, or not theirs)
    id: PlayerId,
    range: Option<(f32, f32)>,
}

/// Who is talking now, and the events the runtime has not taken yet.
#[derive(Default)]
struct Talk {
    /// the players heard talking: relay id -> (their id as told, when their last audio came)
    remote: HashMap<u16, (PlayerId, f64)>,
    /// each player's last stretch's last packet (its seq)
    ended: HashMap<u16, u32>,
    /// this player's voice is being sent (the id told)
    local: Option<PlayerId>,
    events: VecDeque<VoiceEvent>,
}

impl Talk {
    fn tell(&mut self, id: PlayerId, talking: bool) {
        if self.events.len() >= MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(VoiceEvent::Talking { id, talking });
    }

    /// A late packet of a stretch that ended.
    fn straggler(&self, from: u16, seq: u32) -> bool {
        self.ended.get(&from).is_some_and(|&e| (-STRAGGLER..=0).contains(&(seq.wrapping_sub(e) as i32)))
    }

    /// Audio from a player this one hears, at `now` (s); id: theirs (None: not known - no events).
    fn heard(&mut self, from: u16, seq: u32, last: bool, id: Option<PlayerId>, now: f64) {
        if let Some(t) = self.remote.get_mut(&from) {
            t.1 = now;
        } else if let Some(id) = id.filter(|_| !self.straggler(from, seq)) {
            self.tell(id.clone(), true);
            self.remote.insert(from, (id, now));
        }
        if last {
            self.ended.insert(from, seq);
            self.stop(from);
        }
    }

    fn stop(&mut self, from: u16) {
        if let Some((id, _)) = self.remote.remove(&from) {
            self.tell(id, false);
        }
    }

    /// The players whose audio stopped coming more than QUIET s before `now` stopped talking.
    fn quiet(&mut self, now: f64) {
        let gone: Vec<u16> = self.remote.iter().filter(|(_, (_, t))| now - t > QUIET).map(|(&r, _)| r).collect();
        for r in gone {
            self.stop(r);
        }
    }

    /// A packet of this player's voice went out (sent), or could not (no connection, nobody to send to); last: the
    /// end of the stretch. id: this player's (None: no events).
    fn sent(&mut self, id: Option<&PlayerId>, sent: bool, last: bool) {
        if sent && self.local.is_none() {
            if let Some(id) = id {
                self.tell(id.clone(), true);
                self.local = Some(id.clone());
            }
        }
        if last || !sent {
            if let Some(id) = self.local.take() {
                self.tell(id, false);
            }
        }
    }

    /// Nobody talks any more (another room, the end).
    fn end_all(&mut self) {
        let all: Vec<u16> = self.remote.keys().copied().collect();
        for r in all {
            self.stop(r);
        }
        self.sent(None, false, true);
        self.ended.clear();
    }
}

struct Shared {
    cfg: Mutex<Cfg>,
    peers: Mutex<HashMap<u16, Peer>>,
    /// everyone heard from in the room, by relay id
    seen: Mutex<HashMap<u16, Seen>>,
    talk: Mutex<Talk>,
    state: AtomicU8,
    running: AtomicBool,
    epoch: Instant,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Shared {
    fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    fn new() -> Shared {
        Shared {
            cfg: Mutex::new(Cfg::default()),
            peers: Mutex::new(HashMap::new()),
            seen: Mutex::new(HashMap::new()),
            talk: Mutex::new(Talk::default()),
            state: AtomicU8::new(OFF),
            running: AtomicBool::new(true),
            epoch: Instant::now(),
            thread: Mutex::new(None),
        }
    }

    /// A new room: what was heard in the last one goes (anyone talking there stops).
    fn forget_room(&self) {
        lock(&self.peers).clear();
        lock(&self.seen).clear();
        lock(&self.talk).end_all();
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
        let sh = Arc::new(Shared::new());
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
        c.me_id = feed.me_id.clone().filter(|_| c.me > 0);
        c.range = feed.range.map(|(n, f)| (n as f32, f as f32)).filter(|&r| packet::good_range(r));
        // (relay ids: 1..=65535, never this player's own)
        let rid = |&i: &i64| u16::try_from(i).ok().filter(|&i| i > 0 && i != me);
        c.to = feed.to.iter().filter_map(rid).collect();
        c.mic = feed.mic;
        c.ptt = feed.ptt;
        let real = || feed.speakers.iter().filter(|(_, s)| s.src == 0);
        c.hears = real().filter(|(_, s)| s.gain > 0.0).filter_map(|(id, _)| rid(id)).collect();
        c.ids = real().filter(|(_, s)| !s.id.text.is_empty()).filter_map(|(id, s)| Some((rid(id)?, s.id.clone()))).collect();
        let mut present_to = c.to.clone();
        for r in real().filter_map(|(id, _)| rid(id)) {
            if !present_to.contains(&r) {
                present_to.push(r);
            }
        }
        c.present_to = present_to;
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
            ID_TAKEN => "id_taken",
            _ => "off",
        };
        let now = self.sh.now();
        let heard = lock(&self.sh.peers).values().filter(|p| now - p.jitter.last_arrival() <= HEARD).count();
        VoiceStatus { state, heard, players: self.players().len() }
    }

    /// The other players in the room: heard from (any packet) in the last PRESENT s, by their id - the feed's for
    /// that relay id, else the one their packets give (neither: left out). Sorted, each once.
    pub fn players(&self) -> Vec<PlayerId> {
        let ids = lock(&self.sh.cfg).ids.clone();
        let now = self.sh.now();
        let mut out: Vec<PlayerId> = lock(&self.sh.seen)
            .iter()
            .filter(|(_, s)| now - s.t <= PRESENT)
            .map(|(r, s)| ids.get(r).unwrap_or(&s.id).clone())
            .filter(|id| !id.text.is_empty())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The range player `rid` (a relay id) announces in their packets: [near, far] in the game's units (None: none
    /// given, or not heard from in the last PRESENT s).
    pub fn range_of(&self, rid: i64) -> Option<(f64, f64)> {
        let rid = u16::try_from(rid).ok()?;
        let now = self.sh.now();
        let seen = lock(&self.sh.seen);
        let s = seen.get(&rid).filter(|s| now - s.t <= PRESENT)?;
        s.range.map(|(n, f)| (n as f64, f as f64))
    }

    /// Who started or stopped talking since the last call, oldest first (each player's state only when it changes).
    pub fn take_events(&self) -> Vec<VoiceEvent> {
        lock(&self.sh.talk).events.drain(..).collect()
    }

    /// Stops the voice thread (it leaves the room) and waits for it.
    pub fn stop(&self) {
        self.sh.running.store(false, Ordering::SeqCst);
        let t = lock(&self.sh.thread).take();
        if let Some(t) = t {
            let _ = t.join();
        }
        self.sh.forget_room();
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
        let p = Packet { seq: self.seq, last, frames: std::mem::take(&mut self.frames), ..Default::default() };
        self.seq = self.seq.wrapping_add(1);
        p
    }

    /// A block of microphone audio and what the player was doing -> the packets ready to send (a stretch's last one
    /// padded with silence to whole frames, flagged last). Their id and range are the sender's to fill in.
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

/// The connection as the voice thread holds it: the room and id it was made for, and when.
struct Link {
    conn: Conn,
    room: String,
    me: u16,
    since: Instant,
    /// when presence last went out (None: not yet)
    presence: Option<Instant>,
}

/// The voice thread: the connection (made and dropped as the feed says; again after a drop: 1, 2, 4 ... 30 s; never
/// again to a room where this player's number is taken), the packets arriving, the keep-alive, presence, the
/// microphone's blocks into packets out.
fn run(sh: Arc<Shared>, rx: Receiver<Block>, relay: String, log: Log) {
    let mut link: Option<Link> = None;
    let mut backoff = Duration::from_secs(1);
    let mut retry_at = Instant::now();
    let mut told = String::new();
    let mut last_ping = Instant::now();
    let mut last_room = String::new();
    // (the room and id the relay pushed out: not again until one of them changes)
    let mut taken: Option<(String, u16)> = None;
    let mut sender = match Sender::new() {
        Ok(s) => Some(s),
        Err(e) => {
            log(&format!("voice: {e} - your voice is not sent"));
            None
        }
    };
    let mut presence_seq = 0u32;
    let mut got = Vec::new();
    let mut last_forget = Instant::now();
    let mut failures = 0u32; // (failed tries in a row)
    while sh.running.load(Ordering::SeqCst) {
        let (wanted, room, me, key, to, mode, me_id, range, present_to) = {
            let c = lock(&sh.cfg);
            let mode = match (c.mic, c.ptt) {
                (false, _) => None,
                (true, Some(held)) => Some(Mode::PushToTalk(held)),
                (true, None) => Some(Mode::Detector(false)), // (talking: per block)
            };
            (c.wanted(), c.room.clone(), c.me, c.key, c.to.clone(), mode, c.me_id.clone(), c.range, c.present_to.clone())
        };
        if room != last_room {
            // (a new room: a new key - what was buffered and heard goes; connect at once)
            sh.forget_room();
            last_room = room.clone();
            backoff = Duration::from_secs(1);
            retry_at = Instant::now();
        }
        if taken.as_ref().is_some_and(|(r, m)| *r != room || *m != me) {
            taken = None;
        }
        // the connection
        if link.as_ref().is_some_and(|l| !wanted || l.room != room || l.me != me) {
            if let Some(l) = link.take() {
                l.conn.close();
                log("voice: left the room");
            }
        }
        if !wanted {
            sh.state.store(OFF, Ordering::SeqCst);
            failures = 0;
            told.clear();
        } else if taken.is_some() {
            sh.state.store(ID_TAKEN, Ordering::SeqCst);
        } else if link.is_none() && Instant::now() >= retry_at {
            if failures < UNREACHABLE_AFTER {
                sh.state.store(CONNECTING, Ordering::SeqCst);
            }
            match Conn::join(&relay, &room, me) {
                Ok(conn) => {
                    log(&format!("voice: in the room (player {me})"));
                    link = Some(Link { conn, room: room.clone(), me, since: Instant::now(), presence: None });
                    sh.state.store(CONNECTED, Ordering::SeqCst);
                    failures = 0;
                    last_ping = Instant::now();
                    told.clear();
                }
                Err(e) if e.taken => {
                    log(&format!("voice: {} - no voice in this room", e.text));
                    taken = Some((room.clone(), me));
                    sh.state.store(ID_TAKEN, Ordering::SeqCst);
                }
                Err(e) => {
                    let msg = format!("voice: cannot reach the relay: {}", e.text);
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
        // what arrives (poll waits a few ms: the loop's pace while connected), the keep-alive, presence
        let mut dropped = None;
        if let Some(l) = link.as_mut() {
            got.clear();
            if let Err(e) = l.conn.poll(&mut got) {
                dropped = Some(e);
            } else if last_ping.elapsed() >= PING_EVERY {
                last_ping = Instant::now();
                dropped = l.conn.ping().err();
            }
            if let Some(key) = key {
                let now = sh.now();
                for f in &got {
                    receive(&sh, f, &key, now);
                }
                if dropped.is_none() && l.presence.is_none_or(|t| t.elapsed() >= PRESENCE_EVERY) {
                    l.presence = Some(Instant::now());
                    let p = Packet { seq: presence_seq, presence: true, range, id: me_id.clone().unwrap_or_default(), ..Default::default() };
                    presence_seq = presence_seq.wrapping_add(1);
                    dropped = send(&mut l.conn, &key, me, &present_to, &p);
                }
            }
        }
        // the microphone's blocks (not connected: wait for them a little instead)
        let first = if link.is_some() {
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
                for mut p in s.push(&b.x, m) {
                    p.id = me_id.clone().unwrap_or_default();
                    p.range = range;
                    let mut sent = false;
                    if let (Some(l), Some(key), None, false) = (link.as_mut(), key.as_ref(), &dropped, to.is_empty()) {
                        dropped = send(&mut l.conn, key, me, &to, &p);
                        sent = dropped.is_none();
                    }
                    lock(&sh.talk).sent(me_id.as_ref(), sent, p.last);
                }
            }
            block = rx.try_recv().ok();
        }
        if let Some(e) = dropped {
            if let Some(l) = link.take() {
                let lasted = l.since.elapsed();
                if l.conn.replaced() {
                    taken = Some((l.room.clone(), l.me));
                }
                l.conn.close();
                if taken.is_some() {
                    log(&format!("voice: {e}: another player in this room has this player's number - no voice in this room"));
                    sh.state.store(ID_TAKEN, Ordering::SeqCst);
                } else {
                    log(&format!("voice: the connection to the relay dropped ({e}) - connecting again in {} s", backoff.as_secs()));
                    if lasted >= STEADY {
                        backoff = Duration::from_secs(1);
                    }
                    retry_at = Instant::now() + backoff;
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                    sh.state.store(CONNECTING, Ordering::SeqCst);
                }
            }
        }
        {
            let mut talk = lock(&sh.talk);
            talk.quiet(sh.now());
            if link.is_none() {
                talk.sent(None, false, false); // (not connected: this player's voice is not going out)
            }
        }
        if last_forget.elapsed() >= Duration::from_secs(5) {
            // (senders long quiet: their buffers and decoders go; players gone from the room)
            last_forget = Instant::now();
            let now = sh.now();
            lock(&sh.peers).retain(|_, p| now - p.jitter.last_arrival() <= FORGET);
            lock(&sh.seen).retain(|_, s| now - s.t <= PRESENT);
        }
    }
    if let Some(l) = link.take() {
        l.conn.close();
    }
    lock(&sh.talk).end_all();
    sh.state.store(OFF, Ordering::SeqCst);
}

/// A packet out to players `to` (MAX_TO a frame: more go in several); Some(error): the connection is broken.
fn send(c: &mut Conn, key: &[u8; crypto::KEY_LEN], me: u16, to: &[u16], p: &Packet) -> Option<String> {
    let payload = crypto::seal(key, me, &p.encode()).ok()?;
    for chunk in to.chunks(frames::MAX_TO) {
        let frame = frames::voice_frame(chunk, &payload)?;
        if let Err(e) = c.send(frame) {
            return Some(e);
        }
    }
    None
}

/// Notes a packet from `from` (any kind) at `now`: when, its range, and its id if it is theirs (the id's number in
/// this room is `from`: a packet cannot speak for someone else) -> that id.
fn note(sh: &Shared, from: u16, p: &Packet, room: &str, now: f64) -> Option<PlayerId> {
    let mut seen = lock(&sh.seen);
    // (an id already checked is not hashed again)
    let checked = seen.get(&from).is_some_and(|s| s.id == p.id);
    let theirs = !p.id.text.is_empty() && (checked || relay_id(room, &p.id) == i64::from(from));
    let id = if theirs { p.id.clone() } else { PlayerId::default() };
    seen.insert(from, Seen { t: now, id: id.clone(), range: p.range });
    theirs.then_some(id)
}

/// A frame from the relay at `now` (s): decrypted with the room's key, the sender noted (players(), range_of), and
/// audio from a player this one hears into their jitter buffer (and their talking told).
fn receive(sh: &Shared, f: &[u8], key: &[u8; crypto::KEY_LEN], now: f64) {
    let Some((from, payload)) = frames::parse_out(f) else { return };
    let Some(plain) = crypto::open(key, from, payload) else { return };
    let Some(p) = Packet::decode(&plain) else { return };
    let (hears, known, room) = {
        let c = lock(&sh.cfg);
        (c.hears.contains(&from), c.ids.get(&from).cloned(), room_name(&c.room).to_string())
    };
    let told = note(sh, from, &p, &room, now);
    if p.presence || p.frames.is_empty() || !hears {
        return; // (not in the speakers, or gain 0: not played)
    }
    lock(&sh.talk).heard(from, p.seq, p.last, known.or(told), now);
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
    use kd_common::feed::Speaker;

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
        f.me_id = Some(PlayerId::string("me"));
        f.range = Some((8.0, 25.0));
        f.speakers.insert(2, Speaker { src: 0, gain: 0.5, id: PlayerId::string("two"), ..Default::default() });
        f.speakers.insert(3, Speaker { src: 0, gain: 0.0, id: PlayerId::number(3), ..Default::default() });
        f.speakers.insert(4, Speaker { src: 1, gain: 1.0, ..Default::default() });
        f.speakers.insert(5, Speaker { src: 0, gain: 0.0, id: PlayerId::string("five"), ..Default::default() });
        v.set_feed(&f);
        {
            let c = lock(&v.sh.cfg);
            assert!(c.wanted() && c.me == 7 && c.key == Some([0xbb; 32]));
            assert_eq!(c.to, vec![2, 3], "never to myself");
            assert_eq!(c.hears, vec![2], "src 0 and gain above 0 only");
            assert_eq!(c.present_to, vec![2, 3, 5], "presence: `to` and every real player");
            assert_eq!(c.ids.len(), 3);
            assert_eq!((c.me_id.clone(), c.range), (Some(PlayerId::string("me")), Some((8.0, 25.0))));
        }
        // (a range that cannot go in a packet: none)
        v.set_feed(&Feed { range: Some((5.0, 1.0)), ..f.clone() });
        assert_eq!(lock(&v.sh.cfg).range, None);
        v.set_feed(&Feed { key: "b".repeat(64), me: 7, me_id: Some(PlayerId::string("me")), ..Default::default() });
        assert!(!lock(&v.sh.cfg).wanted(), "no room");
        assert_eq!(lock(&v.sh.cfg).me_id, None, "no room: no id");
        v.stop();
        assert_eq!(v.status(), VoiceStatus { state: "off", heard: 0, players: 0 });
    }

    const ROOM: &str = "0123456789abcdef0123456789abcdef";
    const KEY: [u8; 32] = [5; 32];

    /// A frame as the relay hands it on: packet p from player `from`, sealed with KEY.
    fn frame(from: u16, p: &Packet) -> Vec<u8> {
        let mut f = vec![frames::VOICE];
        f.extend_from_slice(&from.to_be_bytes());
        f.extend(crypto::seal(&KEY, from, &p.encode()).unwrap());
        f
    }

    /// A Voice (its thread not started) in ROOM, hearing `hears` (relay ids with the ids the feed gives them).
    fn listener(hears: &[(i64, Option<PlayerId>)]) -> Voice {
        let (tx, _) = sync_channel(1);
        let v = Voice { sh: Arc::new(Shared::new()), tx };
        let mut f = Feed { room: ROOM.into(), key: "05".repeat(32), me: 1, me_id: Some(PlayerId::string("me")), ..Default::default() };
        for (rid, id) in hears {
            f.speakers.insert(*rid, Speaker { src: 0, gain: 1.0, id: id.clone().unwrap_or_default(), ..Default::default() });
        }
        v.set_feed(&f);
        v
    }

    fn talking(id: &PlayerId, talking: bool) -> VoiceEvent {
        VoiceEvent::Talking { id: id.clone(), talking }
    }

    #[test]
    fn players_and_ranges_from_any_packet() {
        let ana = PlayerId::string("ana");
        let bo = PlayerId::number(76561198000000002);
        let (ra, rb) = (relay_id(ROOM, &ana) as u16, relay_id(ROOM, &bo) as u16);
        // (the feed knows ana by another name: the feed's wins; bo is not in the feed: his packets' id)
        let v = listener(&[(ra.into(), Some(PlayerId::string("Ana (feed)")))]);
        let presence = |id: &PlayerId, range| Packet { presence: true, id: id.clone(), range, ..Default::default() };
        receive(&v.sh, &frame(ra, &presence(&ana, Some((8.0, 25.0)))), &KEY, v.sh.now());
        receive(&v.sh, &frame(rb, &presence(&bo, None)), &KEY, v.sh.now());
        assert_eq!(v.players(), vec![bo.clone(), PlayerId::string("Ana (feed)")]);
        assert_eq!((v.range_of(ra.into()), v.range_of(rb.into()), v.range_of(9)), (Some((8.0, 25.0)), None, None));
        assert_eq!(v.status().players, 2);
        // an id that is not the sender's (its number is another): not taken; another key: not heard at all
        let liar = PlayerId::string("liar");
        let other = if relay_id(ROOM, &liar) == 77 { 78 } else { 77 };
        receive(&v.sh, &frame(other, &presence(&liar, None)), &KEY, v.sh.now());
        let mut bad = frame(99, &presence(&PlayerId::string("x"), None));
        bad[5] ^= 1;
        receive(&v.sh, &bad, &KEY, v.sh.now());
        assert_eq!(v.players().len(), 2, "{:?}", v.players());
        assert!(lock(&v.sh.seen).contains_key(&other) && !lock(&v.sh.seen).contains_key(&99));
        // gone PRESENT s later
        lock(&v.sh.seen).values_mut().for_each(|s| s.t -= PRESENT + 1.0);
        assert!(v.players().is_empty() && v.range_of(ra.into()).is_none());
        // presence is no audio: nothing to play, no talking
        assert!(lock(&v.sh.peers).is_empty() && v.take_events().is_empty());
    }

    #[test]
    fn remote_talking_starts_and_stops() {
        let ana = PlayerId::string("ana");
        let ra = relay_id(ROOM, &ana) as u16;
        let v = listener(&[(ra.into(), Some(ana.clone()))]);
        let audio = |seq, last| Packet { seq, last, id: ana.clone(), frames: vec![vec![0xf8]; 3], ..Default::default() };
        // a stretch: true once, false on its last packet
        for (seq, t) in [(0, 0.0), (1, 0.06), (2, 0.12)] {
            receive(&v.sh, &frame(ra, &audio(seq, false)), &KEY, t);
        }
        receive(&v.sh, &frame(ra, &audio(3, true)), &KEY, 0.18);
        assert_eq!(v.take_events(), vec![talking(&ana, true), talking(&ana, false)]);
        // (a late packet of that stretch starts nothing)
        receive(&v.sh, &frame(ra, &audio(2, false)), &KEY, 0.2);
        lock(&v.sh.talk).quiet(1.0);
        assert!(v.take_events().is_empty());
        // the next stretch's last packet lost: false 0.5 s after the last audio
        receive(&v.sh, &frame(ra, &audio(4, false)), &KEY, 2.0);
        receive(&v.sh, &frame(ra, &audio(5, false)), &KEY, 2.06);
        lock(&v.sh.talk).quiet(2.5);
        assert_eq!(v.take_events(), vec![talking(&ana, true)]);
        lock(&v.sh.talk).quiet(2.57);
        assert_eq!(v.take_events(), vec![talking(&ana, false)]);
        // a player not heard (not in the speakers) tells nothing
        let bo = PlayerId::string("bo");
        let rb = relay_id(ROOM, &bo) as u16;
        receive(&v.sh, &frame(rb, &Packet { id: bo.clone(), frames: vec![vec![0xf8]], ..Default::default() }), &KEY, 3.0);
        assert!(v.take_events().is_empty());
        // a new room: whoever talks stops
        receive(&v.sh, &frame(ra, &audio(6, false)), &KEY, 4.0);
        v.sh.forget_room();
        assert_eq!(v.take_events(), vec![talking(&ana, true), talking(&ana, false)]);
    }

    #[test]
    fn remote_talking_needs_an_id() {
        // (no id in the feed or the packets: the voice plays, no events)
        let v = listener(&[(42, None)]);
        receive(&v.sh, &frame(42, &Packet { frames: vec![vec![0xf8]; 3], ..Default::default() }), &KEY, 0.0);
        assert!(v.take_events().is_empty());
        assert_eq!(lock(&v.sh.peers).len(), 1);
    }

    #[test]
    fn local_talking_starts_and_stops() {
        let me = PlayerId::string("me");
        let mut t = Talk::default();
        t.sent(Some(&me), true, false);
        t.sent(Some(&me), true, false);
        t.sent(Some(&me), true, true);
        assert_eq!(t.events.drain(..).collect::<Vec<_>>(), vec![talking(&me, true), talking(&me, false)]);
        // nobody to send to mid-stretch: stopped; then sent again: started again
        t.sent(Some(&me), true, false);
        t.sent(Some(&me), false, false);
        t.sent(Some(&me), false, false);
        t.sent(Some(&me), true, false);
        t.end_all();
        let want = vec![talking(&me, true), talking(&me, false), talking(&me, true), talking(&me, false)];
        assert_eq!(t.events.drain(..).collect::<Vec<_>>(), want);
        // no id: nothing to tell
        t.sent(None, true, false);
        t.sent(None, true, true);
        assert!(t.events.is_empty());
        // (events nobody takes: only the newest MAX_EVENTS kept)
        for _ in 0..MAX_EVENTS {
            t.sent(Some(&me), true, true);
        }
        assert_eq!(t.events.len(), MAX_EVENTS);
    }
}
