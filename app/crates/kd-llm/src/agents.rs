//! The agent service: the game's agent definitions and lines come in (feed()), the replies go out (Event), on a thread of
//! its own; each request to the LLM on a thread of its own too.
//!
//! - Each agent keeps its conversation: the last MAX_TURNS turns (a fixed window - no summaries), its first a player's.
//! - Its queue: a line that comes while the agent's request runs (or while its function calls wait for the game's
//!   results) waits; once it is free, every waiting line goes in ONE turn, and the reply answers them all (`ids`).
//! - Function calls: the reply's calls go to the game (Event::Reply with `calls`); the game runs them, checks them
//!   against its rules and answers each (feed's agent_results); with all of them answered (or after RESULTS_WAIT
//!   without), they go back to the LLM and its follow-up is another reply to the same lines.
//! - The host's limits: requests a minute (more wait in the queue: "busy"), the month's tokens in and out (the Ledger,
//!   shared with the window: each request holds what it could use at worst before it goes - "budget" when that would
//!   pass the limit).
//! - What it knows: Event::Known (each agent's id and version) whenever that changes - the game sends a definition only
//!   when it is missing or older.
//! - A new game session (`sid`) starts every agent over; `agent_forget` starts one over and cancels what it had under
//!   way (its lines answered "cancelled"). At most MAX_QUEUE lines wait per agent ("queue_full").
//! - A reply that is not one says why with a code (Error) and words (`why`).
//! - Scores (the agent's `criteria`): a turn of only players' lines and context events first goes to a short request
//!   that rates it 0..1 on each criterion - the reply's own request with the question at the end (api::Turn::Note), so
//!   the reply's, when it comes, starts as one the provider has cached; the scores go to the game (Event::Scores, with a turn number)
//!   and the turn waits for its decision (decide(); DECIDE_WAIT at most, then it is answered). Not answered: its lines
//!   are answered `skipped`, kept (the newest MAX_KEPT) and put before its next turn that goes to the model. No scores
//!   (an answer with none, a failed request: it fails open): the turn goes as any other. A turn with another event,
//!   and a follow-up to results, is never scored.
use crate::api::{self, Call, Config, Reply, Turn};
use crate::ledger::{Ledger, Tokens};
use kd_common::feed::{AgentDecision, AgentDef, AgentLine};
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
/// a score request takes at most this long (then: no scores, the turn is answered)
pub const SCORE_TIMEOUT: Duration = Duration::from_secs(20);
/// a scored turn waits this long for the game's decision, then it is answered
pub const DECIDE_WAIT: Duration = Duration::from_secs(3);
/// the lines kept from the model (turns the game decided not to answer), at most (the newest): they start the agent's
/// next turn that goes
pub const MAX_KEPT: usize = 12;
/// before the kept lines in a turn, and before the new lines after them
pub const KEPT_HEADER: &str = "[said near you before, which you did not answer:]";
pub const NOW_HEADER: &str = "[now:]";

