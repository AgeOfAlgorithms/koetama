//! The agent service: the game's agent definitions and lines come in (feed()), the replies go out (Event), on a thread of
//! its own; each request to the LLM on a thread of its own too.
//!
//! - Each agent keeps its conversation: the last MAX_TURNS turns (a fixed window - no summaries), its first a player's.
//! - Its queue: a line that comes while the agent's request runs (or while its function calls wait for the game's
//!   results) waits; once it is free, every waiting line goes in ONE turn, and the reply answers them all (`ids`).
//! - Function calls: the reply's calls go to the game (Event::Reply with `calls`); the game runs them, checks them
//!   against its rules and answers each (feed's agent_results); with all of them answered (or after RESULTS_WAIT
//!   without), they go back to the LLM and its follow-up is another reply to the same lines.
//! - The host's limits: requests a minute (more wait in the queue: "busy"), the month's budget (the Ledger, shared with
//!   the window: each request holds what it could cost at worst before it goes - "budget" when that would pass it).
//! - What it knows: Event::Known (each agent's id and version) whenever that changes - the game sends a definition only
//!   when it is missing or older.
//! - A new game session (`sid`) starts every agent over; `agent_forget` starts one over and cancels what it had under
//!   way (its lines answered "cancelled"). At most MAX_QUEUE lines wait per agent ("queue_full").
//! - A reply that is not one says why with a code (Error) and words (`why`).
use crate::api::{self, Call, Config, Reply, Turn};
use crate::ledger::Ledger;
use kd_common::feed::{AgentDef, AgentLine};
use kd_common::Log;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// an agent's conversation keeps this many turns (a player's line and the reply are two)
pub const MAX_TURNS: usize = 24;
/// function calls wait this long for the game's results, then go back "(no result)"
pub const RESULTS_WAIT: Duration = Duration::from_secs(20);
/// one request to the LLM takes at most this long
pub const TIMEOUT: Duration = Duration::from_secs(60);
/// a streaming agent's words go to the game at most this often
pub const PARTIAL_EVERY: Duration = Duration::from_millis(150);
/// the line ids remembered in a session (each answered once), the newest
const SEEN_MAX: usize = 4096;
/// the most lines waiting for one agent; more are answered "queue_full"
pub const MAX_QUEUE: usize = 16;

/// Why a reply is not one (agent_reply's `error`; `why` says more).
pub mod error {
    /// the host's budget for the month would be passed
    pub const BUDGET: &str = "budget";
    /// a line for an agent Koetama does not have (send its definition)
    pub const UNKNOWN_AGENT: &str = "unknown_agent";
    /// the request failed (the model, the network, the key)
    pub const FAILED: &str = "failed";
    /// agent_forget (or a new definition's reset) dropped it
    pub const CANCELLED: &str = "cancelled";
    /// too many lines were waiting for the agent
    pub const QUEUE_FULL: &str = "queue_full";
}

