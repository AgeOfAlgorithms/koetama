//! Mozilla's model files: the Marian binary model (model.<pair>.intgemm.alphas.bin) and the lexical shortlist
//! (lex.50.50.<pair>.s2t.bin).
//!
//! The model (marian-dev src/common/binary.cpp): u64 version, u64 count, count headers of 4 x u64 (name length with its
//! NUL, type, shape length, data length), the names, the shapes (i32), a u64 padding length and the padding, then each
//! tensor's data. Types: 0x404 float32; 0x4101 intgemm8 - the int8 values stored transposed (a matrix (in, out) as
//! [out][in]; the embeddings as they are, one row per word), followed by their float multiplier (value = q / mult).
use std::collections::HashMap;

use super::gemm::Int8;

pub enum Tensor {
    F32 { data: Vec<f32> },
    I8(Int8),
}

const FLOAT32: u64 = 0x404;
const INTGEMM: u64 = 0x4000;

fn u64_at(b: &[u8], off: usize) -> Result<u64, String> {
    b.get(off..off + 8)
        .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
        .ok_or_else(|| "the model file is cut short".into())
}

fn f32s(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect()
}

/// The tensors by name, and the embedded config (special:model.yml).
pub fn read_model(b: &[u8]) -> Result<(HashMap<String, Tensor>, String), String> {
    let n = u64_at(b, 8)? as usize;
    if n > 100_000 {
        return Err("not a Marian model".into());
    }
    let mut off = 16;
    let mut heads = Vec::with_capacity(n);
    for _ in 0..n {
        heads.push([u64_at(b, off)?, u64_at(b, off + 8)?, u64_at(b, off + 16)?, u64_at(b, off + 24)?]);
        off += 32;
    }
    let mut names = Vec::with_capacity(n);
    for h in &heads {
        let len = h[0] as usize;
        let raw = b.get(off..off + len.saturating_sub(1)).ok_or("the model file is cut short")?;
        names.push(String::from_utf8_lossy(raw).into_owned());
        off += len;
    }
    let mut shapes = Vec::with_capacity(n);
    for h in &heads {
        let len = h[2] as usize;
        let raw = b.get(off..off + 4 * len).ok_or("the model file is cut short")?;
        shapes.push(raw.as_chunks::<4>().0.iter().map(|c| i32::from_le_bytes(*c).max(0) as usize).collect::<Vec<_>>());
        off += 4 * len;
    }
    let pad = u64_at(b, off)? as usize;
    off += 8 + pad;
    let mut out = HashMap::new();
    let mut config = String::new();
    for ((h, name), shape) in heads.iter().zip(names).zip(shapes) {
        let (kind, len) = (h[1], h[3] as usize);
        let raw = b.get(off..off + len).ok_or("the model file is cut short")?;
        off += len;
        let count: usize = shape.iter().product();
        if name == "special:model.yml" {
            config = String::from_utf8_lossy(raw).trim_end_matches('\0').to_string();
        } else if kind & INTGEMM != 0 {
            if name.ends_with("_QuantMultA") || shape.len() != 2 {
                continue; // (an alpha stored with the 8-bit type: not one this engine reads)
            }
            let q: Vec<i8> = raw.get(..count).ok_or("an 8-bit tensor is cut short")?.iter().map(|&v| v as i8).collect();
            let m = raw.get(count..count + 4).ok_or("an 8-bit tensor has no multiplier")?;
            let mult = f32::from_le_bytes(m.try_into().unwrap());
            let int8 = if name.ends_with("Wemb") {
                Int8 { rows: shape[0], cols: shape[1], q, mult }
            } else {
                Int8 { rows: shape[1], cols: shape[0], q, mult }
            };
            out.insert(name, Tensor::I8(int8));
        } else if kind == FLOAT32 {
            let data = f32s(raw.get(..4 * count).ok_or("a float tensor is cut short")?);
            out.insert(name, Tensor::F32 { data });
        }
    }
    Ok((out, config))
}

/// The top-level `key: value` settings of special:model.yml.
pub fn parse_config(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter(|l| !l.starts_with(' ') && !l.starts_with('-'))
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// The lexical shortlist (marian-dev src/data/shortlist.h, BinaryShortlistGenerator): the target words worth scoring
/// for a sentence - the `first` commonest ones, plus each source word's best.
pub struct Shortlist {
    first: usize,
    offsets: Vec<u64>,
    lists: Vec<u32>,
}

const SHORTLIST_MAGIC: u64 = 0xF11A48D5013417F5;

impl Shortlist {
    pub fn read(b: &[u8]) -> Result<Shortlist, String> {
        if u64_at(b, 0)? != SHORTLIST_MAGIC {
            return Err("not a binary shortlist".into());
        }
        let first = u64_at(b, 16)? as usize;
        let n_off = u64_at(b, 32)? as usize;
        let n_lists = u64_at(b, 40)? as usize;
        let start = 48;
        let lists_at = start + 8 * n_off;
        let offsets: Vec<u64> = b
            .get(start..lists_at)
            .ok_or("the shortlist is cut short")?
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| u64::from_le_bytes(*c))
            .collect();
        let lists: Vec<u32> = b
            .get(lists_at..lists_at + 4 * n_lists)
            .ok_or("the shortlist is cut short")?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect();
        Ok(Shortlist { first, offsets, lists })
    }

    /// The target ids to score for source `ids` (ascending): the commonest, each source word's list, the source words
    /// themselves when the two sides share a vocabulary, padded to a multiple of 8 as Marian pads it.
    pub fn select(&self, ids: &[u32], vocab: usize, shared: bool) -> Vec<u32> {
        let mut sel = vec![false; vocab];
        sel[..self.first.min(vocab)].iter_mut().for_each(|s| *s = true);
        for &w in ids {
            let w = w as usize;
            if shared && w < vocab {
                sel[w] = true;
            }
            if w + 1 < self.offsets.len() {
                let (a, b) = (self.offsets[w] as usize, self.offsets[w + 1] as usize);
                for &t in self.lists.get(a..b).unwrap_or(&[]) {
                    if (t as usize) < vocab {
                        sel[t as usize] = true;
                    }
                }
            }
        }
        let mut n = sel.iter().filter(|&&s| s).count();
        let mut i = self.first;
        while n % 8 != 0 && i < vocab {
            if !sel[i] {
                sel[i] = true;
                n += 1;
            }
            i += 1;
        }
        sel.iter().enumerate().filter(|(_, &s)| s).map(|(i, _)| i as u32).collect()
    }

    pub fn bytes(&self) -> usize {
        self.offsets.len() * 8 + self.lists.len() * 4
    }
}
