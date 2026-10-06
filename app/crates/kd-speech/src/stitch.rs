//! Mixed languages ("auto": the language decided per stretch): asr.py quiet_point, segments, transcribe_mixed.
use crate::models::Models;
use crate::{roll_model, LID_HOP, LID_MIN, LID_QUIET, LID_SURE, LID_SWITCH, LID_WIN, MIXED_LANGS, RATE};
use std::collections::HashMap;
use std::time::Instant;

const SR: f64 = RATE as f64;

/// The language detector segments() asks: a stretch of audio -> probabilities over MIXED_LANGS.
pub type Probs<'a> = dyn FnMut(&[f32]) -> Result<Vec<f64>, String> + 'a;

/// mean(x²), empty: None (numpy: nan, which never compares true)
fn energy(x: &[f32]) -> Option<f64> {
    if x.is_empty() {
        return None;
    }
    Some(x.iter().map(|&v| v as f64 * v as f64).sum::<f64>() / x.len() as f64)
}

/// x[a:b] as Python slices it (clipped to the audio)
fn slice(x: &[f32], a: usize, b: usize) -> &[f32] {
    let a = a.min(x.len());
    &x[a..b.clamp(a, x.len())]
}

/// The first index of the largest value (numpy's argmax).
fn argmax(v: &[f64]) -> usize {
    let mut best = 0;
    for (i, &p) in v.iter().enumerate() {
        if p > v[best] {
            best = i;
        }
    }
    best
}

/// The quietest 20 ms within +-span s of t (asr.py's default span: 0.3): a cut there rarely splits a word.
pub fn quiet_point(x: &[f32], t: f64, span: f64) -> f64 {
    let a = ((t - span).max(0.0) * SR) as i64;
    let b = ((x.len() as f64 / SR).min(t + span) * SR) as i64;
    let (mut best, mut bt) = (1e9, t);
    let mut k = a;
    while k < (a + 1).max(b - 320) {
        if let Some(e) = energy(slice(x, k as usize, k as usize + 320)) {
            if e < best {
                best = e;
                bt = (k + 160) as f64 / SR;
            }
        }
        k += 80;
    }
    bt
}

