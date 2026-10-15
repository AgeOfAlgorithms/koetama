# Voice extraction for Koetama: keeping only the player's voice

*2026-10-15. Researched from the web on 2026-10-15 (papers, vendor help pages and release notes, GitHub and Hugging
Face pages and APIs, challenge reports), plus Koetama's own code and the 2026-10-15 noise suppression measurements
(`docs/noise-suppression.md`). Every figure is the source's own, on the hardware it names, unless marked **read by
us** or **measured by us**. "[i]" = an inference no source states. "Not published" = searched for and not found.
Nothing here was measured for voice extraction yet.*

## Summary

**Yes, it exists.** Research calls it target speaker extraction (TSE) or personalized speech enhancement (PSE).
Products call it "voice isolation" or "background voice cancellation". A noise suppressor keeps any speech: it is
trained to remove what is not speech. Extraction keeps one particular voice. There are two kinds:
- **Enrolled.** A speaker embedding (a 192-512-number "voiceprint" from a few seconds of the player's voice) tells
  the model whose voice to keep. Teams, Zoom and Fraunhofer work this way, as do all the open models below.
- **Not enrolled.** The model keeps the "primary talker": the voice that is loud, close and dry. Krisp Background
  Voice Cancellation needs a headset, and Webex needs the talker within 1 m of the microphone.

**Shippable today, with limits:**
- Commercial products do it inside their own apps (Teams, Zoom, Webex, Krisp). None can be embedded in a free MIT
  app. Krisp's SDK is sales-gated.
- Among open models, **no released model is causal, 48 kHz, light on CPU and permissively licensed (code and
  weights) all at once.**
  - The closest is **VoxLock**: Apache-2.0 repository with ONNX weights, 16 kHz, about 24 ms algorithmic latency.
    It was published 4 days ago, and by its author's figures it runs barely inside real time on 1-4 CPU threads.
    That is roughly 10x DeepFilterNet 3's 7.8 % of a core [i].
  - The architecture that fits Koetama best is a personalized DeepFilterNet (pDeepFilterNet2). It costs the same as
    DeepFilterNet2 and removes interfering speech much better, but it exists only as a paper.
- The cheap parts already ship in Koetama: speaker embeddings (sherpa-onnx 1.13.8, **read by us**), voice
  detection, and DeepFilterNet 3.

**Recommendation, in order:**

| # | what | removes | effort | main risk |
|---|---|---|---|---|
| 0 | **Measure first** (`bench/extract.py`, "How we would test it") | - | small | - |
| 1 | **Echo cancellation** (sonora AEC3, system loopback as reference), only when the player uses speakers | game sound, other players, music from the PC's own speakers | small-medium | clock drift and Bluetooth delay; useless with headphones |
| 2 | **Speaker-verification gate** after DeepFilterNet 3, opt-in "Only my voice" | a TV or roommate talking while the player is silent | medium | muting the player; cannot split overlapping speech; ~0.5-1.5 s to react [i] |
| 3 | **VoxLock via `ort`**, as an experiment, shipped only if a paced single-thread run fits beside a game and the weights' licence is clarified | other voices, even overlapping ones | medium | CPU; 16 kHz; a 4-day-old model; muting the player |
| 4 | **Train a personalized DeepFilterNet** (pDeepFilterNet2's method) | other voices at DeepFilterNet's cost, at 48 kHz [i] | large (GPU training, own data pipeline) | quality and over-suppression are ours to solve |

**What must be measured first:**
- What players actually hear leaking: game sound from speakers (an echo problem), or other people in the room (an
  extraction problem).
- VoxLock's paced CPU on the Ryzen 7 3700X, one thread, 40 ms chunks.
- How often each option mutes the player's own voice, including shouting and whispering.
- Which bandwidth Opus picks at 24 kbit/s. If it is wideband (8 kHz), a 16 kHz extractor loses nothing that gets sent
  [i]. This was not checked in the noise study either.

## What players compare us to

Most of what gamers use removes noise only: Discord's Krisp, NVIDIA Broadcast, Google Meet, and the game SDKs. Only
the meeting apps and Krisp's own app claim to remove other voices. None of them publishes a latency or CPU figure for
voice removal.

| product | removes other voices? | enrollment | latency / cost published | available to Koetama |
|---|---|---|---|---|
| Krisp Background Voice Cancellation (desktop app, RTC SDK) | yes: "all other nearby human voices, resulting in only the primary speaker's voice" ([release notes 2023-03-13](https://krisp.launchnotes.io/announcements/ann_FOdwanTsBSQRv)). In 2022 it was "Voice Cancellation", where voices "from another pitch category" are eliminated ([Wayback 2022](https://webcf.waybackmachine.org/web/20220516044421/https://krisp.ai/product)) | none; "you only need to wear a headset" | 15 ms algorithmic, from a third party only ([Vapi](https://vapi.ai/partnerships/directory/partner/krisp-technologies-inc)) | an Enterprise-track feature in 2022-23; current consumer availability not found. The SDK has a "Request SDK Access" form, no public price, no free tier and no open-source program ([pricing](https://krisp.ai/sdk-pricing/)); one directory says "from $5,000 per year" ([fitgap](https://us.fitgap.com/products/krisp), unverified) |
| Krisp VIVA Voice Isolation (SDK for voice agents) | yes: "filtering out background noise and other voices" ([Pipecat](https://docs.pipecat.ai/api-reference/server/utilities/audio/krisp-viva-filter.md)) | "No voice enrollment needed" ([developers](https://krisp.ai/developers/)) | p95 per 20 ms frame on cloud CPUs, Aug 2026: Lite 0.7-1.0 ms, Balanced 2-3 ms ([Krisp blog](https://krisp.ai/blog/voice-isolation-cpu-optimization/)); 8-48 kHz | needs an SDK licence and an API key |
| Microsoft Teams voice isolation | yes: removes "sounds or voices ... that don't match the profile" ([Learn](https://learn.microsoft.com/en-us/microsoftteams/voice-isolation)) | voice profile of about 30 s, read aloud or built passively in a meeting, stored on the device, deletable ([Support](https://support.microsoft.com/office/voice-isolation-in-microsoft-teams-calls-and-meetings-a9756ea9-4cec-44c4-aefb-6f5d17c89427)) | not published; error "ufd2" = device running too many apps | Teams desktop only (rolled out April-June 2024, [MC690921](https://mc.merill.net/message/MC690921)) |
| Zoom Personalized Audio Isolation | yes: suppresses "conversations from other people within microphone range" ([Zoom bluepaper, snippet](https://library.zoom.com/zoom-workplace/artificial-intelligence/artificial-intelligence-bluepaper/algorithms/algorithm-features)) | voiceprint, built automatically or from a recording, stored locally ([Krisp blog, snippet](https://krisp.ai/blog/zoom-voice-isolation/); [UNH KB](https://td.usnh.edu/TDClient/60/Portal/KB/ArticleDet?ID=4921)) | not published | Zoom only |
| Webex "Optimize for my voice" | yes: removes "all noise and background voices" ([Webex Help](https://help.webex.com/en-us/article/yrsw3l)) | none; a primary-talker detector. Cisco's poster: talker within 1 m, words chopped when the talker is mistaken for an interferer ([sigport](https://sigport.org/sites/all/modules/pubdlcnt/pubdlcnt.php?fid=8965)) | not published | Webex only |
| Agora Conversational AI "selective attention" | claims to "track the primary speaker" and block "background voices" ([Agora](https://prod.agora.io/en/products/conversational-ai-engine)) | none mentioned | not published | paid platform, for voice agents |
| Fraunhofer upHear Voice Isolation | "only a pre-selected voice" passes ([product sheet](https://www.iis.fraunhofer.de/content/dam/iis/en/doc/ame/product-sheets/uphear/upHear_Microphone_processsing_product_sheet.pdf)) | a voice "fingerprint" made in seconds | not published | licensing not found |
| Sanas NC Voice Isolation | "background speech" removal, per a search snippet ([docs](https://developer.sanas.ai/Docs/Models/Human-Human/NC-Voice-Isolation), 404 on fetch) | not found | not found | not found |
| Discord (Krisp inside) | **no**: removes "background noises that aren't human voices" ([FAQ, snippet](https://support.discord.com/hc/en-us/articles/360040843952)); no BVC in Discord found | - | not published | - |
| Google Meet | **no**: "Voices from TV or people talking won't be canceled" ([Meet help](https://support.google.com/a/users/answer/11053176)) | - | - | - |
| NVIDIA Broadcast | no claim: noise removal only; needs an RTX GPU ([NVIDIA](https://www.nvidia.com/en-us/geforce/broadcasting/broadcast-app/)) | - | - | players can put it in front of Koetama |
| Windows Studio Effects "Voice focus" / Voice Clarity | no claim about other voices: "Isolates your voice and minimizes background noise", needs an NPU ([Support](https://support.microsoft.com/en-us/windows/ai/ai-features/windows-studio-effects)); Voice Clarity = noise suppression, echo cancellation and dereverberation ([Windows Latest](https://windowslatest.com/2024/02/04/microsoft-tests-windows-11s-new-voice-ai-feature-for-all-pcs)) | none | not published | Windows 11 only |
| Apple Voice Isolation | "blocks ambient noise"; mechanism not published ([Apple](https://support.apple.com/101993)) | none | - | Apple devices, user-selected |
| Elgato Voice Focus, SteelSeries ClearCast, Logitech Blue VO!CE | no: noise, reverb and echo ([Elgato](https://help.elgato.com/hc/en-us/articles/31577824956433-Wave-Link-Voice-Focus), [SteelSeries](https://steelseries.com/en-th/blog/how-to-eliminate-noise-from-microphone-communication-869), [Logitech](https://www.logitechg.com/discover/technology/blue-voice)) | - | - | - |
| Game SDKs: ODIN, Vivox, EOS | no: VAD, AGC, echo cancellation and noise suppression only ([4Players](https://docs.4players.io/voice/introduction/comparison/)) | - | - | - |

Where sources disagree or say nothing:
- Krisp's developer page is ambiguous about which product the "removes nearby competing voices, no enrollment" text
  belongs to.
- Zoom's "High" noise suppression level is said to remove colleagues' voices only by secondary guides
  ([Zapier](https://zapier.com/blog/zoom-noise-cancellation.md)).
- No vendor publishes how much of a nearby voice is removed, or how often the user's own voice is cut.
- Teams' model is "personalized deep voice quality enhancement". [i]: probably a personalized DeepVQE, Microsoft's
  joint echo/noise/reverb network "tested for the Microsoft Teams platform" ([arXiv 2306.03177](https://arxiv.org/abs/2306.03177)),
  built with PSE that reuses the model's own embedding ([arXiv 2406.09928](https://arxiv.org/abs/2406.09928)).

[i] For a headset gamer, Krisp's and Webex's enrollment-free "near, loud, dry voice" cue matches the usual setup. A
sibling at the same distance can only be rejected by identity (enrollment). No product claims to remove game sound
from loudspeakers, except by treating it as background speech. A player can already put Krisp's desktop app in front
of Koetama as a virtual microphone, with no licence needed by Koetama.

## Open models

Causal PSE/TSE models with 20-60 ms latency are well established in papers. Almost none release weights. The two
2026 exceptions are the REAL-TSE challenge's causal baselines (licence unstated) and VoxLock, which is fine-tuned from
one of them. Toolkits such as WeSep and ClearerVoice-Studio are offline and mostly 8-16 kHz.

| model | causal / streaming | latency | params / compute | measured CPU (source, hardware) | rate | enrollment | weights released? | code license | weights license | status |
|---|---|---|---|---|---|---|---|---|---|---|
| **VoxLock** ([HF](https://huggingface.co/arbazsiddiqui/voxlock), [GitHub](https://github.com/arbazsiddiqui/voxlock)) | yes: causal band-split RNN, state carried in `chunk.onnx` | ~24 ms algorithmic; chunks ≥ 40 ms | 27M | RTF (author's card; hardware attribution partly inconsistent): M2 Pro, 40 ms chunks, int8, 0.89 on 2 threads / 0.69 on 4; fp32 1.33 / 0.97. 8-vCPU Xeon, 4 threads, 0.89. Xeon, 80 ms, 2 threads, 0.67. 1 vCPU, 160 ms, 1 thread, 0.73. Half a vCPU is "too little" | 16k | 3 s (default: the loudest 3 s in the first 15 s); `enroll.onnx` runs once; WeSpeaker ECAPA-TDNN512 | yes: ONNX int8 75 MB, int8-full 67 MB, fp16 48 MB, fp32 95 MB | Apache-2.0 (repo) | no separate statement; fine-tuned from a REAL-TSE checkpoint of unstated licence; ECAPA trained on VoxCeleb (CC BY 4.0) | repo created 2026-10-11, 21 stars ([API](https://api.github.com/repos/arbazsiddiqui/voxlock)) |
| REAL-TSE baselines `spk_emb_causal_100`, `tfmap_context_causal_100` ([repo](https://github.com/REAL-TSE/wesep-real-tse)) | yes: causal BSRNN | online track ≤ 100 ms ([arXiv 2607.15198](https://arxiv.org/html/2607.15198)) | not published | not published | 16k | one utterance (dev ≥ 5 s); ECAPA-TDNN or TF-map | yes, Google Drive | Apache-2.0 (WeSep) | not stated | SLT 2026 challenge |
| pDeepFilterNet2 ([arXiv 2404.08022](https://arxiv.org/html/2404.08022v1)) | yes | 40 ms (20 ms window, 2-frame lookahead) | 2.31M / 0.33 GMAC | RTF 0.025, i7-10870H (paper): same as DeepFilterNet2 | not stated (DFN2 is 48k [i]) | frozen ECAPA, 192-dim; length not stated | no (audio demo only) | none | none | ICASSP 2024 |
| Personalized PercepNet ([paper](https://jmvalin.ca/papers/percepnet_pers.pdf)) | yes | 10 ms frames + 30 ms lookahead | PPN-512 8.5M; PPN-1024 26.5M | 4.7 % / 17.2 % of one 1.8 GHz i7-8565U core | **48k** | own embedder, 6 s inputs; 20 s examples in evaluation | no | none | none | Interspeech 2021 |
| TEA-PSE 3.0 ([arXiv 2303.07704](https://arxiv.org/html/2303.07704v1)) | not stated | 20 ms window / 10 ms hop | 22.24M / 19.66 GMAC/s | RTF 0.46 (ONNX), Xeon E5-2678 v3 | "full-band" | ECAPA + local-global representation | no | none | none | 1st in DNS 2023 |
| SpeakerBeam-SS ([ISCA](https://www.isca-archive.org/interspeech_2024/sato24_interspeech.pdf)) | yes | 20 ms (variants 60 / 140 ms) | 7.93M | RTF 0.36, one AMD EPYC 7502P core, C++ | 16k | learned encoder | no | none | none | Interspeech 2024 |
| E3Net ([arXiv 2204.00771](https://arxiv.org/html/2204.00771v1)) | yes | 20 ms window / 10 ms hop | 4.50-10.85M | RTF 0.036 (student) to 0.122 (teacher), one Xeon W-2133 thread | not stated | d-vector | no | none | none | Interspeech 2022 |
| pDCCRN ([arXiv 2110.09625](https://ar5iv.labs.arxiv.org/html/2110.09625)) | yes | 32 ms (per SpeakerBeam-SS) | 1.50-3.96M | RTF 0.094-0.195, one Xeon W-2133 thread (E3Net paper) | not stated | Res2Net d-vector, ~60 s | no | none | none | ICASSP 2022 |
| VoiceFilter-Lite ([arXiv 2009.04323](https://arxiv.org/pdf/2009.04323)) | yes | frame by frame | 2.2 MB int8 | "real time on device" | ASR features, not audio for listening | d-vector, stored on the device | no | none | none | Interspeech 2020 |
| FireRedChat-pVAD ([HF](https://huggingface.co/FireRedTeam/FireRedChat-pvad)) | personal VAD (gate, not separation) | not published | not published | not published | not published | SpeechBrain ECAPA, updated after the first utterance | yes, ONNX | Apache-2.0 | Apache-2.0 | 2025-09; a LiveKit plugin |
| Personal VAD 1/2 (Google, [arXiv 1908.04284](https://arxiv.org/abs/1908.04284), [2204.03793](https://arxiv.org/abs/2204.03793)) | yes | frame level | ~130k (v1) | not published | - | d-vector | no | none | none | - |
| anchor-fast-weight ([GitHub](https://github.com/ym2976/anchor-fast-weight)) | chunked | 250 ms chunks, up to 4 s context | 41k-parameter adapter on Conv-TasNet | < 5 % overhead ([arXiv 2609.20463](https://arxiv.org/abs/2609.20463)) | 16k | adapts while streaming | no | MIT | - | Sep 2026 |
| WeSep ([GitHub](https://github.com/wenet-e2e/wesep)), ClearerVoice-Studio ([GitHub](https://github.com/modelscope/ClearerVoice-Studio)) | no causal model documented | - | - | - | 8-16k | reference speech | WeSep "coming soon"; ClearerVoice: only an audio-visual TSE found | Apache-2.0 | not stated (one HF repo tagged apache-2.0) | research toolkits |

Quality, where measured:
- Personalization helps only when another talker is present.
  - pDeepFilterNet2 against DeepFilterNet2 (VCTK + DNS5 noise): PESQ 2.10 → 2.36, STOI 0.748 → 0.784. Unchanged on
    noise-only, "much better" with interfering speech.
  - Personalized PercepNet against PercepNet (target + interferer + noise): PESQ 1.75 → 2.41, MOS 2.62 → 3.21.
- On 2 talkers + noise (SpeakerBeam-SS paper): causal SpeakerBeam-SS 11.58 dB SDR, pDCCRN 10.70, E3Net 9.83, from a
  -0.45 dB mixture.
- On real conversations it is hard. REAL-TSE's causal baselines reach only 0.37-0.39 speaker similarity on EVAL-2.
- VoxLock's author reports 4.95 % WER with 0.11 background words leaked per minute on Krisp's Voice Isolation
  Benchmark, against Krisp VI 2.5's 5.30 % and 23.21 % unprocessed. This is not independently verified.

**Notes on the serious candidates:**
- **VoxLock** is the only drop-in: two ONNX files and a stateful chunk call, so `ort` could run it with no new
  runtime [i].
  - Against: 16 kHz; 27M parameters and near real time on several threads; trained only on LibriSpeech and AMI; no
    limitations section; one target only.
  - Its weights inherit an unstated REAL-TSE licence.
- **pDeepFilterNet2** is the right shape. It is DeepFilterNet2 with an ECAPA embedding concatenated into a unified
  encoder: personalization adds almost no compute, and it would slot where DeepFilterNet 3 is now. It has no code
  and no weights.
- **REAL-TSE baselines** could be benchmarked but not shipped until a licence is stated.
- **SpeakerBeam-SS** is the best causal quality with published CPU, at ~36 % of an EPYC core [i]. No weights.
- **TEA-PSE 3.0** has the best DNS scores, but it is 22M parameters at RTF 0.46. No weights.
- **Personalized PercepNet** is the only confirmed 48 kHz, low-cost PSE (4.7 % of a laptop core). It was never
  released.
- **E3Net and pDCCRN** are Microsoft's lineage, with the over-suppression metric (TSOS). Paper only.
- **VoiceFilter-Lite** feeds speech recognition, not a listener. Paper only.

**Excluded:**
- The BUT/NTT SpeakerBeam repository: an evaluation-only licence forbids distributing it
  ([LICENSE](https://raw.githubusercontent.com/BUTSpeechFIT/speakerbeam/main/LICENSE.txt)), and it is 8 kHz without
  weights.
- SpEx+ and X-TaSNet: offline, no official weights.
- MeanFlowTSE: 346-1376 MB, ~0.7 s per call on a GPU ([HF](https://huggingface.co/nmj21c/MeanFlowTSE)).
- StarTSE: an autoregressive language model, 560 ms ([arXiv 2604.19635](https://arxiv.org/pdf/2604.19635)).
- RAVEN and AV-MossFormer2: need a camera.
- Samsung's cross-attention PSE and TargetVoice: no weights.
- sherpa-onnx: has no TSE. Its "source separation" splits music stems
  ([c-api.h](https://raw.githubusercontent.com/k2-fsa/sherpa-onnx/master/sherpa-onnx/c-api/c-api.h)).
- No Rust or C port of any TSE/PSE model was found.

## Three routes for Koetama, plus echo cancellation

| route | removes | added delay | CPU (one core) | effort | licences |
|---|---|---|---|---|---|
| (a) VoxLock via `ort` | other voices, also while overlapping; noise [i] | ~24 ms + ≥ 40 ms chunk + resampling [i] | RTF 0.73 on one Xeon vCPU (160 ms chunks) ≈ 73 % of a core [i]; 3700X not measured | medium | Apache-2.0 code; weights unclear |
| (b) verification gate after DFN3 | a voice talking while the player is silent | none in the audio; the decision lags ~0.5-1.5 s [i] | CAM++: RTF 0.013 per thread (CPU unnamed, [arXiv 2303.00332](https://arxiv.org/html/2303.00332v3)) → ~3-4 % for a 1.5 s window every 0.5 s [i] | medium | sherpa-onnx Apache-2.0; CAM++ Apache-2.0 (3D-Speaker, per checkpoint unconfirmed) or CC BY 4.0 (WeSpeaker) |
| (c) personalized DeepFilterNet | other voices at 48 kHz [i] | 30-40 ms, as DFN3 | ≈ DFN3 (7.8 %, measured) per the pDF2 paper's equal RTF [i] | large | ours (MIT); permissive training data exists |
| (d) echo cancellation | everything the PC plays, from its speakers | ~10 ms framing [i] | sonora's full AEC3 + NS + AGC2: 13.3 µs per 10 ms on an M4 Max ≈ 0.1 % [i] | small-medium | BSD-3 (sonora) or MIT/BSD-3 (aec3) |

**(a) A ready streaming extraction model: VoxLock.**
- **Chain:** 48 → 16 kHz, then `chunk.onnx` per ≥ 40 ms, then back to 48 kHz, in place of or before DeepFilterNet 3.
  Everything above 8 kHz is lost; whether Opus at 24 kbit/s sends it anyway is unchecked.
- **Delay:** at 40 ms chunks, two Opus frames wait for one chunk. That adds ~64 ms or more on top of a 60 ms packet,
  more than any noise suppressor [i].
- **CPU:** on the author's figures it is too heavy beside a game. The 1-thread configuration buys its RTF of 0.73
  with 160 ms of extra delay.
- **Variant [i]:** run VoxLock on the 16 kHz branch the speech detector already uses. Apply its output-to-input
  energy ratio as a slow gain on the 48 kHz path. This keeps the full band, but it becomes a gate (route b) during
  overlap.
- **Risks:**
  - A model 4 days old, not reviewed by anyone else.
  - Over-suppression of the player is unmeasured.
  - The licence chain of the weights.
- **Verdict:** benchmark it, don't plan on shipping it.

**(b) A speaker-verification gate on top of DeepFilterNet 3.**
- **The parts are already linked.** sherpa-onnx 1.13.8's Rust crate has `SpeakerEmbeddingExtractor`
  (`create_stream`, `is_ready`, `compute`) and `SpeakerEmbeddingManager` (`add_list`, `verify`, and
  `get_best_matches`, which returns a score) (**read by us**). Use the score for hysteresis; `verify` gives only a
  yes or no.
- **The models are 16 kHz ONNX, 23-221 MB** ([release](https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-recongition-models)).
  CAM++ is ~29 MB, 7.2M parameters, 0.65-0.73 % EER on VoxCeleb1-O.
- **Design [i]:**
  - The existing detector finds speech on the raw 16 kHz branch.
  - During speech, embed a trailing 1-1.5 s window every 0.25-0.5 s and score it against the enrolled set.
  - Attenuate by about 20 dB, not a hard mute, only after several low scores in a row. Release fast when the score
    rises. Hold, and show a "voice not recognised" light.
  - It runs after DeepFilterNet 3, before AGC.
- **What it cannot do:** split two voices speaking at once. Short windows are also unreliable: ECAPA's EER is 0.95 %
  on full utterances, 1.79 % at 2 s and 3.94 % at 1 s ([arXiv 2406.07103](https://arxiv.org/pdf/2406.07103)).
- **Bonus [i]:** the same scores could stop a roommate's or the TV's words from being transcribed by speech to text,
  which runs on the same 16 kHz branch.
- **Alternative:** FireRedChat-pVAD, Apache-2.0 ONNX, a frame-level personal VAD. Its card gives no sample rate,
  latency or accuracy, and it ships as a LiveKit plugin.
- **Risks:**
  - Falsely muting the player.
  - Re-enrollment as the player's voice changes.
  - The ~1 s reaction: the first words of a TV line pass [i].

**(c) Train a personalized DeepFilterNet.** Copy pDeepFilterNet2's method onto DeepFilterNet 3: a frozen
192-dimension ECAPA embedding concatenated into the encoder.
- **Training:** on DNS5's personalized track recipe. Its sources are permissive: LibriVox public domain, VCTK ODC-By,
  VoxCeleb2 CC BY 4.0, Freesound CC0 only, DEMAND CC BY-SA 3.0, OpenSLR RIRs Apache-2.0
  ([DNS-Challenge](https://github.com/microsoft/DNS-Challenge)).
- **Shipping:** export and run it the way DeepFilterNet 3 runs now.
- **Effort:** large. A GPU training setup, with mixtures carrying interferers at -5 to 10 dB SIR. A loss that
  penalizes over-suppression, as Microsoft's asymmetric loss did, cut its over-suppression from 14.51 % to 1.20 % of
  frames ([arXiv 2110.09625](https://ar5iv.labs.arxiv.org/html/2110.09625)). And evaluation.
- **Prize:** the only route to overlap removal at 48 kHz for about DeepFilterNet 3's cost. Worth it only if (a) and
  (b) show players want it.

**(d) Echo cancellation for game sound from the player's speakers.**
- **Often the real problem for gamers [i].** NPC dialogue, other players' voices that Koetama itself plays, and music
  are all speech-like, so a noise suppressor or a gate passes them. AEC removes them, because the PC knows exactly
  what it played. Microsoft notes that PSE in full-duplex calls has to be combined with AEC
  ([arXiv 2211.02773](https://arxiv.org/pdf/2211.02773)).
- **Reference signal:**
  - Windows: cpal (already in Koetama, 0.18) opens WASAPI loopback when you build an input stream on an output
    device ([cpal source](https://raw.githubusercontent.com/RustAudio/cpal/master/src/host/wasapi/device.rs)). "WASAPI
    provides loopback mode primarily to support acoustic echo cancellation"
    ([Learn](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording)).
  - Game-only process loopback needs build 20348; the user's 19045 lacks it, and the whole system mix is the better
    reference anyway [i].
  - Linux: the PipeWire sink monitor ([echo-cancel module](https://docs.pipewire.org/page_module_echo_cancel.html)).
    Whether cpal lists monitors is unverified.
- **Library:** sonora 0.2.0, a pure-Rust WebRTC M145 port that passes the 2400+ upstream tests. The neural residual
  echo estimator is not ported ([README](https://github.com/dignifiedquire/sonora)). Alternatives: `aec3` (v0.4.0,
  self-described work in progress) and `aec-rs` (speexdsp).
- **Order:** AEC → DeepFilterNet 3 → gate → AGC → Opus, as WebRTC does.
- **Risks [i]:**
  - Clock drift between microphone and output.
  - Bluetooth output delay.
  - Double-talk ducking the player while the game is loud.
  - No x86 timings or echo-return-loss figures on game audio.
- **Headphones make it moot:** the AEC converges to pass-through. Offer Auto/On/Off.

## Enrollment, failure modes and privacy

**How much voice.**
- Published enrollments are 3 s (VoxLock, TargetVoice ([ISCA](https://www.isca-archive.org/interspeech_2025/pallala25_interspeech.pdf))),
  3-43 s, mean 9.1 s (REAL-TSE), ~30 s (Teams, DNS5) and ~60 s (pDCCRN).
- Longer is better. In a d-vector study, 3 s of enrollment gave 13.54 % EER on short tests, against 7.68 % with 30 s
  ([arXiv 1705.03670](https://arxiv.org/pdf/1705.03670)).
- Phonetic variety mattered more than length, in a small 40-speaker study
  ([sigport](https://sigport.org/sites/all/modules/pubdlcnt/pubdlcnt.php?fid=3593)).
- No source gives a minimum-length curve for the causal PSE models.

**UX [i]:**
- An opt-in "Only my voice" setting.
- A guided 10-20 s reading of a varied text, plus an optional "call-out" clip spoken loudly.
- A quality check: enough speech, no second voice.
- Score against the best of several stored embeddings, not only their mean.
- "Re-record" and "Add a sample" buttons.
- A prompt to re-enroll when the player's own scores drift down (a cold, a new microphone).
- Any slow self-adaptation must be capped, so a roommate cannot drift in. Streaming TSE with static enrollment does
  degrade under enrollment/mixture mismatch ([arXiv 2609.20463](https://arxiv.org/abs/2609.20463)).

**Failure modes, worst first:**
1. **Muting the player.** Microsoft named it target-speaker over-suppression: "especially the causal" models remove
   the target. pDCCRN did on 14.51 % of frames in one test set; E3Net's baseline cut 3.75 s per 30 minutes and its
   distilled student 0.50 s ([arXiv 2204.00771](https://arxiv.org/html/2204.00771v1)). Webex chops words when the
   talker is mistaken for an interferer or sits over 1 m away. In voice chat this is worse than leaking a TV [i]:
   default to pass, attenuate rather than mute, and add a bypass hotkey.
2. **Shouting and whispering.** Vocal effort other than normal "severely degrades" speaker verification.
   Compensation recovers up to 19 % (shouted) and 52 % (whispered) relative EER
   ([Prieto et al. 2022](https://zaguan.unizar.es/record/148274); [arXiv 2008.02487](https://arxiv.org/abs/2008.02487)).
   Gamers shout: hence the loud enrollment clip [i].
3. **Similar voices.** SpeakerBeam "sometimes fails ... in same-gender mixtures"
   ([arXiv 2001.08378](https://arxiv.org/pdf/2001.08378)). Siblings and co-streamers are the hard case.
4. **Distance.** E3Net degrades when the target is not closer to the microphone than the interferer. Teams offers to
   turn isolation off when it detects someone sharing the device.
5. **Children and teens.** Voices change over months. No source was found [i].

**Privacy.**
- A stored voiceprint used to recognise someone is likely biometric "special category" data in the EU. The EDPB's
  draft voice-assistant guidance asks for freely given consent, equivalent function without a voiceprint, and local
  storage ([Pearl Cohen summary](https://www.pearlcohen.com/edpb-issues-draft-guidance-on-virtual-voice-assistants/);
  whether the draft was finalized was not confirmed). Vendors treat embeddings as possibly biometric under BIPA-style
  laws ([AssemblyAI](https://www.assemblyai.com/legal/biometric-notice-and-consent-faqs)).
- So [i]:
  - Opt-in, off by default.
  - Keep only the embedding vectors (192-512 floats, a few kB), never the recording, in the local config folder.
  - Store the embedding model's name and version with them; a new model means re-enrolling.
  - Never send them over the game link.
  - A visible "Delete my voice profile" button.
  - Teams and Zoom also keep the profile on the device and let users delete it.

## How we would test it

The plan is a `bench/extract.py` built like `bench/denoise.py`: a Rust runner that processes the audio as the app
would, at a paced real-time rate, on the user's PC.

- **Target ("the player").** Use a speaker from LibriSpeech test-clean or EdAcc (`bench/datasets.py`), enrolled from
  *other* utterances of the same speaker. `datasets.py` now takes one sentence per LibriSpeech speaker, so it needs a
  per-speaker mode [i]. Later, with the user's consent, use the user's own recordings, including shouted ones, kept
  local.
- **Interferers:**
  - A different real speaker, the same sex and the opposite sex, at SIR +10 / +5 / 0 / -5 dB, dry and convolved with
    a room impulse response (a TV or roommate across the room).
  - The same, plus the real keyboard and vacuum noise already used.
  - Game sound played "through speakers" with a delay and a room response, with the clean game track as the AEC
    reference.
- **Measures:**
  - **Interfering voice removed.** Words of the interferer appearing in Koetama's speech to text, per minute (the
    leak measure VoxLock reports). The level of the output when the interferer talks alone.
  - **Target kept.** SI-SDR against the clean target, and WER on the target's words.
  - **False muting of the player.** On target-only input (normal, loud, quiet), the share of target speech frames
    attenuated by more than 6 dB, and seconds lost per 30 minutes (Microsoft's measure). This is the gate for shipping
    anything.
  - **Gate reaction time.** How many seconds from the interferer's start until it is attenuated.
  - **CPU, paced.** Mean, p99 and maximum time per 10 ms of audio, and the share of one core, with one thread, for
    VoxLock (int8, 40 and 80 ms chunks), the gate (CAM++ every 0.25 and 0.5 s) and sonora AEC3. Measure without a
    game first; with a game only when the user agrees and isn't playing.
- **Compare against the baseline:** DeepFilterNet 3 alone on the same mixes, to show what extraction adds over plain
  suppression.

## Gaps

- **Krisp BVC:** no Krisp-published latency, CPU, model size, price or terms for a free app. Whether consumer plans
  include it in 2026 is not known.
- **Teams, Zoom, Webex:** no latency, CPU or independent quality measurement. No product publishes how much of a
  nearby voice it removes.
- **VoxLock:**
  - No x86 desktop timing, and no paced figures.
  - Exact ONNX tensor shapes and whether `ort` runs it without custom ops: not verified.
  - No weights licence of its own. The REAL-TSE checkpoint licence is unstated.
  - Over-suppression and same-sex performance: not reported.
- **Sample rates:** not stated for pDeepFilterNet2, E3Net or pDCCRN. No open 48 kHz extractor was found.
- **Speaker-embedding cost:** no sherpa-onnx CPU timings. The CAM++ paper does not name its CPU. RAM use is not
  published for any model here.
- **Licences:** the licence of each 3D-Speaker CAM++ checkpoint is unconfirmed. WeSpeaker's CN-Celeb models have no
  stated licence.
- **FireRedChat-pVAD:** no architecture, sample rate, latency or accuracy.
- **Shouting, illness, children, laughter:** no figures for causal models.
- **AEC:** no x86 sonora/aec3 timings, no echo-return-loss figures on game audio, drift with Bluetooth untested.
  Whether cpal lists PipeWire monitors is unverified.
- **Opus:** the bandwidth chosen at 24 kbit/s VOIP is unchecked. It decides whether a 16 kHz extractor costs audible
  quality.
- **Privacy:** whether the EDPB voice-assistant guidelines were finalized was not confirmed from a primary source.
