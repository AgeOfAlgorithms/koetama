//! A connection to the relay's room (PROTOCOL.md "Real voices"): a WebSocket to
//! `<relay>/v1/room/<room>?me=<me>` (wss: TLS by rustls with the Mozilla roots; ws: plain, for a local relay and the
//! tests), blocking with short read timeouts, used by one thread (the voice thread). Binary frames both ways
//! (frames.rs); the text "ping" keeps it alive (answered "pong").
use std::io::ErrorKind;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Connector, Message, WebSocket};

/// how long connecting (and the handshake) may take
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// how long one poll() waits for something to arrive
pub const POLL: Duration = Duration::from_millis(5);
/// a send the relay does not take within this: the connection is broken
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
/// the keep-alive's text
pub const PING: &str = "ping";
/// the close code of a connection replaced by another with the same player id (relay/src/index.js)
pub const REPLACED: u16 = 4000;
/// the HTTP status of a relay that refuses a player id already in the room
pub const TAKEN: u16 = 409;
/// the largest message taken from the relay (bytes)
pub const MAX_MESSAGE: usize = 16 * 1024;

/// The room's address on the relay. room: "<32 hex>" or "<32 hex>@<region>" (where the room should live: the relay
/// asks Cloudflare for it when the room is made; kd_common::feed::REGIONS).
pub fn room_url(relay: &str, room: &str, me: u16) -> String {
    let (room, region) = room.split_once('@').map_or((room, ""), |(r, g)| (r, g));
    let region = if region.is_empty() { String::new() } else { format!("&region={region}") };
    format!("{}/v1/room/{room}?me={me}{region}", relay.trim_end_matches('/'))
}

fn tls() -> Result<Arc<rustls::ClientConfig>, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

pub struct Conn {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
    /// the code the relay closed it with (None: open, or closed without one)
    code: Option<u16>,
}

/// Why a connection could not be made: a message a player can read, and whether the relay refused the player id
/// (TAKEN: another connection in the room has it).
#[derive(Clone, Debug, PartialEq)]
pub struct OpenError {
    pub text: String,
    pub taken: bool,
}

impl From<String> for OpenError {
    fn from(text: String) -> OpenError {
        OpenError { text, taken: false }
    }
}

impl Conn {
    /// Connects to the room (blocking, at most ~CONNECT_TIMEOUT per address). Err: a message a player can read.
    pub fn open(relay: &str, room: &str, me: u16) -> Result<Conn, String> {
        Conn::join(relay, room, me).map_err(|e| e.text)
    }

    /// open, telling a refused player id apart.
    pub fn join(relay: &str, room: &str, me: u16) -> Result<Conn, OpenError> {
        let url = room_url(relay, room, me);
        let uri: tungstenite::http::Uri = url.parse().map_err(|_| format!("not a relay address: {relay}"))?;
        let secure = match uri.scheme_str() {
            Some("wss") => true,
            Some("ws") => false,
            _ => return Err(format!("the relay address must start with wss:// (or ws://): {relay}").into()),
        };
        let host = uri.host().ok_or_else(|| format!("no host in {relay}"))?.trim_matches(['[', ']']).to_string();
        let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
        let addrs: Vec<_> = (host.as_str(), port).to_socket_addrs().map_err(|e| format!("{host}: {e}"))?.collect();
        let mut last = format!("{host}: no address");
        let mut tcp = None;
        for a in addrs {
            match TcpStream::connect_timeout(&a, CONNECT_TIMEOUT) {
                Ok(s) => {
                    tcp = Some(s);
                    break;
                }
                Err(e) => last = format!("{host}: {e}"),
            }
        }
        let tcp = tcp.ok_or(last)?;
        let _ = tcp.set_nodelay(true);
        tcp.set_read_timeout(Some(CONNECT_TIMEOUT)).map_err(|e| e.to_string())?;
        tcp.set_write_timeout(Some(WRITE_TIMEOUT)).map_err(|e| e.to_string())?;
        let connector = if secure { Connector::Rustls(tls()?) } else { Connector::Plain };
        // (the relay sends packets of ~4 KB at most: anything far bigger is refused before it is held in memory)
        let limits =
            tungstenite::protocol::WebSocketConfig::default().max_message_size(Some(MAX_MESSAGE)).max_frame_size(Some(MAX_MESSAGE));
        let (ws, _) = tungstenite::client_tls_with_config(url.as_str(), tcp, Some(limits), Some(connector)).map_err(|e| match e {
            tungstenite::HandshakeError::Failure(tungstenite::Error::Http(r)) if r.status().as_u16() == TAKEN => {
                OpenError { text: "the relay refused this player id: another player in the room has it".into(), taken: true }
            }
            tungstenite::HandshakeError::Failure(e) => e.to_string().into(),
            tungstenite::HandshakeError::Interrupted(_) => String::from("the relay did not answer in time").into(),
        })?;
        let sock = match ws.get_ref() {
            MaybeTlsStream::Plain(s) => s,
            MaybeTlsStream::Rustls(s) => &s.sock,
            _ => return Err(String::from("an unexpected connection type").into()),
        };
        sock.set_read_timeout(Some(POLL)).map_err(|e| e.to_string())?;
        Ok(Conn { ws, code: None })
    }

    /// One binary frame (frames::voice_frame) to the relay.
    pub fn send(&mut self, frame: Vec<u8>) -> Result<(), String> {
        self.ws.send(Message::binary(frame)).map_err(|e| e.to_string())
    }

    /// The keep-alive.
    pub fn ping(&mut self) -> Result<(), String> {
        self.ws.send(Message::text(PING)).map_err(|e| e.to_string())
    }

    /// The binary frames that arrived (waits up to ~POLL when none has). Err: the connection is gone.
    pub fn poll(&mut self, out: &mut Vec<Vec<u8>>) -> Result<(), String> {
        loop {
            match self.ws.read() {
                Ok(Message::Binary(b)) => out.push(b.to_vec()),
                Ok(Message::Close(f)) => {
                    self.code = f.as_ref().map(|f| u16::from(f.code));
                    return Err(match f {
                        Some(f) if u16::from(f.code) == REPLACED => {
                            "replaced by another connection with the same player id".into()
                        }
                        Some(f) => format!("closed by the relay ({}: {})", u16::from(f.code), f.reason),
                        None => "closed by the relay".into(),
                    })
                }
                Ok(_) => {} // ("pong", WebSocket pings: tungstenite answers them)
                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    return Ok(())
                }
                Err(e) => return Err(e.to_string()),
            }
        }
    }

    /// The relay closed it because another connection with the same player id came (REPLACED).
    pub fn replaced(&self) -> bool {
        self.code == Some(REPLACED)
    }

    /// Says goodbye (best effort, never waits long).
    pub fn close(mut self) {
        let _ = self.ws.close(None);
        let _ = self.ws.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rooms_address_with_and_without_a_region() {
        let r = "0123456789abcdef0123456789abcdef";
        assert_eq!(room_url("wss://relay.example/", r, 7), format!("wss://relay.example/v1/room/{r}?me=7"));
        assert_eq!(
            room_url("wss://relay.example", &format!("{r}@weur"), 7),
            format!("wss://relay.example/v1/room/{r}?me=7&region=weur")
        );
    }
}
