# Kotodama

**Proximity voice chat with live speech-to-text, for games.** Kotodama ("word spirit" in Japanese) runs next to
your game. It plays the other players' voices placed where they stand: louder when close, from their side, muffled
behind walls. It also writes what you say as you say it, in many languages, so the game can show your words in
speech bubbles and a chat history. A spoken line can even open a door.

Everything runs on your own PC: no audio or text is sent to any service. **Status:** early prototype (version 0.2:
the app is written in Rust; the first version was Python).

## Games

Pick the game in Kotodama's window. Each game is a small module in [`app/crates/kd-games`](app/crates/kd-games/).

| Game | What it needs |
|---|---|
| Teardown | The [Proximity Babble Chat](https://github.com/AgeOfAlgorithms/teardown-prox-text-chat-mod) mod (Steam Workshop) |

Adding a game: see [PROTOCOL.md](PROTOCOL.md), "Adding a game".

## Install (Windows)

1. Download `Kotodama-Setup-<version>.exe` from [Releases](https://github.com/AgeOfAlgorithms/proximity-voice-chat-STT-engine/releases).
2. Run it. It installs for your user only, so it needs no administrator rights.
3. Start Kotodama, pick your game, then start the game.

The first time you speak, Kotodama downloads the speech model for your language once: about 670 MB for English and
the other European languages, 225 MB for Russian, 240 MB for Chinese, Cantonese, Japanese and Korean. The window
shows the download's progress, and a broken download picks up where it stopped. Updates: **Check for updates** in the
window installs the new version and restarts Kotodama. Uninstalling keeps your settings and downloaded models in
`%LOCALAPPDATA%\Kotodama`; delete that folder to remove them too.

**Linux / Steam Deck:** download `Kotodama-<version>-linux.tar.gz`, unpack it, run `Kotodama/Kotodama`. Teardown
runs through Proton; Kotodama finds its files inside Teardown's Proton folder.

## What it does

| Part | What |
|---|---|
| Voices | Each voice gets the game's volume, direction and muffling (distance, walls), mixed with a short delay. |
| Speech detection | Silero VAD v5 finds where a line starts and ends. |
| Speech to text | The line so far is written again every second (the live words: only what two passes agree on, so they never jump back), then once more when you stop. Each language has its own model: Parakeet TDT 0.6B v3 (English, other European languages), GigaAM v3 (Russian), SenseVoice Small (Mandarin, Cantonese, Japanese, Korean). |
| Word times | Each word's start time travels with the text, so a player who walks up mid-sentence sees only what was said after they arrived. |
| Several languages | Pick every language you speak. With several, SpeechBrain's VoxLingua107 detector splits a line by language (choosing among exactly yours), and each stretch is written by its own model. A line can mix languages. |

## Languages

In Kotodama's window, **Languages I speak** → **Choose...**: tick every language you speak. Kotodama loads only the
speech models those need, and shows each model, whether it is loaded and about how much memory it takes. Fewer
languages are lighter and more accurate. Until you choose, it follows the game's setting ("Language I speak").

| Support | Languages | Model, memory |
|---|---|---|
| Fully supported | English, Spanish, French, German, Italian, Portuguese, Dutch, Polish, Ukrainian | Parakeet v3, ~0.7 GB |
| | Russian | GigaAM v3, ~0.25 GB |
| | Mandarin, Cantonese, Japanese, Korean | SenseVoice, ~0.26 GB |
| Soft support (beta: less accurate) | Czech, Slovak, Romanian, Croatian, Bulgarian, Finnish, Swedish, Hungarian | Parakeet v3 |
| Weak support (experimental: many words come out wrong) | Danish, Estonian, Latvian, Lithuanian, Slovenian, Greek, Maltese | Parakeet v3 |

Several languages add the language detector (~0.1 GB). The first time a model is needed it downloads once; loading
it takes 1.5 to 3.5 s.

## Run from source

Kotodama is a Rust program ([`app/`](app/)): a few small crates, listed in [`app/DESIGN.md`](app/DESIGN.md). It needs
Rust (stable; on Windows the MSVC build tools).

    cd app
    cargo run -p kotodama                       # the window
    cargo run -p kotodama -- --cli --help       # the command line, with test modes (no microphone needed)

The speech engine is [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) with ONNX Runtime; its libraries download
with the first build. The language detector is built once with `python engine/export_lid.py` (needs torch,
speechbrain and onnx, in a separate environment if you like) and found in `export/lid/`.

The first version was Python ([`engine/`](engine/)). It stays as the reference the Rust app is tested against:
`app/fixtures/make_fixtures.py` writes its answers, and the Rust tests compare. The benchmarks behind every model
choice are in [`bench/`](bench/).

## Build

    python build.py          # dist/Kotodama/ and, on Windows with Inno Setup 6, dist/Kotodama-Setup-<version>.exe

The installed app is one program (`Kotodama.exe`, with the C runtime built in), the speech engine's two libraries
(`sherpa-onnx-c-api.dll`, `onnxruntime.dll`) and the language detector: about 90 MB, a 52 MB installer. GitHub
Actions builds Windows and Linux on every push ([`.github/workflows/build.yml`](.github/workflows/build.yml)); a
`v<version>` tag makes a draft release.

`Kotodama --selftest` checks that a build's native parts load: the window, sound, sherpa-onnx, the shipped language
detector, HTTPS for the model downloads and the update check. CI runs it on each build, and on Windows also on the
installed copy, before it uninstalls it again. The installer takes `/VERYSILENT` for an install without questions;
add `/RELAUNCH=1` to start Kotodama afterwards (the updater does).

## Tests

    cd app
    cargo test --workspace                          # each part against the Python reference's answers
    cargo test --workspace -- --include-ignored     # + the real speech models and the sound devices (this PC)
    python engine/test_e2e.py                       # with KOTODAMA_EXE=dist/Kotodama/Kotodama.exe: a fake game,
                                                    # the built app end to end

## License

MIT (see [LICENSE](LICENSE)). The models and libraries Kotodama uses, with their licenses, are listed in
[THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt).
