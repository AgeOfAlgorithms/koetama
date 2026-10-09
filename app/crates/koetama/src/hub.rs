//! The hub (PROTOCOL.md "Hub"): for a game whose script runs only on the host. The host's feed lists the other
//! players (`players`: a feed each); for each one the hub makes a join code (told to the game once per session, a
//! `join_code` object, for it to show that player) and a link through the relay (kd_voice::hub::Channel: a key
//! exchange in the room the code makes, then a room of the link's own - the code works once). It sends each player's
//! Koetama their feed over the link when it changes (and twice a second), and tells the game what each player's
//! Koetama sends back, with `"player": <id>` added (api::from_player), and when one joins or leaves (a `player`
//! object). A link silent for REPAIR_AFTER (their Koetama closed) is dropped and the player gets a new code.
use kd_common::feed::{Feed, PlayerId};
use kd_common::Log;
use kd_games::{api, Game};
use kd_voice::hub::{new_code, Channel, HUB, REPAIR_AFTER};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// a player's feed is sent again this often even when it did not change (their Koetama counts a game as gone after
/// 1.5 s without one)
const RESEND: Duration = Duration::from_millis(500);

struct Peer {
    id: PlayerId,
    code: String,
    link: Channel,
    /// their feed as the game gave it last, and what was sent, when
    raw: String,
    sent: String,
    sent_at: Instant,
    /// their Koetama is there (as last told to the game)
    joined: bool,
    /// the game session the code was told in
    told_in: Option<i64>,
}

pub struct Hub {
    relay: String,
    log: Log,
    /// by the player's id text
    peers: BTreeMap<String, Peer>,
    /// a paired link silent this long: a new code (REPAIR_AFTER; shorter in the tests)
    repair_after: Duration,
}

/// A new code and a link for it (None: no randomness - logged).
fn new_link(relay: &str, log: &Log) -> Option<(String, Channel)> {
    let code = match new_code() {
        Ok(c) => c,
        Err(e) => {
            log(&format!("hub: {e}"));
            return None;
        }
    };
    let link = Channel::start(relay.to_string(), &code, HUB, log.clone())?;
    Some((code, link))
}

impl Hub {
    pub fn new(relay: String, log: Log) -> Hub {
        Hub { relay, log, peers: BTreeMap::new(), repair_after: REPAIR_AFTER }
    }

    /// The players the host's feed lists now: a link (and a code) for each new one, their feed kept; a player no
    /// longer listed is let go.
    pub fn set_players(&mut self, players: &[Feed]) {
        let listed: Vec<&str> = players.iter().filter_map(|p| p.me_id.as_ref().map(|i| i.text.as_str())).collect();
        self.peers.retain(|k, _| listed.contains(&k.as_str()));
        for p in players {
            let Some(id) = p.me_id.clone() else { continue };
            if let Some(peer) = self.peers.get_mut(&id.text) {
                peer.raw = p.raw.clone();
                continue;
            }
            let Some((code, link)) = new_link(&self.relay, &self.log) else { continue };
            // (not the code itself: a log gets shared, and the code is the key to that player's link)
            (self.log)(&format!("hub: player {} has a join code (shown to them by the game)", id.text));
            self.peers.insert(
                id.text.clone(),
                Peer {
                    id,
                    code,
                    link,
                    raw: p.raw.clone(),
                    sent: String::new(),
                    sent_at: Instant::now() - RESEND,
                    joined: false,
                    told_in: None,
                },
            );
        }
    }

