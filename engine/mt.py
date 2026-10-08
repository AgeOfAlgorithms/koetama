"""Mozilla's translation models (Firefox Translations: Marian students) run in numpy - the REFERENCE the Rust engine
(app/crates/kd-translate) is compared against, as asr.py is for speech. A model is a directory with Mozilla's files:
model.<pair>.intgemm.alphas.bin, lex.50.50.<pair>.s2t.bin, vocab.<pair>.spm (or srcvocab / trgvocab).

The architecture (each model's own special:model.yml; browsermt/marian-dev src/models/transformer.h):
  encoder   embeddings x sqrt(dim) + sinusoidal positions (sin on the first half of the dims, cos on the second),
            then enc-depth x [self-attention, feed-forward], each post-processed "dan": add the input, layer-norm
            (eps 1e-6)
  decoder   the previous word's embedding (zeros at the first step) x sqrt(dim) + the step's position, then
            dec-depth x [SSRU, attention over the encoder, feed-forward], each "dan"
  SSRU      x = q W ; f = q Wf + bf ; c = sigmoid(f) c + (1 - sigmoid(f)) x ; out = relu(c)
  output    logits = h Wemb' + b over the lexical shortlist (the 100 commonest target words, each source word's best
            50, the source words themselves with a shared vocabulary, padded to a multiple of 8); greedy.
  weights   8-bit, stored quantized and transposed, each followed by its float multiplier (value = q / mult); the
            embeddings are dequantized at load. INT8 = True also quantizes each matrix's input with the file's
            "alpha" (<name>_QuantMultA) as intgemm does - Mozilla's arithmetic; False: float inputs."""
import glob
import math
import os
import struct

import numpy as np
import sentencepiece as spm

INT8 = True
LN_EPS = 1e-6
SHORTLIST_MAGIC = 0xF11A48D5013417F5
EOS = 0


# ---------------------------------------------------------------- the files
def read_bin(path):
    """{name: array} from a Marian binary model; int8 matrices as {'q': (out, in) int8, 'mult': float}; and the
    embedded config text"""
    b = open(path, 'rb').read()
    _, n = struct.unpack_from('<QQ', b, 0)
    off = 16
    heads = [struct.unpack_from('<QQQQ', b, off + 32 * i) for i in range(n)]
    off += 32 * n
    names = []
    for nl, _, _, _ in heads:
        names.append(b[off:off + nl - 1].decode())
        off += nl
    shapes = []
    for _, _, sl, _ in heads:
        shapes.append(struct.unpack_from('<' + 'i' * sl, b, off))
        off += 4 * sl
    (pad,) = struct.unpack_from('<Q', b, off)
    off += 8 + pad
    out, config = {}, ''
    for (nl, t, sl, dl), name, shape in zip(heads, names, shapes):
        raw = b[off:off + dl]
        off += dl
        if name == 'special:model.yml':
            config = raw.decode(errors='replace').rstrip('\x00')
            continue
        if t & 0x4000:                                            # (intgemm8: quantized, then its multiplier)
            count = int(np.prod(shape))
            q = np.frombuffer(raw[:count], dtype=np.int8)
            mult = struct.unpack_from('<f', raw, count)[0]
            if name.endswith('_QuantMultA'):
                out[name] = np.float32(mult)                      # (an "alpha" stored with the intgemm type)
            elif name.endswith('Wemb'):
                out[name] = (q.reshape(shape).astype(np.float32) / mult)   # (embeddings: dequantized, row = word)
                out[name + ':int8'] = dict(q=q.reshape(shape), mult=np.float32(mult))   # (the output layer's form)
            else:
                rows, cols = shape                                # (stored transposed: [cols][rows])
                out[name] = dict(q=q.reshape(cols, rows), mult=np.float32(mult))
        else:
            arr = np.frombuffer(raw, dtype=np.float32)[:int(np.prod(shape))].reshape(shape)
            out[name] = arr[0, 0] if name.endswith('_QuantMultA') else arr.astype(np.float32)
    return out, config


def parse_config(text):
    """the few settings this needs from special:model.yml"""
    cfg = {}
    for line in text.splitlines():
        if ':' in line and not line.startswith(' '):
            k, v = line.split(':', 1)
            cfg[k.strip()] = v.strip()
    return cfg


