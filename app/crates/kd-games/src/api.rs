//! The game API (PROTOCOL.md "The objects"): the JSON objects a game and Koetama exchange, the same over both
//! transports - the feed (game -> Koetama: parse_feed) and what Koetama sends (hello, speech, room, voice, translation,
//! translations_status: one line each, "type" first). The socket connector sends them as lines, the files connector
//! as numbered files (json, or a Teardown prefab whose tag holds the object's hex: object_prefab).
use crate::files;
use crate::profile::Profile;
use kd_common::feed::{self, Device, Effects, Feed, Out, PlayerId, RuleState, Speaker, Via};
use kd_common::paths;
use serde_json::Value;
use std::collections::BTreeMap;

/// The protocol's number (the hello's "protocol").
pub const PROTOCOL: u64 = 2;

/// A string as JSON.
fn js(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Seconds as JSON, to 1/100 s (never NaN or infinite: 0).
pub fn json_secs(x: f64) -> String {
    let r = (x * 100.0).round() / 100.0;
    if r.is_finite() {
        format!("{r}")
    } else {
        "0".into()
    }
}

// ---------------------------------------------------------------- Koetama -> game
/// What Koetama does for this game (the hello's "features"): what the profile uses, and "rooms" (real voices
/// through the relay) with "voices".
pub fn features(profile: &Profile) -> Vec<&'static str> {
    let mut f = Vec::new();
    if profile.speech {
        f.push("speech");
    }
    if profile.voices {
        f.extend(["voices", "rooms"]);
    }
    if profile.translate {
        f.push("translate");
    }
    f
}

/// {"type":"hello","app":..,"version":..,"protocol":2,"features":[..]}: first, each session.
pub fn hello(features: &[&str]) -> String {
    let f: Vec<String> = features.iter().map(|x| js(x)).collect();
    format!(
        "{{\"type\":\"hello\",\"app\":{},\"version\":{},\"protocol\":{PROTOCOL},\"features\":[{}]}}",
        js(paths::APP_NAME),
        js(paths::VERSION),
        f.join(",")
    )
}

/// What the player said, by Koetama's message kind: 's' {"type":"speech","kind":"start","utt":..}, 'l' "live" and 'f'
/// "final" with "text" (and "times" + "ago" when known: both or neither); 'r' (text "<room>:<key>") is the session's
/// voice room: room().
pub fn speech(kind: char, utt: u32, text: &str, times: Option<&[f64]>, ago: Option<f64>) -> String {
    let kind = match kind {
        'r' => {
            let (r, k) = text.split_once(':').unwrap_or((text, ""));
            return room(r, k);
        }
        's' => return format!("{{\"type\":\"speech\",\"kind\":\"start\",\"utt\":{utt}}}"),
        'l' => "live",
        _ => "final",
    };
    let mut line = format!("{{\"type\":\"speech\",\"kind\":\"{kind}\",\"utt\":{utt},\"text\":{}", js(text));
    if let (Some(times), Some(ago)) = (times, ago) {
        let w: Vec<String> = times.iter().map(|&x| json_secs(x)).collect();
        line.push_str(&format!(",\"times\":[{}],\"ago\":{}", w.join(","), json_secs(ago.max(0.0))));
    }
    line.push('}');
    line
}

/// {"type":"room","room":..,"key":..}: a new voice room for the session.
pub fn room(room: &str, key: &str) -> String {
    format!("{{\"type\":\"room\",\"room\":{},\"key\":{}}}", js(room), js(key))
}

/// {"type":"voice","state":"off"|"connecting"|"connected"|"unreachable"|"id_taken","players":[ids]}: the voice
/// chat, and the other players whose Koetama is in the room.
pub fn voice(state: &str, players: &[PlayerId]) -> String {
    let ids: Vec<String> = players.iter().map(PlayerId::json).collect();
    format!("{{\"type\":\"voice\",\"state\":{},\"players\":[{}]}}", js(state), ids.join(","))
}

/// {"type":"talking","id":..,"talking":true|false}: a player's voice started or stopped being heard (this player's:
/// being sent).
pub fn talking(id: &PlayerId, on: bool) -> String {
    format!("{{\"type\":\"talking\",\"id\":{},\"talking\":{on}}}", id.json())
}

/// {"type":"status","speech":"off"|"loading"|"ready"|"error","microphone":"closed"|"open"|"none"}
pub fn status(speech: &str, microphone: &str) -> String {
    format!("{{\"type\":\"status\",\"speech\":{},\"microphone\":{}}}", js(speech), js(microphone))
}

