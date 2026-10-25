//! The agent service against a fake LLM (both API styles, on this PC): lines that come while an agent's request runs wait
//! and go as one turn, function calls go to the game and their results back to the LLM, the host's budget and
//! requests a minute, a failed request, a new session, an agent nobody defined.
use kd_common::feed::{AgentDef, AgentFunction, AgentLine, PlayerId};
use kd_llm::{Agents, Api, Config, Event, Ledger};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

type Answer = Box<dyn Fn(usize, &Value) -> (u16, Value, Duration) + Send + Sync>;

/// A fake LLM: each request's headers and body kept; `answer(n, body)` says the status, the JSON and how long to take.
struct Fake {
    url: String,
    seen: Arc<Mutex<Vec<(String, Value)>>>,
}

fn fake(answer: Answer) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let seen: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let (s, answer) = (seen.clone(), Arc::new(answer));
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let (s, answer) = (s.clone(), answer.clone());
            std::thread::spawn(move || {
                let mut r = BufReader::new(conn.try_clone().unwrap());
                let (mut head, mut len) = (String::new(), 0usize);
                loop {
                    let mut l = String::new();
                    if r.read_line(&mut l).unwrap_or(0) == 0 {
                        return;
                    }
                    if l == "\r\n" {
                        break;
                    }
                    if let Some(v) = l.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                    head.push_str(&l);
                }
                let mut b = vec![0u8; len];
                r.read_exact(&mut b).unwrap();
                let body: Value = serde_json::from_slice(&b).unwrap();
                let n = {
                    let mut g = s.lock().unwrap();
                    g.push((head, body.clone()));
                    g.len() - 1
                };
                let (status, out, wait) = answer(n, &body);
                std::thread::sleep(wait);
                let mut c = conn;
                if let Value::String(events) = &out {
                    // (a stream: its events one by one, 200 ms apart, as a model writes)
                    let _ = write!(c, "HTTP/1.1 {status} X\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n");
                    for e in events.split("\n\n").filter(|e| !e.is_empty()) {
                        let _ = write!(c, "{e}\n\n");
                        let _ = c.flush();
                        std::thread::sleep(Duration::from_millis(200));
                    }
                    return;
                }
                let out = out.to_string();
                let _ = write!(
                    c,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                    out.len()
                );
            });
        }
    });
    Fake { url, seen }
}

fn openai_text(text: &str) -> Value {
    json!({"choices": [{"message": {"content": text}}], "usage": {"prompt_tokens": 100, "completion_tokens": 20}})
}

struct Rig {
    agents: Agents,
    events: Arc<(Mutex<Vec<Event>>, Condvar)>,
    ledger: Arc<Ledger>,
}

fn rig(cfg: Config) -> Rig {
    let events: Arc<(Mutex<Vec<Event>>, Condvar)> = Arc::default();
    let ev = events.clone();
    let ledger = Ledger::new(0.0);
    let agents = Agents::start(
        cfg,
        ledger.clone(),
        Arc::new(move |e| {
            ev.0.lock().unwrap().push(e);
            ev.1.notify_all();
        }),
        kd_common::null_log(),
    );
    Rig { agents, events, ledger }
}

fn config(api: Api, url: &str) -> Config {
    Config { api, url: url.into(), model: "m".into(), key: "sk-test".into(), budget: 5.0, price_in: 1.0, price_out: 5.0, per_minute: 60 }
}

fn pig(functions: Vec<AgentFunction>) -> AgentDef {
    AgentDef { id: "pig".into(), version: 1, character: true, stream: false, persona: "You are the Pig King.".into(), functions, max_words: 40, voice: String::new() }
}

fn line(id: i64, agent: &str, who: &str, text: &str) -> AgentLine {
    AgentLine { id, agent: agent.into(), text: text.into(), from_id: Some(PlayerId::string(who)), from_name: who.into(), ..Default::default() }
}

