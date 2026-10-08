//! The game's state as Koetama sees it (engine/games/base.py): whom the player hears and how, and what the game
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
    /// the player's translation rules (version 6): (from, to) in Koetama's language codes, at most MAX_RULES; empty:
    /// translation off (translate_rules)
    #[serde(default)]
    pub rules: Vec<(String, String)>,
    /// the lines the game wants translated (version 6): (id, text), at most MAX_REQUESTS, each id once; a text that
    /// was not good UTF-8 of at most MAX_REQUEST_BYTES is "" (its reply: "") (translate_requests)
    #[serde(default)]
    pub translate: Vec<(i64, String)>,
}

/// the most translation rules a feed has
pub const MAX_RULES: usize = 2;
/// the most lines to translate in one feed
pub const MAX_REQUESTS: usize = 16;
/// the most bytes (UTF-8) of one line to translate
pub const MAX_REQUEST_BYTES: usize = 400;
/// the largest request id (15 digits: exact in a Lua number)
pub const MAX_REQUEST_ID: i64 = 999_999_999_999_999;

/// A language code as a rule names it: 1 to 16 ASCII letters, digits, - or _ (which languages have models is the
/// translator's business: a rule it has none for is reported "unavailable").
pub fn lang_code(s: &str) -> bool {
    (1..=16).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

/// The rules as the feed keeps them: good codes only, each rule once, the first MAX_RULES.
pub fn translate_rules(rules: impl IntoIterator<Item = (String, String)>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for r in rules {
        if out.len() == MAX_RULES {
            break;
        }
        if lang_code(&r.0) && lang_code(&r.1) && !out.contains(&r) {
            out.push(r);
        }
    }
    out
}

/// The feed's rules field (version 6): "ja>en,ko>en" -> [("ja", "en"), ("ko", "en")]; malformed items skipped.
pub fn parse_rules(s: &str) -> Vec<(String, String)> {
    if s.is_empty() {
        return Vec::new();
    }
    translate_rules(s.split(',').filter_map(|item| item.split_once('>').map(|(a, b)| (a.to_string(), b.to_string()))))
}

/// A request id as the feed writes it: 1 to 15 ASCII digits, at least 1.
pub fn request_id(s: &str) -> Option<i64> {
    if s.is_empty() || s.len() > 15 || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok().filter(|&n| n >= 1)
}

/// A line to translate as the feed keeps it: good UTF-8 of at most MAX_REQUEST_BYTES, else "" (answered "").
pub fn request_text(bytes: &[u8]) -> String {
    if bytes.len() > MAX_REQUEST_BYTES {
        return String::new();
    }
    String::from_utf8(bytes.to_vec()).unwrap_or_default()
}

/// The requests as the feed keeps them: good ids only (1..=MAX_REQUEST_ID), each id once (the first), the first
/// MAX_REQUESTS.
pub fn translate_requests(items: impl IntoIterator<Item = (Option<i64>, String)>) -> Vec<(i64, String)> {
    let mut out: Vec<(i64, String)> = Vec::new();
    for (id, text) in items {
        if out.len() == MAX_REQUESTS {
            break;
        }
        if let Some(id) = id.filter(|n| (1..=MAX_REQUEST_ID).contains(n)) {
            if !out.iter().any(|(i, _)| *i == id) {
                out.push((id, text));
            }
        }
    }
    out
}

/// The feed's requests field (version 6): "<id>:<hex of the UTF-8 text>;..." -> [(id, text)]. An item without a
/// good id is skipped; one with a bad hex (odd, not hex digits) or text keeps its id with "" (answered "").
pub fn parse_requests(s: &str) -> Vec<(i64, String)> {
    let items = s.split(';').filter(|i| !i.is_empty()).filter_map(|item| {
        let (id, hex) = item.split_once(':')?;
        Some((request_id(id), request_text(&unhex(hex).unwrap_or_default())))
    });
    translate_requests(items)
}

/// Hex digits (either case) -> bytes; None if odd or not hex.
fn unhex(h: &str) -> Option<Vec<u8>> {
    if !h.len().is_multiple_of(2) || !h.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).ok()).collect()
}

/// A translation rule's state, for the game (PROTOCOL.md version 6, message kind 'd').
#[derive(Clone, Debug, PartialEq)]
pub struct RuleState {
    pub from: String,
    pub to: String,
    /// "ready", "downloading", "loading", "unavailable", "error"
    pub state: String,
    /// 0..1 while downloading
    pub progress: f64,
}

impl RuleState {
    /// "ja>en=ready", "ko>en=downloading 42" (whole percent, rounded down)
    pub fn wire(&self) -> String {
        if self.state == "downloading" {
            let pct = if self.progress.is_finite() { (self.progress.clamp(0.0, 1.0) * 100.0).floor() as u32 } else { 0 };
            format!("{}>{}={} {pct}", self.from, self.to, self.state)
        } else {
            format!("{}>{}={}", self.from, self.to, self.state)
        }
    }
}

/// The rules' states as message kind 'd' carries them: each wire(), comma-separated ("" for no rules).
pub fn rules_wire(rules: &[RuleState]) -> String {
    rules.iter().map(RuleState::wire).collect::<Vec<_>>().join(",")
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
