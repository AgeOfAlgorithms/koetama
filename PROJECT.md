# Koetama (repo github.com/AgeOfAlgorithms/koetama; until 2026-10-07 proximity-voice-chat-STT-engine)

**Koetama** (声魂, Japanese "voice spirit"; the user's pick, 2026-10-08 - it was **Kotodama**, "word spirit", until a search showed Kotodama AI, a voice-cloning and transcription app, and a Kotodama teleprompter) is the app behind proximity voice chat in games:
it plays the other players' voices (mixed by the game's distances, directions and walls) and turns what the
player says into text - live words while they talk, the finished line after - on the player's own PC (CPU only,
nothing sent anywhere). Games are modules (`app/crates/kd-games`), picked in the app's window; the first is Teardown,
through the mod Proximity Babble Chat (repo teardown-mods, folder `proxchat/`; the mod's side is `voice.lua`). The
link: PROTOCOL.md. Moved out of the mod's repo on 2026-10-05 (the user: its own project, decoupled from the chat mod,
for other games later). License: MIT (the user, 2026-10-06), "Copyright (c) 2026 AgeOfAlgorithms" (the user's pseudonym; their real name kept out of the repo and its history, 2026-10-06).

## Layout

The app is Rust (`app/`, since 2026-10-06: the user asked for the rewrite - a smaller download, less memory, a plain
native program); `engine/` is the Python version it was ported from, kept as the reference and for the tools.

| path | what |
|---|---|
| `app/DESIGN.md` | the crates, their interfaces, which Python each replaces |
| `app/crates/koetama` | the program: the window (egui), `--cli` (teardown_helper.py's flags), `--selftest`, the runtime, the microphone, settings |
| `app/crates/kd-common` | names and folders, word units and times (as the game's voice.lua), model downloads, the Feed |
| `app/crates/kd-audio` | the voice mixer, the muffle low-pass, wav, resampling (rubato), sound devices (cpal; the audio thread at MMCSS priority) |
| `app/crates/kd-speech` | speech to text through sherpa-onnx (shared libraries) and the language detector through ort (the same onnxruntime.dll): VAD, rolling passes, LocalAgreement, "auto" stitching, the recording microphones |
| `app/crates/kd-games` | the Game trait, Steam, the Teardown link (feed reader, message files, test voices) |
| `app/crates/kd-update` | updates from GitHub Releases (a 404 = no release yet) |
| `app/crates/kd-voice` | real voices between players (PROTOCOL.md "Real voices"): the relay client (WebSocket), ChaCha20-Poly1305, Opus (opus-rs), the jitter buffer, the send gate |
| `relay/` | the voice relay (a Cloudflare Worker, deployed at koetama-relay.ageofalgorithms.workers.dev) |
| `app/fixtures/` | the Python reference's answers (make_fixtures.py, make_speech_fixtures.py) the Rust tests compare against |
| `app/.cargo/config.toml` | the C runtime built into the exe (no VC++ redistributable needed) |
| `engine/koetama.py` | the app's window (tkinter): game picker, connection state, microphone / speakers / volume, what it hears, updates, licenses; `--cli` = the command line |
| `engine/runtime.py` | the running app, game-independent: the game module, the mixer and its output, the speech to text, the microphone (open only while the game wants it) |
| `engine/games/` | one module per game (`base.Game`: locate, start/stop, on_feed, send, test clips); `teardown.py`: the savegame feed + files link (Windows, and Linux through Proton) |
| `engine/audio.py` | the voice mixer (numpy only: the muffle is an FIR of two one-pole low-passes; scipy dropped from the app), resampling (sherpa-onnx's), wav files, devices |
| `engine/steam.py` | Steam libraries, an app's install / Workshop folder, its Proton prefix (Linux), the real Documents folder |
| `engine/asr.py` | speech to text: Silero VAD, the rolling passes (Parakeet v3 / GigaAM v3 / SenseVoice by language), word times, "auto" language (SpeechBrain detector + stitching) |
| `engine/paths.py` | the name, version, repo; the user's data folder (%LOCALAPPDATA%\Koetama: settings, models, test voices) |
| `engine/fetch.py` | model downloads over plain HTTPS (no Hugging Face library in the app): the pinned files of each model, resumable, into %LOCALAPPDATA%/Koetama/models; a developer's Hugging Face cache is used where it has them; KOETAMA_MODELS_URL = a mirror |
| `engine/updater.py` | updates from GitHub Releases: check, download, SHA256SUMS + same-publisher signature, run the installer silently |
| `engine/teardown_helper.py` | the command line for Teardown with the test modes (--auto-speech, --mic-wav, --transcribe, --auto, --type, --demo) |
| `engine/speech.py` | the first speech detector + faster-whisper path (kept: test_helper and bench/lid.py use it; not in the app) |
| `engine/export_lid.py` | builds the language detector's ONNX file (env pclid; CI too) |
| `engine/make_notices.py`, `engine/licenses/` | writes THIRD_PARTY_NOTICES.txt: the models, sherpa-onnx + ONNX Runtime, every Rust crate in the program (cargo metadata, Windows + Linux), full license texts |
| `engine/make_dummy_lines.py` | the mod's voice dummies' lines with word times (PC.VDUMMY_LINES) |
| `engine/test_*.py` | test_app (updater, Steam, games, runtime, filter), test_helper (link, mixer, VAD, word times), test_asr (the models on the benchmark clips), test_e2e (a fake game), test_auto_speech (22 recorded lines in real time) |
| `build.py`, `installer/koetama.iss` | the build: cargo (release), the exe + sherpa-onnx-c-api + onnxruntime libraries, the detector + notices; Inno Setup installer (per user); SHA256SUMS.txt |
| `.github/workflows/build.yml` | CI: cargo tests, then Windows (installer, selftest, install/uninstall check) and Linux (tar.gz, selftest under xvfb) on every push to main that changes the app; a v<version> tag: a DRAFT release |
| `bench/` | the benchmarks behind every model choice (reports in `export/asrbench/*.md`) |
| `probes/` | the in-game feasibility probes (each a tiny Teardown mod + a Python side) |
| `export/`, `build/`, `dist/` | generated, git-ignored |

## Environments (conda, conda-forge only; pip inside)

- Rust: rustup stable (installed 2026-10-06, not on PATH by default: `$HOME/.cargo/bin`), MSVC: Visual Studio 2022
  Build Tools (C++ workload). sherpa-onnx's prebuilt shared libraries download on the first build (into the target
  folder); kd-speech's build.rs copies them next to the test binaries (System32 has an older onnxruntime.dll that
  Windows would load first otherwise).
- `pcvoice`: the app and its tests - python 3.12 + pip only (numpy, sounddevice, sherpa-onnx, onnxruntime,
  huggingface_hub, psutil; nuitka for builds; faster-whisper, scipy, opencc, pillow for old paths / tests only).
  Never conda's numpy/scipy here (MKL/OpenMP clash killed the process).
- `pclid`: torch CPU + speechbrain + onnx, for `engine/export_lid.py` only.
- `pcbench`: the benchmarks' extra tools (edge-tts for clips).
- `teardown` (in teardown-mods): LuaJIT and PIL for the mod side.

## Commands (from the repo root)

    export PATH="$HOME/.cargo/bin:$PATH"
    (cd app && cargo run -p koetama)     # the app (Rust); -- --cli ... for the command line
    (cd app && cargo test --workspace)    # the Rust tests (-- --include-ignored: + the real models and devices)
    python build.py                       # dist/Koetama/ + dist/Koetama-Setup-<v>.exe
    $P app/fixtures/make_fixtures.py --real   # the Python reference's answers again (after a Python change)

The Python reference:

    P=<conda>/envs/pcvoice/python.exe
    $P engine/koetama.py                 # the app
    $P engine/test_app.py ; $P engine/test_helper.py ; $P engine/test_asr.py ; $P engine/test_e2e.py ; $P engine/test_auto_speech.py
    $P engine/teardown_helper.py          # the command line (Teardown running, a level with the mod)
    dist/Koetama/Koetama.exe --selftest # the build's window, sound, ONNX, detector, HTTPS, updates (CI runs it)
    KOETAMA_EXE=dist/Koetama/Koetama.exe $P engine/test_e2e.py   # the built exe end to end
    <conda>/envs/pclid/python.exe engine/export_lid.py   # rebuild export/lid/voxlingua107-ecapa.onnx
    $P engine/make_notices.py             # after any model or package change

Heavy jobs (benchmarks, long tests, builds) only while the user is not playing: they stream the game over Moonlight
and a busy CPU freezes it. Commit as AgeOfAlgorithms (123909089+AgeOfAlgorithms@users.noreply.github.com).

## Release plan (the user, 2026-10-06): open source + SignPath

Open source (MIT), builds by GitHub Actions, signed for free by SignPath Foundation (publisher shown as "SignPath
Foundation"; needs an OSI license, an automated build, the project already released). Steam was considered and
dropped as too much work for now. Steps: [done] license, window, build, installer, updater, CI; [the user] make the
repo public (first strip personal paths and private notes), publish the first release (unsigned), apply to SignPath,
then add its signing step to the workflow. The speech models stay OUT of the signed package (SenseVoice's FunASR
license is not OSI): they download per language (pinned Hugging Face revisions now; a mirror on our own release
later). The language detector (Apache-2.0) ships inside the app. Optional: a winget listing.

**Signed updates (2026-10-09, from the security audit).** The updater installs a release only when its
`SHA256SUMS.txt` carries `SHA256SUMS.txt.sig`: an Ed25519 signature (128 hex digits) over the file's exact bytes by
the release key, whose public key is built into the app (`kd_update::RELEASE_KEY`), and the file's first line is
`# koetama <version>` for that very release (an older signed file under a new tag is refused). Unsigned, a bad
signature, another version: the window and `--selftest` say why and only the releases page is offered. Authenticode
(same publisher) stays as an extra layer once builds are signed. The private key is
`$HOME/.koetama-signing/release.key` (PKCS#8 PEM), outside every repo: **back it up offline (a USB stick,
a password manager) and never commit it or give it to CI.** Losing it means no installed copy can auto-update
again until users reinstall a build with a new key by hand. Releasing (from `app/`, `export PATH="$HOME/.cargo/bin:$PATH"`):

1. Set the version in `app/Cargo.toml`, commit, tag and push: `git tag v<version> && git push origin main v<version>`.
2. CI builds both systems and drafts the release with the installer, the Linux build and `SHA256SUMS.txt` (first line
   `# koetama <version>`, written from the tag). Nothing is public yet.
3. Download the draft's checksums (do not open them in an editor - the signature is over the exact bytes):
   `gh release download v<version> -R AgeOfAlgorithms/koetama -p SHA256SUMS.txt -D <some folder>`
4. Sign: `cargo run -p kd-update --example release_key -- sign $HOME/.koetama-signing/release.key
   <some folder>/SHA256SUMS.txt` (writes `SHA256SUMS.txt.sig`; refuses without the version line, or with a key
   that is not the built-in one). Check: `cargo run -p kd-update --example release_key -- verify <some folder>/SHA256SUMS.txt`.
5. Upload only the signature: `gh release upload v<version> <some folder>/SHA256SUMS.txt.sig -R AgeOfAlgorithms/koetama`.
6. Publish the draft (GitHub's release page -> Edit -> Publish, or `gh release edit v<version> --draft=false -R AgeOfAlgorithms/koetama`).

A new key (only if the old one is lost or leaked): `cargo run -p kd-update --example release_key -- new-key <file>`
(never overwrites a file) prints the `pub const RELEASE_KEY` line to paste into `app/crates/kd-update/src/lib.rs`.

## Open

- **Real voices (2026-10-07; built and tested offline and through the live relay, NOT tried in-game):** feed version
  5 (room, key, me, to; Python reference and fixtures too), the `r` message (a fresh room once per game session; the
  socket connector: once per connection), the crate kd-voice (the relay client, end-to-end encryption, Opus 24 kbit/s
  in 60 ms packets, a jitter buffer 60..300 ms with Opus loss concealment, the send gate: push to talk + 0.25 s, or
  the speech detector from 0.3 s before it noticed), the mixer plays real players (src 0) from it, the window's
  Sound card and the command line show the voice chat's state. The microphone runs at 48 kHz when the game plays
  voices (16 kHz for the speech made from it). Opus is opus-rs (pure Rust, no CMake): unsafe-libopus was tried
  first and REJECTED - its SILK loss concealment gives full-scale noise. `cargo test -p kd-voice --test relay --
  --include-ignored` also runs four Koetamas through the live relay.
- **Chat translation (2026-10-08; NOT tried in-game yet; the service tested with real downloads,
  `examples/translate_live.rs`; the Marian engine in plain Rust matches Mozilla's quality at ~2.5x its WASM speed:
  app/DESIGN.md "The engine"):** the feed's `translations` and `to_translate` (Python reference and fixtures too),
  the `translation` / `translations_status` objects, profiles' `"translate"` (the
  built-in Teardown profile has it), `kd-translate` catalog / detect / service, the runtime, a Translation card in the
  window. Open: try it in-game with the mod; Maltese -> English has only a pre-release Mozilla model (used: no release
  exists; nothing into Maltese); the model
  list's `filter_expression` (some versions are meant for Android only) is not looked at - the newest numeric version
  is used, as the benchmark did.
- The models mirror (our own GitHub release) - after the repo is public.
- Linux build: made by CI, not tried on a real Linux / Steam Deck; no auto-update there (the app opens the page).
- The voice dummies' test voices are made with the Windows computer voices: none on Linux (silent dummies).
- Verified on this PC (2026-10-06, no game running): the built exe end to end (test_e2e with KOETAMA_EXE), also with
  an empty model cache (it downloaded Parakeet itself, 644 MB); --selftest; the installer: silent install, the installed
  selftest, an update with /RELAUNCH=1 while Koetama runs (old closed, new started), uninstall (user data kept).
  Not yet: the window with Teardown running, a real update from a published release (needs a v0.1.1 release).
- The Rust port (2026-10-06): every crate matches the Python on the fixtures (text, mixer to 5e-7, feed/link files
  byte-identical, stitching, LocalAgreement, the real models' transcripts / times / detector / mixed lines, the
  Listener end to end); the Python test_e2e.py passes against the Rust exe; selftest + installer cycle pass. Not yet:
  in-game with Teardown, Linux (CI builds it), a real update from one release to the next. Python 0.1 was never
  released: the first release is the Rust 0.2.
- Languages (the user, 2026-10-06): the player ticks every language they speak in Koetama's window (three tiers as
  the mod's Voice page: full / soft (beta) / weak (experimental)); only their models load (the window lists each
  model, loaded or not, ~memory), several = "auto" among exactly them (fewer candidates, fewer wrong stretches). None
  ticked: the game's "Language I speak". A change waits 1.5 s to settle, then loads the new and unloads the rest.
  Measured: en+ru 1.27 GB for the whole app (all 10 "auto" languages: ~1.5 GB). The mod's own language picker is
  still there (whether to drop it: the user's call).
- Idle memory of the window ~180 MB (measured with OpenGL; Windows now draws with wgpu). The detector ships with its
  weights stored as float16 (43 MB, was 86; the user's decision 2026-10-06, bench/lidquant.py: the same results; int8
  was REJECTED by the user - a third of the one-word callouts lost): 88 MB installed, a 52 MB installer.
- (Python packaging, before the port) a conda Python keeps its modules' DLLs in Library/bin, which Nuitka misses (build.py CONDA_DLLS;
  tcl86t needs zlib1.dll - without it the window silently failed to open while --cli worked). huggingface_hub broke in
  the compiled build (lazy imports): models come through engine/fetch.py. PowerShell's Start-Process -Wait also waits
  for the children (a relaunched Koetama): use WaitForExit() when testing the relaunch.
- Private notes before the repo goes public: this file (Moonlight/the user's setup, local paths), CLAUDE.md, and the
  git history (local conda paths in old commits; nothing secret found).

## History: research and decisions (moved from proxchat/PROJECT.md, 2026-10-05)

Goal: real proximity voice on top of the text chat. A script has no microphone or network API (checked
`data/script_defs.lua`; the game has no voice chat), so capture needs a helper program outside the game.
Prior art: TeardownVC (pooiod, Workshop 3653434619, github.com/pooiod/TeardownVC; no license: take no code
or text from it). **Credit (the user's decision, 2026-10-04):** when voice ships, the README and the Workshop
description say: "Voice chat was inspired by TeardownVC by pooiod (github.com/pooiod/TeardownVC), which
showed that a mod can feed a companion app through the save file. No code is shared." What the two probes measured in-game (single player, local mods):

- **game -> helper: `savegame.mod.*` keys reach savegame.xml within a frame** (written every frame: 97 % of
  frames seen, median 17 ms between file updates, max 47; 10 Hz writes: 197 of 200 seen; 0 partial reads in
  1364 changes at 330 polls/s; 60 fps kept). log.txt is buffered (lines 1.4 s late at the median, up to 6 s).
- **helper -> game: `HasFile` asks the disk live** (created or deleted -> seen in ~17 ms including the
  savegame leg; with or without an extension; a file there before the load too). `MOD/../file` works (one
  level up, the mods folder); an absolute path returns false. 60 extra calls a frame: 59.9 -> 59.2 fps.
- **`LoadSound` loads an .ogg that appeared after the level loaded, and `PlaySound` plays it** (heard: two
  files dropped into `MOD/sig/` mid-level). `UnloadSound`, `IsSoundPlaying`, `GetSoundProgress` exist (unused).
- **A voice streamed as clips sounds fine** (voiceprobe, a 7 s computer-voice sentence written clip by clip
  in real time, loaded the frame `HasFile` sees it, played on a clock from the first clip + 50 ms): the user
  judged all runs "great" - 0.4 / 0.2 / 0.1 s clips, crossfaded (24 ms) or plain cuts, started on the frame
  or lined up with `SetSoundProgress`. No clip late for its slot, none missing, 60 fps kept, no file held
  open afterwards (`UnloadSound` 1 s after a clip ends). Said -> heard: 225 ms with 0.1 s clips, ~330 ms
  with 0.2 s, 534 ms with 0.4 s (plain cuts 0.2 s: 294 ms), before the network. A clip played at a fixed
  point gets quieter as the player moves away (the engine's own falloff, `nominalDistance` 10 m).
  One 160 ms frame in the last run (cause unknown; 40-70 ms frames also occur with a single file).
- Also there: `GetClipboardText` / `SetClipboardText` (a two-way channel, but it is the player's clipboard).
  A web page cannot replace the helper: Chromium's file access blocks everything under AppData.

**Decided (the user, 2026-10-04): the HELPER plays the voices** (it mixes them itself, fed by the game through
savegame.xml: per speaker a volume, a direction and a muffle amount, from the chat's own ranges and walls),
not the game from clips: smooth whatever the game's frame rate does (a clip can only start on a frame; at
30 fps or in a destruction hitch the game-played voice would stutter - not tested) and about half the delay
(~0.1-0.15 s estimated against 0.25-0.3 s). **Voice settings go in the chat window, on their own Voice page** (2026-10-05: a tab shown only with the helper; the game
passes them to the helper through the savegame); the helper's own window stays small (what only it knows:
which microphone / output device). Helper -> game through `HasFile` bits: helper running, who is talking,
a mic level for a meter.

**Step 1 built and tried in-game (2026-10-04; the user: "helper audio is working well" - no detail on walls, the slider or the ranges): the voice dummies.**
Proximity Babble Chat's `mods/proximity chat/voice.lua` (teardown-mods repo) (included by main.lua after chat_core.lua, which calls it through guarded
hooks: `PC.voiceTick`, `PC.voiceDraw`, `PC.voiceDummyCommand`, `PC.voiceOn`; chat_core alone still works):
- `/dummy voice` (three: whisperer, speaker, yeller, green heads, a name over each, "(( name ))" while it
  talks), `/dummy voice 1|2|3`, `/dummy voice clear`; `/dummy clear` removes both kinds. Separate from the
  chat's dummies (ids 2000-2002, `c.voice`); not in /help or the README yet. They talk in turns
  (`cfg.voiceTurn` 5 s, `voiceGap` 0.7 s). Walls apply to them (the chat's `PC.wallState`).
- The feed, 20 times a second while there are voices: `savegame.mod.pcvx.f` =
  `1|seq|voice volume|id,src,talk,gain,azimuth,elevation,muffle;...` (format at the top of voice.lua): gain
  from `PC.loudness` over `PC.hearDist` (the mode's range, buffer, walls), direction from the camera, muffle
  rising across the buffer (to `voiceBufferMuffle` 0.85) or from a wall (through 0.75, a way round 0.3). An
  empty list once = off (also written at start over a feed left on by an earlier session).
- Voice page: a "Voice volume" slider while there are voices (`savegame.mod.pcvxvol`, sent in the feed).
- The helper prototype: `engine/teardown_helper.py` (Python: numpy, scipy, sounddevice from conda-forge
  `python-sounddevice`): reads the feed (shared-read polling, 5 ms; a feed already in the file at start is
  ignored until it changes), mixes at 48 kHz stereo: constant-power pan from the azimuth (x cos elevation),
  behind a little duller and quieter, a two-pole low-pass for the muffle (16 kHz -> 400 Hz), every value
  gliding over 50 ms, fade-out when the feed stops for 1.5 s, a soft limiter. WASAPI output (22 ms delay
  measured here; the default MME was 100 ms). The dummies' voices: the Windows computer voices, made once
  into `export/voicehelper/`. `--demo` (no game: a voice circles you), `--list`, `--device N`, `--volume`.
  `engine/test_helper.py`: the parser, the link, the mixer, the speech detector, the transcript
  filter and (in env pcvoice) the real model; no sound device needed.

**Speech-to-text benchmark (2026-10-04, `bench/`, results in `export/asrbench/report.md`,
`codeswitch.md`, `shortwords.md`, `second.md`).** Computer voices (Windows David/Mark/Zira, Piper en/ru/zh/es/de),
52 lines + 4 noise-only clips, clean and "room" (echo, 100 Hz-7 kHz, noise 15 dB under), Ryzen 7 3700X, 4 threads,
one other core kept busy (env `pcbench`, pip only; models from HF `csukuangfj2/sherpa-onnx-nemotron-3.5-*`).
- Nemotron 3.5 streaming 0.56 s int8 (sherpa-onnx 1.13.8): text complete 0.5-0.7 s after speech ends, 0.33 s CPU
  per s of speech, 850 MB, never invented text on noise, word timestamps 90 % within 0.24 s. Errors clean / room:
  en 0.6 / 3.2, ru 10.9 / 30.9, zh 9.8 / 14.7 (characters), es 0 / 10.3, de 0 / 17.4. Whisper small: 2.1 s after
  the line, 1.0 s CPU per s, invents "Thank you."/"You" on noise; Whisper medium: 6.2 s (too slow), best ru/zh.
- Nemotron WEAKNESSES: drops single-word lines ("Okay.", "No.", "Go.", "Да.", "好。": 89 of 140 empty; Whisper
  small 0) and the first ~0.3 s of a stream (always feed a lead-in: our clips start with 0.5 s). A spoken primer
  ("hi", "One, two, three", a beep, noise) did not help ("hi" was never transcribed). blank_penalty 1-2 brings
  single words back (47 of 140 empty at 2) but makes sentences worse (ru 10.9 -> 21.8): only for short bursts.
  Auto language doubles ru errors: pass the language. Beam search / hotwords: not supported by sherpa-onnx for NeMo
  streaming transducers (greedy only). The 1.12 s model: same accuracy, half the CPU (0.19 s per s), bubble
  updates once a second.
- Second pass on the finished line by a language specialist (sherpa-onnx, int8): Russian GigaAM v3 CTC 5.5 / 5.5 %
  in 0.17 s (225 MB, MIT); Chinese SenseVoice Small 5.9 / 6.9 % in 0.12 s (237 MB, FunASR model license - check).
  Streaming alternatives were weaker in noise (T-one ru 9.1 / 20.0, Paraformer zh 4.9 / 12.7, Zipformer 8.8 / 11.8).
- Noise suppression before Nemotron: GTCRN hurts everywhere; DPDFNet2 helps en/es/de in the room a little, hurts
  zh and clean ru, 0.15 s CPU per s: not by default.
- Switching language mid-utterance (8 mixed lines, auto): Nemotron 23 % wrong (keeps both languages across a phrase
  boundary, drops a single foreign word), Whisper small / medium 29 % (force one language).

**Step 3 built (2026-10-04; offline tests pass, NOT tried in-game; the user is away a week, no real-voice test):
the benchmark's two-pass design in the helper, and live words in the game.**
- Helper `engine/asr.py` (sherpa-onnx in env `pcvoice`, models downloaded once from Hugging Face,
  ~1.5 GB for English): Silero VAD v5 (a 0.5 s pause ends a line, 15 s at most) -> Nemotron 3.5 560 ms int8 fed
  from 1 s before the speech, its words so far sent as LIVE messages -> at the end a second pass:
  `SECOND = {ru: GigaAM v3 CTC, zh: SenseVoice, es: Parakeet v3, en: Parakeet v3}`, else Nemotron's text; a
  short line (< 1.2 s, or Nemotron heard nothing) goes to Parakeet in the European languages. `auto`: the
  second pass from the transcript's script (Cyrillic -> ru, CJK -> zh). `Microphone` at 16 kHz (WASAPI
  auto-convert); `WavMicrophone` = a recording played as the microphone (`teardown_helper.py --mic-wav f.wav`).
  `teardown_helper.py --transcribe f.wav --lang ru` runs a file through it. Whisper (speech.py) is no longer used.
- Measured offline (test_asr.py: the benchmark clips streamed as from a microphone, 50 ms blocks): errors clean /
  room en 0.0 / 1.9, ru 9.1 / 5.5, zh 4.9 / 8.8, es 0.0 / 2.6, de 0.0 / 19.6; 8 of 8 one-word callouts right
  (Parakeet); hiss: no line; ~0.27 s CPU per s of audio. A long pause inside a sentence splits it into two lines.
- Messages to the game: `pcvx_t<n>.xml` now `<body tags="pcvx k=l|f u=<utterance> t=<hex>"/>` (no k = "f");
  an empty "f" clears the live words. The feed is version 3: `...|mic|lang|speakers` (the helper reads 2 too).
- Game (voice.lua + guarded hooks in chat_core): Voice page row "Language I speak (click: next)" (16 languages
  + Auto, `savegame.mod.pcvxlang`, a new `PC.cycle` button); live words -> `server.pc_live` (every 0.25 s
  at most; not for Global or a dead channel) -> ONLY to the players within the mode's outer range +
  whisperSlack (`ClientCall client.pc_live`, never in shared - so Whisper works like Speak and Yell and stays
  private; the user: "whispering should behave the same as the rest", 2026-10-05) -> a bubble with the words
  so far (the end, after "…") instead of the "..." within the mode's words range, "..." in its buffer; the
  finished line (`server.pc_say` with voice) clears it, so does 4 s without news; the speaker sees
  "Speaking: <words>" above the hint. `teardown_helper.py --auto` now sends each test line as live words first.
- Tests: test_proxchat.lua 378, test_helper.py 60, test_asr.py 46, test_e2e.py 4 (fake game + the real helper
  with a recording as the microphone: 11 live messages, 4 lines, in order).

**Live words WITHOUT Nemotron (2026-10-05, `bench/rolling.py`, `export/asrbench/rolling.md`):** the line
so far re-transcribed every 1 s by the language's own offline model (en/es/de Parakeet v3, ru GigaAM v3, zh
SenseVoice), shown at once, "stable" = two passes agree. Against Nemotron streaming on the same lines and clock
(room): live words show as soon (0.5-0.64 s after a word ends vs 0.55-0.69; stable after 1.2-1.5 s) and are far
more accurate (en 4.5 vs 7.1 %, ru 9.1 vs 38.2, zh 9.8 vs 17.6, es 15.4 vs 23.1); no shown word changed on
short lines, 1-2 per 10-17 s line; CPU per s of speech lower (en 0.33 vs 0.44, ru 0.21 vs 0.42, zh 0.16 vs
0.39); finished lines the same (the same final model; de 15.2 vs 19.6 room, 6 lines only - the earlier full-clip
test had Parakeet de worse). Whole-line passes stay cheap up to the 15 s cap (0.22-0.46 s per s): the windowed
variant (lock + re-decode the tail) is not needed (it gave identical numbers - apparently never kicked in).
Memory of each model alone: Nemotron 777 MB, Parakeet 779, GigaAM 302, SenseVoice 346 -> an English player
~0.8 GB instead of ~1.6, Russian ~0.3, Chinese ~0.35. Lost without Nemotron: Arabic, Hindi, Vietnamese,
Turkish (no model), ja/ko via SenseVoice and fr/it/pt/nl via Parakeet untested.

**Language identification and mixed lines (2026-10-05, `bench/lid.py`, `export/asrbench/lid.md`; env `pclid`
for SpeechBrain - torch CPU; its files COPIED, Windows refuses its symlinks):** 104 lines, 140 one-word lines, 20
mixed utterances (2-3 languages, a voice per language - easier than one real speaker). Among the 10 supported
languages: lines right Whisper tiny 100/104, AmberNet 101, SpeechBrain ECAPA 101; ONE-WORD lines Whisper tiny
122/140, AmberNet 83, SpeechBrain 80. Time per s of audio: Whisper tiny 202 ms (every check a 30 s window; base
ruled out by the user as too slow), AmberNet 14 ms (community ONNX surogate/ambernet-langid, 117 MB, NVIDIA NGC
terms - check before shipping), SpeechBrain 20 ms (85 MB, Apache-2.0). Stitching (1 s windows every 0.25 s ->
language per 0.25 s frame, 3-frame vote, runs < 0.5 s merged, cut at the quietest point near a change, each
segment by its language's model): words wrong AmberNet 7 %, Whisper tiny 8, SpeechBrain 10; the true segments 6
(the ceiling); ONE model for the whole line 43-58 % (Parakeet drops or garbles the other languages). Longer
windows were worse (1.5 s: 16-27 %). Boundary error with 1 s windows ~0.06-0.11 s. Lost: a single foreign word
inside a sentence ("Where is the ключ for...", "这个 chalice 很值钱", "Run, 它就在..."): shorter than the 0.5 s
merge. A one-word line's language is a guess for every model but Whisper: use the player's setting then.

**Helper now (2026-10-05): the rolling design** (`asr.RollingLine`; **Nemotron removed from the helper** on the
user's word, 2026-10-05 - its code, `--design`, its cached downloads; the asrbench scripts that measured it stay as
the record): the line so far by the language's own model every 1 s (`roll_model`: ru
GigaAM, zh/yue/ja/ko SenseVoice, else Parakeet), the finished line by one more pass. Language "auto" (the game's
"Auto (guess)"): each pass cut into stretches by the language detector (`asr.segments`; AmberNet until
2026-10-05, now SpeechBrain - see "Licenses" below; 1 s windows every 0.25 s) and each stretch written by its language's model (`transcribe_mixed`): 7 % words wrong on the 20 mixed
lines, 0.2-0.6 s per line. Cutting by MODEL instead of by language (Parakeet writes all the European ones) was
tried: 17 % (Parakeet decides one language per clip and drops the other) - cut by language. All models for
auto: Parakeet + GigaAM + SenseVoice + the detector ~1.5 GB.
`teardown_helper.py --auto-speech`: 22 recorded lines (export/asrbench/lid, from lid.py prep) through the REAL pipeline
in real time, each in its own language (mixed ones "auto"): 4 English sentences, 4 one-word callouts, 3 ru,
2 zh, es, de, 7 mixed (2-3 languages). `test_auto_speech.py` (fake game): 22 played, 21 lines arrive, sentences
with live words first; "Run!" (room) came out empty; one-word lines have no live words (done before the first
1 s pass).

**Languages (the user's decision, 2026-10-05).** The Voice page's "Language I speak" opens a list in three
groups (`PC.VOICE_LANGS`, a code, a name, a group), by the models' published error rates:
- **Supported** (say so in the README / Workshop text): English, Spanish, French, German, Italian, Portuguese,
  Dutch, Polish, Ukrainian (Parakeet v3, under 8 % words wrong on FLEURS), Russian (GigaAM v3), Mandarin,
  Cantonese, Japanese, Korean (SenseVoice); and Auto (guess).
- **Beta** ("soft support", shown as such: "(beta)" after the name): Czech, Slovak, Romanian, Croatian,
  Bulgarian, Finnish, Swedish, Hungarian (Parakeet 8-16 %).
- **Experimental** (they work, but are never called supported): Danish, Estonian, Latvian, Lithuanian,
  Slovenian, Greek, Maltese (Parakeet 17-24 %).
- Turkish, Vietnamese, Arabic, Hindi left the list (only Nemotron wrote them); a saved one reads as English.
- **SenseVoice: the 2024-07-17 release** (`csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17`),
  language "auto". The 2025-09-09 int8 one used before is a Cantonese fine-tune (ASLP-lab WSYue-ASR) that lost
  Japanese and Korean: 74 / 91 % of the characters wrong against 2.0 / 1.1 % (Cantonese 7.6 against 6.3, Mandarin
  1.2 both; 8 lines each by Microsoft neural voices via edge-tts, `bench/cjk.py`). A fixed language
  setting changed nothing measurable, and sherpa-onnx ignores a per-stream one.
- **Auto stays on 10 languages** (`asr.MIXED_LANGS`: en ru zh es de fr it pt ja ko; Cantonese is found as zh and
  SenseVoice writes it). More candidates, more wrong stretches (`bench/autolangs.py`): with the 13
  supported ones Russian lines went 8 -> 18 % words wrong (taken for Ukrainian), English 3 -> 5 %, mixed lines
  7 -> 9 %, single-language lines with a wrong stretch 18 -> 30 of 104 (measured with AmberNet). Neither detector has a
  Cantonese label.
- The detector runs on a moving window, not the growing line: 1 s windows every 0.25 s, each scored once and
  cached by the line (4 one-second windows per second of speech, however long the line).

**Licenses (checked 2026-10-05, for commercial use, the helper maybe serving other games later):**
- Fine: Parakeet v3 (CC-BY-4.0: credit NVIDIA, link the license, say it was converted to ONNX), GigaAM v3 (MIT),
  Silero VAD (MIT), Nemotron 3.5 (OpenMDW-1.1, commercial use stated), Whisper / faster-whisper / ctranslate2
  (MIT), sherpa-onnx (Apache-2.0), onnxruntime (MIT), numpy / scipy / psutil (BSD), sounddevice + PortAudio
  (MIT), huggingface_hub (Apache-2.0), CPython (PSF), PyInstaller (bootloader exception) or Nuitka (runtime
  exception) for an .exe. csukuangfj's conversions carry no license of their own: the original's applies.
- **AmberNet could not ship** (NVIDIA NGC terms only - no redistribution; the surogate ONNX re-upload says
  `nvidia-ngc-terms-of-use` itself). **Replaced (the user, 2026-10-05) by SpeechBrain `lang-id-voxlingua107-ecapa`**
  (Apache-2.0; VoxLingua107 data CC-BY-4.0), exported to ONNX by us: `engine/export_lid.py` (env
  pclid) writes `export/lid/voxlingua107-ecapa.onnx` (86 MB, fp32) + `.json` (the labels). torch.stft's complex
  numbers do not export: the STFT is two convolutions (cos / sin kernels x SpeechBrain's window); the rest is
  SpeechBrain's own modules. Checked against SpeechBrain itself on 72 cuts of 0.5-2.7 s: the same language every
  time, log-probabilities within 0.00005. The helper looks in its model folder, then in export/lid. **To ship it
  must be hosted somewhere** (e.g. our own Hugging Face repo, Apache-2.0 with the attribution) - not done (asks the
  user first: it publishes a file).
  Swapped in as it was, it was a little worse than AmberNet (`lidswap.md`: mixed lines 9.8 % words wrong against 6.7,
  Russian lines 10.9 against 8.2, German 29.3 against 23.9; the same 20 ms per s of audio). The stitching was
  then retuned (`bench/lidtune.py`, `lidtune.md` / `lidtune2.md`): quiet frames do not vote (the quiet
  around a word and the 1 s pre-roll had got languages of their own: "Okay." -> French + Russian), a Viterbi
  path where a change of language costs 4 (instead of the 3-frame vote), a line with under 1.5 s of SPEECH is one
  stretch, and a short line the detector is not sure of takes the language of the player's line before
  (`Listener.last_lang`). The helper now (`lidswap_helper.md`), against AmberNet as it was:
  single-language lines en 1.9 / ru 7.3 / zh 7.8 / es 1.3 / de 16.3 % words wrong (AmberNet 3.2 / 8.2 / 9.3 / 2.6 /
  23.9), lines with a wrong stretch 8 of 104 (18), one-word callouts with a wrong stretch 47 of 140 (86; the
  fallback to the line before is not in this figure); **mixed lines 11.4 % (AmberNet 6.7): worse**. Tried and not
  better: the log of the mean probability per frame, switch costs 1 / 2 / 8, 1.5 s windows (single lines better,
  mixed 22 %).
- **Risk: SenseVoice's FunASR Model License v1.1**: commercial use not forbidden, attribution and the model name
  required, but it can be changed by Alibaba "automatically", forfeits the license on "unjustified denigration",
  leaves its governing law blank. (The old 2025-09-09 file was trained on WenetSpeech-Yue, CC-BY-NC: dropped.)
- **Model downloads pinned (2026-10-05)**: `asr.MODELS` holds each repo's tested revision (a commit hash: what it
  points to cannot change) and the files used (SenseVoice's repo also holds a 900 MB full-precision copy: a player
  now downloads 240 MB, not 1.2 GB). What a player downloads: Parakeet 670 MB, GigaAM 225, SenseVoice 240 (Auto:
  all three), the detector 86, Silero 2. Pinning does not help if an author DELETES a repo: then mirror the files
  (our own Hugging Face repo or a GitHub release; every license here allows redistribution with the notices) or
  bundle them (~1.2 GB) - the user's choice, open.
- **SenseVoice stays** (the user, 2026-10-05: its license allows commercial use; credit Alibaba and keep the name).
- **Build** (the user agreed): leave sounddevice's `*-asio.dll` out (Steinberg ASIO SDK: GPLv3 or a Steinberg
  agreement); ship `engine/THIRD_PARTY_NOTICES.txt` with the helper. It is written by
  `make_notices.py` (pcvoice env): the 5 models (authors, license, source, what was changed) and every package
  the helper runs on (its 7 direct ones + what they require: 27 + Python), each with its full license text
  (model texts kept in `engine/licenses/`). It refuses a license not on its checked list or a
  package without a license text. Run it again after any model or package change. certifi and tqdm are MPL-2.0:
  fine unmodified (the notice points to their source).

**Voice test mod (2026-10-04, for multiplayer with friends):** `python tools/make_voice_test.py` (teardown env:
it also stamps VOICE TEST on the previews) copies `mods/proximity chat/` WITHOUT its id.txt (the public item
3812301496 - with it, Publish would overwrite the public mod) to `mods/proximity chat voice test/`, info.txt
"Proximity Babble Chat (VOICE TEST)". Publish it as its own item (friends-only / unlisted), then copy its new
id.txt back into that folder (the script keeps it across runs and refuses the public id). Only one of the two
mods may be on at a time. The helper finds the Workshop copy (it writes to the Workshop content folder too:
writable here) - `MOD/../` from a Workshop mod and a joiner's machine are still untested.

**Idea (the user, 2026-10-04): transcripts.** Each speaker's helper transcribes its own microphone per
utterance (a small local speech model, e.g. whisper.cpp tiny / base; not measured) and the text goes
through the chat's normal path as a message flagged "voice": one history for typed and spoken lines, the
same ranges and garbling, readable without the helper, and `proxchat.said.*` gives it to game mods (a door
that opens to a spoken password). Needs text from the helper INTO the game - measured in-game (textprobe,
single player, local mod):
- **helper -> game TEXT works: the helper writes a prefab file, a client script `Spawn`s it and reads
  `GetDescription` (free text) or `GetTagValue` (hex) from the body it made, then `Delete`s it.** 16 of 16
  lines intact both ways: punctuation and quotes (XML-escaped in the file), Cyrillic / Chinese / Arabic /
  Greek, 90 and 300 characters; a file with or without the `<prefab>` wrapper; `MOD/../` too. Written ->
  read -> back through the savegame: median 28 ms, max 56; no frame over 31 ms. A file rewritten under the
  same name is read fresh.
- `UiGetImageSize` of an image written mid-level works (a new name: 37x53 read), but the same name again
  is remembered (41x59 twice): not needed now.
- In a script `dofile`, `loadfile`, `loadstring` are functions; `io`, `os`, `require` are nil (not tried).

**Step 2 built (2026-10-04; offline tests pass - mod 363 checks, helper 56): what you say becomes a chat
line. The TEXT PATH is verified in-game** (single player, local mod, the user remote over Moonlight with no
microphone: `teardown_helper.py --auto` sent 7 lines; log.txt shows each `Spawning: MOD/../pcvx_t<n>.xml` 8 s apart, all
acked and deleted; the user: "they all arrived fine", Russian and Chinese shown, the long line in pieces, a line
said after switching the chat to Yell came out as a yell). Not yet seen: the "disconnected" line, the tags'
exact wording, a real microphone and the speech model in a session. `--type` (keys typed into the helper's
console) never reached the helper over Moonlight, neither by reading stdin nor by msvcrt: use `--auto`.
- The feed is version 2: `2|seq|volume|session|ack|ping|mic|speakers` (top of voice.lua). Written 20 times
  a second with voices to hear, 5 times with only the helper there, not at all otherwise.
- **The helper is found by the game**: it puts `pcvx_on` next to the mod's folder (`MOD/../pcvx_`; the
  local mods folder, and the Workshop content folder when it exists); the script looks once a second (one
  `HasFile`), then feeds and pings it every 2 s; the helper answers ping n with the file `pcvx_p<n % 1000>`
  (a file left by a crash does not answer: off after 5 s, looked for again every 15 s). A history line
  says "Voice helper connected." / "disconnected.".
- **Text**: the helper writes `pcvx_t<n>.xml` (a prefab, `<body tags="pcvx t=<hex>"/>`), numbered per
  session from the game's ack; the script reads the next number (`Spawn`, `GetTagValue`, `Delete`), acks
  it in the feed, the helper deletes the file. A new level = a new session number: the helper starts over.
- **Said like a typed line, flagged as voice**: `PC.say(text, mode, true)` in the current mode -> `server.pc_say(...,
  voice)` -> `msg.v` / the whisper and dead ClientCalls carry it -> no babble, the history tag reads
  `[spoken]` / `[whispered]` / `[yelled]` / `[global, spoken]`, and `proxchat.said.<n>.voice` is true (a door
  that opens to a spoken password reads that event). A long sentence is cut between words into pieces of
  `maxLen` characters, one per rate limit.
- **Voice page** (while the helper is connected): "Write what I say in the chat" On / Off, default Off, saved
  (`savegame.mod.pcvxtext`); the feed's mic flag follows it and the helper opens the microphone only then
  (and only while the game is sending).
- **The helper's ears** (`engine/speech.py`): the default microphone (WASAPI), a speech detector
  (30 ms frames against a learned noise floor: 12 dB above it and above -45 dB; starts after 90 ms, ends
  after 0.7 s of quiet, cut at 15 s; one level held for 1.2 s is noise, not speech), then faster-whisper
  (`base.en`, CPU int8, 4 threads, beam 5; `--model`, `--language`, `--threads`) per utterance, on this
  machine. A transcript filter drops non-speech marks and what the model "hears" in noise. Measured here
  (Teardown not running): 13 s of the test voice in 0.68 s, word for word; 3 s of noise: nothing.
- **Environment: conda env `pcvoice`, pip packages only** (python 3.12 + pip from conda-forge, then
  `pip install numpy scipy sounddevice faster-whisper`). With conda-forge's numpy / scipy (MKL +
  llvm-openmp) in the same env the first `transcribe` killed the process (exit 127, no message): the
  wheel's own OpenMP runtime against conda's. Run: `<conda>/envs/pcvoice/python.exe
  engine/teardown_helper.py`. The model (~145 MB) is in `~/.cache/huggingface`.
- Open: the helper's files (for a local mod) are in Documents/Teardown/mods, which OneDrive syncs here (a
  transcript is a file there for a moment); the game's sound from loudspeakers reaches the microphone and
  is transcribed too (headphones); does a read of savegame.xml at the wrong moment ever make the game's own
  save fail? (none seen: the probes read it 330 times a second; the helper now reads 100 times a second).

Not measured: a joiner's machine (client scripts of a Workshop mod), `MOD/../` from the Workshop folder,
`LoadSound` again on a name whose file changed, several speakers at once (clips loaded per second), a long
session (memory after many loads / unloads), a real microphone voice with room noise, deeper `../`.

**2026-10-07: the voice server's region, Koetama's version, the voice server's state.** The host picks where a
session's voice room lives (the mod's Settings, "Voice server": Auto or a Cloudflare region; feed v5's `region` field,
the relay's `&region=` location hint; the region is part of the room's name, so a change moves everyone to a new
room). Measured from Toronto, one hop: Auto 61 ms, North America East 62, West 108, Europe West 138, Asia-Pacific 185.
Koetama writes `pcvx_v5` (each feed version it reads) next to `pcvx_on`: a mod that doesn't find its version tells
its player to update Koetama. And `pcvx_vc` / `pcvx_vx` (in the room / the relay can't be reached: two failed
tries in a row), shown on the mod's Voice page; the socket connector gets a `voice` line instead.

