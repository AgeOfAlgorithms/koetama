//! Agents (docs/llm-agents.md): a game mod defines its agents (persona, functions) and sends the lines said to
//! them; the HOST's Koetama asks its LLM and hands the replies (words, function calls) back to the game. Only the host
//! talks to the LLM - the game already carries every player's words to the host's script.
//!
//! - api: the two API styles (OpenAI-compatible chat completions; Anthropic messages) - the request each wants, what
//!   its answer says, and the call itself.
//! - agents: the service on a thread of its own - each agent's conversation (a fixed window of turns), its queue (lines
//!   that come while its request runs wait, then go together as one turn), function calls waiting for the game's
//!   results, and the host's limits (requests a minute, the month's budget).
//!
//! It knows no game: an agent is an id, a persona and functions the mod gives.
pub mod agents;
pub mod api;
pub mod ledger;
pub mod prices;

pub use api::{Api, Call, Config, Reply, Turn};
pub use agents::{error, Agents, Event, OnEvent};
pub use ledger::Ledger;
pub use prices::Prices;
