//! The JSON objects Koetama sends a game, one per line (PROTOCOL.md "The socket connector"): the socket connector's
//! lines, and the files connector's message files in its "json" format (each file holds one of the same objects).
//! "type" first, then the fields in the order the protocol lists them.
use kd_common::feed::RuleState;

use crate::files::json_secs;

/// A string as JSON.
fn js(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// {"type":"hello","app":..,"version":..,"protocol":..,"features":[..]}: the first line on a connection.
pub fn hello(app: &str, version: &str, protocol: u64, features: &[&str]) -> String {
    let f: Vec<String> = features.iter().map(|x| js(x)).collect();
    format!(
        "{{\"type\":\"hello\",\"app\":{},\"version\":{},\"protocol\":{protocol},\"features\":[{}]}}",
        js(app),
        js(version),
        f.join(",")
    )
}

/// What the player said (kind 's' started talking, 'l' the words so far, 'f' the finished line):
/// {"type":"msg","kind":..,"utt":..,"text":..[,"times":[..],"ago":..]} - times and ago both or neither. Kind 'r', the
/// session's voice room (text "<room>:<key>"), is {"type":"room","room":..,"key":..}.
pub fn message(kind: char, utt: u32, text: &str, times: Option<&[f64]>, ago: Option<f64>) -> String {
    if kind == 'r' {
        let (room, key) = text.split_once(':').unwrap_or((text, ""));
        return format!("{{\"type\":\"room\",\"room\":{},\"key\":{}}}", js(room), js(key));
    }
    let mut line = format!("{{\"type\":\"msg\",\"kind\":{},\"utt\":{utt},\"text\":{}", js(&kind.to_string()), js(text));
    if let (Some(times), Some(ago)) = (times, ago) {
        let w: Vec<String> = times.iter().map(|&x| json_secs(x)).collect();
        line.push_str(&format!(",\"times\":[{}],\"ago\":{}", w.join(","), json_secs(ago.max(0.0))));
    }
    line.push('}');
    line
}

/// {"type":"voice","state":"off"|"connecting"|"connected"|"unreachable"}
pub fn voice(state: &str) -> String {
    format!("{{\"type\":\"voice\",\"state\":{}}}", js(state))
}

/// {"type":"translation","id":..,"text":..}: the translation of line `id` ("": nothing to show).
pub fn translation(id: i64, text: &str) -> String {
    format!("{{\"type\":\"translation\",\"id\":{id},\"text\":{}}}", js(text))
}

/// {"type":"translations_status","translations":[{"from":..,"to":..,"state":..[,"progress":0..1]}]}: each translation's
/// state; "progress" (to 1/100) only while downloading.
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
