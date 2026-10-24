//! NPCs that talk (docs/llm-npcs.md): a game mod defines its NPCs (persona, functions) and sends the lines said to
//! them; the HOST's Koetama asks its LLM and hands the replies (words, function calls) back to the game. Only the host
//! talks to the LLM - the game already carries every player's words to the host's script.
//!
//! - api: the two API styles (OpenAI-compatible chat completions; Anthropic messages) - the request each wants, what
//!   its answer says, and the call itself.
//! - npcs: the service on a thread of its own - each NPC's conversation (a fixed window of turns), its queue (lines
//!   that come while its request runs wait, then go together as one turn), function calls waiting for the game's
//!   results, and the host's limits (requests a minute, the month's budget).
//!
//! It knows no game: an NPC is an id, a persona and functions the mod gives.
pub mod api;
pub mod npcs;

pub use api::{Api, Call, Config, Reply, Turn};
pub use npcs::{Event, Npcs, OnEvent};
