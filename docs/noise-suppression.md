# Noise suppression for Koetama's voice: what to build

*2026-10-15. Researched from the web on 2026-10-15 (vendor pages, papers, repositories, crates.io and Hugging Face
APIs, issue trackers), plus Koetama's own code and the 2026-10-04 speech-to-text benchmark (`export/asrbench/second.md`).
Every figure is the source's own, on the hardware it names, unless marked **measured by us**. "[i]" = an inference no
source states. "Not published" = searched for and not found. No number here is estimated unless marked [i].*

## Summary

**Build first: "Standard" = RNNoise through `nnnoiseless`.** Pure Rust (BSD-3, code and weights), 48 kHz native,
480-sample (10 ms) frames, weights about 85 kB inside the binary. It is what Mumble, OBS and (since September 2026)
Vivox use. **Measured** (2026-10-15, below): 10 ms delay, 0.5 % of one core; removes a vacuum cleaner well (38-43 dB),
keyboard (18-19 dB) and mouse clicks (9-12 dB) less.

**As an option: "Strong" = DeepFilterNet 3 (measured 2026-10-15, below).** 7.8 % of one core and 30 ms on the
user's Ryzen 7 3700X; it keeps speech best of the three, removes keyboard and mouse noise well, but leaves a steady
fan's hum. MIT/Apache, but its library is unmaintained (last commit 2024-10; it builds only with tract pinned to
0.21.4), so it would be vendored or kept on a pinned git revision. **DPDFNet** removes the most noise (the pauses
go silent) but costs a third of a core and 40 ms: not beside a game for now.

**Skip:**
- GTCRN and UL-UNAS: 16 kHz only. GTCRN scored below RNNoise on a hard multilingual set and hurt our speech to text.
- Facebook Denoiser: CC-BY-NC.
- Krisp: proprietary, sales-gated.
- NVIDIA Broadcast, AMD Noise Suppression: tied to their GPUs. Players can already run them in front of Koetama.
- URGENT-challenge winners: offline, up to 102M parameters.
- The tonarino `webrtc-audio-processing` crate: no MSVC build.
- Windows Voice Clarity as the main route: Windows 11 only, and the user's PC runs Windows 10.

**Speech to text stays on the raw microphone - for now.** It already does: the 16 kHz branch is taken before the
voice path. Denoisers raised Whisper's errors in two 2025-2026 studies, and ours in mild noise; but on real speech in
loud typing DeepFilterNet 3 cut our errors to a third ("Does it help speech to text?", below).

**Still to measure** (the rest was measured 2026-10-15, "Measured on our PC"):
- The CPU cost with a game running and Koetama below its priority (measured without one: `bench/denoise.py --cpu-only`).
- How each model treats keyboard clicks, game sound leaking from speakers, laughter and shouting. No source covers
  these.
- Whether `nnnoiseless` can load the 2024 RNNoise 0.2 weights. It bundles the 2017 ones.
- Which bandwidth Opus picks at 24 kbit/s VOIP: it decides how much a full-band model buys over a 16 kHz one.

## Measured on our PC (2026-10-15)

