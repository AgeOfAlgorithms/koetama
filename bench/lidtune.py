"""Tuning the "auto" stitching with the SpeechBrain detector. Two ideas against what the first comparison showed
(lidswap.md: quiet stretches - before and after a word, the helper's 1 s pre-roll - got a language of their own,
"Okay." came out as French + Russian):
  quiet   frames whose loudness is QUIET dB under the line's loudest (or under -50 dBFS) do not vote: windows
          that are mostly quiet are left out, and the quiet frames take their neighbours' language
  switch  a language change costs SWITCH (in log-probability, per change) in a Viterbi pass over the frames, so a
          line holds its language unless the evidence for a change is strong (instead of the 3-frame vote)
Words wrong on single-language lines, the 20 mixed lines and the one-word callouts, as in lidswap.py.

    <conda>/envs/pcvoice/python.exe bench/lidtune.py

Report: export/asrbench/lidtune2.md (run 1, with the configurations now commented out: lidtune.md)
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

RATE = asr.RATE


def frame_db(x, hop):
    n = int(hop * RATE)
    k = int(np.ceil(len(x) / n))
    return np.array([10 * np.log10(np.mean(x[i * n:(i + 1) * n] ** 2) + 1e-12) for i in range(k)])


def segments(models, x, fallback, quiet, switch, win, cache, logmean=False, span=False):
    hop = asr.LID_HOP
    dur = len(x) / RATE
    L = len(asr.MIXED_LANGS)
    db = frame_db(x, hop)
    n = len(db)
    voiced = np.ones(n, bool) if quiet is None else (db > max(db.max() - quiet, -50.0))
    if not voiced.any():
        return [(fallback, 0.0, dur)]
    a, b = np.flatnonzero(voiced)[[0, -1]]
    if dur < win + hop * 2 or (span and (b + 1 - a) * hop < win + hop * 2):   # (span: by the speech, not the clip)
        p = asr.lid_probs(models, x[int(a * hop * RATE):int((b + 1) * hop * RATE)])
        lang = asr.MIXED_LANGS[int(p.argmax())] if p.max() > asr.LID_SURE else fallback
        return [(lang, 0.0, dur)]
    score = np.zeros((n, L))
    cnt = np.zeros(n)
    t = 0.0
    while t + win <= dur + 1e-6:
        f0, f1 = int(round(t / hop)), min(n, int(np.ceil((t + win) / hop)))
        if voiced[f0:f1].mean() >= 0.3:                       # (a mostly quiet window does not vote)
            key = (round(t / hop), win)
            if key not in cache:
                cache[key] = asr.lid_probs(models, x[int(t * RATE):int((t + win) * RATE)])
            score[f0:f1] += cache[key] if logmean else np.log(cache[key] + 1e-9)
            cnt[f0:f1] += 1
        t += hop
    has = (cnt > 0) & voiced
    score[has] /= cnt[has, None]
    if logmean:
        score[has] = np.log(score[has] + 1e-9)
    score[~has] = 0.0                                         # (no evidence: any language, the path carries on)
    if switch is None:                                        # (the old way: 3-frame vote on the argmax)
        lab = score.argmax(1)
        lab[~has] = -1
        out = lab.copy()
        for f in range(1, n - 1):
            w = [l for l in lab[f - 1:f + 2] if l >= 0]
            if w:
                out[f] = max(set(w), key=w.count)
        lab = out
        last = next((l for l in lab if l >= 0), None)
        if last is None:
            return [(fallback, 0.0, dur)]
        for f in range(n):
            if lab[f] < 0:
                lab[f] = last
            last = lab[f]
    else:                                                     # (Viterbi: a change costs `switch`)
        cost = np.zeros(L)
        back = np.zeros((n, L), int)
        for f in range(n):
            stay = cost
            move = cost.max() - switch
            best_prev = int(cost.argmax())
            back[f] = np.where(stay >= move, np.arange(L), best_prev)
            cost = np.maximum(stay, move) + score[f]
        lab = np.zeros(n, int)
        lab[-1] = int(cost.argmax())
        for f in range(n - 1, 0, -1):
            lab[f - 1] = back[f, lab[f]]
    segs = []
    for f in range(n):
        l = asr.MIXED_LANGS[lab[f]]
        if segs and segs[-1][0] == l:
            segs[-1][2] = (f + 1) * hop
        else:
            segs.append([l, f * hop, (f + 1) * hop])
    while len(segs) > 1:
        short = [i for i, s in enumerate(segs) if s[2] - s[1] < asr.LID_MIN]
        if not short:
            break
        i = short[0]
        j = i - 1 if i > 0 else i + 1
        segs[j][1], segs[j][2] = min(segs[j][1], segs[i][1]), max(segs[j][2], segs[i][2])
        segs.pop(i)
        k = 0
        while k < len(segs) - 1:
            if segs[k][0] == segs[k + 1][0]:
                segs[k][2] = segs[k + 1][2]
                segs.pop(k + 1)
            else:
                k += 1
    segs[0][1], segs[-1][2] = 0.0, dur
    for i in range(1, len(segs)):
        cut = asr.quiet_point(x, segs[i][1])
        segs[i - 1][2] = segs[i][1] = cut
    return [tuple(s) for s in segs]


def toks(t):
    out = []
    for w in B.norm(t, 'en'):
        out += B.norm(w, 'zh') if re.search(r'[\u3400-\u9fff]', w) else [w]
    return out


CONFIGS = [   # (label, quiet dB or None, switch cost or None = the 3-frame vote, window s, options)
    # (run 1, 2026-10-05 01:58: the frame score = the mean of the windows' log-probabilities)
    # ('as now (vote, every frame counts)', None, None, 1.0, {}),
    # ('quiet frames left out, vote', 25, None, 1.0, {}),
    # ('quiet left out, switch cost 1', 25, 1.0, 1.0, {}),
    # ('quiet left out, switch cost 2', 25, 2.0, 1.0, {}),
    ('quiet left out, switch cost 4', 25, 4.0, 1.0, {}),
    # ('quiet left out, switch cost 8', 25, 8.0, 1.0, {}),
    # ('quiet left out, switch cost 4, 1.5 s windows', 25, 4.0, 1.5, {}),
    # (run 2: the frame score = the log of the windows' mean probability - softer; a short SPEECH span -> one stretch)
    ('log of mean probability, quiet left out, switch cost 2', 25, 2.0, 1.0, dict(logmean=True)),
    ('log of mean probability, quiet left out, switch cost 4', 25, 4.0, 1.0, dict(logmean=True)),
    ('switch cost 4, short speech: one stretch (else English)', 25, 4.0, 1.0, dict(span=True)),
    ("... else the language of the player's line before", 25, 4.0, 1.0, dict(span=True, fallback='own')),
]


def main():
    items = json.load(open(os.path.join(B.OUT, 'lid', 'items.json'), encoding='utf-8'))
    models = asr.Models(threads=4, log=lambda s: None)
    for name in ('parakeet', 'gigaam', 'sensevoice', 'langid'):
        models.get(name)
    audio = {it['path']: B.read(it['path']) for it in items}
    lid_cache = {it['path']: {} for it in items}
    asr_cache = {}

    def write(x, path, lang, a, b):
        key = (path, asr.roll_model(lang), round(a, 3), round(b, 3))
        if key not in asr_cache:
            t, _ = models.offline(asr.roll_model(lang), x[int(a * RATE):int(b * RATE)])
            asr_cache[key] = asr.tidy(t)
        return asr_cache[key]

    rows = []
    for label, quiet, switch, win, opt in CONFIGS:
        opt = dict(opt)
        fb = opt.pop('fallback', 'en')
        t0 = time.perf_counter()
        acc, wrong = {}, {}
        for it in items:
            x = audio[it['path']]
            segs = segments(models, x, it['lang'] if fb == 'own' else 'en', quiet, switch, win, lid_cache[it['path']], **opt)
            text = ' '.join(t for t in (write(x, it['path'], l, a, b) for l, a, b in segs) if t)
            key = 'mixed' if it['kind'] == 'mixed' else ('%s %s' % (it['kind'], it['lang']))
            r = acc.setdefault(key, [0, 0])
            r[0] += B.edits(toks(it['text']), toks(text))
            r[1] += len(toks(it['text']))
            if it['kind'] != 'mixed':
                w = wrong.setdefault(it['kind'], [0, 0])
                w[0] += int(any(l != it['lang'] for l, _, _ in segs))
                w[1] += 1
        rows.append((label, acc, wrong))
        print(label, {k: '%.1f' % (100.0 * v[0] / v[1]) for k, v in sorted(acc.items())}, wrong, '%.0f s' % (time.perf_counter() - t0), flush=True)
    keys = ['line en', 'line ru', 'line zh', 'line es', 'line de', 'mixed', 'word en', 'word ru', 'word zh']
    L = ['# "Auto" stitching with SpeechBrain: quiet frames and a switch cost (%s)' % time.strftime('%Y-%m-%d %H:%M'), '',
         'SpeechBrain VoxLingua107 ECAPA (our ONNX), 10 candidates, windows every %.2f s, stretches under %.1f s merged. Words '
         'wrong (%%; Chinese by characters), clean + noisy-room clips. "Wrong language": items with any stretch in another '
         'language. AmberNet as the helper had it (lidswap.md): lines en 3.2, ru 8.2, zh 9.3, es 2.6, de 23.9, mixed 6.7; '
         'one word en 41.8, ru 35.0, zh 77.8.' % (asr.LID_HOP, asr.LID_MIN), '',
         '"... else the language of the player\'s line before": a short line in Auto falls back to the language the '
         'helper found in the player\'s previous line (here: the item\'s own language, as if the line before was in it).', '',
         '| stitching | ' + ' | '.join(keys) + ' | wrong language: lines | one word |', '|---|' + '---|' * (len(keys) + 2)]
    for label, acc, wrong in rows:
        L.append('| %s | %s | %d / %d | %d / %d |' % (label, ' | '.join('%.1f' % (100.0 * acc[k][0] / acc[k][1]) for k in keys),
                                                      wrong['line'][0], wrong['line'][1], wrong['word'][0], wrong['word'][1]))
    open(os.path.join(B.OUT, 'lidtune2.md'), 'w', encoding='utf-8').write('\n'.join(L) + '\n')
    print('\n'.join(L))


if __name__ == '__main__':
    sys.exit(main())
