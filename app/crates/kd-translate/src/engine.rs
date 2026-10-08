//! One translation direction: a folder with Mozilla's files (model.<pair>.intgemm.alphas.bin, lex.50.50.<pair>.s2t.bin,
//! vocab.<pair>.spm or srcvocab / trgvocab), run in plain Rust. engine/mt.py is the reference it is tested against.
//!
//! The model (Marian "transformer" students, browsermt/marian-dev src/models/transformer.h; each file's own config):
//!   encoder   embeddings x sqrt(dim) + sinusoidal positions, then enc-depth x [self-attention, feed-forward], each
//!             followed by "dan": add the input, layer-norm
//!   decoder   the previous word's embedding (zeros at the first step) x sqrt(dim) + the step's position, then
//!             dec-depth x [SSRU, attention over the encoder, feed-forward], each "dan"
//!   output    the target embeddings over the lexical shortlist, greedy
//! Every matrix product is 8-bit (gemm.rs). The weights stay 8-bit in memory, embeddings included (a row is turned
//! into floats when looked up), so a direction holds about its file's size.
mod gemm;
mod marian;
pub mod spm;

use std::collections::HashMap;
use std::path::Path;

use gemm::Int8;
use marian::{Shortlist, Tensor};
use spm::Vocab;

const LN_EPS: f32 = 1e-6;
const EOS: u32 = 0;
/// the longest piece of a sentence translated at once, in tokens (bergamot's max-length-break is 128)
const MAX_TOKENS: usize = 128;

pub use gemm::kernel_name;

#[doc(hidden)]
pub fn spm_for_tests(b: &[u8]) -> Result<Vocab, String> {
    Vocab::read(b)
}

struct Lin {
    w: Int8,
    alpha: Option<f32>,
    b: Option<Vec<f32>>,
}

struct Norm {
    scale: Vec<f32>,
    bias: Vec<f32>,
}

struct Attn {
    q: Lin,
    k: Lin,
    v: Lin,
    o: Lin,
    ln: Norm,
}

struct Ffn {
    w1: Lin,
    w2: Lin,
    ln: Norm,
}

struct Ssru {
    w: Lin,
    wf: Lin,
    ln: Norm,
}

struct DecLayer {
    rnn: Ssru,
    ctx: Attn,
    ffn: Ffn,
}

pub struct Model {
    dim: usize,
    heads: usize,
    src_emb: Option<Int8>,
    /// (the target embeddings: the decoder's input and, tied, its output layer; also the source's when shared)
    trg_emb: Int8,
    out_b: Vec<f32>,
    out_alpha: Option<f32>,
    enc: Vec<(Attn, Ffn)>,
    dec: Vec<DecLayer>,
    src_vocab: Vocab,
    trg_vocab: Option<Vocab>,
    shortlist: Option<Shortlist>,
}

struct Tensors(HashMap<String, Tensor>);

impl Tensors {
    fn int8(&mut self, name: &str) -> Result<Int8, String> {
        match self.0.remove(name) {
            Some(Tensor::I8(m)) => Ok(m),
            _ => Err(format!("the model has no 8-bit {name}")),
        }
    }

    fn floats(&mut self, name: &str) -> Result<Vec<f32>, String> {
        match self.0.remove(name) {
            Some(Tensor::F32 { data }) => Ok(data),
            _ => Err(format!("the model has no {name}")),
        }
    }

    fn scalar(&mut self, name: &str) -> Option<f32> {
        self.floats(name).ok().and_then(|v| v.first().copied())
    }

    /// <prefix>_W<x> with its alpha and bias <prefix>_b<x>
    fn lin(&mut self, prefix: &str, x: &str, bias: bool) -> Result<Lin, String> {
        let name = format!("{prefix}_W{x}");
        let w = self.int8(&name)?;
        let alpha = self.scalar(&format!("{name}_QuantMultA"));
        let b = if bias { Some(self.floats(&format!("{prefix}_b{x}"))?) } else { None };
        Ok(Lin { w, alpha, b })
    }

    fn norm(&mut self, prefix: &str) -> Result<Norm, String> {
        Ok(Norm {
            scale: self.floats(&format!("{prefix}_ln_scale"))?,
            bias: self.floats(&format!("{prefix}_ln_bias"))?,
        })
    }

    fn attn(&mut self, prefix: &str) -> Result<Attn, String> {
        Ok(Attn {
            q: self.lin(prefix, "q", true)?,
            k: self.lin(prefix, "k", true)?,
            v: self.lin(prefix, "v", true)?,
            o: self.lin(prefix, "o", true)?,
            ln: self.norm(&format!("{prefix}_Wo"))?,
        })
    }

