//! One line while the player talks (asr.py RollingLine).
use crate::models::Models;
use crate::stitch::transcribe_mixed_in;
use crate::{roll_model, MIXED_LANGS, RATE, ROLL_MAX, ROLL_SLOW};
use kd_common::text::{is_wide, tidy, unit_key, units};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What comes with live words: each unit's start (s from t0 = when the line's audio began).
#[derive(Clone, Debug)]
pub struct LineInfo {
    pub times: Vec<f64>,
    pub t0: Instant,
}

/// What comes with a finished line.
#[derive(Clone, Debug)]
pub struct FinalInfo {
    /// the live words shown before
    pub live: String,
    /// the model that wrote it ("auto": the languages, "en+ru")
    pub used: String,
    /// its language ("auto": the language it was mostly in)
    pub lang: String,
    /// s of speech fed (from the detected start)
    pub speech: f64,
    /// s the final pass took
    pub second_s: f64,
    /// each unit's start, s from t0
    pub times: Vec<f64>,
    pub t0: Instant,
    /// s finishing took
    pub finish_s: f64,
    /// s the live words' passes took
    pub live_cpu: f64,
    pub passes: u32,
}

/// The rolling design (bench/rolling.py; no streaming model): the line so far transcribed again every
/// ROLL_EVERY s by the language's own model, shown at once; at the end one pass over the whole line. Lines are
/// at most MAX_LINE s, where a whole-line pass still costs well under real time (no window needed).
/// lang "auto": the line may change language - each pass is cut into stretches by language (the detector) and each
/// stretch written by its language's model (transcribe_mixed).
pub struct RollingLine {
    models: Arc<Models>,
    pub utt: u32,
    pub lang: String,
    pub fallback: String,
    pub live: bool,
    pub model: &'static str,
    audio: Vec<f32>,
    /// when the line's audio begins: word times count from here
    pub t0: Instant,
    /// the live words so far: [(space before, unit, time)]
    pub committed: Vec<(String, String, f64)>,
    prev: Option<Vec<String>>, // (the last pass's units, compared: unit_key)
    pub times: Vec<f64>,
    pub text: String,
    pub speech: f64,
    next: f64,
    compute: f64,
    passes: u32,
    langs: Vec<String>,
    segs: Vec<(String, f64, f64)>,
    lid_cache: HashMap<i64, Vec<f64>>,
    /// lang "auto": the languages it may be in (the ones the player speaks; MIXED_LANGS if not told)
    cands: Vec<String>,
}

impl RollingLine {
    pub fn new(models: Arc<Models>, utt: u32, lang: &str, preroll: Vec<f32>, fallback: &str, live: bool) -> RollingLine {
        let now = Instant::now();
        let t0 = now.checked_sub(Duration::from_secs_f64(preroll.len() as f64 / RATE as f64)).unwrap_or(now);
        let next = models.every();
        RollingLine {
            models,
            utt,
            lang: lang.to_string(),
            fallback: fallback.to_string(),
            live,
            model: roll_model(lang),
            audio: preroll,
            t0,
            committed: Vec::new(),
            prev: None,
            times: Vec::new(),
            text: String::new(),
            speech: 0.0,
            next,
            compute: 0.0,
            passes: 0,
            langs: vec![lang.to_string()],
            segs: Vec::new(),
            lid_cache: HashMap::new(),
            cands: MIXED_LANGS.iter().map(|l| l.to_string()).collect(),
        }
    }

    /// The line's candidate languages for "auto" (the ones the player speaks).
    pub fn with_candidates(mut self, cands: &[String]) -> RollingLine {
        if !cands.is_empty() {
            self.cands = cands.to_vec();
        }
        self
    }

    /// A line with no audio and no models loaded (the LocalAgreement tests: commit only).
    pub fn for_test() -> RollingLine {
        RollingLine::new(Models::new(1, kd_common::null_log()), 0, "en", Vec::new(), "en", true)
    }

    /// (text, [unit times], seconds)
    fn pass(&mut self, audio: &[f32]) -> Result<(String, Vec<f64>, f64), String> {
        if self.lang == "auto" {
            let m = transcribe_mixed_in(&self.models, &self.cands, audio, &self.fallback, Some(&mut self.lid_cache))?;
            self.langs = m.langs;
            self.segs = m.segs;
            return Ok((tidy(&m.text), m.times, m.took));
        }
        self.models.offline_timed(self.model, audio, 0.0)
    }

