//! SentencePiece (google/sentencepiece), the part Mozilla's vocabularies use: a unigram model read from the .spm file
//! (a protobuf ModelProto), its normalizer (the precompiled nmt_nfkc character map, extra spaces removed, spaces as
//! U+2581 and one in front), the best split by the pieces' scores (Viterbi), unknown characters as their UTF-8 bytes
//! (byte fallback), and the way back from ids to text. Tested against Python's sentencepiece (app/fixtures/mt.json).
use std::collections::HashMap;

const SPACE: &str = "\u{2581}";
const UNK_PENALTY: f32 = 10.0;
const USER_DEFINED_BONUS: f32 = 0.1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Normal,
    Unknown,
    Control,
    UserDefined,
    Unused,
    Byte(u8),
}

pub struct Vocab {
    pieces: Vec<(String, f32, Kind)>,
    index: HashMap<String, u32>,
    /// the longest piece, in bytes
    longest: usize,
    unk: u32,
    bytes: [u32; 256],
    byte_fallback: bool,
    min_score: f32,
    max_score: f32,
    unk_surface: String,
    norm: Normalizer,
}

// ------------------------------------------------------------------ the protobuf
struct Proto<'a> {
    b: &'a [u8],
    at: usize,
}

enum Field<'a> {
    Varint(u64),
    Fixed32(u32),
    Bytes(&'a [u8]),
    Other,
}

impl<'a> Proto<'a> {
    fn new(b: &'a [u8]) -> Self {
        Proto { b, at: 0 }
    }

    fn varint(&mut self) -> Result<u64, String> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self.b.get(self.at).ok_or("the vocabulary file is cut short")?;
            self.at += 1;
            v |= ((byte & 0x7F) as u64) << shift;
            if byte & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err("a bad number in the vocabulary file".into())
    }

    fn next(&mut self) -> Result<Option<(u64, Field<'a>)>, String> {
        if self.at >= self.b.len() {
            return Ok(None);
        }
        let key = self.varint()?;
        let field = match key & 7 {
            0 => Field::Varint(self.varint()?),
            1 => {
                self.at += 8;
                Field::Other
            }
            2 => {
                let len = self.varint()? as usize;
                let s = self.b.get(self.at..self.at + len).ok_or("the vocabulary file is cut short")?;
                self.at += len;
                Field::Bytes(s)
            }
            5 => {
                let s = self.b.get(self.at..self.at + 4).ok_or("the vocabulary file is cut short")?;
                self.at += 4;
                Field::Fixed32(u32::from_le_bytes(s.try_into().unwrap()))
            }
            _ => return Err("not a SentencePiece vocabulary".into()),
        };
        Ok(Some((key >> 3, field)))
    }
}

