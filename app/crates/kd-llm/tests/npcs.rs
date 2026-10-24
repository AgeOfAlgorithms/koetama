//! The NPC service against a fake LLM (both API styles, on this PC): lines that come while an NPC's request runs wait
//! and go as one turn, function calls go to the game and their results back to the LLM, the host's budget and
//! requests a minute, a failed request, a new session, an NPC nobody defined.
use kd_common::feed::{NpcDef, NpcFunction, NpcLine, PlayerId};
use kd_llm::{Api, Config, Event, Npcs};
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
                let out = out.to_string();
                let mut c = conn;
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
    npcs: Npcs,
    events: Arc<(Mutex<Vec<Event>>, Condvar)>,
}

fn rig(cfg: Config) -> Rig {
    let events: Arc<(Mutex<Vec<Event>>, Condvar)> = Arc::default();
    let ev = events.clone();
    let npcs = Npcs::start(
        cfg,
        Arc::new(move |e| {
            ev.0.lock().unwrap().push(e);
            ev.1.notify_all();
        }),
        kd_common::null_log(),
    );
    Rig { npcs, events }
}

fn config(api: Api, url: &str) -> Config {
    Config { api, url: url.into(), model: "m".into(), key: "sk-test".into(), budget: 5.0, price_in: 1.0, price_out: 5.0, per_minute: 60, ..Default::default() }
}

fn pig(functions: Vec<NpcFunction>) -> NpcDef {
    NpcDef { id: "pig".into(), version: 1, persona: "You are the Pig King.".into(), functions, max_words: 40, voice: String::new() }
}

fn line(id: i64, npc: &str, who: &str, text: &str) -> NpcLine {
    NpcLine { id, npc: npc.into(), text: text.into(), from_id: Some(PlayerId::string(who)), from_name: who.into(), ..Default::default() }
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
                Event::Reply { ids, text, calls, error, .. } => Some((ids.clone(), text.clone(), calls.len(), error.clone())),
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
    r.npcs.feed(1, &[pig(vec![])], &[line(1, "pig", "Wendy", "hello pig")], &[], &[]);
    std::thread::sleep(Duration::from_millis(150));
    // (the game sends its lines again in every feed until they are answered: each is asked once)
    r.npcs.feed(1, &[], &[line(1, "pig", "Wendy", "hello pig"), line(2, "pig", "Wes", "hi"), line(3, "pig", "Wilson", "hey")], &[], &[]);
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
    let spent = r.events.0.lock().unwrap().iter().rev().find_map(|e| if let Event::Spent(s) = e { Some(*s) } else { None }).unwrap();
    assert!((spent - 2.0 * (100.0 * 1.0 + 20.0 * 5.0) / 1e6).abs() < 1e-12, "spent {spent}");
    r.npcs.stop();
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
    let trade = NpcFunction { name: "offer_trade".into(), description: "Offer a trade".into(), parameters: json!({"type": "object"}) };
    r.npcs.feed(1, &[pig(vec![trade])], &[line(41, "pig", "Wendy", "10 meat for a helmet?")], &[], &[]);
    let got = r.wait_replies(1);
    assert_eq!(got[0], (vec![41], "Twelve meat.".into(), 1, String::new()));
    match &r.events.0.lock().unwrap().iter().find(|e| matches!(e, Event::Reply { .. })).unwrap() {
        Event::Reply { calls, .. } => assert_eq!((calls[0].id.as_str(), calls[0].args["price"].as_i64()), ("call_1", Some(12))),
        _ => unreachable!(),
    }
    // (a line while the call waits for the game: it waits too)
    r.npcs.feed(1, &[], &[line(42, "pig", "Wes", "me too")], &[], &[]);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(f.seen.lock().unwrap().len(), 1, "nothing asked while a call waits for its result");
    r.npcs.feed(1, &[], &[], &[("call_1".into(), "ok: offered 1 football helmet for 12 meat".into())], &[]);
    let got = r.wait_replies(3);
    assert_eq!(got[1], (vec![41], "Pleasure doing business.".into(), 0, String::new()), "the follow-up answers the same line");
    assert_eq!(got[2].0, vec![42], "then the line that waited");
    let seen = f.seen.lock().unwrap();
    let m = seen[1].1["messages"].as_array().unwrap();
    assert_eq!(m[3], json!({"role": "tool", "tool_call_id": "call_1", "content": "ok: offered 1 football helmet for 12 meat"}));
    assert_eq!(seen[1].1["tools"][0]["function"]["name"], "offer_trade");
    drop(seen);
    r.npcs.stop();
}

