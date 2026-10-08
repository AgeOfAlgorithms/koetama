//! The game's state as Kotodama sees it (engine/games/base.py): whom the player hears and how, and what the game
//! wants. A game module reads it from the game (Teardown: its savegame.xml) and hands it to the mixer and the runtime.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One voice to play.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Speaker {
    /// which recorded voice (the game's test speakers: 1 whisperer, 2 speaker, 3 yeller)
    pub src: i64,
    pub talk: bool,
    /// 0..1
    pub gain: f64,
    /// degrees from where the camera looks: 0 ahead, 90 right, +-180 behind
    pub az: f64,
    /// degrees up
    pub el: f64,
    /// 0 clear .. 1 behind walls
    pub muffle: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Feed {
    pub seq: i64,
    /// the player's voice volume in the game, 0..1
    pub vol: f64,
    /// the game's session (a new level: a new one)
    pub sid: i64,
    /// the last message number the game has read
    pub ack: i64,
    /// the game asks "are you there": answered with a file
    pub ping: i64,
    /// the player's speech should be heard and written
    pub mic: bool,
    /// push to talk: Some(held) - only what the player says while the key is held is heard (the microphone stays
    /// open, so the start of a line is not lost); None - the speech detector decides (always on)
    #[serde(default)]
    pub ptt: Option<bool>,
    /// the language the player speaks: "en", "ru", ... or "auto"
    pub lang: String,
    /// live words while they talk (false: only the finished line, less CPU)
    pub live: bool,
    pub speakers: BTreeMap<i64, Speaker>,
    /// the session's voice room (version 5): 32 lower-case hex digits; "" = no room (no real voices sent or heard).
    /// Only kept with a good key and id (voice_room)
    #[serde(default)]
    pub room: String,
    /// the room's key: 64 lower-case hex digits (32 bytes); "" when room is
    #[serde(default)]
    pub key: String,
    /// this player's id in the game session, 1..=MAX_ID; 0 when there is no room
    #[serde(default)]
    pub me: i64,
    /// the players who should get this player's voice now (ids 1..=MAX_ID, each once, at most MAX_TO); empty: nobody
    #[serde(default)]
    pub to: Vec<i64>,
    /// where the room should live (a relay region, REGIONS); "": wherever the relay puts it (near the first player)
    #[serde(default)]
    pub region: String,
}

/// the regions a voice room can be asked to live in (Cloudflare's Durable Object location hints; the relay's REGIONS)
pub const REGIONS: [&str; 11] = ["wnam", "enam", "sam", "weur", "eeur", "apac", "apac-ne", "apac-se", "oc", "afr", "me"];

/// A region as the feed writes it: one of REGIONS, else "" ("auto", unknown, or no room).
pub fn voice_region(s: &str, room: &str) -> String {
    if !room.is_empty() && REGIONS.contains(&s) {
        s.into()
    } else {
        String::new()
    }
}

/// the largest player id in a voice room (the relay's u16)
pub const MAX_ID: i64 = 65535;
/// the most players one voice packet goes to (the relay's limit)
pub const MAX_TO: usize = 64;

/// n lower-case hex digits (exactly)
fn lower_hex(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// A player id as the feed writes it: 1 to 5 ASCII digits, 1..=MAX_ID.
pub fn player_id(s: &str) -> Option<i64> {
    if s.is_empty() || s.len() > 5 || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok().filter(|&n| (1..=MAX_ID).contains(&n))
}

/// (room, key, me) as the feed keeps them: all three good (a 32-hex room, a 64-hex key, an id) or none of them
/// ("", "", 0): a half-made room is no room.
pub fn voice_room(room: &str, key: &str, me: Option<i64>) -> (String, String, i64) {
    match me {
        Some(me) if lower_hex(room, 32) && lower_hex(key, 64) && (1..=MAX_ID).contains(&me) => (room.into(), key.into(), me),
        _ => (String::new(), String::new(), 0),
    }
}

/// The ids in `to` as the feed keeps them: good ones (1..=MAX_ID) in order, each once, the first MAX_TO; others skipped.
pub fn voice_to(ids: impl IntoIterator<Item = Option<i64>>) -> Vec<i64> {
    let mut out = Vec::new();
    for id in ids.into_iter().flatten() {
        if (1..=MAX_ID).contains(&id) && !out.contains(&id) && out.len() < MAX_TO {
            out.push(id);
        }
    }
    out
}

impl Feed {
    /// The feed names a voice room (version 5).
    pub fn has_room(&self) -> bool {
        !self.room.is_empty()
    }
}

/// Where a game module hands each new feed: the voice mixer (kd_audio::MixerSink), or a test's recorder.
pub trait FeedSink: Send + Sync {
    fn set_feed(&self, feed: Feed);
    /// a feed came at most kd_audio::STALE s ago (the game is running the mod)
    fn fresh(&self) -> bool;
}