    /// The players' feeds out, what their Koetamas sent in (to the game), the codes and joins told. `sid`: the game's
    /// session now (a code is told again in each new one).
    pub fn pump(&mut self, game: &dyn Game, sid: Option<i64>) {
        for peer in self.peers.values_mut() {
            // (paired, then silent: their Koetama closed - a new code for them; the old link is dropped, its code
            //  was used up already)
            if peer.link.quiet().is_some_and(|q| q >= self.repair_after) {
                if let Some((code, link)) = new_link(&self.relay, &self.log) {
                    (self.log)(&format!("hub: player {}'s Koetama is gone - a new join code for them", peer.id.text));
                    peer.code = code;
                    peer.link = link;
                    peer.told_in = None;
                    peer.sent.clear();
                }
            }
            if sid.is_some() && peer.told_in != sid && game.send_object(api::join_code(&peer.id, &peer.code)) {
                peer.told_in = sid;
            }
            if peer.link.connected() && !peer.raw.is_empty() && (peer.raw != peer.sent || peer.sent_at.elapsed() >= RESEND) {
                if let Ok(feed) = serde_json::from_str::<Value>(&peer.raw) {
                    peer.link.send(&serde_json::json!({ "feed": feed }));
                    peer.sent = peer.raw.clone();
                    peer.sent_at = Instant::now();
                }
            }
            for msg in peer.link.received() {
                let Some(objects) = msg.get("objects").and_then(Value::as_array) else { continue };
                for o in objects.iter().filter_map(Value::as_str) {
                    if let Some(tagged) = api::from_player(o, &peer.id) {
                        game.send_object(tagged);
                    }
                }
            }
            let there = peer.link.other_there();
            if there != peer.joined && game.send_object(api::player(&peer.id, there)) {
                (self.log)(&format!("hub: player {} {}", peer.id.text, if there { "joined" } else { "left" }));
                peer.joined = there;
            }
        }
    }

    /// (player id, code, joined) for the window.
    pub fn players(&self) -> Vec<(String, String, bool)> {
        self.peers.values().map(|p| (p.id.text.clone(), p.code.clone(), p.joined)).collect()
    }
}

#[cfg(test)]
mod tests {
    //! The hub and a player's Koetama through a stand-in relay (the rules of relay/src/frames.js: each packet to the
    //! players it names, in its room): they pair; the code is then used up (a third party with it gets nothing, and
    //! sends nothing to the host's game); a dropped connection comes back over the link without a new code; a player
    //! whose Koetama is gone gets a new code.
    use super::*;
    use kd_voice::hub::{message_frames, pairing_room, Handshake, PLAYER};
    use kd_voice::relay::Conn;
    use serde_json::json;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc::{channel, Sender};
    use std::sync::{Arc, Mutex};
    use tungstenite::handshake::server::{Request, Response};
    use tungstenite::Message;

    enum Out {
        Frame(Vec<u8>),
        /// the connection drops (no close: the TCP connection just goes)
        Cut,
    }

    type Rooms = Arc<Mutex<HashMap<String, HashMap<u16, Sender<Out>>>>>;

    #[derive(Clone)]
    struct Mock {
        url: String,
        rooms: Rooms,
        /// each connection made: (room, id)
        opened: Arc<Mutex<Vec<(String, u16)>>>,
    }

    impl Mock {
        fn start() -> Mock {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let m = Mock { url: format!("ws://{}", l.local_addr().unwrap()), rooms: Arc::default(), opened: Arc::default() };
            let m2 = m.clone();
            std::thread::spawn(move || {
                for s in l.incoming().flatten() {
                    let m = m2.clone();
                    std::thread::spawn(move || m.serve(s));
                }
            });
            m
        }

        /// Cuts every connection with this id (a network drop).
        fn cut(&self, id: u16) {
            for r in self.rooms.lock().unwrap().values() {
                if let Some(t) = r.get(&id) {
                    let _ = t.send(Out::Cut);
                }
            }
        }

        fn opened(&self, room: &str, id: u16) -> usize {
            self.opened.lock().unwrap().iter().filter(|(r, i)| r == room && *i == id).count()
        }

        #[allow(clippy::result_large_err)] // (tungstenite's handshake callback type)
        fn serve(&self, s: TcpStream) {
            let mut path = String::new();
            let Ok(mut ws) = tungstenite::accept_hdr(s, |req: &Request, resp: Response| {
                path = req.uri().to_string();
                Ok(resp)
            }) else {
                return;
            };
            let Some((room, me)) = path.strip_prefix("/v1/room/").and_then(|p| p.split_once("?me=")) else { return };
            // (?me=<id>, then maybe &region=.. / &owner=..)
            let (room, me) = (room.to_string(), me.split('&').next().unwrap().parse::<u16>().unwrap());
            self.opened.lock().unwrap().push((room.clone(), me));
            let (tx, rx) = channel::<Out>();
            if let Some(old) = self.rooms.lock().unwrap().entry(room.clone()).or_default().insert(me, tx) {
                let _ = old.send(Out::Cut);
            }
            ws.get_ref().set_read_timeout(Some(Duration::from_millis(2))).unwrap();
            loop {
                match ws.read() {
                    Ok(Message::Binary(b)) => {
                        if let Some((to, out)) = kd_voice::frames::route(&b, me) {
                            let r = self.rooms.lock().unwrap();
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
                    Ok(Message::Close(_)) => return,
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(e)) if kd_common::timed_out(&e) => {}
                    Err(_) => return,
                }
                while let Ok(out) = rx.try_recv() {
                    match out {
                        Out::Frame(f) => {
                            if ws.send(Message::binary(f)).is_err() {
                                return;
                            }
                        }
                        Out::Cut => return,
                    }
                }
            }
        }
    }

