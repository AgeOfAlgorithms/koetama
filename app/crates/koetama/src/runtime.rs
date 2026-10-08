//! The running app, game-independent (engine/runtime.py): the chosen game's link, the voice mixer and its output, the
//! speech-to-text and the microphone. The window (gui.rs) and the command line (cli.rs) both drive one of these:
//! start(), then tick() a few times a second, stop() at the end; status() says what is going on.
//!
//! The microphone is open only while the game wants it (its feed's mic flag) and is running; the speech models load
//! the first time it is wanted (downloaded once). Which models: the languages the player speaks (chosen in the window;
//! none chosen: the game's "Language I speak") - one language, its model; several, theirs and the language detector,
//! choosing among exactly those. A change of languages loads what is new and lets go of what is no longer needed.
//!
//! Real voices (a game that plays voices; PROTOCOL.md version 5): the voice chat (kd_voice::Voice) follows each feed
//! (the room, whom to send to, whom to hear), takes the microphone's audio and plays what arrives through the mixer;
//! once per game session the runtime sends the game a fresh room (kind 'r').
//!
//! Translation (a game that uses it; PROTOCOL.md version 6): the translator (kd_translate::Translator) gets each
//! feed's rules and new lines as the feed is read; its replies go to the game at once (kind 'x'), and the rules' states
//! on each change (kind 'd') - again when the game starts a new session or reconnects.
use crate::mic::Microphone;
use kd_audio::{Mixer, MixerSink, Output, SharedMixer, STALE};
use kd_common::Log;
use kd_games::{Game, GameKind};
use kd_speech::{Callbacks, Listener, Mic, ModelState, Models, MIXED_LANGS, MODEL_INFO};
use kd_translate::service::{Event, RuleStatus, Translator};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Makes the microphone-like the listener hears instead of the real microphone (--mic-wav, --auto-speech).
pub type MicSource = Box<dyn FnOnce(Listener, Log) -> Box<dyn Mic>>;