impl Rig {
    fn wait(&self, what: &str, cond: impl Fn(&[Event]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut ev = self.events.0.lock().unwrap();
        while !cond(&ev) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for {what}: {ev:#?}");
            ev = self.events.1.wait_timeout(ev, left).unwrap().0;
        }
    }

    fn replies(&self) -> Vec<(Vec<i64>, String, usize, String)> {
        self.events
            .0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                Event::Reply { ids, text, calls, error, why, .. } => {
                    Some((ids.clone(), text.clone(), calls.len(), if error.is_empty() { String::new() } else { format!("{error}: {why}") }))
                }
                _ => None,
            })
            .collect()
    }

    fn wait_replies(&self, n: usize) -> Vec<(Vec<i64>, String, usize, String)> {
        self.wait(&format!("{n} replies"), |ev| ev.iter().filter(|e| matches!(e, Event::Reply { .. })).count() >= n);
        self.replies()
    }

    fn last_status(&self) -> Option<&'static str> {
        self.events.0.lock().unwrap().iter().rev().find_map(|e| match e {
            Event::Status { state, .. } => Some(*state),
            _ => None,
        })
    }
}

fn user_text(body: &Value) -> String {
    body["messages"].as_array().unwrap().iter().rev().find(|m| m["role"] == "user").unwrap()["content"].as_str().unwrap_or("").to_string()
}

#[test]
fn lines_that_come_while_it_answers_go_as_one_turn() {
    let f = fake(Box::new(|n, _| (200, openai_text(if n == 0 { "Hello, Wendy." } else { "Hello, you two." }), Duration::from_millis(if n == 0 { 500 } else { 0 }))));
    let r = rig(config(Api::OpenAi, &f.url));
    r.agents.feed(1, &[pig(vec![])], &[line(1, "pig", "Wendy", "hello pig")], &[], &[]);
    std::thread::sleep(Duration::from_millis(150));
    // (the game sends its lines again in every feed until they are answered: each is asked once)
    r.agents.feed(1, &[], &[line(1, "pig", "Wendy", "hello pig"), line(2, "pig", "Wes", "hi"), line(3, "pig", "Wilson", "hey")], &[], &[]);
    let got = r.wait_replies(2);
    assert_eq!(got[0], (vec![1], "Hello, Wendy.".into(), 0, String::new()));
    assert_eq!(got[1], (vec![2, 3], "Hello, you two.".into(), 0, String::new()), "the waiting lines answered together");
    let seen = f.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(user_text(&seen[1].1), "Wes: hi\nWilson: hey");
    assert!(seen[1].1["messages"][0]["content"].as_str().unwrap().ends_with("You are the Pig King."));
    assert!(seen[0].0.contains("authorization: Bearer sk-test") || seen[0].0.to_ascii_lowercase().contains("authorization: bearer sk-test"));
    assert_eq!(seen[1].1["messages"].as_array().unwrap().len(), 4, "system, the first turn, its reply, the new turn");
    drop(seen);
    let spent = r.ledger.spent();
    assert!((spent - 2.0 * (100.0 * 1.0 + 20.0 * 5.0) / 1e6).abs() < 1e-12, "spent {spent}");
    r.agents.stop();
}