#[test]
fn the_budget_and_requests_a_minute() {
    let f = fake(Box::new(|_, _| (200, openai_text("ok"), Duration::ZERO)));
    // (one reply costs 100 * 1 + 20 * 5 = 200 millionths of a dollar: a budget of 150 millionths is spent by one)
    let r = rig(Config { budget: 0.000150, ..config(Api::OpenAi, &f.url) });
    r.npcs.feed(1, &[pig(vec![])], &[line(1, "pig", "Wendy", "a")], &[], &[]);
    r.wait_replies(1);
    r.npcs.feed(1, &[], &[line(2, "pig", "Wendy", "b")], &[], &[]);
    let got = r.wait_replies(2);
    assert_eq!(got[1], (vec![2], String::new(), 0, "budget".into()));
    r.wait("the budget status", |ev| ev.iter().any(|e| matches!(e, Event::Status { state: "budget", .. })));
    assert_eq!(f.seen.lock().unwrap().len(), 1);
    r.npcs.stop();
    // (one request a minute: a second NPC's line waits - "busy")
    let r = rig(Config { per_minute: 1, ..config(Api::OpenAi, &f.url) });
    let mut cow = pig(vec![]);
    cow.id = "cow".into();
    r.npcs.feed(2, &[pig(vec![]), cow], &[line(1, "pig", "Wendy", "a"), line(2, "cow", "Wendy", "b")], &[], &[]);
    r.wait_replies(1);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(r.replies().len(), 1, "the second waits for the minute");
    assert_eq!(r.last_status(), Some("busy"));
    r.npcs.stop();
}

#[test]
fn a_failed_request_a_new_session_and_an_unknown_npc() {
    let f = fake(Box::new(|n, _| {
        if n == 0 {
            (401, json!({"error": {"message": "Incorrect API key provided", "type": "invalid_request_error"}}), Duration::ZERO)
        } else {
            (200, openai_text("better now"), Duration::ZERO)
        }
    }));
    let r = rig(config(Api::OpenAi, &f.url));
    r.npcs.feed(1, &[pig(vec![])], &[line(1, "pig", "Wendy", "a"), line(2, "nobody", "Wendy", "b")], &[], &[]);
    let got = r.wait_replies(2);
    assert!(got.iter().any(|g| g.0 == vec![2] && g.3.contains("no NPC \"nobody\"")));
    assert!(got.iter().any(|g| g.0 == vec![1] && g.3.contains("401") && g.3.contains("Incorrect API key")), "{got:?}");
    r.wait("the error status", |ev| ev.iter().any(|e| matches!(e, Event::Status { state: "error", .. })));
    r.npcs.feed(1, &[], &[line(3, "pig", "Wendy", "c")], &[], &[]);
    r.wait_replies(3);
    assert_eq!(f.seen.lock().unwrap()[1].1["messages"].as_array().unwrap().len(), 2, "the failed turn was left out");
    // (a new session: the NPC starts over - and must be sent again)
    r.npcs.feed(2, &[pig(vec![])], &[line(1, "pig", "Wendy", "again")], &[], &[]);
    r.wait_replies(4);
    assert_eq!(f.seen.lock().unwrap()[2].1["messages"].as_array().unwrap().len(), 2, "a new session's NPC remembers nothing");
    assert_eq!(r.last_status(), Some("ready"));
    r.npcs.stop();
}

#[test]
fn anthropic_style() {
    let f = fake(Box::new(|_, _| {
        (200, json!({"content": [{"type": "text", "text": "Oink."}], "usage": {"input_tokens": 50, "output_tokens": 5}}), Duration::ZERO)
    }));
    let r = rig(config(Api::Anthropic, &f.url));
    r.npcs.feed(1, &[pig(vec![])], &[line(1, "pig", "Wendy", "hello")], &[], &[]);
    let got = r.wait_replies(1);
    assert_eq!(got[0].1, "Oink.");
    let seen = f.seen.lock().unwrap();
    let head = seen[0].0.to_ascii_lowercase();
    assert!(head.starts_with("post /v1/messages") && head.contains("x-api-key: sk-test") && head.contains("anthropic-version"));
    assert_eq!(seen[0].1["messages"][0], json!({"role": "user", "content": "Wendy: hello"}));
    drop(seen);
    r.npcs.stop();
}
