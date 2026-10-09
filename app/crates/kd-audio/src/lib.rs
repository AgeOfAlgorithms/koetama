//! Audio for Koetama, game-independent (engine/audio.py): the voice mixer (each voice placed by the game's gain,
//! direction and muffle), the low-pass that muffles, resampling, wav files, and the sound devices (cpal: the output
//! the mixer plays to, the microphone).
//!
//! Threads: a `Mixer` lives in a `SharedMixer` (Arc<Mutex>): the game's reader calls `MixerSink::set_feed`, the
//! output's audio thread locks it for each block (lock, render, unlock). `Output` and `Input` own their cpal stream:
//! keep them alive as long as the sound should play; dropping one stops it.
mod devices;
pub mod effects;
mod mixer;
mod wav;

pub use devices::{input_devices, output_devices, BlockFn, Input, Output, Rechunk, IN_BLOCK_S, OUT_BLOCK};
pub use mixer::{
    behind, lock, lowpass, lowpass_ir, pan_gains, Clip, Mixer, MixerSink, SharedMixer, Streams, BEHIND_MUFFLE,
    BEHIND_QUIET, CUT_CLEAR, CUT_MUFFLED, HEADROOM, LP_TAPS, PAN, RATE, SMOOTH, STALE,
};
pub use wav::{load_wav, percentile, read_wav, resample, write_wav16};
/// cpal's sample types (Output::open_converted takes one)
pub use cpal::SampleFormat;
