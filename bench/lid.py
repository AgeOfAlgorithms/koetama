"""Spoken language identification for the voice helper, and stitching mixed-language lines.

  models   Whisper tiny / base (faster-whisper, its language detection), AmberNet (NVIDIA, 107 languages, community
           ONNX export surogate/ambernet-langid, 29 M), SpeechBrain ECAPA VoxLingua107 (107 languages, Apache-2.0)
  1. which language: the benchmark's 52 lines (clean, room) and one-word callouts; top-1 among all the model's
     languages, and among the languages the helper has recognisers for (SUPPORTED)
  2. where it changes: mixed utterances (pieces said by voices of two languages, no pause); each model on
     overlapping windows -> language over time -> segments -> each segment transcribed by its language's model
     (Parakeet / GigaAM / SenseVoice), stitched; against the true segments and single-model transcription

Steps (each in its environment; run_lid.sh runs them all):
    pcbench  python bench/lid.py prep        audio for 1 and 2 -> export/asrbench/lid/
    pcbench  python bench/lid.py whisper     Whisper tiny + base -> lid/whisper.json
    pclid    python bench/lid.py classify    AmberNet + SpeechBrain -> lid/classify.json
    pcbench  python bench/lid.py report      segments, stitching, the report -> export/asrbench/lid.md
"""
import json
import os
import re
import sys
import time
import wave

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(os.path.dirname(HERE), 'export', 'asrbench')
LID = os.path.join(OUT, 'lid')
SUPPORTED = ['en', 'ru', 'zh', 'es', 'de', 'fr', 'it', 'pt', 'ja', 'ko']
WINDOWS = [1.0, 1.5, 2.0]
HOP = 0.25
THREADS = 4
PIPER = {'en': 'vits-piper-en_US-ryan-medium', 'ru': 'vits-piper-ru_RU-dmitri-medium', 'zh': 'vits-piper-zh_CN-huayan-medium',
         'es': 'vits-piper-es_ES-davefx-medium', 'de': 'vits-piper-de_DE-thorsten-medium'}
MIXED = [
    [('ru', 'Я нашёл'), ('en', 'the golden key'), ('ru', 'за картиной в зале.')],
    [('en', 'Wait for me,'), ('ru', 'я спускаюсь по лестнице.')],
    [('ru', 'Беги, оно прямо за тобой,'), ('en', 'run run run!')],
    [('zh', '我们需要'), ('en', 'a lever'), ('zh', '才能打开这扇门。')],
    [('en', 'Run,'), ('zh', '它就在你后面！')],
    [('es', 'Corre,'), ('en', 'it is right behind you!')],
    [('en', 'Okay guys,'), ('de', 'lauf, es ist direkt hinter dir!')],
    [('de', 'Ich habe den'), ('en', 'flashlight'), ('de', 'im Keller gefunden.')],
    [('en', 'I think the monster is in the basement,'), ('ru', 'давайте пойдём туда вместе.')],
    [('ru', 'Подождите меня у двери,'), ('en', 'I need to grab the flashlight first.')],
    [('zh', '我觉得我们应该先回船上，'), ('en', 'because it is getting dark outside.')],
    [('en', 'Did anyone see where the key went?'), ('zh', '我好像把它丢在大厅里了。')],
    # (one foreign word inside a sentence)
    [('en', 'Where is the'), ('ru', 'ключ'), ('en', 'for the tower door?')],
    [('zh', '这个'), ('en', 'chalice'), ('zh', '很值钱。')],
    # (three languages)
    [('en', 'Okay, listen,'), ('ru', 'ключ у меня,'), ('zh', '我们走吧。')],
    [('zh', '快点，'), ('en', 'the monster is coming,'), ('ru', 'бегите к кораблю!')],
    [('ru', 'Я нашёл фонарик,'), ('en', 'but the battery is dead,'), ('de', 'hat jemand eine Batterie?')],
    [('es', 'Hola amigos,'), ('en', 'I found the gold,'), ('zh', '我们回家吧。')],
    [('de', 'Achtung,'), ('en', 'there is a trap right here,'), ('ru', 'осторожно!')],
    [('ru', 'Слушайте,'), ('zh', '门在那边，'), ('en', 'follow me.')],
]


def write_wav(path, x):
    with wave.open(path, 'wb') as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(16000)
        w.writeframes((np.clip(x, -1, 1) * 32767).astype(np.int16).tobytes())


