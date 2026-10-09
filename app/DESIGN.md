# Koetama in Rust: the crates and their interfaces

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
      crates/kd-translate     chat translation: Mozilla's models, language detection, the translator (engine/mt.py)
      crates/koetama         the program: runtime, window (egui), command line, selftest         (runtime.py, koetama.py, teardown_helper.py)

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
            room, key, me, to, region,               // (the voice room; "" / 0 / [] without one)
            translations: Vec<(from, to)>, to_translate: Vec<(id, text)>},   // ([] without)
       Speaker {src, talk, gain, az, el, muffle},   // src: a test voice (0: a real player, id = their player id)
       MAX_ID, MAX_TO, voice_room(room, key, me) -> (room, key, me), voice_to(ids) -> Vec<i64>, voice_region,
       MAX_TRANSLATIONS, MAX_REQUESTS, MAX_REQUEST_BYTES, MAX_REQUEST_ID, lang_code, translation_pairs(pairs),
       translate_requests(items), request_text,
       RuleState { from, to, state: "ready" | "downloading" | "loading" | "unavailable" | "error", progress },
       translations_wire(&[RuleState]) -> "ja>en=ready,ko>en=downloading 42",   // (the status line)
       trait FeedSink: Send + Sync { fn set_feed(&self, Feed); fn fresh(&self) -> bool }}
```
The crates below depend only on kd-common (not on each other), so they can be written at the same time; the program
(crates/koetama) joins them: kd_audio::MixerSink is the FeedSink a game feeds, the test voices a game makes are wav
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
                                                                         // api::speech: 's' 'l' 'f', 'r' the room
    fn send_text(&self, text: &str) -> bool;                             // a typed line (--type, --auto)
    fn send_translation(&self, id: i64, text: &str, rule: Option<(&str, &str)>) -> bool;
                                                                         // api::translation (one per line id; from / to)
    fn send_translations_state(&self, states: &[RuleState]) -> bool;     // api::translations_status (on each change)
    fn set_standing(&self, kind: &'static str, object: String);          // a "standing" object (voice, status): sent now
                                                                         // and again after every hello (new session /
                                                                         // connection / hub link)
    fn send_object(&self, object: String) -> bool;                       // any other object (talking, join_code, player,
                                                                         // a player's objects from the hub)
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
pub mod api { PROTOCOL = 2, parse_feed(&Value) -> Feed, feed_from_text(object or its hex), features(profile),
              player_id(&Value) -> Option<PlayerId>, whole, MAX_PLAYERS = 32,
              hello, speech, room, voice(state, players), talking(id, on), status(speech, microphone),
              translation(id, text, from/to), translations_status, join_code, player, from_player(object, id),
              object_prefab, json_secs }
                                                                        // the game API (PROTOCOL.md "The objects"): one
                                                                        // parser and the objects, for both connectors
pub mod files { FilesGame, Link, LinkRules, FeedReader, FeedScan, FeedRules, parse_feed, find_feeds, TRANSLATION_MAX }
                                                                        // the files connector (object n of a session
                                                                        // in <prefix>t<n>: json or a prefab; 1 = hello)
pub mod socket { SocketGame, parse_socket_feed, PROTOCOL, MAX_LINE }    // the socket connector
pub mod joined { JoinedGame, profile() }                                // a game hosted on another PC: the "game" is
                                                                        // the host's Koetama, through the relay (Hub)
pub mod http { HttpGame, MAX_WAIT = 1 s, MAX_BODY = 64 KB }            // the HTTP connector: POST / the feed, the answer
                                                                        // {"objects":[..],"last":n}; objects kept until
                                                                        // acked; "wait" holds the answer; Origin allowlist
pub mod voices { make_in, for_profile }                                 // test voices (Windows SAPI)
pub mod teardown { APPID, TEXT_MAX, parse_feed, find_feeds, read_shared, FeedReader, Link,
                   make_voices, VOICES, NAMES, savegame_path, io_dirs, Teardown (= FilesGame), profile() }
```
Env overrides as Python: SAVEPROBE_DIR (the feed file's folder; the file keeps the profile's name), HFP_MODS (the
only output folder) - test_e2e.py uses them; KOETAMA_PROFILES_DIR (tests: the profiles folder).

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

## koetama (the program)