#[test]
fn function_calls_go_to_the_game_and_their_results_back() {
    let f = fake(Box::new(|n, _| {
        let v = if n == 0 {
            json!({"choices": [{"message": {"content": "Twelve meat.", "tool_calls": [{"id": "call_1", "type": "function",
                "function": {"name": "offer_trade", "arguments": "{\"item\":\"footballhat\",\"price\":12}"}}]}}]})
        } else {
            openai_text("Pleasure doing business.")
        };
        (200, v, Duration::ZERO)
    }));
    let r = rig(config(Api::OpenAi, &f.url));
    let trade = AgentFunction { name: "offer_trade".into(), description: "Offer a trade".into(), parameters: json!({"type": "object"}) };
    r.agents.feed(1, &[pig(vec![trade])], &[line(41, "pig", "Wendy", "10 meat for a helmet?")], &[], &[]);
    let got = r.wait_replies(1);
    assert_eq!(got[0], (vec![41], "Twelve meat.".into(), 1, String::new()));
    match &r.events.0.lock().unwrap().iter().find(|e| matches!(e, Event::Reply { .. })).unwrap() {
        Event::Reply { calls, .. } => assert_eq!((calls[0].id.as_str(), calls[0].args["price"].as_i64()), ("call_1", Some(12))),
        _ => unreachable!(),
    }
    // (a line while the call waits for the game: it waits too)
    r.agents.feed(1, &[], &[line(42, "pig", "Wes", "me too")], &[], &[]);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(f.seen.lock().unwrap().len(), 1, "nothing asked while a call waits for its result");
    r.agents.feed(1, &[], &[], &[("call_1".into(), "ok: offered 1 football helmet for 12 meat".into())], &[]);
    let got = r.wait_replies(3);
    assert_eq!(got[1], (vec![41], "Pleasure doing business.".into(), 0, String::new()), "the follow-up answers the same line");
    assert_eq!(got[2].0, vec![42], "then the line that waited");
    let seen = f.seen.lock().unwrap();
    let m = seen[1].1["messages"].as_array().unwrap();
    assert_eq!(m[3], json!({"role": "tool", "tool_call_id": "call_1", "content": "ok: offered 1 football helmet for 12 meat"}));
    assert_eq!(seen[1].1["tools"][0]["function"]["name"], "offer_trade");
    drop(seen);
    r.agents.stop();
}

#[test]
fn the_budget_and_requests_a_minute() {
    let f = fake(Box::new(|_, _| (200, openai_text("ok"), Duration::ZERO)));
    // (a request holds what it could cost at worst - its prompt and 336 tokens out: ~1,800 millionths of a dollar -
    //  and one reply costs 100 * 1 + 20 * 5 = 200: with 1,950 the first goes, the second would pass the budget)
    let r = rig(Config { budget: 0.001950, ..config(Api::OpenAi, &f.url) });
    r.agents.feed(1, &[pig(vec![])], &[line(1, "pig", "Wendy", "a")], &[], &[]);
    r.wait_replies(1);
    r.agents.feed(1, &[], &[line(2, "pig", "Wendy", "b")], &[], &[]);
    let got = r.wait_replies(2);
    assert!(got[1].0 == vec![2] && got[1].3.starts_with("budget"), "{:?}", got[1]);
    assert_eq!(f.seen.lock().unwrap().len(), 1);
    r.agents.stop();
    // (one request a minute: a second agent's line waits - "busy")
    let r = rig(Config { per_minute: 1, ..config(Api::OpenAi, &f.url) });
    let mut cow = pig(vec![]);
    cow.id = "cow".into();
    r.agents.feed(2, &[pig(vec![]), cow], &[line(1, "pig", "Wendy", "a"), line(2, "cow", "Wendy", "b")], &[], &[]);
    r.wait_replies(1);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(r.replies().len(), 1, "the second waits for the minute");
    assert_eq!(r.last_status(), Some("busy"));
    r.agents.stop();
}