    fn ffn(&mut self, prefix: &str) -> Result<Ffn, String> {
        Ok(Ffn {
            w1: self.lin(prefix, "1", true)?,
            w2: self.lin(prefix, "2", true)?,
            ln: self.norm(&format!("{prefix}_ffn"))?,
        })
    }
}

fn find(dir: &Path, prefix: &str, suffix: &str) -> Option<std::path::PathBuf> {
    let mut found: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(prefix) && n.ends_with(suffix))
        })
        .collect();
    found.sort();
    found.into_iter().next()
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

impl Model {
    /// Loads a direction's files (blocking; tens of ms).
    pub fn load(dir: &Path) -> Result<Model, String> {
        let bin = find(dir, "model.", ".bin").ok_or_else(|| format!("no model.*.bin in {}", dir.display()))?;
        let (tensors, config) = marian::read_model(&read(&bin)?)?;
        let cfg = marian::parse_config(&config);
        let get = |k: &str| -> Result<usize, String> {
            cfg.get(k).and_then(|v| v.parse().ok()).ok_or_else(|| format!("the model's config has no {k}"))
        };
        let (dim, heads, enc_depth, dec_depth) =
            (get("dim-emb")?, get("transformer-heads")?, get("enc-depth")?, get("dec-depth")?);
        for (k, want) in
            [("dec-cell", "ssru"), ("transformer-decoder-autoreg", "rnn"), ("transformer-postprocess", "dan")]
        {
            if cfg.get(k).map(String::as_str) != Some(want) {
                return Err(format!("unsupported model: {k} is not {want}"));
            }
        }
        if dim % heads != 0 || dim % 2 != 0 {
            return Err("unsupported model: odd dimensions".into());
        }
        let mut t = Tensors(tensors);
        let (src_emb, trg_emb) = if t.0.contains_key("Wemb") {
            (None, t.int8("Wemb")?)
        } else {
            (Some(t.int8("encoder_Wemb")?), t.int8("decoder_Wemb")?)
        };
        let out_b = t.floats("decoder_ff_logit_out_b")?;
        let out_alpha = t.scalar("none_QuantMultA");
        let mut enc = Vec::new();
        for i in 1..=enc_depth {
            enc.push((t.attn(&format!("encoder_l{i}_self"))?, t.ffn(&format!("encoder_l{i}_ffn"))?));
        }
        let mut dec = Vec::new();
        for i in 1..=dec_depth {
            let pre = format!("decoder_l{i}_rnn");
            dec.push(DecLayer {
                rnn: Ssru {
                    w: t.lin(&pre, "", false)?,
                    wf: t.lin(&pre, "f", true)?,
                    ln: t.norm(&format!("{pre}_ffn"))?,
                },
                ctx: t.attn(&format!("decoder_l{i}_context"))?,
                ffn: t.ffn(&format!("decoder_l{i}_ffn"))?,
            });
        }
        let (src_vocab, trg_vocab) = match find(dir, "vocab.", ".spm") {
            Some(v) => (Vocab::read(&read(&v)?)?, None),
            None => {
                let s = find(dir, "srcvocab.", ".spm").ok_or("no vocab.*.spm or srcvocab.*.spm")?;
                let g = find(dir, "trgvocab.", ".spm").ok_or("no trgvocab.*.spm")?;
                (Vocab::read(&read(&s)?)?, Some(Vocab::read(&read(&g)?)?))
            }
        };
        let shortlist = match find(dir, "lex.", ".bin") {
            Some(p) => Some(Shortlist::read(&read(&p)?)?),
            None => None,
        };
        let m = Model { dim, heads, src_emb, trg_emb, out_b, out_alpha, enc, dec, src_vocab, trg_vocab, shortlist };
        let src_rows = m.src_emb.as_ref().unwrap_or(&m.trg_emb).rows;
        if m.trg_emb.cols != dim
            || src_rows < m.src_vocab.len()
            || m.trg_emb.rows < m.trg_vocab().len()
            || m.out_b.len() != m.trg_emb.rows
        {
            return Err("the model's embeddings do not match its vocabulary".into());
        }
        Ok(m)
    }

    fn trg_vocab(&self) -> &Vocab {
        self.trg_vocab.as_ref().unwrap_or(&self.src_vocab)
    }

