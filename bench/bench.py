"""Speech-to-text benchmark for the voice helper: Nemotron 3.5 ASR Streaming (sherpa-onnx) against Whisper
small / medium (faster-whisper), on the clips from make_clips.py, on this machine's CPU.

    C:/Users/user/miniconda3/envs/pcbench/python.exe bench/bench.py            # everything, then the report
    C:/Users/user/miniconda3/envs/pcbench/python.exe bench/bench.py --no-load  # without the busy core
    C:/Users/user/miniconda3/envs/pcbench/python.exe bench/bench.py --report   # only the report again

Each model runs in its own process (its memory is its own) with 4 threads, while another process keeps one
core busy (Teardown's main thread does). Per clip it records the text, the time, and:
  Whisper   the whole utterance is transcribed after it ends: the delay = the transcription time (plus
            the wait to be sure the speaker stopped, the same for every model, not counted)
  Nemotron  fed 0.1 s of audio at a time as a microphone would; each step's compute time is measured and
            replayed on a real-time clock: the delay = from the end of the speech until the live text was
            already complete (it is shown while the speaker talks). A new stream per line (started 0.5 s
            before the speech, as after a speech detector), or "-live": one stream kept listening through all
            the lines of a language, its text cleared after each (as a helper whose microphone stays on)
Word timestamps are checked against the Windows voices' own word positions.
Results: export/asrbench/results/*.json, report: export/asrbench/report.md
"""
import argparse
import json
import os
import statistics
import subprocess
import sys
import time
import unicodedata
import wave

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(os.path.dirname(HERE), 'export', 'asrbench')
RES = os.path.join(OUT, 'results')
THREADS = 4
STEP = 0.1                     # s of audio per microphone block

NEMO = 'csukuangfj2/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-%s-2026-06-11'
CONFIGS = {
    'whisper-small':      dict(kind='whisper', model='Systran/faster-whisper-small', words=True),
    'whisper-small-nots': dict(kind='whisper', model='Systran/faster-whisper-small', words=False),
    'whisper-medium':     dict(kind='whisper', model='Systran/faster-whisper-medium', words=True),
    'nemotron-560-int8':  dict(kind='nemo', model=NEMO % '560ms-int8', chunk=0.56, int8=True, lang=True),
    'nemotron-560-int8-live': dict(kind='nemo', model=NEMO % '560ms-int8', chunk=0.56, int8=True, lang=True, live=True),
    'nemotron-560-int8-auto': dict(kind='nemo', model=NEMO % '560ms-int8', chunk=0.56, int8=True, lang=False),
    'nemotron-560-fp32':  dict(kind='nemo', model=NEMO % '560ms', chunk=0.56, int8=False, lang=True),
    'nemotron-160-int8':  dict(kind='nemo', model=NEMO % '160ms-int8', chunk=0.16, int8=True, lang=True),
    'nemotron-560-int8-hi':      dict(kind='nemo', model=NEMO % '560ms-int8', chunk=0.56, int8=True, lang=True, prime='en'),
    'nemotron-560-int8-hi-lang': dict(kind='nemo', model=NEMO % '560ms-int8', chunk=0.56, int8=True, lang=True, prime='lang'),
    'nemotron-560-int8-beep':    dict(kind='nemo', model=NEMO % '560ms-int8', chunk=0.56, int8=True, lang=True, prime='beep'),
    'nemotron-560-int8-noise':   dict(kind='nemo', model=NEMO % '560ms-int8', chunk=0.56, int8=True, lang=True, prime='noise'),
    'nemotron-560-int8-count':   dict(kind='nemo', model=NEMO % '560ms-int8', chunk=0.56, int8=True, lang=True, prime='count'),
    # round 1: decoder settings, and the 1.12 s chunk
    'nemotron-560-int8-bp1':     dict(kind='nemo', model=NEMO % '560ms-int8', chunk=0.56, int8=True, lang=True, bp=1.0),
    'nemotron-560-int8-bp2':     dict(kind='nemo', model=NEMO % '560ms-int8', chunk=0.56, int8=True, lang=True, bp=2.0),
    'nemotron-1120-int8':        dict(kind='nemo', model=NEMO % '1120ms-int8', chunk=1.12, int8=True, lang=True),
    # (beam search and hotwords: sherpa-onnx 1.13.8 runs NeMo streaming transducers with greedy search only -
    #  'Unsupported decoding method: modified_beam_search')
}
ORDER = list(CONFIGS)
PRIMERS = {'en': ('vits-piper-en_US-ryan-medium', 'Hi.'), 'ru': ('vits-piper-ru_RU-dmitri-medium', 'Привет.'),
           'zh': ('vits-piper-zh_CN-huayan-medium', '你好。'), 'es': ('vits-piper-es_ES-davefx-medium', 'Hola.'),
           'de': ('vits-piper-de_DE-thorsten-medium', 'Hallo.'), 'count': ('vits-piper-en_US-ryan-medium', 'One, two, three.')}
