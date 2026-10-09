//! A hub's link to one of its players (PROTOCOL.md "Hub"): for a game whose script runs only on the host, the host's
//! Koetama (the hub) and each other player's Koetama talk through the relay. JSON messages, encrypted, cut into parts:
//!   plaintext [3][message: u32 BE][part: u8][parts: u8][bytes] (at most PART bytes), the hub relay id 1, the player 2.
//! The join code is used ONCE, for a key exchange: in the room the code makes (sealed with the code's key) the
//! player's Koetama says `{"hello": <its X25519 public key>}` and the hub answers `{"welcome": <its own>}`; both
//! derive the LINK (link_of: a key and a room from the code's key and the X25519 secret) and from then on talk only
//! in the link's room. The hub leaves the code's room after the first hello and never listens there again; feeds and
//! objects never go through it. A Channel is one end: a thread holding the connection (made again after a drop, to
//! the link's room once paired), messages in and out.
use crate::relay::Conn;
use crate::{crypto, frames};
use kd_common::{feed, Log};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use x25519_dalek::{PublicKey, StaticSecret};

/// the hub's relay id in a player's room, and the player's
pub const HUB: u16 = 1;
pub const PLAYER: u16 = 2;
/// the plaintext's type for a message part (voice packets are 2)
pub const MESSAGE: u8 = 3;
/// the most bytes of a message in one part (the relay takes 4000 a packet, with the nonce and tag)
pub const PART: usize = 3500;
/// a join code's characters: no 0 / O, 1 / I / L
pub const ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
/// the other end counts as there while it was heard from within this
pub const ALIVE: Duration = Duration::from_secs(5);
/// a paired link not heard from this long: its player's Koetama is gone - the hub drops it and makes the player a new
/// code (a link that merely dropped comes back long before: the player's Koetama connects again to the link's room)
pub const REPAIR_AFTER: Duration = Duration::from_secs(30);
/// the player's hello is said again this often until the hub answers
const HELLO_EVERY: Duration = Duration::from_secs(1);
const PING_EVERY: Duration = Duration::from_secs(20);
const BACKOFF_MAX: Duration = Duration::from_secs(10);

/// A new join code: 8 characters of ALPHABET (about 40 bits), shown as "K7QF-4MXA".
pub fn new_code() -> Result<String, String> {
    let b = crypto::random_bytes(8)?;
    let s: String = b.iter().map(|x| ALPHABET[*x as usize % ALPHABET.len()] as char).collect();
    Ok(format!("{}-{}", &s[..4], &s[4..]))
}

/// A code as a player typed it: its 8 characters (any case, with or without the dash and spaces); None if it is not
/// one ("o" and "i" / "l" read as 0 and 1 are not in it: refused, not guessed).
pub fn normalize_code(s: &str) -> Option<String> {
    let c: String = s.chars().filter(|c| !c.is_whitespace() && *c != '-').collect::<String>().to_ascii_uppercase();
    (c.len() == 8 && c.bytes().all(|b| ALPHABET.contains(&b))).then_some(c)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The room and key a code makes (both ends the same): HMAC-SHA256 of the code's 8 characters. Only the handshake
/// goes through this room.
pub fn pairing_room(code: &str) -> Option<(String, [u8; crypto::KEY_LEN])> {
    let c = normalize_code(code)?;
    let room = hex(&feed::hmac_sha256(c.as_bytes(), b"koetama hub room"))[..32].to_string();
    Some((room, feed::hmac_sha256(c.as_bytes(), b"koetama hub key")))
}

/// A message's plaintexts: [3][message][part][parts][bytes].
pub fn parts(message: u32, bytes: &[u8]) -> Vec<Vec<u8>> {
    let chunks: Vec<&[u8]> = if bytes.is_empty() { vec![&[][..]] } else { bytes.chunks(PART).collect() };
    let n = chunks.len().min(255);
    chunks
        .iter()
        .take(n)
        .enumerate()
        .map(|(i, c)| {
            let mut p = vec![MESSAGE];
            p.extend_from_slice(&message.to_be_bytes());
            p.push(i as u8);
            p.push(n as u8);
            p.extend_from_slice(c);
            p
        })
        .collect()
}

/// A message being put together: when its first part came, and its parts so far.
type Partial = (Instant, Vec<Option<Vec<u8>>>);

/// Puts a message's parts together again (parts of a message never completed are dropped after a while).
#[derive(Default)]
pub struct Assembler {
    open: HashMap<u32, Partial>,
}

impl Assembler {
    /// A plaintext in; Some(the whole message) once its last part is.
    pub fn add(&mut self, plain: &[u8]) -> Option<Vec<u8>> {
        if plain.len() < 7 || plain[0] != MESSAGE {
            return None;
        }
        let id = u32::from_be_bytes([plain[1], plain[2], plain[3], plain[4]]);
        let (part, n) = (plain[5] as usize, plain[6] as usize);
        if n == 0 || part >= n {
            return None;
        }
        self.open.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(10));
        let e = self.open.entry(id).or_insert_with(|| (Instant::now(), vec![None; n]));
        if e.1.len() != n {
            return None;
        }
        e.1[part] = Some(plain[7..].to_vec());
        if e.1.iter().all(Option::is_some) {
            let (_, ps) = self.open.remove(&id)?;
            return Some(ps.into_iter().flatten().flatten().collect());
        }
        None
    }
}

