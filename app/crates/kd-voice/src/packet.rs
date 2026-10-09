//! A voice packet's plaintext (PROTOCOL.md "Real voices"), version 2:
//!   [2][seq: u32 BE][flags: u8][near: f32 BE][far: f32 BE][n: u8][the sender's id: n bytes UTF-8]
//!   [k][k x (len: u16 BE, Opus bytes)]
//! flags: 1 = the last packet of a stretch of talking, 2 = presence (no audio: k = 0), 4 = the id is a number.
//! The range (0, 0) is none given; an id of 0 bytes is none given.
use kd_common::feed::{PlayerId, MAX_ID_CHARS};

pub const VERSION: u8 = 2;
/// flags: the last packet of a stretch of talking
pub const LAST: u8 = 1;
/// flags: presence - "I am in the room", no audio
pub const PRESENCE: u8 = 2;
/// flags: the sender's id is a number (PlayerId.number)
pub const NUMBER: u8 = 4;
/// frames one packet may hold (Koetama sends PER_PACKET)
pub const MAX_FRAMES: usize = 16;
/// bytes of an id (n is a u8)
pub const MAX_ID_BYTES: usize = 255;
/// the bytes before the id: version, seq, flags, near, far, n
const HEAD: usize = 15;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Packet {
    /// counts up by one per voice packet from a sender (wrapping); presence packets count on their own
    pub seq: u32,
    /// the last packet of a stretch of talking: the listener's voice ends after it
    pub last: bool,
    /// presence: no audio, only who is in the room (and their range)
    pub presence: bool,
    /// how far the sender's voice reaches now, [near, far] in the game's units (None: not given)
    pub range: Option<(f32, f32)>,
    /// the sender's id as its game gives it (text "": none given)
    pub id: PlayerId,
    /// Opus frames, 20 ms each, in order
    pub frames: Vec<Vec<u8>>,
}

/// A range a packet can carry: finite, 0 <= near < far.
pub fn good_range((near, far): (f32, f32)) -> bool {
    near.is_finite() && far.is_finite() && 0.0 <= near && near < far
}

/// An id a packet can carry: none (""), or 1 to MAX_ID_CHARS characters (at most MAX_ID_BYTES bytes) without control
/// characters; a number as its decimal digits, written the way Rust writes an i64.
pub fn good_id(id: &PlayerId) -> bool {
    if id.text.is_empty() {
        return !id.number;
    }
    let fits = id.text.len() <= MAX_ID_BYTES && id.text.chars().count() <= MAX_ID_CHARS;
    let clean = !id.text.chars().any(char::is_control);
    let number = !id.number || id.text.parse::<i64>().is_ok_and(|n| n.to_string() == id.text);
    fits && clean && number
}

impl Packet {
    /// The plaintext. A range or id it cannot carry (good_range, good_id) goes as none.
    pub fn encode(&self) -> Vec<u8> {
        let id = if good_id(&self.id) { self.id.clone() } else { PlayerId::default() };
        let (near, far) = self.range.filter(|&r| good_range(r)).unwrap_or((0.0, 0.0));
        let flags = (if self.last { LAST } else { 0 })
            | (if self.presence { PRESENCE } else { 0 })
            | (if id.number { NUMBER } else { 0 });
        let mut b = Vec::with_capacity(HEAD + id.text.len() + 1 + self.frames.iter().map(|f| 2 + f.len()).sum::<usize>());
        b.push(VERSION);
        b.extend_from_slice(&self.seq.to_be_bytes());
        b.push(flags);
        b.extend_from_slice(&near.to_be_bytes());
        b.extend_from_slice(&far.to_be_bytes());
        b.push(id.text.len() as u8);
        b.extend_from_slice(id.text.as_bytes());
        let frames = if self.presence { &[][..] } else { &self.frames[..] };
        b.push(frames.len() as u8);
        for f in frames {
            b.extend_from_slice(&(f.len() as u16).to_be_bytes());
            b.extend_from_slice(f);
        }
        b
    }

