//! A hub's link to one of its players (PROTOCOL.md "Hub"): for a game whose script runs only on the host, the host's
//! Koetama (the hub) and each other player's Koetama talk through the relay, in a room of their own made from the
//! player's join code - only the two of them have it. JSON messages, encrypted with the room's key, cut into parts:
//!   plaintext [3][message: u32 BE][part: u8][parts: u8][bytes] (at most PART bytes), the hub relay id 1, the player 2.
//! A Channel is one end: a thread holding the connection (made again after a drop), messages in and out.
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

/// The room and key a code makes (both ends the same): HMAC-SHA256 of the code's 8 characters.
pub fn pairing_room(code: &str) -> Option<(String, [u8; crypto::KEY_LEN])> {
    let c = normalize_code(code)?;
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
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

struct Shared {
    running: AtomicBool,
    heard: Mutex<Option<Instant>>,
    connected: AtomicBool,
}

/// One end of a hub's link with a player (`me`: HUB or PLAYER; the other end is the other one).
pub struct Channel {
    out: Sender<Vec<u8>>,
    inbox: Mutex<Receiver<Value>>,
    sh: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Channel {
    /// Starts the link for `code` (None: not a code). Nothing waits: the thread connects, and again after a drop.
    pub fn start(relay: String, code: &str, me: u16, log: Log) -> Option<Channel> {
        let (room, key) = pairing_room(code)?;
        let peer = if me == HUB { PLAYER } else { HUB };
        let (out_tx, out_rx) = channel::<Vec<u8>>();
        let (in_tx, in_rx) = channel::<Value>();
        let sh = Arc::new(Shared { running: AtomicBool::new(true), heard: Mutex::new(None), connected: AtomicBool::new(false) });
        let sh2 = sh.clone();
        let thread = std::thread::Builder::new()
            .name("hub link".into())
            .spawn(move || run(relay, room, key, me, peer, out_rx, in_tx, sh2, log))
            .ok();
        Some(Channel { out: out_tx, inbox: Mutex::new(in_rx), sh, thread })
    }

    /// A message to the other end (sent once connected; dropped if the link is down then).
    pub fn send(&self, msg: &Value) {
        let _ = self.out.send(msg.to_string().into_bytes());
    }

    /// The messages that came since the last call.
    pub fn received(&self) -> Vec<Value> {
        let rx = self.inbox.lock().unwrap_or_else(|e| e.into_inner());
        rx.try_iter().collect()
    }

    /// Was the other end heard from lately (ALIVE)?
    pub fn other_there(&self) -> bool {
        self.sh.heard.lock().unwrap_or_else(|e| e.into_inner()).is_some_and(|t| t.elapsed() < ALIVE)
    }

    /// Is this end in the room?
    pub fn connected(&self) -> bool {
        self.sh.connected.load(Ordering::SeqCst)
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

#[allow(clippy::too_many_arguments)]
fn run(
    relay: String,
    room: String,
    key: [u8; crypto::KEY_LEN],
    me: u16,
    peer: u16,
    out: Receiver<Vec<u8>>,
    inbox: Sender<Value>,
    sh: Arc<Shared>,
    log: Log,
) {
    let mut conn: Option<Conn> = None;
    let mut backoff = Duration::from_secs(1);
    let mut retry_at = Instant::now();
    let mut last_ping = Instant::now();
    let mut told = String::new();
    let mut next_id: u32 = 0;
    let mut asm = Assembler::default();
    let mut got = Vec::new();
    while sh.running.load(Ordering::SeqCst) {
        if conn.is_none() {
            sh.connected.store(false, Ordering::SeqCst);
            if Instant::now() < retry_at {
                std::thread::sleep(Duration::from_millis(20));
                // (messages for the other end while the link is down: dropped - the next feed carries everything)
                while out.try_recv().is_ok() {}
                continue;
            }
            match Conn::open(&relay, &room, me) {
                Ok(c) => {
                    conn = Some(c);
                    sh.connected.store(true, Ordering::SeqCst);
                    backoff = Duration::from_secs(1);
                    last_ping = Instant::now();
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
        // what the other end sent
        got.clear();
        if let Err(e) = c.poll(&mut got) {
            broken = Some(e);
        }
        for f in &got {
            let Some((from, payload)) = frames::parse_out(f) else { continue };
            if from != peer {
                continue;
            }
            let Some(plain) = crypto::open(&key, from, payload) else { continue };
            *sh.heard.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
            if let Some(msg) = asm.add(&plain) {
                if let Ok(v) = serde_json::from_slice::<Value>(&msg) {
                    let _ = inbox.send(v);
                }
            }
        }
        // what this end sends
        while broken.is_none() {
            let Ok(msg) = out.try_recv() else { break };
            next_id = next_id.wrapping_add(1);
            for p in parts(next_id, &msg) {
                let sent = crypto::seal(&key, me, &p)
                    .ok()
                    .and_then(|payload| frames::voice_frame(&[peer], &payload))
                    .map(|frame| c.send(frame));
                if let Some(Err(e)) = sent {
                    broken = Some(e);
                    break;
                }
            }
        }
        if broken.is_none() && last_ping.elapsed() >= PING_EVERY {
            last_ping = Instant::now();
            if let Err(e) = c.ping() {
                broken = Some(e);
            }
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
}
