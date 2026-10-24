//! The NPC service: the game's NPC definitions and lines come in (feed()), the replies go out (Event), on a thread of
//! its own; each request to the LLM on a thread of its own too.
//!
//! - Each NPC keeps its conversation: the last MAX_TURNS turns (a fixed window - no summaries), its first a player's.
//! - Its queue: a line that comes while the NPC's request runs (or while its function calls wait for the game's
//!   results) waits; once it is free, every waiting line goes in ONE turn, and the reply answers them all (`ids`).
//! - Function calls: the reply's calls go to the game (Event::Reply with `calls`); the game runs them, checks them
//!   against its rules and answers each (feed's npc_results); with all of them answered (or after RESULTS_WAIT
//!   without), they go back to the LLM and its follow-up is another reply to the same lines.
//! - The host's limits: requests a minute (more wait in the queue: "busy"), the month's budget (spent: the lines get
//!   an error reply: "budget"). Each reply's cost is told (Event::Spent) for the host's setting to keep.
//! - A new game session (`sid`) starts every NPC over; `npc_forget` starts one over.
use crate::api::{self, Call, Config, Reply, Turn};
use kd_common::feed::{NpcDef, NpcLine};
use kd_common::Log;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// an NPC's conversation keeps this many turns (a player's line and the reply are two)
pub const MAX_TURNS: usize = 24;
/// function calls wait this long for the game's results, then go back "(no result)"
pub const RESULTS_WAIT: Duration = Duration::from_secs(20);
/// one request to the LLM takes at most this long
pub const TIMEOUT: Duration = Duration::from_secs(60);
/// the line ids remembered in a session (each answered once), the newest
const SEEN_MAX: usize = 4096;

/// What the service tells the program (for the game, and the host's setting).
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// the reply to lines `ids` of NPC `npc`: its words and function calls (the game runs them and answers with
    /// npc_results); `error` when it could not be asked ("budget", or what failed) - then text and calls are empty
    Reply { ids: Vec<i64>, npc: String, text: String, calls: Vec<Call>, error: String },
    /// "ready", "busy" (requests a minute reached: lines wait), "budget" (the month's budget is spent), "error" (the
    /// last request failed: `why`); spent: this month so far
    Status { state: &'static str, why: String, spent: f64 },
    /// what was spent this month so far, after each reply (the host's setting keeps it)
    Spent(f64),
}

pub type OnEvent = Arc<dyn Fn(Event) + Send + Sync>;

enum Msg {
    Config(Config),
    Feed { sid: i64, npcs: Vec<NpcDef>, lines: Vec<NpcLine>, results: Vec<(String, String)>, forget: Vec<String> },
    Done { npc: String, gen: u64, result: Result<Reply, String> },
    Stop,
}

