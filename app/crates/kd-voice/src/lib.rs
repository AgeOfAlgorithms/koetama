//! Real voices between players (PROTOCOL.md "Real voices"). The game names a voice
//! ROOM (a name and a key every player in its session gets: Koetama makes it, kind 'r'), whom this player's voice
//! should reach now (`to`) and how loud each other player is (the speakers, src 0). Each Koetama holds a WebSocket
//! to the room on the relay (a Cloudflare Worker) and sends its player's voice there, encrypted with the room's key:
//! the relay only moves opaque bytes to the players named.
//!
//!   microphone (48 kHz) -> gate (push to talk / the speech detector) -> Opus (20 ms frames, 3 a packet) ->
//!   plaintext packet -> ChaCha20-Poly1305 -> relay frame [1][n][to..][payload] -> WebSocket
//!   WebSocket -> [1][from][payload] -> decrypt (aad = from) -> per-sender jitter buffer -> Opus (loss concealment) ->
//!   the mixer (kd_audio::Streams), placed like the test voices by the feed's gain, direction and muffle
//!
//! Threads: one "voice" thread per Voice (the connection, reading, encoding, sending); the microphone's callback only
//! queues its blocks (Voice::push_mic); the audio output's callback decodes as it plays (Playback, under a short lock).
pub mod codec;
pub mod crypto;
pub mod frames;
pub mod hub;
pub mod agc;
pub mod gate;
pub mod jitter;
pub mod packet;
pub mod relay;
mod voice;

pub use voice::{Playback, Sender, Voice, VoiceEvent, VoiceStatus, HEARD, PRESENCE_EVERY, PRESENT};

/// The relay Koetama uses (KOETAMA_RELAY overrides it: tests, a relay of one's own)
pub const RELAY: &str = "wss://koetama-relay.ageofalgorithms.workers.dev";
/// Hz: Opus's full band, and the mixer's rate
pub const RATE: u32 = 48000;
/// samples in one Opus frame (20 ms)
pub const FRAME: usize = 960;
/// Opus frames in one packet (60 ms)
pub const PER_PACKET: usize = 3;
/// samples in one packet
pub const PACKET: usize = FRAME * PER_PACKET;
/// bits a second (Opus VOIP)
pub const BITRATE: i32 = 24000;

/// The relay's address: KOETAMA_RELAY (set and not empty), else RELAY.
pub fn relay_url() -> String {
    std::env::var("KOETAMA_RELAY").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| RELAY.into())
}
