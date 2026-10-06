//! The running app, game-independent (engine/runtime.py): the chosen game's link, the voice mixer and its output, the
//! speech-to-text and the microphone. The window (gui.rs) and the command line (cli.rs) both drive one of these:
//! start(), then tick() a few times a second, stop() at the end; status() says what is going on.
//!
//! The microphone is open only while the game wants it (its feed's mic flag) and is running; the speech models load
//! the first time it is wanted (downloaded once), and again for another language.
use crate::mic::Microphone;
use kd_audio::{Mixer, MixerSink, Output, SharedMixer, STALE};
use kd_common::Log;
use kd_games::{Game, GameKind};
use kd_speech::{Callbacks, Listener, Mic, Models};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Makes the microphone-like the listener hears instead of the real microphone (--mic-wav, --auto-speech).
pub type MicSource = Box<dyn FnOnce(Listener, Log) -> Box<dyn Mic>>;

pub struct Options {
    pub threads: usize,
    pub out_device: Option<String>,
    pub mic_device: Option<String>,
    pub volume: f64,
    /// None: the game's setting; "auto speech" sets its own per line
    pub lang: Option<String>,
    pub no_mic: bool,
    /// where the mod looks for Kotodama's files (None: the game module's own)
    pub io_dir: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Options { threads: 4, out_device: None, mic_device: None, volume: 1.0, lang: None, no_mic: false, io_dir: None }
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

pub struct Status {
    /// "waiting" (no feed yet: the game is not running the mod), "paused", "connected"
    pub state: &'static str,
    /// "off", "wanted", "loading", "listening", "talking"
    pub mic: &'static str,
    pub download: Option<(String, u64, u64)>,
    pub level: f64,
    pub lang: String,
    pub live: String,
    pub last: String,
    pub speakers: Vec<SpeakerStatus>,
    pub error: String,
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
    loaded_lang: String,
    said: Arc<Mutex<Said>>,
}

impl Runtime {
    // ---- start / stop
    pub fn start(kind: GameKind, log: Log, opts: Options, mic_source: Option<MicSource>) -> Runtime {
        let mixer: SharedMixer = Arc::new(Mutex::new(Mixer::new(HashMap::new())));
        mixer.lock().unwrap().volume = opts.volume.clamp(0.0, 1.0);
        let sink: Arc<dyn kd_common::feed::FeedSink> = Arc::new(MixerSink(mixer.clone()));
        let game = (kind.make)(sink, log.clone(), opts.io_dir.clone());
        // (no test voices: the game's test speakers are silent)
        let mut clips = HashMap::new();
        for (src, path) in game.test_voices() {
            match kd_audio::load_wav(&path) {
                Ok(x) => {
                    clips.insert(src, Arc::new(x));
                }
                Err(e) => log(&format!("test voices: {}: {e}", path.display())),
            }
        }
        mixer.lock().unwrap().clips = clips;
        let mut rt = Runtime {
            kind,
            log,
            opts,
            mixer,
            game: Arc::new(Mutex::new(game)),
            output: None,
            listener: None,
            mic: None,
            mic_is_source: mic_source.is_some(),
            ready: Arc::new(Mutex::new(Ready::NotAsked)),
            loaded_lang: String::new(),
            said: Arc::new(Mutex::new(Said::default())),
        };
        rt.open_output();
        rt.game.lock().unwrap().start();
        if !rt.opts.no_mic {
            rt.make_listener(mic_source);
        }
        rt
    }