    /// One chat line (it may hold several sentences) into the target language. Blocking: ~10-60 ms a line.
    pub fn translate(&self, text: &str) -> Result<String, String> {
        let mut out = String::new();
        for sentence in split_sentences(text) {
            let ids = self.src_vocab.encode(sentence);
            if ids.is_empty() {
                continue;
            }
            for chunk in self.chunks(&ids) {
                let mut src = chunk.to_vec();
                src.push(EOS);
                let piece = self.trg_vocab().decode(&self.translate_ids(&src));
                let piece = piece.trim();
                if piece.is_empty() {
                    continue;
                }
                if !out.is_empty() && !out.ends_with(is_cjk) && !piece.starts_with(is_cjk) {
                    out.push(' ');
                }
                out.push_str(piece);
            }
        }
        Ok(out)
    }

    /// A sentence too long for one go, cut at word starts into pieces of at most MAX_TOKENS.
    fn chunks<'a>(&self, ids: &'a [u32]) -> Vec<&'a [u32]> {
        let mut out = Vec::new();
        let mut rest = ids;
        while rest.len() > MAX_TOKENS {
            let cut =
                (MAX_TOKENS / 2..MAX_TOKENS).rev().find(|&i| self.src_vocab.starts_word(rest[i])).unwrap_or(MAX_TOKENS);
            out.push(&rest[..cut]);
            rest = &rest[cut..];
        }
        out.push(rest);
        out
    }

    #[doc(hidden)]
    pub fn encode_for_tests(&self, text: &str) -> Vec<u32> {
        let mut ids = self.src_vocab.encode(text);
        ids.push(EOS);
        ids
    }

    #[doc(hidden)]
    pub fn shortlist_for_tests(&self, ids: &[u32]) -> Vec<u32> {
        self.shortlist(ids).unwrap_or_default()
    }

    /// The memory the model holds, in bytes.
    pub fn bytes(&self) -> usize {
        let lin = |l: &Lin| l.w.q.len() + l.b.as_ref().map_or(0, |b| b.len() * 4);
        let norm = |n: &Norm| (n.scale.len() + n.bias.len()) * 4;
        let attn = |a: &Attn| lin(&a.q) + lin(&a.k) + lin(&a.v) + lin(&a.o) + norm(&a.ln);
        let ffn = |f: &Ffn| lin(&f.w1) + lin(&f.w2) + norm(&f.ln);
        self.src_emb.as_ref().map_or(0, |e| e.q.len())
            + self.trg_emb.q.len()
            + self.out_b.len() * 4
            + self.enc.iter().map(|(a, f)| attn(a) + ffn(f)).sum::<usize>()
            + self
                .dec
                .iter()
                .map(|d| lin(&d.rnn.w) + lin(&d.rnn.wf) + norm(&d.rnn.ln) + attn(&d.ctx) + ffn(&d.ffn))
                .sum::<usize>()
            + self.src_vocab.bytes()
            + self.trg_vocab.as_ref().map_or(0, |v| v.bytes())
            + self.shortlist.as_ref().map_or(0, |s| s.bytes())
    }

    /// Source ids (ending with the end-of-sentence id) -> target ids, greedy.
    pub fn translate_ids(&self, ids: &[u32]) -> Vec<u32> {
        let d = self.dim;
        let mut s = Scratch::default();
        let ctx = self.encode(ids, &mut s);
        let n = ids.len();
        let cache: Vec<(Vec<f32>, Vec<f32>)> = self
            .dec
            .iter()
            .map(|l| {
                let mut k = vec![0.0; n * d];
                let mut v = vec![0.0; n * d];
                lin(&l.ctx.k, &ctx, n, &mut k, &mut s.q);
                lin(&l.ctx.v, &ctx, n, &mut v, &mut s.q);
                (k, v)
            })
            .collect();
        let cand = self.shortlist(ids);
        let out_w = match &cand {
            Some(c) => self.trg_emb.select(c),
            None => Int8 { rows: self.trg_emb.rows, cols: d, q: self.trg_emb.q.clone(), mult: self.trg_emb.mult },
        };
        let out_b: Vec<f32> = match &cand {
            Some(c) => c.iter().map(|&i| self.out_b[i as usize]).collect(),
            None => self.out_b.clone(),
        };
        let out_lin = Lin { w: out_w, alpha: self.out_alpha, b: Some(out_b) };
        let mut cells = vec![vec![0.0f32; d]; self.dec.len()];
        let mut logits = vec![0.0; out_lin.w.rows];
        let mut x = vec![0.0; d];
        let mut out = Vec::new();
        let mut prev: Option<u32> = None;
        let scale = (d as f32).sqrt();
        for step in 0..=(2 * n) {
            match prev {
                Some(p) => self.trg_emb.row_f32(p as usize, &mut x),
                None => x.iter_mut().for_each(|v| *v = 0.0),
            }
            let pos = positions(step, 1, d);
            for (v, p) in x.iter_mut().zip(&pos) {
                *v = *v * scale + p;
            }
            for (l, cell) in self.dec.iter().zip(cells.iter_mut()) {
                let (xw, f) = (&mut s.a, &mut s.b);
                xw.resize(d, 0.0);
                f.resize(d, 0.0);
                lin(&l.rnn.w, &x, 1, xw, &mut s.q);
                lin(&l.rnn.wf, &x, 1, f, &mut s.q);
                for i in 0..d {
                    let g = 1.0 / (1.0 + (-f[i]).exp());
                    cell[i] = g * cell[i] + (1.0 - g) * xw[i];
                    x[i] += cell[i].max(0.0);
                }
                layer_norm(&mut x, d, &l.rnn.ln);
                let (k, v) = &cache[self.dec.iter().position(|o| std::ptr::eq(o, l)).unwrap()];
                self.attention(&l.ctx, &mut x, 1, k, v, n, &mut s);
                ffn(&l.ffn, &mut x, 1, &mut s);
            }
            lin(&out_lin, &x, 1, &mut logits, &mut s.q);
            let best = argmax(&logits);
            let id = cand.as_ref().map_or(best as u32, |c| c[best]);
            if id == EOS {
                break;
            }
            out.push(id);
            prev = Some(id);
        }
        out
    }

    fn shortlist(&self, ids: &[u32]) -> Option<Vec<u32>> {
        let s = self.shortlist.as_ref()?;
        Some(s.select(ids, self.trg_emb.rows, self.trg_vocab.is_none()))
    }

    fn encode(&self, ids: &[u32], s: &mut Scratch) -> Vec<f32> {
        let d = self.dim;
        let n = ids.len();
        let emb = self.src_emb.as_ref().unwrap_or(&self.trg_emb);
        let mut x = vec![0.0f32; n * d];
        let pos = positions(0, n, d);
        let scale = (d as f32).sqrt();
        for (i, &id) in ids.iter().enumerate() {
            let row = &mut x[i * d..(i + 1) * d];
            emb.row_f32(id as usize, row);
            for (j, v) in row.iter_mut().enumerate() {
                *v = *v * scale + pos[i * d + j];
            }
        }
        let mut k = vec![0.0; n * d];
        let mut v = vec![0.0; n * d];
        for (att, f) in &self.enc {
            lin(&att.k, &x, n, &mut k, &mut s.q);
            lin(&att.v, &x, n, &mut v, &mut s.q);
            self.attention(att, &mut x, n, &k, &v, n, s);
            ffn(f, &mut x, n, s);
        }
        x
    }

    /// x [tq, dim] <- LN(x + Wo . attention(Wq x, k, v)); k, v [ts, dim] already projected
    #[allow(clippy::too_many_arguments)]
    fn attention(&self, a: &Attn, x: &mut [f32], tq: usize, k: &[f32], v: &[f32], ts: usize, s: &mut Scratch) {
        let d = self.dim;
        let dk = d / self.heads;
        let q = &mut s.a;
        q.resize(tq * d, 0.0);
        lin(&a.q, x, tq, q, &mut s.q);
        let o = &mut s.b;
        o.clear();
        o.resize(tq * d, 0.0);
        let w = &mut s.w;
        w.resize(ts, 0.0);
        let inv = 1.0 / (dk as f32).sqrt();
        for h in 0..self.heads {
            let off = h * dk;
            for i in 0..tq {
                let qi = &q[i * d + off..i * d + off + dk];
                let mut top = f32::MIN;
                for j in 0..ts {
                    let kj = &k[j * d + off..j * d + off + dk];
                    w[j] = qi.iter().zip(kj).map(|(a, b)| a * b).sum::<f32>() * inv;
                    top = top.max(w[j]);
                }
                let mut sum = 0.0;
                for wj in w.iter_mut() {
                    *wj = (*wj - top).exp();
                    sum += *wj;
                }
                let oi = &mut o[i * d + off..i * d + off + dk];
                for j in 0..ts {
                    let p = w[j] / sum;
                    for (ov, vv) in oi.iter_mut().zip(&v[j * d + off..j * d + off + dk]) {
                        *ov += p * vv;
                    }
                }
            }
        }
        let y = &mut s.c;
        y.resize(tq * d, 0.0);
        lin(&a.o, o, tq, y, &mut s.q);
        for (xv, yv) in x.iter_mut().zip(y.iter()) {
            *xv += yv;
        }
        layer_norm(x, d, &a.ln);
    }
}

