//! A line's UNITS and when each was said (engine/asr.py: units, unit_times, unit_key, tidy).
//!
//! Units are runs of letters between spaces, and each CJK / kana / Hangul character on its own (Chinese and Japanese
//! have no spaces). The game splits the same way (voice.lua PC.voiceUnits): each unit's start time travels with the
//! text, so a listener who arrives (or leaves) mid-sentence gets only the words said while they were in reach.
//! Indexes are in characters (Unicode scalar values), as Python's and the game's.

/// The wide (CJK, kana, Hangul, full-width) character ranges: each such character is a unit of its own.
pub const WIDE: [(u32, u32); 10] = [
    (0x3040, 0x30FF),
    (0x31F0, 0x31FF),
    (0x1100, 0x11FF),
    (0x3000, 0x303F),
    (0x3130, 0x318F),
    (0x3400, 0x9FFF),
    (0xAC00, 0xD7AF),
    (0xF900, 0xFAFF),
    (0xFF00, 0xFFEF),
    (0x2B1A, 0x2B1A),
];

/// Lua's %s: ASCII white space only.
pub fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
}

pub fn is_wide(c: char) -> bool {
    let c = c as u32;
    WIDE.iter().any(|&(a, b)| a <= c && c <= b)
}

/// [(index of its first character, the unit)] of a text.
pub fn units(text: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut open = false; // (the last unit is a word still growing)
    for (i, ch) in text.chars().enumerate() {
        if is_space(ch) {
            open = false;
        } else if is_wide(ch) {
            out.push((i, ch.to_string()));
            open = false;
        } else if !open {
            out.push((i, ch.to_string()));
            open = true;
        } else {
            out.last_mut().unwrap().1.push(ch);
        }
    }
    out
}

/// Python's `\w`: a letter, a digit or the underscore.
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A unit compared between two passes: lower case, no punctuation.
pub fn unit_key(u: &str) -> String {
    u.to_lowercase().chars().filter(|&c| is_word_char(c)).collect()
}

/// A transcript as a chat line: single spaces, a capital letter (GigaAM writes lower case without punctuation).
pub fn tidy(text: &str) -> String {
    let joined = text.split(char::is_whitespace).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    if !joined.chars().any(is_word_char) {
        return String::new();
    }
    let mut cs = joined.chars();
    let first = cs.next().unwrap();
    if first.is_lowercase() {
        first.to_uppercase().chain(cs).collect()
    } else {
        joined
    }
}

/// Python's round(x, 2): the nearest hundredth of the exact value (ties to even).
pub fn round2(x: f64) -> f64 {
    format!("{x:.2}").parse().unwrap_or(x)
}

fn lower_eq(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

/// The start time (s, + offset) of each unit of `text`, from a model's tokens and their timestamps (sherpa-onnx:
/// result.tokens / .timestamps; SentencePiece's '▁' is a space). The text may differ a little from the tokens
/// (tidy's capital, spaces): each character is found in the tokens' text a few places ahead. No tokens (or not one
/// time per token): spread evenly over `dur`.
pub fn unit_times(text: &str, tokens: &[String], stamps: &[f32], offset: f64, dur: Option<f64>) -> Vec<f64> {
    let us = units(text);
    if us.is_empty() {
        return Vec::new();
    }
    if tokens.is_empty() || tokens.len() != stamps.len() {
        let d = dur.unwrap_or(0.0);
        let n = us.len() as f64;
        return (0..us.len()).map(|k| round2(offset + d * k as f64 / n)).collect();
    }
    let mut concat: Vec<char> = Vec::new();
    let mut ct: Vec<f64> = Vec::new();
    for (tok, &t) in tokens.iter().zip(stamps) {
        for c in tok.chars() {
            concat.push(if c == '\u{2581}' { ' ' } else { c });
            ct.push(t as f64);
        }
    }
    let mut times = Vec::new();
    let mut j = 0usize;
    let mut last = stamps[0] as f64;
    for ch in text.chars() {
        let end = concat.len().min(j + 4);
        for k in j..end {
            if lower_eq(concat[k], ch) {
                last = ct[k];
                j = k + 1;
                break;
            }
        }
        times.push(last);
    }
    let mut out: Vec<f64> = us.iter().map(|(i, _)| round2(offset + times[*i])).collect();
    for k in 1..out.len() {
        // (never earlier than the unit before)
        if out[k] < out[k - 1] {
            out[k] = out[k - 1];
        }
    }
    out
}