/// A paired link's room and key (both ends the same).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub room: String,
    pub key: [u8; crypto::KEY_LEN],
}

/// The link a handshake makes: key = HMAC-SHA256(code key, "koetama hub link" | X25519 shared secret | hub's public
/// key | player's public key), room = the first 32 hex digits of HMAC-SHA256(key, "koetama hub link room").
pub fn link_of(code_key: &[u8; crypto::KEY_LEN], shared: &[u8; 32], hub_pub: &[u8; 32], player_pub: &[u8; 32]) -> Link {
    let mut m = b"koetama hub link".to_vec();
    m.extend_from_slice(shared);
    m.extend_from_slice(hub_pub);
    m.extend_from_slice(player_pub);
    let key = feed::hmac_sha256(code_key, &m);
    let room = hex(&feed::hmac_sha256(&key, b"koetama hub link room"))[..crypto::ROOM_HEX].to_string();
    Link { room, key }
}

/// One end's side of the key exchange in the code's room (a fresh X25519 key pair: the private key from the OS).
pub struct Handshake {
    me: u16,
    code_key: [u8; crypto::KEY_LEN],
    secret: StaticSecret,
    public: [u8; 32],
    done: bool,
}

impl Handshake {
    pub fn new(code_key: [u8; crypto::KEY_LEN], me: u16) -> Result<Handshake, String> {
        let mut b = [0u8; 32];
        b.copy_from_slice(&crypto::random_bytes(32)?);
        let secret = StaticSecret::from(b);
        let public = PublicKey::from(&secret).to_bytes();
        Ok(Handshake { me, code_key, secret, public, done: false })
    }

    /// What the player says (again until answered): `{"hello": <public key, 64 hex>}`; the hub says nothing first.
    pub fn hello(&self) -> Option<Value> {
        (self.me == PLAYER && !self.done).then(|| serde_json::json!({ "hello": hex(&self.public) }))
    }

    /// Paired already (anything more in the code's room is ignored)?
    pub fn done(&self) -> bool {
        self.done
    }

    /// A message from the other end in the code's room -> (what to answer, the link). The hub takes the first good
    /// `hello` and answers `welcome`; the player takes a `welcome`. None: not for this end, not a key, or paired
    /// already (a second hello: ignored - the code is used up).
    pub fn answer(&mut self, msg: &Value) -> Option<(Option<Value>, Link)> {
        if self.done {
            return None;
        }
        let field = if self.me == HUB { "hello" } else { "welcome" };
        let theirs = crypto::key_from_hex(msg.get(field)?.as_str()?)?;
        let shared = self.secret.diffie_hellman(&PublicKey::from(theirs));
        // (a low-order point from the other end: no secret at all - refused)
        if !shared.was_contributory() {
            return None;
        }
        let (hub_pub, player_pub) = if self.me == HUB { (self.public, theirs) } else { (theirs, self.public) };
        let link = link_of(&self.code_key, shared.as_bytes(), &hub_pub, &player_pub);
        self.done = true;
        let reply = (self.me == HUB).then(|| serde_json::json!({ "welcome": hex(&self.public) }));
        Some((reply, link))
    }
}

/// A frame from the relay -> its plaintext, if it came from `peer` and opens under `key` (else dropped).
pub fn open_frame(key: &[u8; crypto::KEY_LEN], peer: u16, frame: &[u8]) -> Option<Vec<u8>> {
    let (from, payload) = frames::parse_out(frame)?;
    if from != peer {
        return None;
    }
    crypto::open(key, from, payload)
}

/// A message's frames for the relay (each part sealed under `key`, from `me` to `peer`).
pub fn message_frames(key: &[u8; crypto::KEY_LEN], me: u16, peer: u16, id: u32, bytes: &[u8]) -> Vec<Vec<u8>> {
    parts(id, bytes)
        .iter()
        .filter_map(|p| crypto::seal(key, me, p).ok().and_then(|payload| frames::voice_frame(&[peer], &payload)))
        .collect()
}

