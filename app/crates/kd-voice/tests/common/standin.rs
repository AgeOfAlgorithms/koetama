//! A stand-in relay on 127.0.0.1 with the rules of relay/src/frames.js: each packet to the players it names only,
//! never back to the sender; a second connection with a player's id replaces the first (close 4000) only for the same
//! owner, else it is refused (409). Shared by the tests (relay.rs, voice_zoo.rs) and the example standin_relay
//! (tools/voice_e2e.py runs it).
#![allow(dead_code)]
use kd_voice::frames;
use std::collections::HashMap;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::CloseFrame;
use tungstenite::Message;

/// What a player's connection is told by the others' threads.
pub enum Out {
    Frame(Vec<u8>),
    /// another connection with the same id came: closed with 4000 (relay/src/index.js)
    Replaced,
}

pub type Rooms = Arc<Mutex<HashMap<String, HashMap<u16, Sender<Out>>>>>;
pub type Counts = Arc<Mutex<HashMap<u16, u64>>>;

/// A presence packet's payload from a player without an id: nonce, the 16-byte plaintext, tag (a voice packet's is
/// longer: it has frames)
pub const BARE_PRESENCE: usize = 12 + 16 + 16;

/// A stand-in relay on 127.0.0.1.
#[derive(Clone)]
pub struct Mock {
    /// its ws:// address
    pub url: String,
    /// voice frames delivered to each player id (not presence from players without ids: BARE_PRESENCE)
    pub delivered: Counts,
    /// connections made / tried, per player id
    pub opened: Counts,
    pub tried: Counts,
    /// player ids it refuses (409, as a relay that keeps the first connection with an id would)
    pub refuse: Arc<Mutex<Vec<u16>>>,
    /// (room, player id) -> the owner of the connection that has it ("" none)
    pub owners: Arc<Mutex<HashMap<(String, u16), String>>>,
}

pub fn count(c: &Counts, id: u16) -> u64 {
    c.lock().unwrap().get(&id).copied().unwrap_or(0)
}

pub fn mock_relay() -> Mock {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let m = Mock {
        url: format!("ws://{}", l.local_addr().unwrap()),
        delivered: Arc::default(),
        opened: Arc::default(),
        tried: Arc::default(),
        refuse: Arc::default(),
        owners: Arc::default(),
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

/// /v1/room/<32 hex>?me=<id>[&owner=<32 hex>][&...] -> (room, id, owner: "" none)
pub fn room_and_id(path: &str) -> Option<(String, u16, String)> {
    let (room, query) = path.strip_prefix("/v1/room/")?.split_once('?')?;
    let ok = room.len() == 32 && room.bytes().all(|c| c.is_ascii_hexdigit());
    let param = |k: &str| query.split('&').find_map(|p| p.strip_prefix(k)).unwrap_or("");
    let owner = param("owner=");
    let owner = if owner.len() == 32 && owner.bytes().all(|c| c.is_ascii_hexdigit()) { owner } else { "" };
    ok.then(|| Some((room.to_string(), frames::parse_id(param("me="))?, owner.to_string())))?
}

#[allow(clippy::result_large_err)] // (tungstenite's handshake callback type)
pub fn serve(s: TcpStream, rooms: Rooms, m: Mock) {
    let mut path = String::new();
    let Ok(mut ws) = tungstenite::accept_hdr(s, |req: &Request, resp: Response| {
        path = req.uri().to_string();
        if let Some((room, me, owner)) = room_and_id(&path) {
            *m.tried.lock().unwrap().entry(me).or_default() += 1;
            // (as relay/src/frames.js onSameId: an id held by another owner is refused)
            let held = m.owners.lock().unwrap().get(&(room, me)).cloned().filter(|o| !o.is_empty() && *o != owner);
            if m.refuse.lock().unwrap().contains(&me) || held.is_some() {
                let no: ErrorResponse = tungstenite::http::Response::builder().status(409).body(Some("taken".into())).unwrap();
                return Err(no);
            }
        }
        Ok(resp)
    }) else {
        return;
    };
    let Some((room, me, owner)) = room_and_id(&path) else { return };
    m.owners.lock().unwrap().insert((room.clone(), me), owner);
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
            Err(tungstenite::Error::Io(e)) if kd_common::timed_out(&e) => {}
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