/// {"type":"translation","id":..,"text":..[,"from":..,"to":..]}: the translation of line `id` ("": nothing to show;
/// from / to: the translation used, with a text).
pub fn translation(id: i64, text: &str, rule: Option<(&str, &str)>) -> String {
    match rule.filter(|_| !text.is_empty()) {
        Some((f, t)) => format!(
            "{{\"type\":\"translation\",\"id\":{id},\"text\":{},\"from\":{},\"to\":{}}}",
            js(text),
            js(f),
            js(t)
        ),
        None => format!("{{\"type\":\"translation\",\"id\":{id},\"text\":{}}}", js(text)),
    }
}

/// {"type":"join_code","player":..,"code":..}: a hub's code for that player to type into their Koetama.
pub fn join_code(player: &PlayerId, code: &str) -> String {
    format!("{{\"type\":\"join_code\",\"player\":{},\"code\":{}}}", player.json(), js(code))
}

/// {"type":"player","player":..,"joined":true|false}: a hub's player's Koetama joined or left.
pub fn player(player: &PlayerId, joined: bool) -> String {
    format!("{{\"type\":\"player\",\"player\":{},\"joined\":{joined}}}", player.json())
}

/// the longest object a hub takes from one of its players (bytes)
pub const MAX_PLAYER_OBJECT: usize = 16 * 1024;

/// An object one of a hub's players' Koetama sent, as the hub tells its game, with "player" after "type" - or None.
/// That Koetama is someone else's program (or anyone with the join code): the object is read as JSON, only the kinds
/// a player's Koetama sends are taken (speech, talking, translation, translations_status, status, voice), and each is
/// made again here from its checked fields, so nothing else reaches the game (no room, no join code, no second
/// object, no other player's name).
pub fn from_player(object: &str, player: &PlayerId) -> Option<String> {
    if object.len() > MAX_PLAYER_OBJECT {
        return None;
    }
    let v: Value = serde_json::from_str(object).ok()?;
    let s = |k: &str| v.get(k).and_then(Value::as_str);
    let one_of = |k: &str, allowed: &[&'static str]| -> Option<&'static str> {
        let x = s(k)?;
        allowed.iter().find(|a| **a == x).copied()
    };
    let lang = |x: Option<&Value>| x.and_then(Value::as_str).filter(|l| LANG.is_match(l)).map(str::to_string);
    let text = |k: &str, most: usize| s(k).map(|t| t.chars().filter(|c| !c.is_control() || *c == '\n').take(most).collect::<String>());
    let clean = match s("type")? {
        "speech" => {
            let utt = u32::try_from(whole(v.get("utt")?)?).ok()?;
            let kind = match one_of("kind", &["start", "live", "final"])? {
                "start" => 's',
                "live" => 'l',
                _ => 'f',
            };
            let words = text("text", files::TEXT_MAX).unwrap_or_default();
            let times: Option<Vec<f64>> = v.get("times").and_then(Value::as_array).map(|a| {
                a.iter().take(files::TEXT_MAX).filter_map(Value::as_f64).filter(|x| x.is_finite()).collect()
            });
            let ago = v.get("ago").and_then(Value::as_f64).filter(|x| x.is_finite());
            match (times, ago) {
                (Some(t), Some(a)) => speech(kind, utt, &words, Some(&t), Some(a)),
                _ => speech(kind, utt, &words, None, None),
            }
        }
        "talking" => talking(&player_id(v.get("id")?)?, v.get("talking")?.as_bool()?),
        "translation" => {
            let id = whole(v.get("id")?)?;
            let t = text("text", files::TRANSLATION_MAX)?;
            match (lang(v.get("from")), lang(v.get("to"))) {
                (Some(f), Some(to)) => translation(id, &t, Some((&f, &to))),
                _ => translation(id, &t, None),
            }
        }
        "translations_status" => {
            // ("into": a language code, or "" - translation off; an older Koetama leaves it out)
            let into = match v.get("into") {
                None => String::new(),
                Some(Value::String(s)) if s.is_empty() => String::new(),
                x => lang(x)?,
            };
            let mut states = Vec::new();
            for r in v.get("translations")?.as_array()?.iter().take(feed::MAX_PAIRS_TOLD) {
                states.push(RuleState {
                    from: lang(r.get("from"))?,
                    to: lang(r.get("to"))?,
                    state: ["ready", "downloading", "loading", "unavailable", "error"]
                        .iter()
                        .find(|a| r.get("state").and_then(Value::as_str) == Some(**a))?
                        .to_string(),
                    progress: r.get("progress").and_then(Value::as_f64).filter(|x| x.is_finite()).unwrap_or(0.0),
                });
            }
            translations_status(&into, &states)
        }
        "status" => status(
            one_of("speech", &["off", "loading", "ready", "error"])?,
            one_of("microphone", &["closed", "open", "none"])?,
        ),
        "voice" => {
            let state = one_of("state", &["off", "connecting", "connected", "unreachable", "id_taken"])?;
            let players: Vec<PlayerId> = v
                .get("players")
                .and_then(Value::as_array)
                .map(|a| a.iter().take(MAX_PLAYERS * 2).filter_map(player_id).collect())
                .unwrap_or_default();
            voice(state, &players)
        }
        _ => return None,
    };
    // (made here: it starts {"type":"<kind>", - the player goes after that)
    let rest = clean.strip_prefix("{\"type\":")?;
    let end = rest.find(',').unwrap_or(rest.len().saturating_sub(1));
    Some(format!("{{\"type\":{},\"player\":{}{}", &rest[..end], player.json(), &rest[end..]))
}

/// a language code as Koetama writes them: "en", "zh-Hant", "yue"
static LANG: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"^[A-Za-z]{2,3}(-[A-Za-z0-9]{2,8})?$").expect("a valid regex"));