PRIME_CUT = 0.2        # s into the clip: tokens stamped before this came from the primer (the clip's speech starts at 0.5)

# hotwords: the game's own vocabulary, boosted while decoding. Half of them are in the clips (where the
# models erred or might); the other half are not (they must not turn up where nobody said them).
HOTWORDS = [
    'great hall', 'chalice', 'lever', 'sesame', 'flashlight', 'ledge', 'windmill', 'quiet', 'loot',
    'рычаг', 'золотую чашу', 'мельницы', 'картиной', 'корабль', 'подвал',
    '画', '钥匙', '拉杆', '金杯', '风车',
    'crowbar', 'drawbridge', 'gargoyle', 'catacombs', 'лопата', 'факел', '火把', '吊桥',
]
HOT_SCORE = 2.0


def hotwords_file(tokens_path):
    """write the hotwords as the model's own word pieces, space-separated: the model ships no scoring file for
    sherpa-onnx to split words itself. Each word becomes the fewest pieces from tokens.txt ("▁" starts a word);
    with modeling_unit "cjkchar" sherpa-onnx takes space-separated pieces as they are."""
    vocab = set()
    for line in open(tokens_path, encoding='utf-8'):
        parts = line.rstrip('\n').rsplit(' ', 1)
        if len(parts) == 2:
            vocab.add(parts[0])

    def split(word):
        best = [None] * (len(word) + 1)               # best[i]: fewest pieces covering word[:i]
        best[0] = []
        for i in range(1, len(word) + 1):
            for j in range(max(0, i - 20), i):
                if best[j] is not None and word[j:i] in vocab and (best[i] is None or len(best[j]) + 1 < len(best[i])):
                    best[i] = best[j] + [word[j:i]]
        return best[-1]
    lines = []
    for phrase in HOTWORDS:
        pieces = []
        for w in phrase.split():
            got = split('▁' + w) or split('▁' + w.capitalize())
            if got is None:
                pieces = None
                break
            pieces += got
        if pieces:
            lines.append(' '.join(pieces))
    path = os.path.join(OUT, 'hotwords.txt')
    open(path, 'w', encoding='utf-8').write('\n'.join(lines) + '\n')
    return path


# ---------------------------------------------------------------- scoring
_t2s = None


def norm(text, lang):
    text = unicodedata.normalize('NFKC', text).lower().replace('ё', 'е')
    text = ''.join(' ' if unicodedata.category(ch)[0] in 'PS' else ch for ch in text)
    if lang == 'zh':                                            # (Chinese: characters; traditional ones count as simplified)
        global _t2s
        if _t2s is None:
            import opencc
            _t2s = opencc.OpenCC('t2s')
        return [ch for ch in _t2s.convert(text) if not ch.isspace()]
    return text.split()


def edits(ref, hyp):
    d = list(range(len(hyp) + 1))
    for i in range(1, len(ref) + 1):
        prev, d[0] = d[0], i
        for j in range(1, len(hyp) + 1):
            cur = min(d[j] + 1, d[j - 1] + 1, prev + (ref[i - 1] != hyp[j - 1]))
            prev, d[j] = d[j], cur
    return d[len(hyp)]


def read(path):
    with wave.open(path, 'rb') as w:
        return np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float32) / 32768.0


# ---------------------------------------------------------------- the models
class Whisper:
    def __init__(self, cfg):
        from faster_whisper import WhisperModel
        self.cfg = cfg
        self.m = WhisperModel(cfg['model'], device='cpu', compute_type='int8', cpu_threads=THREADS)

    def run(self, x, clip):
        t0 = time.perf_counter()
        segs, _ = self.m.transcribe(x, language=clip['lang'], beam_size=5, temperature=0.0, condition_on_previous_text=False,
                                    without_timestamps=False, word_timestamps=self.cfg['words'], vad_filter=False)
        segs = list(segs)
        took = time.perf_counter() - t0
        words = [(w.word.strip(), w.start) for s in segs for w in (s.words or [])]
        return dict(text=''.join(s.text for s in segs).strip(), compute=took, delay=took, words=words or None)


