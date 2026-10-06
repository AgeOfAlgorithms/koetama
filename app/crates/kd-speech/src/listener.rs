//! The microphone's audio in, lines out (asr.py Listener, low_priority).
use crate::models::Models;
use crate::rolling::{FinalInfo, LineInfo, RollingLine};
use crate::{lock, roll_model, MAX_LINE, MIXED_LANGS, PREROLL, RATE};
use kd_common::Log;
use sherpa_onnx::VoiceActivityDetector;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

/// blocks queued by push() before the worker drops them (far behind)
const QUEUE: usize = 200;

pub type OnStart = Box<dyn Fn(u32) + Send + Sync>;
pub type OnLive = Box<dyn Fn(u32, &str, &LineInfo) + Send + Sync>;
pub type OnFinal = Box<dyn Fn(u32, &str, &FinalInfo) + Send + Sync>;

/// What a Listener tells: on_start(utt) - the speech detector heard a line begin (at once); on_live(utt, words,
/// info) while the player talks (only ever growing); on_final(utt, text, info) after (text may be "": nothing made
/// out). They run on the thread feeding the audio (the worker after start()) and must not call feed / flush / stop.
pub struct Callbacks {
    pub on_start: OnStart,
    pub on_live: OnLive,
    pub on_final: OnFinal,
}

/// The speech detector and the line going on (one feed at a time).
struct State {
    vad: VoiceActivityDetector,
    win: usize,
    ring: Vec<f32>,    // the last PREROLL s (before speech is detected)
    pending: Vec<f32>, // not yet a whole VAD window
    line: Option<RollingLine>,
    utt: u32,
    last_lang: String, // (auto: the language of the line before - a short line's fallback)
}

struct Inner {
    cb: Callbacks,
    models: Arc<Models>,
    log: Log,
    lang: Mutex<String>,
    live: AtomicBool, // (false: no live words, only the finished line - less CPU)
    talking: AtomicBool,
    state: Mutex<State>,
    tx: SyncSender<Vec<f32>>,
    rx: Mutex<Option<Receiver<Vec<f32>>>>, // (the worker holds it while it runs)
    running: AtomicBool,
    worker: Mutex<Option<JoinHandle<()>>>,
}

/// The microphone's audio in (16 kHz float32, any block size), lines out through the Callbacks. A handle: clones
/// share one listener, usable from any thread (the window sets the language while the worker transcribes; push()
/// from the audio callback never waits for a model pass).
#[derive(Clone)]
pub struct Listener(Arc<Inner>);

impl Listener {
    /// Makes the speech detector (downloaded the first time); the models load on first use (or warm()).
    pub fn new(cb: Callbacks, models: Arc<Models>, live: bool) -> Result<Listener, String> {
        let (vad, win) = models.vad()?;
        let (tx, rx) = sync_channel(QUEUE);
        let log = models.log();
        Ok(Listener(Arc::new(Inner {
            cb,
            models,
            log,
            lang: Mutex::new("en".into()),
            live: AtomicBool::new(live),
            talking: AtomicBool::new(false),
            state: Mutex::new(State {
                vad,
                win,
                ring: Vec::new(),
                pending: Vec::new(),
                line: None,
                utt: 0,
                last_lang: "en".into(),
            }),
            tx,
            rx: Mutex::new(Some(rx)),
            running: AtomicBool::new(false),
            worker: Mutex::new(None),
        })))
    }

    /// The game's "Language I speak" ("auto": found per stretch); "" -> en. The next line uses it.
    pub fn set_language(&self, lang: &str) {
        *lock(&self.0.lang) = if lang.is_empty() { "en".into() } else { lang.to_string() };
    }

    pub fn language(&self) -> String {
        lock(&self.0.lang).clone()
    }

    /// Live words on or off (off: only the finished line - less CPU); the next line uses it.
    pub fn set_live(&self, on: bool) {
        self.0.live.store(on, Ordering::Relaxed);
    }

    /// Load the models a language needs now (the first line would wait for them otherwise). Blocking.
    pub fn warm(&self, lang: Option<&str>) -> Result<(), String> {
        let lang = lang.map(str::to_string).unwrap_or_else(|| self.language());
        if lang == "auto" {
            // (mixed: the detector and every language's model)
            let mut names = vec!["langid"];
            for l in MIXED_LANGS {
                let m = roll_model(l);
                if !names.contains(&m) {
                    names.push(m);
                }
            }
            for name in names {
                self.0.models.load(name)?;
            }
            Ok(())
        } else {
            self.0.models.load(roll_model(&lang))
        }
    }

    /// Audio in, synchronously: the callbacks run before it returns (the command line, tests; the worker).
    pub fn feed(&self, x: &[f32]) {
        let mut st = lock(&self.0.state);
        self.0.feed(&mut st, x);
    }

    /// The end of the input: a line still going is finished.
    pub fn flush(&self) {
        let mut st = lock(&self.0.state);
        st.vad.flush();
        while !st.vad.is_empty() {
            st.vad.pop();
        }
        self.0.end(&mut st);
    }