pub struct Options {
    pub threads: usize,
    pub out_device: Option<String>,
    pub mic_device: Option<String>,
    pub volume: f64,
    /// the languages the player speaks (empty: the game's "Language I speak"); "auto speech" sets its own per line
    pub langs: Vec<String>,
    pub no_mic: bool,
    /// where the mod looks for Koetama's files (None: the game module's own)
    pub io_dir: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            threads: 4,
            out_device: None,
            mic_device: None,
            volume: 1.0,
            langs: Vec::new(),
            no_mic: false,
            io_dir: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Ready {
    NotAsked,
    Loading,
    Loaded,
}

/// What the listener's thread and the model loading tell the window.
#[derive(Default)]
struct Said {
    live_now: String,
    last_said: String,
    error: String,
}

pub struct SpeakerStatus {
    pub name: String,
    pub talk: bool,
    pub gain: f64,
    pub az: f64,
    pub muffle: f64,
}

/// A speech model as the window shows it.
pub struct ModelRow {
    pub title: &'static str,
    pub memory_mb: u32,
    /// the chosen languages need it
    pub needed: bool,
    pub state: ModelState,
}

pub struct Status {
    /// "waiting" (no feed yet: the game is not running the mod), "paused", "connected"
    pub state: &'static str,
    /// "off", "wanted", "loading", "listening", "talking"
    pub mic: &'static str,
    pub download: Option<(String, u64, u64)>,
    pub level: f64,
    /// the languages in use (the player's choice, or the game's setting) and whether they are the game's
    pub langs: Vec<String>,
    pub langs_from_game: bool,
    pub models: Vec<ModelRow>,
    pub live: String,
    pub last: String,
    pub speakers: Vec<SpeakerStatus>,
    pub error: String,
    /// the voice chat (None: the game plays no voices)
    pub voice: Option<kd_voice::VoiceStatus>,
    /// the translation rules and their states (None: the game does not use translation)
    pub translate: Option<Vec<RuleStatus>>,
}

pub struct Runtime {
    pub kind: GameKind,
    log: Log,
    opts: Options,
    pub mixer: SharedMixer,
    pub game: Arc<Mutex<Box<dyn Game>>>,
    output: Option<Output>,
    listener: Option<Listener>,
    mic: Option<Box<dyn Mic>>,
    /// the microphone is a recording or a playlist (not switched with the device setting)
    mic_is_source: bool,
    ready: Arc<Mutex<Ready>>,
    loaded_key: String,
    /// the languages as last seen, and since when (a change waits until it settles: ticking boxes one by one does
    /// not load and unload models for each step)
    seen_key: String,
    seen_since: std::time::Instant,
    warming: Arc<AtomicBool>,
    said: Arc<Mutex<Said>>,
    /// the voice chat (a game that plays voices)
    voice: Option<kd_voice::Voice>,
    /// the game session the last voice room was sent for
    room_sid: Option<i64>,
    /// the voice chat's state as last told to the game (and whether the game was connected then)
    voice_told: Option<&'static str>,
    voice_told_to: bool,
    /// the translator (a game that uses translation)
    translator: Option<Translator>,
    /// the rules' states as last told to the game: (its session then, what was told)
    rules_told: Arc<Mutex<Option<(i64, String)>>>,
}

/// The game, once it is made (the translator's replies go to it; the translator starts before it).
type GameSlot = Arc<Mutex<Option<Arc<Mutex<Box<dyn Game>>>>>>;

/// Tells the game the rules' states; remembers what was told in which session (false: no game listening).
fn tell_rules(game: &Mutex<Box<dyn Game>>, rules: &[RuleStatus], told: &Mutex<Option<(i64, String)>>) -> bool {
    let g = game.lock().unwrap();
    let Some(sid) = g.feed().map(|f| f.sid) else { return false };
    let common: Vec<kd_common::feed::RuleState> = rules.iter().map(RuleStatus::common).collect();
    let wire = kd_common::feed::translations_wire(&common);
    if !g.send_translations_state(&common) {
        return false;
    }
    *told.lock().unwrap() = Some((sid, wire));
    true
}

/// Where the game's feeds go: the mixer, the voice chat, and the push-to-talk key straight to the listener (as each
/// feed is read, not at the next tick: the end of a line follows the key at once).
struct Sink {
    mixer: MixerSink,
    listener: Arc<Mutex<Option<Listener>>>,
    voice: Option<kd_voice::Voice>,
    /// the translator, and the session its ids belong to
    translator: Option<Translator>,
    sid: Mutex<Option<i64>>,
}

impl kd_common::feed::FeedSink for Sink {
    fn set_feed(&self, feed: kd_common::feed::Feed) {
        if let Some(l) = self.listener.lock().unwrap().as_ref() {
            l.set_push_to_talk(if feed.mic { feed.ptt } else { None });
        }
        if let Some(v) = &self.voice {
            v.set_feed(&feed);
        }
        if let Some(t) = &self.translator {
            // (a new session: the game's ids start over)
            let mut sid = self.sid.lock().unwrap();
            if *sid != Some(feed.sid) {
                if sid.is_some() {
                    t.new_session();
                }
                *sid = Some(feed.sid);
            }
            t.set_rules(&feed.translations);
            for (id, text) in &feed.to_translate {
                t.request(*id, text); // (ids already queued or answered: ignored)
            }
        }
        self.mixer.set_feed(feed);
    }