`koetama` (window), `koetama --cli [teardown_helper.py's flags]`, `koetama --selftest`. Runtime as runtime.py
(start / tick / stop / status), the window as koetama.py (egui), the command line as teardown_helper.py - the same
flags, so `engine/test_e2e.py` with `KOETAMA_EXE=<the Rust exe>` tests the port end to end.

## kd-voice (real voices; PROTOCOL.md "Real voices")

No Python counterpart: the reference for the wire is the contract and the relay's own code (`relay/src/frames.js`).

```rust
pub const RELAY: &str;  pub fn relay_url() -> String;    // KOETAMA_RELAY overrides
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
    pub fn status(&self) -> VoiceStatus { state: "off" | "connecting" | "connected" | "id_taken", heard, players };
    pub fn players(&self) -> Vec<PlayerId>;         // heard from (voice or presence) within PRESENT = 15 s
    pub fn range_of(&self, rid: i64) -> Option<(f64, f64)>;   // the range a player's packets announce
    pub fn take_events(&self) -> Vec<VoiceEvent>;   // VoiceEvent::Talking { id, talking } (remote and this player)
    pub fn stop(&self);
}
pub mod hub { HUB = 1, PLAYER = 2, MESSAGE = 3, PART = 3500, ALPHABET, new_code, normalize_code, pairing_room,
              parts, Assembler, Channel::start(relay, code, me, log) -> Option<Channel>: send(&Value), received(),
              other_there(), connected() }
```
Ids: a player id is a number or a string (`kd_common::feed::PlayerId`, at most 64 characters / 255 bytes); its
relay id is `feed::relay_id(room, id)` (the first 2 bytes of SHA-256("koetama id:" room ":" id), 0 -> 65535). Two
players with the same relay id: the relay keeps the newer connection and closes the older with 4000 (`replaced()`),
which stays out (`id_taken`). `room_seed` makes room and key (`feed::room_from_seed`: HMAC-SHA256). Packet v2:
`[2][seq][flags: 1 last, 2 presence, 4 the id is a number][near f32][far f32][n][id][k][frames]` - every packet
says who sent it and their range; a presence packet (no frames) goes every PRESENCE_EVERY = 5 s to the speakers and
`to`, so `players()` knows who is in the room before they talk. Talking events: a remote player starts with their
first audio and stops on the last flag (or 0.5 s without audio); this player with sending.

Positions (`api::place`): with `listener` and a speaker's `position`, Koetama computes azimuth / elevation (from the
listener's right / up / forward) and the gain (`feed::falloff`: 1 within near, ((far - d) / (far - near))^2 out to
far), the range being the one the player's packets announce, else the speaker's `range`, else the feed's, else
DEFAULT_RANGE (10, 30); a `gain` the game gives wins. With no `to`, it is every speaker within far x 1.1.