**2026-10-08: chat translation (PROTOCOL.md "Translation").** The game sends the chat lines it shows
(feed v6: up to two rules `ja>en,ko>en`, up to 16 lines as `<id>:<hex>`); Koetama translates every stretch of a line
in a rule's source language and answers each id once (`x`; "" when nothing needed it or the models are not ready),
and tells the rules' states (`d`: ready / downloading N / loading / unavailable / error). Models: Mozilla's Firefox
Translations (Remote Settings list, newest purely numeric version, sha256-checked downloads into
`%LOCALAPPDATA%/Koetama/translate/<from>-<to>/<version>/`; a pair without English goes through it: two models; yue
uses zh-Hant as a source only). Mixed-language lines are split by script, then by a small detector for Latin and
Cyrillic text (whatlang, MIT, plus letter and little-word rules and a Maltese check; whatlang has no Maltese): 98.5 %
of the 2900 NTREX sentences come out as one stretch in the right language (app/DESIGN.md, kd-translate). Translations
are cut at 1000 characters (TRANSLATION_MAX; speech lines stay at 400).

**2026-10-08: protocol 2 - one JSON API, two transports (PROTOCOL.md).** The user asked for the simplest API. Before,
a files game wrote a 17-field pipe string (versions 2-6 all still read), Koetama answered with prefab tags of one-letter
kinds and hex text, and announced itself with marker files (`pcvx_v5`, `pcvx_v6`, `pcvx_vc` / `pcvx_vx`), while a
socket game spoke JSON with other names. Now both connectors carry the same objects (`app/crates/kd-games/src/api.rs`,
`engine/games/teardown.py`): the game's `feed` (`volume`, `listen` off / always / push_to_talk + `talk_key`, `lang`,
`live`, `speakers` with `azimuth` / `elevation` and `test_voice` + `talking` for test voices, the room, `translations`,
`to_translate`) and Koetama's `hello` (with `features`), `speech` (start / live / final), `room`, `voice`,
`translation`, `translations_status`. Over files the feed is the object or its hex (Teardown: hex - whether Teardown
escapes quotes in savegame.xml is unknown; our probes always used hex), plus `seq` / `session` / `ack` / `ping`;
Koetama's files are `on`, `p<n>` and object n of each session (`t<n>`: json, or a prefab whose tag `j` is the object's
hex; object 1 is the hello). No feed versions: an older Koetama is recognised by its `v5` / `v6` files (Koetama 0.4.0
sweeps them). Profiles' test voices are `{"id": ...}` (was `src`). The Teardown mod was ported by an agent (JSON in
Lua, hex both ways).

