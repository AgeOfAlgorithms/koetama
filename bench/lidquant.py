"""A smaller language detector: the VoxLingua107 ECAPA ONNX (86 MB, float32) quantised to int8, against the original on
the benchmark's recordings (export/asrbench/lid: 124 single-language lines and 140 one-word callouts in en / ru / zh /
es / de, clean and in a noisy room, and 20 mixed-language lines).

    <conda>/envs/pclid/python.exe bench/lidquant.py make     # the int8 copies (needs onnx + onnxruntime)
    <conda>/envs/pcvoice/python.exe bench/lidquant.py eval   # the comparison -> export/asrbench/lidquant.md

Measured, per detector:
  clip      the detector's language (of the 10 "auto" ones) for a whole recording - as a short line or one word is
            decided - right or wrong (% right)
  window    1 s windows every 0.25 s over every recording: the same language as the original's (% agreeing)
  stitched  asr.segments() (the app's stitching) on the single-language recordings: every stretch in the right
            language, with a fallback that is never the right one (so an unsure short line counts as wrong)
  mixed     the mixed lines: the share of their speech time stitched into the right language
  time      ms per 1 s window (4 threads), and the file's size
The audio front end (the spectrum as two convolutions and the mel filterbank: /Conv, /Conv_1, /fbanks/MatMul) stays
float32 in every quantised copy.

Copies (2026-10-06): int8 dynamic quantisation made the detector 10x SLOWER (162 ms per 1 s window against 17: onnxruntime
has no fast kernel for its ConvInteger on a desktop CPU) - dropped. Static int8 (QDQ, calibrated on the clean lines' 1 s
windows: onnxruntime runs it with its fast QLinearConv) and fp16 storage (the weights kept as float16 in the file, cast
back to float32 when it loads: the same computation and speed, half the size) are compared.
"""
import json
import os
import sys
import time

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
LID = os.path.join(ROOT, 'export', 'lid')
SRC = os.path.join(LID, 'voxlingua107-ecapa.fp32.onnx')   # (export_lid.py's float32 export: the baseline)
OUT = os.path.join(ROOT, 'export', 'asrbench')
FRONT = ['/Conv', '/Conv_1', '/fbanks/MatMul']
VARIANTS = ['int8-static', 'fp16w']


def path_of(name):
    return SRC if name == 'fp32' else os.path.join(LID, 'voxlingua107-ecapa.%s.onnx' % name)


def calibration_windows(n=160):
    """1 s windows from the clean single-language lines (the noisy-room copies and the one-word callouts are not used)"""
    import wave
    items = json.load(open(os.path.join(OUT, 'lid', 'items.json'), encoding='utf-8'))
    out = []
    for it in items:
        if it['kind'] == 'line' and it['id'].endswith('_clean'):
            with wave.open(it['path'], 'rb') as w:
                x = np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(np.float32) / 32768
            for k in range(0, len(x) - 16000, 8000):
                out.append(x[k:k + 16000])
    rng = np.random.default_rng(3)
    return [out[i] for i in rng.choice(len(out), min(n, len(out)), replace=False)]


def make():
    import onnx
    from onnx import helper, numpy_helper, TensorProto
    from onnxruntime.quantization import quantize_static, QuantType, QuantFormat, CalibrationDataReader
    from onnxruntime.quantization.shape_inference import quant_pre_process
    names = {n.name for n in onnx.load(SRC).graph.node}
    keep = [k for k in FRONT if k in names]
    # static int8 (QDQ), calibrated
    pre = os.path.join(LID, 'voxlingua107-ecapa.pre.onnx')
    quant_pre_process(SRC, pre, skip_symbolic_shape=True)

    class Reader(CalibrationDataReader):
        def __init__(self):
            self.it = iter([{'audio': w[None]} for w in calibration_windows()])

        def get_next(self):
            return next(self.it, None)
    quantize_static(pre, path_of('int8-static'), Reader(), quant_format=QuantFormat.QDQ, per_channel=True,
                    weight_type=QuantType.QInt8, activation_type=QuantType.QUInt8, op_types_to_quantize=['Conv', 'MatMul'],
                    nodes_to_exclude=keep)
    os.remove(pre)
    print('int8-static: %.1f MB' % (os.path.getsize(path_of('int8-static')) / 1e6))
    # fp16 storage: each big float initializer as float16 + a Cast back to float32 (folded when the model loads)
    m = onnx.load(SRC)
    g = m.graph
    casts = []
    for t in list(g.initializer):
        if t.data_type == TensorProto.FLOAT and np.prod(t.dims) >= 1024:
            a = numpy_helper.to_array(t)
            h = numpy_helper.from_array(a.astype(np.float16), t.name + '_fp16')
            g.initializer.remove(t)
            g.initializer.append(h)
            casts.append(helper.make_node('Cast', [h.name], [t.name], to=TensorProto.FLOAT, name=t.name + '_cast'))
    for c in reversed(casts):
        g.node.insert(0, c)
    onnx.save(m, path_of('fp16w'))
    print('fp16w: %.1f MB (%d weights stored as float16)' % (os.path.getsize(path_of('fp16w')) / 1e6, len(casts)))