    fn fresh(&self) -> bool {
        self.mixer.fresh()
    }
}

impl Runtime {
    // ---- start / stop
    pub fn start(
        kind: GameKind,
        log: Log,
        opts: Options,
        mic_source: Option<MicSource>,
    ) -> Runtime {
        let mixer: SharedMixer = Arc::new(Mutex::new(Mixer::new(HashMap::new())));
        mixer.lock().unwrap().volume = opts.volume.clamp(0.0, 1.0);
        // (real voices: for a game that plays voices; the relay is KOETAMA_RELAY or Koetama's own)
        let voice = kind.voices.then(|| kd_voice::Voice::start(kd_voice::relay_url(), log.clone()));
        if let Some(v) = &voice {
            mixer.lock().unwrap().streams = Some(Box::new(v.playback()));
        }
        let ptt_to: Arc<Mutex<Option<Listener>>> = Arc::new(Mutex::new(None));
        // (translation: the translator's replies and states go straight to the game)
        let slot: GameSlot = Arc::new(Mutex::new(None));
        let rules_told: Arc<Mutex<Option<(i64, String)>>> = Arc::new(Mutex::new(None));
        let translator = kind.translate.then(|| {
            let (slot, told) = (slot.clone(), rules_told.clone());
            Translator::mozilla(
                Arc::new(move |e| {
                    let Some(game) = slot.lock().unwrap().clone() else { return };
                    match e {
                        Event::Reply { id, text } => {
                            game.lock().unwrap().send_translation(id, &text);
                        }
                        Event::Status(rules) => {
                            tell_rules(&game, &rules, &told);
                        }
                    }
                }),
                log.clone(),
            )
        });
        let sink: Arc<dyn kd_common::feed::FeedSink> = Arc::new(Sink {
            mixer: MixerSink(mixer.clone()),
            listener: ptt_to.clone(),
            voice: voice.clone(),
            translator: translator.clone(),
            sid: Mutex::new(None),
        });
        let game = kind.make(sink, log.clone(), opts.io_dir.clone());
        // (no test voices: the game's test speakers are silent)
        let mut clips = HashMap::new();
        // (a speech-only game mod plays no voices: no test voices, no sound output)
        let voices = if kind.voices { game.test_voices() } else { HashMap::new() };
        for (src, path) in voices {
            match kd_audio::load_wav(&path) {
                Ok(x) => {
                    clips.insert(src, Arc::new(x));
                }
                Err(e) => log(&format!("test voices: {}: {e}", path.display())),
            }
        }
        mixer.lock().unwrap().clips = clips;
        let game = Arc::new(Mutex::new(game));
        *slot.lock().unwrap() = Some(game.clone());
        let mut rt = Runtime {
            kind,
            log,
            opts,
            mixer,
            game,
            output: None,
            listener: None,
            mic: None,
            mic_is_source: mic_source.is_some(),
            ready: Arc::new(Mutex::new(Ready::NotAsked)),
            loaded_key: String::new(),
            seen_key: String::new(),
            seen_since: std::time::Instant::now(),
            warming: Arc::new(AtomicBool::new(false)),
            said: Arc::new(Mutex::new(Said::default())),
            voice,
            room_sid: None,
            voice_told: None,
            voice_told_to: false,
            translator,
            rules_told,
        };
        if rt.kind.voices {
            rt.open_output();
        }
        rt.game.lock().unwrap().start();
        // (a voices-only game mod: no microphone, no speech models)
        if !rt.opts.no_mic && rt.kind.speech {
            rt.make_listener(mic_source);
            *ptt_to.lock().unwrap() = rt.listener.clone();
        }
        rt
    }

    fn open_output(&mut self) {
        self.output = None;
        match Output::open(
            self.mixer.clone(),
            self.opts.out_device.as_deref(),
            self.log.clone(),
        ) {
            Ok(o) => self.output = Some(o),
            Err(e) => {
                let msg = format!("no sound output: {e}");
                (self.log)(&msg);
                self.said.lock().unwrap().error = msg;
            }
        }
    }

    fn make_listener(&mut self, mic_source: Option<MicSource>) {
        let models = Models::new(self.opts.threads, self.log.clone());
        let (g1, g2, g3) = (self.game.clone(), self.game.clone(), self.game.clone());
        let (s1, s2) = (self.said.clone(), self.said.clone());
        let log = self.log.clone();
        let cb = Callbacks {
            // (talking: their head bobs at once)
            on_start: Box::new(move |utt| {
                g1.lock().unwrap().send('s', utt, "", None, None);
            }),
            on_live: Box::new(move |utt, words, info| {
                s1.lock().unwrap().live_now = words.to_string();
                g2.lock()
                    .unwrap()
                    .send('l', utt, words, Some(&info.times), Some(info.t0));
            }),
            on_final: Box::new(move |utt, text, info| {
                let sent =
                    g3.lock()
                        .unwrap()
                        .send('f', utt, text, Some(&info.times), Some(info.t0));
                let mut s = s2.lock().unwrap();
                s.live_now.clear();
                if !text.is_empty() {
                    s.last_said = text.to_string();
                    log(&format!(
                        "you said{}: {text}",
                        if sent { "" } else { " [no game to tell]" }
                    ));
                }
            }),
        };
        match Listener::new(cb, models, true) {
            Ok(l) => {
                self.mic = Some(match mic_source {
                    Some(make) => make(l.clone(), self.log.clone()),
                    None => Box::new(
                        Microphone::new(l.clone(), self.opts.mic_device.clone(), self.log.clone())
                            .with_voice(self.voice.clone()),
                    ),
                });
                self.listener = Some(l);
            }
            Err(e) => {
                let msg = format!("the speech-to-text could not start: {e}");
                (self.log)(&msg);
                self.said.lock().unwrap().error = msg;
            }
        }
    }

    pub fn stop(&mut self) {
        if let Some(m) = self.mic.as_mut() {
            m.close();
        }
        if let Some(l) = &self.listener {
            if l.started() {
                l.stop();
            }
        }
        self.output = None;
        if let Some(v) = &self.voice {
            v.stop();
        }
        // (before the game's lock is taken: the translator's thread may be waiting on it to send a reply)
        if let Some(t) = &self.translator {
            t.stop();
        }
        self.game.lock().unwrap().stop();
    }

