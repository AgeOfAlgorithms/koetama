//! A voice packet's plaintext (PROTOCOL.md "Sending and receiving"):
//!   [1][seq: u32 big-endian][flags: u8, 1 = the last packet of a stretch of talking][k][k x (len: u16 BE, Opus bytes)]

pub const VERSION: u8 = 1;
/// flags: the last packet of a stretch of talking
pub const LAST: u8 = 1;
/// frames one packet may hold (Koetama sends PER_PACKET)
pub const MAX_FRAMES: usize = 16;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Packet {
    /// counts up by one per packet from a sender (wrapping)
    pub seq: u32,
    /// the last packet of a stretch of talking: the listener's voice ends after it
    pub last: bool,
    /// Opus frames, 20 ms each, in order
    pub frames: Vec<Vec<u8>>,
}

impl Packet {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(7 + self.frames.iter().map(|f| 2 + f.len()).sum::<usize>());
        b.push(VERSION);
        b.extend_from_slice(&self.seq.to_be_bytes());
        b.push(if self.last { LAST } else { 0 });
        b.push(self.frames.len() as u8);
        for f in &self.frames {
            b.extend_from_slice(&(f.len() as u16).to_be_bytes());
            b.extend_from_slice(f);
        }
        b
    }

    /// A plaintext -> the packet, or None (another version, cut short, bytes left over, too many frames).
    /// Unknown flag bits are ignored (room for later versions).
    pub fn decode(b: &[u8]) -> Option<Packet> {
        if b.len() < 7 || b[0] != VERSION {
            return None;
        }
        let seq = u32::from_be_bytes([b[1], b[2], b[3], b[4]]);
        let last = b[5] & LAST != 0;
        let k = b[6] as usize;
        if k > MAX_FRAMES {
            return None;
        }
        let mut at = 7;
        let mut frames = Vec::with_capacity(k);
        for _ in 0..k {
            let n = u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]) as usize;
            at += 2;
            frames.push(b.get(at..at + n)?.to_vec());
            at += n;
        }
        (at == b.len()).then_some(Packet { seq, last, frames })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        let p = Packet { seq: 0x01020304, last: true, frames: vec![vec![9, 8], vec![], vec![7]] };
        let b = p.encode();
        assert_eq!(b, vec![1, 1, 2, 3, 4, 1, 3, 0, 2, 9, 8, 0, 0, 0, 1, 7]);
        assert_eq!(Packet::decode(&b), Some(p));
        let q = Packet { seq: u32::MAX, last: false, frames: vec![vec![5; 300]] };
        assert_eq!(Packet::decode(&q.encode()), Some(q));
        // malformed: refused
        assert!(Packet::decode(&b[..b.len() - 1]).is_none(), "cut short");
        assert!(Packet::decode(&[b.as_slice(), &[0]].concat()).is_none(), "a byte left over");
        assert!(Packet::decode(&[2, 0, 0, 0, 1, 0, 0]).is_none(), "another version");
        assert!(Packet::decode(&[1, 0, 0, 0, 1, 0]).is_none());
        assert!(Packet::decode(&[1, 0, 0, 0, 1, 0, 17]).is_none(), "too many frames");
        assert!(Packet::decode(&[1, 0, 0, 0, 1, 0, 1, 0]).is_none());
        // (flag bits it does not know: ignored)
        assert!(!Packet::decode(&[1, 0, 0, 0, 1, 0xfe, 0]).unwrap().last);
        assert!(Packet::decode(&[1, 0, 0, 0, 1, 0xff, 0]).unwrap().last);
    }
}
