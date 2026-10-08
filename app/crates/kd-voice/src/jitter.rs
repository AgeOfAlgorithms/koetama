//! One sender's voice as it arrives -> steady audio (PROTOCOL.md "Playing"). Packets come late, out of order or not at
//! all; the audio output takes the voice at a steady pace. A stretch of talking starts once START is buffered and HOLD
//! has passed since its first packet arrived (a cushion: packets come a packet's time apart, give or take); a
//! missing packet is concealed by Opus frame by frame (if it turns up within one packet's time it still plays: the
//! voice then runs that much later), then given up; more than MAX buffered drops the oldest audio (the delay never
//! grows past it). A stretch ends after its last packet, or END_AFTER s without packets.
use crate::codec::FrameDecoder;
use crate::packet::Packet;
use crate::{FRAME, PER_PACKET, RATE};
use std::collections::VecDeque;

/// samples buffered before a stretch starts playing (60 ms)
pub const START: usize = (RATE as usize) * 60 / 1000;
/// s after a stretch's first packet before it plays: room for the next packet to come a little late (the sender's
/// microphone blocks and the network both move packets by tens of ms; without it the buffer ran dry and concealed)
pub const HOLD: f64 = 0.04;
/// samples buffered at most (300 ms): older audio is dropped
pub const MAX: usize = (RATE as usize) * 300 / 1000;
/// s without a packet: the stretch is over
pub const END_AFTER: f64 = 0.5;
/// a seq this far from the expected one: the sender started over (a restart), not a late packet
const RESTART: i32 = 1000;

/// b - a, wrapping (seq numbers wrap at 2^32)
fn diff(b: u32, a: u32) -> i32 {
    b.wrapping_sub(a) as i32
}

pub struct Jitter<D: FrameDecoder> {
    dec: D,
    /// packets waiting to play, any order
    packets: Vec<Packet>,
    /// the next seq to play (None: no stretch playing)
    next: Option<u32>,
    /// the last seq played or given up (a late packet of it is dropped)
    played: Option<u32>,
    /// decoded, not played yet
    pcm: VecDeque<f32>,
    /// frames concealed in place of `next` so far
    concealed: usize,
    /// the stretch's last packet is decoded: it ends when pcm runs out
    ending: bool,
    /// when the last packet arrived (s)
    last_t: f64,
    /// when the first packet of the stretch to come arrived (s; the HOLD before it plays)
    first_t: Option<f64>,
    frame: Vec<f32>,
    /// packets that came too late, duplicates, packets given up, packets dropped as too much (counts, for tests)
    pub late: u64,
    pub lost: u64,
    pub dropped: u64,
}

impl<D: FrameDecoder> Jitter<D> {
    pub fn new(dec: D) -> Jitter<D> {
        Jitter {
            dec,
            packets: Vec::new(),
            next: None,
            played: None,
            pcm: VecDeque::new(),
            concealed: 0,
            ending: false,
            last_t: f64::NEG_INFINITY,
            first_t: None,
            frame: vec![0.0; FRAME],
            late: 0,
            lost: 0,
            dropped: 0,
        }
    }

    /// Samples waiting: decoded, and in packets.
    pub fn buffered(&self) -> usize {
        self.pcm.len() + self.packets.iter().map(|p| p.frames.len() * FRAME).sum::<usize>()
    }

    /// A stretch is playing.
    pub fn playing(&self) -> bool {
        self.next.is_some()
    }

    /// When the last packet arrived (s; -inf: none yet).
    pub fn last_arrival(&self) -> f64 {
        self.last_t
    }

    /// A packet arrived at `now` (s).
    pub fn insert(&mut self, p: Packet, now: f64) {
        self.last_t = now;
        if self.next.is_none() && self.first_t.is_none() {
            self.first_t = Some(now);
        }
        let reference = self.next.or(self.played.map(|s| s.wrapping_add(1)));
        if let Some(r) = reference {
            let d = diff(p.seq, r);
            if !(-RESTART..=RESTART).contains(&d) {
                // (the sender started over: what was going on is dropped)
                self.restart();
            } else if d < 0 {
                self.late += 1;
                return;
            }
        }
        if self.packets.iter().any(|q| q.seq == p.seq) {
            self.late += 1;
            return;
        }
        self.packets.push(p);
        self.trim();
    }

    fn restart(&mut self) {
        self.first_t = None;
        self.packets.clear();
        self.pcm.clear();
        self.next = None;
        self.played = None;
        self.concealed = 0;
        self.ending = false;
    }

    /// The earliest packet waiting (by seq, relative to the next one to play or the first waiting).
    fn earliest(&self) -> Option<usize> {
        let base = self.next.or(self.packets.first().map(|p| p.seq))?;
        (0..self.packets.len()).min_by_key(|&i| diff(self.packets[i].seq, base))
    }

