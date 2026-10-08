//! The relay's binary frames (relay/src/frames.js, the same rules):
//!   client -> relay   [1][n][to_1 .. to_n: u16 big-endian][payload]   a voice packet for players to_1 .. to_n
//!   relay -> client   [1][from: u16 big-endian][payload]               the same payload, and who sent it
//! The payload is opaque to the relay (crypto::seal).

pub const VOICE: u8 = 1;
/// recipients in one packet
pub const MAX_TO: usize = 64;
/// bytes of payload in one packet (60 ms of Opus at 24 kbit/s is ~200)
pub const MAX_PAYLOAD: usize = 4000;

/// A player id from the room URL's ?me= (1 to 5 digits, 1..65535), or None.
pub fn parse_id(s: &str) -> Option<u16> {
    if s.is_empty() || s.len() > 5 || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse::<u32>().ok().filter(|&n| (1..=65535).contains(&n)).map(|n| n as u16)
}

/// A frame for the relay: payload for players `to`; None if there are none, more than MAX_TO, or the payload is
/// empty or over MAX_PAYLOAD (the relay would drop it).
pub fn voice_frame(to: &[u16], payload: &[u8]) -> Option<Vec<u8>> {
    if to.is_empty() || to.len() > MAX_TO || payload.is_empty() || payload.len() > MAX_PAYLOAD {
        return None;
    }
    let mut b = Vec::with_capacity(2 + 2 * to.len() + payload.len());
    b.push(VOICE);
    b.push(to.len() as u8);
    for id in to {
        b.extend_from_slice(&id.to_be_bytes());
    }
    b.extend_from_slice(payload);
    Some(b)
}

/// A frame from the relay -> (from, payload), or None.
pub fn parse_out(b: &[u8]) -> Option<(u16, &[u8])> {
    if b.len() < 4 || b[0] != VOICE {
        return None;
    }
    Some((u16::from_be_bytes([b[1], b[2]]), &b[3..]))
}

/// What the relay does with a client's frame (relay/src/frames.js route): -> (the players it goes to, the frame they
/// get), or None (malformed: dropped). The sender never gets its own packet back; repeated ids count once.
/// (The tests' stand-in relay.)
pub fn route(b: &[u8], from: u16) -> Option<(Vec<u16>, Vec<u8>)> {
    if b.len() < 2 || b[0] != VOICE {
        return None;
    }
    let n = b[1] as usize;
    let head = 2 + 2 * n;
    if n > MAX_TO || b.len() < head {
        return None;
    }
    let payload = &b[head..];
    if payload.is_empty() || payload.len() > MAX_PAYLOAD {
        return None;
    }
    let mut to = Vec::new();
    for i in 0..n {
        let id = u16::from_be_bytes([b[2 + 2 * i], b[3 + 2 * i]]);
        if id >= 1 && id != from && !to.contains(&id) {
            to.push(id);
        }
    }
    let mut out = Vec::with_capacity(3 + payload.len());
    out.push(VOICE);
    out.extend_from_slice(&from.to_be_bytes());
    out.extend_from_slice(payload);
    Some((to, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_as_the_relay() {
        // (relay/src/frames.js gives the same)
        let f = voice_frame(&[2, 3, 2, 1, 0], &[9, 8, 7]).unwrap();
        assert_eq!(f, vec![1, 5, 0, 2, 0, 3, 0, 2, 0, 1, 0, 0, 9, 8, 7]);
        let (to, out) = route(&f, 1).unwrap();
        assert_eq!(to, vec![2, 3], "not back to the sender, each once, no 0");
        assert_eq!(out, vec![1, 0, 1, 9, 8, 7]);
        assert_eq!(parse_out(&out), Some((1, &[9u8, 8, 7][..])));
        let big = voice_frame(&[300], &[1]).unwrap();
        assert_eq!(&big[..4], &[1, 1, 1, 44]);
        assert_eq!(route(&big, 65535).unwrap().1[..3], [1, 255, 255]);
        // malformed: dropped
        assert!(route(&[], 1).is_none() && route(&[2, 0, 1], 1).is_none() && route(&[1, 2, 0, 5], 1).is_none());
        assert!(route(&[1, 1, 0, 5], 1).is_none(), "no payload");
        assert!(route(&[1, 65, 0], 1).is_none(), "over 64 recipients");
        let mut huge = vec![1, 1, 0, 2];
        huge.extend(std::iter::repeat_n(0u8, MAX_PAYLOAD + 1));
        assert!(route(&huge, 1).is_none());
        assert!(voice_frame(&[], &[1]).is_none() && voice_frame(&[1], &[]).is_none());
        assert!(voice_frame(&[1; 65], &[1]).is_none() && voice_frame(&[1; 64], &[1]).is_some());
        assert!(parse_out(&[1, 0, 1]).is_none() && parse_out(&[2, 0, 1, 5]).is_none());
        assert_eq!(parse_id("7"), Some(7));
        assert_eq!(parse_id("65535"), Some(65535));
        assert!(parse_id("0").is_none() && parse_id("65536").is_none() && parse_id("+7").is_none() && parse_id("").is_none());
    }
}