class Nemotron:
    def __init__(self, cfg):
        import sherpa_onnx
        from huggingface_hub import snapshot_download
        self.cfg = cfg
        d = snapshot_download(cfg['model'])
        sfx = '.int8.onnx' if cfg['int8'] else '.onnx'
        extra = {}
        if cfg.get('beam') or cfg.get('hot'):
            extra.update(decoding_method='modified_beam_search', max_active_paths=cfg.get('beam', 4))
        if cfg.get('bp'):
            extra['blank_penalty'] = cfg['bp']
        if cfg.get('hot'):                 # (game words, split into the model's own word pieces by us: see hotwords_file)
            extra.update(hotwords_file=hotwords_file(os.path.join(d, 'tokens.txt')), hotwords_score=HOT_SCORE,
                         modeling_unit='cjkchar')
        self.r = sherpa_onnx.OnlineRecognizer.from_transducer(
            tokens=os.path.join(d, 'tokens.txt'), encoder=os.path.join(d, 'encoder' + sfx), decoder=os.path.join(d, 'decoder' + sfx),
            joiner=os.path.join(d, 'joiner' + sfx), num_threads=THREADS, **extra)

    stream = None
    stream_lang = None
    primers = {}

    def primer(self, lang):
        """a short spoken "hi" fed before each utterance ("-hi": always English; "-hi-lang": in the utterance's
        language), made once with the benchmark's Piper voices and cut to the speech"""
        if lang not in self.primers and lang in ('beep', 'noise'):
            n = int(16000 * (0.3 if lang == 'beep' else 0.5))
            t = np.arange(n) / 16000.0
            if lang == 'beep':                  # (a 1 kHz tone, soft edges)
                y = np.sin(2 * np.pi * 1000 * t) * np.minimum(1, np.minimum(t, t[::-1]) / 0.01) * 0.3
            else:                               # (a hiss at about speech level)
                y = np.random.default_rng(3).standard_normal(n) * 0.08
            self.primers[lang] = np.concatenate([y.astype(np.float32), np.zeros(int(16000 * 0.15), np.float32)])
        if lang not in self.primers:
            import make_clips as M
            voice, text = PRIMERS[lang]
            y, sr = M.piper(voice, text)
            y = M.to16k(y, sr)
            y = y / (np.abs(y).max() + 1e-9) * 0.5
            s0, s1 = M.speech_span(y, 0)
            self.primers[lang] = np.concatenate([y[int(s0 * 16000):int(s1 * 16000)], np.zeros(int(16000 * 0.15), np.float32)])
        return self.primers[lang]

    def run(self, x, clip):
        r = self.r
        prime = self.cfg.get('prime')
        P = 0.0
        if prime:                           # (the primer goes in first, all at once: it is a recording, not live)
            p = self.primer(clip['lang'] if prime == 'lang' else prime)
            P = len(p) / 16000.0
            x = np.concatenate([p, x])
        live = self.cfg.get('live')
        if live and self.stream is not None and self.stream_lang == clip['lang']:
            s = self.stream                 # (kept listening: the same stream as the last line, its text cleared)
        else:
            s = r.create_stream()
            if self.cfg['lang']:
                s.set_option('language', clip['lang'])
            if live:
                self.stream, self.stream_lang = s, clip['lang']
        n = int(16000 * STEP)
        busy = 0.0                  # the replayed real-time clock: when the decoder is free again
        timeline = []               # (clock, text)
        compute = 0.0
        for k in range(0, len(x), n):
            arrive = max(0.0, (k + n) / 16000.0 - P)       # (the clip's own clock; the primer is there at 0)
            s.accept_waveform(16000, x[k:k + n])
            if not r.is_ready(s):
                continue
            t0 = time.perf_counter()
            while r.is_ready(s):
                r.decode_stream(s)
            dt = time.perf_counter() - t0
            compute += dt
            busy = max(arrive, busy) + dt
            timeline.append((busy, r.get_result(s)))
        end = len(x) / 16000.0 - P
        if live:                        # (kept listening: what the line added, then its text cleared, the model's memory kept)
            res = r.get_result_all(s)
            busy = max(end, busy)
            r.reset(s)
        else:                           # (the end: the rest flushed)
            t0 = time.perf_counter()
            s.input_finished()
            while r.is_ready(s):
                r.decode_stream(s)
            dt = time.perf_counter() - t0
            compute += dt
            busy = max(end, busy) + dt
            res = r.get_result_all(s)
        toks, stamps = list(res.tokens), [t - P for t in res.timestamps]
        cut = ''
        if prime:                       # (what came out while the primer played - "Hi", "Hey", anything - is dropped)
            keep = [i for i, t in enumerate(stamps) if t >= PRIME_CUT]
            cut = ''.join(toks[i] for i in range(len(toks)) if i not in keep).strip()
            toks, stamps = [toks[i] for i in keep], [stamps[i] for i in keep]
            final = ''.join(toks).strip()
        else:
            final = res.text.strip()
        timeline.append((busy, res.text.strip()))
        target = norm(final, clip['lang'])
        n_cut = len(norm(cut, clip['lang']))
        done = next((t for t, txt in timeline if norm(txt, clip['lang'])[n_cut:] == target), busy)
        words, cur = [], None
        for tok, ts in zip(toks, stamps):                       # (a token starting with a space starts a word)
            if tok.startswith(' ') or cur is None:
                cur = [tok.strip(), ts]
                words.append(cur)
            else:
                cur[0] += tok
        words = [(w, t) for w, t in words if w]
        return dict(text=final, compute=compute, delay=max(0.0, done - clip['speech_end']), words=words or None,
                    streamed=done < end, primed=cut if prime else None)