    /// At most MAX buffered: the oldest audio goes.
    fn trim(&mut self) {
        while self.buffered() > MAX {
            let over = self.buffered() - MAX;
            if !self.pcm.is_empty() {
                let n = over.min(self.pcm.len());
                self.pcm.drain(..n);
                continue;
            }
            let Some(i) = self.earliest() else { return };
            let p = self.packets.remove(i);
            self.dropped += 1;
            self.played = Some(p.seq);
            if self.next.is_some() {
                self.next = Some(p.seq.wrapping_add(1));
                self.concealed = 0;
            }
        }
    }

    /// The next out.len() samples at `now` (s) into out (silence where there is none); false: nothing played.
    pub fn pull(&mut self, out: &mut [f32], now: f64) -> bool {
        if self.next.is_none() {
            let complete = self.packets.iter().any(|p| p.last);
            let held = self.first_t.is_some_and(|t| now - t >= HOLD);
            if self.packets.is_empty() || ((self.buffered() < START || !held) && !complete) {
                out.fill(0.0);
                return false;
            }
            self.first_t = None;
            // (a stretch starts)
            let i = self.earliest().unwrap_or(0);
            self.next = Some(self.packets[i].seq);
            self.concealed = 0;
            self.ending = false;
        }
        while self.pcm.len() < out.len() && !self.ending {
            let Some(n) = self.next else { break };
            if let Some(i) = self.packets.iter().position(|p| p.seq == n) {
                let p = self.packets.remove(i);
                for f in &p.frames {
                    self.dec.decode((!f.is_empty()).then_some(f.as_slice()), &mut self.frame);
                    self.pcm.extend(self.frame.iter().copied());
                }
                self.played = Some(n);
                self.next = Some(n.wrapping_add(1));
                self.concealed = 0;
                self.ending = p.last;
            } else if now - self.last_t > END_AFTER {
                // (the sender went quiet without a last packet)
                self.ending = true;
            } else {
                // (missing: conceal a frame; a packet's time without it: given up)
                self.dec.decode(None, &mut self.frame);
                self.pcm.extend(self.frame.iter().copied());
                self.concealed += 1;
                if self.concealed >= PER_PACKET {
                    self.lost += 1;
                    self.played = Some(n);
                    self.next = Some(n.wrapping_add(1));
                    self.concealed = 0;
                }
            }
        }
        let k = self.pcm.len().min(out.len());
        for (o, s) in out.iter_mut().zip(self.pcm.drain(..k)) {
            *o = s;
        }
        out[k..].fill(0.0);
        if self.ending && self.pcm.is_empty() {
            // (the stretch is over)
            self.next = None;
            self.ending = false;
        }
        k > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in decoder: a frame's first byte as its every sample; a concealed frame is -1.
    struct Fake;
    impl FrameDecoder for Fake {
        fn decode(&mut self, frame: Option<&[u8]>, out: &mut [f32]) {
            out.fill(frame.map_or(-1.0, |f| f[0] as f32));
        }
    }

    /// packet seq: three frames whose values are seq*10 + 0, 1, 2
    fn pk(seq: u32, last: bool) -> Packet {
        Packet { seq, last, frames: (0..3).map(|i| vec![(seq * 10 + i) as u8]).collect() }
    }

    /// pull n frames' worth, one frame at a time -> each frame's value (None: silence)
    fn play(j: &mut Jitter<Fake>, frames: usize, now: f64) -> Vec<Option<f32>> {
        let mut out = vec![0.0; FRAME];
        (0..frames).map(|_| j.pull(&mut out, now).then_some(out[0])).collect()
    }

    #[test]
    fn in_order_and_out_of_order() {
        let mut j = Jitter::new(Fake);
        let mut out = vec![0.0; FRAME];
        assert!(!j.pull(&mut out, 0.0), "nothing yet");
        // (out of order: 2 before 1)
        j.insert(pk(2, false), 0.0);
        j.insert(pk(1, false), 0.0);
        j.insert(pk(3, true), 0.0);
        let got = play(&mut j, 10, 0.1);
        let want: Vec<Option<f32>> =
            [10.0, 11.0, 12.0, 20.0, 21.0, 22.0, 30.0, 31.0, 32.0].iter().map(|&v| Some(v)).chain([None]).collect();
        assert_eq!(got, want);
        assert!(!j.playing(), "ended after the last packet");
        // a late packet of a stretch already played: dropped
        j.insert(pk(2, false), 0.2);
        assert_eq!(j.late, 1);
        assert_eq!(play(&mut j, 2, 0.2), vec![None, None]);
        // the next stretch goes on from there
        j.insert(pk(4, true), 0.3);
        assert_eq!(play(&mut j, 4, 0.3), vec![Some(40.0), Some(41.0), Some(42.0), None]);
    }

    #[test]
    fn a_missing_packet_is_concealed_then_given_up() {
        let mut j = Jitter::new(Fake);
        j.insert(pk(1, false), 0.0);
        j.insert(pk(3, false), 0.0);
        // 1 plays, 2 is missing: three frames concealed (-1), then 3
        let got = play(&mut j, 9, 0.05);
        assert_eq!(got, [10.0, 11.0, 12.0, -1.0, -1.0, -1.0, 30.0, 31.0, 32.0].map(Some).to_vec());
        assert_eq!(j.lost, 1);
        // 2 turning up now is too late
        j.insert(pk(2, false), 0.1);
        assert_eq!(j.late, 1);
    }

    #[test]
    fn a_packet_a_little_late_still_plays() {
        let mut j = Jitter::new(Fake);
        j.insert(pk(1, false), 0.0);
        assert_eq!(play(&mut j, 4, 0.05), [10.0, 11.0, 12.0, -1.0].map(Some).to_vec());
        // 2 arrives one frame late: it plays (the voice now runs 20 ms later)
        j.insert(pk(2, true), 0.08);
        assert_eq!(play(&mut j, 4, 0.08), vec![Some(20.0), Some(21.0), Some(22.0), None]);
        assert_eq!((j.lost, j.late), (0, 0));
    }

    #[test]
    fn a_stretch_waits_for_its_cushion() {
        let mut j = Jitter::new(Fake);
        j.insert(pk(1, false), 1.0);
        // (60 ms buffered, but only 20 ms since it came: not yet - the next packet may come a little late)
        assert_eq!(play(&mut j, 1, 1.02), vec![None]);
        j.insert(pk(2, false), 1.03);
        assert_eq!(play(&mut j, 4, 1.0 + HOLD), [10.0, 11.0, 12.0, 20.0].map(Some).to_vec(), "then it plays");
        // (a stretch that is complete plays at once: there is nothing more to wait for)
        let mut k = Jitter::new(Fake);
        k.insert(pk(1, true), 2.0);
        assert_eq!(play(&mut k, 1, 2.0), vec![Some(10.0)]);
    }

    #[test]
    fn at_most_300_ms_and_the_end_after_silence() {
        let mut j = Jitter::new(Fake);
        // (nobody pulling: 8 packets = 480 ms arrive; only the newest 300 ms are kept)
        for s in 1..=8 {
            j.insert(pk(s, false), 0.0);
        }
        assert!(j.buffered() <= MAX && j.dropped == 3, "{} {}", j.buffered(), j.dropped);
        let got = play(&mut j, 2, 0.05);
        assert_eq!(got, vec![Some(40.0), Some(41.0)], "the oldest went");
        // no more packets: played out, then concealed until END_AFTER, then over
        play(&mut j, 13, 0.1);
        assert!(j.playing());
        let mut out = vec![0.0; FRAME];
        while j.pull(&mut out, 0.7) {}
        assert!(!j.playing());
        assert!(!j.pull(&mut out, 0.7) && out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn a_restarted_sender_and_wrapping_seq() {
        let mut j = Jitter::new(Fake);
        j.insert(Packet { seq: u32::MAX, last: false, frames: vec![vec![1]; 3] }, 0.0);
        j.insert(Packet { seq: 0, last: true, frames: vec![vec![2]; 3] }, 0.0);
        let got = play(&mut j, 7, 0.0);
        assert_eq!(got, [1.0, 1.0, 1.0, 2.0, 2.0, 2.0].map(Some).into_iter().chain([None]).collect::<Vec<_>>());
        // (the sender's Koetama started over: its seq is far from the last one) - not late, a new start
        j.insert(Packet { seq: 2_000_000, last: true, frames: vec![vec![7]; 3] }, 1.0);
        assert_eq!(j.late, 0);
        assert_eq!(play(&mut j, 3, 1.0), [7.0, 7.0, 7.0].map(Some).to_vec());
    }

    #[test]
    fn short_stretch_and_odd_pull_sizes() {
        let mut j = Jitter::new(Fake);
        // (one frame, last: under START but complete - it plays)
        j.insert(Packet { seq: 9, last: true, frames: vec![vec![5]] }, 0.0);
        let mut out = vec![0.0; 700];
        assert!(j.pull(&mut out, 0.0) && out.iter().all(|&s| s == 5.0));
        assert!(j.pull(&mut out, 0.0) && out[..260].iter().all(|&s| s == 5.0) && out[260..].iter().all(|&s| s == 0.0));
        assert!(!j.pull(&mut out, 0.0) && !j.playing());
    }
}