    // ---- settings while running
    pub fn set_volume(&mut self, v: f64) {
        self.opts.volume = v.clamp(0.0, 1.0);
        self.mixer.lock().unwrap().volume = self.opts.volume;
    }

    pub fn set_output(&mut self, device: Option<String>) {
        self.opts.out_device = device;
        if self.kind.voices {
            self.open_output();
        }
    }

    pub fn set_mic(&mut self, device: Option<String>) {
        self.opts.mic_device = device;
        let Some(l) = &self.listener else { return };
        if self.mic_is_source {
            return;
        }
        let was = self.mic.as_ref().map(|m| m.is_open()).unwrap_or(false);
        if let Some(m) = self.mic.as_mut() {
            m.close();
        }
        let mut m: Box<dyn Mic> = Box::new(
            Microphone::new(l.clone(), self.opts.mic_device.clone(), self.log.clone()).with_voice(self.voice.clone()),
        );
        if was {
            m.open();
        }
        self.mic = Some(m);
    }

    // ---- a few times a second
    /// The languages in use: the player's choice, else the game's "Language I speak" ("auto": MIXED_LANGS); and
    /// whether they came from the game.
    pub fn languages(&self) -> (Vec<String>, bool) {
        if !self.opts.langs.is_empty() {
            return (self.opts.langs.clone(), false);
        }
        let lang = self.game.lock().unwrap().language();
        if lang == "auto" {
            (MIXED_LANGS.iter().map(|l| l.to_string()).collect(), true)
        } else {
            (vec![lang], true)
        }
    }

    /// The player's languages (empty: follow the game's setting). The models follow on the next tick.
    pub fn set_languages(&mut self, langs: Vec<String>) {
        self.opts.langs = langs;
    }

    fn warm(&mut self, langs: Vec<String>) {
        let (l, ready, said, log, warming) = (
            self.listener.clone(),
            self.ready.clone(),
            self.said.clone(),
            self.log.clone(),
            self.warming.clone(),
        );
        let Some(l) = l else { return };
        self.loaded_key = langs.join(",");
        warming.store(true, Ordering::SeqCst);
        std::thread::spawn(move || {
            l.set_languages(&langs);
            match l.warm(None) {
                Ok(()) => *ready.lock().unwrap() = Ready::Loaded,
                Err(e) => {
                    let msg = format!("the speech models could not be loaded: {e}");
                    log(&msg);
                    said.lock().unwrap().error = msg;
                    *ready.lock().unwrap() = Ready::NotAsked;
                }
            }
            warming.store(false, Ordering::SeqCst);
        });
    }

    /// Once per game session (a new session in the feed, the game connected): a fresh voice room for it, kind 'r'
    /// "<room>:<key>". The game's host keeps the first one its session gets and hands it to every player.
    fn send_room(&mut self) {
        if self.voice.is_none() {
            return;
        }
        let game = self.game.clone();
        let g = game.lock().unwrap();
        let Some(f) = g.feed() else { return };
        if !g.connected() || self.room_sid == Some(f.sid) {
            return;
        }
        match kd_voice::crypto::new_room() {
            Ok(text) => {
                if g.send('r', 0, &text, None, None) {
                    self.room_sid = Some(f.sid);
                }
            }
            Err(e) => {
                (self.log)(&format!("voice: {e}"));
                self.room_sid = Some(f.sid); // (said once per session)
            }
        }
    }

    /// The rules' states again when the game is in a session they were not told in (a new level, a reconnect); none
    /// told while there are no rules.
    fn tell_rules_again(&mut self) {
        let Some(t) = &self.translator else { return };
        let sid = {
            let g = self.game.lock().unwrap();
            match g.feed() {
                Some(f) if g.connected() => f.sid,
                _ => return,
            }
        };
        if self.rules_told.lock().unwrap().as_ref().is_some_and(|(s, _)| *s == sid) {
            return;
        }
        let rules = t.status();
        if rules.is_empty() {
            *self.rules_told.lock().unwrap() = Some((sid, String::new()));
        } else {
            tell_rules(&self.game, &rules, &self.rules_told);
        }
    }