def worker(name, cond, out):
    import psutil
    cfg = CONFIGS[name]
    proc = psutil.Process()
    t0 = time.perf_counter()
    model = Whisper(cfg) if cfg['kind'] == 'whisper' else Nemotron(cfg)
    load = time.perf_counter() - t0
    clips = json.load(open(os.path.join(OUT, 'clips.json'), encoding='utf-8'))
    cpu0 = sum(proc.cpu_times()[:2])
    peak = proc.memory_info().rss
    rows = []
    w0 = time.perf_counter()
    for c in clips:
        x = read(os.path.join(OUT, 'clips', '%s_%s.wav' % (c['id'], cond)))
        r = model.run(x, c)
        peak = max(peak, proc.memory_info().rss)
        ref, hyp = norm(c['text'], c['lang']), norm(r['text'], c['lang'])
        r.update(id=c['id'], lang=c['lang'], ref=c['text'], errors=edits(ref, hyp), n=len(ref), dur=c['dur'])
        rows.append(r)
    wall = time.perf_counter() - w0
    cpu = sum(proc.cpu_times()[:2]) - cpu0
    json.dump(dict(name=name, cond=cond, load=load, wall=wall, cpu=cpu, peak_mb=peak / 1e6, rows=rows),
              open(out, 'w', encoding='utf-8'), ensure_ascii=False, indent=1)


# ---------------------------------------------------------------- running it all
def spinner():
    """a process keeping one core busy (as Teardown's main thread does)"""
    return subprocess.Popen([sys.executable, '-c', 'while True: pass'])