    /// The live words: only units two passes in a row agree on, never the last one (it may be cut off), and
    /// once shown never taken back - the bubble fills chunk by chunk (LocalAgreement). True if it grew.
    pub fn commit(&mut self, text: &str, times: &[f64]) -> bool {
        let us = units(text);
        let keys: Vec<String> = us.iter().map(|(_, u)| unit_key(u)).collect();
        let chars: Vec<char> = text.chars().collect();
        let mut grew = false;
        if let Some(prev) = &self.prev {
            if times.len() == us.len() {
                let mut k = 0;
                while k < keys.len().min(prev.len()) && keys[k] == prev[k] {
                    k += 1;
                }
                let k = (k as isize).min(us.len() as isize - 1);
                let mut i = self.committed.len();
                while (i as isize) < k {
                    let (start, u) = &us[i];
                    let mut sep = String::new();
                    if i > 0 {
                        let (pstart, pu) = &us[i - 1];
                        let end = pstart + pu.chars().count();
                        sep = chars[end..*start].iter().collect();
                        let wide = u.chars().next().is_some_and(is_wide) || pu.chars().last().is_some_and(is_wide);
                        if sep.is_empty() && !wide {
                            sep = " ".into(); // (two units that were apart stay apart)
                        }
                    }
                    self.committed.push((sep, u.clone(), times[i]));
                    grew = true;
                    i += 1;
                }
            }
        }
        self.prev = Some(keys);
        grew
    }

    /// More of the line's audio: Some(the live words) when they grew. A pass every Models::every s of speech.
    pub fn feed(&mut self, x: &[f32]) -> Result<Option<String>, String> {
        self.audio.extend_from_slice(x);
        self.speech += x.len() as f64 / RATE as f64;
        if !self.live || self.speech < self.next {
            // (live words off: only the finished line)
            return Ok(None);
        }
        let mut every = self.models.every();
        let audio = std::mem::take(&mut self.audio);
        let r = self.pass(&audio);
        self.audio = audio;
        let (text, times, took) = match r {
            Ok(r) => r,
            Err(e) => {
                self.next = self.speech + every; // (a failed pass is tried again at the next interval, not every block)
                return Err(e);
            }
        };
        if took > ROLL_SLOW * every && every < ROLL_MAX {
            // (this PC is slow for it: pass less often)
            every = ROLL_MAX.min(every * 2.0);
            self.models.set_every(every);
            (self.models.log())(&format!("live words every {every:.0} s (a pass took {took:.2} s)"));
        }
        self.next = self.speech + every;
        self.compute += took;
        self.passes += 1;
        if !text.is_empty() && self.commit(&text, &times) {
            self.text = self.committed.iter().map(|(sep, u, _)| format!("{sep}{u}")).collect();
            self.times = self.committed.iter().map(|c| c.2).collect();
            return Ok(Some(self.text.clone()));
        }
        Ok(None)
    }

    /// The finished line: one pass over all of it (and 0.2 s of quiet after).
    pub fn finish(&mut self) -> Result<(String, FinalInfo), String> {
        let t0 = Instant::now();
        let mut audio = std::mem::take(&mut self.audio);
        let n = audio.len();
        audio.resize(n + (RATE as f64 * 0.2) as usize, 0.0);
        let r = self.pass(&audio);
        audio.truncate(n);
        self.audio = audio;
        let (text, times, took) = r?;
        let auto = self.lang == "auto";
        let used = if auto {
            let mut seen: Vec<&str> = Vec::new();
            for l in &self.langs {
                if !seen.contains(&l.as_str()) {
                    seen.push(l);
                }
            }
            seen.join("+")
        } else {
            self.model.to_string()
        };
        let mut main = self.lang.clone(); // (auto: the language it was mostly in)
        if auto && !self.segs.is_empty() {
            let mut span: Vec<(&str, f64)> = Vec::new();
            for (l, a, b) in &self.segs {
                match span.iter_mut().find(|s| s.0 == l) {
                    Some(s) => s.1 += b - a,
                    None => span.push((l, b - a)),
                }
            }
            let mut best = 0;
            for (i, s) in span.iter().enumerate() {
                if s.1 > span[best].1 {
                    best = i;
                }
            }
            main = span[best].0.to_string();
        }
        let info = FinalInfo {
            live: self.text.clone(),
            used,
            lang: main,
            speech: self.speech,
            second_s: took,
            times,
            t0: self.t0,
            finish_s: t0.elapsed().as_secs_f64(),
            live_cpu: self.compute,
            passes: self.passes,
        };
        Ok((text, info))
    }

    /// What a line that could not be finished reports (the error is logged): no text.
    pub(crate) fn failed(&self) -> FinalInfo {
        FinalInfo {
            live: self.text.clone(),
            used: self.model.to_string(),
            lang: self.lang.clone(),
            speech: self.speech,
            second_s: 0.0,
            times: Vec::new(),
            t0: self.t0,
            finish_s: 0.0,
            live_cpu: self.compute,
            passes: self.passes,
        }
    }
}
