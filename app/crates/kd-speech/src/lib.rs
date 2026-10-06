//! Kotodama's speech-to-text (engine/asr.py): what the player says, as live words while they talk and as a finished
//! line after. Chosen by the benchmarks in bench/ (see PROJECT.md):
//!
//! ```text
//!   speech detector   Silero VAD v5 (2.3 MB, MIT): where a line starts and ends
//!   the words         the line so far, transcribed again every ROLL_EVERY s while the player talks (the live words),
//!                     then once more over the whole line (the finished line), by the language's own model:
//!                       Russian                                GigaAM v3 CTC (Sber, MIT)
//!                       Mandarin, Cantonese, Japanese, Korean  SenseVoice Small (Alibaba, FunASR model license)
//!                       the other European languages           Parakeet TDT 0.6B v3 int8 (NVIDIA, CC-BY-4.0)
//!   "auto" language   SpeechBrain's VoxLingua107 ECAPA language detector (Apache-2.0; our ONNX export,
//!                     engine/export_lid.py) cuts a line into stretches by language, each written by its
//!                     language's model
//! ```
//! Everything runs on the CPU (sherpa-onnx / ONNX Runtime), on this machine; nothing is sent anywhere. The models are
//! downloaded once (Hugging Face) the first time they are needed.
//!
//! ```text
//! let lst = Listener::new(callbacks, Models::new(4, log), true)?;   // on_start, on_live, on_final
//! lst.set_language("ru");                 // the game's "Language I speak" ("auto": found per stretch)
//! lst.feed(&samples_16k);                 // from the microphone, any block size; or in a thread: lst.start(), lst.push()
//! ```
//!
//! Engines: sherpa-onnx (shared: sherpa-onnx-c-api.dll + onnxruntime.dll next to the program) for the speech models
//! and the speech detector, `ort` (load-dynamic) on THAT SAME onnxruntime.dll for the language detector
//! ([`init_onnxruntime`], done by [`Models::load`]`("langid")` itself).
mod listener;
mod mics;
mod models;
mod rolling;
mod stitch;

pub use listener::{low_priority, Callbacks, Listener, OnFinal, OnLive, OnStart};
pub use mics::{Mic, PlaylistMicrophone, WavMicrophone};
pub use models::{init_onnxruntime, lid_dir, Models};
pub use rolling::{FinalInfo, LineInfo, RollingLine};
pub use stitch::{quiet_point, segments, transcribe_mixed, Mixed, Probs};

pub const RATE: u32 = 16000;
/// s before the detected speech fed too
pub const PREROLL: f64 = 1.0;
/// s: a longer line is cut
pub const MAX_LINE: f64 = 15.0;
pub const VAD_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad_v5.onnx";

/// A speech model: its Hugging Face repo, the exact revision tested, the files used; the loader: Models::load.
/// (pinned: a later change to a repo cannot change what the helper runs; only these files are downloaded - the
///  SenseVoice repo also holds a 900 MB full-precision copy and test recordings the helper does not use)
#[derive(Clone, Copy, Debug)]
pub struct ModelSpec {
    pub name: &'static str,
    pub repo: &'static str,
    pub revision: &'static str,
    pub files: &'static [&'static str],
}

pub const MODELS: [ModelSpec; 3] = [
    ModelSpec {
        name: "gigaam",
        repo: "csukuangfj/sherpa-onnx-nemo-ctc-giga-am-v3-russian-2025-12-16",
        revision: "32a4c7cc81809bd132e2d935ab99e9e6ab47fbec",
        files: &["model.int8.onnx", "tokens.txt", "LICENSE"],
    },
    // (the 2024-07-17 release: the 2025-09-09 one is a Cantonese fine-tune that lost Japanese and Korean - 74 / 91 %
    //  of the characters wrong against 2.0 / 1.1 % here; bench/cjk.py)
    ModelSpec {
        name: "sensevoice",
        repo: "csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17",
        revision: "2365baeacb507f821a0c8120fcee3d484dba7a07",
        files: &["model.int8.onnx", "tokens.txt", "LICENSE"],
    },
    ModelSpec {
        name: "parakeet",
        repo: "csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
        revision: "2bda32ec70b097a55adaa07d9a7173915b43cc78",
        files: &["encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"],
    },
];

/// s: a rolling line is transcribed again this often...
pub const ROLL_EVERY: f64 = 1.0;
/// ...unless a pass takes more than this share of it: then twice as long (up to ROLL_MAX),
pub const ROLL_SLOW: f64 = 0.5;
///    remembered for the session (Models::every): a slow PC gets fewer, later live words
pub const ROLL_MAX: f64 = 4.0;
/// a language's own model (else Parakeet)
pub const ROLL_MODEL: [(&str, &str); 5] =
    [("ru", "gigaam"), ("zh", "sensevoice"), ("yue", "sensevoice"), ("ja", "sensevoice"), ("ko", "sensevoice")];

/// The one model of a language in the rolling design: its own recogniser, else Parakeet v3.
pub fn roll_model(lang: &str) -> &'static str {
    ROLL_MODEL.iter().find(|(l, _)| *l == lang).map(|(_, m)| *m).unwrap_or("parakeet")
}

// ---------------------------------------------------------------- mixed languages ("auto": the language decided per stretch)
/// SpeechBrain lang-id-voxlingua107-ecapa (Apache-2.0) as ONNX: export_lid.py builds it
pub const LID_NAME: &str = "voxlingua107-ecapa";
/// The languages "auto" chooses between; Cantonese is found as zh and SenseVoice writes it. Not every supported one:
/// (more candidates, more wrong stretches - the 13 supported ones made Russian lines 18 % wrong against 8 % (taken for
///  Ukrainian), English 5 % against 3 %; bench/autolangs.py. A player of another language picks it.)
pub const MIXED_LANGS: [&str; 10] = ["en", "ru", "zh", "es", "de", "fr", "it", "pt", "ja", "ko"];
/// s: windows (bench/lid.py)
pub const LID_WIN: f64 = 1.0;
/// s: the shortest stretch kept (bench/lid.py)
pub const LID_MIN: f64 = 0.5;
/// s between two windows (0.5: half the detector's cost - bench/budget.py)
pub const LID_HOP: f64 = 0.25;
/// dB under the line's loudest 0.25 s (or under -50 dBFS): quiet, it does not vote
pub const LID_QUIET: f64 = 25.0;
/// what a change of language costs (log-probability) - bench/lidtune.py
pub const LID_SWITCH: f64 = 4.0;
/// a short line: the detector's language only when this sure, else the fallback
pub const LID_SURE: f64 = 0.8;

/// A poisoned lock is still used (a callback that panicked must not stop the speech for good).
pub(crate) fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
