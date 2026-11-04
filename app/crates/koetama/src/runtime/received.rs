//! What of the game's requests Koetama has (PROTOCOL.md "received"): the feed resends its lines for the agents, the
//! results of their calls, the decisions on scored turns and the lines to translate until it is told they arrived.
use super::Sink;
use std::collections::HashSet;

/// The feed's requests already acknowledged this session (PROTOCOL.md "received").
#[derive(Default)]
pub(super) struct Received {
    sid: Option<i64>,
    lines: HashSet<i64>,
    results: HashSet<String>,
    decisions: HashSet<i64>,
    translate: HashSet<i64>,
}

impl Sink {
    /// The feed's requests Koetama has not said it has yet: one "received" for them (sent at the next tick) - the game
    /// may stop sending them, their answers come as before. Only what is used: agent lines and results with the
    /// agents on, lines to translate with translation on and not stopped.
    pub(super) fn acknowledge(&self, feed: &kd_common::feed::Feed) {
        let mut r = self.received.lock().unwrap();
        if r.sid != Some(feed.sid) {
            *r = Received { sid: Some(feed.sid), ..Default::default() };
        }
        let mut lines = Vec::new();
        let mut results = Vec::new();
        let mut decisions = Vec::new();
        let mut translate = Vec::new();
        if self.agents.is_some() {
            lines = feed.to_agent.iter().map(|l| l.id).filter(|id| r.lines.insert(*id)).collect();
            results = feed.agent_results.iter().map(|(c, _)| c.clone()).filter(|c| r.results.insert(c.clone())).collect();
            decisions = feed.agent_decisions.iter().map(|d| d.turn).filter(|t| r.decisions.insert(*t)).collect();
        }
        if self.translator.is_some() && feed.translate {
            translate = feed.to_translate.iter().map(|(id, _)| *id).filter(|id| r.translate.insert(*id)).collect();
        }
        if !lines.is_empty() || !results.is_empty() || !decisions.is_empty() || !translate.is_empty() {
            self.receipts.lock().unwrap().push(kd_games::api::received(&lines, &results, &decisions, &translate));
        }
    }
}
