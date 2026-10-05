"""Offline test of the helper's speech-to-text (asr.py) as a microphone would feed it: the benchmark's clips
(bench/make_clips.py: export/asrbench/clips) and one-word callouts, one language at a time, glued
into one long recording with pauses, fed in 50 ms blocks. Checks: one finished line per spoken line, the
right words (error rate), live words before each line ends, each language written by its own model.

    C:/Users/user/miniconda3/envs/pcvoice/python.exe engine/test_asr.py          (downloads the models once)
"""
import json
import os
import sys
import time

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(os.path.dirname(HERE), 'bench'))
import asr                  # noqa: E402

OUT = os.path.join(os.path.dirname(HERE), 'export', 'asrbench')
FAILED = NCHECK = 0


def check(cond, msg):
    global FAILED, NCHECK
    NCHECK += 1
    if not cond:
        FAILED += 1
    print(('ok   ' if cond else 'FAIL ') + msg, flush=True)


def norm(text, lang):
    import unicodedata
    text = unicodedata.normalize('NFKC', text).lower().replace('ё', 'е')
    text = ''.join(' ' if unicodedata.category(ch)[0] in 'PS' else ch for ch in text)
    if lang == 'zh':
        try:
            import opencc
            text = opencc.OpenCC('t2s').convert(text)
        except ImportError:
            pass
        return [ch for ch in text if not ch.isspace()]
    return text.split()


def edits(a, b):
    d = list(range(len(b) + 1))
    for i in range(1, len(a) + 1):
        prev, d[0] = d[0], i
        for j in range(1, len(b) + 1):
            cur = min(d[j] + 1, d[j - 1] + 1, prev + (a[i - 1] != b[j - 1]))
            prev, d[j] = d[j], cur
    return d[len(b)]


def read(path):
    import wave
    with wave.open(path, 'rb') as w:
        return np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float32) / 32768.0


def run(models, lang, pieces, gap=1.2):
    """pieces: [(reference text, audio)] glued with `gap` s of quiet; returns (finals, lives per utt, seconds)"""
    finals, lives = [], {}
    lst = asr.Listener(lambda u, t: lives.setdefault(u, []).append(t), lambda u, t, i: finals.append((u, t, i)),
                       log=lambda s: None, models=models)
    lst.set_language(lang)
    rng = np.random.default_rng(5)
    audio = [np.zeros(int(asr.RATE * 1.0), np.float32)]
    for _, x in pieces:
        audio += [x, (rng.standard_normal(int(asr.RATE * gap)) * 0.002).astype(np.float32)]
    audio = np.concatenate(audio)
    t0 = time.perf_counter()
    n = int(asr.RATE * 0.05)
    for k in range(0, len(audio), n):
        lst.feed(audio[k:k + n])
    lst.flush()
    return finals, lives, time.perf_counter() - t0, len(audio) / asr.RATE


def score(lang, pieces, finals):
    """the whole stream as one text (a pause may split a line in two: that is not an error);
    (word errors, words, lines fewer than said, lines more than said)"""
    refs = [w for t, _ in pieces for w in norm(t, lang)]
    hyps = [w for _, t, _ in finals for w in norm(t, lang)]
    n_hyp = sum(1 for _, t, _ in finals if t.strip())
    return edits(refs, hyps), len(refs), max(0, len(pieces) - n_hyp), max(0, n_hyp - len(pieces))