/// Why a reply is not one (agent_reply's `error`; `why` says more).
pub mod error {
    /// the host's tokens for the month (in or out) would be passed
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
    /// agent_results); `error` (an error:: code) and `why` when it is not one - then text and calls are empty;
    /// `skipped`: the game decided on the turn's scores not to answer these lines (no words, no calls, no error)
    Reply { ids: Vec<i64>, agent: String, text: String, calls: Vec<Call>, error: &'static str, why: String, skipped: bool },
    /// the turn `turn` (the lines `ids`) of agent `agent` scored on its criteria (those the model rated, 0..1, in the
    /// criteria's order): it waits for the game's decision (Agents::decide)
    Scores { agent: String, turn: i64, ids: Vec<i64>, scores: Vec<(String, f64)> },
    /// a streaming agent's words so far, while its reply comes (a few times a second at most; the Reply follows)
    Partial { ids: Vec<i64>, agent: String, text: String },
    /// "ready", "busy" (requests a minute reached: lines wait), "budget" (the month's tokens are used up), "error" (the
    /// last request failed: `why`); used: this month's tokens so far
    Status { state: &'static str, why: String, used: Tokens },
    /// the agents Koetama has now, (id, version), sorted: after each change (and empty for a new session)
    Known(Vec<(String, i64)>),
}

pub type OnEvent = Arc<dyn Fn(Event) + Send + Sync>;

enum Msg {
    Config(Config),
    Feed { sid: i64, agents: Vec<AgentDef>, lines: Vec<AgentLine>, results: Vec<(String, String)>, forget: Vec<String> },
    Done { agent: String, gen: u64, result: Result<Reply, String> },
    /// a score request's answer
    Scored { agent: String, gen: u64, result: Result<Reply, String> },
    /// the game's decisions on scored turns
    Decide(Vec<AgentDecision>),
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
            turns: 0,
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

    /// The game's decisions on scored turns (the feed's agent_decisions; after that feed's feed()): a decision for a
    /// turn that does not wait for one (unknown, already decided or answered) is ignored.
    pub fn decide(&self, decisions: &[AgentDecision]) {
        self.send(Msg::Decide(decisions.to_vec()));
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

/// A request under way: its generation (a reply from an older one is dropped), the lines it answers, the history's
/// length before it (put back if it fails), what the ledger holds for it, and how many kept lines its turn starts with
/// (let go once it is answered).
struct Busy {
    gen: u64,
    ids: Vec<i64>,
    before: usize,
    held: Tokens,
    kept: usize,
}

/// A score request under way: its generation (an answer from an older one is dropped), the turn's lines, what the
/// ledger holds for it.
struct Scoring {
    gen: u64,
    lines: Vec<AgentLine>,
    held: Tokens,
}

/// A scored turn waiting for the game's decision: its number (agent_scores' `turn`), its lines, since when.
struct Deciding {
    turn: i64,
    lines: Vec<AgentLine>,
    since: Instant,
}

/// An agent on the thread.
#[derive(Default)]
struct Agent {
    def: AgentDef,
    history: Vec<Turn>,
    /// lines waiting for the agent to be free
    queue: Vec<AgentLine>,
    /// the request under way
    busy: Option<Busy>,
    /// its score request under way
    scoring: Option<Scoring>,
    /// its scored turn waiting for the game's decision
    deciding: Option<Deciding>,
    /// a scored turn to be answered, waiting for requests a minute (it goes before the queue, not scored again)
    passed: Option<Vec<AgentLine>>,
    /// function calls waiting for the game's results
    waiting: Option<Waiting>,
    /// lines of turns the game decided not to answer (the newest MAX_KEPT, as a turn writes them): its next turn that
    /// goes starts with them
    kept: VecDeque<String>,
}

impl Agent {
    /// Nothing under way: a turn may go.
    fn free(&self) -> bool {
        self.busy.is_none() && self.scoring.is_none() && self.deciding.is_none() && self.waiting.is_none()
    }

    /// A turn is waiting.
    fn has_turn(&self) -> bool {
        self.passed.is_some() || !self.queue.is_empty()
    }
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
    /// the last scored turn's number (agent_scores' `turn`: never used twice while the service runs)
    turns: i64,
    /// the state as last told (Event::Status): state, why, tens of thousands of tokens used
    told: Option<(&'static str, String, u64)>,
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

/// A line as a turn writes it: `Name: words`, an event in [brackets].
fn line_text(l: &AgentLine) -> String {
    let who = if !l.from_name.is_empty() { Some(l.from_name.clone()) } else { l.from_id.as_ref().map(|p| p.text.clone()) };
    match who {
        Some(w) => format!("{w}: {}", l.text),
        None => format!("[{}]", l.text),
    }
}

/// The kept lines under KEPT_HEADER, then NOW_HEADER ("" when none are kept).
fn kept_block(kept: &[String]) -> String {
    if kept.is_empty() {
        return String::new();
    }
    format!("{KEPT_HEADER}\n{}\n{NOW_HEADER}\n", kept.join("\n"))
}

/// The lines as one turn: the kept lines of turns not answered before (if any), the newest game state, then each line.
fn user_turn(lines: &[AgentLine], kept: &[String]) -> String {
    let mut out = kept_block(kept);
    if let Some(s) = lines.iter().rev().map(|l| l.state.as_str()).find(|s| !s.is_empty()) {
        out.push_str(&format!("[the game: {s}]\n"));
    }
    for l in lines {
        out.push_str(&line_text(l));
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// A turn that is scored: the agent has criteria, and the lines are players' or context events only (a turn with any
/// other event - something the game asks it to act on - goes straight to the model).
fn scored(def: &AgentDef, lines: &[AgentLine]) -> bool {
    !def.criteria.is_empty() && lines.iter().all(|l| !l.is_event() || l.context)
}

/// The score request's question, Koetama's note at the end of the reply's own request (prefix sharing - the user,
/// 2026-11-04): what it rates and how to answer. The request before it is exactly the one that answers the turn: the
/// provider has it cached when that one comes.
fn score_question(def: &AgentDef) -> String {
    let list: Vec<String> = def.criteria.iter().map(|c| format!("- {}: {}", c.name, c.description)).collect();
    let line: Vec<String> = def.criteria.iter().map(|c| format!("{}=0.0", c.name)).collect();
    format!(
        "[Koetama asks this, not a player: do not reply to them now, and do not use a function. Rate what was just said \
         or happened near you - the lines just above (after {NOW_HEADER} when there is one); what came before only tells \
         you what is going on - on each of these, from 0 (not at all) to 1 (completely):\n{}\nAnswer with one line and \
         nothing else, every name once: {}]",
        list.join("\n"),
        line.join(" ")
    )
}

/// The score answer's longest: a few tokens a criterion, and some to spare.
fn score_max_tokens(def: &AgentDef) -> u32 {
    (16 + 10 * def.criteria.len() as u32).min(128)
}


/// The scores in an answer, in the criteria's order: each `name=number` (or `name: number`, spaces around either; a
/// name in any case) for one of the criteria, the first of each, clamped to 0..1 - after any thinking a model wrote out
/// (`</think>`). Names it did not give (or gave no number for) are left out; none: an empty list.
fn read_scores(text: &str, criteria: &[kd_common::feed::AgentCriterion]) -> Vec<(String, f64)> {
    let t: Vec<char> = text.rsplit("</think>").next().unwrap_or(text).chars().collect();
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut got: Vec<(String, f64)> = Vec::new();
    let mut i = 0;
    while i < t.len() {
        if !word(t[i]) || (i > 0 && word(t[i - 1])) {
            i += 1;
            continue;
        }
        let start = i;
        while i < t.len() && word(t[i]) {
            i += 1;
        }
        let name: String = t[start..i].iter().collect::<String>().to_lowercase();
        let mut j = i;
        while j < t.len() && t[j].is_whitespace() {
            j += 1;
        }
        if j >= t.len() || (t[j] != '=' && t[j] != ':') {
            continue;
        }
        j += 1;
        while j < t.len() && t[j].is_whitespace() {
            j += 1;
        }
        let from = j;
        while j < t.len() && (t[j].is_ascii_digit() || t[j] == '.' || (j == from && t[j] == '-')) {
            j += 1;
        }
        let number: String = t[from..j].iter().collect();
        let Ok(x) = number.trim_end_matches('.').parse::<f64>() else { continue };
        if x.is_finite() && criteria.iter().any(|c| c.name == name) && !got.iter().any(|(n, _)| *n == name) {
            got.push((name, x.clamp(0.0, 1.0)));
        }
        i = j;
    }
    criteria.iter().filter_map(|c| got.iter().find(|(n, _)| *n == c.name).cloned()).collect()
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
                Ok(Msg::Scored { agent, gen, result }) => self.scored(&agent, gen, result),
                Ok(Msg::Decide(decisions)) => {
                    for d in decisions {
                        self.decided(&d);
                    }
                }
                Ok(Msg::Partial { agent, gen, text }) => {
                    if let Some(b) = self.agents.get(&agent).and_then(|a| a.busy.as_ref()) {
                        if b.gen == gen {
                            (self.on_event)(Event::Partial { ids: b.ids.clone(), agent, text });
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.overdue_results();
            self.overdue_decisions();
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

    /// agent_forget: its conversation (and the lines kept from it) starts over, and what it had under way is cancelled (a
    /// request's or score request's answer dropped; its lines, a scored turn's waiting for the game's decision, the
    /// waiting ones and those whose calls wait for results answered "cancelled").
    fn forget(&mut self, k: &str) {
        let Some(n) = self.agents.get_mut(k) else { return };
        n.history.clear();
        n.kept.clear();
        let mut ids: Vec<i64> = Vec::new();
        if let Some(b) = n.busy.take() {
            ids.extend(b.ids);
            self.ledger.settle(b.held, Tokens::default());
        }
        if let Some(s) = n.scoring.take() {
            ids.extend(s.lines.iter().map(|l| l.id));
            self.ledger.settle(s.held, Tokens::default());
        }
        if let Some(w) = n.deciding.take() {
            ids.extend(w.lines.iter().map(|l| l.id));
        }
        if let Some(p) = n.passed.take() {
            ids.extend(p.iter().map(|l| l.id));
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
        self.ask(k, ids, before, 0);
    }

    /// Every agent that is free and has lines waiting: its lines go as one turn (within the host's limits) - first to
    /// be scored when it is one that is scored; a scored turn the game wants answered goes straight on.
    fn dispatch(&mut self) {
        let free: Vec<String> = self.agents.iter().filter(|(_, n)| n.free() && n.has_turn()).map(|(k, _)| k.clone()).collect();
        for k in free {
            if !self.may_send() {
                return; // (requests a minute reached: they wait)
            }
            let n = self.agents.get_mut(&k).expect("listed");
            if let Some(lines) = n.passed.take() {
                self.turn(&k, lines);
                continue;
            }
            let lines: Vec<AgentLine> = std::mem::take(&mut n.queue);
            if scored(&n.def, &lines) {
                self.score(&k, lines);
            } else {
                self.turn(&k, lines);
            }
        }
    }

    /// The lines go to the model as the agent's next turn (after the kept lines, if any).
    fn turn(&mut self, k: &str, lines: Vec<AgentLine>) {
        let Some(n) = self.agents.get_mut(k) else { return };
        let ids = lines.iter().map(|l| l.id).collect();
        let kept: Vec<String> = n.kept.iter().cloned().collect();
        n.history = trimmed(std::mem::take(&mut n.history));
        let before = n.history.len();
        n.history.push(Turn::User(user_turn(&lines, &kept)));
        self.ask(k, ids, before, kept.len());
    }

    /// Why a request may not go: the month's tokens.
    fn budget_why(&self) -> String {
        let (u, l) = (self.ledger.used(), self.cfg.limit);
        format!("this month's tokens would be passed ({} of {} in, {} of {} out used)", u.input, l.input, u.output, l.output)
    }

    /// Ask for the turn's scores on the agent's criteria: the request that would answer the turn - its system prompt,
    /// functions, conversation and the turn (the kept lines first), exactly - with the question after it (a note), so
    /// the answer's request, when it comes, starts as one the provider has just cached; short, not streamed; a request
    /// a minute and held in the ledger as any other ("budget" when it cannot be). The answer comes as Msg::Scored. The
    /// agent is busy meanwhile (lines that come wait).
    fn score(&mut self, k: &str, lines: Vec<AgentLine>) {
        let Some(n) = self.agents.get_mut(k) else { return };
        let kept: Vec<String> = n.kept.iter().cloned().collect();
        let mut turns = trimmed(n.history.clone());
        turns.push(Turn::User(user_turn(&lines, &kept)));
        turns.push(Turn::Note(score_question(&n.def)));
        let (cfg, sys, functions) = (self.cfg.clone(), system(&n.def), n.def.functions.clone());
        let max_tokens = score_max_tokens(&n.def);
        let prompt: usize = sys.len()
            + functions.iter().map(|f| f.description.len() + f.parameters.to_string().len()).sum::<usize>()
            + turns.iter().map(api::turn_len).sum::<usize>();
        let most = Tokens::new(prompt as u64 / 3, max_tokens as u64);
        if !self.ledger.reserve(most, cfg.limit) {
            let why = self.budget_why();
            self.fail(lines.iter().map(|l| l.id).collect(), k, error::BUDGET, why);
            return;
        }
        self.gen += 1;
        n.scoring = Some(Scoring { gen: self.gen, lines, held: most });
        let (tx, agent, gen) = (self.tx.clone(), k.to_string(), self.gen);
        std::thread::spawn(move || {
            let result = api::complete(&cfg, &sys, &functions, &turns, max_tokens, SCORE_TIMEOUT);
            let _ = tx.send(Msg::Scored { agent, gen, result });
        });
    }

    /// The scores came: they go to the game, and the turn waits for its decision (DECIDE_WAIT at most). None in the
    /// answer, or the request failed (it fails open): the turn goes to the model as any other.
    fn scored(&mut self, k: &str, gen: u64, result: Result<Reply, String>) {
        let Some(n) = self.agents.get_mut(k) else { return };
        let Some(s) = n.scoring.take() else { return };
        if s.gen != gen {
            n.scoring = Some(s);
            return;
        }
        let scores = match &result {
            Ok(r) => {
                self.ledger.settle(s.held, r.tokens());
                let scores = read_scores(&r.text, &n.def.criteria);
                (self.log)(&if scores.is_empty() {
                    format!(
                        "agent {k}: scores unclear ({:?}, {} tokens in, {} out): answered",
                        r.text.chars().take(80).collect::<String>(),
                        r.tokens_in,
                        r.tokens_out
                    )
                } else {
                    let said: Vec<String> = scores.iter().map(|(name, x)| format!("{name}={x}")).collect();
                    format!("agent {k}: scores {} ({} tokens in)", said.join(" "), r.tokens_in)
                });
                scores
            }
            Err(e) => {
                self.ledger.settle(s.held, Tokens::default());
                (self.log)(&format!("agent {k}: scores failed ({e}): answered"));
                Vec::new()
            }
        };
        if scores.is_empty() {
            self.answer(k, s.lines);
            return;
        }
        self.turns += 1;
        let ids = s.lines.iter().map(|l| l.id).collect();
        n.deciding = Some(Deciding { turn: self.turns, lines: s.lines, since: Instant::now() });
        (self.on_event)(Event::Scores { agent: k.to_string(), turn: self.turns, ids, scores });
    }

    /// The game's decision on a scored turn: answered (the normal request) or not (its lines answered `skipped` and
    /// kept for the next turn). One for a turn that does not wait for one is ignored.
    fn decided(&mut self, d: &AgentDecision) {
        let Some(n) = self.agents.get_mut(&d.agent) else { return };
        if !n.deciding.as_ref().is_some_and(|w| w.turn == d.turn) {
            return;
        }
        let Some(w) = n.deciding.take() else { return };
        (self.log)(&format!("agent {}: decision {} (turn {})", d.agent, if d.answer { "answer" } else { "skip" }, d.turn));
        if d.answer {
            self.answer(&d.agent, w.lines);
        } else {
            self.skip(&d.agent, w.lines);
        }
    }

    /// Scored turns the game has not decided on in DECIDE_WAIT are answered.
    fn overdue_decisions(&mut self) {
        let late: Vec<String> = self
            .agents
            .iter()
            .filter(|(_, n)| n.deciding.as_ref().is_some_and(|w| w.since.elapsed() >= DECIDE_WAIT))
            .map(|(k, _)| k.clone())
            .collect();
        for k in late {
            let Some(w) = self.agents.get_mut(&k).and_then(|n| n.deciding.take()) else { continue };
            (self.log)(&format!("agent {k}: no decision in {} s (turn {}): answered", DECIDE_WAIT.as_secs(), w.turn));
            self.answer(&k, w.lines);
        }
    }

    /// A scored turn is answered: the normal request now, or first once requests a minute allow.
    fn answer(&mut self, k: &str, lines: Vec<AgentLine>) {
        if self.may_send() {
            self.turn(k, lines);
        } else if let Some(n) = self.agents.get_mut(k) {
            n.passed = Some(lines); // (requests a minute reached: it goes first once they allow)
        }
    }

    /// A scored turn is not answered: its lines are answered `skipped`, and kept to start the agent's next turn.
    fn skip(&mut self, k: &str, lines: Vec<AgentLine>) {
        let Some(n) = self.agents.get_mut(k) else { return };
        n.kept.extend(lines.iter().map(line_text));
        while n.kept.len() > MAX_KEPT {
            n.kept.pop_front();
        }
        (self.on_event)(Event::Reply {
            ids: lines.iter().map(|l| l.id).collect(),
            agent: k.to_string(),
            text: String::new(),
            calls: Vec::new(),
            error: "",
            why: String::new(),
            skipped: true,
        });
    }

    fn over_budget(&self) -> bool {
        self.ledger.used().reached(self.cfg.limit)
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

    /// Ask the LLM for agent k's next words (its history already has the new turn, which starts with `kept` of the kept
    /// lines), on a thread of its own - if the ledger can hold what it could use at worst (its prompt, at 3 characters a
    /// token - most text is nearer 4 - and its longest reply): else its lines are answered "budget" and the turn taken
    /// back (the kept lines stay for the next).
    fn ask(&mut self, k: &str, ids: Vec<i64>, before: usize, kept: usize) {
        let Some(n) = self.agents.get_mut(k) else { return };
        let (cfg, sys, functions, turns) = (self.cfg.clone(), system(&n.def), n.def.functions.clone(), n.history.clone());
        let max_tokens = (n.def.max_words.max(1) as u32).saturating_mul(2).saturating_add(256).min(4096);
        let prompt: usize = sys.len()
            + functions.iter().map(|f| f.description.len() + f.parameters.to_string().len()).sum::<usize>()
            + turns.iter().map(api::turn_len).sum::<usize>();
        let most = Tokens::new(prompt as u64 / 3, max_tokens as u64);
        if !self.ledger.reserve(most, cfg.limit) {
            n.history.truncate(before);
            let why = self.budget_why();
            self.fail(ids, k, error::BUDGET, why);
            return;
        }
        self.gen += 1;
        n.busy = Some(Busy { gen: self.gen, ids, before, held: most, kept });
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
        let Some(b) = n.busy.take() else { return };
        if b.gen != gen {
            n.busy = Some(b);
            return;
        }
        let Busy { ids, before, held, kept, .. } = b;
        match result {
            Ok(r) => {
                // (the kept lines were in this turn: answered now)
                let kept = kept.min(n.kept.len());
                n.kept.drain(..kept);
                n.history.push(Turn::Assistant { text: r.text.clone(), calls: r.calls.clone() });
                if !r.calls.is_empty() {
                    n.waiting = Some((ids.clone(), r.calls.iter().map(|c| c.id.clone()).collect(), Vec::new(), Instant::now()));
                }
                self.ledger.settle(held, r.tokens());
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
                self.ledger.settle(held, Tokens::default());
                (self.log)(&format!("agent {k}: {e}"));
                self.failed = Some(e.clone());
                self.fail(ids, k, error::FAILED, e);
            }
        }
    }

    fn reply(&self, ids: Vec<i64>, agent: &str, text: String, calls: Vec<Call>) {
        (self.on_event)(Event::Reply { ids, agent: agent.to_string(), text, calls, error: "", why: String::new(), skipped: false });
    }

    fn fail(&self, ids: Vec<i64>, agent: &str, code: &'static str, why: String) {
        (self.on_event)(Event::Reply {
            ids,
            agent: agent.to_string(),
            text: String::new(),
            calls: Vec::new(),
            error: code,
            why,
            skipped: false,
        });
    }

    /// The status when it changed, and the agents it has when they changed.
    fn tell(&mut self) {
        let mut known: Vec<(String, i64)> = self.agents.iter().map(|(k, a)| (k.clone(), a.def.version)).collect();
        known.sort();
        if self.known.as_ref() != Some(&known) {
            (self.on_event)(Event::Known(known.clone()));
            self.known = Some(known);
        }
        let now = if self.over_budget() {
            ("budget", String::new())
        } else if let Some(e) = &self.failed {
            ("error", e.clone())
        } else if self.agents.values().any(|n| n.free() && n.has_turn()) {
            ("busy", String::new())
        } else {
            ("ready", String::new())
        };
        // (again when the state changes, or every 10,000 tokens used: the game shows it)
        let used = self.ledger.used();
        let now = (now.0, now.1, (used.input + used.output) / 10_000);
        if self.told.as_ref() != Some(&now) {
            (self.on_event)(Event::Status { state: now.0, why: now.1.clone(), used });
            self.told = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kd_common::feed::{AgentCriterion, PlayerId};

    fn line(id: i64, from: Option<&str>, text: &str, state: &str) -> AgentLine {
        AgentLine {
            id,
            agent: "pig".into(),
            text: text.into(),
            from_id: from.map(PlayerId::string),
            from_name: String::new(),
            state: state.into(),
            context: false,
        }
    }

    #[test]
    fn lines_make_one_turn() {
        let t = user_turn(
            &[
                line(1, Some("Wendy"), "10 meat for a helmet?", "winter"),
                line(2, None, "the pig king burped", ""),
                line(3, Some("Wes"), "hi", "winter, day 31"),
            ],
            &[],
        );
        assert_eq!(t, "[the game: winter, day 31]\nWendy: 10 meat for a helmet?\n[the pig king burped]\nWes: hi");
        let d = AgentDef { persona: "You are the Pig King.".into(), max_words: 40, character: true, ..Default::default() };
        assert!(system(&d).ends_with("You are the Pig King.") && system(&d).contains("at most 40 words"));
        let director = AgentDef { persona: "Plan the next wave.".into(), character: false, ..d };
        assert_eq!(system(&director), "Plan the next wave.", "not a character: the persona alone");
    }

    fn criteria() -> Vec<AgentCriterion> {
        let c = |name: &str, description: &str| AgentCriterion { name: name.into(), description: description.into() };
        vec![c("topic", "Mochi is the topic"), c("interest", "treats, the ball, play"), c("wellbeing", "his or his owner's safety")]
    }

    #[test]
    fn which_turns_are_scored() {
        let dog = AgentDef { persona: "You are Mochi, a corgi.".into(), criteria: criteria(), ..Default::default() };
        let petted = AgentLine { context: true, ..line(2, None, "Alex petted you", "") };
        let found = line(3, None, "you found Sam: tell them the message", "");
        assert!(scored(&dog, &[line(1, Some("Alex"), "hi", "")]), "a player's line");
        assert!(scored(&dog, &[line(1, Some("Alex"), "hi", ""), petted.clone()]), "and a context event");
        assert!(scored(&dog, std::slice::from_ref(&petted)), "a context event alone");
        assert!(!scored(&dog, &[line(1, Some("Alex"), "hi", ""), found.clone()]), "an event the game asks it to act on: never");
        let plain = AgentDef { criteria: vec![], ..dog.clone() };
        assert!(!scored(&plain, &[line(1, Some("Alex"), "hi", "")]), "no criteria: never");
        // (the question: what to rate and how to answer - after the reply's own request)
        let q = score_question(&dog);
        assert!(q.starts_with("[Koetama asks this, not a player: do not reply to them now, and do not use a function."), "{q}");
        assert!(q.contains(":\n- topic: Mochi is the topic\n- interest: treats, the ball, play\n- wellbeing: his or his owner's safety\n"));
        assert!(q.ends_with("every name once: topic=0.0 interest=0.0 wellbeing=0.0]"), "{q}");
        assert_eq!(score_max_tokens(&dog), 46);
        let many = AgentDef { criteria: (0..8).map(|i| AgentCriterion { name: format!("c{i}"), description: "x".into() }).collect(), ..dog };
        assert_eq!(score_max_tokens(&many), 96);
    }

    #[test]
    fn scores_are_read() {
        let s = |pairs: &[(&str, f64)]| pairs.iter().map(|(n, x)| (n.to_string(), *x)).collect::<Vec<_>>();
        for (t, want) in [
            ("topic=0.9 interest=0.1 wellbeing=0.0", s(&[("topic", 0.9), ("interest", 0.1), ("wellbeing", 0.0)])),
            ("wellbeing = 1  topic: 0.25", s(&[("topic", 0.25), ("wellbeing", 1.0)])),
            ("Sure! Topic=0.8, interest=0.3.", s(&[("topic", 0.8), ("interest", 0.3)])),
            ("<think>topic=0.1 is too low</think>\ntopic=0.7 interest=2 wellbeing=-1", s(&[("topic", 0.7), ("interest", 1.0), ("wellbeing", 0.0)])),
            ("topic=0.4 topic=0.9 mood=0.5", s(&[("topic", 0.4)])),
            ("subtopic=0.9", vec![]),
            ("topic=high interest=", vec![]),
            ("", vec![]),
            ("I cannot rate that.", vec![]),
        ] {
            assert_eq!(read_scores(t, &criteria()), want, "{t:?}");
        }
    }

    #[test]
    fn kept_lines_start_the_next_turn() {
        let kept = vec!["Alex: nice weather".to_string(), "[Sam waved]".to_string()];
        let t = user_turn(&[line(4, Some("Alex"), "Mochi, ball!", "the yard")], &kept);
        assert_eq!(
            t,
            "[said near you before, which you did not answer:]\nAlex: nice weather\n[Sam waved]\n[now:]\n[the game: the yard]\nAlex: Mochi, ball!"
        );
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
