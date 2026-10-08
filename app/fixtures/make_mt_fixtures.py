"""Fixtures for kd-translate's engine (env mtbench: numpy, sentencepiece): what Python's own SentencePiece and the numpy
reference (engine/mt.py) answer, for the Rust tests to compare against. Needs Mozilla's model folders in bench/mt/models
(the benchmark's run_mozilla.mjs downloads them). The test sentences are NTREX-128's (Microsoft, CC-BY-SA-4.0; bench/mt/prep.py).
    python app/fixtures/make_mt_fixtures.py        -> app/fixtures/mt.json"""
import json
import os
import sys

import sentencepiece as spm

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, os.path.join(ROOT, 'engine'))
import mt  # noqa: E402

MODELS = os.path.join(ROOT, 'bench', 'mt', 'models')
DATA = os.path.join(ROOT, 'bench', 'mt', 'data')
LANGS = 'en es fr de it pt nl pl uk ru zh yue ja ko cs sk ro hr bg fi sv hu da et lv lt sl el mt'.split()
EDGE = ['', ' ', 'a', '  leading and   inner   spaces  ', 'emoji 😀 and ✓ marks', 'Ｆｕｌｌｗｉｄｔｈ ＡＢＣ １２３',
        'tab\tand\nnewline', 'ﬁ ligature, café, naïve, Ångström', '数字123とABC', 'unknown 𓂀 hieroglyph',
        'ok', 'lol!!!', '???', 'Ça va? ¿Qué tal? 你好吗？', '​zero​width']


def lines(lang, n):
    return open(os.path.join(DATA, f'{lang}.txt'), encoding='utf-8').read().rstrip('\n').split('\n')[:n]


def main():
    out = dict(spm=[], translate=[])
    vocabs = ['es-en/vocab.esen.spm', 'ja-en/vocab.jaen.spm', 'en-ja/srcvocab.enja.spm', 'en-ja/trgvocab.enja.spm',
              'zh-Hans-en/vocab.zhen.spm', 'en-ko/srcvocab.enko.spm']
    texts = EDGE + [l for lang in LANGS for l in lines(lang, 5)]
    for v in vocabs:
        path = os.path.join(MODELS, v)
        if not os.path.exists(path):
            print('missing', v)
            continue
        sp = spm.SentencePieceProcessor(model_file=path)
        enc = [sp.encode(t) for t in texts]
        out['spm'].append(dict(vocab=v, texts=texts, ids=enc, decoded=[sp.decode(e) for e in enc]))
    mt.INT8 = True
    for pair, lang in [('es-en', 'es'), ('en-ja', 'en'), ('ja-en', 'ja'), ('en-de', 'en')]:
        m = mt.Model(os.path.join(MODELS, pair))
        cases = []
        for t in lines(lang, 6) + (['Can anyone hear me?', 'The basement door is locked.'] if lang == 'en' else []):
            ids = m.src_sp.encode(t) + [mt.EOS]
            cases.append(dict(text=t, src_ids=ids, shortlist=[int(x) for x in m.shortlist(ids)],
                              out_ids=m.translate_ids(ids), out=m.translate(t)))
        out['translate'].append(dict(pair=pair, cases=cases))
        print(pair, 'done')
    json.dump(out, open(os.path.join(ROOT, 'app', 'fixtures', 'mt.json'), 'w', encoding='utf-8'), ensure_ascii=False)
    print('wrote app/fixtures/mt.json')


if __name__ == '__main__':
    main()
