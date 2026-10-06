"""Which languages may "Auto (guess)" choose between? More candidates cover more players but give the language
detector (AmberNet) more ways to be wrong. Words wrong through asr.transcribe_mixed on the benchmark's
single-language lines (en, ru, zh, es, de) and the 20 mixed lines, for growing candidate sets.

    <conda>/envs/pcvoice/python.exe bench/autolangs.py

Report: export/asrbench/autolangs.md
"""
import json
import os
import re
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(os.path.dirname(HERE), 'engine'))
import bench as B           # noqa: E402
import asr                  # noqa: E402

SETS = [
    ('the 10 so far', ['en', 'ru', 'zh', 'es', 'de', 'fr', 'it', 'pt', 'ja', 'ko']),
    ('supported (13; Cantonese is found as zh)', ['en', 'it', 'es', 'pt', 'de', 'fr', 'uk', 'pl', 'nl', 'ru', 'zh', 'ko', 'ja']),
    ('supported + beta (21)', ['en', 'it', 'es', 'pt', 'de', 'fr', 'uk', 'pl', 'nl', 'ru', 'zh', 'ko', 'ja',
                               'sk', 'cs', 'ro', 'hr', 'bg', 'fi', 'sv', 'hu']),
    ('all 28', ['en', 'it', 'es', 'pt', 'de', 'fr', 'uk', 'pl', 'nl', 'ru', 'zh', 'ko', 'ja',
                'sk', 'cs', 'ro', 'hr', 'bg', 'fi', 'sv', 'hu', 'et', 'da', 'lt', 'mt', 'el', 'lv', 'sl']),
]


def toks(t):
    out = []
    for w in B.norm(t, 'en'):
        out += B.norm(w, 'zh') if re.search(r'[\u3400-\u9fff]', w) else [w]
    return out


def main():
    items = [i for i in json.load(open(os.path.join(B.OUT, 'lid', 'items.json'), encoding='utf-8')) if i['kind'] in ('line', 'mixed')]
    models = asr.Models(threads=4, log=lambda s: None)
    rows = []
    for label, langs in SETS:
        asr.MIXED_LANGS = langs
        models.loaded.pop('langid', None)
        acc = {}
        wrong_lang = 0
        for it in items:
            x = B.read(it['path'])
            text, seg_langs, _ = asr.transcribe_mixed(models, x)
            key = 'mixed' if it['kind'] == 'mixed' else it['lang']
            r = acc.setdefault(key, [0, 0])
            r[0] += B.edits(toks(it['text']), toks(text))
            r[1] += len(toks(it['text']))
            if it['kind'] == 'line' and any(l != it['lang'] for l in seg_langs):
                wrong_lang += 1
        rows.append((label, acc, wrong_lang))
        print(label, {k: '%.1f' % (100.0 * v[0] / v[1]) for k, v in acc.items()}, wrong_lang, flush=True)
    keys = ['en', 'ru', 'zh', 'es', 'de', 'mixed']
    n_lines = sum(1 for i in items if i['kind'] == 'line')
    L = ['# Auto language: how many candidates (%s)' % time.strftime('%Y-%m-%d %H:%M'), '',
         'Words wrong (%%; Chinese by characters) through the helper\'s stitching (transcribe_mixed), the clean and the noisy-room clip of each line. '
         '"Lines with a wrong stretch": of the %d single-language lines, how many got any stretch in another language.' % n_lines, '',
         '| candidates | ' + ' | '.join(keys) + ' | lines with a wrong stretch |', '|---|' + '---|' * (len(keys) + 1)]
    for label, acc, wl in rows:
        L.append('| %s | %s | %d |' % (label, ' | '.join('%.1f' % (100.0 * acc[k][0] / acc[k][1]) for k in keys), wl))
    open(os.path.join(B.OUT, 'autolangs.md'), 'w', encoding='utf-8').write('\n'.join(L) + '\n')
    print('\n'.join(L))


if __name__ == '__main__':
    sys.exit(main())