/// The service's handle (clones share it): its thread runs until stop().
#[derive(Clone)]
pub struct Npcs {
    tx: Arc<Mutex<Sender<Msg>>>,
    thread: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl Npcs {
    pub fn start(cfg: Config, on_event: OnEvent, log: Log) -> Npcs {
        let (tx, rx) = mpsc::channel();
        let mut w = Worker {
            cfg,
            on_event,
            log,
            tx: tx.clone(),
            sid: None,
            npcs: HashMap::new(),
            seen: HashSet::new(),
            seen_order: VecDeque::new(),
            sent: VecDeque::new(),
            gen: 0,
            told: None,
            failed: None,
        };
        let thread = std::thread::Builder::new()
            .name("koetama-npcs".into())
            .spawn(move || w.run(rx))
            .expect("the NPC thread");
        Npcs { tx: Arc::new(Mutex::new(tx)), thread: Arc::new(Mutex::new(Some(thread))) }
    }

    fn send(&self, m: Msg) {
        let _ = self.tx.lock().unwrap_or_else(|e| e.into_inner()).send(m);
    }

    /// The host changed the setting (model, key, limits): what comes next uses it.
    pub fn set_config(&self, cfg: Config) {
        self.send(Msg::Config(cfg));
    }

    /// What a feed says for NPCs (cheap: call it with every feed; each line is answered once).
    pub fn feed(&self, sid: i64, npcs: &[NpcDef], lines: &[NpcLine], results: &[(String, String)], forget: &[String]) {
        self.send(Msg::Feed { sid, npcs: npcs.to_vec(), lines: lines.to_vec(), results: results.to_vec(), forget: forget.to_vec() });
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

/// An NPC on the thread.
#[derive(Default)]
struct Npc {
    def: NpcDef,
    history: Vec<Turn>,
    /// lines waiting for the NPC to be free
    queue: Vec<NpcLine>,
    /// the request under way: its generation (a reply from an older one is dropped), the lines it answers, and the
    /// history's length before it (put back if it fails)
    busy: Option<(u64, Vec<i64>, usize)>,
    /// function calls waiting for the game's results
    waiting: Option<Waiting>,
}

struct Worker {
    cfg: Config,
    on_event: OnEvent,
    log: Log,
    tx: Sender<Msg>,
    sid: Option<i64>,
    npcs: HashMap<String, Npc>,
    seen: HashSet<i64>,
    seen_order: VecDeque<i64>,
    /// when the last requests were sent (requests a minute)
    sent: VecDeque<Instant>,
    gen: u64,
    told: Option<(&'static str, String)>,
    /// the last request failed (and why), until one succeeds
    failed: Option<String>,
}

/// Koetama's words before every persona: how the lines come and what a reply is (the mod's persona says who).
fn system(def: &NpcDef) -> String {
    format!(
        "You are a character in a multiplayer game, in its world. Each message lists what was said near you, one line \
         each as `Name: words`; a line in [brackets] is something that happened, and [the game: ...] is what the game \
         says is going on. Reply only with what you say aloud, in at most {} words, in the language you are spoken to. \
         Use your functions to act; the game decides whether an action happens.\n\n{}",
        def.max_words, def.persona
    )
}

/// The lines as one turn: the newest game state first, then each line.
fn user_turn(lines: &[NpcLine]) -> String {
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
                    // (what was spent is the service's to count from here on: the setting may come with an older figure)
                    let spent = self.cfg.spent.max(c.spent);
                    self.cfg = Config { spent, ..c };
                }
                Ok(Msg::Feed { sid, npcs, lines, results, forget }) => self.feed(sid, npcs, lines, results, forget),
                Ok(Msg::Done { npc, gen, result }) => self.done(&npc, gen, result),
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.overdue_results();
            self.dispatch();
            self.tell();
        }
    }

    fn feed(&mut self, sid: i64, npcs: Vec<NpcDef>, lines: Vec<NpcLine>, results: Vec<(String, String)>, forget: Vec<String>) {
        if self.sid != Some(sid) {
            // (a new game session: every NPC starts over; a request under way is dropped when it comes back)
            self.sid = Some(sid);
            self.npcs.clear();
            self.seen.clear();
            self.seen_order.clear();
        }
        for d in npcs {
            let n = self.npcs.entry(d.id.clone()).or_default();
            n.def = d;
        }
        for id in forget {
            if let Some(n) = self.npcs.get_mut(&id) {
                n.history.clear();
            }
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
            match self.npcs.get_mut(&l.npc) {
                Some(n) => n.queue.push(l),
                None => self.reply(vec![l.id], &l.npc, String::new(), Vec::new(), format!("no NPC \"{}\" (send it in npcs first)", l.npc)),
            }
        }
        for (call, result) in results {
            for n in self.npcs.values_mut() {
                if let Some((_, calls, got, _)) = n.waiting.as_mut() {
                    if calls.contains(&call) && !got.iter().any(|(c, _)| *c == call) {
                        got.push((call.clone(), result.clone()));
                    }
                }
            }
        }
        // (every call answered: the results go back to the LLM)
        let ready: Vec<String> = self
            .npcs
            .iter()
            .filter(|(_, n)| n.waiting.as_ref().is_some_and(|(_, calls, got, _)| got.len() >= calls.len()))
            .map(|(k, _)| k.clone())
            .collect();
        for k in ready {
            self.results_back(&k);
        }
    }

    /// Calls the game has not answered in RESULTS_WAIT go back "(no result)".
    fn overdue_results(&mut self) {
        let late: Vec<String> = self
            .npcs
            .iter()
            .filter(|(_, n)| n.waiting.as_ref().is_some_and(|(.., since)| since.elapsed() >= RESULTS_WAIT))
            .map(|(k, _)| k.clone())
            .collect();
        for k in late {
            if let Some((_, calls, got, _)) = self.npcs.get_mut(&k).and_then(|n| n.waiting.as_mut()) {
                for c in calls.iter() {
                    if !got.iter().any(|(g, _)| g == c) {
                        got.push((c.clone(), "(no result)".into()));
                    }
                }
            }
            self.results_back(&k);
        }
    }

    /// The NPC's calls are all answered: their results join its conversation and its follow-up is asked for.
    fn results_back(&mut self, k: &str) {
        let Some(n) = self.npcs.get_mut(k) else { return };
        let Some((ids, calls, got, _)) = n.waiting.take() else { return };
        // (in the order the LLM made the calls)
        let ordered: Vec<(String, String)> =
            calls.iter().filter_map(|c| got.iter().find(|(g, _)| g == c).cloned()).collect();
        let before = n.history.len();
        n.history.push(Turn::Results(ordered));
        self.ask(k, ids, before);
    }

    /// Every NPC that is free and has lines waiting: its lines go as one turn (within the host's limits).
    fn dispatch(&mut self) {
        let free: Vec<String> = self
            .npcs
            .iter()
            .filter(|(_, n)| n.busy.is_none() && n.waiting.is_none() && !n.queue.is_empty())
            .map(|(k, _)| k.clone())
            .collect();
        for k in free {
            if self.over_budget() {
                let n = self.npcs.get_mut(&k).expect("listed");
                let ids: Vec<i64> = n.queue.drain(..).map(|l| l.id).collect();
                self.reply(ids, &k, String::new(), Vec::new(), "budget".into());
                continue;
            }
            if !self.may_send() {
                return; // (requests a minute reached: they wait)
            }
            let n = self.npcs.get_mut(&k).expect("listed");
            let lines: Vec<NpcLine> = std::mem::take(&mut n.queue);
            let ids = lines.iter().map(|l| l.id).collect();
            n.history = trimmed(std::mem::take(&mut n.history));
            let before = n.history.len();
            n.history.push(Turn::User(user_turn(&lines)));
            self.ask(&k, ids, before);
        }
    }

    fn over_budget(&self) -> bool {
        self.cfg.spent >= self.cfg.budget
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

    /// Ask the LLM for NPC k's next words (its history already has the new turn), on a thread of its own.
    fn ask(&mut self, k: &str, ids: Vec<i64>, before: usize) {
        let Some(n) = self.npcs.get_mut(k) else { return };
        self.gen += 1;
        n.busy = Some((self.gen, ids, before));
        let (cfg, sys, functions, turns) = (self.cfg.clone(), system(&n.def), n.def.functions.clone(), n.history.clone());
        let max_tokens = (n.def.max_words.max(1) as u32).saturating_mul(2).saturating_add(256).min(4096);
        let (tx, npc, gen) = (self.tx.clone(), k.to_string(), self.gen);
        std::thread::spawn(move || {
            let result = api::complete(&cfg, &sys, &functions, &turns, max_tokens, TIMEOUT);
            let _ = tx.send(Msg::Done { npc, gen, result });
        });
    }

    fn done(&mut self, k: &str, gen: u64, result: Result<Reply, String>) {
        let Some(n) = self.npcs.get_mut(k) else { return };
        let Some((g, ids, before)) = n.busy.take() else { return };
        if g != gen {
            n.busy = Some((g, ids, before));
            return;
        }
        match result {
            Ok(r) => {
                n.history.push(Turn::Assistant { text: r.text.clone(), calls: r.calls.clone() });
                if !r.calls.is_empty() {
                    n.waiting = Some((ids.clone(), r.calls.iter().map(|c| c.id.clone()).collect(), Vec::new(), Instant::now()));
                }
                self.cfg.spent += self.cfg.cost(&r);
                self.failed = None;
                (self.log)(&format!(
                    "NPC {k}: replied ({} tokens in, {} out{})",
                    r.tokens_in,
                    r.tokens_out,
                    if r.calls.is_empty() { String::new() } else { format!(", {} function calls", r.calls.len()) }
                ));
                (self.on_event)(Event::Spent(self.cfg.spent));
                self.reply(ids, k, r.text, r.calls, String::new());
            }
            Err(e) => {
                n.history.truncate(before);
                (self.log)(&format!("NPC {k}: {e}"));
                self.failed = Some(e.clone());
                self.reply(ids, k, String::new(), Vec::new(), e);
            }
        }
    }

    fn reply(&self, ids: Vec<i64>, npc: &str, text: String, calls: Vec<Call>, error: String) {
        (self.on_event)(Event::Reply { ids, npc: npc.to_string(), text, calls, error });
    }

    /// The status when it changed.
    fn tell(&mut self) {
        let waiting = self.npcs.values().any(|n| !n.queue.is_empty());
        let now = if self.over_budget() {
            ("budget", String::new())
        } else if let Some(e) = &self.failed {
            ("error", e.clone())
        } else if waiting && self.npcs.values().any(|n| n.busy.is_none() && n.waiting.is_none() && !n.queue.is_empty()) {
            ("busy", String::new())
        } else {
            ("ready", String::new())
        };
        if self.told.as_ref() != Some(&now) {
            (self.on_event)(Event::Status { state: now.0, why: now.1.clone(), spent: self.cfg.spent });
            self.told = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kd_common::feed::PlayerId;

    fn line(id: i64, from: Option<&str>, text: &str, state: &str) -> NpcLine {
        NpcLine {
            id,
            npc: "pig".into(),
            text: text.into(),
            from_id: from.map(PlayerId::string),
            from_name: String::new(),
            state: state.into(),
            heard_by: Vec::new(),
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
        let d = NpcDef { persona: "You are the Pig King.".into(), max_words: 40, ..Default::default() };
        assert!(system(&d).ends_with("You are the Pig King.") && system(&d).contains("at most 40 words"));
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
