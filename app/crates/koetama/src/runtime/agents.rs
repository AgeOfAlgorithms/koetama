//! The runtime's agents (docs/llm-agents.md): the host's LLM for the game mod's agents, once the host turned it on for
//! the game - its replies, the words so far of a streaming one, the agents it knows and its state go straight to the
//! game; the window reads the state. The tokens used are the ledger's, shared with the window.
use super::GameSlot;
use kd_common::Log;
use std::sync::{Arc, Mutex};

/// The agents' LLM as the window shows it, and as the game's "agent_status" says it.
#[derive(Clone, Debug, Default)]
pub struct AgentStatus {
    /// "ready", "busy", "budget", "error" ("" before the first word from the service)
    pub state: &'static str,
    pub why: String,
    /// the model and the month's limit, as set now (Runtime::set_llm)
    pub model: String,
    pub limit: kd_llm::Tokens,
}

/// The agents' service for the game in `slot` (made later: nothing is sent before it is). No ledger: one of its own,
/// from nothing used (the command line).
pub(super) fn start(
    cfg: &kd_llm::Config,
    ledger: Option<Arc<kd_llm::Ledger>>,
    slot: &GameSlot,
    status: &Arc<Mutex<AgentStatus>>,
    log: &Log,
) -> kd_llm::Agents {
    let (slot, st) = (slot.clone(), status.clone());
    {
        let mut s = st.lock().unwrap();
        (s.model, s.limit) = (cfg.model.clone(), cfg.limit);
    }
    let ledger = ledger.unwrap_or_else(|| kd_llm::Ledger::new(kd_llm::Tokens::default()));
    kd_llm::Agents::start(
        cfg.clone(),
        ledger,
        Arc::new(move |e| {
            let game = slot.lock().unwrap().clone();
            match e {
                kd_llm::Event::Reply { ids, agent, text, calls, error, why } => {
                    let calls: Vec<(String, String, serde_json::Value)> =
                        calls.into_iter().map(|c| (c.id, c.name, c.args)).collect();
                    if let Some(g) = game {
                        g.lock().unwrap().send_object(kd_games::api::agent_reply(&ids, &agent, &text, &calls, error, &why, true));
                    }
                }
                kd_llm::Event::Partial { ids, agent, text } => {
                    if let Some(g) = game {
                        g.lock().unwrap().send_object(kd_games::api::agent_reply(&ids, &agent, &text, &[], "", "", false));
                    }
                }
                kd_llm::Event::Known(list) => {
                    if let Some(g) = game {
                        g.lock().unwrap().set_standing("agents_known", kd_games::api::agents_known(&list));
                    }
                }
                kd_llm::Event::Status { state, why, used } => {
                    let (model, limit) = {
                        let mut s = st.lock().unwrap();
                        (s.state, s.why) = (state, why.clone());
                        (s.model.clone(), s.limit)
                    };
                    if let Some(g) = game {
                        let o = kd_games::api::agent_status(state, &why, &model, (used.input, used.output), (limit.input, limit.output));
                        g.lock().unwrap().set_standing("agent_status", o);
                    }
                }
            }
        }),
        log.clone(),
    )
}