/// {"type":"translations_status","into":..,"translations":[{"from":..,"to":..,"state":..[,"progress":0..1]}]}: the
/// language the player's chat is translated into ("" off) and each pair in use, its state; "progress" (to 1/100)
/// only while downloading.
pub fn translations_status(into: &str, states: &[RuleState]) -> String {
    let items: Vec<String> = states
        .iter()
        .map(|r| {
            let progress = if r.state == "downloading" {
                format!(",\"progress\":{}", json_secs(r.progress.clamp(0.0, 1.0)))
            } else {
                String::new()
            };
            format!("{{\"from\":{},\"to\":{},\"state\":{}{progress}}}", js(&r.from), js(&r.to), js(&r.state))
        })
        .collect();
    format!("{{\"type\":\"translations_status\",\"into\":{},\"translations\":[{}]}}", js(into), items.join(","))
}

/// An object as the files connector's teardown-prefab format writes it: a prefab whose body's tag j holds the
/// object's hex (the mod Spawns it and reads the tag; a tag value cannot hold quotes or spaces).
pub fn object_prefab(object: &str) -> String {
    let hex: String = object.bytes().map(|b| format!("{b:02x}")).collect();
    format!("<prefab version=\"1.5.2\">\n\t<body tags=\"pcvx j={hex}\"/>\n</prefab>\n")
}

// ---------------------------------------------------------------- game -> Koetama
/// A whole number as JSON gives it - also written as a decimal (1.0, 1.7e12: a Lua JSON library's numbers are all
/// floats), when it is whole and fits.
pub fn whole(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().filter(|f| f.is_finite() && f.fract() == 0.0 && f.abs() < 9.0e15).map(|f| f as i64))
}

/// A list as JSON gives it - an empty object {} too (Lua has one table type: its JSON libraries write an empty one as
/// {}); None for anything else.
fn list(v: &Value) -> Option<&[Value]> {
    match v {
        Value::Array(a) => Some(a),
        Value::Object(o) if o.is_empty() => Some(&[]),
        _ => None,
    }
}

/// Hex (either case, even length) -> bytes.
fn unhex(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if !b.len().is_multiple_of(2) {
        return None;
    }
    let digit = |c: u8| (c as char).to_digit(16);
    b.chunks(2).map(|p| Some((digit(p[0])? * 16 + digit(p[1])?) as u8)).collect()
}

/// A feed as the files connector finds it: the JSON object, or its hex (Teardown's registry string).
pub fn feed_from_text(text: &str) -> Result<Feed, String> {
    let t = text.trim();
    let json = if t.starts_with('{') {
        t.to_string()
    } else {
        let bytes = unhex(t).ok_or("the feed is neither JSON nor hex")?;
        String::from_utf8(bytes).map_err(|_| "the feed's hex is not UTF-8")?
    };
    let v: Value = serde_json::from_str(&json).map_err(|e| format!("the feed is not JSON: {e}"))?;
    parse_feed(&v)
}