def read_wav(path):
    with wave.open(path, 'rb') as w:
        return np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(np.float32) / 32768


# ---------------------------------------------------------------- 0. the audio
def prep():
    sys.path.insert(0, HERE)
    import make_clips as M
    import shortwords as W
    os.makedirs(LID, exist_ok=True)
    items = []
    for c in json.load(open(os.path.join(OUT, 'clips.json'), encoding='utf-8')):
        if c['text']:
            for cond in ('clean', 'room'):
                x = read_wav(os.path.join(OUT, 'clips', '%s_%s.wav' % (c['id'], cond)))
                x = x[max(0, int((c['speech_start'] - 0.3) * 16000)):int((c['speech_end'] + 0.3) * 16000)]
                p = os.path.join(LID, 'line_%s_%s.wav' % (c['id'], cond))
                write_wav(p, x)
                items.append(dict(kind='line', id='%s_%s' % (c['id'], cond), lang=c['lang'], text=c['text'], path=p, dur=len(x) / 16000))
    for k, c in enumerate(W.make()):
        for cond in ('clean', 'room'):
            p = os.path.join(LID, 'word_%03d_%s.wav' % (k, cond))
            write_wav(p, c[cond])
            items.append(dict(kind='word', id='w%03d_%s' % (k, cond), lang=c['lang'], text=c['text'], path=p, dur=len(c[cond]) / 16000))
    for k, parts in enumerate(MIXED):
        pieces, segs, t = [np.zeros(int(16000 * 0.3), np.float32)], [], 0.3
        for lang, text in parts:
            x, sr = M.piper(PIPER[lang], text)
            x = M.to16k(x, sr)
            x = x / (np.abs(x).max() + 1e-9) * 0.5
            s0, s1 = M.speech_span(x, 0)
            x = x[int(s0 * 16000):int(s1 * 16000)]
            segs.append(dict(lang=lang, text=text, t0=t, t1=t + len(x) / 16000))
            pieces += [x, np.zeros(int(16000 * 0.06), np.float32)]
            t += len(x) / 16000 + 0.06
        x = np.concatenate(pieces[:-1] + [np.zeros(int(16000 * 0.3), np.float32)])
        p = os.path.join(LID, 'mixed_%02d.wav' % k)
        write_wav(p, x)
        items.append(dict(kind='mixed', id='m%02d' % k, lang=segs[0]['lang'], text=' '.join(s['text'] for s in segs), path=p,
                          dur=len(x) / 16000, segs=segs))
    json.dump(items, open(os.path.join(LID, 'items.json'), 'w', encoding='utf-8'), ensure_ascii=False, indent=1)
    print('%d items' % len(items))


def windows(dur):
    """(t0, t1) windows of each length in WINDOWS, every HOP s"""
    out = []
    for w in WINDOWS:
        t = 0.0
        while t + w <= dur + 1e-6 or (t == 0.0 and dur < w):
            out.append((round(t, 3), round(min(dur, t + w), 3), w))
            t += HOP
    return out


def run_model(name, fn, items):
    """fn(audio) -> {lang: prob}; whole items + windows over the mixed ones; times each call"""
    res = {}
    times = []
    for it in items:
        x = read_wav(it['path'])
        t0 = time.perf_counter()
        probs = fn(x)
        times.append((len(x) / 16000, time.perf_counter() - t0))
        r = dict(probs=probs)
        if it['kind'] == 'mixed':
            r['windows'] = []
            for a, b, w in windows(it['dur']):
                r['windows'].append(dict(t0=a, t1=b, w=w, probs=fn(x[int(a * 16000):int(b * 16000)])))
        res[it['id']] = r
    print(name, 'done', flush=True)
    return dict(results=res, times=times)


def top(probs, k=8):
    return dict(sorted(probs.items(), key=lambda kv: -kv[1])[:k] + [(l, probs.get(l, 0.0)) for l in SUPPORTED])