def report():
    clips = {c['id']: c for c in json.load(open(os.path.join(OUT, 'clips.json'), encoding='utf-8'))}
    langs = ['en', 'ru', 'zh', 'es', 'de']
    res = {}
    for f in os.listdir(RES):
        if f.endswith('.json'):
            d = json.load(open(os.path.join(RES, f), encoding='utf-8'))
            res[(d['name'], d['cond'])] = d
    L = []
    say = L.append
    say('# Speech-to-text benchmark (%s)' % time.strftime('%Y-%m-%d %H:%M'))
    say('')
    say('Computer voices (Windows David / Mark / Zira, Piper), %d lines; `room` = webcam-in-a-room version (echo, band-limit, '
        'noise 15 dB under the voice). CPU: Ryzen 7 3700X, 4 threads per model%s. Error rate: words (Chinese: characters), '
        'after lower-casing and removing punctuation.' % (sum(1 for c in clips.values() if c['text']),
                                                         ', one other core kept busy' if os.path.exists(os.path.join(RES, 'LOAD')) else ''))
    for cond in ('clean', 'room'):
        say('')
        say('## %s' % cond)
        say('')
        say('| model | ' + ' | '.join('%s err %%' % l for l in langs) + ' | delay after speech, median / p90 | CPU per s of speech | made-up text on noise | memory | load |')
        say('|---|' + '---|' * (len(langs) + 5))
        for name in ORDER:
            d = res.get((name, cond))
            if not d:
                continue
            rows = [r for r in d['rows'] if r['n']]
            errs = []
            for l in langs:
                rr = [r for r in rows if r['lang'] == l]
                errs.append('%.1f' % (100.0 * sum(r['errors'] for r in rr) / max(1, sum(r['n'] for r in rr))) if rr else '-')
            delays = sorted(r['delay'] for r in rows)
            p90 = delays[int(0.9 * (len(delays) - 1) + 0.5)]
            speech = sum(clips[r['id']]['speech_end'] - clips[r['id']]['speech_start'] for r in rows)
            made_up = [r['text'] for r in d['rows'] if not r['n'] and r['text'].strip()]
            say('| %s | %s | %.2f / %.2f s | %.2f s | %d of 4%s | %.0f MB | %.1f s |' % (
                name, ' | '.join(errs), statistics.median(delays), p90, sum(r['compute'] for r in rows) / speech,
                len(made_up), (' ("%s")' % made_up[0][:30]) if made_up else '', d['peak_mb'], d['load']))
    # word timestamps (the Windows voices' clips, clean)
    say('')
    say('## Word timestamps (clean, the %d Windows-voice clips: start of each word against the voice\'s own)' %
        sum(1 for c in clips.values() if c['words']))
    say('')
    say('| model | words matched | median error | 90 %% within |')
    say('|---|---|---|---|')
    for name in ORDER:
        d = res.get((name, 'clean'))
        if not d:
            continue
        errs, total = [], 0
        for r in d['rows']:
            c = clips[r['id']]
            if not c['words']:
                continue
            total += len(c['words'])
            if not r.get('words'):
                continue
            ref = [(norm(w, 'en'), t) for w, t in c['words']]
            hyp = [(norm(w, 'en'), t) for w, t in r['words']]
            j = 0
            for wr, tr in ref:                       # (each reference word: the next hypothesis word that is it)
                for k in range(j, min(len(hyp), j + 3)):
                    if hyp[k][0] == wr:
                        errs.append(abs(hyp[k][1] - tr))
                        j = k + 1
                        break
        if errs:
            errs.sort()
            say('| %s | %d of %d | %.0f ms | %.0f ms |' % (name, len(errs), total, statistics.median(errs) * 1000, errs[int(0.9 * (len(errs) - 1))] * 1000))
        else:
            say('| %s | none (no word timestamps) | | |' % name)
    # priming: what the "hi" before each line turned into (it is dropped from the text)
    primed = [n for n in ORDER if CONFIGS[n].get('prime') and (n, 'clean') in res]
    if primed:
        say('')
        say('## Priming: the "hi" fed before each line (dropped from the text by its timestamps)')
        say('')
        say('| model | condition | the "hi" came out as something | as what (most common) |')
        say('|---|---|---|---|')
        for name in primed:
            for cond in ('clean', 'room'):
                d = res.get((name, cond))
                if not d:
                    continue
                outs = [r.get('primed') or '' for r in d['rows']]
                seen = [o for o in outs if o]
                common = {}
                for o in seen:
                    common[o] = common.get(o, 0) + 1
                top = ', '.join('"%s" x%d' % kv for kv in sorted(common.items(), key=lambda kv: -kv[1])[:4])
                say('| %s | %s | %d of %d | %s |' % (name, cond, len(seen), len(outs), top))
    # the worst lines
    say('')
    say('## What each model got wrong (room)')
    for name in ORDER:
        d = res.get((name, 'room'))
        if not d:
            continue
        bad = [r for r in d['rows'] if r['n'] and r['errors']]
        say('')
        say('**%s**: %d lines with errors' % (name, len(bad)))
        for r in bad[:12]:
            say('- `%s` %s -> %s' % (r['id'], r['ref'], r['text'] or '(nothing)'))
    path = os.path.join(OUT, 'report.md')
    open(path, 'w', encoding='utf-8').write('\n'.join(L) + '\n')
    print('\n'.join(L))
    print('\nreport:', path)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--worker', nargs=3, metavar=('CONFIG', 'COND', 'OUT'))
    ap.add_argument('--only', nargs='*', help='configs to run (default: all)')
    ap.add_argument('--no-load', action='store_true')
    ap.add_argument('--report', action='store_true')
    args = ap.parse_args()
    if args.worker:
        return worker(*args.worker)
    if args.report:
        return report()
    os.makedirs(RES, exist_ok=True)
    flag = os.path.join(RES, 'LOAD')
    if args.no_load:
        if os.path.exists(flag):
            os.remove(flag)
    else:
        open(flag, 'w').write('1')
    spin = None if args.no_load else spinner()
    try:
        for name in args.only or ORDER:
            for cond in ('clean', 'room'):
                out = os.path.join(RES, '%s_%s.json' % (name, cond))
                t0 = time.time()
                p = subprocess.run([sys.executable, os.path.abspath(__file__), '--worker', name, cond, out],
                                   stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding='utf-8', errors='replace')
                status = 'ok' if p.returncode == 0 else 'FAILED (%d): %s' % (p.returncode, p.stdout[-400:])
                print('%-24s %-6s %5.0f s  %s' % (name, cond, time.time() - t0, status), flush=True)
    finally:
        if spin:
            spin.kill()
    report()


if __name__ == '__main__':
    sys.exit(main())