    fn open_output(&mut self) {
        self.output = None;
        match Output::open(self.mixer.clone(), self.opts.out_device.as_deref(), self.log.clone()) {
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
                g2.lock().unwrap().send('l', utt, words, Some(&info.times), Some(info.t0));
            }),
            on_final: Box::new(move |utt, text, info| {
                let sent = g3.lock().unwrap().send('f', utt, text, Some(&info.times), Some(info.t0));
                let mut s = s2.lock().unwrap();
                s.live_now.clear();
                if !text.is_empty() {
                    s.last_said = text.to_string();
                    log(&format!("you said{}: {text}", if sent { "" } else { " [no game to tell]" }));
                }
            }),
        };
        match Listener::new(cb, models, true) {
            Ok(l) => {
                self.mic = Some(match mic_source {
                    Some(make) => make(l.clone(), self.log.clone()),
                    None => Box::new(Microphone::new(l.clone(), self.opts.mic_device.clone(), self.log.clone())),
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
        self.game.lock().unwrap().stop();
    }

    // ---- settings while running
    pub fn set_volume(&mut self, v: f64) {
        self.opts.volume = v.clamp(0.0, 1.0);
        self.mixer.lock().unwrap().volume = self.opts.volume;
    }

    pub fn set_output(&mut self, device: Option<String>) {
        self.opts.out_device = device;
        self.open_output();
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
        let mut m: Box<dyn Mic> = Box::new(Microphone::new(l.clone(), self.opts.mic_device.clone(), self.log.clone()));
        if was {
            m.open();
        }
        self.mic = Some(m);
    }

    // ---- a few times a second
    pub fn language(&self) -> String {
        self.opts.lang.clone().unwrap_or_else(|| self.game.lock().unwrap().language())
    }

    fn warm(&self, lang: String) {
        let (l, ready, said, log) = (self.listener.clone(), self.ready.clone(), self.said.clone(), self.log.clone());
        let Some(l) = l else { return };
        std::thread::spawn(move || {
            l.set_language(&lang);
            match l.warm(None) {
                Ok(()) => *ready.lock().unwrap() = Ready::Loaded,
                Err(e) => {
                    let msg = format!("the speech models could not be loaded: {e}");
                    log(&msg);
                    said.lock().unwrap().error = msg;
                    *ready.lock().unwrap() = Ready::NotAsked;
                }
            }
        });
    }

    pub fn tick(&mut self) {
        let Some(l) = self.listener.clone() else { return };
        let (want, live) = {
            let g = self.game.lock().unwrap();
            (g.wants_mic() && g.connected(), g.live_words())
        };
        let lang = self.language();
        let ready = *self.ready.lock().unwrap();
        if want && ready == Ready::NotAsked {
            // (load the models first: downloaded the first time)
            *self.ready.lock().unwrap() = Ready::Loading;
            self.loaded_lang = lang.clone();
            (self.log)(&format!("loading the speech models for \"{lang}\" (the first time they are downloaded)..."));
            self.warm(lang.clone());
        } else if ready == Ready::Loaded && lang != self.loaded_lang {
            // (another language: its model in the background)
            self.loaded_lang = lang.clone();
            (self.log)(&format!("language: {lang}"));
            self.warm(lang.clone());
        }
        l.set_live(live);
        let ready = *self.ready.lock().unwrap();
        let Some(mic) = self.mic.as_mut() else { return };
        if want && ready == Ready::Loaded && !mic.is_open() {
            l.set_language(&lang);
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
            for sp in f.speakers.values() {
                speakers.push(SpeakerStatus { name: g.speaker_name(sp.src), talk: sp.talk, gain: sp.gain, az: sp.az, muffle: sp.muffle });
            }
        }
        let said = self.said.lock().unwrap();
        Status {
            state,
            mic,
            download: self.listener.as_ref().and_then(|l| l.models().downloading()),
            level: self.mic.as_ref().map(|m| m.level()).unwrap_or(-120.0),
            lang: self.opts.lang.clone().unwrap_or_else(|| g.language()),
            live: said.live_now.clone(),
            last: said.last_said.clone(),
            speakers,
            error: said.error.clone(),
        }
    }

    pub fn output_info(&self) -> Option<(String, f64)> {
        self.output.as_ref().map(|o| (o.device_name(), o.latency_ms()))
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop();
    }
}