#[test]
fn a_failed_request_a_new_session_and_an_unknown_agent() {
    let f = fake(Box::new(|n, _| {
        if n == 0 {
            (401, json!({"error": {"message": "Incorrect API key provided", "type": "invalid_request_error"}}), Duration::ZERO)
        } else {
            (200, openai_text("better now"), Duration::ZERO)
        }
    }));
    let r = rig(config(Api::OpenAi, &f.url));
    r.agents.feed(1, &[pig(vec![])], &[line(1, "pig", "Wendy", "a"), line(2, "nobody", "Wendy", "b")], &[], &[]);
    let got = r.wait_replies(2);
    assert!(got.iter().any(|g| g.0 == vec![2] && g.3.starts_with("unknown_agent")), "{got:?}");
    assert!(got.iter().any(|g| g.0 == vec![1] && g.3.starts_with("failed") && g.3.contains("401") && g.3.contains("Incorrect API key")), "{got:?}");
    r.wait("the error status", |ev| ev.iter().any(|e| matches!(e, Event::Status { state: "error", .. })));
    // (another model: the old one's failure is not shown as its own - the user, 2026-10-24: it "didn't get cleared")
    r.agents.set_config(Config { model: "other".into(), ..config(Api::OpenAi, &f.url) });
    r.wait("ready again", |ev| matches!(ev.last(), Some(Event::Status { state: "ready", .. })));
    r.agents.feed(1, &[], &[line(3, "pig", "Wendy", "c")], &[], &[]);
    r.wait_replies(3);
    assert_eq!(f.seen.lock().unwrap()[1].1["messages"].as_array().unwrap().len(), 2, "the failed turn was left out");
    // (a new session: the agent starts over - and must be sent again)
    r.agents.feed(2, &[pig(vec![])], &[line(1, "pig", "Wendy", "again")], &[], &[]);
    r.wait_replies(4);
    assert_eq!(f.seen.lock().unwrap()[2].1["messages"].as_array().unwrap().len(), 2, "a new session's agent remembers nothing");
    assert_eq!(r.last_status(), Some("ready"));
    r.agents.stop();
}

#[test]
fn anthropic_style() {
    let f = fake(Box::new(|_, _| {
        (200, json!({"content": [{"type": "text", "text": "Oink."}], "usage": {"input_tokens": 50, "output_tokens": 5}}), Duration::ZERO)
    }));
    let r = rig(config(Api::Anthropic, &f.url));
    r.agents.feed(1, &[pig(vec![])], &[line(1, "pig", "Wendy", "hello")], &[], &[]);
    let got = r.wait_replies(1);
    assert_eq!(got[0].1, "Oink.");
    let seen = f.seen.lock().unwrap();
    let head = seen[0].0.to_ascii_lowercase();
    assert!(head.starts_with("post /v1/messages") && head.contains("x-api-key: sk-test") && head.contains("anthropic-version"));
    assert_eq!(seen[0].1["messages"][0], json!({"role": "user", "content": "Wendy: hello"}));
    drop(seen);
    r.agents.stop();
}

#[test]
fn a_reasoning_model_is_asked_again_without_reasoning() {
    // (OpenAI's reasoning models refuse functions on chat completions unless reasoning_effort is "none")
    let f = fake(Box::new(|_, body| {
        if body.get("reasoning_effort").and_then(Value::as_str) == Some("none") {
            (200, openai_text("Oink."), Duration::ZERO)
        } else {
            (400, json!({"error": {"message": "Function tools with reasoning_effort are not supported for this model in /v1/chat/completions. To use function tools, use /v1/responses or set reasoning_effort to 'none'."}}), Duration::ZERO)
        }
    }));
    let r = rig(config(Api::OpenAi, &f.url));
    r.agents.feed(1, &[pig(vec![])], &[line(1, "pig", "Wendy", "hello")], &[], &[]);
    assert_eq!(r.wait_replies(1)[0], (vec![1], "Oink.".into(), 0, String::new()));
    assert_eq!(f.seen.lock().unwrap().len(), 2, "asked once more, with reasoning off");
    r.agents.stop();
}