    /// The host's game: every object it is told.
    #[derive(Default)]
    struct HostGame {
        got: Mutex<Vec<Value>>,
    }

    impl Game for HostGame {
        fn id(&self) -> &'static str {
            "test-host"
        }
        fn name(&self) -> &'static str {
            "test"
        }
        fn needs(&self) -> &'static str {
            ""
        }
        fn send_object(&self, object: String) -> bool {
            self.got.lock().unwrap().push(serde_json::from_str(&object).unwrap());
            true
        }
        fn feed(&self) -> Option<Feed> {
            None
        }
        fn connected(&self) -> bool {
            true
        }
    }

    impl HostGame {
        fn of_type(&self, t: &str) -> Vec<Value> {
            self.got.lock().unwrap().iter().filter(|o| o["type"] == t).cloned().collect()
        }
    }

    fn quiet_log() -> Log {
        Arc::new(|_: &str| {})
    }

    /// The host's feed: bob is the one other player.
    fn players() -> Vec<Feed> {
        let f = json!({"type": "feed", "session": 1, "me": "host", "room": "0123456789abcdef0123456789abcdef",
                       "key": "11".repeat(32), "players": [{"id": "bob", "listen": "always", "lang": "en"}]});
        api::parse_feed(&f).unwrap().players
    }

    /// Pumps the hub (and runs `each` with every round) until `done`, at most `secs`; true if it was.
    fn pump_until(hub: &mut Hub, game: &HostGame, secs: f64, mut each: impl FnMut(), mut done: impl FnMut(&Hub) -> bool) -> bool {
        let t0 = Instant::now();
        while t0.elapsed().as_secs_f64() < secs {
            hub.pump(game, Some(1));
            each();
            if done(hub) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// A player's Koetama, as far as the link goes: says it is here every 0.3 s, keeps the feeds it gets.
    struct Bob {
        link: Channel,
        beat: Instant,
        feeds: Vec<Value>,
    }

    impl Bob {
        fn new(relay: &str, code: &str) -> Bob {
            Bob { link: Channel::start(relay.into(), code, PLAYER, quiet_log()).unwrap(), beat: Instant::now(), feeds: Vec::new() }
        }
        fn step(&mut self) {
            if self.link.connected() && self.beat.elapsed() > Duration::from_millis(300) {
                self.beat = Instant::now();
                self.link.send(&json!({ "objects": [] }));
            }
            self.feeds.extend(self.link.received().into_iter().filter_map(|m| m.get("feed").cloned()));
        }
    }

    #[test]
    fn a_join_code_works_once() {
        let mock = Mock::start();
        let game = HostGame::default();
        let mut hub = Hub::new(mock.url.clone(), quiet_log());
        hub.set_players(&players());
        assert!(pump_until(&mut hub, &game, 2.0, || {}, |_| !game.of_type("join_code").is_empty()));
        let code = game.of_type("join_code")[0]["code"].as_str().unwrap().to_string();
        let (code_room, code_key) = pairing_room(&code).unwrap();

        // ---- bob pairs: the game hears "joined" only once the link is up, and bob gets his feed over it
        let bob = RefCell::new(Bob::new(&mock.url, &code));
        let t0 = Instant::now();
        assert!(pump_until(&mut hub, &game, 10.0, || bob.borrow_mut().step(), |_| {
            let b = bob.borrow();
            !b.feeds.is_empty() && !game.of_type("player").is_empty()
        }));
        eprintln!("paired, the feed and `joined` in {:.0} ms", t0.elapsed().as_secs_f64() * 1000.0);
        assert!(bob.borrow().link.paired() && bob.borrow().link.connected());
        assert_eq!(bob.borrow().feeds[0]["listen"], "always");
        assert_eq!(game.of_type("player"), vec![json!({"type": "player", "player": "bob", "joined": true})]);
        assert_eq!(hub.players(), vec![("bob".into(), code.clone(), true)]);
        assert_eq!(mock.opened(&code_room, HUB), 1, "the hub was in the code's room once");

        // ---- a third party with the code: as a player (a hello, then objects, by hand; then a Koetama), as the hub -
        //      nothing either way
        let mut raw = Conn::open(&mock.url, &code_room, PLAYER).unwrap();
        let hello = Handshake::new(code_key, PLAYER).unwrap().hello().unwrap();
        let lie = json!({ "objects": [api::speech('f', 9, "the thief speaking", None, None)] });
        let mut raw_got = Vec::new();
        pump_until(&mut hub, &game, 1.5, || {
            bob.borrow_mut().step();
            for (i, m) in [&hello, &lie].iter().enumerate() {
                for f in message_frames(&code_key, PLAYER, HUB, 100 + i as u32, m.to_string().as_bytes()) {
                    raw.send(f).unwrap();
                }
            }
            raw.poll(&mut raw_got).unwrap();
        }, |_| false);
        raw.close();
        assert!(raw_got.is_empty(), "nothing at all came back in the code's room");
        // (one at a time: two with the code would pair with each other - not with this session)
        let thief = RefCell::new(Bob::new(&mock.url, &code));
        pump_until(&mut hub, &game, 2.5, || {
            bob.borrow_mut().step();
            thief.borrow_mut().step();
        }, |_| false);
        {
            let t = thief.borrow();
            assert!(!t.link.paired() && t.feeds.is_empty() && !t.link.connected(), "the thief: no link, no feed");
        }
        drop(thief);
        let fake_hub = Channel::start(mock.url.clone(), &code, HUB, quiet_log()).unwrap();
        pump_until(&mut hub, &game, 2.0, || bob.borrow_mut().step(), |_| false);
        assert!(fake_hub.received().is_empty() && !fake_hub.paired(), "as the hub: nothing from bob");
        assert!(game.of_type("speech").is_empty(), "nothing from the thief reached the host's game");
        assert_eq!(mock.opened(&code_room, HUB), 2, "(the fake hub) - the real one never came back to the code's room");
        assert_eq!(game.of_type("join_code").len(), 1);
        assert!(hub.players()[0].2, "bob still there");
        drop(fake_hub);

        // ---- bob's connection drops: back over the link, no new code
        let before = bob.borrow().feeds.len();
        mock.cut(PLAYER);
        let t0 = Instant::now();
        assert!(pump_until(&mut hub, &game, 10.0, || bob.borrow_mut().step(), |_| {
            let b = bob.borrow();
            b.link.connected() && b.feeds.len() > before + 2
        }));
        eprintln!("back after a drop in {:.0} ms", t0.elapsed().as_secs_f64() * 1000.0);
        bob.borrow().link.send(&json!({ "objects": [api::speech('f', 1, "bob again", None, None)] }));
        assert!(pump_until(&mut hub, &game, 5.0, || bob.borrow_mut().step(), |_| !game.of_type("speech").is_empty()));
        assert_eq!(game.of_type("speech")[0]["player"], "bob");
        assert_eq!(game.of_type("join_code").len(), 1, "no new code");
        assert_eq!(hub.players()[0].1, code);

        // ---- bob's Koetama closes: after the silence, the player left and a new code (which works)
        drop(bob);
        hub.repair_after = Duration::from_secs(2);
        assert!(pump_until(&mut hub, &game, 10.0, || {}, |_| game.of_type("join_code").len() == 2));
        let code2 = game.of_type("join_code")[1]["code"].as_str().unwrap().to_string();
        assert_ne!(code2, code);
        assert_eq!(game.of_type("player").last().unwrap()["joined"], false);
        let bob2 = RefCell::new(Bob::new(&mock.url, &code2));
        assert!(pump_until(&mut hub, &game, 10.0, || bob2.borrow_mut().step(), |h| {
            h.players()[0].2 && !bob2.borrow().feeds.is_empty()
        }));
        let old = RefCell::new(Bob::new(&mock.url, &code));
        pump_until(&mut hub, &game, 2.0, || old.borrow_mut().step(), |_| false);
        assert!(!old.borrow().link.paired(), "the first code stays used up");
    }
}
