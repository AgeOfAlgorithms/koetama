//! The game's state as Koetama sees it (engine/games/base.py): whom the player hears and how, and what the game
//! wants. A game module reads it from the game (Teardown: its savegame.xml) and hands it to the mixer and the runtime.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A player's id as the game gives it (PROTOCOL.md "Real voices"): a whole number or a string of 1 to 64 characters.
/// In a voice room each one is a 16-bit number (relay_id).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PlayerId {
    /// the id's text (a number: its decimal digits)
    pub text: String,
    /// the game gave a number (it gets a number back)
    pub number: bool,
}

impl PlayerId {
    pub fn number(n: i64) -> PlayerId {
        PlayerId { text: n.to_string(), number: true }
    }

    pub fn string(s: &str) -> PlayerId {
        PlayerId { text: s.to_string(), number: false }
    }

    /// As JSON, the way the game gave it: 7 or "7656...".
    pub fn json(&self) -> String {
        if self.number {
            self.text.clone()
        } else {
            serde_json::to_string(&self.text).unwrap_or_else(|_| "\"\"".into())
        }
    }
}

/// The most characters of a string id.
pub const MAX_ID_CHARS: usize = 64;

/// A player id's number in a voice room: the first two bytes of SHA-256("koetama id:" + room + ":" + id), 1..=MAX_ID
/// (0 maps to MAX_ID) - every Koetama in the room names every player alike; another room, other numbers.
pub fn relay_id(room: &str, id: &PlayerId) -> i64 {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(format!("koetama id:{room}:{}", id.text).as_bytes());
    let n = u16::from_be_bytes([h[0], h[1]]) as i64;
    if n == 0 {
        MAX_ID
    } else {
        n
    }
}

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
    /// the game's id for this player (PROTOCOL.md "Real voices"; the speakers map is keyed by its relay_id)
    #[serde(default)]
    pub id: PlayerId,
    /// the name to show for them ("" none)
    #[serde(default)]
    pub name: String,
    /// how far they are, when the feed gives positions (az / el are then worked out from them): the gain follows the
    /// distance and their range, unless the game gave a gain (gain_given)
    #[serde(default)]
    pub distance: Option<f64>,
    #[serde(default)]
    pub gain_given: bool,
    /// the range the feed gives for this speaker ([near, far]; a test voice's own), if any
    #[serde(default)]
    pub range: Option<(f64, f64)>,
}

/// The loudness at a distance for a voice reaching (near, far): 1 within near, ((far - d) / (far - near))^2 beyond,
/// 0 from far on (PROTOCOL.md "Positions and ranges").
pub fn falloff(distance: f64, (near, far): (f64, f64)) -> f64 {
    if !distance.is_finite() || distance >= far {
        return 0.0;
    }
    if distance <= near || far <= near {
        return 1.0;
    }
    let left = (far - distance) / (far - near);
    left * left
}

/// the range a voice reaches when nothing says otherwise
pub const DEFAULT_RANGE: (f64, f64) = (10.0, 30.0);

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
    /// the session's voice room (PROTOCOL.md "Real voices"): 32 lower-case hex digits; "" = no room (no real voices sent or heard).
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
    /// the player's translations (PROTOCOL.md "Translation"): (from, to) in Koetama's language codes, at most MAX_TRANSLATIONS; empty:
    /// translation off (translation_pairs)
    #[serde(default)]
    pub translations: Vec<(String, String)>,
    /// the lines the game wants translated (to_translate): (id, text), at most MAX_REQUESTS, each id once; a text that
    /// was not good UTF-8 of at most MAX_REQUEST_BYTES is "" (its reply: "") (translate_requests)
    #[serde(default)]
    pub to_translate: Vec<(i64, String)>,
    /// this player's id as the game gives it (`me`; None: none given - no room); `me` is its relay_id
    #[serde(default)]
    pub me_id: Option<PlayerId>,
    /// this player's name
    #[serde(default)]
    pub name: String,
    /// how far this player's voice reaches now, [near, far] in the game's units (PROTOCOL.md "Positions and ranges")
    #[serde(default)]
    pub range: Option<(f64, f64)>,
    /// the speakers' ids that clash with this player's (or each other's) number in this room
    #[serde(default)]
    pub clashes: Vec<PlayerId>,
    /// a hub's other players: a feed each, its me_id the player (PROTOCOL.md "Hub")
    #[serde(default)]
    pub players: Vec<Feed>,
    /// a hub's player's feed as JSON, as the hub sends it to that player's Koetama (the host's room, region and
    /// session put in where it has none of its own, "me" its id); "" for any other feed
    #[serde(skip)]
    pub raw: String,
}

/// the most translation rules a feed has
pub const MAX_TRANSLATIONS: usize = 2;
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

/// The rules as the feed keeps them: good codes only, each rule once, the first MAX_TRANSLATIONS.
pub fn translation_pairs(rules: impl IntoIterator<Item = (String, String)>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for r in rules {
        if out.len() == MAX_TRANSLATIONS {
            break;
        }
        if lang_code(&r.0) && lang_code(&r.1) && !out.contains(&r) {
            out.push(r);
        }
    }
    out
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

/// A translation's state, for the game (PROTOCOL.md "Translation": translations_status).
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

/// The states as one line ("ja>en=ready,ko>en=downloading 42"; "" for none): the status line, and what tells a change.
pub fn translations_wire(rules: &[RuleState]) -> String {
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

/// (room, key, me) as the feed keeps them: all three good (a 32-hex room, a 64-hex key, an id) or none of them
/// ("", "", 0): a half-made room is no room.
pub fn voice_room(room: &str, key: &str, me: Option<&PlayerId>) -> (String, String, i64) {
    match me {
        Some(me) if lower_hex(room, 32) && lower_hex(key, 64) => (room.into(), key.into(), relay_id(room, me)),
        _ => (String::new(), String::new(), 0),
    }
}

/// HMAC-SHA256 (RFC 2104).
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let pad = |b: u8| k.iter().map(|&x| x ^ b).collect::<Vec<u8>>();
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(msg).finalize();
    Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize().into()
}

/// The room and key every player's Koetama makes from the session's room_seed (PROTOCOL.md "Real voices"): the
/// first 32 hex digits of HMAC-SHA256(seed, "koetama room"), and HMAC-SHA256(seed, "koetama key") in hex.
pub fn room_from_seed(seed: &str) -> (String, String) {
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let room = hex(&hmac_sha256(seed.as_bytes(), b"koetama room"));
    (room[..32].to_string(), hex(&hmac_sha256(seed.as_bytes(), b"koetama key")))
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
    /// The feed names a voice room.
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