/// A feed object -> a Feed (PROTOCOL.md "Game -> Koetama: the feed"). Missing fields: their defaults; values of the
/// wrong kind: an error; bad room / key / me: no room; bad ids, codes and lines: skipped. The files transport's
/// seq / session / ack / ping are read too (the socket connector sets its own seq and session). A hub's `players`:
/// a Feed each (their room the host's unless their own gives one).
pub fn parse_feed(v: &Value) -> Result<Feed, String> {
    let mut feed = parse_one(v, None)?;
    match v.get("players") {
        None | Some(Value::Null) => {}
        Some(x) if list(x).is_some_and(|a| a.len() <= MAX_PLAYERS) => {
            let room = (feed.room.clone(), feed.key.clone(), feed.region.clone());
            for p in list(x).unwrap_or_default() {
                if !p.is_object() {
                    return Err("each of \"players\" must be a feed object".into());
                }
                let mut sub = parse_one(p, Some(&room))?;
                // (a player's feed names them with "id"; "me" too)
                if let Some(id) = p.get("id").and_then(player_id) {
                    sub.me = if sub.room.is_empty() { 0 } else { feed::relay_id(&sub.room, &id) };
                    sub.me_id = Some(id);
                }
                if sub.me_id.is_some() && !feed.players.iter().any(|q: &Feed| q.me_id == sub.me_id) {
                    sub.raw = player_feed(p, &feed, sub.me_id.as_ref());
                    feed.players.push(sub);
                }
            }
        }
        Some(_) => return Err(format!("\"players\" must be a list of at most {MAX_PLAYERS} feeds")),
    }
    Ok(feed)
}

/// the most players a hub's feed names
pub const MAX_PLAYERS: usize = 32;

/// A hub's player's feed as the hub sends it on (PROTOCOL.md "Hub"): theirs, with "me" their id, and the host's room,
/// key, region and session where it has none of its own.
fn player_feed(p: &Value, host: &Feed, id: Option<&PlayerId>) -> String {
    let mut m = p.as_object().cloned().unwrap_or_default();
    m.insert("type".into(), Value::from("feed"));
    if let Some(id) = id {
        let v = if id.number { id.text.parse::<i64>().map(Value::from).unwrap_or(Value::from(id.text.clone())) } else { Value::from(id.text.clone()) };
        m.insert("me".into(), v);
    }
    let own_room = ["room_seed", "room", "key"].iter().any(|k| m.get(*k).is_some_and(|v| !v.is_null()));
    if !own_room && !host.room.is_empty() {
        m.insert("room".into(), Value::from(host.room.clone()));
        m.insert("key".into(), Value::from(host.key.clone()));
    }
    if m.get("region").is_none() && !host.region.is_empty() {
        m.insert("region".into(), Value::from(host.region.clone()));
    }
    m.insert("session".into(), Value::from(host.sid));
    m.remove("players");
    Value::Object(m).to_string()
}

/// A player id as JSON gives it: a whole number, or a string of 1 to 64 characters (at most 255 bytes: it travels in
/// the voice packets) without control characters.
pub fn player_id(v: &Value) -> Option<PlayerId> {
    match v {
        Value::String(s)
            if (1..=feed::MAX_ID_CHARS).contains(&s.chars().count()) && s.len() <= 255 && s.chars().all(feed::id_char_ok) =>
        {
            Some(PlayerId::string(s))
        }
        Value::String(_) => None,
        x => whole(x).map(PlayerId::number),
    }
}