    /// A plaintext -> the packet, or None (another version, cut short, bytes left over, too many frames, a bad
    /// range or id, presence with audio). Unknown flag bits are ignored (room for later versions).
    pub fn decode(b: &[u8]) -> Option<Packet> {
        if b.len() < HEAD + 1 || b[0] != VERSION {
            return None;
        }
        let seq = u32::from_be_bytes([b[1], b[2], b[3], b[4]]);
        let flags = b[5];
        let f32_at = |i: usize| f32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let range = match (f32_at(6), f32_at(10)) {
            (near, far) if near == 0.0 && far == 0.0 => None,
            r if good_range(r) => Some(r),
            _ => return None,
        };
        let n = b[14] as usize;
        let text = std::str::from_utf8(b.get(HEAD..HEAD + n)?).ok()?;
        let id = PlayerId { text: text.into(), number: flags & NUMBER != 0 };
        if !good_id(&id) {
            return None;
        }
        let mut at = HEAD + n;
        let k = *b.get(at)? as usize;
        let presence = flags & PRESENCE != 0;
        if k > MAX_FRAMES || (presence && k > 0) {
            return None;
        }
        at += 1;
        let mut frames = Vec::with_capacity(k);
        for _ in 0..k {
            let len = u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]) as usize;
            at += 2;
            frames.push(b.get(at..at + len)?.to_vec());
            at += len;
        }
        (at == b.len()).then_some(Packet { seq, last: flags & LAST != 0, presence, range, id, frames })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [2][seq][flags][near][far][n][id][k]
    fn head(flags: u8, near: f32, far: f32, id: &[u8], k: u8) -> Vec<u8> {
        let mut b = vec![2, 0, 0, 0, 1, flags];
        b.extend_from_slice(&near.to_be_bytes());
        b.extend_from_slice(&far.to_be_bytes());
        b.push(id.len() as u8);
        b.extend_from_slice(id);
        b.push(k);
        b
    }

    #[test]
    fn layout() {
        let p = Packet {
            seq: 0x01020304,
            last: true,
            range: Some((8.0, 25.0)),
            id: PlayerId::string("Ana"),
            frames: vec![vec![9, 8], vec![], vec![7]],
            ..Default::default()
        };
        let b = p.encode();
        let mut want = vec![2, 1, 2, 3, 4, 1];
        want.extend_from_slice(&[0x41, 0, 0, 0, 0x41, 0xc8, 0, 0]); // (8.0, 25.0)
        want.extend_from_slice(&[3, b'A', b'n', b'a', 3, 0, 2, 9, 8, 0, 0, 0, 1, 7]);
        assert_eq!(b, want);
        assert_eq!(Packet::decode(&b), Some(p));
        let q = Packet { seq: u32::MAX, frames: vec![vec![5; 300]], ..Default::default() };
        assert_eq!(Packet::decode(&q.encode()), Some(q));
        // a number id: flag 4
        let r = Packet { id: PlayerId::number(76561198000000002), frames: vec![vec![1]], ..Default::default() };
        let rb = r.encode();
        assert_eq!(rb[5], NUMBER);
        assert_eq!(Packet::decode(&rb), Some(r));
        // malformed: refused
        assert!(Packet::decode(&b[..b.len() - 1]).is_none(), "cut short");
        assert!(Packet::decode(&[b.as_slice(), &[0]].concat()).is_none(), "a byte left over");
        let mut v1 = b.clone();
        v1[0] = 1;
        assert!(Packet::decode(&v1).is_none(), "another version");
        assert!(Packet::decode(&head(0, 0.0, 0.0, b"", 0)[..15]).is_none(), "no frame count");
        assert!(Packet::decode(&head(0, 0.0, 0.0, b"", 17)).is_none(), "too many frames");
        assert!(Packet::decode(&[head(0, 0.0, 0.0, b"", 1), vec![0]].concat()).is_none());
        assert!(Packet::decode(&head(0, 0.0, 0.0, b"abc", 0)[..17]).is_none(), "the id cut short");
        // (flag bits it does not know: ignored)
        assert!(!Packet::decode(&head(0xf8, 0.0, 0.0, b"", 0)).unwrap().last);
        assert!(Packet::decode(&head(0xf9, 0.0, 0.0, b"", 0)).unwrap().last);
    }

    #[test]
    fn presence() {
        let p = Packet { seq: 1, presence: true, range: Some((1.0, 4.0)), id: PlayerId::string("cy"), ..Default::default() };
        let b = p.encode();
        assert_eq!(b, head(PRESENCE, 1.0, 4.0, b"cy", 0));
        assert_eq!(Packet::decode(&b), Some(p.clone()));
        // (presence carries no audio: frames are not sent, and a presence packet with frames is refused)
        let with = Packet { frames: vec![vec![1]], ..p.clone() };
        assert_eq!(Packet::decode(&with.encode()), Some(p));
        assert!(Packet::decode(&[head(PRESENCE, 0.0, 0.0, b"", 1), vec![0, 0]].concat()).is_none());
    }

    #[test]
    fn ranges_and_ids() {
        // (0, 0): none; a range it cannot carry goes as none
        assert_eq!(Packet::decode(&head(0, 0.0, 0.0, b"", 0)).unwrap().range, None);
        assert_eq!(Packet::decode(&head(0, 0.0, 30.0, b"", 0)).unwrap().range, Some((0.0, 30.0)));
        for (near, far) in [(5.0, 5.0), (9.0, 2.0), (-1.0, 3.0), (f32::NAN, 3.0), (0.0, f32::INFINITY)] {
            assert!(Packet::decode(&head(0, near, far, b"", 0)).is_none(), "{near} {far}");
            let p = Packet { range: Some((near, far)), ..Default::default() };
            assert_eq!(Packet::decode(&p.encode()).unwrap().range, None);
        }
        // ids: 64 characters at most, UTF-8, no control characters; a number as an i64's digits
        let long = "é".repeat(64);
        assert_eq!(Packet::decode(&head(0, 0.0, 0.0, long.as_bytes(), 0)).unwrap().id.text, long);
        assert!(Packet::decode(&head(0, 0.0, 0.0, "a".repeat(65).as_bytes(), 0)).is_none());
        assert!(Packet::decode(&head(0, 0.0, 0.0, &[0xff, 0xfe], 0)).is_none(), "not UTF-8");
        assert!(Packet::decode(&head(0, 0.0, 0.0, b"a\nb", 0)).is_none());
        assert!(Packet::decode(&head(NUMBER, 0.0, 0.0, b"-12", 0)).unwrap().id == PlayerId::number(-12));
        for bad in [&b"12a"[..], b"007", b"", b"99999999999999999999"] {
            assert!(Packet::decode(&head(NUMBER, 0.0, 0.0, bad, 0)).is_none(), "{bad:?}");
        }
        // (an id that does not fit - 64 four-byte characters are 256 bytes - goes as none)
        let wide = Packet { id: PlayerId::string(&"😀".repeat(64)), ..Default::default() };
        assert_eq!(Packet::decode(&wide.encode()).unwrap().id, PlayerId::default());
    }
}
