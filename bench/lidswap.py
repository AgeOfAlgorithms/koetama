"""AmberNet (NVIDIA; NGC terms, cannot ship) against its replacement, SpeechBrain's VoxLingua107 ECAPA (Apache-2.0, our
ONNX export) in the helper's own "auto" path (asr.transcribe_mixed: windows, vote, stitch, each stretch by its
language's model). Words wrong on the benchmark's single-language lines, the 20 mixed lines and the one-word callouts
(said alone, where the detector's confidence decides between its guess and English); the detector's own time.

    C:/Users/user/miniconda3/envs/pcvoice/python.exe bench/lidswap.py [helper]
    (helper: only the helper's detector with asr.py's settings -> lidswap_helper.md)

Report: export/asrbench/lidswap.md. (AmberNet is loaded here only, from the Hugging Face cache, for the comparison.)
"""
import json
import os
import re
import sys
import time

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(os.path.dirname(HERE), 'engine'))
import bench as B           # noqa: E402
import asr                  # noqa: E402

SB_PROBS = asr.lid_probs


def amber_load(models):
    import onnxruntime as ort
    from huggingface_hub import snapshot_download
    d = snapshot_download('surogate/ambernet-langid')
    so = ort.SessionOptions()
    so.intra_op_num_threads = models.threads
    sess = ort.InferenceSession(os.path.join(d, 'ambernet.onnx'), so, providers=['CPUExecutionProvider'])
    labels = json.load(open(os.path.join(d, 'config.json'), encoding='utf-8'))['labels']
    return sess, [labels.index(l) for l in asr.MIXED_LANGS]


def amber_probs(models, x):
    sess, idx = models.get('ambernet')
    if len(x) < 1600:
        x = np.concatenate([x, np.zeros(1600 - len(x), np.float32)])
    logits, _ = sess.run(None, {'audio': x[None].astype(np.float32), 'audio_len': np.array([len(x)], np.int64)})
    z = logits[0][idx]
    p = np.exp(z - z.max())
    return p / p.sum()


asr.Models._load_ambernet = amber_load
LID_TIME = [0.0, 0]


def timed(fn):
    def f(models, x):
        t0 = time.perf_counter()
        p = fn(models, x)
        LID_TIME[0] += time.perf_counter() - t0
        LID_TIME[1] += len(x) / asr.RATE
        return p
    return f


def toks(t):
    out = []
    for w in B.norm(t, 'en'):
        out += B.norm(w, 'zh') if re.search(r'[\u3400-\u9fff]', w) else [w]
    return out


CONFIGS = [   # (label, probs function, window s, sure, kinds)
    ('AmberNet, 1 s windows', amber_probs, 1.0, 0.8, ('line', 'mixed', 'word')),
    ('SpeechBrain, 1 s windows', SB_PROBS, 1.0, 0.8, ('line', 'mixed', 'word')),
    ('SpeechBrain, 1.5 s windows', SB_PROBS, 1.5, 0.8, ('line', 'mixed')),
    ('SpeechBrain, one word: its guess when over 50 % sure', SB_PROBS, 1.0, 0.5, ('word',)),
    ('SpeechBrain, one word: its guess when over 95 % sure', SB_PROBS, 1.0, 0.95, ('word',)),
]


def main():
    global CONFIGS
    if sys.argv[1:] == ['helper']:                   # (only the helper's own settings, as asr.py has them now)
        CONFIGS = [('the helper now: SpeechBrain, asr.py settings', SB_PROBS, asr.LID_WIN, asr.LID_SURE, ('line', 'mixed', 'word'))]
    items = json.load(open(os.path.join(B.OUT, 'lid', 'items.json'), encoding='utf-8'))
    models = asr.Models(threads=4, log=lambda s: None)
    for name in ('parakeet', 'gigaam', 'sensevoice', 'langid', 'ambernet'):
        models.get(name)
    rows = []
    for label, fn, win, sure, kinds in CONFIGS:
        asr.lid_probs = timed(fn)
        asr.LID_WIN, asr.LID_SURE = win, sure
        LID_TIME[:] = [0.0, 0]
        acc, wrong = {}, {}
        for it in items:
            if it['kind'] not in kinds:
                continue
            text, langs, _ = asr.transcribe_mixed(models, B.read(it['path']))
            key = 'mixed' if it['kind'] == 'mixed' else ('%s %s' % (it['kind'], it['lang']))
            r = acc.setdefault(key, [0, 0])
            r[0] += B.edits(toks(it['text']), toks(text))
            r[1] += len(toks(it['text']))
            if it['kind'] != 'mixed':
                w = wrong.setdefault(it['kind'], [0, 0])
                w[0] += int(any(l != it['lang'] for l in langs))
                w[1] += 1
        ms = 1000.0 * LID_TIME[0] / max(1e-9, LID_TIME[1])
        rows.append((label, acc, wrong, ms))
        print(label, {k: '%.1f' % (100.0 * v[0] / v[1]) for k, v in sorted(acc.items())}, wrong, '%.1f ms per s' % ms, flush=True)
    asr.lid_probs = SB_PROBS
    keys = ['line en', 'line ru', 'line zh', 'line es', 'line de', 'mixed', 'word en', 'word ru', 'word zh']
    L = ['# Language detector: AmberNet against SpeechBrain (%s)' % time.strftime('%Y-%m-%d %H:%M'), '',
         'Through the helper\'s "auto" path (asr.transcribe_mixed), 10 candidate languages, windows every %.2f s. Words '
         'wrong (%%; Chinese by characters), clean + noisy-room clips. "Wrong language": lines / words with any stretch in '
         'another language. Detector time: ms of computing per s of audio it looked at.' % asr.LID_HOP, '',
         '| detector | ' + ' | '.join(keys) + ' | wrong language: lines | wrong language: one word | detector time |',
         '|---|' + '---|' * (len(keys) + 3)]
    for label, acc, wrong, ms in rows:
        cells = ['%.1f' % (100.0 * acc[k][0] / acc[k][1]) if k in acc else '' for k in keys]
        wl = '%d / %d' % tuple(wrong['line']) if 'line' in wrong else ''
        ww = '%d / %d' % tuple(wrong['word']) if 'word' in wrong else ''
        L.append('| %s | %s | %s | %s | %.1f ms |' % (label, ' | '.join(cells), wl, ww, ms))
    open(os.path.join(B.OUT, 'lidswap%s.md' % ('_helper' if sys.argv[1:] == ['helper'] else '')), 'w', encoding='utf-8').write('\n'.join(L) + '\n')
    print('\n'.join(L))


if __name__ == '__main__':
    sys.exit(main())