/// A 3-vector: three finite numbers.
fn vec3(v: Option<&Value>) -> Option<[f64; 3]> {
    let a = list(v?)?;
    if a.len() != 3 {
        return None;
    }
    let n: Vec<f64> = a.iter().filter_map(Value::as_f64).filter(|f| f.is_finite()).collect();
    (n.len() == 3).then(|| [n[0], n[1], n[2]])
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// a direction scaled to length 1 (None: no length)
fn unit(a: [f64; 3]) -> Option<[f64; 3]> {
    let n = dot(a, a).sqrt();
    (n > 1e-9).then(|| [a[0] / n, a[1] / n, a[2] / n])
}

/// Where this player hears from: position and the forward / right / up directions (PROTOCOL.md "Positions and
/// ranges").
#[derive(Clone, Copy, Debug)]
struct Ears {
    position: [f64; 3],
    forward: [f64; 3],
    right: [f64; 3],
    up: [f64; 3],
}

impl Ears {
    fn from(v: &Value) -> Option<Ears> {
        Some(Ears {
            position: vec3(v.get("position"))?,
            forward: unit(vec3(v.get("forward"))?)?,
            right: unit(vec3(v.get("right"))?)?,
            up: unit(vec3(v.get("up"))?)?,
        })
    }

    /// (azimuth, elevation in degrees, distance) of a point
    fn place(&self, p: [f64; 3]) -> (f64, f64, f64) {
        let d = sub(p, self.position);
        let (x, y, z) = (dot(d, self.right), dot(d, self.up), dot(d, self.forward));
        let az = x.atan2(z).to_degrees();
        let el = y.atan2((x * x + z * z).sqrt()).to_degrees();
        (az, el, dot(d, d).sqrt())
    }
}

/// [near, far]: two numbers, 0 <= near < far.
fn range(v: Option<&Value>) -> Option<(f64, f64)> {
    let a = list(v?)?;
    match (a.first().and_then(Value::as_f64), a.get(1).and_then(Value::as_f64), a.len()) {
        (Some(n), Some(f), 2) if n.is_finite() && f.is_finite() && 0.0 <= n && n < f => Some((n, f)),
        _ => None,
    }
}

/// A short text field (a name): control characters out, at most 64 characters.
fn short_text(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or("").chars().filter(|c| !c.is_control()).take(64).collect()
}

/// A number field of an object (absent or null: `def`).
fn number(o: &Value, k: &str, def: f64) -> Result<f64, String> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(def),
        Some(x) => x.as_f64().filter(|f| f.is_finite()).ok_or(format!("\"{k}\" must be a number")),
    }
}

/// `effects` (PROTOCOL.md "Sound effects") on top of `base` (a device's preset): each field given replaces the
/// preset's (0, false or null: that effect off).
fn parse_effects(v: Option<&Value>, base: Effects) -> Result<Effects, String> {
    let o = match v {
        None | Some(Value::Null) => return Ok(base),
        Some(o) if o.is_object() => o,
        Some(_) => return Err("\"effects\" must be an object".into()),
    };
    let off = |k: &str| matches!(o.get(k), Some(Value::Bool(false)) | Some(Value::Null));
    let amount = |k: &str, def: f64, lo: f64, hi: f64| -> Result<f64, String> {
        if off(k) {
            return Ok(0.0);
        }
        Ok(number(o, k, def).map_err(|_| format!("the effect \"{k}\" must be a number"))?.clamp(lo, hi))
    };
    let pair = |k: &str| -> Option<(f64, f64)> {
        let a = list(o.get(k)?)?;
        match (a.first().and_then(Value::as_f64), a.get(1).and_then(Value::as_f64), a.len()) {
            (Some(x), Some(y), 2) if x.is_finite() && y.is_finite() => Some((x, y)),
            _ => None,
        }
    };
    let band = match o.get("band") {
        None => base.band,
        Some(Value::Null) | Some(Value::Bool(false)) => None,
        Some(_) => {
            let (lo, hi) = pair("band").ok_or("the effect \"band\" must be [low Hz, high Hz]")?;
            let (lo, hi) = (lo.clamp(20.0, 20000.0), hi.clamp(20.0, 20000.0));
            if lo >= hi {
                return Err("the effect \"band\" must be [low Hz, high Hz], low below high".into());
            }
            Some((lo, hi))
        }
    };
    let echo = match o.get("echo") {
        None => base.echo,
        Some(Value::Null) | Some(Value::Bool(false)) => None,
        Some(x) if x.is_number() => {
            let d = x.as_f64().filter(|d| d.is_finite()).unwrap_or(0.0);
            (d > 0.0).then_some((d.clamp(0.02, 1.0), 0.35))
        }
        Some(_) => {
            let (d, f) = pair("echo").ok_or("the effect \"echo\" must be a delay in seconds or [delay, feedback]")?;
            (d > 0.0).then_some((d.clamp(0.02, 1.0), f.clamp(0.0, 0.9)))
        }
    };
    Ok(Effects {
        band,
        drive: amount("drive", base.drive, 0.0, 1.0)?,
        compress: amount("compress", base.compress, 0.0, 1.0)?,
        hiss: amount("static", base.hiss, 0.0, 1.0)?,
        crackle: amount("crackle", base.crackle, 0.0, 1.0)?,
        squelch: amount("squelch", base.squelch, 0.0, 1.0)?,
        horn: amount("horn", base.horn, 0.0, 1.0)?,
        lofi: amount("lofi", base.lofi, 0.0, 1.0)?,
        wobble: amount("wobble", base.wobble, 0.0, 1.0)?,
        pitch: amount("pitch", base.pitch, -12.0, 12.0)?,
        robot: amount("robot", base.robot, 0.0, 2000.0)?,
        echo,
        reverb: amount("reverb", base.reverb, 0.0, 1.0)?,
        hum: amount("hum", base.hum, 0.0, 1.0)?,
    })
}