**2026-10-09: what other games need (docs/game-survey.md; the user: "do all 5").** A survey of how mods in other
games can reach a local app (Valheim, Godot, Tabletop Simulator, Monster Hunter Rise demos in `examples/mods/`, an
HTTP connector for sandboxed scripts) found five gaps, all closed in protocol 2 without a version bump:
1. `room_seed`: one shared secret (a lobby id, a server's join secret) instead of the game making room + key.
2. Player ids may be strings (Steam ids, names): relay ids are hashed from them; a clash is `voice` `id_taken`.
3. Positions: `listener` + speakers' `position` + `range` [near, far] - Koetama computes direction, loudness and `to`
   (a game no longer does vector maths in its script language).
4. Objects for the game's HUD: `talking` (who speaks now, this player too), `status` (speech engine and microphone),
   `voice` with `players` (who is in the voice room: voice packets v2 carry the sender's id and range, and a presence
   packet every 5 s), `translation` with `from` / `to`.
5. The hub, for games whose script runs only on the host (Tabletop Simulator, many server-side mods): the host's feed
   lists `players`; each gets a join code (8 characters, shown by the game) to type into their own Koetama ("Join a
   hosted game", `--join`), which then works for them as if their game fed it, through the relay.
Tested: Rust and Python suites (fixtures shared), and over the live relay (no relay change was needed): a joined
player's typed line reached the host's game 251 ms after it was typed; two seeded Koetamas saw each other in
`players` and the listener heard `talking` ~140 ms after the speaker's own Koetama. NOT tried in a real game yet.
