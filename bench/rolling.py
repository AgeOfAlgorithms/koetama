"""Live words WITHOUT a streaming model: the line so far re-transcribed every INTERVAL s by the language's own
offline model (Parakeet v3 / GigaAM v3 / SenseVoice), against Nemotron 3.5 streaming.

    <conda>/envs/pcvoice/python.exe bench/rolling.py

How a rolling line works (RollingLine):
  - every INTERVAL s the audio since the line began (1 s before the speech) is transcribed again
  - shown: the words two passes in a row agree on (stable), then the rest (may still change)
  - a line longer than WINDOW s: the stable words up to TAIL s before the end are locked in, and from then on only
    the audio from just before the first unlocked word is transcribed (the word timestamps give the cut; words
    that start before the cut are dropped from the new pass - the overlap)
  - the end of the line (0.5 s of quiet): one pass over the whole line - the finished line, as now
Measured on the benchmark's room clips as a microphone would deliver them (a replayed clock): how long after a
word is said it shows (any / stable), how often shown words change, CPU per second of speech, the accuracy of
the live words and of the finished line, memory per model; and long lines (three clips in one, 10-17 s).
Report: export/asrbench/rolling.md
"""
import json
import os
import re
import statistics
import subprocess
import sys
import time

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(os.path.dirname(HERE), 'engine'))
import bench as B           # noqa: E402
import asr                  # noqa: E402

INTERVAL = 1.0
WINDOW = 8.0
TAIL = 3.0
END_WAIT = 0.5             # s of quiet before the line ends (the speech detector)
MODEL = {'en': 'parakeet', 'es': 'parakeet', 'de': 'parakeet', 'ru': 'gigaam', 'zh': 'sensevoice'}
CJK = re.compile(r'[\u3400-\u9fff]')


def words_of(res, offset):
    """[(word, start s)] from a result's tokens and timestamps (a token starting with a space, or a space token,
    starts a word; a Chinese character is a word)"""
    out, cur = [], None
    for tok, ts in zip(res.tokens, res.timestamps):
        t = offset + float(ts)
        if not tok.strip():
            cur = None
            continue
        if CJK.search(tok):
            for ch in tok.strip():
                out.append([ch, t])
            cur = None
            continue
        if tok.startswith(' ') or cur is None:
            cur = [tok.strip(), t]
            out.append(cur)
        else:
            cur[0] += tok
    return [(w, t) for w, t in out if w]


def key(w):
    return re.sub(r'[^\w]', '', w.lower())


class RollingLine:
    def __init__(self, rec, window=WINDOW):
        self.rec, self.window = rec, window
        self.locked = []          # [(word, start)] locked in
        self.cut = 0.0            # s: passes start here
        self.prev = []            # the last pass's words (after the cut)
        self.stable = 0           # how many of prev agree with the pass before

    def decode(self, audio, start):
        s = self.rec.create_stream()
        s.accept_waveform(16000, np.concatenate([audio[int(start * 16000):], np.zeros(4800, np.float32)]))
        self.rec.decode_stream(s)
        return words_of(s.result, start)

    def step(self, audio):
        """a pass over what has arrived; returns (shown words, how many of them are stable)"""
        now = len(audio) / 16000.0
        hyp = [(w, t) for w, t in self.decode(audio, self.cut) if not self.locked or t >= self.locked[-1][1] + 0.05]
        agree = 0
        while agree < min(len(hyp), len(self.prev)) and key(hyp[agree][0]) == key(self.prev[agree][0]):
            agree += 1
        self.prev, self.stable = hyp, agree
        if now - self.cut > self.window and agree:          # (long: lock the settled start, pass on from there)
            lock = [w for w in hyp[:agree] if w[1] < now - TAIL]
            if lock and len(lock) < len(hyp):
                self.locked += lock
                nxt = hyp[len(lock)][1]
                self.cut = max(0.0, nxt - 0.25)
                self.prev = hyp[len(lock):]
                self.stable = agree - len(lock)
        return self.locked + self.prev, len(self.locked) + self.stable

    def final(self, audio):
        s = self.rec.create_stream()
        s.accept_waveform(16000, np.concatenate([audio, np.zeros(4800, np.float32)]))
        self.rec.decode_stream(s)
        return s.result.text.strip()


def simulate(rec, audio, lang, s0, s1, ref, word_times, window=WINDOW):
    """audio: the clip from 1 s before the speech; s0, s1: speech start / end in it. A replayed clock: a pass
    starts at max(its time, the last pass's end). Returns metrics."""
    line = RollingLine(rec, window)
    clock, cpu, events = 0.0, 0.0, []
    t = s0 + INTERVAL
    end = s1 + END_WAIT
    while t < end:
        t0 = time.perf_counter()
        shown, stable = line.step(audio[:int(t * 16000)])
        dt = time.perf_counter() - t0
        cpu += dt
        clock = max(clock, t) + dt
        events.append((clock, [w for w, _ in shown], stable))
        t += INTERVAL
    t0 = time.perf_counter()
    final = line.final(audio[:int(min(len(audio) / 16000.0, end) * 16000)])
    dt = time.perf_counter() - t0
    cpu += dt
    clock = max(clock, end) + dt
    return metrics(events, final, clock, cpu, lang, s0, s1, ref, word_times)


