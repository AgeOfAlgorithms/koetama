# Kotodama in Rust: the crates and their interfaces

The Rust port of `engine/` (the Python app). The Python code stays as the REFERENCE: same behaviour, same constants,
same files for the game. `app/fixtures/make_fixtures.py` runs the Python code and writes its answers (JSON) next to
itself; each crate's tests compare against them. Do not change the Python side to fit the port.

    app/
      Cargo.toml              the workspace (shared dependency versions: [workspace.dependencies])
      fixtures/*.json         the Python answers (make_fixtures.py; --real adds the models on benchmark clips)
      crates/kd-common        names, folders, word units, model downloads, the Feed type          (engine/paths.py, fetch.py, asr.py text helpers)
      crates/kd-audio         the voice mixer, the muffle low-pass, wav, resampling, sound devices (engine/audio.py)
      crates/kd-speech        speech to text: models, VAD, language stitching, live words, mics  (engine/asr.py)
      crates/kd-games         the Game trait, game mod profiles, connectors (files, socket), Steam (engine/games/, steam.py)
      crates/kd-update        updates from GitHub Releases                                        (engine/updater.py)
      crates/kd-voice         real voices between players: the relay, encryption, Opus, jitter   (new: no Python)
      crates/kotodama         the program: runtime, window (egui), command line, selftest         (runtime.py, kotodama.py, teardown_helper.py)

Toolchain: Rust stable (MSVC on Windows). Engines: `sherpa-onnx` 1.13.8 (feature `shared`: sherpa-onnx-c-api.dll +
onnxruntime.dll 1.28.2, copied next to the build's exe) and `ort` 2.0.0-rc.13 with `load-dynamic`, loading THAT SAME
onnxruntime.dll (`ort::init_from(<exe dir>/onnxruntime.dll)`), so one ONNX Runtime ships. Verified: Parakeet through
sherpa and the language detector through ort give the Python numbers exactly.

Style: like the Python - short doc comments saying what a thing is for, constants named as in Python with their
Python comments, parenthetical "(why)" comments where the Python has them. Errors: `Result<_, String>` at crate
boundaries (messages a user can read), no panics on bad input from the game or the network.

## kd-common (done)

```rust
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;  stdout_log(), null_log()
paths::{APP_NAME, APP_ID, VERSION, REPO, data_dir(), models_dir(), settings_path(), home(), app_root(), repo_root()}
text::{WIDE, is_space, is_wide, units(&str) -> Vec<(usize, String)>, is_word_char, unit_key, tidy, round2,
       unit_times(text, &[String], &[f32], offset, Option<dur>) -> Vec<f64>}
fetch::{base_url(), hf_cache_dir(repo, rev), repo_files(repo, rev, &[&str], models_dir, log, progress) -> PathBuf,
        download(url, dest, progress(done, total), tries), get_text(url, accept, timeout)}
feed::{Feed {seq, vol, sid, ack, ping, mic, ptt, lang, live, speakers: BTreeMap<i64, Speaker>,
            room, key, me, to},                      // (version 5: the voice room; "" / 0 / [] without one)
       Speaker {src, talk, gain, az, el, muffle},   // src 0: a real player, id = their player id
       MAX_ID, MAX_TO, player_id(&str), voice_room(room, key, me) -> (room, key, me), voice_to(ids) -> Vec<i64>,
       trait FeedSink: Send + Sync { fn set_feed(&self, Feed); fn fresh(&self) -> bool }}
```
The crates below depend only on kd-common (not on each other), so they can be written at the same time; the program
(crates/kotodama) joins them: kd_audio::MixerSink is the FeedSink a game feeds, the test voices a game makes are wav
files the program loads with kd_audio::load_wav, and the real microphone (kd_audio::Input feeding a
kd_speech::Listener) is in the program too.

## kd-audio (engine/audio.py)

```rust
pub const RATE: u32 = 48000;  SMOOTH, STALE, CUT_CLEAR, CUT_MUFFLED, PAN, BEHIND_MUFFLE, BEHIND_QUIET, HEADROOM, LP_TAPS
pub fn lowpass_ir(a: f64) -> Vec<f64>;
pub fn lowpass(x: &[f32], hist: &mut Vec<f32>, a: f64) -> Vec<f32>;     // hist: the last LP_TAPS inputs
pub fn pan_gains(az: f64, el: f64) -> (f64, f64);
pub fn behind(az: f64) -> f64;
pub type Clip = Arc<Vec<f32>>;                                          // mono at RATE
pub trait Streams: Send { fn pull(&mut self, id: i64, out: &mut [f32]) -> bool; }   // real players' voices (kd-voice)
pub struct Mixer { pub clips: HashMap<i64, Clip>, pub streams: Option<Box<dyn Streams>>, pub volume: f64, .. }
impl Mixer {
    pub fn new(clips: HashMap<i64, Clip>) -> Mixer;                      // the real clock
    pub fn with_clock(clips, clock: Box<dyn Fn() -> f64 + Send>) -> Mixer; // seconds (tests: a fake clock)
    pub fn set_feed(&mut self, feed: Feed);
    pub fn fresh(&self) -> bool;                                         // a feed, at most STALE s old
    pub fn feed(&self) -> Option<&Feed>;
    pub fn feed_age(&self) -> f64;                                       // s since the last feed (huge if none)
    pub fn render(&mut self, frames: usize) -> Vec<[f32; 2]>;
    pub fn render_into(&mut self, out: &mut [f32]);                      // interleaved stereo: the audio callback
    pub fn voice_pos(&self, id: i64) -> Option<usize>;
    pub fn level(&self, id: i64) -> f64;
}
pub type SharedMixer = Arc<Mutex<Mixer>>;
pub struct MixerSink(pub SharedMixer);  impl FeedSink for MixerSink { .. }   // what a game feeds
pub fn read_wav(path) -> io::Result<(Vec<f32>, u32)>;   // 16-bit PCM (and 32-bit float) wav -> mono f32, its rate
pub fn write_wav16(path, x: &[f32], rate: u32) -> io::Result<()>;
pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32>;             // band-limited (rubato)
pub struct Rechunk;  Rechunk::new(from, to, chunk, ch)?.push(x, &mut |y| ..);   // the same, streaming (the microphone)
pub fn percentile(x: &[f32], q: f64) -> f64;                             // numpy's default (linear)
pub fn load_wav(path) -> io::Result<Vec<f32>>;                           // a mixer clip, as Python's load_wav
pub fn output_devices() -> Vec<String>;  pub fn input_devices() -> Vec<String>;   // names (cpal; WASAPI on Windows)
pub struct Output;  impl Output {
    pub fn open(mixer: SharedMixer, device: Option<&str>, log: Log) -> Result<Output, String>;  // 48 kHz stereo, small
    pub fn device_name(&self) -> String;  pub fn latency_ms(&self) -> f64;                     // blocks; falls back to the default
}
pub struct Input;  impl Input {
    pub fn open(device: Option<&str>, rate: u32, on_block: Box<dyn FnMut(&[f32]) + Send>, log: Log) -> Result<Input, String>;
    pub fn device_name(&self) -> String;                                 // mono at `rate`, ~50 ms blocks, any device rate
}
```
A speaker with src 0 is a real player: the mixer pulls their voice from `streams` (talk ignored; gain 0 or not in the
feed: not played, not even pulled), placed like a test voice. Devices are chosen by NAME (the window stores names). The output callback must never block on the game: it locks
the mixer, renders, unlocks. The audio thread runs at raised priority (Windows: MMCSS "Pro Audio" or
THREAD_PRIORITY_TIME_CRITICAL if cpal does not already): the speech work runs the process below normal priority.

## kd-speech (engine/asr.py)

```rust
pub const RATE: u32 = 16000;  PREROLL, MAX_LINE, VAD_URL, ROLL_EVERY, ROLL_SLOW, ROLL_MAX, LID_NAME, LID_WIN,
                               LID_MIN, LID_HOP, LID_QUIET, LID_SWITCH, LID_SURE
pub const MIXED_LANGS: [&str; 10];
pub struct ModelSpec { pub name, pub repo, pub revision, pub files: &'static [&'static str] }
pub const MODELS: [ModelSpec; 3];                                        // gigaam, sensevoice, parakeet: pinned as Python
pub fn roll_model(lang: &str) -> &'static str;
pub fn init_onnxruntime() -> Result<(), String>;   // ort::init_from the onnxruntime.dll next to the exe (once)
pub fn lid_dir() -> Result<PathBuf, String>;       // models_dir, app_root/models, repo_root/export/lid
pub struct Models;  impl Models {
    pub fn new(threads: usize, log: Log) -> Arc<Models>;
    pub fn downloading(&self) -> Option<(String, u64, u64)>;            // (file, done, total) while downloading
    pub fn model_dir(&self, name: &str) -> Result<PathBuf, String>;
    pub fn load(&self, name: &str) -> Result<(), String>;               // "parakeet" | "gigaam" | "sensevoice" | "langid"
    pub fn offline_full(&self, name, audio: &[f32]) -> Result<(String, Vec<String>, Vec<f32>, f64), String>;
    pub fn offline_timed(&self, name, audio: &[f32], offset: f64) -> Result<(String, Vec<f64>, f64), String>;
    pub fn lid_probs(&self, x: &[f32]) -> Result<Vec<f64>, String>;    // over MIXED_LANGS
    pub fn lid_probs_in(&self, x: &[f32], langs: &[String]) -> Result<Vec<f64>, String>;  // over the player's
    pub fn unload(&self, name: &str) -> bool;  pub fn loaded(&self) -> Vec<String>;
    pub fn state(&self, name: &str) -> ModelState;                    // NotLoaded | Downloading(f, done, total) | Loading | Loaded
    pub fn every(&self) -> f64;  pub fn set_every(&self, s: f64);       // the live-words interval (slow PC: longer)
}
pub struct Lang { code, name, english, tier: Tier }  pub enum Tier { Full, Soft, Weak }  pub const LANGS: [Lang; 29];
pub fn plan(langs: &[String]) -> (String, Vec<String>);  pub fn models_for(langs: &[String]) -> Vec<&'static str>;
pub const MODEL_INFO: [ModelInfo; 4];   // name, title, memory_mb (for the window)
pub fn segments_in(langs: &[String], x, fallback, cache, probs) -> ...;   // segments among the player's languages
pub fn segments(x: &[f32], fallback: &str, cache: Option<&mut HashMap<i64, Vec<f64>>>,
                probs: &mut dyn FnMut(&[f32]) -> Result<Vec<f64>, String>) -> Result<Vec<(String, f64, f64)>, String>;
pub fn quiet_point(x: &[f32], t: f64, span: f64) -> f64;
pub struct Mixed { pub text: String, pub langs: Vec<String>, pub segs: Vec<(String, f64, f64)>, pub times: Vec<f64>, pub took: f64 }
pub fn transcribe_mixed(models: &Models, x: &[f32], fallback: &str, cache: Option<&mut HashMap<i64, Vec<f64>>>) -> Result<Mixed, String>;
pub struct RollingLine { pub committed: Vec<(String, String, f64)>, .. }   // (space before, unit, time)
impl RollingLine { pub fn commit(&mut self, text: &str, times: &[f64]) -> bool; pub fn for_test() -> RollingLine; .. }
pub struct LineInfo { pub times: Vec<f64>, pub t0: Instant }
pub struct FinalInfo { pub live: String, pub used: String, pub lang: String, pub speech: f64, pub second_s: f64,
                       pub times: Vec<f64>, pub t0: Instant, pub finish_s: f64, pub live_cpu: f64, pub passes: u32 }
pub struct Callbacks { pub on_start: Box<dyn Fn(u32) + Send + Sync>,
                       pub on_live: Box<dyn Fn(u32, &str, &LineInfo) + Send + Sync>,
                       pub on_final: Box<dyn Fn(u32, &str, &FinalInfo) + Send + Sync> }
#[derive(Clone)] pub struct Listener;  impl Listener {                    // a handle (Arc inside)
    pub fn new(cb: Callbacks, models: Arc<Models>, live: bool) -> Result<Listener, String>;  // makes the VAD
    pub fn set_language(&self, lang: &str);  pub fn language(&self) -> String;  pub fn set_live(&self, on: bool);
    pub fn set_languages(&self, langs: &[String]);  pub fn languages(&self) -> Vec<String>;   // the player's: one, or "auto" among them
    // warm(None): load what the languages need, let go of the rest
    pub fn warm(&self, lang: Option<&str>) -> Result<(), String>;      // blocking: load what a language needs
    pub fn feed(&self, x: &[f32]);  pub fn flush(&self);               // synchronous (--transcribe, tests)
    pub fn push(&self, x: &[f32]);                                     // the mic callback: queued, dropped when far behind
    pub fn start(&self);  pub fn stop(&self);  pub fn started(&self) -> bool;   // the worker thread (low priority)
    pub fn talking(&self) -> bool;  pub fn models(&self) -> Arc<Models>;
}
pub trait Mic { fn is_open(&self) -> bool; fn open(&mut self) -> bool; fn close(&mut self); fn level(&self) -> f64; }
                                // (the real microphone, kd_audio::Input at 16 kHz -> Listener::push, is in the program)
pub struct WavMicrophone;       // WavMicrophone::new(listener, audio_16k, log)            (real time, then quiet)
pub struct PlaylistMicrophone;  // PlaylistMicrophone::new(listener, items: Vec<(lang, audio_16k, said)>, gap, log)
pub fn low_priority();          // this process below normal (Windows), as Python
```

## kd-games (engine/games/, engine/steam.py)

```rust
pub trait Game: Send {
    fn id(&self) -> &'static str;  fn name(&self) -> &'static str;  fn needs(&self) -> &'static str;
    fn locate(&self) -> (bool, String);
    fn start(&mut self);  fn stop(&mut self);
    fn send(&self, kind: char, utt: u32, text: &str, times: Option<&[f64]>, t0: Option<Instant>) -> bool;
    fn send_text(&self, text: &str) -> bool;                             // a typed line (--type, --auto)
    fn test_voices(&self) -> HashMap<i64, PathBuf>;                      // wav files (the program loads them)
    fn speaker_name(&self, src: i64) -> String;
    fn feed(&self) -> Option<Feed>;                                      // the latest
    fn connected(&self) -> bool;                                         // the mixer's feed is fresh
    fn wants_mic(&self) -> bool;  fn language(&self) -> String;  fn live_words(&self) -> bool;  // from feed()
    fn updates(&self) -> u64;                                            // live feeds read
    fn describe(&self) -> Vec<String>;                                   // "reading <save>", "my files ... go to: <dirs>"
}
pub struct GameKind;  pub fn games();  pub fn by_id();  ...                // see "Game mod profiles" below
pub mod steam { steam_root, libraries, app_library, install_dir, workshop_dir, proton_user, documents_dir }
pub mod profile { Profile, Connector, FilesConfig, SocketConfig, MessageFormat, TestVoice, PathTemplate, PathSpec,
                  Place, safe_prefix, this_pc }                          // the profile format, validation, summary
pub mod files { FilesGame, Link, LinkRules, FeedReader, FeedScan, FeedRules, parse_feed, find_feeds, text_prefab,
                json_message, times_hex }                               // the files connector
pub mod socket { SocketGame, parse_socket_feed, PROTOCOL, MAX_LINE }   // the socket connector
pub mod voices { make_in, for_profile }                                 // test voices (Windows SAPI)
pub mod teardown { APPID, TEXT_MAX, parse_feed, find_feeds, read_shared, FeedReader, times_hex, text_prefab, Link,
                   make_voices, VOICES, NAMES, savegame_path, io_dirs, Teardown (= FilesGame), profile() }
```
Env overrides as Python: SAVEPROBE_DIR (the feed file's folder; the file keeps the profile's name), HFP_MODS (the
only output folder) - test_e2e.py uses them; KOTODAMA_PROFILES_DIR (tests: the profiles folder).

## kd-update (engine/updater.py)

```rust
pub fn version_tuple(v: &str) -> Vec<u32>;  pub fn newer(latest: &str, current: &str) -> bool;
pub struct Release { pub version, pub notes, pub installer: Option<String>, pub installer_url: Option<String>,
                     pub sums_url: Option<String>, pub page: String }
pub fn parse_release(json: &str, current: &str) -> Option<Release>;   // None: not newer, a draft, a prerelease
pub fn check(timeout: Duration) -> Result<Option<Release>, String>;
pub fn expected_sha(sums: &str, name: &str) -> Option<String>;  pub fn sha256_file(path) -> io::Result<String>;
pub fn signer(path) -> (Option<String>, Option<String>);              // Windows Authenticode (status, subject)
pub fn download(r: &Release, progress: &dyn Fn(f64)) -> Result<PathBuf, String>;  // checksum + same publisher
pub fn install(path) -> io::Result<()>;                                // /VERYSILENT ... /RELAUNCH=1, detached
pub const PAGE: &str;  pub fn api_url() -> String;
```

## kotodama (the program)

`kotodama` (window), `kotodama --cli [teardown_helper.py's flags]`, `kotodama --selftest`. Runtime as runtime.py
(start / tick / stop / status), the window as kotodama.py (egui), the command line as teardown_helper.py - the same
flags, so `engine/test_e2e.py` with `KOTODAMA_EXE=<the Rust exe>` tests the port end to end.

## kd-voice (real voices; PROTOCOL.md "Real voices: rooms and the relay (version 5)")

No Python counterpart: the reference for the wire is the contract and the relay's own code (`relay/src/frames.js`).

```rust
pub const RELAY: &str;  pub fn relay_url() -> String;    // KOTODAMA_RELAY overrides
pub const RATE: u32 = 48000;  FRAME = 960 (20 ms);  PER_PACKET = 3;  PACKET;  BITRATE = 24000
pub mod frames { VOICE, MAX_TO, MAX_PAYLOAD, voice_frame(to, payload) -> Option<Vec<u8>>, parse_out(b) -> Option<(from, &[u8])>,
                 route(b, from) -> Option<(to, out)> /* the relay's rule: the tests' stand-in relay */, parse_id }