def main():
    clips = [c for c in json.load(open(os.path.join(OUT, 'clips.json'), encoding='utf-8')) if c['text']]
    models = asr.Models(threads=4, log=print)
    t0 = time.perf_counter()
    for name in ('parakeet', 'gigaam', 'sensevoice'):
        models.get(name)
    print('models loaded in %.1f s' % (time.perf_counter() - t0))
    expect = {'en': 4.0, 'ru': 9.0, 'zh': 10.0, 'es': 6.0, 'de': 22.0}      # (room: the benchmark's best per language, with a margin)
    for lang in ('en', 'ru', 'zh', 'es', 'de'):
        for cond in ('clean', 'room'):
            pieces = []
            for c in [c for c in clips if c['lang'] == lang]:
                x = read(os.path.join(OUT, 'clips', '%s_%s.wav' % (c['id'], cond)))
                s0, s1 = int((c['speech_start'] - 0.05) * asr.RATE), int((c['speech_end'] + 0.05) * asr.RATE)
                pieces.append((c['text'], x[max(0, s0):s1]))
            finals, lives, took, dur = run(models, lang, pieces)
            e, n, missed, extra = score(lang, pieces, finals)
            used = {}
            for _, _, info in finals:
                used[info['used']] = used.get(info['used'], 0) + 1
            live_lines = sum(1 for u, _, _ in finals if lives.get(u))
            err = 100.0 * e / max(1, n)
            print('  %s %-5s %2d lines -> %2d finals (%s), %d with live words first, error %.1f %%, %.1f s for %.1f s of audio'
                  % (lang, cond, len(pieces), len(finals), ', '.join('%s %d' % kv for kv in sorted(used.items())), live_lines, err, took, dur))
            check(missed == 0 and extra <= 2, '%s %s: every spoken line came out (%d split in two at a pause)' % (lang, cond, extra))
            check(live_lines >= len(finals) - 2, '%s %s: live words came before the finished line (%d of %d)' % (lang, cond, live_lines, len(pieces)))
            if cond == 'room':
                check(err <= expect[lang], '%s room: error %.1f %% (benchmark best + margin: %.0f %%)' % (lang, err, expect[lang]))
            check(used.get(asr.roll_model(lang), 0) >= len(pieces) - 1, "%s %s: the language's own model (%s) wrote the lines" % (lang, cond, asr.roll_model(lang)))
            check(took < dur * 0.6, '%s %s: faster than real time with room to spare (%.1f s for %.1f s)' % (lang, cond, took, dur))
            if e:
                print('      said:  %s' % ' / '.join(t for t, _ in pieces))
                print('      lines: %s' % ' / '.join('%s[%s]' % (t, i['used']) for _, t, i in finals))
    # one-word callouts (English: Parakeet for short lines)
    import make_clips as M
    words = [('Okay.', 'Microsoft David Desktop'), ('No.', 'Microsoft Zira Desktop'), ('Go.', 'Microsoft Mark'),
             ('Yes.', 'Microsoft David Desktop'), ('Run!', 'Microsoft Zira Desktop'), ('Wait.', 'Microsoft Mark'),
             ('Help!', 'Microsoft David Desktop'), ('Stop!', 'Microsoft Zira Desktop')]
    pieces = []
    tmp = os.path.join(OUT, 'ta_tmp.wav')
    for w, v in words:
        M.sapi(v, w, tmp)
        x, sr = M.read_wav(tmp)
        x = M.to16k(x, sr)
        x = x / (np.abs(x).max() + 1e-9) * 0.5
        s0, s1 = M.speech_span(x, 0)
        pieces.append((w, x[int(s0 * 16000):int(s1 * 16000) + 800]))
    os.remove(tmp)
    finals, lives, took, dur = run(models, 'en', pieces, gap=1.5)
    hits = sum(1 for (ref, _), (_, hyp, _) in zip(pieces, finals) if norm(ref, 'en') == norm(hyp, 'en'))
    print('  one-word callouts: %s' % ', '.join('%s->%s[%s]' % (r, h or '-', i['used']) for (r, _), (_, h, i) in zip(pieces, finals)))
    check(len(finals) == len(pieces), 'one-word callouts: each one is a line (%d of %d)' % (len(finals), len(pieces)))
    check(hits >= 6, 'one-word callouts: %d of %d right' % (hits, len(pieces)))
    # nothing said: no line
    rng = np.random.default_rng(9)
    noise = (rng.standard_normal(int(asr.RATE * 6)) * 0.01).astype(np.float32)
    lst = asr.Listener(lambda u, t: None, lambda u, t, i: got.append(t), log=lambda s: None, models=models)
    got = []
    for k in range(0, len(noise), 800):
        lst.feed(noise[k:k + 800])
    lst.flush()
    check(not [t for t in got if t.strip()], 'six seconds of hiss: no line (%s)' % got)
    print('\n%d checks, %d failed' % (NCHECK, FAILED))
    return 1 if FAILED else 0


if __name__ == '__main__':
    sys.exit(main())