`bench/denoise.py` (with `bench/denoise/`, a Rust program that runs each model as the app would: one thread, the
model's own 10 ms frame) on the user's PC: Ryzen 7 3700X, no game running. Eight English benchmark sentences (48 s,
computer voices recorded at 16 kHz, so nothing above 8 kHz: this cannot show what the 48 kHz models do to high
frequencies), speech at -20 dBFS, mixed with real keyboard typing, mouse clicks and a vacuum cleaner (ESC-50, local
only: CC BY-NC) and a synthetic fan (pink noise and a 120 Hz hum), at 5 and 15 dB under the speech. Models:
RNNoise (`nnnoiseless` 0.5, its 2017 weights), DPDFNet `dpdfnet2_48khz_hr` (sherpa-onnx 1.13.8 streaming), DeepFilterNet
3 (libDF at its last commit, tract pinned to 0.21.4 - newer tract releases no longer build with it - and the
thresholds of DeepFilterNet's own command line, -15 / 35 / 35: with the library's defaults, -10 / 30 / 20, it
skipped most noisy frames as "clean speech" and scored worse than no processing; that first run is kept in
`export/noise/results-run1-dfn3-library-defaults.md`). Listening files (each noise at 5 dB, before and after each
model): `export/noise/listen/`.

**Cost and delay** (paced in real time: each frame waits for the moment a microphone would hand it over):

| model | delay | CPU per 10 ms frame: mean / p99 / max | share of one core | load |
|---|---|---|---|---|
| RNNoise | 10 ms | 0.05 / 0.07 / 0.21 ms | **0.5 %** | instant |
| DeepFilterNet 3 | 30 ms | 0.78 / 1.19 / 1.91 ms | **7.8 %** | 0.4 s |
| DPDFNet 48 kHz | 40 ms | 3.37 / 3.66 / 5.10 ms | **33.7 %** | 0.2 s |

DPDFNet's delay is 40 ms, not the README's 20 (the [i] above holds). Its cost is a third of a core, ~4x
DeepFilterNet 3's: too much to run beside a game by default.

**What each does** (noise removed = the noise alone through the model; SI-SDR = the noisy speech through the model
against the clean speech, higher is better; WER = Koetama's speech to text on the result):

| noise | no processing: SI-SDR / WER | RNNoise: removed / SI-SDR / WER | DeepFilterNet 3 | DPDFNet 48 kHz |
|---|---|---|---|---|
| keyboard, 5 dB | 0.6 / 1 % | 19 dB / 8.2 / 0 % | 27 dB / **14.2** / 3 % | **56 dB** / 12.3 / 1 % |
| keyboard, 15 dB | 10.6 / 0 % | 18 dB / 11.5 / 0 % | 22 dB / **19.7** / 0 % | **59 dB** / 13.8 / 1 % |
| mouse, 5 dB | 0.6 / 1 % | 12 dB / 8.3 / 1 % | 23 dB / **15.3** / 1 % | **27 dB** / 12.6 / 0 % |
| mouse, 15 dB | 10.6 / 0 % | 9 dB / 11.3 / 1 % | 17 dB / **20.6** / 0 % | **48 dB** / 14.0 / 0 % |
| vacuum, 5 dB | 0.6 / 1 % | 43 dB / 7.5 / 8 % | 22 dB / **11.5** / 12 % | **45 dB** / 10.6 / 4 % |
| vacuum, 15 dB | 10.6 / 1 % | 38 dB / 11.4 / 1 % | 22 dB / **17.4** / 0 % | **45 dB** / 13.4 / 0 % |
| fan, 5 dB | 0.5 / 0 % | 13 dB / 8.7 / 1 % | 7 dB / 7.0 / 3 % | **59 dB** / **11.6** / 1 % |
| fan, 15 dB | 10.6 / 0 % | 43 dB / 11.9 / 0 % | 5 dB / **14.1** / 1 % | **59 dB** / 13.7 / 0 % |
| clean speech kept (SI-SDR) | - | 13.0 | **40.3** | 13.7 |

What it says:
- **DPDFNet removes the most noise by far** (45-59 dB: the pauses go silent), on every noise, but changes clean speech
  as much as RNNoise does (13.7 dB) and costs a third of a core.
- **DeepFilterNet 3 keeps speech best** (clean speech nearly untouched, 40 dB; the best SI-SDR on every real noise)
  but leaves more noise between words (17-27 dB removed) and barely touches the steady fan with its hum (5-7 dB).
- **RNNoise is ~70x cheaper than DeepFilterNet 3** and good on steady noise (vacuum 38-43 dB), weak on clicks
  (keyboard 18-19 dB, mouse 9-12 dB), and changes clean speech somewhat (13 dB).
- **Speech to text barely cares**: Parakeet already understood the raw mixes (0-1 % WER), and the denoisers made it
  worse at the vacuum's 5 dB (RNNoise 8 %, DeepFilterNet 3 12 %). Keeping speech to text on the raw microphone is
  right.
- SI-SDR punishes any change to the waveform, audible or not: what players hear needs the listening files.

### Does it help speech to text? (2026-10-15, `bench/denoise_stt.py`)

The user doubted the first benchmark's "speech to text barely cares" - rightly: its speech (computer voices) was too
clean and its noise too quiet to tell (0-1 % word errors with no suppression). Harder: every line, noise at the speech's
own level and louder, and real people (`bench/datasets.py`; local only): Golos far-field (Sber; Russian spoken to a
smart speaker across a room) and speechocean762 (English read by Mandarin-speaking learners, half of them children,
pronunciation scored 3 of 10 - the worst still spoken). Word error rate of Koetama's own speech to text:

| noise | Golos far-field, Russian (no noise: 6 %): raw / RNNoise / DeepFilterNet 3 | computer voices, English (1.3 %) | speechocean762, English (72 %) |
|---|---|---|---|
| keyboard +5 dB | 30 / 17 / **15** % | **0** / 0.6 / 1.3 % | 82 / 82 / 81 % |
| keyboard 0 dB | 74 / 25 / **21** % | **0.6** / 1.3 / 3.2 % | 94 / **82** / 83 % |
| keyboard -5 dB | 91 / 43 / **35** % | 17 / 17 / **8** % | 98 / **86** / 88 % |
| vacuum +5 dB | 40 / 45 / **38** % | **1.9** / 2.6 / 5.2 % | 91 / 89 / 89 % |
| vacuum 0 dB | 73 / 68 / **60** % | 39 / 34 / **17** % | 97 / 93 / **91** % |
| vacuum -5 dB | 99 / 96 / **88** % | 95 / 83 / **62** % | 98 / 97 / **95** % |
| fan +5 dB | 23 / 27 / **22** % | **0.6** / 3.2 / 1.9 % | **84** / 87 / 89 % |
| fan 0 dB | **31** / 39 / 38 % | **5.8** / 9.0 / 11 % | **88** / 92 / 91 % |
| fan -5 dB | 66 / 78 / **61** % | **36** / 46 / 45 % | (not run) |

(Russian and Chinese computer voices: the same picture as English - export/noise/stt-results-en-ru-zh.md.)

- **On real speech in keyboard or vacuum noise, DeepFilterNet 3 helps speech to text a lot**: Golos with typing at
  the speech's level, 74 % -> 21 %. RNNoise helps nearly as much on typing, less on the vacuum.
- **In mild noise on clean speech it costs a little** (computer voices, +5 dB: 0 -> 1.3 %), and **a steady fan with
  its hum is the exception everywhere**: suppression is neutral or worse there.
- speechocean762 is near the floor (72 % with no noise at all: the words are mispronounced, the reading halting -
  "He reached for her hand" came out "He Rodder. Fat for here."): it says little either way.
- So "speech to text stays on the raw microphone" (above) is right for mild noise and wrong for loud typing. Whether to
  give speech to text the cleaned voice when the player picks Strong - or when the room is loud - is the next question.

### On real voices only (2026-10-15, later)

The user asked for real voices throughout, with accents. Four real-speech sets (`bench/datasets.py`, local only):
EdAcc (conversations in English: Kenyan, Indian, Ghanaian, Nigerian, Romanian, Latin American and Spanish accents, 56
sentences), LibriSpeech test-clean (native English, 40 speakers), Golos crowd (Russian, read close to a phone) and Golos
far-field. Real noise only: keyboard, vacuum (ESC-50) and 7 real fans and air conditioners (Wikimedia Commons, CC BY /
BY-SA), mixed digitally. Word error rate, raw -> DeepFilterNet 3:

| noise | EdAcc (accents) | LibriSpeech (native) | Golos crowd (Russian) | Golos far-field (Russian) |
|---|---|---|---|---|
| none | 18.4 -> 19.9 % | 3.9 -> 3.0 % | 20.0 -> 19.6 % | 6.0 -> 6.5 % |
| keyboard +5 dB | 32.8 -> 39.8 % | 12.1 -> 6.4 % | 48.0 -> 35.1 % | 32.7 -> 18.6 % |
| keyboard 0 dB | 66.3 -> 50.4 % | 25.4 -> 21.2 % | 70.2 -> 39.6 % | 80.4 -> 28.1 % |
| vacuum +5 dB | 49.8 -> 63.8 % | 19.4 -> 30.7 % | 75.1 -> 54.7 % | 36.7 -> 36.2 % |
| vacuum 0 dB | 84.6 -> 81.1 % | 58.0 -> 63.1 % | 83.6 -> 74.2 % | 72.4 -> 60.3 % |
| real fans +5 dB | 63.6 -> 69.1 % | 52.7 -> 44.9 % | 59.1 -> 66.7 % | 37.7 -> 54.8 % |
| real fans 0 dB | 76.6 -> 74.5 % | 66.3 -> 61.8 % | 77.3 -> 80.9 % | 63.8 -> 75.4 % |

- With no noise it costs nothing that matters (-0.9 to +1.5 points).
- **Typing**: cleaning helps almost everywhere, a lot when loud (Golos far-field 80 -> 28 %).
- **Steady noise (vacuum, fans)**: a coin flip - it often hurts English (LibriSpeech vacuum +5 dB 19 -> 31 %).
- The spread is wide: which noise lands under which sentence moves results (Golos far-field, real fans 0 dB: -7
  points in one run, +12 in the next). **Decision (2026-10-15): speech to text stays on the raw microphone** - giving it
  the cleaned voice would help loud typists and hurt some players with a fan. Switching only for clicky noise would need
  its own design and tests.
- Voice quality on EdAcc (`export/noise/results-all-edacc.md`, listening files in `export/noise/listen-edacc/`): the
  "clean speech kept" scores are not meaningful there (EdAcc's recordings carry room noise, which the models remove);
  CPU paced: RNNoise 0.6 %, DeepFilterNet 3 10.6 %, DPDFNet 34.8 % of one core.

### Gentler cleaning for speech to text (2026-10-15/16)

The user asked: mix raw audio back in, and clean less deeply. `bench/denoise_stt.py <set> --variants
raw,dfn3,mix20,mix40,lim12,lim6 --noises keyboard,vacuum,fans --snrs 5,0 --none`, the same four real-speech sets and
real noises as above. `mixNN`: DeepFilterNet 3's output (lined up: its delay, 30 ms) with NN % of the raw mix added back;
`limNN`: DeepFilterNet 3 told to take off at most NN dB (`denoise_bench --atten NN`, libDF's attenuation limit). Every
variant hears the same noisy mix within a set, so they compare fairly; the raw and dfn3 numbers differ from the table
above because the noise lands differently each run. Word error rate, averaged over the four sets (28 rows: 4 sets x no
noise + 6 noisy):

| noise | raw | dfn3 | mix20 | mix40 | lim12 | lim6 |
|---|---|---|---|---|---|---|
| none | 12.1 % | 12.2 % | 12.4 % | 12.3 % | **11.7 %** | 11.8 % |
| keyboard | 41.7 % | 27.7 % | 27.1 % | 26.7 % | **25.2 %** | 27.2 % |
| vacuum | 59.9 % | 57.4 % | 46.0 % | 46.0 % | **44.9 %** | 48.2 % |
| fans | 67.4 % | 68.5 % | 65.6 % | **65.3 %** | 65.7 % | 65.5 % |
| all 28 | 50.0 % | 45.6 % | 41.4 % | 41.2 % | **40.5 %** | 41.9 % |
| rows > 1 point worse than raw | - | 9 | 6 | 6 | 4 | **3** |
| worst row vs raw | - | +15.1 | +8.3 | +6.1 | +8.1 | **+5.6** |

- **Cleaning less deeply beats cleaning fully everywhere.** Full DeepFilterNet 3 turned the steady noises into a coin
  flip; with a 12 dB limit vacuum goes 59.9 -> 44.9 % (full: 57.4 %), and no noise costs nothing (11.7 vs 12.1 %).
- **lim12 is the best overall** (40.5 % against raw 50.0 %), and mixing back in does about as well (41.2-41.4 %). lim6 is
  the safest (3 rows worse than raw, at most +5.6 points) and gives up some of the gain in loud noise (Golos crowd vacuum
  0 dB: 75.1 % against lim12 65.8 %).
- **The weak spot is Golos far-field with fans**: everything cleaned is worse than raw there (fans +5 dB raw 46.2 %, lim12
  52.8 %, lim6 49.7 %; 0 dB raw 67.3 %, lim12 75.4 %). Far-field speech under a fan is the one case where the model takes
  speech for noise. EdAcc with fans is at the floor (91-94 % for every variant): it says nothing.
- Per set, the logs: `export/noise/stt-variants.log`, `-2.log` (EdAcc), `-3.log` (LibriSpeech, Golos crowd, Golos
  far-field); not committed.
- Cost: speech to text on a cleaned voice needs its own DeepFilterNet 3 run (limited) beside the one for the voice sent -
  another 8-11 % of one core - unless the player picked Strong, where the two could share one run only if the voice sent
  also used the limit.

## What players compare us to

Players hear Discord, so the bar is Krisp: an ML model on the player's CPU, on by default since 2020. Game voice SDKs
sit well below that bar: WebRTC's classic suppressor, with RNNoise as Vivox's 2026 upgrade. A built-in RNNoise already
matches or beats every game SDK found [i]. A DFN-class "Strong" mode is the open-source route to Discord-like quality.

| product | what it uses | latency | resources | license / availability to Koetama |
|---|---|---|---|---|
| Discord | Krisp, licensed, on device (desktop since Apr 2020, then mobile) ([Wikipedia](https://en.wikipedia.org/wiki/Krisp), [Discord blog](https://discord.com/blog/tune-into-discord-with-krisp-noise-suppression-on-ios-and-android)). Its "Standard" option has no official description: guides disagree (noise gate vs frequency filter); WebRTC NS is likely [i]. No sign of an in-house model as of Oct 2026 | not published | not published; a third-party guide (2026-09-08) says Krisp switches off under very high CPU ([Technobezz](https://www.technobezz.com/how-to-fix-discord-noise-suppression-not-working)) | n/a |
| Krisp SDK | proprietary RTC noise cancellation, CPU, narrow/wide/full band; Windows, Linux, macOS, WASM, mobile; C and Rust bindings ([developers](https://krisp.ai/developers/), [docs](https://sdk-docs.krisp.ai/docs/getting-started)) | "ultra-low", no number; a third party claims ~25 ms ([Fora Soft](https://www.forasoft.com/learn/ai-for-video-engineering/articles-ai/real-time-noise-suppression-krisp-rnnoise-deepfilternet), unverified) | Voice Isolation (not the RTC model), p95 per 20 ms frame on cloud CPUs, Aug 2026: Lite 0.7-1.0 ms, Balanced 2-3 ms ([Krisp blog](https://krisp.ai/blog/voice-isolation-cpu-optimization/)). Model size not published | sales form only; no public price, no open-source program. Cannot ship in an MIT app [i] |
| NVIDIA Broadcast / Maxine AFX | neural, Tensor Cores; RTX 2060 or better; Windows only ([FAQ](https://www.nvidia.com/en-gb/geforce/broadcasting/broadcast-app/faq)) | not published | AFX Denoiser on an RTX 3500 Ada laptop: p50 0.374 ms per 10 ms frame, VRAM 215 MiB p95 ([perf guide](https://docs.nvidia.com/maxine/afx/latest/WindowsAFXSDK/ReleaseReadiness.html)); Broadcast costs 5-10 % game performance on an RTX 5090 | end-user tool; SDK license terms not stated. Players can put it in front of Koetama as a virtual microphone |
| AMD Noise Suppression | driver feature, Radeon RX 6000+ / Ryzen 6000+ APUs, Windows ([AMD](https://www.amd.com/en/products/software/adrenalin/amd-noise-suppression.html)) | not published | not published | end-user tool only |
| Windows 11 Voice Clarity (Deep Noise Suppression) | "low complexity AI models" on the CPU, also echo cancellation and dereverberation; for apps capturing in Communications signal-processing mode; all PCs since build 26040 (Jan 2024) ([Windows Central](https://www.windowscentral.com/software-apps/windows-11/all-windows-11-pcs-will-soon-support-this-ai-feature-once-exclusive-to-surface-no-npu-required)) | not published | not published | free, no code beyond the capture mode, but not on Windows 10 or Linux, and an OEM driver can replace it [i] |
| Vivox (Unity) | WebRTC NS on by default; RNNoise after it, off by default, strength 0-100 (default 40), since Core 5.28.0 (2026-09-23) ([Unity](https://discussions.unity.com/t/cleaner-voice-chat-is-here-rnnoise-is-coming-to-vivox/1737421)) | not published | not published | proprietary SDK |
| Epic Online Services | unnamed suppressor, on by default (`DisableNoiseSupression`); EOS Voice not supported on Linux ([release notes](https://dev.epicgames.com/docs/epic-online-services/release-notes), [voice](https://dev.epicgames.com/docs/epic-online-services/multiplayer/voice-and-rtc-interface/voice-interface/voice-overview)) | not published | not published | proprietary SDK |
| ODIN (4Players) | `OdinApmConfig`: VAD, gate, echo canceller, high-pass, `noise_suppression_level`, transient suppressor, gain controller ([docs](https://docs.4players.io/voice/core/api/types/OdinApmConfig/)); the field set mirrors WebRTC APM one for one [i] | not published | not published | proprietary SDK |
| Photon Voice | none; voice detection only (staff answer, 2019) ([forum](https://forum.photonengine.com/discussion/14804/how-to-reduce-noise-echo-when-using-photon-voice)) | - | - | - |
| Steam voice (ISteamUser) | no suppression documented; output "is not the raw feed" ([Steamworks](https://partner.steamgames.com/doc/features/voice)) | - | - | - |
| Agora | proprietary "AI Noise Suppression", with an ultra-low-latency mode ([docs](https://docs.agora.io/en/realtime-media/voice/build/enhance-the-audio-experience/ai-noise-suppression/electron.md)) | not published | not published | paid service |
| WebRTC / Chrome | classic DSP: Wiener filter, SNR estimation, VAD; levels Low to VeryHigh ([sonora-ns](https://docs.rs/sonora-ns), [APM](https://webrtc.googlesource.com/src/+/3273b5efd40cc8a923d1d6f5f59fb235f2b981c5/modules/audio_processing/include/audio_processing.cc)). Chrome adds no neural NS; it only reports the Windows effect (2025 commit) ([Chromium](https://chromium.googlesource.com/chromium/src/+/5835457d91615d2d6cc582f3bda054c1f506ef49)) | 10 ms frames, about one frame added [i] | sonora's full AEC3 + NS + AGC2: 13.3 us per 10 ms frame, Apple M4 Max ([sonora](https://raw.githubusercontent.com/dignifiedquire/sonora/main/README.md)) | BSD-3, usable |

Others: Teams, Zoom and Google Meet run proprietary models. Meet's ran in the cloud at its 2020 launch
([Business Insider](https://www.businessinsider.in/tech/news/google-meet-gets-ai-noise-cancellation-feature/amp_articleshow/76295768.cms)).
Apple's Voice Isolation is a mode only the user can select, and only on Apple devices
([Apple forums](https://developer.apple.com/forums/thread/844448)). Dolby.io's voice APIs are gone: their docs have
redirected to OptiView since 2024 ([apis.io](https://apis.io/providers/voxeet/)).

## Open models

The research state of the art no longer measures real time. The DNS Challenge series ended with ICASSP 2023. It
required ≤ 20 ms algorithmic plus buffering latency and RTF ≤ 0.5 on one i5 thread
([Microsoft](https://www.microsoft.com/en-us/research/academic-program/deep-noise-suppression-challenge-icassp-2023)).
Its successor, URGENT (Interspeech 2025, ICASSP 2026), sets no latency or compute limit. The 2025 winner had 102M
parameters, and a generative entry hallucinated content
([Saijo et al.](https://www.isca-archive.org/interspeech_2025/saijo25_interspeech.pdf),
[arXiv 2601.13531](https://arxiv.org/html/2601.13531v1)). Real-time work continues in ultra-light causal models and in
Interspeech 2026's "Real-Time, Low-Latency and Edge" session ([ISCA](https://www.isca-archive.org/interspeech_2026/)).

| model | rate | latency (algorithmic) | params / compute | measured CPU (source, hardware) | model size | quality (metric, set) | code license | weights license | status |
|---|---|---|---|---|---|---|---|---|---|
| RNNoise 2017 weights (`nnnoiseless`) | 48k | 20 ms window, 10 ms hop, 10 ms lookahead ([demo](https://jmvalin.ca/demo/rnnoise/)); 10 ms re-blocking in practice ([sink](https://github.com/NC1107/sink/pull/86)) | ~87.5k weights, ~40 MFLOPS ([paper](https://ar5iv.labs.arxiv.org/html/1709.08243)) | 1.3 % of one i7-4800MQ core, plain C (paper); ~0.3 % of a core via nnnoiseless (sink, hardware not stated) | 85 kB at 8 bit | PESQ 2.29 VB-DEMAND ([UL-UNAS](https://arxiv.org/html/2503.00340)); PESQ 2.36 DPDFNet multilingual low-SNR set ([arXiv](https://arxiv.org/html/2512.16420v2)); DNSMOS OVRL 2.53 DNS3 blind ([GTCRN](https://github.com/Xiaobin-Rong/gtcrn)) | BSD-3 | BSD-3, by the top-level COPYING ([Debian review](https://dfsg-new-queue.debian.org/reviews/rnnoise)) | nnnoiseless 0.5.2, 2025-12-18 |
| RNNoise 0.2 (C) | 48k | not published (unchanged? [i]) | not published; a "little" model also offered | SSE4.1/AVX2 with run-time detection, no figure ([release](https://github.com/xiph/rnnoise/releases/tag/v0.2)) | not published | no public benchmark | BSD-3 | BSD-3; trained on public data only | 2024-04-15 |
| DeepFilterNet3 | 48k | 40 ms: 20 ms window, 10 ms hop, 2-frame lookahead ([paper](https://arxiv.org/pdf/2305.08227)); the LADSPA model has no lookahead, 20 ms minimum ([ladspa](https://github.com/Rikorose/DeepFilterNet/tree/main/ladspa)) | 2.14M / 0.35 GMAC/s | RTF 0.19, one thread, i5-8250U (paper); ~10 % of a core + ~150 MB as a LADSPA plugin (sink); RTF 1.15 underruns on a 13th-gen i9 ([#482](https://github.com/Rikorose/DeepFilterNet/issues/482)) | 7.98 MB `.tar.gz` | PESQ 3.17 VB-DEMAND; 2.76 multilingual set; DNSMOS OVL 2.958 DNS4 blind | MIT / Apache-2.0 | "permissive" per the paper, no separate file | v0.5.6, 2023-08-31; crate 0.2.5 (2022) |
| DPDFNet-2 48 kHz (`dpdfnet2_48khz_hr`) | 48k | first output after ~20 ms, then 10 ms per hop ([README](https://github.com/ceva-ip/DPDFNet)); deep filter lookahead 2 frames, so ~40 ms [i] | 2.58M / 2.42 GMAC/s | none on CPU (NPU only). **Measured by us:** the 16 kHz `dpdfnet2` (1.35 GMAC/s) took 0.150 s per s of audio, one thread, offline, Ryzen 7 3700X | 10.6 MB ONNX (sherpa release); 10.0 MB on Ceva's GitHub | 16 kHz variant: PESQ 3.14 multilingual set, DNSMOS OVL 3.104 DNS4 blind; the 48 kHz variant is not evaluated | Apache-2.0 | Apache-2.0 ([HF](https://huggingface.co/Ceva-IP/DPDFNet)) | paper 2025-12; repo pushed 2026-10-12 |
| DPDFNet-8 48 kHz | 48k | as above | 3.63M / 7.17 GMAC/s | the 16 kHz -8 is not real time on Ceva's NPU | 14.2 MB | PESQ 3.20 multilingual set (16 kHz) | Apache-2.0 | Apache-2.0 | as above |
| GTCRN | 16k | 32 ms window, 16 ms hop, no lookahead ([stream](https://raw.githubusercontent.com/Xiaobin-Rong/gtcrn/main/stream/gtcrn_stream.py)) | 48.2k / 33 MMAC/s | RTF 0.07 streaming, i5-12400 (README). **Measured by us:** 0.059 s per s, one thread, offline, Ryzen 7 3700X | 0.5 MB | PESQ 2.87 VB-DEMAND; 2.21 multilingual set (below RNNoise) | MIT | MIT (repo; no separate file) | pushed 2026-08-03 |
| UL-UNAS | 16k | 32 ms window, 16 ms hop, no lookahead | 171k / 35 MMAC/s | not published | not published | PESQ 3.09 VB-DEMAND; 2.30 DNS3 ([paper](https://arxiv.org/html/2503.00340)) | MIT | MIT (repo) | weights 2026-02-01 ([repo](https://github.com/Xiaobin-Rong/ul-unas)) |
| WebRTC NS (`sonora-ns`) | 48k (10 ms frames) | ~10 ms [i] | classic DSP | 13.3 us per 10 ms frame for the whole APM, M4 Max | none | not published | BSD-3 | - | sonora 0.2.0, 2026-07-29 |
| DTLN | 16k | 32 ms ([repo](https://github.com/breizhn/DTLN)) | 0.99M / 0.12 GMAC/s | 0.27-0.65 ms per 8 ms block, i5-6600K | not published | PESQ 3.04 DNS 2020 | MIT | MIT | old |
| Facebook Denoiser | - | 41.3 ms (H=48) | - | RTF 0.8, one thread, quad i5 2 GHz ([repo](https://github.com/facebookresearch/denoiser)) | - | - | CC-BY-NC 4.0 | CC-BY-NC 4.0 | archived 2023-10-31 |
| NSNet2 | 48k baseline | 20 ms window (file name) | not published | NSNet v1: 0.16 ms per 20 ms frame, quad i5, ORT 1.1 (2020) | not published | not published | - | CC BY 4.0, per a third-party wrapper only ([PyPI](https://pypi.org/project/nsnet2-denoiser/0.2.3/)) | 2022 baseline |

Where the sources disagree:
- **DFN speed.** DFN2 reported RTF 0.04 on a notebook i5 ([arXiv 2205.05474](https://arxiv.org/abs/2205.05474)).
  DFN3 reported 0.19 on an i5-8250U. A 2026 blog says "22x real time on one pinned core"
  ([creativeainews](https://www.creativeainews.com/articles/deepfilternet-vs-rnnoise-vs-ffmpeg-noise-removal-2026/)).
- **DFN3 lookahead.** The UL-UNAS table lists "DeepFilterNet" with 0 ms lookahead, probably v1 [i]. Use the DFN3
  paper's 40 ms.
- **RNNoise size.** 0.087M parameters in the DPDFNet paper, 0.06M in the GTCRN README.
- **Who built the test sets.** The multilingual low-SNR set (324 clips, 0/5/10 dB, 12 languages) was built by the
  DPDFNet authors.

### The serious candidates

**RNNoise / nnnoiseless.** Hybrid DSP plus a small GRU: it predicts 22 Bark-band gains and a pitch filter at 48 kHz.
nnnoiseless is pure Rust, "mostly safe", and its API is simple: `DenoiseState::new()`, then
`process_frame(&mut out, &in)` on 480 samples ([docs.rs](https://docs.rs/nnnoiseless/latest/nnnoiseless/struct.DenoiseState.html)).
- Input is in i16 range, not [-1, 1].
- The first frame fades in.
- Each frame returns a speech probability.
- Version 0.5.1 deliberately went back to the original 2017 weights, "which work better for voice detection".
- `RnnModel::from_bytes` loads other weights, but RNNoise's format changed after 0.1.1
  ([CHANGELOG](https://raw.githubusercontent.com/jneem/nnnoiseless/main/CHANGELOG.md), [rnnoise](https://github.com/xiph/rnnoise)).
- One Rust voice app (sink) measured it on its own speech: noise between words -31 → -46 dBFS, SI-SDR -0.2 → 6.5 dB,
  speech about 1.4 dB quieter, "handles fan and hum well but not keyboard clicks" ([sink PR #86](https://github.com/NC1107/sink/pull/86)).

**DeepFilterNet3.** A two-stage model: ERB-band gains, then a deep filter on the lowest 96 bins (≤ 4.8 kHz).
- It predicts local SNR. Below -10 dB it returns silence; above 20 dB it skips the deep filter
  ([paper](https://arxiv.org/pdf/2305.08227)). In loud noise that could clip soft syllables [i]. The threshold is
  configurable (`min_db_thresh`).
- Real-world complaints: underruns when starved of CPU, choppy output unless fed exactly 480 samples
  ([#655](https://github.com/Rikorose/DeepFilterNet/issues/655)), crackling in 0.5.6.
- Issues are closed by a stale bot. EasyEffects uses it for its "Deep noise remover"; on a Raspberry Pi, latency rose
  from ~15 to ~55 ms with it on and kept growing under load ([easyeffects #3851](https://github.com/wwmm/easyeffects/issues/3851)).

**DPDFNet (Ceva, Dec 2025).** DeepFilterNet2 with dual-path RNN blocks in the encoder; causal; 8, 16 and 48 kHz
models. The best open quality figures found: PESQ 3.14 against DFN3's 2.76 on the multilingual set, and better
DNSMOS than DFN2/DFN3 on DNS4 blind ([arXiv 2512.16420](https://arxiv.org/html/2512.16420v2)). Caveats:
- The paper evaluates at 16 kHz and says 48 kHz has "negligible loss", with no numbers.
- It costs 4-20x DFN3's MACs.
- Ceva's own `StreamEnhancer` has a 0-1 strength slider ([PyPI](https://pypi.org/project/dpdfnet/)). sherpa-onnx's
  streaming mode ignores the attenuation limit.
- Its noise data (MUSAN, FSD50K) includes some CC-BY-NC clips. Whether that reaches trained weights is legally
  unsettled [i]. Ceva publishes the weights as Apache-2.0.

**GTCRN (and UL-UNAS).** Tiny (0.5 MB, ~0.06 s CPU per s measured by us), MIT, already in sherpa-onnx. But they are
16 kHz only, so a 48 → 16 → 48 kHz path would cut everything above 8 kHz. GTCRN also fell below RNNoise on the
multilingual low-SNR set. Before Nemotron it raised errors in every language we tested. Not for the voice path.

**WebRTC NS / sonora.** Classic, nearly free, BSD-3, with four levels. It is the floor (the likely Discord
"Standard" [i]), not parity. sonora's real value is **AEC3**, which matters for game sound from speakers (see Design).

**Excluded:**
- Facebook Denoiser: CC-BY-NC, archived.
- NSNet2: license only from a third party, no quality numbers.
- DTLN, Fast-ULCNet, FRCRN, LiSenNet, FSPEN: 16 kHz. Fast-ULCNet also has no stated code license, and LiSenNet's
  port is gated.
- PercepNet: no code.
- FastEnhancer-M (48 kHz, int8 C runtime RTF 0.069 on an M2 core): license not stated
  ([arXiv 2607.25350](https://arxiv.org/abs/2607.25350)).
- URGENT systems: too large.

**May a free MIT app ship or download the weights?**
- **RNNoise (BSD-3), DFN3 (MIT/Apache), DPDFNet (Apache-2.0), GTCRN (MIT): yes.** The license text goes in the
  third-party notices; for Apache-2.0, any NOTICE file too [i].
- **Facebook Denoiser: no.** A non-commercial license on a bundled file would contradict MIT's grant [i].
- **Krisp: no.**
- **NSNet2, FastEnhancer, Fast-ULCNet: not until a primary source states a license.**

## Ways to run them in Koetama

| route | models | effort | risks | license | added to the download |
|---|---|---|---|---|---|
| `nnnoiseless` 0.5.2 | RNNoise (2017 weights built in; others via `RnnModel`) | small: one crate, no native code, 480-sample frames | old weights; keyboard clicks pass; speech ~1.4 dB quieter (sink) | BSD-3 | ~85 kB of weights plus code [i] |
| sherpa-onnx online denoiser (already linked: `sherpa-onnx = "=1.13.8"`) | DPDFNet 8/16/48 kHz, GTCRN | small: Rust `OnlineSpeechDenoiser` exists in 1.13.8 (**read by us**: `create`, `run(&self, samples, rate)`, `flush`, `reset`, `sample_rate`, `frame_shift_in_samples`) | C header: `Run` "is not thread-safe", allocates its output per call, may return nothing until enough input arrives; yet the Rust wrapper declares `unsafe impl Sync` - own it from one thread only. Attenuation limit ignored when streaming. No x86 RTF published ([c-api.h](https://raw.githubusercontent.com/k2-fsa/sherpa-onnx/master/sherpa-onnx/c-api/c-api.h), [sherpa docs](https://k2-fsa.github.io/sherpa/onnx/speech-enhancement/dpdfnet.html)) | Apache-2.0 (sherpa-onnx, DPDFNet) | 0 code; model on first use: 10.6 MB (`dpdfnet2_48khz_hr`) or 14.6 MB (`dpdfnet8`, 16 kHz) ([release](https://api.github.com/repos/k2-fsa/sherpa-onnx/releases/tags/speech-enhancement-models)) |
| `sonora` 0.2.0 | WebRTC NS, AEC3, AGC2 | small-medium; pure Rust, Windows x86_64 in CI, MSRV 1.91 (we have 1.99) | an "AI assisted" port, 7 months old; passes the 2400+ upstream C++ tests ([README](https://raw.githubusercontent.com/dignifiedquire/sonora/main/README.md)) | BSD-3 | small [i] |
| libDF (`deep_filter`, git) | DFN3 (and the low-latency model) | medium: crates.io stops at 0.2.5 (2022), so pin a git commit; `DfTract::process` per 480-sample hop; `with_atten_lim`, `with_thresholds` ([tract.rs](https://raw.githubusercontent.com/Rikorose/DeepFilterNet/main/libDF/src/tract.rs)) | unmaintained since 2023-24; brings tract 0.21 (a second inference runtime beside ORT) | MIT / Apache-2.0 | 7.98 MB model (embeddable) + tract [i] |
| `ort` with ONNX exports (Koetama already has `ort` 2.0.0-rc.13, load-dynamic, sharing `onnxruntime.dll`: **read by us**) | DFN3 community exports | large: no official stateful export; you rewrite STFT, ERB features and the deep filter; exports may drop GRU state outside tract ([#655](https://github.com/Rikorose/DeepFilterNet/issues/655), [soniqo](https://huggingface.co/soniqo/DeepFilterNet3-ONNX)) | correctness of a hand-made front end | MIT / Apache-2.0 | ~16 MB (one community export) |
| `webrtc-audio-processing` 2.1.0 (tonarino) | WebRTC APM in C++ | medium-large | builds with meson, ninja and abseil; MSVC support is PR #102, open since 2026-08-08; issue #34 since 2023 ([PR](https://github.com/tonarino/webrtc-audio-processing/pull/102)) | BSD-3 | small |
| `rnnoise2` (C bindings, 2026) | RNNoise 0.2 presumably [i] | small | new, unreviewed | BSD-3 | small |
| Windows Communications capture mode | Voice Clarity | small, Windows only | Windows 11 only; applies its own AEC/AGC; driver-dependent | free | 0 |

The cheapest pair is `nnnoiseless` for Standard and the sherpa-onnx denoiser for Strong. Neither adds a native build
step [i].

## Design

**Where in the chain.** Today `Sender::push` runs gate → AGC → Opus on the voice thread. The microphone callback
only queues blocks (`Voice::push_mic`, ~3 s queue, "never waits"). Read by us in `kd-voice/src/voice.rs`. The
suppressor goes at the top of `Sender::push`, before the gate, for these reasons [i]:
- **Placement order.** Every reference pipeline suppresses before gain and gate:
  - WebRTC APM: high-pass, then AEC3, then NS, then AGC2 ([audio_processing_impl.cc](https://webrtc.googlesource.com/src/+/refs/heads/main/modules/audio_processing/audio_processing_impl.cc)).
  - Mumble: Speex AEC, then RNNoise, then the Speex preprocessor with AGC, then VAD, then Opus
    ([AudioInput.cpp](https://raw.githubusercontent.com/mumble-voip/mumble/master/src/mumble/AudioInput.cpp)).
  - sink and voice-cat: suppression before the gate as well.
- **It must run on every block while the microphone is open**, not only while talking:
  - The models are recurrent and need warm state.
  - The gate's 0.3 s pre-roll comes from audio buffered before the detector fired.
- **The thread already exists.** The voice thread is the worker the callback never waits on. The cost lands there,
  and the queue depth tells when it falls behind.
- **The detector stays on the raw audio.** The gate's "talking" comes from the speech detector on the raw 16 kHz
  branch. A d-ms suppressor delay shifts the audio later than the decision, so a stretch's last d ms could be cut.
  Flush the denoiser, or hold the gate's end d ms longer.

**Speech to text stays on the raw microphone.** It already branches off before the voice path (`kd_audio::Rechunk`
makes the 16 kHz). Keep it there:
- MetricGAN+ enhancement made four ASR systems worse in all 40 configurations. Whisper's semantic WER at 10 dB SNR
  went 8.82 % → 25.83 % ([arXiv 2512.17562](https://arxiv.org/abs/2512.17562v1)).
- SAM-Audio enhancement raised Whisper's errors, more for larger models ([arXiv 2603.04710](https://arxiv.org/abs/2603.04710)).
- **Measured by us** (2026-10-04, Nemotron 3.5, clean / room error %):
  - GTCRN: worse in every language (de room 17.4 → 32.6).
  - DPDFNet-2 (16 kHz): helped en room (3.2 → 1.9), es room (10.3 → 5.1) and de room (17.4 → 15.2); hurt zh (clean
    9.8 → 13.7, room 14.7 → 18.6) and clean ru (10.9 → 14.5).

**Latency budget.** The voice path already holds audio for a 60 ms packet (3 × 20 ms Opus frames), and the receiver's
jitter buffer starts at 60 ms (`jitter::START`). Added delay per suppressor:
- RNNoise: ~10 ms, a sixth of one packet.
- DPDFNet / DFN3: 20-40 ms, up to two thirds of a packet. Acceptable for an opt-in mode. Above DNS's 20 ms rule.

**Echo is not noise.** A noise suppressor is trained to keep speech. Game voices, music and speech-like effects
leaking from speakers will mostly pass; fans and hum will not [i]. No source measured any model on game leakage. The
fix for leakage is echo cancellation with the game's output as reference (WASAPI loopback / PulseAudio monitor),
e.g. sonora-aec3 or the `aec3` crate ([crates.io](https://crates.io/api/v1/crates/aec3)), or headphones. Treat this
as a separate, later feature.

**Gain and suppression interact.** Koetama's AGC (`agc.rs`) follows speech power only, with up to +30 dB of gain. So
it does not chase noise, but whatever noise survives is raised by the same gain. A quiet microphone in a noisy room
is therefore where suppression matters most [i]. Mumble's preprocessor lowers its Speex suppression strength as the
current AGC gain rises (AudioInput.cpp, above). Copy the idea only if testing shows over-suppression on boosted
microphones [i].

**The setting [i].** "Noise suppression: Off / Standard / Strong".
- **Standard** = RNNoise, the default: cheap enough to leave on.
- **Strong** = DPDFNet-48k (or DFN3). The model is downloaded on first choice, like sink's opt-in DFN
  ([sink PR #86](https://github.com/NC1107/sink/pull/86)).
- **Off**: for players already running Krisp or NVIDIA Broadcast.
- **Fallback**: if the voice queue grows while Strong is on, drop to Standard and log it. sink falls back to RNNoise
  when its DFN plugin aborts.
- **Test microphone / Hear yourself** must hear the processed signal, so players can judge it.

## How we would test it

- **`kd_audio::simmic` noise types.** Today it has white noise only (`noise_db`, one "noisy" microphone at -45 dBFS).
  Add [i]:
  - fan/hum (stationary, low-frequency harmonics);
  - keyboard clicks (impulsive);
  - babble or game music (speech-like);
  - speakers' game sound with a delay (echo);

  each at 0 / 5 / 10 / 15 dB SNR, and once on the quiet microphone so the AGC's +30 dB acts on the residue.
- **`kd-voice/tests/voice_zoo.rs` measures** (sink's, which are cheap to compute):
  - speech level against the reference microphone (within the existing 3 dB);
  - noise between words in dBFS (Off -31, RNNoise -46, DFN about -63 on sink's clip);
  - SI-SDR.

  RNNoise tests can run in the pull-request workflow (no models). DPDFNet tests need the model, so they run locally.
- **`tools/voice_e2e.py`.** Run A on `--sim-mic` / `--mic-wav` with each noise and each setting, and B with
  `--out-wav`. Report B's speech level, B's noise floor between words, and the speech to text of what B heard. Recall
  of the spoken line is the intelligibility check that matters: a suppressor that cuts syllables loses words there.
  Also measure each model's real delay: cross-correlate A's input with B's output.
- **Paced CPU benchmark on the user's PC.** Run 10 ms frames at real-time pace, not as fast as possible. FastEnhancer's
  authors found pacing to a real-time frame deadline cost 4.2x per frame against racing through a file. On their M2,
  ORT fp32 was 3.3x slower than an int8 C runtime ([arXiv 2607.25350](https://arxiv.org/abs/2607.25350)).
  - Measure p50 / p95 / p99 per-frame time and the voice queue's depth.
  - Run with Koetama below a running game's priority. That needs the user's go-ahead and a time they are not playing
    (heavy jobs and launching games wait for them).
  - Compare nnnoiseless, `dpdfnet2_48khz_hr`, and DFN3 if it gets that far.
  - A Strong mode whose p99 approaches 10 ms, or that costs more than a few percent of a core while gaming, should
    not ship as is [i].

## Gaps

- **Discord:** no engineering source. What "Standard" is, Krisp's model size, delay and desktop CPU cost: all not
  published. Krisp's SDK price and terms for free apps: not published.
- **Other vendors:** no latency figure for NVIDIA Broadcast, AMD, Windows Voice Clarity, Zoom, Meet or Apple. No
  current Maxine license terms. EOS's algorithm and ODIN's engine are unconfirmed.
- **DPDFNet:** no CPU figure for any variant on x86. No quality figure at 48 kHz. Its true algorithmic delay
  (20 vs ~40 ms) is unclear.
- **RNNoise 0.2:** no quality, size or CPU figure. Whether nnnoiseless can load its weights is unverified.
- **DFN3:** no Windows CPU measurement. Several of its ONNX-state claims are unverified community comments.
- **Every model:** no source on music, laughter, shouting or game-sound leakage. Memory (RAM) is published only for
  DFN as a LADSPA plugin (~150 MB, sink).
- **sherpa-onnx:** the online denoiser's frame shift per model is not documented (query `frame_shift_in_samples`).
  Its x86 speed is unpublished.
- **Opus:** which bandwidth libopus chooses at 24 kbit/s VOIP mono was not checked. It bounds how much a full-band
  suppressor matters.
- **Licenses:** NSNet2's weights license comes only from a third-party wrapper. Whether CC-BY-NC clips in training
  data (FSD50K, MUSAN) affect weights is legally unsettled.
- **Our own benchmark:** the 2026-10-04 figures are offline, on computer voices with synthetic room noise, at 16 kHz.
  They are not the paced 48 kHz cost.