#[test]
fn a_streaming_agent_says_its_words_as_they_come() {
    // (OpenAI-style events: three pieces of text, a function call in two pieces, the tokens counted)
    let f = fake(Box::new(|_, body| {
        assert_eq!(body["stream"], true, "asked to stream");
        let ev = |d: Value| format!("data: {d}\n\n");
        let mut s = String::new();
        for t in ["Twelve ", "meat, ", "not ten."] {
            s += &ev(json!({"choices": [{"delta": {"content": t}}]}));
        }
        s += &ev(json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c1", "function": {"name": "offer_trade", "arguments": "{\"price\":"}}]}}]}));
        s += &ev(json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "12}"}}]}}]}));
        s += &ev(json!({"choices": [], "usage": {"prompt_tokens": 80, "completion_tokens": 12}}));
        s += "data: [DONE]\n\n";
        (200, Value::String(s), Duration::ZERO)
    }));
    let r = rig(config(Api::OpenAi, &f.url));
    let mut talker = pig(vec![]);
    talker.stream = true;
    r.agents.feed(1, &[talker], &[line(1, "pig", "Wendy", "10 meat?")], &[], &[]);
    let got = r.wait_replies(1);
    assert_eq!(got[0], (vec![1], "Twelve meat, not ten.".into(), 1, String::new()));
    let partial: Vec<String> = r
        .events
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| if let Event::Partial { ids, text, .. } = e { assert_eq!(ids, &vec![1]); Some(text.clone()) } else { None })
        .collect();
    assert!(partial.len() >= 2, "the words came as they were written: {partial:?}");
    assert!(partial.windows(2).all(|w| w[1].starts_with(&w[0]) && w[1].len() > w[0].len()), "each the words so far: {partial:?}");
    assert!(partial[0].starts_with("Twelve"));
    r.agents.stop();
}

#[test]
fn forgetting_cancels_and_the_queue_has_a_cap() {
    let f = fake(Box::new(|_, _| (200, openai_text("Hm."), Duration::from_millis(600))));
    let r = rig(config(Api::OpenAi, &f.url));
    r.agents.feed(1, &[pig(vec![])], &[line(1, "pig", "Wendy", "a")], &[], &[]);
    std::thread::sleep(Duration::from_millis(150));
    // (while it answers: 17 more lines - 16 wait, the 17th is refused)
    let more: Vec<_> = (2..=18).map(|i| line(i, "pig", "Wes", "b")).collect();
    r.agents.feed(1, &[], &more[..8], &[], &[]);
    r.agents.feed(1, &[], &more[8..16], &[], &[]);
    r.agents.feed(1, &[], &more[16..], &[], &[]);
    r.wait("the queue refused one", |ev| ev.iter().any(|e| matches!(e, Event::Reply { error: "queue_full", .. })));
    // (forget: what was under way and what waited answered "cancelled"; the late reply is dropped)
    r.agents.feed(1, &[], &[], &[], &["pig".to_string()]);
    // (one reply for all it cancelled, after the queue's refusal)
    let got = r.wait_replies(2);
    let cancelled: Vec<i64> = got.iter().filter(|g| g.3.starts_with("cancelled")).flat_map(|g| g.0.clone()).collect();
    assert!(cancelled.contains(&1) && cancelled.contains(&2) && cancelled.contains(&17), "{got:?}");
    std::thread::sleep(Duration::from_millis(800));
    assert!(!r.replies().iter().any(|g| g.1 == "Hm."), "the reply under way was dropped");
    assert!(r.ledger.spent() == 0.0, "and its hold let go, nothing counted");
    r.agents.stop();
}

#[test]
fn what_it_knows_is_told() {
    let f = fake(Box::new(|_, _| (200, openai_text("ok"), Duration::ZERO)));
    let r = rig(config(Api::OpenAi, &f.url));
    let mut cow = pig(vec![]);
    cow.id = "cow".into();
    cow.version = 7;
    r.agents.feed(1, &[pig(vec![]), cow], &[], &[], &[]);
    r.wait("the known agents", |ev| ev.iter().any(|e| matches!(e, Event::Known(k) if k == &vec![("cow".to_string(), 7), ("pig".to_string(), 1)])));
    // (a new session: nothing known until the game sends them again)
    r.agents.feed(2, &[], &[], &[], &[]);
    r.wait("nothing known", |ev| matches!(ev.iter().rev().find(|e| matches!(e, Event::Known(_))), Some(Event::Known(k)) if k.is_empty()));
    r.agents.stop();
}
