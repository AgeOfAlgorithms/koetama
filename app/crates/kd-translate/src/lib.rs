//! Chat translation on the player's own PC (PROTOCOL.md "Translation").
//!
//!   engine     Mozilla's Firefox Translations models (Marian students: an 8-bit transformer encoder, an SSRU decoder)
//!              run in plain Rust - the model file, the 8-bit matrix maths, the SentencePiece vocabulary, the lexical
//!              shortlist, greedy decoding. engine/mt.py is its reference.
//!   catalog    Mozilla's model list (Remote Settings) and the downloads: which files a pair needs (one direction with
//!              English, two through it), into Koetama's data folder, checked (sha256); the languages to translate into
//!   detect     which language each stretch of a chat line is in (by script, then a small detector for Latin /
//!              Cyrillic text; mixed-language lines are split)
//!   service    the translator: the player's setting (the target language, the languages they speak, downloads on or
//!              off), a request queue on a thread of its own, a pair per foreign language seen (its models fetched
//!              the first time, the least recently used let go past four), the pairs' states for the game
pub mod catalog;
pub mod detect;
pub mod engine;
pub mod service;

pub use engine::Model;