# ---------------------------------------------------------------- 1a. Whisper (env pcbench)
def whisper():
    from faster_whisper import WhisperModel
    items = json.load(open(os.path.join(LID, 'items.json'), encoding='utf-8'))
    out = {}
    for size in ('tiny',):                               # (base: too slow - every check runs a 30 s window; the user
                                                         #  ruled it out, 2026-10-05)
        m = WhisperModel(size, device='cpu', compute_type='int8', cpu_threads=THREADS)

        def fn(x, m=m):
            _, probs = m.detect_language(x)[0], dict(m.detect_language(x)[2])
            return top(probs)
        m.detect_language(np.zeros(16000, np.float32))
        out['Whisper %s' % size] = run_model('whisper ' + size, fn, items)
        out['Whisper %s' % size]['mb'] = {'tiny': 39, 'base': 74}[size]
        json.dump(out, open(os.path.join(LID, 'whisper.json'), 'w', encoding='utf-8'))   # (saved after each model)


# ---------------------------------------------------------------- 1b. AmberNet + SpeechBrain (env pclid)
def classify():
    import onnxruntime as ort
    from huggingface_hub import snapshot_download
    items = json.load(open(os.path.join(LID, 'items.json'), encoding='utf-8'))
    out = {}
    d = snapshot_download('surogate/ambernet-langid')
    so = ort.SessionOptions()
    so.intra_op_num_threads = THREADS
    sess = ort.InferenceSession(os.path.join(d, 'ambernet.onnx'), so, providers=['CPUExecutionProvider'])
    labels = json.load(open(os.path.join(d, 'config.json')))['labels']

    def amber(x):
        if len(x) < 1600:
            x = np.concatenate([x, np.zeros(1600 - len(x), np.float32)])
        logits, _ = sess.run(None, {'audio': x[None].astype(np.float32), 'audio_len': np.array([len(x)], np.int64)})
        p = np.exp(logits[0] - logits[0].max())
        p /= p.sum()
        return top({labels[i]: float(p[i]) for i in range(len(labels))})
    amber(np.zeros(16000, np.float32))
    out['AmberNet'] = run_model('ambernet', amber, items)
    out['AmberNet']['mb'] = round(os.path.getsize(os.path.join(d, 'ambernet.onnx')) / 1e6)
    json.dump(out, open(os.path.join(LID, 'classify.json'), 'w', encoding='utf-8'))      # (saved after each model)
    import torch
    torch.set_num_threads(THREADS)
    from speechbrain.inference.classifiers import EncoderClassifier
    from speechbrain.utils.fetching import LocalStrategy
    sb = EncoderClassifier.from_hparams(source='speechbrain/lang-id-voxlingua107-ecapa', savedir=os.path.join(LID, 'sb_model'),
                                        run_opts={'device': 'cpu'}, local_strategy=LocalStrategy.COPY)   # (Windows: no symlinks)
    codes = [lab.split(':')[0].strip() for lab in sb.hparams.label_encoder.decode_ndim(list(range(107)))]

    def sbrain(x):
        if len(x) < 1600:
            x = np.concatenate([x, np.zeros(1600 - len(x), np.float32)])
        with torch.no_grad():
            lp = sb.classify_batch(torch.from_numpy(x)[None])[0][0]
        p = torch.softmax(lp, -1).numpy()
        return top({codes[i]: float(p[i]) for i in range(len(codes))})
    sbrain(np.zeros(16000, np.float32))
    out['SpeechBrain ECAPA'] = run_model('speechbrain', sbrain, items)
    out['SpeechBrain ECAPA']['mb'] = round(sum(os.path.getsize(os.path.join(LID, 'sb_model', f)) for f in os.listdir(os.path.join(LID, 'sb_model'))
                                               if f.endswith('.ckpt')) / 1e6)
    json.dump(out, open(os.path.join(LID, 'classify.json'), 'w', encoding='utf-8'))