/// [(lang, start s, end s)] of a line that may change language. The detector on LID_WIN s windows every LID_HOP s;
/// each LID_HOP s frame scores each language by the mean log-probability of the windows covering it. Quiet frames
/// (LID_QUIET) and windows mostly quiet do not vote: before this, the quiet around a word (and the helper's 1 s of
/// audio from before the speech) got a language of its own - "Okay." came out French + Russian. The frames' languages
/// are then the best path where each change costs LID_SWITCH (Viterbi): a line keeps its language unless the evidence
/// for a change is strong. Stretches under LID_MIN s join a neighbour; each cut goes to the quietest point near the
/// change. Less than LID_WIN + 2 hops of SPEECH (one word, a short call): one stretch, in the detector's language if
/// it is LID_SURE, else `fallback` (the helper passes the language of the player's line before).
/// (Measured with SpeechBrain, bench/lidtune.py: single-language lines and one-word lines better than the
/// plain 3-frame vote and than AmberNet had them; mixed lines 11 % words wrong against AmberNet's 7.)
/// (Cut by LANGUAGE, not by the model that writes it: Parakeet decides one language per clip, so English and German
/// handed to it as one piece lose one of them - grouping by model tried 2026-10-05: 17 % words wrong against 7 %.)
/// cache: {window index k (its start / LID_HOP): probabilities} kept by a growing line - its earlier windows never
/// change. probs: the detector (Models::lid_probs; tests: a stand-in).
pub fn segments(
    x: &[f32],
    fallback: &str,
    mut cache: Option<&mut HashMap<i64, Vec<f64>>>,
    probs: &mut Probs,
) -> Result<Vec<(String, f64, f64)>, String> {
    let dur = x.len() as f64 / SR;
    let nl = MIXED_LANGS.len();
    let hop = (LID_HOP * SR) as usize;
    let n = x.len().div_ceil(hop).max(1);
    let db: Vec<f64> = (0..n)
        .map(|i| match energy(slice(x, i * hop, (i + 1) * hop)) {
            Some(e) => 10.0 * (e + 1e-12).log10(),
            None => -120.0,
        })
        .collect();
    let top = db.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let floor = (top - LID_QUIET).max(-50.0);
    let voiced: Vec<bool> = db.iter().map(|&d| d > floor).collect();
    let (Some(v0), Some(v1)) = (voiced.iter().position(|&v| v), voiced.iter().rposition(|&v| v)) else {
        return Ok(vec![(fallback.to_string(), 0.0, dur)]);
    };
    if (v1 + 1 - v0) as f64 * LID_HOP < LID_WIN + LID_HOP * 2.0 {
        // (short speech: one stretch)
        let p = probs(slice(x, v0 * hop, (v1 + 1) * hop))?;
        let best = argmax(&p);
        let lang = if p[best] > LID_SURE { MIXED_LANGS[best] } else { fallback };
        return Ok(vec![(lang.to_string(), 0.0, dur)]);
    }
    let mut score = vec![vec![0.0f64; nl]; n];
    let mut cnt = vec![0usize; n];
    let mut k: i64 = 0;
    loop {
        let t = k as f64 * LID_HOP;
        if t + LID_WIN > dur + 1e-6 {
            break;
        }
        let f0 = (t / LID_HOP).round() as usize;
        let f1 = n.min(((t + LID_WIN) / LID_HOP).ceil() as usize);
        let share = if f1 > f0 { voiced[f0..f1].iter().filter(|&&v| v).count() as f64 / (f1 - f0) as f64 } else { f64::NAN };
        if share >= 0.3 {
            // (a mostly quiet window does not vote)
            let p = match cache.as_deref().and_then(|c| c.get(&k)) {
                Some(p) => p.clone(),
                None => {
                    let p = probs(slice(x, (t * SR) as usize, ((t + LID_WIN) * SR) as usize))?;
                    if let Some(c) = cache.as_deref_mut() {
                        c.insert(k, p.clone());
                    }
                    p
                }
            };
            if p.len() != nl {
                return Err(format!("the language detector gave {} probabilities, not {nl}", p.len()));
            }
            for f in f0..f1 {
                for (s, &q) in score[f].iter_mut().zip(&p) {
                    *s += (q + 1e-9).ln();
                }
                cnt[f] += 1;
            }
        }
        k += 1;
    }
    for f in 0..n {
        if cnt[f] > 0 && voiced[f] {
            score[f].iter_mut().for_each(|s| *s /= cnt[f] as f64);
        } else {
            score[f].iter_mut().for_each(|s| *s = 0.0); // (no evidence there: the path carries its language through)
        }
    }
    let mut cost = vec![0.0f64; nl];
    let mut back = vec![vec![0usize; nl]; n];
    for f in 0..n {
        // (Viterbi: staying is free, a change costs LID_SWITCH)
        let best = argmax(&cost);
        let mv = cost[best] - LID_SWITCH;
        for l in 0..nl {
            back[f][l] = if cost[l] >= mv { l } else { best };
            cost[l] = cost[l].max(mv) + score[f][l];
        }
    }
    let mut lab = vec![0usize; n];
    lab[n - 1] = argmax(&cost);
    for f in (1..n).rev() {
        lab[f - 1] = back[f][lab[f]];
    }
    let mut segs: Vec<(usize, f64, f64)> = Vec::new();
    for (f, &l) in lab.iter().enumerate() {
        match segs.last_mut() {
            Some(s) if s.0 == l => s.2 = (f + 1) as f64 * LID_HOP,
            _ => segs.push((l, f as f64 * LID_HOP, (f + 1) as f64 * LID_HOP)),
        }
    }
    while segs.len() > 1 {
        let Some(i) = segs.iter().position(|s| s.2 - s.1 < LID_MIN) else { break };
        let j = if i > 0 { i - 1 } else { i + 1 };
        segs[j].1 = segs[j].1.min(segs[i].1);
        segs[j].2 = segs[j].2.max(segs[i].2);
        segs.remove(i);
        let mut k = 0;
        while k + 1 < segs.len() {
            if segs[k].0 == segs[k + 1].0 {
                segs[k].2 = segs[k + 1].2;
                segs.remove(k + 1);
            } else {
                k += 1;
            }
        }
    }
    segs[0].1 = 0.0;
    let last = segs.len() - 1;
    segs[last].2 = dur;
    for i in 1..segs.len() {
        let cut = quiet_point(x, segs[i].1, 0.3);
        segs[i - 1].2 = cut;
        segs[i].1 = cut;
    }
    Ok(segs.into_iter().map(|(l, a, b)| (MIXED_LANGS[l].to_string(), a, b)).collect())
}

/// A line in mixed languages: the text, each stretch's language, the stretches, each unit's start time, the seconds.
#[derive(Clone, Debug, Default)]
pub struct Mixed {
    pub text: String,
    pub langs: Vec<String>,
    pub segs: Vec<(String, f64, f64)>,
    pub times: Vec<f64>,
    pub took: f64,
}

/// A line in any of MIXED_LANGS, even several: cut into stretches, each written by its language's model.
pub fn transcribe_mixed(
    models: &Models,
    x: &[f32],
    fallback: &str,
    cache: Option<&mut HashMap<i64, Vec<f64>>>,
) -> Result<Mixed, String> {
    let t0 = Instant::now();
    let segs = segments(x, fallback, cache, &mut |w: &[f32]| models.lid_probs(w))?;
    let mut parts: Vec<String> = Vec::new();
    let mut times = Vec::new();
    for (lang, a, b) in &segs {
        let (text, tt, _) = models.offline_timed(roll_model(lang), slice(x, (a * SR) as usize, (b * SR) as usize), *a)?;
        if !text.is_empty() {
            parts.push(text);
            times.extend(tt);
        }
    }
    Ok(Mixed {
        text: parts.join(" "),
        langs: segs.iter().map(|s| s.0.clone()).collect(),
        segs,
        times,
        took: t0.elapsed().as_secs_f64(),
    })
}