def read_shortlist(path):
    b = open(path, 'rb').read()
    magic, _, first, best, n_off, n_lists = struct.unpack_from('<QQQQQQ', b, 0)
    assert magic == SHORTLIST_MAGIC, 'not a binary shortlist'
    off = 48
    word_off = np.frombuffer(b, dtype=np.uint64, count=n_off, offset=off)
    lists = np.frombuffer(b, dtype=np.uint32, count=n_lists, offset=off + 8 * n_off)
    return dict(first=int(first), word_off=word_off, lists=lists)


# ---------------------------------------------------------------- the maths
def layer_norm(x, scale, bias):
    mu = x.mean(-1, keepdims=True)
    sd = np.sqrt(((x - mu) ** 2).mean(-1, keepdims=True) + LN_EPS)
    return (x - mu) / sd * scale + bias


def dot(x, w, alpha=None):
    """x [n, in] times an int8 matrix ([out, in] quantized): intgemm's arithmetic when INT8 and alpha - the input
    quantized with the file's multiplier (round, clipped to +-127), the integer product, then divided back"""
    if INT8 and alpha is not None:
        xq = np.clip(np.rint(x * alpha), -127, 127).astype(np.int32)
        return (xq @ w['q'].astype(np.int32).T).astype(np.float32) / (alpha * w['mult'])
    return x @ (w['q'].astype(np.float32).T / w['mult'])


def positions(start, n, dim):
    half = dim // 2
    inc = math.log(10000.0) / (half - 1)
    p = np.arange(start, start + n, dtype=np.float32)[:, None]
    v = p * np.exp(np.arange(half, dtype=np.float32) * -inc)[None, :]
    return np.concatenate([np.sin(v), np.cos(v)], axis=1).astype(np.float32)


def softmax(z):
    z = z - z.max(-1, keepdims=True)
    e = np.exp(z)
    return e / e.sum(-1, keepdims=True)