/// What the service tells the program (for the game, and the host's setting).
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// the reply to lines `ids` of agent `agent`: its words and function calls (the game runs them and answers with
    /// agent_results); `error` (an error:: code) and `why` when it is not one - then text and calls are empty
    Reply { ids: Vec<i64>, agent: String, text: String, calls: Vec<Call>, error: &'static str, why: String },
    /// a streaming agent's words so far, while its reply comes (a few times a second at most; the Reply follows)
    Partial { ids: Vec<i64>, agent: String, text: String },
    /// "ready", "busy" (requests a minute reached: lines wait), "budget" (the month's budget is spent), "error" (the
    /// last request failed: `why`); spent: this month so far
    Status { state: &'static str, why: String, spent: f64 },
    /// the agents Koetama has now, (id, version), sorted: after each change (and empty for a new session)
    Known(Vec<(String, i64)>),
}

pub type OnEvent = Arc<dyn Fn(Event) + Send + Sync>;

enum Msg {
    Config(Config),
    Feed { sid: i64, agents: Vec<AgentDef>, lines: Vec<AgentLine>, results: Vec<(String, String)>, forget: Vec<String> },
    Done { agent: String, gen: u64, result: Result<Reply, String> },
    Partial { agent: String, gen: u64, text: String },
    Stop,
}

/// The service's handle (clones share it): its thread runs until stop().
#[derive(Clone)]
pub struct Agents {
    tx: Arc<Mutex<Sender<Msg>>>,
    thread: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl Agents {
    /// The service, counting what it spends in `ledger` (shared with whatever else asks the same model).
    pub fn start(cfg: Config, ledger: Arc<Ledger>, on_event: OnEvent, log: Log) -> Agents {
        let (tx, rx) = mpsc::channel();
        let mut w = Worker {
            cfg,
            ledger,
            known: None,
            on_event,
            log,
            tx: tx.clone(),
            sid: None,
            agents: HashMap::new(),
            seen: HashSet::new(),
            seen_order: VecDeque::new(),
            sent: VecDeque::new(),
            gen: 0,
            told: None,
            failed: None,
        };
        let thread = std::thread::Builder::new()
            .name("koetama-agents".into())
            .spawn(move || w.run(rx))
            .expect("the agent thread");
        Agents { tx: Arc::new(Mutex::new(tx)), thread: Arc::new(Mutex::new(Some(thread))) }
    }

    fn send(&self, m: Msg) {
        let _ = self.tx.lock().unwrap_or_else(|e| e.into_inner()).send(m);
    }

    /// The host changed the setting (model, key, limits): what comes next uses it.
    pub fn set_config(&self, cfg: Config) {
        self.send(Msg::Config(cfg));
    }

    /// What a feed says for agents (cheap: call it with every feed; each line is answered once).
    pub fn feed(&self, sid: i64, agents: &[AgentDef], lines: &[AgentLine], results: &[(String, String)], forget: &[String]) {
        self.send(Msg::Feed { sid, agents: agents.to_vec(), lines: lines.to_vec(), results: results.to_vec(), forget: forget.to_vec() });
    }

    /// Stops the thread (a request under way finishes on its own; its reply is dropped).
    pub fn stop(&self) {
        self.send(Msg::Stop);
        if let Some(t) = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = t.join();
        }
    }
}

/// Function calls waiting for the game's results: the lines they answer, the call ids, the results so far, since when.
type Waiting = (Vec<i64>, Vec<String>, Vec<(String, String)>, Instant);

/// An agent on the thread.
#[derive(Default)]
struct Agent {
    def: AgentDef,
    history: Vec<Turn>,
    /// lines waiting for the agent to be free
    queue: Vec<AgentLine>,
    /// the request under way: its generation (a reply from an older one is dropped), the lines it answers, the
    /// history's length before it (put back if it fails), and what the ledger holds for it
    busy: Option<(u64, Vec<i64>, usize, f64)>,
    /// function calls waiting for the game's results
    waiting: Option<Waiting>,
}

struct Worker {
    cfg: Config,
    ledger: Arc<Ledger>,
    /// the agents as last told (Event::Known)
    known: Option<Vec<(String, i64)>>,
    on_event: OnEvent,
    log: Log,
    tx: Sender<Msg>,
    sid: Option<i64>,
    agents: HashMap<String, Agent>,
    seen: HashSet<i64>,
    seen_order: VecDeque<i64>,
    /// when the last requests were sent (requests a minute)
    sent: VecDeque<Instant>,
    gen: u64,
    /// the state as last told (Event::Status): state, why, cents spent
    told: Option<(&'static str, String, i64)>,
    /// the last request failed (and why), until one succeeds
    failed: Option<String>,
}

/// The system prompt: for a character, Koetama's words before its persona - how the lines come and what a reply is
/// (the mod's persona says who); for any other agent (a director, a writer, a command parser), the persona alone.
fn system(def: &AgentDef) -> String {
    if !def.character {
        return def.persona.clone();
    }
    format!(
        "You are a character in a multiplayer game, in its world. Each message lists what was said near you, one line \
         each as `Name: words`; a line in [brackets] is something that happened, and [the game: ...] is what the game \
         says is going on. Reply only with what you say aloud, in at most {} words, in the language you are spoken to. \
         Use your functions to act; the game decides whether an action happens.\n\n{}",
        def.max_words, def.persona
    )
}

/// The lines as one turn: the newest game state first, then each line.
fn user_turn(lines: &[AgentLine]) -> String {
    let mut out = String::new();
    if let Some(s) = lines.iter().rev().map(|l| l.state.as_str()).find(|s| !s.is_empty()) {
        out.push_str(&format!("[the game: {s}]\n"));
    }
    for l in lines {
        let who = if !l.from_name.is_empty() {
            Some(l.from_name.clone())
        } else {
            l.from_id.as_ref().map(|p| p.text.clone())
        };
        match who {
            Some(w) => out.push_str(&format!("{w}: {}\n", l.text)),
            None => out.push_str(&format!("[{}]\n", l.text)),
        }
    }
    out.trim_end().to_string()
}

/// The history kept to MAX_TURNS, starting with a player's turn (never with a reply or results cut from theirs).
fn trimmed(mut h: Vec<Turn>) -> Vec<Turn> {
    if h.len() > MAX_TURNS {
        h.drain(..h.len() - MAX_TURNS);
    }
    while !h.is_empty() && !matches!(h[0], Turn::User(_)) {
        h.remove(0);
    }
    h
}

impl Worker {
    fn run(&mut self, rx: mpsc::Receiver<Msg>) {
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(Msg::Config(c)) => {
                    // (another API, address, model or key: the last failure was the old one's - not shown as this one's)
                    if (c.api, &c.url, &c.model, &c.key) != (self.cfg.api, &self.cfg.url, &self.cfg.model, &self.cfg.key) {
                        self.failed = None;
                    }
                    self.cfg = c;
                }
                Ok(Msg::Feed { sid, agents, lines, results, forget }) => self.feed(sid, agents, lines, results, forget),
                Ok(Msg::Done { agent, gen, result }) => self.done(&agent, gen, result),
                Ok(Msg::Partial { agent, gen, text }) => {
                    if let Some((g, ids, ..)) = self.agents.get(&agent).and_then(|a| a.busy.as_ref()) {
                        if *g == gen {
                            (self.on_event)(Event::Partial { ids: ids.clone(), agent, text });
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.overdue_results();
            self.dispatch();
            self.tell();
        }
    }

    fn feed(&mut self, sid: i64, agents: Vec<AgentDef>, lines: Vec<AgentLine>, results: Vec<(String, String)>, forget: Vec<String>) {
        if self.sid != Some(sid) {
            // (a new game session: every agent starts over; a request under way is dropped when it comes back)
            self.sid = Some(sid);
            self.agents.clear();
            self.seen.clear();
            self.seen_order.clear();
        }
        for d in agents {
            let n = self.agents.entry(d.id.clone()).or_default();
            n.def = d;
        }
        for id in forget {
            self.forget(&id);
        }
        for l in lines {
            if !self.seen.insert(l.id) {
                continue;
            }
            self.seen_order.push_back(l.id);
            if self.seen_order.len() > SEEN_MAX {
                if let Some(old) = self.seen_order.pop_front() {
                    self.seen.remove(&old);
                }
            }
            match self.agents.get_mut(&l.agent) {
                Some(n) if n.queue.len() < MAX_QUEUE => n.queue.push(l),
                Some(_) => self.fail(vec![l.id], &l.agent, error::QUEUE_FULL, format!("{MAX_QUEUE} lines already wait for it")),
                None => self.fail(vec![l.id], &l.agent, error::UNKNOWN_AGENT, format!("no agent \"{}\": send its definition in agents", l.agent)),
            }
        }
        for (call, result) in results {
            for n in self.agents.values_mut() {
                if let Some((_, calls, got, _)) = n.waiting.as_mut() {
                    if calls.contains(&call) && !got.iter().any(|(c, _)| *c == call) {
                        got.push((call.clone(), result.clone()));
                    }
                }
            }
        }
        // (every call answered: the results go back to the LLM)
        let ready: Vec<String> = self
            .agents
            .iter()
            .filter(|(_, n)| n.waiting.as_ref().is_some_and(|(_, calls, got, _)| got.len() >= calls.len()))
            .map(|(k, _)| k.clone())
            .collect();
        for k in ready {
            self.results_back(&k);
        }
    }

    /// agent_forget: its conversation starts over, and what it had under way is cancelled (a request's reply dropped,
    /// its lines, the waiting ones and those whose calls wait for results answered "cancelled").
    fn forget(&mut self, k: &str) {
        let Some(n) = self.agents.get_mut(k) else { return };
        n.history.clear();
        let mut ids: Vec<i64> = Vec::new();
        if let Some((_, busy_ids, _, held)) = n.busy.take() {
            ids.extend(busy_ids);
            self.ledger.settle(held, 0.0);
        }
        if let Some((waiting_ids, ..)) = n.waiting.take() {
            ids.extend(waiting_ids);
        }
        ids.extend(n.queue.drain(..).map(|l| l.id));
        if !ids.is_empty() {
            self.fail(ids, k, error::CANCELLED, "the game made it forget".into());
        }
    }

    /// Calls the game has not answered in RESULTS_WAIT go back "(no result)".
    fn overdue_results(&mut self) {
        let late: Vec<String> = self
            .agents
            .iter()
            .filter(|(_, n)| n.waiting.as_ref().is_some_and(|(.., since)| since.elapsed() >= RESULTS_WAIT))
            .map(|(k, _)| k.clone())
            .collect();
        for k in late {
            if let Some((_, calls, got, _)) = self.agents.get_mut(&k).and_then(|n| n.waiting.as_mut()) {
                for c in calls.iter() {
                    if !got.iter().any(|(g, _)| g == c) {
                        got.push((c.clone(), "(no result)".into()));
                    }
                }
            }
            self.results_back(&k);
        }
    }

    /// The agent's calls are all answered: their results join its conversation and its follow-up is asked for.
    fn results_back(&mut self, k: &str) {
        let Some(n) = self.agents.get_mut(k) else { return };
        let Some((ids, calls, got, _)) = n.waiting.take() else { return };
        // (in the order the LLM made the calls)
        let ordered: Vec<(String, String)> =
            calls.iter().filter_map(|c| got.iter().find(|(g, _)| g == c).cloned()).collect();
        let before = n.history.len();
        n.history.push(Turn::Results(ordered));
        self.ask(k, ids, before);
    }

    /// Every agent that is free and has lines waiting: its lines go as one turn (within the host's limits).
    fn dispatch(&mut self) {
        let free: Vec<String> = self
            .agents
            .iter()
            .filter(|(_, n)| n.busy.is_none() && n.waiting.is_none() && !n.queue.is_empty())
            .map(|(k, _)| k.clone())
            .collect();
        for k in free {
            if !self.may_send() {
                return; // (requests a minute reached: they wait)
            }
            let n = self.agents.get_mut(&k).expect("listed");
            let lines: Vec<AgentLine> = std::mem::take(&mut n.queue);
            let ids = lines.iter().map(|l| l.id).collect();
            n.history = trimmed(std::mem::take(&mut n.history));
            let before = n.history.len();
            n.history.push(Turn::User(user_turn(&lines)));
            self.ask(&k, ids, before);
        }
    }

    fn over_budget(&self) -> bool {
        self.ledger.spent() >= self.cfg.budget
    }

    /// A request may go now (requests a minute): counted if so.
    fn may_send(&mut self) -> bool {
        let now = Instant::now();
        while self.sent.front().is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60)) {
            self.sent.pop_front();
        }
        if self.sent.len() >= self.cfg.per_minute.max(1) as usize {
            return false;
        }
        self.sent.push_back(now);
        true
    }

    /// Ask the LLM for agent k's next words (its history already has the new turn), on a thread of its own - if the
    /// ledger can hold what it could cost at worst (its prompt, about 4 characters a token, and its longest reply):
    /// else its lines are answered "budget" and the turn taken back.
    fn ask(&mut self, k: &str, ids: Vec<i64>, before: usize) {
        let Some(n) = self.agents.get_mut(k) else { return };
        let (cfg, sys, functions, turns) = (self.cfg.clone(), system(&n.def), n.def.functions.clone(), n.history.clone());
        let max_tokens = (n.def.max_words.max(1) as u32).saturating_mul(2).saturating_add(256).min(4096);
        let prompt: usize = sys.len()
            + functions.iter().map(|f| f.description.len() + f.parameters.to_string().len()).sum::<usize>()
            + turns.iter().map(api::turn_len).sum::<usize>();
        let most = (prompt as f64 / 4.0 * cfg.price_in + max_tokens as f64 * cfg.price_out) / 1e6;
        if !self.ledger.reserve(most, cfg.budget) {
            n.history.truncate(before);
            self.fail(ids, k, error::BUDGET, format!("this month's ${:.2} would be passed", cfg.budget));
            return;
        }
        self.gen += 1;
        n.busy = Some((self.gen, ids, before, most));
        let (tx, agent, gen, stream) = (self.tx.clone(), k.to_string(), self.gen, n.def.stream);
        std::thread::spawn(move || {
            let result = if stream {
                // (the words so far to the game at most every PARTIAL_EVERY, and only when there are more)
                let (mut last, mut told) = (Instant::now() - PARTIAL_EVERY, 0usize);
                let (ptx, pagent) = (tx.clone(), agent.clone());
                api::complete_streaming(&cfg, &sys, &functions, &turns, max_tokens, TIMEOUT, &mut |t: &str| {
                    if t.len() > told && last.elapsed() >= PARTIAL_EVERY {
                        (last, told) = (Instant::now(), t.len());
                        let _ = ptx.send(Msg::Partial { agent: pagent.clone(), gen, text: t.to_string() });
                    }
                })
            } else {
                api::complete(&cfg, &sys, &functions, &turns, max_tokens, TIMEOUT)
            };
            let _ = tx.send(Msg::Done { agent, gen, result });
        });
    }

    fn done(&mut self, k: &str, gen: u64, result: Result<Reply, String>) {
        let Some(n) = self.agents.get_mut(k) else { return };
        let Some((g, ids, before, held)) = n.busy.take() else { return };
        if g != gen {
            n.busy = Some((g, ids, before, held));
            return;
        }
        match result {
            Ok(r) => {
                n.history.push(Turn::Assistant { text: r.text.clone(), calls: r.calls.clone() });
                if !r.calls.is_empty() {
                    n.waiting = Some((ids.clone(), r.calls.iter().map(|c| c.id.clone()).collect(), Vec::new(), Instant::now()));
                }
                self.ledger.settle(held, self.cfg.cost(&r));
                self.failed = None;
                (self.log)(&format!(
                    "agent {k}: replied ({} tokens in, {} out{})",
                    r.tokens_in,
                    r.tokens_out,
                    if r.calls.is_empty() { String::new() } else { format!(", {} function calls", r.calls.len()) }
                ));
                self.reply(ids, k, r.text, r.calls);
            }
            Err(e) => {
                n.history.truncate(before);
                self.ledger.settle(held, 0.0);
                (self.log)(&format!("agent {k}: {e}"));
                self.failed = Some(e.clone());
                self.fail(ids, k, error::FAILED, e);
            }
        }
    }

    fn reply(&self, ids: Vec<i64>, agent: &str, text: String, calls: Vec<Call>) {
        (self.on_event)(Event::Reply { ids, agent: agent.to_string(), text, calls, error: "", why: String::new() });
    }

    fn fail(&self, ids: Vec<i64>, agent: &str, code: &'static str, why: String) {
        (self.on_event)(Event::Reply { ids, agent: agent.to_string(), text: String::new(), calls: Vec::new(), error: code, why });
    }

    /// The status when it changed, and the agents it has when they changed.
    fn tell(&mut self) {
        let mut known: Vec<(String, i64)> = self.agents.iter().map(|(k, a)| (k.clone(), a.def.version)).collect();
        known.sort();
        if self.known.as_ref() != Some(&known) {
            (self.on_event)(Event::Known(known.clone()));
            self.known = Some(known);
        }
        let waiting = self.agents.values().any(|n| !n.queue.is_empty());
        let now = if self.over_budget() {
            ("budget", String::new())
        } else if let Some(e) = &self.failed {
            ("error", e.clone())
        } else if waiting && self.agents.values().any(|n| n.busy.is_none() && n.waiting.is_none() && !n.queue.is_empty()) {
            ("busy", String::new())
        } else {
            ("ready", String::new())
        };
        // (again when the state changes, or what was spent by a cent: the game shows it)
        let spent = self.ledger.spent();
        let now = (now.0, now.1, (spent * 100.0).round() as i64);
        if self.told.as_ref() != Some(&now) {
            (self.on_event)(Event::Status { state: now.0, why: now.1.clone(), spent });
            self.told = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kd_common::feed::PlayerId;

    fn line(id: i64, from: Option<&str>, text: &str, state: &str) -> AgentLine {
        AgentLine {
            id,
            agent: "pig".into(),
            text: text.into(),
            from_id: from.map(PlayerId::string),
            from_name: String::new(),
            state: state.into(),
        }
    }

    #[test]
    fn lines_make_one_turn() {
        let t = user_turn(&[
            line(1, Some("Wendy"), "10 meat for a helmet?", "winter"),
            line(2, None, "the pig king burped", ""),
            line(3, Some("Wes"), "hi", "winter, day 31"),
        ]);
        assert_eq!(t, "[the game: winter, day 31]\nWendy: 10 meat for a helmet?\n[the pig king burped]\nWes: hi");
        let d = AgentDef { persona: "You are the Pig King.".into(), max_words: 40, character: true, ..Default::default() };
        assert!(system(&d).ends_with("You are the Pig King.") && system(&d).contains("at most 40 words"));
        let director = AgentDef { persona: "Plan the next wave.".into(), character: false, ..d };
        assert_eq!(system(&director), "Plan the next wave.", "not a character: the persona alone");
    }

    #[test]
    fn the_window_starts_with_a_players_turn() {
        let mut h = Vec::new();
        for i in 0..30 {
            h.push(Turn::User(format!("u{i}")));
            h.push(Turn::Assistant { text: format!("a{i}"), calls: vec![] });
        }
        let t = trimmed(h);
        assert!(t.len() <= MAX_TURNS && matches!(t[0], Turn::User(_)));
        assert_eq!(t.last(), Some(&Turn::Assistant { text: "a29".into(), calls: vec![] }));
        let odd = trimmed(vec![Turn::Results(vec![]), Turn::Assistant { text: "x".into(), calls: vec![] }, Turn::User("u".into())]);
        assert_eq!(odd, vec![Turn::User("u".into())]);
    }
}
