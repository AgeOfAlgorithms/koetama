//! The HTTP connector (PROTOCOL.md "Transport: HTTP"): for a game whose mod can make HTTP requests but not hold a
//! connection open (Tabletop Simulator's WebRequest, Garry's Mod's HTTP, Northstar, a browser game). Koetama listens
//! on 127.0.0.1:<port>; the mod POSTs its feed object to "/" and the answer is what Koetama has for it:
//!     {"objects": [ {...}, ... ], "first": f, "last": n}
//! The objects are numbered per session (1 is the hello); the next feed's "ack" says the last one the mod has, and
//! Koetama keeps the rest until then (a lost answer loses nothing). "wait": up to 1 s - the answer waits for an object
//! that long (a mod polling back to back gets each one at once, without a busy loop); a newer feed ends the wait at
//! once (a mod with a request waiting can send a change right away). GET / says what is listening.
//! Browsers: a request with an Origin header is refused unless the profile lists that origin (allow_origins) - no
//! web page can read what the player says. A minimal HTTP/1.1 server: Content-Length bodies, one request per
//! connection (Connection: close).
use crate::api;
use crate::files::{as_used, cut_line, cut_translation, py_strip};
use crate::profile::{Connector, Profile};
use crate::{intern, voices, Game};
use kd_common::feed::{Feed, FeedSink, RuleState};
use kd_common::Log;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// the longest an answer waits for an object ("wait"; the game still sends a feed at least once a second)
pub const MAX_WAIT: f64 = 1.0;
/// the largest request body
pub const MAX_BODY: usize = 64 * 1024;
/// requests answered at once (more: 503)
const MAX_OPEN: usize = 16;
const RETRY: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_millis(20);
const IO_TIMEOUT: Duration = Duration::from_secs(3);
/// a whole request (head and body) must come within this (slow senders do not hold a slot)
const REQUEST_TIME: Duration = Duration::from_secs(5);
/// the largest ack a session's numbering starts after (a feed's near i64::MAX does not overflow it)
const MAX_ACK: i64 = 1 << 52;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What Koetama has for the game this session.
#[derive(Default)]
struct Outbox {
    /// the game's session (None: no game yet)
    session: Option<i64>,
    /// (number, object), oldest first; dropped once acked
    objects: Vec<(i64, String)>,
    /// the next object's number
    next: i64,
    /// the standing objects as last told (kind -> object: a new session hears them after its hello)
    standing: std::collections::BTreeMap<&'static str, String>,
}

struct Shared {
    running: AtomicBool,
    listening: AtomicBool,
    feed: Mutex<Option<Feed>>,
    updates: AtomicU64,
    out: Mutex<Outbox>,
    arrived: Condvar,
    /// counts the feeds taken: a waiting answer ends when a newer feed comes
    feeds: AtomicU64,
    open: AtomicUsize,
    /// the last error said in the log (each different one once)
    said: Mutex<String>,
}