impl Vocab {
    pub fn read(b: &[u8]) -> Result<Vocab, String> {
        let mut pieces = Vec::new();
        let (mut model_type, mut byte_fallback, mut unk_surface) = (1u64, false, " \u{2047} ".to_string());
        let (mut charsmap, mut dummy_prefix, mut squeeze, mut escape) = (Vec::new(), true, true, true);
        let mut top = Proto::new(b);
        while let Some((num, f)) = top.next()? {
            match (num, f) {
                (1, Field::Bytes(m)) => {
                    let (mut piece, mut score, mut kind) = (String::new(), 0f32, 1u64);
                    let mut p = Proto::new(m);
                    while let Some((n, f)) = p.next()? {
                        match (n, f) {
                            (1, Field::Bytes(s)) => piece = String::from_utf8_lossy(s).into_owned(),
                            (2, Field::Fixed32(v)) => score = f32::from_bits(v),
                            (3, Field::Varint(v)) => kind = v,
                            _ => {}
                        }
                    }
                    let kind = match kind {
                        2 => Kind::Unknown,
                        3 => Kind::Control,
                        4 => Kind::UserDefined,
                        5 => Kind::Unused,
                        6 => Kind::Byte(byte_of(&piece).ok_or("a bad byte piece")?),
                        _ => Kind::Normal,
                    };
                    pieces.push((piece, score, kind));
                }
                (2, Field::Bytes(m)) => {
                    let mut p = Proto::new(m);
                    while let Some((n, f)) = p.next()? {
                        match (n, f) {
                            (3, Field::Varint(v)) => model_type = v,
                            (35, Field::Varint(v)) => byte_fallback = v != 0,
                            (44, Field::Bytes(s)) => unk_surface = String::from_utf8_lossy(s).into_owned(),
                            _ => {}
                        }
                    }
                }
                (3, Field::Bytes(m)) => {
                    let mut p = Proto::new(m);
                    while let Some((n, f)) = p.next()? {
                        match (n, f) {
                            (2, Field::Bytes(s)) => charsmap = s.to_vec(),
                            (3, Field::Varint(v)) => dummy_prefix = v != 0,
                            (4, Field::Varint(v)) => squeeze = v != 0,
                            (5, Field::Varint(v)) => escape = v != 0,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        if model_type != 1 {
            return Err("only unigram SentencePiece vocabularies are supported".into());
        }
        let unk = pieces.iter().position(|p| p.2 == Kind::Unknown).ok_or("the vocabulary has no unknown piece")? as u32;
        let mut index = HashMap::new();
        let mut bytes = [unk; 256];
        let (mut min_score, mut max_score, mut longest) = (f32::MAX, f32::MIN, 1);
        for (i, (piece, score, kind)) in pieces.iter().enumerate() {
            match kind {
                Kind::Normal | Kind::UserDefined | Kind::Unused => {
                    index.insert(piece.clone(), i as u32);
                    longest = longest.max(piece.len());
                }
                Kind::Byte(v) => bytes[*v as usize] = i as u32,
                _ => {}
            }
            if *kind == Kind::Normal {
                min_score = min_score.min(*score);
                max_score = max_score.max(*score);
            }
        }
        let norm = Normalizer::new(&charsmap, dummy_prefix, squeeze, escape)?;
        Ok(Vocab { pieces, index, longest, unk, bytes, byte_fallback, min_score, max_score, unk_surface, norm })
    }

    pub fn len(&self) -> usize {
        self.pieces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    /// The memory this holds, roughly.
    pub fn bytes(&self) -> usize {
        self.pieces.iter().map(|p| p.0.len() * 2 + 64).sum::<usize>() + self.norm.bytes()
    }

    /// Is this id's piece the start of a word (it begins with the space mark)?
    pub fn starts_word(&self, id: u32) -> bool {
        self.pieces.get(id as usize).is_some_and(|p| p.0.starts_with(SPACE))
    }

    /// Text -> ids (no end-of-sentence id).
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let s = self.norm.normalize(text);
        if s.is_empty() {
            return Vec::new();
        }
        let sb = s.as_bytes();
        let n = sb.len();
        // best[end] = (score, start, id) of the best split of s[..end]
        let mut best: Vec<(f32, usize, u32)> = vec![(0.0, usize::MAX, 0); n + 1];
        best[0].1 = 0;
        let unk_score = self.min_score - UNK_PENALTY;
        let mut start = 0;
        while start < n {
            let here = best[start].0;
            let ch = char_len(sb[start]).min(n - start);
            let mut single = false;
            let mut end = start;
            while end < n && end - start < self.longest {
                end += char_len(sb[end]).min(n - end);
                if end - start > self.longest {
                    break;
                }
                let Some(&id) = self.index.get(&s[start..end]) else {
                    continue;
                };
                let (_, score, kind) = &self.pieces[id as usize];
                if *kind == Kind::Unused {
                    continue;
                }
                let score = if *kind == Kind::UserDefined {
                    (end - start) as f32 * self.max_score - USER_DEFINED_BONUS
                } else {
                    *score
                } + here;
                if end - start == ch {
                    single = true;
                }
                let t = &mut best[end];
                if t.1 == usize::MAX || score > t.0 {
                    *t = (score, start, id);
                }
            }
            if !single {
                let t = &mut best[start + ch];
                let score = unk_score + here;
                if t.1 == usize::MAX || score > t.0 {
                    *t = (score, start, self.unk);
                }
            }
            start += ch;
        }
        let mut path = Vec::new();
        let mut end = n;
        while end > 0 {
            let (_, start, id) = best[end];
            path.push((start, end, id));
            end = start;
        }
        path.reverse();
        let mut ids = Vec::with_capacity(path.len());
        let mut prev_unk = false;
        for (start, end, id) in path {
            let unk = id == self.unk;
            if unk && self.byte_fallback {
                ids.extend(sb[start..end].iter().map(|&b| self.bytes[b as usize]));
            } else if !(unk && prev_unk) {
                ids.push(id);
            }
            prev_unk = unk;
        }
        ids
    }

    /// Ids -> text (control ids such as the end of sentence give nothing).
    pub fn decode(&self, ids: &[u32]) -> String {
        let mut out = String::new();
        let mut bos = true;
        let mut pending: Vec<u8> = Vec::new();
        let emit = |out: &mut String, bos: &mut bool, text: &str| {
            out.push_str(text);
            if !text.is_empty() {
                *bos = false;
            }
        };
        let flush = |out: &mut String, bos: &mut bool, pending: &mut Vec<u8>| {
            let mut at = 0;
            while at < pending.len() {
                match std::str::from_utf8(&pending[at..]) {
                    Ok(s) => {
                        emit(out, bos, s);
                        at = pending.len();
                    }
                    Err(e) if e.valid_up_to() > 0 => {
                        emit(out, bos, std::str::from_utf8(&pending[at..at + e.valid_up_to()]).unwrap());
                        at += e.valid_up_to();
                    }
                    Err(_) => {
                        emit(out, bos, "\u{FFFD}");
                        at += 1;
                    }
                }
            }
            pending.clear();
        };
        for &id in ids {
            let Some((piece, _, kind)) = self.pieces.get(id as usize) else {
                continue;
            };
            if let Kind::Byte(v) = kind {
                pending.push(*v);
                continue;
            }
            flush(&mut out, &mut bos, &mut pending);
            let text = match kind {
                Kind::Control => String::new(),
                Kind::Unknown => self.unk_surface.clone(),
                _ => {
                    let p = if bos && (self.norm.dummy_prefix || self.norm.squeeze) {
                        piece.strip_prefix(SPACE).unwrap_or(piece)
                    } else {
                        piece
                    };
                    p.replace(SPACE, " ")
                }
            };
            emit(&mut out, &mut bos, &text);
        }
        flush(&mut out, &mut bos, &mut pending);
        out
    }
}

/// "<0x41>" -> 0x41
fn byte_of(piece: &str) -> Option<u8> {
    u8::from_str_radix(piece.strip_prefix("<0x")?.strip_suffix('>')?, 16).ok()
}

/// The length of the UTF-8 character a lead byte starts (1 for a stray byte).
fn char_len(lead: u8) -> usize {
    match lead {
        0xF0..=0xF7 => 4,
        0xE0..=0xEF => 3,
        0xC0..=0xDF => 2,
        _ => 1,
    }
}

// ------------------------------------------------------------------ the normalizer
/// SentencePiece's normalizer (normalizer.cc): the precompiled character map - a Darts double-array trie over the
/// input's bytes whose values point at NUL-terminated replacements - applied longest match first.
struct Normalizer {
    trie: Vec<u32>,
    replaced: Vec<u8>,
    dummy_prefix: bool,
    squeeze: bool,
    escape: bool,
}

impl Normalizer {
    fn new(charsmap: &[u8], dummy_prefix: bool, squeeze: bool, escape: bool) -> Result<Normalizer, String> {
        let (mut trie, mut replaced) = (Vec::new(), Vec::new());
        if charsmap.len() >= 4 {
            let size = u32::from_le_bytes(charsmap[..4].try_into().unwrap()) as usize;
            let blob = charsmap.get(4..4 + size).ok_or("a bad character map in the vocabulary")?;
            trie = blob.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect();
            replaced = charsmap[4 + size..].to_vec();
        }
        Ok(Normalizer { trie, replaced, dummy_prefix, squeeze, escape })
    }

    fn bytes(&self) -> usize {
        self.trie.len() * 4 + self.replaced.len()
    }

    /// The longest rule matching the start of `s`: (bytes matched, replacement).
    fn longest(&self, s: &[u8]) -> Option<(usize, &[u8])> {
        let t = &self.trie;
        if t.is_empty() {
            return None;
        }
        let offset = |u: u32| ((u >> 10) << ((u & (1 << 9)) >> 6)) as usize;
        let mut pos = offset(t[0]);
        let mut found = None;
        for (i, &c) in s.iter().enumerate() {
            pos ^= c as usize;
            let Some(&unit) = t.get(pos) else { break };
            if unit & ((1 << 31) | 0xFF) != c as u32 {
                break;
            }
            pos ^= offset(unit);
            if (unit >> 8) & 1 == 1 {
                let value = (t.get(pos).copied().unwrap_or(0) & 0x7FFF_FFFF) as usize;
                found = Some((i + 1, value));
            }
        }
        let (len, value) = found?;
        let rest = self.replaced.get(value..)?;
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        Some((len, &rest[..end]))
    }

    /// The next piece of input normalized: (its replacement, bytes consumed).
    fn prefix<'a>(&'a self, s: &'a [u8]) -> (&'a [u8], usize) {
        if let Some((len, rep)) = self.longest(s) {
            return (rep, len);
        }
        match std::str::from_utf8(&s[..char_len(s[0]).min(s.len())]) {
            Ok(c) => (c.as_bytes(), c.len()),
            Err(_) => ("\u{FFFD}".as_bytes(), 1),
        }
    }

    fn normalize(&self, text: &str) -> String {
        let mut input = text.as_bytes();
        let mut out: Vec<u8> = Vec::with_capacity(input.len() * 3 / 2 + 4);
        if self.squeeze {
            while !input.is_empty() {
                let (rep, used) = self.prefix(input);
                if rep != b" " {
                    break;
                }
                input = &input[used..];
            }
        }
        if input.is_empty() {
            return String::new();
        }
        if self.dummy_prefix {
            out.extend_from_slice(if self.escape { SPACE.as_bytes() } else { b" " });
        }
        let mut prev_space = self.squeeze;
        while !input.is_empty() {
            let (mut rep, used) = self.prefix(input);
            if prev_space && self.squeeze {
                while let Some(r) = rep.strip_prefix(b" ") {
                    rep = r;
                }
            }
            if !rep.is_empty() {
                for &b in rep {
                    if self.escape && b == b' ' {
                        out.extend_from_slice(SPACE.as_bytes());
                    } else {
                        out.push(b);
                    }
                }
                prev_space = rep.ends_with(b" ");
            }
            input = &input[used..];
            if !self.squeeze {
                prev_space = false;
            }
        }
        if self.squeeze {
            let space: &[u8] = if self.escape { SPACE.as_bytes() } else { b" " };
            while out.ends_with(space) {
                out.truncate(out.len() - space.len());
            }
        }
        String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
    }
}
