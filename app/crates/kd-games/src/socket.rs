//! The SOCKET connector: for a game whose mod can open a TCP connection. Koetama listens on 127.0.0.1:<port> (this
//! computer only, never another interface); one client at a time (a new connection replaces the old one);
//! newline-delimited JSON both ways (PROTOCOL.md "Adding a game mod: profiles"):
//!   mod -> Koetama    {"type":"hello","protocol":2,"game":..,"mod":..}   (optional)
//!                      {"type":"feed",...}   (api::parse_feed; whenever it changes and at least every second: no feed for
//!                      kd_audio::STALE s = not connected)
//!   Koetama -> mod    api.rs's objects: hello (on connect), speech, room (once per connection), voice, translation,
//!                      translations_status
//! No acks or pings: the connection is the liveness.
use crate::files::{as_used, cut_line, cut_translation};
use crate::api;
use crate::profile::{Connector, Profile};
use crate::{intern, voices, Game};
use kd_common::feed::{Feed, FeedSink, RuleState};
use kd_common::Log;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The protocol's number (api::PROTOCOL).
pub use crate::api::PROTOCOL;
/// A line longer than this drops the connection (a feed is a few hundred bytes).
pub const MAX_LINE: usize = 64 * 1024;
/// How often a busy port is tried again.
pub const RETRY: Duration = Duration::from_secs(2);
/// How long the thread waits for the client's bytes before looking for a new connection.
const TICK: Duration = Duration::from_millis(20);
/// A message the mod does not read within this drops the connection (it hangs).
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the listening thread and the program share.
struct Shared {
    running: AtomicBool,
    /// the connected mod (a handle for writing; the thread reads its own)
    client: Mutex<Option<TcpStream>>,
    feed: Mutex<Option<Feed>>,
    updates: AtomicU64,
    /// what the mod's hello said: "<game> (<mod>)"
    peer: Mutex<Option<String>>,
    listening: AtomicBool,
    /// the standing objects as last told (kind -> object): a new connection gets them after its hello
    standing: Mutex<std::collections::BTreeMap<&'static str, String>>,
}

/// One line to the client; false (and the client dropped) if it cannot be written.
fn write_line(client: &mut Option<TcpStream>, line: &str) -> bool {
    let Some(s) = client.as_mut() else {
        return false;
    };
    let mut bytes = line.as_bytes().to_vec();
    bytes.push(b'\n');
    if s.write_all(&bytes).and_then(|_| s.flush()).is_ok() {
        return true;
    }
    let _ = s.shutdown(Shutdown::Both);
    *client = None;
    false
}

fn hello_line(profile: &Profile) -> String {
    api::hello(&api::features(profile))
}

/// A feed line's object -> a Feed (api::parse_feed; seq: counted here; the session is the connection's number, set by
/// the caller; no acks or pings).
pub fn parse_socket_feed(v: &Value, seq: i64) -> Result<Feed, String> {
    let mut feed = api::parse_feed(v)?;
    feed.seq = seq;
    Ok(feed)
}


/// The connection being read, on the thread.
struct Conn {
    stream: TcpStream,
    buf: Vec<u8>,
    /// a malformed line was logged (once per connection)
    told_bad: bool,
}