class Model:
    def __init__(self, folder):
        bins = glob.glob(os.path.join(folder, 'model.*.bin'))
        self.p, text = read_bin(bins[0])
        cfg = parse_config(text)
        self.dim = int(cfg['dim-emb'])
        self.heads = int(cfg['transformer-heads'])
        self.enc_depth = int(cfg['enc-depth'])
        self.dec_depth = int(cfg['dec-depth'])
        lex = glob.glob(os.path.join(folder, 'lex.*.bin'))
        self.short = read_shortlist(lex[0]) if lex else None
        shared = glob.glob(os.path.join(folder, 'vocab.*.spm'))
        if shared:
            self.src_sp = self.trg_sp = spm.SentencePieceProcessor(model_file=shared[0])
        else:
            self.src_sp = spm.SentencePieceProcessor(model_file=glob.glob(os.path.join(folder, 'srcvocab.*.spm'))[0])
            self.trg_sp = spm.SentencePieceProcessor(model_file=glob.glob(os.path.join(folder, 'trgvocab.*.spm'))[0])
        self.shared = bool(shared)
        p = self.p
        self.src_emb = p['Wemb'] if 'Wemb' in p else p['encoder_Wemb']
        self.trg_emb = p['Wemb'] if 'Wemb' in p else p['decoder_Wemb']

    def lin(self, x, name, bias=True):
        y = dot(x, self.p[name], self.p.get(name + '_QuantMultA'))
        if bias:
            y = y + self.p[name.replace('_W', '_b', 1) if '_W' in name else name + '_b'][0]
        return y

    def attention(self, prefix, q_in, k, v):
        """q_in [tq, dim]; k, v [ts, dim] already projected"""
        h, d = self.heads, self.dim // self.heads
        q = self.lin(q_in, prefix + '_Wq')
        qh = q.reshape(-1, h, d).transpose(1, 0, 2)
        kh = k.reshape(-1, h, d).transpose(1, 0, 2)
        vh = v.reshape(-1, h, d).transpose(1, 0, 2)
        w = softmax(qh @ kh.transpose(0, 2, 1) / math.sqrt(d))
        o = (w @ vh).transpose(1, 0, 2).reshape(-1, self.dim)
        o = self.lin(o, prefix + '_Wo')
        return layer_norm(o + q_in, self.p[prefix + '_Wo_ln_scale'][0], self.p[prefix + '_Wo_ln_bias'][0])

    def ffn(self, prefix, x):
        hdn = np.maximum(self.lin(x, prefix + '_W1'), 0)
        o = self.lin(hdn, prefix + '_W2')
        return layer_norm(o + x, self.p[prefix + '_ffn_ln_scale'][0], self.p[prefix + '_ffn_ln_bias'][0])

    def encode(self, ids):
        x = self.src_emb[ids] * math.sqrt(self.dim) + positions(0, len(ids), self.dim)
        for i in range(1, self.enc_depth + 1):
            pre = f'encoder_l{i}_self'
            k, v = self.lin(x, pre + '_Wk'), self.lin(x, pre + '_Wv')
            x = self.attention(pre, x, k, v)
            x = self.ffn(f'encoder_l{i}_ffn', x)
        return x

    def shortlist(self, ids):
        V = self.trg_emb.shape[0]
        sel = np.zeros(V, dtype=bool)
        if self.short is None:
            sel[:] = True
            return np.nonzero(sel)[0]
        s = self.short
        sel[:min(s['first'], V)] = True
        for w in set(ids):
            if self.shared:
                sel[w] = True
            sel[s['lists'][int(s['word_off'][w]):int(s['word_off'][w + 1])]] = True
        n = int(sel.sum())
        i = s['first']
        while n % 8 and i < V:                                    # (a multiple of 8, as Marian pads it)
            if not sel[i]:
                sel[i] = True
                n += 1
            i += 1
        return np.nonzero(sel)[0]

    def translate_ids(self, ids, max_factor=2.0):
        ctx = self.encode(ids)
        cache = [(self.lin(ctx, f'decoder_l{i}_context_Wk'), self.lin(ctx, f'decoder_l{i}_context_Wv'))
                 for i in range(1, self.dec_depth + 1)]
        cand = self.shortlist(ids)
        emb8 = self.p['Wemb:int8' if 'Wemb' in self.p else 'decoder_Wemb:int8']
        out_w = dict(q=emb8['q'][cand], mult=emb8['mult'])       # [cand, dim]: the int8 rows (8-bit like the rest)
        out_b = self.p['decoder_ff_logit_out_b'][0][cand]
        out_alpha = self.p.get('none_QuantMultA')                 # (the output layer's alpha)
        cells = [np.zeros((1, self.dim), dtype=np.float32) for _ in range(self.dec_depth)]
        prev = None
        out = []
        for step in range(int(max_factor * len(ids)) + 1):
            emb = np.zeros((1, self.dim), dtype=np.float32) if prev is None else self.trg_emb[[prev]]
            q = emb * math.sqrt(self.dim) + positions(step, 1, self.dim)
            for i in range(1, self.dec_depth + 1):
                pre = f'decoder_l{i}_rnn'
                xw = dot(q, self.p[pre + '_W'], self.p.get(pre + '_W_QuantMultA'))
                f = dot(q, self.p[pre + '_Wf'], self.p.get(pre + '_Wf_QuantMultA')) + self.p[pre + '_bf'][0]
                g = 1.0 / (1.0 + np.exp(-f))
                cells[i - 1] = g * cells[i - 1] + (1 - g) * xw
                q = layer_norm(np.maximum(cells[i - 1], 0) + q, self.p[pre + '_ffn_ln_scale'][0],
                               self.p[pre + '_ffn_ln_bias'][0])
                k, v = cache[i - 1]
                q = self.attention(f'decoder_l{i}_context', q, k, v)
                q = self.ffn(f'decoder_l{i}_ffn', q)
            logits = dot(q, out_w, out_alpha) + out_b
            prev = int(cand[int(np.argmax(logits[0]))])
            if prev == EOS:
                break
            out.append(prev)
        return out

    def translate(self, text):
        ids = self.src_sp.encode(text) + [EOS]
        return self.trg_sp.decode(self.translate_ids(ids))