#[derive(Default)]
struct Scratch {
    q: Vec<i8>,
    a: Vec<f32>,
    b: Vec<f32>,
    c: Vec<f32>,
    w: Vec<f32>,
}

/// out [m, l.w.rows] = x [m, l.w.cols] . W' + b, 8-bit when the file gives the matrix an alpha
fn lin(l: &Lin, x: &[f32], m: usize, out: &mut [f32], q: &mut Vec<i8>) {
    match l.alpha {
        Some(alpha) => {
            gemm::quantize(&x[..m * l.w.cols], alpha, q);
            gemm::matmul(q, m, alpha, &l.w, l.b.as_deref(), &mut out[..m * l.w.rows]);
        }
        None => gemm::matmul_f32(&x[..m * l.w.cols], m, &l.w, l.b.as_deref(), &mut out[..m * l.w.rows]),
    }
}

/// x [m, dim] <- LN(x + W2 . relu(W1 x))
fn ffn(f: &Ffn, x: &mut [f32], m: usize, s: &mut Scratch) {
    let d = f.w2.w.rows;
    let h = &mut s.a;
    h.resize(m * f.w1.w.rows, 0.0);
    lin(&f.w1, x, m, h, &mut s.q);
    h.iter_mut().for_each(|v| *v = v.max(0.0));
    let y = &mut s.c;
    y.resize(m * d, 0.0);
    lin(&f.w2, h, m, y, &mut s.q);
    for (xv, yv) in x.iter_mut().zip(y.iter()) {
        *xv += yv;
    }
    layer_norm(x, d, &f.ln);
}