/// The thread: listen (retrying a busy port), take connections (a new one replaces the old), read lines.
fn serve(port: u16, profile: Arc<Profile>, sink: Arc<dyn FeedSink>, sh: Arc<Shared>, log: Log) {
    let mut listener: Option<TcpListener> = None;
    let mut retry_at = Instant::now();
    let mut last_err = String::new();
    let mut conn: Option<Conn> = None;
    let mut seq: i64 = 0;
    // (each connection is a game session: its feeds' sid, so the voice room is made anew for it)
    let mut session: i64 = 0;
    let drop_conn = |conn: &mut Option<Conn>, why: &str| {
        if let Some(c) = conn.take() {
            let _ = c.stream.shutdown(Shutdown::Both);
            log(why);
        }
        *lock(&sh.client) = None;
        *lock(&sh.feed) = None;
        *lock(&sh.peer) = None;
    };
    while sh.running.load(Ordering::SeqCst) {
        if listener.is_none() && Instant::now() >= retry_at {
            // (127.0.0.1 only: nothing from another computer can connect)
            match TcpListener::bind((Ipv4Addr::LOCALHOST, port)).and_then(|l| l.set_nonblocking(true).map(|_| l)) {
                Ok(l) => {
                    log(&format!("{}: listening on 127.0.0.1:{port}", profile.game));
                    sh.listening.store(true, Ordering::SeqCst);
                    listener = Some(l);
                }
                Err(e) => {
                    let msg = if e.kind() == ErrorKind::AddrInUse {
                        format!(
                            "{}: port {port} on 127.0.0.1 is busy (another program, or another Koetama, uses it) - \
                             trying again every {} s",
                            profile.game,
                            RETRY.as_secs()
                        )
                    } else {
                        format!("{}: cannot listen on 127.0.0.1:{port}: {e} - trying again every {} s", profile.game, RETRY.as_secs())
                    };
                    if msg != last_err {
                        log(&msg);
                        last_err = msg;
                    }
                    retry_at = Instant::now() + RETRY;
                }
            }
        }
        if let Some(l) = &listener {
            while let Ok((stream, addr)) = l.accept() {
                if !addr.ip().is_loopback() {
                    continue; // (cannot happen on 127.0.0.1; never served anyway)
                }
                let setup = stream
                    .set_nonblocking(false)
                    .and_then(|_| stream.set_read_timeout(Some(TICK)))
                    .and_then(|_| stream.set_write_timeout(Some(WRITE_TIMEOUT)))
                    .and_then(|_| stream.try_clone());
                let Ok(writer) = setup else {
                    continue;
                };
                let _ = stream.set_nodelay(true);
                if conn.is_some() {
                    drop_conn(&mut conn, &format!("{}: a new connection replaces the old one", profile.game));
                } else {
                    log(&format!("{}: the game connected", profile.game));
                }
                let mut client = lock(&sh.client);
                *client = Some(writer);
                write_line(&mut client, &hello_line(&profile));
                // (then the standing objects: the voice chat, the status)
                for o in lock(&sh.standing).values() {
                    write_line(&mut client, o);
                }
                conn = Some(Conn { stream, buf: Vec::new(), told_bad: false });
                session += 1;
            }
        }
        let Some(c) = conn.as_mut() else {
            std::thread::sleep(TICK);
            continue;
        };
        let mut chunk = [0u8; 8192];
        match c.stream.read(&mut chunk) {
            Ok(0) => {
                drop_conn(&mut conn, &format!("{}: the game disconnected", profile.game));
                continue;
            }
            Ok(n) => c.buf.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => continue,
            Err(e) => {
                drop_conn(&mut conn, &format!("{}: the connection broke: {e}", profile.game));
                continue;
            }
        }
        let mut too_long = false;
        while let Some(pos) = c.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = c.buf.drain(..=pos).collect();
            if line.len() > MAX_LINE + 1 {
                too_long = true;
                break;
            }
            if let Err(e) = on_line(&line, &profile, &sink, &sh, &mut seq, session, &log) {
                if !c.told_bad {
                    c.told_bad = true;
                    log(&format!("{}: skipped a malformed line from the game ({e}); more are skipped quietly", profile.game));
                }
            }
        }
        if too_long || c.buf.len() > MAX_LINE {
            drop_conn(&mut conn, &format!("{}: a line over {} KB from the game: connection dropped", profile.game, MAX_LINE / 1024));
        }
    }
    drop_conn(&mut conn, &format!("{}: stopped", profile.game));
    sh.listening.store(false, Ordering::SeqCst);
}

