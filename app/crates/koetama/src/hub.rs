//! The hub (PROTOCOL.md "Hub"): for a game whose script runs only on the host. The host's feed lists the other
//! players (`players`: a feed each); for each one the hub makes a join code (told to the game once per session, a
//! `join_code` object, for it to show that player) and a link through the relay in the room the code makes
//! (kd_voice::hub::Channel). It sends each player's Koetama their feed when it changes (and twice a second), and tells
//! the game what each player's Koetama sends back, with `"player": <id>` added (api::from_player), and when one joins
//! or leaves (a `player` object).
use kd_common::feed::{Feed, PlayerId};
use kd_common::Log;
use kd_games::{api, Game};
use kd_voice::hub::{new_code, Channel, HUB};
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
}

impl Hub {
    pub fn new(relay: String, log: Log) -> Hub {
        Hub { relay, log, peers: BTreeMap::new() }
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
            let code = match new_code() {
                Ok(c) => c,
                Err(e) => {
                    (self.log)(&format!("hub: {e}"));
                    continue;
                }
            };
            let Some(link) = Channel::start(self.relay.clone(), &code, HUB, self.log.clone()) else { continue };
            (self.log)(&format!("hub: player {} gets the join code {code}", id.text));
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