def metrics(events, final, final_clock, cpu, lang, s0, s1, ref, word_times):
    rw = B.norm(ref, lang)
    n = len(rw)
    if word_times and len(word_times) == n:
        ends = [word_times[i + 1] if i + 1 < n else s1 for i in range(n)]
    else:                                                   # (no word times: spread evenly over the speech)
        ends = [s0 + (s1 - s0) * (i + 1) / n for i in range(n)]
    lag_any, lag_stable = [], []
    for i in range(n):
        ta = next((c for c, w, st in events if len(B.norm(' '.join(w), lang)) > i), final_clock)
        ts = next((c for c, w, st in events if len(B.norm(' '.join(w[:st]), lang)) > i), final_clock)
        lag_any.append(ta - ends[i])
        lag_stable.append(ts - ends[i])
    changes = 0
    for (_, a, _), (_, b, _) in zip(events, events[1:]):
        changes += sum(1 for x, y in zip(a, b) if key(x) != key(y))
    last_live = ' '.join(events[-1][1]) if events else ''
    return dict(lag_any=lag_any, lag_stable=lag_stable, changes=changes, passes=len(events), cpu=cpu, speech=s1 - s0,
                final=final, final_err=B.edits(rw, B.norm(final, lang)), live_err=B.edits(rw, B.norm(last_live, lang)), n=n,
                final_after_end=final_clock - s1)


def nemotron_live(models, audio, lang, s0, s1, ref, word_times):
    """Nemotron 3.5 streaming on the same audio and clock (0.1 s blocks; its words never change once shown,
    nearly): the same metrics, for comparison"""
    line = asr.Line(models, 1, lang, np.zeros(0, np.float32))
    clock, cpu, events = 0.0, 0.0, []
    n = 1600
    for k in range(0, int((s1 + END_WAIT) * 16000), n):
        t0 = time.perf_counter()
        line.feed(audio[k:k + n])
        dt = time.perf_counter() - t0
        cpu += dt
        clock = max(clock, (k + n) / 16000.0) + dt
        txt = line.text
        events.append((clock, txt.split() if lang != 'zh' else list(txt.replace(' ', '')), 10 ** 6))
    t0 = time.perf_counter()
    final, info = line.finish()
    dt = time.perf_counter() - t0
    clock = max(clock, s1 + END_WAIT) + dt
    m = metrics(events, final, clock, cpu + dt, lang, s0, s1, ref, word_times)
    m['final_used'] = info['used']
    return m


def memory(name):
    """resident memory of a fresh process holding just this model, after one pass (MB)"""
    code = ('import sys, numpy as np, psutil, os; sys.path.insert(0, %r); import asr; m = asr.Models(log=lambda s: None); '
            'base = psutil.Process().memory_info().rss; r = m.get(%r); '
            'x = np.random.default_rng(1).standard_normal(16000 * 4).astype(np.float32) * 0.05; '
            's = (r.create_stream()); s.accept_waveform(16000, x); '
            '(r.decode_stream(s) if %r != "nemotron" else [r.decode_stream(s) for _ in range(0) ]); '
            'print(round((psutil.Process().memory_info().rss - base) / 1e6), round(psutil.Process().memory_info().rss / 1e6))'
            % (os.path.join(os.path.dirname(HERE), 'engine'), name, name))
    out = subprocess.run([sys.executable, '-c', code], capture_output=True, text=True, env=dict(os.environ, HF_HUB_DISABLE_SYMLINKS_WARNING='1')).stdout.split()
    return (int(out[-2]), int(out[-1])) if len(out) >= 2 else (None, None)


