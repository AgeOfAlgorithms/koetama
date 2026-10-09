//! The game API (PROTOCOL.md "The objects"): the JSON objects a game and Koetama exchange, the same over both
//! transports - the feed (game -> Koetama: parse_feed) and what Koetama sends (hello, speech, room, voice, translation,
//! translations_status: one line each, "type" first). The socket connector sends them as lines, the files connector
//! as numbered files (json, or a Teardown prefab whose tag holds the object's hex: object_prefab).
use crate::profile::Profile;
use kd_common::feed::{self, Feed, RuleState, Speaker};
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

/// {"type":"voice","state":"off"|"connecting"|"connected"|"unreachable"}
pub fn voice(state: &str) -> String {
    format!("{{\"type\":\"voice\",\"state\":{}}}", js(state))
}

/// {"type":"translation","id":..,"text":..}: the translation of line `id` ("": nothing to show).
pub fn translation(id: i64, text: &str) -> String {
    format!("{{\"type\":\"translation\",\"id\":{id},\"text\":{}}}", js(text))
}

/// {"type":"translations_status","translations":[{"from":..,"to":..,"state":..[,"progress":0..1]}]}: each
/// translation's state; "progress" (to 1/100) only while downloading.
pub fn translations_status(states: &[RuleState]) -> String {
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
    format!("{{\"type\":\"translations_status\",\"translations\":[{}]}}", items.join(","))
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
/// seq / session / ack / ping are read too (the socket connector sets its own seq and session).
pub fn parse_feed(v: &Value) -> Result<Feed, String> {
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
    let mut speakers = BTreeMap::new();
    match v.get("speakers") {
        None | Some(Value::Null) => {}
        Some(x) if list(x).is_some_and(|a| a.len() <= 256) => {
            let a = list(x).unwrap_or_default();
            for s in a {
                if !s.is_object() {
                    return Err("each speaker must be an object".into());
                }
                let id = s.get("id").and_then(whole).ok_or("a speaker's \"id\" must be a whole number")?;
                let test_voice = match s.get("test_voice") {
                    None | Some(Value::Null) => 0,
                    Some(x) => whole(x).filter(|n| *n > 0).ok_or("a speaker's \"test_voice\" must be a number from 1")?,
                };
                speakers.insert(
                    id,
                    Speaker {
                        src: test_voice,
                        talk: flag(s, "talking", false)?,
                        gain: num(s, "gain", 1.0)?.clamp(0.0, 1.0),
                        az: num(s, "azimuth", 0.0)?,
                        el: num(s, "elevation", 0.0)?,
                        muffle: num(s, "muffle", 0.0)?.clamp(0.0, 1.0),
                    },
                );
            }
        }
        Some(_) => return Err("\"speakers\" must be a list of at most 256 speakers".into()),
    }
    let me = v.get("me").and_then(whole); // (not a whole number: no room, as a bad one)
    let (room, key, me) = feed::voice_room(&text("room")?, &text("key")?, me);
    let region = feed::voice_region(&text("region")?, &room);
    let to = match v.get("to") {
        None | Some(Value::Null) => Vec::new(),
        Some(x) if list(x).is_some_and(|a| a.len() <= 256) => {
            let ids: Option<Vec<i64>> = list(x).unwrap_or_default().iter().map(whole).collect();
            feed::voice_to(ids.ok_or("\"to\" must be a list of player ids")?.into_iter().map(Some))
        }
        Some(_) => return Err("\"to\" must be a list of at most 256 player ids".into()),
    };
    let translations = match v.get("translations") {
        None | Some(Value::Null) => Vec::new(),
        Some(x) if list(x).is_some_and(|a| a.len() <= 16) => {
            let a = list(x).unwrap_or_default();
            let mut pairs = Vec::new();
            for t in a {
                match (t.get("from"), t.get("to")) {
                    (Some(Value::String(from)), Some(Value::String(to))) => pairs.push((from.clone(), to.clone())),
                    _ => return Err("each of \"translations\" must be {\"from\": a language code, \"to\": a language code}".into()),
                }
            }
            feed::translation_pairs(pairs)
        }
        Some(_) => return Err("\"translations\" must be a list of at most 16 {\"from\", \"to\"} pairs".into()),
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
        translations,
        to_translate,
    })
}
