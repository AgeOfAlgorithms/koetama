//! A game hosted on another PC (PROTOCOL.md "Hub"): this player typed the join code the host's game showed them, and
//! their "game" is the host's Koetama (the hub), through the relay. The hub sends this player's feed (`{"feed": ..}`);
//! whatever Koetama would tell a game goes back to the hub (`{"objects": ["<object>", ...]}`), which tells the host's
//! game with the player's id added. An empty `{"objects": []}` every 2 s says this player's Koetama is still here.
use crate::api;
use crate::files::{cut_line, cut_translation, py_strip};
use crate::profile::Profile;
use crate::{intern, Game};
use kd_common::feed::{Feed, FeedSink, RuleState};
use kd_common::Log;
use kd_voice::hub::{Channel, PLAYER};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// how often this end says it is still here
const HEARTBEAT: Duration = Duration::from_secs(2);

pub struct JoinedGame {
    profile: Arc<Profile>,
    code: String,
    sink: Arc<dyn FeedSink>,
    log: Log,
    link: Arc<Mutex<Option<Arc<Channel>>>>,
    /// the standing objects as last told (sent again whenever the link to the hub comes up)
    standing: Arc<Mutex<std::collections::BTreeMap<&'static str, String>>>,
    feed: Arc<Mutex<Option<Feed>>>,
    updates: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl JoinedGame {
    pub fn new(profile: Arc<Profile>, code: String, sink: Arc<dyn FeedSink>, log: Log) -> JoinedGame {
        JoinedGame {
            profile,
            code,
            sink,
            log,
            link: Arc::new(Mutex::new(None)),
            standing: Arc::new(Mutex::new(std::collections::BTreeMap::new())),
            feed: Arc::new(Mutex::new(None)),
            updates: Arc::new(AtomicU64::new(0)),
            running: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }

    /// Objects for the hub (to its game, as this player's).
    fn to_hub(&self, objects: Vec<String>) -> bool {
        match self.link.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            Some(l) if l.connected() => {
                l.send(&json!({ "objects": objects }));
                true
            }
            _ => false,
        }
    }
}

impl Drop for JoinedGame {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Game for JoinedGame {
    fn id(&self) -> &'static str {
        intern(&self.profile.id)
    }

    fn name(&self) -> &'static str {
        intern(&self.profile.game)
    }

    fn needs(&self) -> &'static str {
        intern(&self.profile.needs)
    }

    fn locate(&self) -> (bool, String) {
        (true, String::new())
    }

    fn start(&mut self) {
        self.stop();
        let Some(link) = Channel::start(kd_voice::relay_url(), &self.code, PLAYER, self.log.clone()) else {
            (self.log)(&format!("{:?} is not a join code (8 letters and digits, like K7QF-4MXA)", self.code));
            return;
        };
        let link = Arc::new(link);
        *self.link.lock().unwrap_or_else(|e| e.into_inner()) = Some(link.clone());
        self.running.store(true, Ordering::SeqCst);
        let (running, sink, keep, updates, log, standing) = (
            self.running.clone(),
            self.sink.clone(),
            self.feed.clone(),
            self.updates.clone(),
            self.log.clone(),
            self.standing.clone(),
        );
        self.thread = std::thread::Builder::new()
            .name("joined game".into())
            .spawn(move || {
                let mut beat = Instant::now() - HEARTBEAT;
                let mut told_bad = false;
                let mut was_up = false;
                while running.load(Ordering::SeqCst) {
                    // (the link (back) up: the hub's game hears the standing objects - the voice chat, the status)
                    let up = link.connected();
                    if up && !was_up {
                        let objects: Vec<String> = standing.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
                        link.send(&json!({ "objects": objects }));
                    }
                    was_up = up;
                    for msg in link.received() {
                        let Some(f) = msg.get("feed") else { continue };
                        match api::parse_feed(f) {
                            Ok(feed) => {
                                *keep.lock().unwrap_or_else(|e| e.into_inner()) = Some(feed.clone());
                                updates.fetch_add(1, Ordering::SeqCst);
                                sink.set_feed(feed);
                            }
                            Err(e) if !told_bad => {
                                log(&format!("the host's feed for this player cannot be read: {e}"));
                                told_bad = true;
                            }
                            Err(_) => {}
                        }
                    }
                    if link.connected() && beat.elapsed() >= HEARTBEAT {
                        beat = Instant::now();
                        link.send(&json!({ "objects": Vec::<String>::new() }));
                    }
                    std::thread::sleep(Duration::from_millis(15));
                }
            })
            .ok();
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        *self.link.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn send(&self, kind: char, utt: u32, text: &str, times: Option<&[f64]>, t0: Option<Instant>) -> bool {
        let (text, times) = cut_line(text, times);
        if kind == 'l' && text.is_empty() {
            return false;
        }
        // (a voice room: the hub's game has one already - the host's Koetama offers its own)
        if kind == 'r' {
            return true;
        }
        let ago = t0.filter(|_| times.is_some()).map(|t| t.elapsed().as_secs_f64());
        self.to_hub(vec![api::speech(kind, utt, &text, times.as_deref(), ago)])
    }

    fn send_text(&self, text: &str) -> bool {
        let text = py_strip(text);
        !text.is_empty() && self.send('f', 0, text, None, None)
    }

    fn send_translation(&self, id: i64, text: &str, rule: Option<(&str, &str)>) -> bool {
        self.to_hub(vec![api::translation(id, &cut_translation(text), rule)])
    }

    fn send_translations_state(&self, states: &[RuleState]) -> bool {
        self.to_hub(vec![api::translations_status(states)])
    }

    fn set_standing(&self, kind: &'static str, object: String) {
        self.standing.lock().unwrap_or_else(|e| e.into_inner()).insert(kind, object.clone());
        self.to_hub(vec![object]);
    }

    fn send_object(&self, object: String) -> bool {
        self.to_hub(vec![object])
    }

    fn test_voices(&self) -> HashMap<i64, PathBuf> {
        HashMap::new()
    }

    fn speaker_name(&self, src: i64) -> String {
        src.to_string()
    }

    fn feed(&self) -> Option<Feed> {
        self.feed.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn connected(&self) -> bool {
        self.sink.fresh()
    }

    fn updates(&self) -> u64 {
        self.updates.load(Ordering::SeqCst)
    }

    fn describe(&self) -> Vec<String> {
        let there = self.link.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|l| l.other_there());
        vec![
            format!("joined a hosted game with the code {}", self.code),
            if there { "the host's Koetama is there".into() } else { "waiting for the host's Koetama".into() },
        ]
    }
}

/// The game mod for a joined game (not a profile anyone writes: GameKind::joined).
pub fn profile() -> Profile {
    let mut p = Profile::parse(
        &json!({"format": 1, "id": "joined-game", "game": "A hosted game", "mod": "joined with a code",
                "url": "https://github.com/AgeOfAlgorithms/koetama", "author": "-",
                "uses": ["voices", "speech", "translate"], "connector": {"type": "socket", "port": 1024}})
        .to_string(),
    )
    .expect("the joined game's profile is valid");
    p.needs = "the host's game (it shows you your join code)".into();
    p
}