def main():
    clips = [c for c in json.load(open(os.path.join(B.OUT, 'clips.json'), encoding='utf-8')) if c['text']]
    models = asr.Models(threads=B.THREADS, log=lambda s: None)
    spin = B.spinner()
    res = {}                  # (method, lang, cond) -> [metrics]
    try:
        for lang in ('en', 'ru', 'zh', 'es', 'de'):
            rec = models.get(MODEL[lang])
            for cond in ('clean', 'room'):
                for c in [c for c in clips if c['lang'] == lang]:
                    x = B.read(os.path.join(B.OUT, 'clips', '%s_%s.wav' % (c['id'], cond)))
                    a0 = max(0.0, c['speech_start'] - 1.0)
                    audio = x[int(a0 * 16000):]
                    s0, s1 = c['speech_start'] - a0, c['speech_end'] - a0
                    wt = [t - a0 for _, t in c['words']] if c['words'] else None
                    res.setdefault(('rolling', lang, cond), []).append(simulate(rec, audio, lang, s0, s1, c['text'], wt))
                    res.setdefault(('nemotron', lang, cond), []).append(nemotron_live(models, audio, lang, s0, s1, c['text'], wt))
            print(lang, 'done', flush=True)
        # long lines: three clips of a language in one (0.25 s between them), rolling with and without the window
        long_res = {}
        for lang in ('en', 'ru', 'zh'):
            rec = models.get(MODEL[lang])
            cc = [c for c in clips if c['lang'] == lang]
            for g in range(0, min(len(cc), 9), 3):
                group = cc[g:g + 3]
                parts, text = [np.zeros(16000, np.float32)], []
                for c in group:
                    x = B.read(os.path.join(B.OUT, 'clips', '%s_room.wav' % c['id']))
                    parts += [x[int(c['speech_start'] * 16000):int(c['speech_end'] * 16000)], np.zeros(4000, np.float32)]
                    text.append(c['text'])
                audio = np.concatenate(parts + [np.zeros(16000, np.float32)])
                s0, s1 = 1.0, len(audio) / 16000.0 - 1.0 - 0.25
                for win, label in ((WINDOW, 'windowed'), (999.0, 'whole line each pass')):
                    long_res.setdefault((label, lang), []).append(simulate(rec, audio, lang, s0, s1, ' '.join(text), None, window=win))
            print(lang, 'long lines done', flush=True)
    finally:
        spin.kill()
    mem = {name: memory(name) for name in ('nemotron', 'parakeet', 'gigaam', 'sensevoice')}
    report(res, long_res, mem)


def report(res, long_res, mem):
    med = lambda xs: statistics.median(xs) if xs else float('nan')

    def p90(xs):
        xs = sorted(xs)
        return xs[int(0.9 * (len(xs) - 1) + 0.5)] if xs else float('nan')
    L = ['# Live words without Nemotron (%s)' % time.strftime('%Y-%m-%d %H:%M'), '',
         'Rolling = the line so far re-transcribed every %.0f s by %s; stable = what two passes agree on. Lag = from the end of '
         'a word being said until it shows (median / 90 %%). Room clips unless noted; 4 threads, one other core busy.'
         % (INTERVAL, ', '.join('%s: %s' % kv for kv in MODEL.items())), '',
         '| | lang | lag, any (s) | lag, stable (s) | shown words changed per line | passes per line | CPU per s of speech | live words err % (room) | finished line err % clean / room | finished after the speech ends (s) |',
         '|---|---|---|---|---|---|---|---|---|---|']
    for lang in ('en', 'ru', 'zh', 'es', 'de'):
        for method in ('nemotron', 'rolling'):
            r = res.get((method, lang, 'room'), [])
            rc = res.get((method, lang, 'clean'), [])
            if not r:
                continue
            la = [x for m in r for x in m['lag_any']]
            ls = [x for m in r for x in m['lag_stable']]
            cpu = sum(m['cpu'] for m in r) / sum(m['speech'] for m in r)
            live = 100.0 * sum(m['live_err'] for m in r) / sum(m['n'] for m in r)
            fin = [100.0 * sum(m['final_err'] for m in rr) / sum(m['n'] for m in rr) for rr in (rc, r)]
            L.append('| %s | %s | %.2f / %.2f | %s | %.1f | %.1f | %.2f s | %.1f | %.1f / %.1f | %.2f |' % (
                'Nemotron streaming (now)' if method == 'nemotron' else '**rolling, every %.0f s**' % INTERVAL, lang,
                med(la), p90(la), '-' if method == 'nemotron' else '%.2f / %.2f' % (med(ls), p90(ls)),
                med([m['changes'] for m in r]), med([m['passes'] for m in r]), cpu, live, fin[0], fin[1],
                med([m['final_after_end'] for m in r])))
    L += ['', '(The finished line for Nemotron = the current design: Nemotron + the second pass for en/ru/zh/es; for de, Nemotron alone.)', '',
          '## Long lines (three lines said as one, 10-17 s, room)', '',
          '| | lang | lag, any (s) | words changed | passes | CPU per s of speech | live words err % | finished err % |', '|---|---|---|---|---|---|---|---|']
    for (label, lang), r in sorted(long_res.items(), key=lambda kv: (kv[0][1], kv[0][0])):
        la = [x for m in r for x in m['lag_any']]
        L.append('| %s | %s | %.2f / %.2f | %.1f | %.1f | %.2f s | %.1f | %.1f |' % (
            label, lang, med(la), p90(la), med([m['changes'] for m in r]), med([m['passes'] for m in r]),
            sum(m['cpu'] for m in r) / sum(m['speech'] for m in r),
            100.0 * sum(m['live_err'] for m in r) / sum(m['n'] for m in r), 100.0 * sum(m['final_err'] for m in r) / sum(m['n'] for m in r)))
    L += ['', '## Memory of each model alone (a fresh process: model loaded + one pass)', '', '| model | added MB | process MB |', '|---|---|---|']
    for name, (add, tot) in mem.items():
        L.append('| %s | %s | %s |' % (name, add, tot))
    path = os.path.join(B.OUT, 'rolling.md')
    open(path, 'w', encoding='utf-8').write('\n'.join(L) + '\n')
    print('\n'.join(L))


if __name__ == '__main__':
    sys.exit(main())