/// A speaker's `via` (PROTOCOL.md "Devices"): each device's places as this player hears them - direction, loudness
/// by the distance and the device's range (or the `gain` given), and how much later than the nearest place each is.
fn parse_via(v: Option<&Value>, ears: Option<Ears>) -> Result<Vec<Via>, String> {
    let a = match v {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(x) => list(x).filter(|a| a.len() <= feed::MAX_VIA).ok_or("\"via\" must be a list of at most 8 devices")?,
    };
    let mut out = Vec::new();
    for d in a {
        if !d.is_object() {
            return Err("each of \"via\" must be an object".into());
        }
        let device = match d.get("device") {
            None | Some(Value::Null) => Device::Plain,
            Some(x) => x
                .as_str()
                .and_then(Device::from_name)
                .ok_or("a device's \"device\" must be \"radio\", \"loudspeaker\", \"pa\" or \"plain\"")?,
        };
        let mut places: Vec<[f64; 3]> = vec3(d.get("position")).into_iter().collect();
        match d.get("positions") {
            None | Some(Value::Null) => {}
            Some(x) => {
                let ps = list(x).filter(|p| p.len() <= feed::MAX_OUTS).ok_or("a device's \"positions\" must be a list of at most 16 positions")?;
                for p in ps {
                    places.push(vec3(Some(p)).ok_or("a device's \"positions\" must be a list of [x, y, z]")?);
                }
            }
        }
        places.truncate(feed::MAX_OUTS);
        let range = range(d.get("range")).unwrap_or(device.default_range());
        let gain_given = d.get("gain").is_some_and(|g| !g.is_null());
        let given = number(d, "gain", 1.0)?.clamp(0.0, 1.0);
        // (a horn pointing somewhere: loud in front, `back` straight behind)
        let facing = vec3(d.get("facing")).and_then(unit);
        let back = number(d, "back", device.default_back())?.clamp(0.0, 1.0);
        let outs: Vec<Out> = match ears.filter(|_| !places.is_empty()) {
            Some(e) => {
                let placed: Vec<(f64, f64, f64)> = places.iter().map(|p| e.place(*p)).collect();
                let nearest = placed.iter().map(|p| p.2).fold(f64::INFINITY, f64::min);
                placed
                    .iter()
                    .zip(&places)
                    .map(|(&(az, el, dist), p)| Out {
                        az,
                        el,
                        gain: if gain_given {
                            given
                        } else {
                            feed::falloff(dist, range) * facing.map_or(1.0, |f| feed::directivity(f, sub(e.position, *p), back))
                        },
                        delay: ((dist - nearest) / feed::SOUND_SPEED).clamp(0.0, feed::MAX_DELAY),
                    })
                    .collect()
            }
            // (no positions, or no listener: one place, where the game says)
            None => vec![Out {
                az: number(d, "azimuth", 0.0)?,
                el: number(d, "elevation", 0.0)?,
                gain: if gain_given || places.is_empty() { given } else { 0.0 },
                delay: 0.0,
            }],
        };
        out.push(Via {
            device,
            outs,
            muffle: number(d, "muffle", 0.0)?.clamp(0.0, 1.0),
            signal: number(d, "signal", 1.0)?.clamp(0.0, 1.0),
            effects: parse_effects(d.get("effects"), Effects::preset(device))?,
        });
    }
    Ok(out)
}