/// One line from the mod. Err: malformed (skipped).
fn on_line(
    line: &[u8],
    profile: &Profile,
    sink: &Arc<dyn FeedSink>,
    sh: &Shared,
    seq: &mut i64,
    session: i64,
    log: &Log,
) -> Result<(), String> {
    let text = std::str::from_utf8(line).map_err(|_| "not UTF-8")?.trim();
    if text.is_empty() {
        return Ok(());
    }
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    match v.get("type").and_then(Value::as_str) {
        Some("feed") => {
            let mut feed = as_used(parse_socket_feed(&v, *seq + 1)?, profile);
            feed.sid = session;
            *seq += 1;
            *lock(&sh.feed) = Some(feed.clone());
            sink.set_feed(feed);
            sh.updates.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        Some("hello") => {
            let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").chars().take(80).collect::<String>();
            let protocol = v.get("protocol").and_then(Value::as_u64).unwrap_or(PROTOCOL);
            let peer = format!("{} ({})", s("game"), s("mod"));
            log(&format!("{}: hello from {peer}, protocol {protocol}", profile.game));
            if protocol > PROTOCOL {
                log(&format!("{}: the mod speaks protocol {protocol}, this Koetama {PROTOCOL}: update Koetama", profile.game));
            }
            *lock(&sh.peer) = Some(peer);
            Ok(())
        }
        // (a later protocol's messages: ignored)
        Some(_) => Ok(()),
        None => Err("no \"type\"".into()),
    }
}

/// A game linked by the socket connector.
pub struct SocketGame {
    profile: Arc<Profile>,
    builtin: bool,
    port: u16,
    sink: Arc<dyn FeedSink>,
    log: Log,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl SocketGame {
    /// profile must use the socket connector (else: a game that never connects).
    pub fn new(profile: Arc<Profile>, builtin: bool, sink: Arc<dyn FeedSink>, log: Log) -> SocketGame {
        let port = match &profile.connector {
            Connector::Socket(s) => s.port,
            Connector::Files(_) | Connector::Http(_) => 0,
        };
        let shared = Arc::new(Shared {
            running: AtomicBool::new(false),
            client: Mutex::new(None),
            feed: Mutex::new(None),
            updates: AtomicU64::new(0),
            peer: Mutex::new(None),
            listening: AtomicBool::new(false),
            standing: Mutex::new(std::collections::BTreeMap::new()),
        });
        SocketGame { profile, builtin, port, sink, log, shared, thread: None }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The port is open (false while it is busy).
    pub fn listening(&self) -> bool {
        self.shared.listening.load(Ordering::SeqCst)
    }

    /// A mod is connected now.
    pub fn has_client(&self) -> bool {
        lock(&self.shared.client).is_some()
    }

    /// What the connected mod's hello said: "<game> (<mod>)".
    pub fn peer(&self) -> Option<String> {
        lock(&self.shared.peer).clone()
    }

    fn send_line(&self, line: &str) -> bool {
        write_line(&mut lock(&self.shared.client), line)
    }
}

impl Drop for SocketGame {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Game for SocketGame {
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
        self.profile.locate(None)
    }

    fn start(&mut self) {
        self.stop();
        if self.port == 0 {
            (self.log)(&format!("{}: not a socket profile", self.profile.id));
            return;
        }
        self.shared.running.store(true, Ordering::SeqCst);
        let (port, p, sink, sh, log) = (self.port, self.profile.clone(), self.sink.clone(), self.shared.clone(), self.log.clone());
        self.thread = std::thread::Builder::new().name("game socket".into()).spawn(move || serve(port, p, sink, sh, log)).ok();
    }

    /// Closes the connection and the port, and waits for the thread (at most ~one TICK).
    fn stop(&mut self) {
        self.shared.running.store(false, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    fn send(&self, kind: char, utt: u32, text: &str, times: Option<&[f64]>, t0: Option<Instant>) -> bool {
        let (text, times) = cut_line(text, times);
        if kind == 'l' && text.is_empty() {
            return false;
        }
        let ago = t0.map(|t| t.elapsed().as_secs_f64());
        let line = api::speech(kind, utt, &text, times.as_deref(), ago);
        self.send_line(&line)
    }

    fn send_text(&self, text: &str) -> bool {
        let text = crate::files::py_strip(text);
        !text.is_empty() && self.send('f', 0, text, None, None)
    }

    fn set_standing(&self, kind: &'static str, object: String) {
        let changed = lock(&self.shared.standing).insert(kind, object.clone()).as_ref() != Some(&object);
        if changed {
            self.send_line(&object);
        }
    }

    fn send_object(&self, object: String) -> bool {
        self.send_line(&object)
    }

    fn send_translation(&self, id: i64, text: &str, rule: Option<(&str, &str)>) -> bool {
        self.send_line(&api::translation(id, &cut_translation(text), rule))
    }

    fn send_translations_state(&self, rules: &[RuleState]) -> bool {
        self.send_line(&api::translations_status(rules))
    }

    fn test_voices(&self) -> HashMap<i64, PathBuf> {
        voices::for_profile(&self.profile, self.builtin)
    }

    fn speaker_name(&self, src: i64) -> String {
        self.profile.speaker_name(src)
    }

    fn feed(&self) -> Option<Feed> {
        lock(&self.shared.feed).clone()
    }

    fn connected(&self) -> bool {
        self.sink.fresh()
    }

    fn updates(&self) -> u64 {
        self.shared.updates.load(Ordering::SeqCst)
    }

    fn describe(&self) -> Vec<String> {
        let mut out = vec![format!("listening on 127.0.0.1:{} (this computer only)", self.port)];
        if let Some(p) = self.peer() {
            out.push(format!("connected: {p}"));
        }
        out
    }
}