    /// From the microphone callback: queued for the worker, never waits (far behind: the block is dropped rather
    /// than lag forever).
    pub fn push(&self, x: &[f32]) {
        let _ = self.0.tx.try_send(x.to_vec());
    }

    /// The worker thread (below normal priority): pushed blocks -> feed.
    pub fn start(&self) {
        let mut worker = lock(&self.0.worker);
        if self.0.running.load(Ordering::SeqCst) {
            return;
        }
        if let Some(h) = worker.take() {
            let _ = h.join(); // (a worker stopping: it hands the queue back)
        }
        let Some(rx) = lock(&self.0.rx).take() else { return };
        self.0.running.store(true, Ordering::SeqCst);
        let weak: Weak<Inner> = Arc::downgrade(&self.0);
        let spawned = std::thread::Builder::new().name("speech".into()).spawn(move || run(weak, rx));
        match spawned {
            Ok(h) => *worker = Some(h),
            Err(e) => {
                self.0.running.store(false, Ordering::SeqCst);
                (self.0.log)(&format!("speech: the worker could not start: {e}"));
            }
        }
    }

    /// Stops the worker (blocks still queued are dropped) and finishes a line still going.
    pub fn stop(&self) {
        self.0.running.store(false, Ordering::SeqCst);
        let h = lock(&self.0.worker).take();
        if let Some(h) = h {
            if h.thread().id() != std::thread::current().id() {
                let _ = h.join();
            }
        }
        if let Some(rx) = lock(&self.0.rx).as_ref() {
            while rx.try_recv().is_ok() {}
        }
        self.flush();
    }

    pub fn started(&self) -> bool {
        self.0.running.load(Ordering::SeqCst)
    }

    /// A line is going on (the speech detector heard speech and it has not ended).
    pub fn talking(&self) -> bool {
        self.0.talking.load(Ordering::Relaxed)
    }

    pub fn models(&self) -> Arc<Models> {
        self.0.models.clone()
    }
}

/// The worker: pushed blocks through feed until stop() (or every handle is gone).
fn run(weak: Weak<Inner>, rx: Receiver<Vec<f32>>) {
    low_priority();
    loop {
        let Some(inner) = weak.upgrade() else { return };
        if !inner.running.load(Ordering::SeqCst) {
            *lock(&inner.rx) = Some(rx);
            return;
        }
        drop(inner); // (no handle kept while waiting: the last Listener dropped ends the worker)
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(x) => {
                let Some(inner) = weak.upgrade() else { return };
                if inner.running.load(Ordering::SeqCst) {
                    let mut st = lock(&inner.state);
                    inner.feed(&mut st, &x);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

impl Inner {
    fn feed(&self, st: &mut State, x: &[f32]) {
        let keep = (RATE as f64 * PREROLL) as usize;
        st.ring.extend_from_slice(x);
        if st.ring.len() > keep {
            let cut = st.ring.len() - keep;
            st.ring.drain(..cut);
        }
        if let Some(line) = st.line.as_mut() {
            match line.feed(x) {
                Ok(Some(words)) => {
                    let info = LineInfo { times: line.times.clone(), t0: line.t0 };
                    (self.cb.on_live)(st.utt, &words, &info);
                }
                Ok(None) => {}
                Err(e) => (self.log)(&format!("speech: {e}")),
            }
        }
        st.pending.extend_from_slice(x);
        let mut at = 0;
        while st.pending.len() - at >= st.win {
            st.vad.accept_waveform(&st.pending[at..at + st.win]);
            at += st.win;
            if st.line.is_none() && st.vad.detected() {
                st.utt += 1;
                let lang = lock(&self.lang).clone();
                let live = self.live.load(Ordering::Relaxed);
                st.line = Some(RollingLine::new(self.models.clone(), st.utt, &lang, st.ring.clone(), &st.last_lang, live));
                self.talking.store(true, Ordering::Relaxed);
                (self.cb.on_start)(st.utt);
            }
            while !st.vad.is_empty() {
                // (a finished segment: the line ends)
                st.vad.pop();
                self.end(st);
            }
        }
        st.pending.drain(..at);
        if st.line.as_ref().is_some_and(|l| l.speech > MAX_LINE + 1.0) {
            self.end(st);
        }
    }

    fn end(&self, st: &mut State) {
        let Some(mut line) = st.line.take() else { return };
        self.talking.store(false, Ordering::Relaxed);
        match line.finish() {
            Ok((text, info)) => {
                if !text.is_empty() && MIXED_LANGS.contains(&info.lang.as_str()) {
                    st.last_lang = info.lang.clone();
                }
                (self.cb.on_final)(line.utt, &text, &info);
            }
            Err(e) => {
                (self.log)(&format!("speech: {e}"));
                (self.cb.on_final)(line.utt, "", &line.failed()); // (the game's bubble still ends)
            }
        }
    }
}

/// The speech work yields to the game: this process below normal priority (its threads, the speech models'
/// worker threads too), so when they compete the live words fall behind instead of Teardown stuttering. The audio
/// output keeps its own (raised) priority (kd-audio's output thread).
pub fn low_priority() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS};
        SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS);
    }
}
