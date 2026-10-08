//! Chat translation on the player's own PC (PROTOCOL.md "Translation").
//!
//!   engine     Mozilla's Firefox Translations models (Marian students: an 8-bit transformer encoder, an SSRU decoder)
//!              run in plain Rust - the model file, the 8-bit matrix maths, the SentencePiece vocabulary, the lexical
//!              shortlist, greedy decoding. engine/mt.py is its reference.
//!   catalog    Mozilla's model list (Remote Settings) and the downloads: which files a rule needs (one direction with
//!              English, two through it), into Koetama's data folder, checked (sha256)
//!   detect     which language each stretch of a chat line is in (by script, then a small detector for Latin /
//!              Cyrillic text; mixed-language lines are split)
//!   service    the translator: up to two rules, a request queue on a thread of its own, the models loaded lazily
//!              (downloaded first) and let go when no rule needs them, the rules' state for the game
pub mod catalog;
pub mod detect;
pub mod engine;
pub mod service;

pub use engine::Model;