pub struct HttpGame {
    profile: Arc<Profile>,
    builtin: bool,
    sink: Arc<dyn FeedSink>,
    log: Log,
    port: u16,
    origins: Vec<String>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl HttpGame {
    pub fn new(profile: Arc<Profile>, builtin: bool, sink: Arc<dyn FeedSink>, log: Log) -> HttpGame {
        let (port, origins) = match &profile.connector {
            Connector::Http(h) => (h.port, h.allow_origins.clone()),
            _ => (0, Vec::new()),
        };
        let shared = Arc::new(Shared {
            running: AtomicBool::new(false),
            listening: AtomicBool::new(false),
            feed: Mutex::new(None),
            updates: AtomicU64::new(0),
            out: Mutex::new(Outbox { next: 1, ..Default::default() }),
            arrived: Condvar::new(),
            feeds: AtomicU64::new(0),
            open: AtomicUsize::new(0),
            said: Mutex::new(String::new()),
        });
        HttpGame { profile, builtin, sink, log, port, origins, shared, thread: None }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn listening(&self) -> bool {
        self.shared.listening.load(Ordering::SeqCst)
    }

    /// An object for the game; false if no game is there yet.
    fn push(&self, object: String) -> bool {
        let mut out = lock(&self.shared.out);
        if out.session.is_none() {
            return false;
        }
        let n = out.next;
        out.next += 1;
        out.objects.push((n, object));
        self.shared.arrived.notify_all();
        true
    }
}

impl Drop for HttpGame {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The listening thread: accepts, and answers each request on a thread of its own.
fn serve(port: u16, ctx: Arc<Ctx>) {
    let sh = &ctx.sh;
    let mut listener: Option<TcpListener> = None;
    let mut retry_at = Instant::now();
    let mut last_err = String::new();
    while sh.running.load(Ordering::SeqCst) {
        if listener.is_none() && Instant::now() >= retry_at {
            match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
                Ok(l) => {
                    (ctx.log)(&format!("{}: answering HTTP on 127.0.0.1:{port}", ctx.profile.game));
                    sh.listening.store(true, Ordering::SeqCst);
                    listener = Some(l);
                }
                Err(e) => {
                    let msg = if e.kind() == ErrorKind::AddrInUse {
                        format!(
                            "{}: port {port} on 127.0.0.1 is busy (another program, or another Koetama, uses it) - \
                             trying again every {} s",
                            ctx.profile.game,
                            RETRY.as_secs()
                        )
                    } else {
                        format!("{}: cannot listen on 127.0.0.1:{port}: {e} - trying again every {} s", ctx.profile.game, RETRY.as_secs())
                    };
                    if msg != last_err {
                        (ctx.log)(&msg);
                        last_err = msg;
                    }
                    retry_at = Instant::now() + RETRY;
                }
            }
        }
        if let Some(l) = &listener {
            // (blocking: a request is taken the moment it comes; stop() wakes it with a connection of its own)
            if let Ok((stream, addr)) = l.accept() {
                if !sh.running.load(Ordering::SeqCst) || !addr.ip().is_loopback() {
                    continue;
                }
                if sh.open.fetch_add(1, Ordering::SeqCst) >= MAX_OPEN {
                    sh.open.fetch_sub(1, Ordering::SeqCst);
                    let _ = reply(stream, 503, "{\"error\":\"busy\"}", None);
                    continue;
                }
                let ctx = ctx.clone();
                let spawned = std::thread::Builder::new().name("game http".into()).spawn(move || {
                    handle(stream, &ctx);
                    ctx.sh.open.fetch_sub(1, Ordering::SeqCst);
                });
                if spawned.is_err() {
                    sh.open.fetch_sub(1, Ordering::SeqCst);
                }
            }
        } else {
            std::thread::sleep(TICK);
        }
    }
    sh.listening.store(false, Ordering::SeqCst);
    sh.arrived.notify_all();
}

/// What a request's thread needs.
struct Ctx {
    profile: Arc<Profile>,
    sink: Arc<dyn FeedSink>,
    log: Log,
    origins: Vec<String>,
    sh: Arc<Shared>,
    seq: AtomicU64,
}

impl Ctx {
    /// An error for the log, each different one once.
    fn say(&self, what: &str) {
        let mut said = lock(&self.sh.said);
        if *said != what {
            (self.log)(&format!("{}: {what}", self.profile.game));
            *said = what.to_string();
        }
    }
}

/// A request: method, path, headers (names lower-cased), body.
struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

/// The request on a connection; Err: the status to answer with.
fn read_request(stream: &mut TcpStream) -> Result<Request, u16> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // (the whole request within REQUEST_TIME: a sender that trickles a byte at a time cannot hold one of the
    //  MAX_OPEN slots - the game's own requests would find them all taken)
    let until = Instant::now() + REQUEST_TIME;
    let read = |stream: &mut TcpStream, chunk: &mut [u8]| -> Result<usize, u16> {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(408);
        }
        let _ = stream.set_read_timeout(Some(left.min(IO_TIMEOUT)));
        stream.read(chunk).map_err(|_| 400u16)
    };
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > 16 * 1024 {
            return Err(431);
        }
        let n = read(stream, &mut chunk)?;
        if n == 0 {
            return Err(400);
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = std::str::from_utf8(&buf[..head_end]).map_err(|_| 400u16)?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let (method, path) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
    let mut headers = HashMap::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    if headers.get("transfer-encoding").is_some_and(|t| !t.eq_ignore_ascii_case("identity")) {
        return Err(411); // (a body needs its Content-Length)
    }
    let len: usize = match headers.get("content-length") {
        Some(v) => v.parse().map_err(|_| 400u16)?,
        None => 0,
    };
    if len > MAX_BODY {
        return Err(413);
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = read(stream, &mut chunk)?;
        if n == 0 {
            return Err(400);
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(len);
    Ok(Request { method, path, headers, body })
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        421 => "Misdirected Request",
        411 => "Length Required",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        _ => "Service Unavailable",
    }
}

/// The answer (JSON), with the CORS headers for an allowed origin.
fn reply(mut stream: TcpStream, status: u16, body: &str, origin: Option<&str>) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n",
        reason(status),
        body.len()
    );
    if let Some(o) = origin {
        head.push_str(&format!(
            "Access-Control-Allow-Origin: {o}\r\nVary: Origin\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
             Access-Control-Allow-Headers: Content-Type\r\nAccess-Control-Allow-Private-Network: true\r\n\
             Access-Control-Max-Age: 600\r\n"
        ));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

fn handle(mut stream: TcpStream, ctx: &Ctx) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let Request { method, path, headers, body } = match read_request(&mut stream) {
        Ok(r) => r,
        Err(status) => {
            // (the rest of a refused body is read and dropped first: closing with unread data would reset the
            //  connection, and the client might never see the answer)
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            let mut sink = [0u8; 8192];
            let mut left = 4 * MAX_BODY;
            while left > 0 {
                match stream.read(&mut sink) {
                    Ok(n) if n > 0 => left = left.saturating_sub(n),
                    _ => break,
                }
            }
            let _ = reply(stream, status, &format!("{{\"error\":\"{}\"}}", reason(status)), None);
            return;
        }
    };
    // (a page that got here by DNS rebinding names its own host: only this PC's names are answered)
    if let Some(h) = headers.get("host") {
        let name = h.rsplit_once(':').filter(|(n, p)| !n.is_empty() && !p.contains(']')).map_or(h.as_str(), |(n, _)| n);
        if !["127.0.0.1", "localhost", "[::1]"].iter().any(|ok| name.eq_ignore_ascii_case(ok)) {
            ctx.say(&format!("refused a request for the host {h:?} (only 127.0.0.1 / localhost)"));
            let _ = reply(stream, 421, "{\"error\":\"not this host\"}", None);
            return;
        }
    }
    // (a browser says where the page is from: only the profile's origins may use Koetama)
    let origin = match headers.get("origin") {
        None => None,
        Some(o) if ctx.origins.iter().any(|a| a == o) => Some(o.as_str()),
        Some(o) => {
            ctx.say(&format!("refused a request from the web page {o:?} (not in the profile's allow_origins)"));
            let _ = reply(stream, 403, "{\"error\":\"this web page may not use Koetama\"}", None);
            return;
        }
    };
    let path = path.split('?').next().unwrap_or("");
    let (status, answer) = match (method.as_str(), path) {
        ("OPTIONS", _) => (204, String::new()),
        ("GET", "/") => (
            200,
            format!(
                "{{\"app\":\"{}\",\"version\":\"{}\",\"protocol\":{},\"game\":{}}}",
                kd_common::paths::APP_NAME,
                kd_common::paths::VERSION,
                api::PROTOCOL,
                serde_json::to_string(&ctx.profile.id).unwrap_or_default()
            ),
        ),
        ("POST", "/") => match on_feed(&body, ctx) {
            Ok(a) => (200, a),
            Err(e) => {
                ctx.say(&format!("a feed it cannot read: {e}"));
                (400, format!("{{\"error\":{}}}", serde_json::to_string(&e).unwrap_or_default()))
            }
        },
        (_, "/") => (405, "{\"error\":\"POST a feed, or GET\"}".into()),
        _ => (404, "{\"error\":\"POST to /\"}".into()),
    };
    let _ = reply(stream, status, &answer, origin);
}

/// A feed from the game: taken, its acked objects dropped, then (after waiting up to "wait" s for one) the objects
/// it does not have yet.
fn on_feed(body: &[u8], ctx: &Ctx) -> Result<String, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("not JSON: {e}"))?;
    let mut feed = as_used(api::parse_feed(&v)?, &ctx.profile);
    let wait = v.get("wait").and_then(Value::as_f64).filter(|w| w.is_finite()).unwrap_or(0.0).clamp(0.0, MAX_WAIT);
    feed.seq = ctx.seq.fetch_add(1, Ordering::SeqCst) as i64 + 1;
    let sh = &ctx.sh;
    {
        let mut out = lock(&sh.out);
        if out.session != Some(feed.sid) {
            // (a new session - or Koetama started in the middle of one: numbers go on after what the game has)
            out.session = Some(feed.sid);
            out.objects.clear();
            out.next = feed.ack.clamp(0, MAX_ACK) + 1;
            let hello = api::hello(&api::features(&ctx.profile));
            let n = out.next;
            out.objects.push((n, hello));
            out.next += 1;
            let standing: Vec<String> = out.standing.values().cloned().collect();
            for o in standing {
                let n = out.next;
                out.objects.push((n, o));
                out.next += 1;
            }
        }
        let ack = feed.ack;
        out.objects.retain(|(n, _)| *n > ack);
    }
    *lock(&sh.feed) = Some(feed.clone());
    ctx.sink.set_feed(feed.clone());
    sh.updates.fetch_add(1, Ordering::SeqCst);
    let me = sh.feeds.fetch_add(1, Ordering::SeqCst) + 1;
    sh.arrived.notify_all(); // (an older answer still waiting: it ends now)
    let deadline = Instant::now() + Duration::from_secs_f64(wait);
    let mut out = lock(&sh.out);
    while out.objects.is_empty() && sh.running.load(Ordering::SeqCst) && sh.feeds.load(Ordering::SeqCst) == me {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        out = sh.arrived.wait_timeout(out, left).unwrap_or_else(|e| e.into_inner()).0;
    }
    let first = out.objects.first().map_or(feed.ack.saturating_add(1), |(n, _)| *n);
    let last = out.objects.last().map_or(feed.ack, |(n, _)| *n);
    let objects: Vec<&str> = out.objects.iter().map(|(_, o)| o.as_str()).collect();
    Ok(format!("{{\"objects\":[{}],\"first\":{first},\"last\":{last}}}", objects.join(",")))
}

