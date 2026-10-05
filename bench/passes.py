"""The open questions before building the two-pass design:
  1. Parakeet TDT 0.6B v3 (NVIDIA, int8, offline) as the second pass for English / Spanish / German / Russian:
     better than Nemotron's own text?
  2. One-word lines ("Okay.", "Да.", "好。"...): do the second-pass models (GigaAM v3, SenseVoice, Parakeet v3)
     get them? (Nemotron alone dropped most; Whisper small got most.)

    C:/Users/user/miniconda3/envs/pcbench/python.exe bench/passes.py

Report: export/asrbench/passes.md
"""
import json
import os
import statistics
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import bench as B           # noqa: E402
import second as S          # noqa: E402
import shortwords as W      # noqa: E402


def parakeet():
    import sherpa_onnx as so
    d = S.get('csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8')
    return so.OfflineRecognizer.from_transducer(encoder=os.path.join(d, 'encoder.int8.onnx'), decoder=os.path.join(d, 'decoder.int8.onnx'),
                                               joiner=os.path.join(d, 'joiner.int8.onnx'), tokens=os.path.join(d, 'tokens.txt'),
                                               num_threads=B.THREADS, model_type='nemo_transducer')


def main():
    spin = B.spinner()
    L = ['# Second-pass questions (%s)' % time.strftime('%Y-%m-%d %H:%M'), '']
    try:
        # 1. Parakeet v3 on the sentences
        clips = [c for c in json.load(open(os.path.join(B.OUT, 'clips.json'), encoding='utf-8')) if c['text']]
        pk = parakeet()
        base = S.baseline()
        L += ['## 1. Parakeet v3 as the second pass (error %, clean / room)', '',
              '| | en | ru | es | de | time per line |', '|---|---|---|---|---|---|']
        res = {}
        times = []
        for cond in ('clean', 'room'):
            for c in [c for c in clips if c['lang'] in ('en', 'ru', 'es', 'de')]:
                x = B.read(os.path.join(B.OUT, 'clips', '%s_%s.wav' % (c['id'], cond)))
                text, took = S.run_offline(pk, x)
                times.append(took)
                k = (c['lang'], cond)
                e, n = res.get(k, (0, 0))
                res[k] = (e + B.edits(B.norm(c['text'], c['lang']), B.norm(text, c['lang'])), n + len(B.norm(c['text'], c['lang'])))
        for name in ('nemotron-560-int8', 'whisper-small', 'whisper-medium'):
            L.append('| *%s* | %s | |' % (name, ' | '.join('%.1f / %.1f' % (base.get((name, l, 'clean'), float('nan')), base.get((name, l, 'room'), float('nan')))
                                                         for l in ('en', 'ru', 'es', 'de'))))
        L.append('| **Parakeet v3 int8** | %s | %.2f s median |' % (' | '.join('%.1f / %.1f' % tuple(100.0 * res[(l, c)][0] / res[(l, c)][1] for c in ('clean', 'room'))
                                                                          for l in ('en', 'ru', 'es', 'de')), statistics.median(times)))
        print('parakeet sentences done', flush=True)

        # 2. one-word lines
        words = W.make()
        import sherpa_onnx as so
        d = S.get('csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09')
        sv = {lang: so.OfflineRecognizer.from_sense_voice(model=os.path.join(d, 'model.int8.onnx'), tokens=os.path.join(d, 'tokens.txt'),
                                                          num_threads=B.THREADS, language=lang, use_itn=True) for lang in ('zh', 'en')}
        giga, _ = [f for n, l, k, f in S.specialists() if n == 'GigaAM v3 CTC'][0]()
        models = [('Parakeet v3', ('en', 'ru'), lambda x, lang: S.run_offline(pk, x)[0]),
                  ('GigaAM v3 CTC', ('ru',), lambda x, lang: S.run_offline(giga, x)[0]),
                  ('SenseVoice', ('zh', 'en'), lambda x, lang: S.run_offline(sv[lang], x)[0])]
        L += ['', '## 2. One-word lines (hits; clean / room)', '',
              '| model | en (50) | ru (10) | zh (10) | nothing at all |', '|---|---|---|---|---|',
              '| *Nemotron (from shortwords.md)* | 23 / 8 | 4 / 2 | 3 / 2 | 89 of 140 |',
              '| *Whisper small (from shortwords.md)* | 43 / 41 | 6 / 7 | 6 / 5 | 0 of 140 |']
        misses = {}
        for name, langs, fn in models:
            cells, empty, total = [], 0, 0
            for lang in ('en', 'ru', 'zh'):
                if lang not in langs:
                    cells.append('-')
                    continue
                hc = []
                for cond in ('clean', 'room'):
                    h = 0
                    for c in [c for c in words if c['lang'] == lang]:
                        t = fn(c[cond], lang)
                        total += 1
                        empty += not t.strip()
                        if W.hit(c['text'], t, lang):
                            h += 1
                        else:
                            misses.setdefault(name, []).append('%s -> %s' % (c['text'], t or '-'))
                    hc.append(h)
                cells.append('%d / %d' % tuple(hc))
            L.append('| **%s** | %s | %d of %d |' % (name, ' | '.join(cells), empty, total))
        L += ['', '## Misses']
        for name, m in misses.items():
            L += ['', '**%s**: %s' % (name, ', '.join(sorted(set(m)))[:600])]
    finally:
        spin.kill()
    path = os.path.join(B.OUT, 'passes.md')
    open(path, 'w', encoding='utf-8').write('\n'.join(L) + '\n')
    print('\n'.join(L))


if __name__ == '__main__':
    sys.exit(main())