# ---------------------------------------------------------------- 2. segments and stitching (env pcbench / pcvoice)
def segment(windows_, dur, w, smooth=True):
    """language over time from the windows of length w: each 0.25 s frame takes the summed (supported-only)
    probabilities of the windows covering it; then frames vote 3 at a time; then runs under 0.5 s are merged
    into the neighbour. -> [(lang, t0, t1)]"""
    n = max(1, int(np.ceil(dur / HOP)))
    acc = np.zeros((n, len(SUPPORTED)))
    for wd in windows_:
        if abs(wd['w'] - w) > 1e-6:
            continue
        p = np.array([wd['probs'].get(l, 0.0) for l in SUPPORTED])
        p = p / (p.sum() + 1e-12)
        for f in range(int(wd['t0'] / HOP), min(n, int(np.ceil(wd['t1'] / HOP)))):
            acc[f] += p
    lab = acc.argmax(1)
    if smooth and n >= 3:
        lab2 = lab.copy()
        for f in range(1, n - 1):
            vals = [lab[f - 1], lab[f], lab[f + 1]]
            lab2[f] = max(set(vals), key=vals.count)
        lab = lab2
    segs = []
    for f in range(n):
        l = SUPPORTED[lab[f]]
        if segs and segs[-1][0] == l:
            segs[-1][2] = (f + 1) * HOP
        else:
            segs.append([l, f * HOP, (f + 1) * HOP])
    changed = True
    while changed and len(segs) > 1:
        changed = False
        for i, s in enumerate(segs):
            if s[2] - s[1] < 0.5:
                j = i - 1 if i > 0 else i + 1
                segs[j][1], segs[j][2] = min(segs[j][1], s[1]), max(segs[j][2], s[2])
                segs.pop(i)
                k = 0                                           # (neighbours of the same language join)
                while k < len(segs) - 1:
                    if segs[k][0] == segs[k + 1][0]:
                        segs[k][2] = segs[k + 1][2]
                        segs.pop(k + 1)
                    else:
                        k += 1
                changed = True
                break
    segs[-1][2] = dur
    return [tuple(s) for s in segs]


def quiet_point(x, t, span=0.3):
    """the quietest 20 ms within +-span s of t (a cut there rarely splits a word)"""
    a, b = int(max(0, t - span) * 16000), int(min(len(x) / 16000, t + span) * 16000)
    n = 320
    best, bt = 1e9, t
    for k in range(a, max(a + 1, b - n), 80):
        e = float(np.mean(x[k:k + n] ** 2))
        if e < best:
            best, bt = e, (k + n / 2) / 16000
    return bt


