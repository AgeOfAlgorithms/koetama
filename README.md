# proximity-voice-chat-STT-engine

A helper program that gives a game proximity voice chat and speech-to-text, running on each player's own PC.

- **Voices:** it plays the other players' voices, each at the volume, direction and muffling the game reports
  (distance, walls).
- **Speech to text:** it listens to the player's microphone and writes what they say, both live while they talk
  and as a finished line after, so the game can show it in speech bubbles and a chat history.
- **Offline:** everything runs on the CPU, on this machine; no audio or text is sent to any service.

**Status:** prototype. The first game is Teardown, through the mod Proximity Babble Chat.

## How it works

| Part | What |
|---|---|
| Speech detection | Silero VAD v5 finds where a line starts and ends. |
| Speech to text | The line so far is transcribed again every second (the live words), then once over the whole line (the finished line), by the model for the player's language: Parakeet TDT 0.6B v3 for English and other European languages, GigaAM v3 for Russian, SenseVoice Small for Mandarin, Cantonese, Japanese and Korean. |
| "Auto" language | SpeechBrain's VoxLingua107 language detector splits a line by language, and each stretch is written by that language's model, so a line can mix languages. |
| Game link | Teardown mods can't open sockets or write files, so the game and the helper talk through the game's save file and small files next to the mod. See [PROTOCOL.md](PROTOCOL.md). |

## Languages

| Support | Languages |
|---|---|
| Supported | English, Spanish, French, German, Italian, Portuguese, Dutch, Polish, Ukrainian, Russian, Mandarin, Cantonese, Japanese, Korean, and Auto |
| Beta | Czech, Slovak, Romanian, Croatian, Bulgarian, Finnish, Swedish, Hungarian |

## Running it (Windows)

1. Create a Python 3.12 environment with pip packages only:

       pip install numpy scipy sounddevice sherpa-onnx onnxruntime huggingface_hub psutil

2. Build the language detector once. This needs a separate environment with torch, speechbrain and onnx:

       python engine/export_lid.py

3. Start the helper, then start a Teardown level with the mod on:

       python engine/helper.py

The speech models download automatically the first time they're needed: Parakeet about 670 MB, GigaAM 225 MB,
SenseVoice 240 MB.

## Tests

    python engine/test_helper.py
    python engine/test_asr.py
    python engine/test_e2e.py
    python engine/test_auto_speech.py

`test_asr.py`, `test_e2e.py` and `test_auto_speech.py` need the benchmark recordings in `export/`. The
`bench/` scripts make them.

## Credits

The models and libraries used are listed, with their licenses, in
[THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt).