/// One feed (the top one, or a hub's player's: `inherit` = the host's room, key and region).
fn parse_one(v: &Value, inherit: Option<&(String, String, String)>) -> Result<Feed, String> {
    if !v.is_object() {
        return Err("the feed must be an object".into());
    }
    let num = |o: &Value, k: &str, def: f64| -> Result<f64, String> {
        match o.get(k) {
            None | Some(Value::Null) => Ok(def),
            Some(x) => x.as_f64().filter(|f| f.is_finite()).ok_or(format!("\"{k}\" must be a number")),
        }
    };
    let int = |k: &str| -> Result<i64, String> {
        match v.get(k) {
            None | Some(Value::Null) => Ok(0),
            Some(x) => whole(x).ok_or(format!("\"{k}\" must be a whole number")),
        }
    };
    let flag = |o: &Value, k: &str, def: bool| -> Result<bool, String> {
        match o.get(k) {
            None | Some(Value::Null) => Ok(def),
            Some(x) => x.as_bool().ok_or(format!("\"{k}\" must be true or false")),
        }
    };
    let text = |k: &str| -> Result<String, String> {
        match v.get(k) {
            None | Some(Value::Null) => Ok(String::new()),
            Some(Value::String(s)) => Ok(s.clone()),
            Some(_) => Err(format!("\"{k}\" must be a string")),
        }
    };
    let (mic, ptt) = match text("listen")?.as_str() {
        "" | "off" => (false, None),
        "always" => (true, None),
        "push_to_talk" => (true, Some(flag(v, "talk_key", false)?)),
        other => return Err(format!("\"listen\" must be \"off\", \"always\" or \"push_to_talk\" (not {other:?})")),
    };
    let lang = match text("lang")?.as_str() {
        "" => "en".to_string(),
        s if s.len() <= 16 && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_') => s.to_string(),
        _ => return Err("\"lang\" must be a language code (\"en\", \"ru\", ... or \"auto\")".into()),
    };
    // the room: a seed every player has, or the room and key themselves; a player's feed in a hub: the host's
    let seed = text("room_seed")?;
    let (room, key) = if !seed.is_empty() && seed.chars().count() <= 256 {
        feed::room_from_seed(&seed)
    } else if v.get("room").is_some() || v.get("key").is_some() || inherit.is_none() {
        (text("room")?, text("key")?)
    } else {
        let (r, k, _) = inherit.cloned().unwrap_or_default();
        (r, k)
    };
    let region_text = if v.get("region").is_none() { inherit.map(|i| i.2.clone()).unwrap_or_default() } else { text("region")? };
    // (not an id: no room, as a bad one; a hub's player's feed names them with "id")
    let me_id = v.get("me").filter(|m| !m.is_null()).or_else(|| inherit.and(v.get("id"))).and_then(player_id);
    let (room, key, me) = feed::voice_room(&room, &key, me_id.as_ref());
    let me_id = me_id.filter(|_| me != 0);
    let region = feed::voice_region(&region_text, &room);
    let ears = match v.get("listener") {
        None | Some(Value::Null) => None,
        Some(l) => Some(Ears::from(l).ok_or("\"listener\" must be {\"position\", \"forward\", \"right\", \"up\"}: four [x, y, z]")?),
    };
    let my_range = range(v.get("range"));
    let mut speakers: BTreeMap<i64, Speaker> = BTreeMap::new();
    let mut clashes: Vec<PlayerId> = Vec::new();
    match v.get("speakers") {
        None | Some(Value::Null) => {}
        Some(x) if list(x).is_some_and(|a| a.len() <= 256) => {
            for s in list(x).unwrap_or_default() {
                if !s.is_object() {
                    return Err("each speaker must be an object".into());
                }
                let id = s.get("id").and_then(player_id).ok_or("a speaker's \"id\" must be a whole number or a string")?;
                let test_voice = match s.get("test_voice") {
                    None | Some(Value::Null) => 0,
                    Some(x) => whole(x).filter(|n| *n > 0).ok_or("a speaker's \"test_voice\" must be a number from 1")?,
                };
                let rid = feed::relay_id(&room, &id);
                // (two players on one number in this room - or one on this player's: the later one is not heard)
                if speakers.get(&rid).is_some_and(|o| o.id != id) || (me != 0 && rid == me && me_id.as_ref() != Some(&id)) {
                    clashes.push(id);
                    continue;
                }
                let placed = match (ears, vec3(s.get("position"))) {
                    (Some(e), Some(p)) => Some(e.place(p)),
                    _ => None,
                };
                let own_range = range(s.get("range"));
                let via = parse_via(s.get("via"), ears)?;
                let gain_given = s.get("gain").is_some_and(|g| !g.is_null());
                let directed = ["position", "azimuth", "elevation"].iter().any(|k| s.get(*k).is_some_and(|x| !x.is_null()));
                let gain = if gain_given {
                    num(s, "gain", 1.0)?.clamp(0.0, 1.0)
                } else if let Some((_, _, d)) = placed {
                    // (for now: the range known here; the runtime puts in the one the player's Koetama announces)
                    feed::falloff(d, own_range.or(my_range).unwrap_or(feed::DEFAULT_RANGE))
                } else if !via.is_empty() && !directed {
                    0.0 // (heard only through their devices)
                } else {
                    1.0
                };
                let (az, el) = match placed {
                    Some((az, el, _)) => (az, el),
                    None => (num(s, "azimuth", 0.0)?, num(s, "elevation", 0.0)?),
                };
                speakers.insert(
                    rid,
                    Speaker {
                        src: test_voice,
                        talk: flag(s, "talking", false)?,
                        gain,
                        az,
                        el,
                        muffle: num(s, "muffle", 0.0)?.clamp(0.0, 1.0),
                        id,
                        name: short_text(s.get("name")),
                        distance: placed.map(|p| p.2),
                        gain_given,
                        range: own_range,
                        via,
                        effects: parse_effects(s.get("effects"), Effects::default())?,
                        volume: match s.get("volume") {
                            None | Some(Value::Null) => None,
                            Some(_) => Some(num(s, "volume", 1.0)?.clamp(0.0, 2.0)),
                        },
                    },
                );
            }
        }
        Some(_) => return Err("\"speakers\" must be a list of at most 256 speakers".into()),
    }
    let to = match v.get("to") {
        None | Some(Value::Null) => match (my_range, ears) {
            // (no "to": with a range and positions, the players within reach)
            (Some((_, far)), Some(_)) => feed::voice_to(
                speakers
                    .iter()
                    .filter(|(_, s)| s.src == 0 && s.distance.is_some_and(|d| d <= far * 1.1))
                    .map(|(&rid, _)| Some(rid))
                    .filter(|r| *r != Some(me)),
            ),
            _ => Vec::new(),
        },
        Some(x) if list(x).is_some_and(|a| a.len() <= 256) => {
            let ids: Option<Vec<PlayerId>> = list(x).unwrap_or_default().iter().map(player_id).collect();
            let ids = ids.ok_or("\"to\" must be a list of player ids")?;
            feed::voice_to(ids.iter().map(|id| feed::relay_id(&room, id)).filter(|&r| r != me).map(Some))
        }
        Some(_) => return Err("\"to\" must be a list of at most 256 player ids".into()),
    };
    // (transmit: into a device - also to these players, or to everyone in the room)
    let mut transmit_all = false;
    let to = match v.get("transmit") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => to,
        Some(Value::Bool(true)) => {
            transmit_all = true;
            to
        }
        Some(x) if list(x).is_some_and(|a| a.len() <= 256) => {
            let ids: Option<Vec<PlayerId>> = list(x).unwrap_or_default().iter().map(player_id).collect();
            let ids = ids.ok_or("\"transmit\" must be true, false or a list of player ids")?;
            let extra = ids.iter().map(|id| feed::relay_id(&room, id)).filter(|&r| r != me);
            feed::voice_to(to.iter().copied().chain(extra).map(Some))
        }
        Some(_) => return Err("\"transmit\" must be true, false or a list of at most 256 player ids".into()),
    };
    let to_translate = match v.get("to_translate") {
        None | Some(Value::Null) => Vec::new(),
        Some(x) if list(x).is_some_and(|a| a.len() <= 64) => {
            let a = list(x).unwrap_or_default();
            let mut items = Vec::new();
            for r in a {
                let id = r.get("id").and_then(whole);
                let text = match r.get("text") {
                    Some(Value::String(t)) => feed::request_text(t.as_bytes()),
                    _ => return Err("each of \"to_translate\" must be {\"id\": a whole number, \"text\": a string}".into()),
                };
                items.push((id, text));
            }
            feed::translate_requests(items)
        }
        Some(_) => return Err("\"to_translate\" must be a list of at most 64 lines".into()),
    };
    Ok(Feed {
        seq: int("seq")?,
        vol: num(v, "volume", 1.0)?.clamp(0.0, 1.0),
        sid: int("session")?,
        ack: int("ack")?,
        ping: int("ping")?,
        mic,
        ptt,
        lang,
        live: flag(v, "live", true)?,
        speakers,
        room,
        key,
        me,
        to,
        region,
        translate: flag(v, "translate", true)?,
        to_translate,
        me_id,
        name: short_text(v.get("name")),
        range: my_range,
        clashes,
        transmit_all,
        players: Vec::new(),
        raw: String::new(),
    })
}