def report():
    sys.path.insert(0, HERE)
    sys.path.insert(0, os.path.join(os.path.dirname(HERE), 'engine'))
    import bench as B
    import asr
    items = json.load(open(os.path.join(LID, 'items.json'), encoding='utf-8'))
    data = {}
    for f in ('whisper.json', 'classify.json'):
        p = os.path.join(LID, f)
        if os.path.exists(p):
            data.update(json.load(open(p, encoding='utf-8')))
    models = asr.Models(threads=THREADS, log=lambda s: None)
    rec_for = {'en': 'parakeet', 'es': 'parakeet', 'de': 'parakeet', 'fr': 'parakeet', 'it': 'parakeet', 'pt': 'parakeet',
               'ru': 'gigaam', 'zh': 'sensevoice', 'ja': 'sensevoice', 'ko': 'sensevoice'}

    def tokens(text):
        out = []
        for w in B.norm(text, 'en'):
            out += B.norm(w, 'zh') if re.search(r'[㐀-鿿]', w) else [w]
        return out

    def transcribe(lang, x):
        return asr.tidy(models.offline(rec_for.get(lang, 'parakeet'), x)[0])

    L = ['# Spoken language identification (%s)' % time.strftime('%Y-%m-%d %H:%M'), '',
         'Supported = %s (the languages the helper has recognisers for). 4 threads.' % ', '.join(SUPPORTED), '',
         '## 1. Which language', '',
         '| model | size | lines: right (all languages) | lines: right (supported only) | one-word: right (supported only) | time per 1 s of audio |',
         '|---|---|---|---|---|---|']
    mixed = [it for it in items if it['kind'] == 'mixed']
    for name, d in data.items():
        r = d['results']
        row = {}
        for kind in ('line', 'word'):
            its = [it for it in items if it['kind'] == kind]
            all_ok = sum(1 for it in its if max(r[it['id']]['probs'], key=r[it['id']]['probs'].get) == it['lang'])
            sup_ok = sum(1 for it in its if max(SUPPORTED, key=lambda l: r[it['id']]['probs'].get(l, 0.0)) == it['lang'])
            row[kind] = (all_ok, sup_ok, len(its))
        per_s = sum(t for _, t in d['times']) / sum(a for a, _ in d['times'])
        L.append('| %s | %s MB | %d / %d | %d / %d | %d / %d | %.0f ms |' % (name, d.get('mb', '?'), row['line'][0], row['line'][2],
                                                                          row['line'][1], row['line'][2], row['word'][1], row['word'][2], per_s * 1000))
    # wrong guesses on lines
    L += ['', 'Wrong guesses (lines, supported only):']
    for name, d in data.items():
        wrong = [(it['id'], max(SUPPORTED, key=lambda l: d['results'][it['id']]['probs'].get(l, 0.0))) for it in items if it['kind'] == 'line'
                 and max(SUPPORTED, key=lambda l: d['results'][it['id']]['probs'].get(l, 0.0)) != it['lang']]
        L.append('- %s: %s' % (name, ', '.join('%s->%s' % w for w in wrong) or 'none'))
    # 2. segments
    L += ['', '## 2. Where the language changes (%d mixed utterances)' % len(mixed), '',
          'Segments from windows of each length (every %.2f s); right = the same languages in the same order; boundary = '
          'how far each found change is from the true one; stitched = each segment cut at the quietest point near the '
          'change and transcribed by its language\'s model.' % HOP, '',
          '| model | window | right language order | boundary error, median | stitched: words wrong |', '|---|---|---|---|---|']
    best = {}
    for name, d in data.items():
        for w in WINDOWS:
            ok, berr, errs, n = 0, [], 0, 0
            outs = []
            for it in mixed:
                x = read_wav(it['path'])
                segs = segment(d['results'][it['id']]['windows'], it['dur'], w)
                true = [s['lang'] for s in it['segs']]
                got = [s[0] for s in segs]
                if got == true:
                    ok += 1
                    for a, b in zip(it['segs'][1:], segs[1:]):
                        berr.append(abs(a['t0'] - b[1]))
                cuts = [0.0] + [quiet_point(x, s[1]) for s in segs[1:]] + [len(x) / 16000]
                text = ' '.join(transcribe(s[0], x[int(cuts[i] * 16000):int(cuts[i + 1] * 16000)]) for i, s in enumerate(segs))
                e = B.edits(tokens(it['text']), tokens(text))
                errs += e
                n += len(tokens(it['text']))
                outs.append(text)
            L.append('| %s | %.1f s | %d / %d | %s | %.0f %% |' % (name, w, ok, len(mixed), '%.2f s' % np.median(berr) if berr else '-', 100.0 * errs / n))
            if name not in best or errs < best[name][0]:
                best[name] = (errs, w, outs)
    # references: true segments, one model for the whole line
    L += ['', '## Against: the true segments, and one model for the whole line', '', '| | words wrong |', '|---|---|']
    refs = {}
    for label in ('true segments (the best stitching can do)', 'Parakeet v3 whole line (25 European languages)',
                  'SenseVoice whole line (zh/en/ja/ko)', 'the first piece\'s language model, whole line'):
        errs, n, outs = 0, 0, []
        for it in mixed:
            x = read_wav(it['path'])
            if label.startswith('true'):
                cuts = [0.0] + [quiet_point(x, s['t0'] - 0.03, 0.08) for s in it['segs'][1:]] + [len(x) / 16000]
                text = ' '.join(transcribe(s['lang'], x[int(cuts[i] * 16000):int(cuts[i + 1] * 16000)]) for i, s in enumerate(it['segs']))
            elif label.startswith('Parakeet'):
                text = transcribe('en', x)
            elif label.startswith('SenseVoice'):
                text = transcribe('zh', x)
            else:
                text = transcribe(it['segs'][0]['lang'], x)
            errs += B.edits(tokens(it['text']), tokens(text))
            n += len(tokens(it['text']))
            outs.append(text)
        refs[label] = outs
        L.append('| %s | %.0f %% |' % (label, 100.0 * errs / n))
    L += ['', '## The mixed lines', '']
    for k, it in enumerate(mixed):
        L.append('- said: %s' % ' | '.join('%s: %s' % (s['lang'], s['text']) for s in it['segs']))
        for name, (_, w, outs) in best.items():
            L.append('  - %s (%.1f s windows): %s' % (name, w, outs[k]))
        L.append('  - true segments: %s' % refs['true segments (the best stitching can do)'][k])
        L.append('  - Parakeet whole: %s' % refs['Parakeet v3 whole line (25 European languages)'][k])
    path = os.path.join(OUT, 'lid.md')
    open(path, 'w', encoding='utf-8').write('\n'.join(L) + '\n')
    print('\n'.join(L))


if __name__ == '__main__':
    {'prep': prep, 'whisper': whisper, 'classify': classify, 'report': report}[sys.argv[1]]()