pub mod crypto { new_room() -> Result<"<32 hex>:<64 hex>">, key_from_hex, seal(key, from, plain), open(key, from, payload) -> Option }
pub mod packet { Packet { seq: u32, last: bool, frames: Vec<Vec<u8>> }: encode(), decode(&[u8]) -> Option }
pub mod codec  { Encoder::new()?.encode(&[f32; FRAME]) -> Vec<u8>; Decoder; trait FrameDecoder { decode(Option<&[u8]>, out) } }
pub mod gate   { PTT_TAIL, PREROLL; enum Mode { Off, PushToTalk(held), Detector(talking) }; Gate::push(x, mode) -> Gated { audio, end } }
pub mod jitter { START, MAX, END_AFTER; Jitter<D: FrameDecoder>::insert(Packet, now); pull(out, now) -> bool }
pub mod relay  { Conn::open(relay, room, me)?; send(frame); ping(); poll(&mut Vec<frame>); close() }
pub struct Sender;  Sender::new()?.push(x_48k, Mode) -> Vec<Packet>;   // gate + Opus + packets
#[derive(Clone)] pub struct Voice;  impl Voice {                         // a handle; the "voice" thread inside
    pub fn start(relay: String, log: Log) -> Voice;
    pub fn set_feed(&self, &Feed);                  // the room, to, whom it hears (src 0, gain > 0), mic / ptt
    pub fn push_mic(&self, x_48k: &[f32], talking: bool);   // the microphone's callback: queued, never waits
    pub fn playback(&self) -> Playback;             // impl kd_audio::Streams: the mixer pulls each sender
    pub fn status(&self) -> VoiceStatus { state: "off" | "connecting" | "connected", heard };
    pub fn stop(&self);
}
```
Threads: the voice thread connects while the feed names a room and the game is feeding (again after a drop: 1, 2, 4
... 30 s), reads the relay (5 ms read timeouts), pings every 20 s, and turns the microphone's queued blocks into
packets (sent only to the feed's `to`). Received packets go into a jitter buffer per sender under one short lock;
the audio output's callback decodes as it pulls. The program: the runtime makes one Voice for a game that plays
voices (Sink::set_feed hands it each feed; tick() sends the game kind 'r' once per session), the microphone runs at
48 kHz for it (kd_audio::Rechunk makes the listener's 16 kHz). Tests: unit tests per module; `tests/relay.rs` runs
four Voices through a stand-in relay (and, `--include-ignored`, the live one). Crates: opus-rs (pure Rust: no
CMake, nothing linked; unsafe-libopus was tried and dropped - broken SILK concealment), chacha20poly1305, getrandom,
tungstenite + rustls (ring) + webpki-roots.

## Building and testing a crate (parallel work)

Each crate builds and tests on its own: `cargo test -p <crate>` from `app/`. While several people (agents) work at
once, each uses its own target folder (`CARGO_TARGET_DIR=app/target/<crate>`) so builds do not wait on each other.
Tests that need the real models (the Hugging Face cache on this PC, export/ from the benchmarks) are `#[ignore]`d and
run with `cargo test -p <crate> -- --include-ignored`.