struct Shared {
    running: AtomicBool,
    /// heard from in the link's room (never the code's)
    heard: Mutex<Option<Instant>>,
    /// when the handshake was done
    paired: Mutex<Option<Instant>>,
    /// in the link's room
    connected: AtomicBool,
}

fn get<T: Copy>(m: &Mutex<T>) -> T {
    *m.lock().unwrap_or_else(|e| e.into_inner())
}

fn set<T>(m: &Mutex<T>, v: T) {
    *m.lock().unwrap_or_else(|e| e.into_inner()) = v;
}

/// One end of a hub's link with a player (`me`: HUB or PLAYER; the other end is the other one).
pub struct Channel {
    out: Sender<Vec<u8>>,
    inbox: Mutex<Receiver<Value>>,
    sh: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Channel {
    /// Starts the link for `code` (None: not a code). Nothing waits: the thread does the handshake in the code's
    /// room, then connects to the link's room (and again there after a drop).
    pub fn start(relay: String, code: &str, me: u16, log: Log) -> Option<Channel> {
        let (room, key) = pairing_room(code)?;
        let hs = match Handshake::new(key, me) {
            Ok(h) => h,
            Err(e) => {
                log(&format!("hub link: {e}"));
                return None;
            }
        };
        let peer = if me == HUB { PLAYER } else { HUB };
        let (out_tx, out_rx) = channel::<Vec<u8>>();
        let (in_tx, in_rx) = channel::<Value>();
        let sh = Arc::new(Shared {
            running: AtomicBool::new(true),
            heard: Mutex::new(None),
            paired: Mutex::new(None),
            connected: AtomicBool::new(false),
        });
        let sh2 = sh.clone();
        let ends = Ends { relay, code_room: room, code_key: key, me, peer };
        let thread =
            std::thread::Builder::new().name("hub link".into()).spawn(move || run(ends, hs, out_rx, in_tx, sh2, log)).ok();
        Some(Channel { out: out_tx, inbox: Mutex::new(in_rx), sh, thread })
    }

    /// A message to the other end, over the link (dropped if it is not up then - never sent in the code's room).
    pub fn send(&self, msg: &Value) {
        let _ = self.out.send(msg.to_string().into_bytes());
    }

    /// The messages that came over the link since the last call.
    pub fn received(&self) -> Vec<Value> {
        let rx = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        rx.try_iter().collect()
    }

    /// Was the other end heard from over the link lately (ALIVE)?
    pub fn other_there(&self) -> bool {
        get(&self.sh.heard).is_some_and(|t| t.elapsed() < ALIVE)
    }

    /// Is this end in the link's room (paired, connected)?
    pub fn connected(&self) -> bool {
        self.sh.connected.load(Ordering::SeqCst)
    }

    /// Was the handshake done (the code used up)?
    pub fn paired(&self) -> bool {
        get(&self.sh.paired).is_some()
    }

    /// How long the paired link has been silent (since the other end was last heard over it, or since pairing);
    /// None before pairing.
    pub fn quiet(&self) -> Option<Duration> {
        let paired = get(&self.sh.paired)?;
        let since = get(&self.sh.heard).map_or(paired, |h| h.max(paired));
        Some(since.elapsed())
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.sh.running.store(false, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Who and where: the relay, the code's room and key, this end and the other.
struct Ends {
    relay: String,
    code_room: String,
    code_key: [u8; crypto::KEY_LEN],
    me: u16,
    peer: u16,
}

/// Sends a message's frames; Err: the connection broke.
fn send_message(c: &mut Conn, frames: Vec<Vec<u8>>) -> Result<(), String> {
    for f in frames {
        c.send(f)?;
    }
    Ok(())
}

fn run(ends: Ends, mut hs: Handshake, out: Receiver<Vec<u8>>, inbox: Sender<Value>, sh: Arc<Shared>, log: Log) {
    let Ends { relay, code_room, code_key, me, peer } = ends;
    let mut link: Option<Link> = None;
    let mut conn: Option<Conn> = None;
    let mut backoff = Duration::from_secs(1);
    let mut retry_at = Instant::now();
    let mut last_ping = Instant::now();
    let mut hello_at = Instant::now();
    let mut told = String::new();
    let mut next_id: u32 = 0;
    let mut asm = Assembler::default();
    let mut got = Vec::new();
    while sh.running.load(Ordering::SeqCst) {
        let (room, key) = match &link {
            Some(l) => (l.room.as_str(), l.key),
            None => (code_room.as_str(), code_key),
        };
        if conn.is_none() {
            sh.connected.store(false, Ordering::SeqCst);
            if Instant::now() < retry_at {
                std::thread::sleep(Duration::from_millis(20));
                // (messages for the other end while the link is down: dropped - the next feed carries everything)
                while out.try_recv().is_ok() {}
                continue;
            }
            match Conn::open(&relay, room, me) {
                Ok(c) => {
                    conn = Some(c);
                    sh.connected.store(link.is_some(), Ordering::SeqCst);
                    backoff = Duration::from_secs(1);
                    last_ping = Instant::now();
                    hello_at = Instant::now() - HELLO_EVERY;
                    told.clear();
                }
                Err(e) => {
                    let msg = format!("hub link: cannot reach the relay: {e}");
                    if msg != told {
                        log(&msg);
                        told = msg;
                    }
                    retry_at = Instant::now() + backoff;
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                    continue;
                }
            }
        }
        let Some(c) = conn.as_mut() else { continue };
        let mut broken = None;
        let mut paired = None;
        // what the other end sent
        got.clear();
        if let Err(e) = c.poll(&mut got) {
            broken = Some(e);
        }
        for f in &got {
            let Some(plain) = open_frame(&key, peer, f) else { continue };
            let Some(msg) = asm.add(&plain) else { continue };
            let Ok(v) = serde_json::from_slice::<Value>(&msg) else { continue };
            if link.is_some() {
                set(&sh.heard, Some(Instant::now()));
                let _ = inbox.send(v);
                continue;
            }
            // (the code's room: the handshake only - nothing else is ever taken from it)
            let Some((reply, l)) = hs.answer(&v) else { continue };
            if let Some(r) = reply {
                next_id = next_id.wrapping_add(1);
                if let Err(e) = send_message(c, message_frames(&code_key, me, peer, next_id, r.to_string().as_bytes())) {
                    broken = Some(e);
                }
            }
            paired = Some(l);
            break;
        }
        if link.is_none() {
            // (nothing but the handshake goes through the code's room)
            while out.try_recv().is_ok() {}
            if paired.is_none() && broken.is_none() && hello_at.elapsed() >= HELLO_EVERY {
                hello_at = Instant::now();
                if let Some(h) = hs.hello() {
                    next_id = next_id.wrapping_add(1);
                    if let Err(e) = send_message(c, message_frames(&code_key, me, peer, next_id, h.to_string().as_bytes())) {
                        broken = Some(e);
                    }
                }
            }
        }
        // what this end sends, over the link
        while link.is_some() && broken.is_none() {
            let Ok(msg) = out.try_recv() else { break };
            next_id = next_id.wrapping_add(1);
            if let Err(e) = send_message(c, message_frames(&key, me, peer, next_id, &msg)) {
                broken = Some(e);
            }
        }
        if broken.is_none() && last_ping.elapsed() >= PING_EVERY {
            last_ping = Instant::now();
            if let Err(e) = c.ping() {
                broken = Some(e);
            }
        }
        if let Some(l) = paired {
            // (paired: out of the code's room for good, into the link's)
            if let Some(c) = conn.take() {
                c.close();
            }
            log("hub link: paired (the join code is used up) - moving to the link's own room");
            link = Some(l);
            set(&sh.paired, Some(Instant::now()));
            asm = Assembler::default();
            retry_at = Instant::now();
            backoff = Duration::from_secs(1);
            continue;
        }
        if let Some(e) = broken {
            if let Some(c) = conn.take() {
                c.close();
            }
            log(&format!("hub link: the connection dropped ({e}) - connecting again"));
            retry_at = Instant::now() + backoff;
        }
    }
    if let Some(c) = conn.take() {
        c.close();
    }
    sh.connected.store(false, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes() {
        let c = new_code().unwrap();
        assert_eq!(c.len(), 9);
        assert_eq!(normalize_code(&c).unwrap().len(), 8);
        assert_eq!(normalize_code(" k7qf 4mxa ").as_deref(), Some("K7QF4MXA"));
        assert!(normalize_code("K7QF-4MX").is_none() && normalize_code("K7QF-4MXO").is_none(), "short, or an O");
        assert_eq!(pairing_room("K7QF-4MXA"), pairing_room("k7qf4mxa"), "the same room however typed");
        assert_ne!(pairing_room("K7QF-4MXA").unwrap().0, pairing_room("K7QF-4MXB").unwrap().0);
    }

    #[test]
    fn parts_and_back() {
        let big: Vec<u8> = (0..9000u32).map(|i| (i % 251) as u8).collect();
        let ps = parts(7, &big);
        assert_eq!(ps.len(), 3);
        assert!(ps.iter().all(|p| p.len() <= PART + 7));
        let mut a = Assembler::default();
        // (out of order, with another message between)
        assert!(a.add(&ps[2]).is_none() && a.add(&parts(8, b"{}")[0]).is_some());
        assert!(a.add(&ps[0]).is_none());
        assert_eq!(a.add(&ps[1]), Some(big));
        assert_eq!(Assembler::default().add(&parts(1, b"")[0]), Some(Vec::new()));
        assert!(Assembler::default().add(&[2, 0, 0, 0, 1, 0, 1]).is_none(), "a voice packet, not a part");
    }

    /// A handshake between a hub and a player for `code`: (hub, player, the hub's link, the player's).
    fn pair(code: &str) -> (Handshake, Handshake, Link, Link) {
        let (_, key) = pairing_room(code).unwrap();
        let (mut hub, mut player) = (Handshake::new(key, HUB).unwrap(), Handshake::new(key, PLAYER).unwrap());
        assert!(hub.hello().is_none(), "the hub says nothing first");
        let hello = player.hello().unwrap();
        let (welcome, at_hub) = hub.answer(&hello).unwrap();
        let (none, at_player) = player.answer(&welcome.unwrap()).unwrap();
        assert!(none.is_none() && hub.done() && player.done() && player.hello().is_none());
        (hub, player, at_hub, at_player)
    }

    #[test]
    fn the_handshake_gives_both_ends_the_same_link() {
        let (_, _, a, b) = pair("K7QF-4MXA");
        assert_eq!(a, b);
        assert!(a.room.len() == 32 && a.room.bytes().all(|c| c.is_ascii_hexdigit()));
        let (code_room, code_key) = pairing_room("K7QF-4MXA").unwrap();
        assert!(a.room != code_room && a.key != code_key, "not the code's room or key");
        let (_, _, c, _) = pair("K7QF-4MXA");
        assert!(c.room != a.room && c.key != a.key, "fresh keys each time: another link from the same code");
        // (the formula, by hand)
        let (s1, s2) = ([1u8; 32], [2u8; 32]);
        assert_eq!(link_of(&code_key, &[7; 32], &s1, &s2), link_of(&code_key, &[7; 32], &s1, &s2));
        assert_ne!(link_of(&code_key, &[7; 32], &s1, &s2), link_of(&code_key, &[7; 32], &s2, &s1), "the roles count");
    }

    #[test]
    fn a_second_hello_after_pairing_is_ignored() {
        let (mut hub, _, _, _) = pair("K7QF-4MXA");
        let (_, key) = pairing_room("K7QF-4MXA").unwrap();
        let other = Handshake::new(key, PLAYER).unwrap();
        assert!(hub.answer(&other.hello().unwrap()).is_none(), "the code is used up");
        // (before pairing: not a key, the wrong message for this end, a low-order point - no link)
        let mut fresh = Handshake::new(key, HUB).unwrap();
        assert!(fresh.answer(&serde_json::json!({"hello": "nope"})).is_none());
        assert!(fresh.answer(&serde_json::json!({"welcome": hex(&[9; 32])})).is_none());
        assert!(fresh.answer(&serde_json::json!({"hello": hex(&[0; 32])})).is_none(), "the zero point: no secret");
        assert!(fresh.answer(&serde_json::json!({"feed": {}})).is_none());
        assert!(!fresh.done() && fresh.answer(&other.hello().unwrap()).is_some());
    }

    #[test]
    fn a_link_message_under_the_wrong_key_is_dropped() {
        let (_, _, link, _) = pair("K7QF-4MXA");
        let (_, code_key) = pairing_room("K7QF-4MXA").unwrap();
        // (as the relay hands it to the hub: from the player)
        let delivered = |key: &[u8; crypto::KEY_LEN], from: u16| {
            let f = message_frames(key, from, HUB, 1, br#"{"objects":[]}"#).remove(0);
            frames::route(&f, from).unwrap().1
        };
        assert!(open_frame(&link.key, PLAYER, &delivered(&link.key, PLAYER)).is_some());
        assert!(open_frame(&link.key, PLAYER, &delivered(&code_key, PLAYER)).is_none(), "the code's key");
        assert!(open_frame(&link.key, PLAYER, &delivered(&[3; 32], PLAYER)).is_none(), "another key");
        assert!(open_frame(&link.key, PLAYER, &delivered(&link.key, 3)).is_none(), "another sender");
    }
}