def evaluate():
    sys.path.insert(0, os.path.join(ROOT, 'engine'))
    import asr
    import audio
    import onnxruntime as ort
    labels = json.load(open(os.path.join(LID, 'voxlingua107-ecapa.json'), encoding='utf-8'))['labels']
    idx = [labels.index(l) for l in asr.MIXED_LANGS]
    items = json.load(open(os.path.join(OUT, 'lid', 'items.json'), encoding='utf-8'))
    clips = {}
    for it in items:
        x, sr = audio.read_wav(it['path'])
        clips[it['id']] = x if sr == asr.RATE else audio.resample(x, sr, asr.RATE)
    names = ['fp32'] + [n for n in VARIANTS if os.path.exists(path_of(n))]
    for name in names:                        # (speed first: a copy much slower than the original is no use)
        so = ort.SessionOptions()
        so.intra_op_num_threads = 4
        sess = ort.InferenceSession(path_of(name), so, providers=['CPUExecutionProvider'])
        w = np.random.default_rng(1).standard_normal((1, 16000)).astype(np.float32) * 0.1
        sess.run(None, {'audio': w})
        t0 = time.perf_counter()
        for _ in range(10):
            sess.run(None, {'audio': w})
        print('%-12s %.0f ms per 1 s window' % (name, (time.perf_counter() - t0) * 100), flush=True)

    class M:                                  # (what asr.lid_probs / asr.segments ask of a Models)
        def __init__(self, sess):
            self.sess = sess

        def get(self, name):
            return self.sess, idx
    rows, ref_windows = [], None
    for name in names:
        so = ort.SessionOptions()
        so.intra_op_num_threads = 4
        sess = ort.InferenceSession(path_of(name), so, providers=['CPUExecutionProvider'])
        models = M(sess)
        clip = {}                             # kind -> [right, total]
        stitched = {}
        windows, wtime, wn = [], 0.0, 0
        mixed_right = mixed_total = 0.0
        for it in items:
            x = clips[it['id']]
            if it['kind'] in ('line', 'word'):
                p = asr.lid_probs(models, x)
                c = clip.setdefault(it['kind'], [0, 0])
                c[0] += asr.MIXED_LANGS[int(np.argmax(p))] == it['lang']
                c[1] += 1
                wrong_fb = 'ko' if it['lang'] != 'ko' else 'en'
                segs = asr.segments(models, x, fallback=wrong_fb, cache={})
                s = stitched.setdefault(it['kind'], [0, 0])
                s[0] += all(l == it['lang'] for l, _, _ in segs)
                s[1] += 1
            else:
                segs = asr.segments(models, x, fallback='en', cache={})
                for ref in it['segs']:                  # (speech time in the right language)
                    for l, a, b in segs:
                        if l == ref['lang']:
                            mixed_right += max(0.0, min(b, ref['t1']) - max(a, ref['t0']))
                    mixed_total += ref['t1'] - ref['t0']
            t = 0.0
            while t + asr.LID_WIN <= len(x) / asr.RATE + 1e-6:
                w = x[int(t * asr.RATE):int((t + asr.LID_WIN) * asr.RATE)]
                t0 = time.perf_counter()
                p = asr.lid_probs(models, w)
                wtime += time.perf_counter() - t0
                wn += 1
                windows.append(int(np.argmax(p)))
                t += asr.LID_HOP
        if ref_windows is None:
            ref_windows = windows
        agree = 100.0 * np.mean(np.array(windows) == np.array(ref_windows))
        rows.append(dict(name=name, size=os.path.getsize(path_of(name)) / 1e6, clip=clip, stitched=stitched,
                         agree=agree, mixed=100.0 * mixed_right / mixed_total, ms=1000.0 * wtime / wn))
        r = rows[-1]
        print('%-12s %5.1f MB  clip lines %5.1f %%  words %5.1f %%  | stitched lines %5.1f %%  words %5.1f %%  | windows agree '
              '%5.1f %%  | mixed %5.1f %%  | %.1f ms' % (
                  name, r['size'], *(100.0 * clip[k][0] / clip[k][1] for k in ('line', 'word')),
                  *(100.0 * stitched[k][0] / stitched[k][1] for k in ('line', 'word')), agree, r['mixed'], r['ms']), flush=True)
    L = ['# The language detector, smaller (bench/lidquant.py)', '',
         'VoxLingua107 ECAPA (ONNX) quantised to int8 (onnxruntime dynamic quantisation, the audio front end kept float32) '
         'against the original, on the benchmark recordings: %d single-language lines, %d one-word callouts (en / ru / zh / '
         'es / de, clean and noisy room), %d mixed lines.' % (stitched_count(items, 'line'), stitched_count(items, 'word'),
                                                             stitched_count(items, 'mixed')), '',
         '| detector | size | clip: lines | clip: one word | stitched: lines | stitched: one word | windows agreeing with fp32 | mixed: time right | ms / 1 s window |',
         '|---|---|---|---|---|---|---|---|---|']
    for r in rows:
        L.append('| %s | %.0f MB | %.1f %% | %.1f %% | %.1f %% | %.1f %% | %.1f %% | %.1f %% | %.1f |' % (
            r['name'], r['size'], *(100.0 * r['clip'][k][0] / r['clip'][k][1] for k in ('line', 'word')),
            *(100.0 * r['stitched'][k][0] / r['stitched'][k][1] for k in ('line', 'word')), r['agree'], r['mixed'], r['ms']))
    L += ['', 'clip: the detector\'s language for the whole recording (how a short line or one word is decided). stitched: '
          'the app\'s stitching (asr.segments) on the whole recording, every stretch right, with a fallback that is never '
          'the right language (an unsure short line counts as wrong; in the app the fallback is the player\'s last '
          'language, usually right). mixed: the share of the mixed lines\' speech time stitched into its language.']
    open(os.path.join(OUT, 'lidquant.md'), 'w', encoding='utf-8').write('\n'.join(L) + '\n')
    print('wrote export/asrbench/lidquant.md')


def stitched_count(items, kind):
    return sum(1 for it in items if it['kind'] == kind)


if __name__ == '__main__':
    {'make': make, 'eval': evaluate}[sys.argv[1]]()