## Game mod profiles (2026-10-06: the user - games are added without code review)

A game mod is a PROFILE (a small JSON file), never code: Kotodama's built-in CONNECTORS do the talking, a profile
only names one and gives its settings. Built-in profiles (Teardown) are compiled in; others are `*.json` files in
`kd_games::profiles_dir()` (= `paths::data_dir()/games`), added from the window ("Add game mod...": a preview of what
the profile reads, writes and listens on, then a copy into that folder) or dropped there by hand.

Connectors:
- `files`: the link PROTOCOL.md describes (Teardown's): a feed string read from a file (a regex finds it; the tag of
  the mod copy that wrote it chooses the output folder), message files written next to the mod. Paths take
  placeholders: {documents} {localappdata} {home} {steam_app:ID} (install folder) {steam_workshop:ID}
  {proton_user:ID} {env:NAME}; an entry may be a list of candidates (the first that resolves and exists is used).
  It deletes only the files it writes (`<prefix>on`, `<prefix>p<n>`, `<prefix>t<n>.<ext>`, `<prefix>w<n>.tmp`), the
  prefix must be 3+ letters/digits/_ ending in `_`, and it writes only into folders that exist.
- `socket`: a TCP server on 127.0.0.1 (the profile's port): newline-separated JSON both ways; the mod sends its
  feed, Kotodama sends what the player said (no acks or pings: the connection is the liveness).

```rust
// kd-games (the interface the window and the runtime use)
#[derive(Clone)] pub struct GameKind {
    pub id: String, pub name: String, pub mod_name: String, pub mod_url: String, pub author: String,
    pub needs: String,                 // "the Proximity Babble Chat mod" (the waiting line)
    pub builtin: bool,                 // compiled in (else a profile file)
    pub source: Option<PathBuf>,       // the profile file
    pub summary: Vec<String>,          // what it does, for the import preview: "reads <file>", "writes into <dir>",
                                       // "listens on 127.0.0.1:<port>" (placeholders already resolved here)
    pub voices: bool,                  // "uses": "voices" - Kotodama plays the speakers from the feed (audio output)
    pub speech: bool,                  // "uses": "speech" - Kotodama listens to the mic, sends what was said
    pub profile: Arc<Profile>,         // the parsed profile
}
impl GameKind { pub fn make(&self, sink: Arc<dyn FeedSink>, log: Log, io_dir: Option<PathBuf>) -> Box<dyn Game>; }
pub fn games() -> Vec<GameKind>;                                   // built-ins, then the profiles folder's valid ones
pub fn by_id(id: &str) -> GameKind;                                // unknown: the first
pub fn profiles_dir() -> PathBuf;
pub fn load_profile(path: &Path) -> Result<GameKind, String>;      // parse + validate (the preview)
pub fn install_profile(path: &Path) -> Result<GameKind, String>;   // validate, copy into profiles_dir (replaces the same id)
pub fn remove_profile(id: &str) -> Result<(), String>;             // a profile file's game (not a built-in)
pub fn bad_profiles() -> Vec<(PathBuf, String)>;                   // files in the folder that do not load, and why
```
games()/by_id()/bad_profiles() are cached until the folder's *.json files change (cheap every frame; the first call
resolves the placeholders: registry + Steam's .vdf files, a few ms). make() is cheap; Game::start() starts the
connector's thread; Game::test_voices() blocks (PowerShell makes missing wavs, seconds each). A profile's "uses"
lists "voices" and/or "speech" (default both): a speech-only game's feed has no speakers, a voices-only game's feed
never asks for the microphone (the connectors enforce both).