    pub fn tick(&mut self) {
        self.send_room();
        self.tell_rules_again();
        if let Some(v) = &self.voice {
            // (the voice chat's link, for the game to show: each change, and again when the game reconnects)
            let state = v.status().state;
            let connected = self.game.lock().unwrap().connected();
            if Some(state) != self.voice_told || connected != self.voice_told_to {
                self.game.lock().unwrap().set_voice_state(state);
                self.voice_told = Some(state);
                self.voice_told_to = connected;
            }
        }
        let Some(l) = self.listener.clone() else {
            return;
        };
        let (want, live, ptt) = {
            let g = self.game.lock().unwrap();
            (g.wants_mic() && g.connected(), g.live_words(), g.push_to_talk())
        };
        let (langs, _) = self.languages();
        let key = langs.join(",");
        let ready = *self.ready.lock().unwrap();
        let warming = self.warming.load(Ordering::SeqCst);
        if key != self.seen_key {
            self.seen_key = key.clone();
            self.seen_since = std::time::Instant::now();
        }
        let settled = self.seen_since.elapsed().as_secs_f64() >= 1.5;
        if want && ready == Ready::NotAsked && !warming {
            // (load the models first: downloaded the first time)
            *self.ready.lock().unwrap() = Ready::Loading;
            (self.log)(&format!(
                "loading the speech models for {key} (the first time they are downloaded)..."
            ));
            self.warm(langs.clone());
        } else if ready == Ready::Loaded && key != self.loaded_key && !warming && settled {
            // (other languages: their models in the background; the ones no longer needed let go)
            (self.log)(&format!("languages: {key}"));
            self.warm(langs.clone());
        }
        let lang = kd_speech::plan(&langs).0;
        l.set_live(live);
        l.set_push_to_talk(ptt);
        let ready = *self.ready.lock().unwrap();
        let Some(mic) = self.mic.as_mut() else { return };
        if want && ready == Ready::Loaded && !mic.is_open() {
            if lang == "auto" || !self.mic_is_source {
                l.set_languages(&langs);
            } else {
                l.set_language(&lang);
            }
            if !l.started() {
                l.start();
            }
            mic.open();
        } else if !want && mic.is_open() {
            mic.close();
        }
    }

    // ---- what is going on
    pub fn status(&self) -> Status {
        let (feed, age) = {
            let m = self.mixer.lock().unwrap();
            (m.feed().cloned(), m.feed_age())
        };
        let state = match &feed {
            None => "waiting",
            Some(_) if age > STALE => "paused",
            Some(_) => "connected",
        };
        let g = self.game.lock().unwrap();
        let ready = *self.ready.lock().unwrap();
        let mic_open = self.mic.as_ref().map(|m| m.is_open()).unwrap_or(false);
        let talking = self.listener.as_ref().map(|l| l.talking()).unwrap_or(false);
        let mic = if mic_open {
            if talking {
                "talking"
            } else {
                "listening"
            }
        } else if ready == Ready::Loading {
            "loading"
        } else if g.wants_mic() && state == "connected" {
            "wanted"
        } else {
            "off"
        };
        let mut speakers = Vec::new();
        if let (Some(f), "connected") = (&feed, state) {
            for (id, sp) in &f.speakers {
                speakers.push(SpeakerStatus {
                    // (a real player: src 0, their player id)
                    name: if sp.src == 0 { format!("player {id}") } else { g.speaker_name(sp.src) },
                    talk: sp.talk,
                    gain: sp.gain,
                    az: sp.az,
                    muffle: sp.muffle,
                });
            }
        }
        let (langs, langs_from_game) = {
            drop(g);
            self.languages()
        };
        let need = kd_speech::models_for(&langs);
        let models = self.listener.as_ref().map(|l| l.models());
        let rows = MODEL_INFO
            .iter()
            .map(|m| ModelRow {
                title: m.title,
                memory_mb: m.memory_mb,
                needed: need.contains(&m.name),
                state: models
                    .as_ref()
                    .map(|ms| ms.state(m.name))
                    .unwrap_or(ModelState::NotLoaded),
            })
            .collect();
        let said = self.said.lock().unwrap();
        Status {
            state,
            mic,
            download: self
                .listener
                .as_ref()
                .and_then(|l| l.models().downloading()),
            level: self.mic.as_ref().map(|m| m.level()).unwrap_or(-120.0),
            langs,
            langs_from_game,
            models: rows,
            live: said.live_now.clone(),
            last: said.last_said.clone(),
            speakers,
            error: said.error.clone(),
            voice: self.voice.as_ref().map(|v| v.status()),
            translate: self.translator.as_ref().map(|t| t.status()),
        }
    }

    pub fn output_info(&self) -> Option<(String, f64)> {
        self.output
            .as_ref()
            .map(|o| (o.device_name(), o.latency_ms()))
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop();
    }
}
