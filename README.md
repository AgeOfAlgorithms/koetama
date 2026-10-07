<p align="center">
  <img src="app/assets/kotodama-256.png" width="128" height="128" alt="Kotodama">
</p>

<h1 align="center">Kotodama</h1>

<p align="center">
  <b>Proximity voice chat with live speech-to-text, for game mods.</b><br>
  Hear other players where they stand. Your words appear in the game as you say them.
</p>

<p align="center">
  <a href="https://github.com/AgeOfAlgorithms/proximity-voice-chat-STT-engine/releases"><img src="https://img.shields.io/github/v/release/AgeOfAlgorithms/proximity-voice-chat-STT-engine?include_prereleases&label=release&color=f26d2a" alt="Release"></a>
  <a href="https://github.com/AgeOfAlgorithms/proximity-voice-chat-STT-engine/actions/workflows/build.yml"><img src="https://github.com/AgeOfAlgorithms/proximity-voice-chat-STT-engine/actions/workflows/build.yml/badge.svg" alt="Build"></a>
  <img src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux-3a2c2b" alt="Windows | Linux">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-f59e0b" alt="MIT license"></a>
</p>

Kotodama ("word spirit" in Japanese) runs next to your game. It plays the other players' voices placed where they
stand: louder when close, from their side, muffled behind walls. It also writes what you say as you say it, so the
game can show your words in speech bubbles and a chat history. A spoken line can even open a door.

**Everything runs on your own PC.** No audio or text is sent to any service, and no account is needed.

> **Status:** early prototype, version 0.2.

## Highlights

- **Positional voices.** Each voice gets the game's volume, direction and muffling, with a short delay.
- **Live words.** Your line appears while you talk and is finished when you stop. Words already shown never jump back.
- **Many languages.** 14 fully supported, 8 in beta, 7 experimental. Speak several and mix them in one line.
- **Fair proximity.** Each word carries the time it was said, so a player who walks up mid-sentence sees only what
  they could have heard.
- **Light on your PC.** Only the speech models for your languages load: about 0.9 GB of memory for one language, up
  to 1.5 GB for several. It runs on the CPU, below the game's priority, and stays idle while you are silent.
- **Any game with a mod.** Games connect through small profile files, not plugins: anyone can add one.

## Install

**Windows:** download `Kotodama-Setup-<version>.exe` from
[Releases](https://github.com/AgeOfAlgorithms/proximity-voice-chat-STT-engine/releases) and run it. It installs for
your user only, with no administrator rights. Then start Kotodama, pick your game mod and start the game.

**Linux / Steam Deck:** download `Kotodama-<version>-linux.tar.gz`, unpack it and run `Kotodama/Kotodama`. Games
running through Proton are found inside their Proton folder.

The first time you speak, Kotodama downloads the speech model for your language once, showing its progress:

| Model | Languages | Download |
|---|---|---|
| Parakeet v3 | English and the other European languages | 670 MB |
| GigaAM v3 | Russian | 225 MB |
| SenseVoice | Mandarin, Cantonese, Japanese, Korean | 240 MB |

**Updates:** the window's **Check for updates** installs a new version and restarts Kotodama. Uninstalling keeps your
settings and models in `%LOCALAPPDATA%\Kotodama`; delete that folder to remove them too.

## Game mods

| Game | Mod | Uses |
|---|---|---|
| Teardown | [Proximity Babble Chat](https://steamcommunity.com/sharedfiles/filedetails/?id=3812301496) | voices and speech to text |

**Adding a game:** a mod made for Kotodama comes with a small profile file (`.json`). In the window, open the game
mod list and choose **Add game mod...**. Kotodama shows what the profile reads, writes and listens on before adding
it. A profile is not a program: it only points Kotodama's built-in connectors at a game's files or at a local port,
and it can ask for voices, speech to text, or both.

**Making a mod for Kotodama:** see [PROTOCOL.md](PROTOCOL.md) for the profile format and the two connectors (files,
or a local socket), with an example profile and test client in [`examples/`](examples/).

## Languages

Open **Languages I speak → Choose...** and tick every language you speak. Fewer languages are lighter and more
accurate. With several, Kotodama tells them apart as you speak, choosing only among yours. Until you choose, it
follows the game's own language setting.

| Support | Languages |
|---|---|
| **Fully supported** | English, Spanish, French, German, Italian, Portuguese, Dutch, Polish, Ukrainian, Russian, Mandarin, Cantonese, Japanese, Korean |
| **Beta** (less accurate) | Czech, Slovak, Romanian, Croatian, Bulgarian, Finnish, Swedish, Hungarian |
| **Experimental** (many words come out wrong) | Danish, Estonian, Latvian, Lithuanian, Slovenian, Greek, Maltese |

## How it works

| Part | How |
|---|---|
| Voices | The game sends each speaker's volume, direction and muffle; Kotodama mixes them in stereo, low-passed behind walls. |
| Speech detection | Silero VAD v5 finds where a line starts and ends. |
| Speech to text | The line so far is transcribed again every second; only words two passes agree on are shown. One more pass when you stop gives the finished line. |
| Language detection | SpeechBrain's VoxLingua107 detector splits a mixed-language line into stretches, each written by its language's model. |
| Engine | [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) and ONNX Runtime, on the CPU. |

## For developers

Kotodama is a Rust program in [`app/`](app/), split into small crates ([`app/DESIGN.md`](app/DESIGN.md)). It needs
stable Rust, and on Windows the MSVC build tools.

```sh
cd app
cargo run -p kotodama                     # the window
cargo run -p kotodama -- --cli --help     # the command line, with test modes (no microphone needed)
cargo test --workspace                    # every part against the Python reference's answers
python build.py                           # dist/Kotodama/ and, with Inno Setup 6, the Windows installer
```

- The language detector is built once with `python engine/export_lid.py` (torch, speechbrain and onnx).
- The first version was Python ([`engine/`](engine/)). It stays as the reference the Rust tests compare against, and
  [`bench/`](bench/) holds the benchmarks behind every model choice.
- `Kotodama --selftest` checks a build's native parts. CI runs it on every build, and on Windows also installs,
  tests and uninstalls the installer.

## License

MIT, see [LICENSE](LICENSE). The models and libraries Kotodama uses are listed with their licenses in
[THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt).