fn layer_norm(x: &mut [f32], d: usize, n: &Norm) {
    for row in x.chunks_exact_mut(d) {
        let mean = row.iter().sum::<f32>() / d as f32;
        let var = row.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / d as f32;
        let inv = 1.0 / (var + LN_EPS).sqrt();
        for ((v, s), b) in row.iter_mut().zip(&n.scale).zip(&n.bias) {
            *v = (*v - mean) * inv * s + b;
        }
    }
}

/// Marian's sinusoidal positions: sin on the first half of the dims, cos on the second.
fn positions(start: usize, n: usize, d: usize) -> Vec<f32> {
    let half = d / 2;
    let inc = (10000.0f32).ln() / (half as f32 - 1.0);
    let mut out = vec![0.0; n * d];
    for p in 0..n {
        for i in 0..half {
            let v = (start + p) as f32 * (-(i as f32) * inc).exp();
            out[p * d + i] = v.sin();
            out[p * d + half + i] = v.cos();
        }
    }
    out
}

fn argmax(v: &[f32]) -> usize {
    let mut best = 0;
    for (i, &x) in v.iter().enumerate() {
        if x > v[best] {
            best = i;
        }
    }
    best
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3000..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xFF00..=0xFFEF)
}

/// A chat line's sentences: cut after . ! ? (followed by a space) and after the CJK 。！？ (whatever follows).
pub fn split_sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (at, c) = chars[i];
        let end_mark = matches!(c, '.' | '!' | '?' | '\u{2026}');
        let cjk_mark = matches!(c, '\u{3002}' | '\u{FF01}' | '\u{FF1F}');
        if end_mark || cjk_mark {
            // (a run of marks stays together: "?!", "...")
            let mut j = i + 1;
            while j < chars.len()
                && matches!(
                    chars[j].1,
                    '.' | '!'
                        | '?'
                        | '\u{2026}'
                        | '\u{3002}'
                        | '\u{FF01}'
                        | '\u{FF1F}'
                        | '"'
                        | '\''
                        | ')'
                        | '\u{300D}'
                        | '\u{300F}'
                )
            {
                j += 1;
            }
            let next = chars.get(j).map(|c| c.1);
            if cjk_mark || next.is_none_or(char::is_whitespace) {
                let cut = chars.get(j).map_or(text.len(), |c| c.0);
                let s = text[start..cut].trim();
                if !s.is_empty() {
                    out.push(s);
                }
                start = cut;
            }
            i = j;
            let _ = at;
            continue;
        }
        i += 1;
    }
    let s = text[start..].trim();
    if !s.is_empty() {
        out.push(s);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentences() {
        assert_eq!(split_sentences("Hi there. How are you?! Fine..."), vec!["Hi there.", "How are you?!", "Fine..."]);
        assert_eq!(split_sentences("3.14 is pi. ok"), vec!["3.14 is pi.", "ok"]);
        assert_eq!(split_sentences("こんにちは。元気？うん"), vec!["こんにちは。", "元気？", "うん"]);
        assert_eq!(split_sentences("  "), Vec::<&str>::new());
        assert_eq!(split_sentences("no end"), vec!["no end"]);
    }
}
