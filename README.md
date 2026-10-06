# Kotodama

**Proximity voice chat with live speech-to-text, for games.** Kotodama ("word spirit" in Japanese) runs next to
your game. It plays the other players' voices placed where they stand: louder when close, from their side, muffled
behind walls. It also writes what you say as you say it, in many languages, so the game can show your words in
speech bubbles and a chat history. A spoken line can even open a door.

Everything runs on your own PC: no audio or text is sent to any service. **Status:** early prototype (version 0.1).

## Games

Pick the game in Kotodama's window. Each game is a small module in [`engine/games/`](engine/games/).

| Game | What it needs |
|---|---|
| Teardown | The [Proximity Babble Chat](https://github.com/AgeOfAlgorithms/teardown-prox-text-chat-mod) mod (Steam Workshop) |

Adding a game: see [PROTOCOL.md](PROTOCOL.md), "Adding a game".

## Install (Windows)

1. Download `Kotodama-Setup-<version>.exe` from [Releases](https://github.com/AgeOfAlgorithms/proximity-voice-chat-STT-engine/releases).
2. Run it. It installs for your user only, so it needs no administrator rights.
3. Start Kotodama, pick your game, then start the game.

The first time you speak, Kotodama downloads the speech model for your language once: about 670 MB for English and
the other European languages, 225 MB for Russian, 240 MB for Chinese, Cantonese, Japanese and Korean. Updates:
**Check for updates** in the window installs a new version for you.

**Linux / Steam Deck:** download `Kotodama-<version>-linux.tar.gz`, unpack it, run `Kotodama/Kotodama`. Teardown
runs through Proton; Kotodama finds its files inside Teardown's Proton folder.

## What it does

| Part | What |
|---|---|
| Voices | Each voice gets the game's volume, direction and muffling (distance, walls), mixed with a short delay. |
| Speech detection | Silero VAD v5 finds where a line starts and ends. |
| Speech to text | The line so far is written again every second (the live words: only what two passes agree on, so they never jump back), then once more when you stop. Each language has its own model: Parakeet TDT 0.6B v3 (English, other European languages), GigaAM v3 (Russian), SenseVoice Small (Mandarin, Cantonese, Japanese, Korean). |
| Word times | Each word's start time travels with the text, so a player who walks up mid-sentence sees only what was said after they arrived. |
| "Auto" language | SpeechBrain's VoxLingua107 detector splits a line by language, and each stretch is written by its own model. A line can mix languages. |

## Languages

| Support | Languages |
|---|---|
| Supported | English, Spanish, French, German, Italian, Portuguese, Dutch, Polish, Ukrainian, Russian, Mandarin, Cantonese, Japanese, Korean, and Auto |
| Beta | Czech, Slovak, Romanian, Croatian, Bulgarian, Finnish, Swedish, Hungarian |

## Run from source

Python 3.12 with pip packages:

    pip install numpy sounddevice sherpa-onnx onnxruntime huggingface_hub psutil
    python engine/kotodama.py                 # the window
    python engine/teardown_helper.py --help   # the command line, with test modes (no microphone needed)

The language detector is built once with `python engine/export_lid.py`. That needs torch, speechbrain and onnx, in a
separate environment if you like.

## Build

    pip install nuitka ordered-set zstandard
    python build.py          # dist/Kotodama/ and, on Windows with Inno Setup 6, dist/Kotodama-Setup-<version>.exe

[Nuitka](https://nuitka.net) compiles the Python into a native program. Unlike self-unpacking Python executables,
which antivirus programs often flag, it doesn't unpack itself at runtime. GitHub Actions builds Windows and Linux on
every push ([`.github/workflows/build.yml`](.github/workflows/build.yml)); a `v<version>` tag makes a draft release.

## Tests

    python engine/test_app.py        # the updater, finding games, the runtime, the mixer's filter
    python engine/test_helper.py     # the Teardown link, the mixer, word times
    python engine/test_asr.py        # the speech models on recorded lines (needs export/ from bench/)
    python engine/test_e2e.py        # a fake game, end to end
    python engine/test_auto_speech.py

## License

MIT (see [LICENSE](LICENSE)). The models and libraries Kotodama uses, with their licenses, are listed in
[THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt).