impl Game for HttpGame {
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
            (self.log)(&format!("{}: not an http profile", self.profile.id));
            return;
        }
        self.shared.running.store(true, Ordering::SeqCst);
        let ctx = Arc::new(Ctx {
            profile: self.profile.clone(),
            sink: self.sink.clone(),
            log: self.log.clone(),
            origins: self.origins.clone(),
            sh: self.shared.clone(),
            seq: AtomicU64::new(0),
        });
        let port = self.port;
        self.thread = std::thread::Builder::new().name("game http".into()).spawn(move || serve(port, ctx)).ok();
    }

    /// Closes the port and waits for the listening thread (answers in flight finish on their own).
    fn stop(&mut self) {
        self.shared.running.store(false, Ordering::SeqCst);
        self.shared.arrived.notify_all();
        if self.thread.is_some() && self.listening() {
            // (the listening thread waits in accept(): a connection wakes it)
            let _ = TcpStream::connect_timeout(&(Ipv4Addr::LOCALHOST, self.port).into(), Duration::from_millis(200));
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let mut out = lock(&self.shared.out);
        out.session = None;
        out.objects.clear();
        out.standing.clear();
    }

    fn send(&self, kind: char, utt: u32, text: &str, times: Option<&[f64]>, t0: Option<Instant>) -> bool {
        let (text, times) = cut_line(text, times);
        if kind == 'l' && text.is_empty() {
            return false;
        }
        let ago = t0.filter(|_| times.is_some()).map(|t| t.elapsed().as_secs_f64());
        self.push(api::speech(kind, utt, &text, times.as_deref(), ago))
    }

    fn send_text(&self, text: &str) -> bool {
        let text = py_strip(text);
        !text.is_empty() && self.send('f', 0, text, None, None)
    }

    fn set_standing(&self, kind: &'static str, object: String) {
        let changed = {
            let mut out = lock(&self.shared.out);
            let changed = out.standing.get(kind) != Some(&object);
            out.standing.insert(kind, object.clone());
            changed
        };
        if changed {
            self.push(object);
        }
    }

    fn send_object(&self, object: String) -> bool {
        self.push(object)
    }

    fn send_translation(&self, id: i64, text: &str, rule: Option<(&str, &str)>) -> bool {
        self.push(api::translation(id, &cut_translation(text), rule))
    }

    fn send_translations_state(&self, states: &[RuleState]) -> bool {
        self.push(api::translations_status(states))
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
        let mut out = vec![format!("answering HTTP on 127.0.0.1:{} (this computer only)", self.port)];
        if !self.origins.is_empty() {
            out.push(format!("web pages allowed: {}", self.origins.join(", ")));
        }
        out
    }
}