Hub (`koetama::hub::Hub`, the host's side; `kd_games::joined::JoinedGame`, a player's): the host's feed lists
`players` (each a feed; api::parse_feed keeps them in `Feed::players`, each with `raw` - its JSON merged with the
host's room / key / region when it has none). For each, the hub makes a join code and a `Channel` (HUB) in the room
the code makes; it tells the game the code (`join_code`, once per session), sends `{"feed": ..}` on each change and
every 0.5 s, and hands the game each `{"objects": [..]}` the player's Koetama sends with `"player"` added
(api::from_player), plus `player` joined / left (other_there: heard within 5 s; the player's Koetama sends an empty
`objects` every 2 s). The player's JoinedGame is a Game like any other: its feed comes from the hub, its objects go
there, its standing objects are sent again when the link comes up. Messages are cut into parts of 3500 bytes
(`[3][msg u32][part][parts][bytes]`, encrypted like voice packets, relay ids 1 and 2).
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

A game mod is a PROFILE (a small JSON file), never code: Koetama's built-in CONNECTORS do the talking, a profile
only names one and gives its settings. Built-in profiles (Teardown) are compiled in; others are `*.json` files in
`kd_games::profiles_dir()` (= `paths::data_dir()/games`), added from the window ("Add game mod...": a preview of what
the profile reads, writes and listens on, then a copy into that folder) or dropped there by hand.

Connectors:
- `files`: the link PROTOCOL.md describes (Teardown's): a feed string read from a file (a regex finds it; the tag of
  the mod copy that wrote it chooses the output folder), message files written next to the mod. Paths take
  placeholders: {documents} {localappdata} {home} {steam_app:ID} (install folder) {steam_workshop:ID}
  {proton_user:ID} {env:NAME}; an entry may be a list of candidates (the first that resolves and exists is used).
  It deletes only the files it writes (`<prefix>on`, `vc`, `vx`, `v<n>`, `p<n>`, `t<n>.<ext>`, `w<n>.tmp`), the
  prefix must be 3+ letters/digits/_ ending in `_`, and it writes only into folders that exist.
- `http`: an HTTP/1.1 server on 127.0.0.1 (the profile's port; std only, Content-Length bodies, Connection: close, a
  thread per request, at most 16 at once): POST / takes the feed and answers the session's objects not yet acked
  (numbered per session, the hello first; a new session - or Koetama started mid-session - numbers after the game's
  ack), waiting up to `wait` (<= 1 s) on a Condvar; GET / says what listens. A request with an Origin header is
  refused unless the profile's allow_origins lists it (then CORS + Private Network Access headers).
- `socket`: a TCP server on 127.0.0.1 (the profile's port): newline-separated JSON both ways; the mod sends its
  feed, Koetama sends its hello (with the features the profile uses), what the player said, the voice room and the
  voice chat's state, and translations (api.rs; no acks or pings: the connection is the liveness).
- (no profile) a JOINED game (`GameKind::joined(code)`, the window's "Join a hosted game", `--join CODE`): see Hub.

```rust
// kd-games (the interface the window and the runtime use)
#[derive(Clone)] pub struct GameKind {
    pub id: String, pub name: String, pub mod_name: String, pub mod_url: String, pub author: String,
    pub needs: String,                 // "the Proximity Babble Chat mod" (the waiting line)
    pub builtin: bool,                 // compiled in (else a profile file)
    pub source: Option<PathBuf>,       // the profile file
    pub summary: Vec<String>,          // what it does, for the import preview: "reads <file>", "writes into <dir>",
                                       // "listens on 127.0.0.1:<port>" (placeholders already resolved here)
    pub voices: bool,                  // "uses": "voices" - Koetama plays the speakers from the feed (audio output)
    pub speech: bool,                  // "uses": "speech" - Koetama listens to the mic, sends what was said
    pub translate: bool,               // "uses": "translate" - Koetama translates the chat lines the feed sends
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
lists "voices", "speech" and/or "translate" (default: voices and speech): a speech-only game's feed has no speakers,
a voices-only game's feed never asks for the microphone, a game without "translate" has no translations or lines to translate (the
connectors enforce all three). A translate-only game needs no microphone, no sound output and no relay.

## kd-translate (PROTOCOL.md "Translation"; engine/mt.py is the engine's reference)

Chat translation on the player's PC with Mozilla's Firefox Translations models (MPL-2.0, ~20-55 MB a direction,
downloaded the first time a rule needs them). Four modules:

```rust
pub mod engine { Model::load(dir) -> Result<Model, String>; model.translate(text) -> Result<String, String>; bytes() }
                                       // one direction (Marian, 8-bit) in plain Rust: below
pub mod catalog {                      // Mozilla's list and the downloads
    RECORDS_URL, CDN_URL, LIST_MAX_AGE (a day), LIST_FILE, root() -> data_dir()/translate,
    mozilla_code(lang, source) -> Option<&str>,    // zh -> zh-Hans; yue -> zh-Hant as a SOURCE only; else the same
    numeric_version("2.1") -> Some([2, 1]),        // pre-releases ("1.0a1"): None - used only by a direction with
                                                   // no release at all (Maltese -> English)
    ModelFile { name, kind, size, sha256, location } .url(),
    Direction { from, to, version, files } .key() "ja-en", .id() "ja-en/2.1", .dir(root), .bytes(), .present(root),
    Catalog::parse(json)?; .direction(from, to) -> newest numeric whole version (else newest pre-release); .route(from, to) -> Result<Vec<Direction>, why>
                                                   // one direction with English, two through it
    load_list(root, max_age, get, log) -> Catalog  // kept in <root>/models.json, fetched at most daily, offline: the kept one
    fetch_route(root, &[Direction], progress(done, total), download, log) -> folders
                                                   // <root>/<from>-<to>/<version>/<own name>, via <name>.dl, sha256 checked,
                                                   // reused when there, the direction's other versions deleted
    get_list, download_file (kd_common::fetch), sha256_file }
pub mod detect {                       // which language each stretch of a line is in
    LANGS (Koetama's 29), MIN_WORDS, SPLIT_WORDS, SPLIT_SURE,
    stretches(line) -> Vec<Stretch { start, end, lang: Option<&str> }>,  // in order, covering the line
    detect(text) -> Option<&str>,                  // the main language (stretches weighed by their letters)
    no_space(c) }                                  // Chinese / Japanese: no space when stretches are joined
pub mod service {                      // the translator
    trait Engine: Send + Sync { translate(&self, text) },   // Model (behind a lock) or a test's fake
    trait Provider: Send + Sync { prepare(from, to, progress) -> Prepared, load(id, dir) -> Arc<dyn Engine> },
    Mozilla::new(root, log),                       // the real Provider: catalog + engine::Model
    enum Prepared { Ready(Vec<(id, dir)>), Unavailable(why), Error(why) },
    enum State { Ready, Downloading(0..1), Loading, Unavailable, Error }  .wire() "downloading 42", .word()
    RuleStatus { from, to, state } .wire(), .common() -> kd_common::feed::RuleState;  status_text(&[RuleStatus]),
    enum Event { Reply { id, text }, Status(Vec<RuleStatus>) },
    #[derive(Clone)] Translator::start(provider, on_event, log) / Translator::mozilla(on_event, log);
        .set_rules(&[(from, to)])  .request(id, text) -> bool (false: id already queued / answered)
        .new_session()  .status()  .stop()
    MAX_RULES = 2, STATUS_EVERY = 0.5 s, RETRY_AFTER = 60 s, join_pieces, percent }
```

Detection (detect.rs): the line is cut by script (Han, kana, Hangul, Latin, Cyrillic, Greek, other; punctuation,
digits and spaces go with the neighbouring stretch: up to the first space with the one before, opening brackets and
quotes with the one after). Kana anywhere in the line makes its Han Japanese; a character only written Cantonese uses
(嘅 咗 喺 哋 ...) makes it Cantonese; else Han is Chinese; Hangul Korean, Greek Greek, other scripts unknown. Latin and
Cyrillic text: whatlang (MIT; trigram profiles, restricted to Koetama's languages), with letters that rule a close
neighbour out (ě ř ů: not Slovak; ы э: not Ukrainian; ñ: not Portuguese, ...), the little words of a short line when
the trigrams are unsure, a Croatian / Slovene word vote, and Maltese - which whatlang does not know - by ħ ċ ġ, "għ"
or its hyphenated articles. A Latin / Cyrillic stretch under MIN_WORDS words, or (with other scripts in the line)
mostly capitalised words - names, brands - takes the line's main language. Within one script, a clause (cut after
". ! ? ; : ," and a space) becomes a stretch of its own only when it has SPLIT_WORDS lower-case words, the trigrams
are SPLIT_SURE sure, the little words agree, and its language is not KIN to the run's (es/pt/it/fr/ro/mt, cs/sk/pl,
hr/sl, ru/uk/bg, da/sv, fi/et, lv/lt, de/nl). Measured on the NTREX-128 sentences of bench/mt/data (100 per language,
2900): 98.5 % come out as one stretch in the right language (the main language right: 98.6 %); Cantonese 93 % (lines
without a Cantonese-only character read as Chinese - the translator lets Chinese and Cantonese stand in for each
other), Croatian 94 %, Slovene 94 %, the rest 96-100 %.

The translator (service.rs): one thread; a rule's models are got ready on a helper thread (Provider::prepare: the
list, the downloads - one rule at a time), then loaded on the translator's thread (shared by rules through the same
direction, let go when no rule uses them). A request: detect::stretches; a stretch in a ready rule's source language
(the first rule from it; Chinese / Cantonese stand in for each other when only one has a rule) goes through the rule's
model(s); the rest are kept; pieces are joined again (two kept pieces as they were; next to a translated one a space,
unless either side is Chinese or Japanese). "" when nothing was translated or the result is the line itself. A line
that comes while a rule's models are downloading or loading is held (in order) until none is, or HOLD_MAX (120 s) -
an early "" would lose it: a game asks once per line. Exactly one reply per id; a new session forgets the ids and drops (never answers) what is still queued from the old one, even
a line being translated when it changed. States are told on each change, a download's progress at most every 0.5 s;
a failed rule is tried again after RETRY_AFTER; rules Koetama can never have (the same language twice, a code it does
not know, into Cantonese) are "unavailable" without asking Mozilla's list.

The program (runtime.rs): a game kind with `translate` gets a Translator::mozilla. Its Sink hands each feed's rules
and requests to it as the feed is read (a new session in the feed: new_session first); the files connector runs the
Link before the Sink, so a new session's files are set up before a reply can be written. Replies go to the game at
once (Game::send_translation), the states on each Status event (Game::send_translations_state) and again in tick() when the
game is in a session they were not told in (none told while there are no rules). Runtime::stop stops the translator
before taking the game's lock (its thread may be waiting on that lock to send a reply). The window shows a
"Translation" card (each rule: "Japanese → English" and its state); the command line's status line "translate: ja →
en ready, ko → en 42 %". Tests: tests/catalog.rs (a canned list, a fake CDN), tests/detect.rs (NTREX, chat lines,
mixed lines), tests/service.rs (a fake engine and model source), kd-games tests/translation.rs and tests/feed.rs
(against the Python fixtures).

### The engine (`engine.rs`, `engine/gemm.rs`, `engine/marian.rs`, `engine/spm.rs`)

Mozilla's models are Marian "transformer" students (each file's own config, special:model.yml): an encoder of 6
layers (self-attention, feed-forward, each "dan": add, layer-norm), a decoder of 2 or 4 layers whose self-attention
is an SSRU (a gated running sum: no growing cache), attention over the encoder, a feed-forward; width 384 (or 256 for
the "tiny" ones), 8 heads, feed-forward 1536, the output layer tied to the target embeddings over the lexical
shortlist (the 100 commonest words plus each source word's best 50). Greedy, at most 2x the source's tokens.

- `marian.rs`: the binary model (u64 version, count, headers, names, shapes, padding, data; 8-bit tensors stored
  transposed - [out][in] - with their float multiplier after them; `<name>_QuantMultA` = the input's alpha) and the
  binary shortlist.
- `gemm.rs`: intgemm's arithmetic - the input quantized with the file's alpha (rounded half to even, clipped to
  +-127), integer dot products, divided back by alpha x the weights' multiplier. AVX2 when the CPU has it
  (`maddubs(|w|, sign(x, w))`: right for the weights' -128 too), plain Rust otherwise; the same integers either way.
- `spm.rs`: SentencePiece unigram, from the .spm protobuf: the precompiled nmt_nfkc map (a Darts trie), extra spaces
  removed, U+2581 for spaces and one in front, the best split by score, unknown characters as UTF-8 byte pieces;
  decoding back. Matches Python's sentencepiece on every fixture line (`app/fixtures/mt.json`, made by
  `make_mt_fixtures.py`), byte for byte.
- `engine.rs`: the model in memory stays 8-bit, the embeddings too (a row is turned into floats when looked up), so
  a direction holds about its files' size. `translate(line)` cuts the line into sentences (. ! ? and 。！？), a
  sentence longer than 128 tokens at word starts, and joins the results (no space after / before CJK).

Tested against engine/mt.py (`tests/engine.rs`; skipped without `bench/mt/models`): the same source ids and
shortlists always; 21 of 28 translations token for token the same - the others part at a near tie, where a value a
hair from a rounding boundary (17.499996) rounds the other way than numpy's summation order gives; Mozilla's own
WASM build agrees with mt.py on about half the sentences for the same reason.

Measured (`examples/mt_bench.rs` on bench/mt's NTREX sentences, 100 per direction, one thread, Ryzen 7 3700X,
scored by `bench/mt/score_rust.py` -> `bench/mt/RUST.md`):

| | chrF Rust | chrF Mozilla (WASM) | ms a sentence (p90) | Mozilla WASM | load | memory |
|---|---|---|---|---|---|---|
| with English (54 directions) | 55.1 | 55.2 | 20 (36) | 52 | 27 ms | ~40 MB |
| through English (16 pairs) | 40.3 | 40.4 | 44 (78) | 114 | 64 ms | ~90 MB |

(News sentences, ~25 words; chat lines are shorter: 5-25 ms. `examples/translate_live.rs` runs the translator with
the real downloads end to end.)

