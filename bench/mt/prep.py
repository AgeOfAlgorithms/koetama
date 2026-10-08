"""The machine-translation benchmark's test set and jobs (bench/mt): NTREX-128 (CC-BY-SA-4.0; the same ~2000 news
sentences professionally translated into 128 languages, so any pair can be scored) - its first N sentences in each of
Koetama's languages - and the directions to run: every language to and from English (one path) and a sample of
non-English pairs (two paths for Mozilla, which only has models with English; Opus-MT's shortest route: a direct
model when there is one). Writes data/<lang>.txt and jobs.json.
    python prep.py            (env mtbench)"""
import json
import os
import re
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
N = 100
NTREX = 'https://raw.githubusercontent.com/MicrosoftTranslator/NTREX/main/NTREX-128/newstest2019-{}.txt'
# Koetama's language -> NTREX's file
FILE = dict(en='src.eng', es='ref.spa', fr='ref.fra', de='ref.deu', it='ref.ita', pt='ref.por', nl='ref.nld',
            pl='ref.pol', uk='ref.ukr', ru='ref.rus', zh='ref.zho-CN', yue='ref.yue', ja='ref.jpn', ko='ref.kor',
            cs='ref.ces', sk='ref.slk', ro='ref.ron', hr='ref.hrv', bg='ref.bul', fi='ref.fin', sv='ref.swe',
            hu='ref.hun', da='ref.dan', et='ref.est', lv='ref.lav', lt='ref.lit', sl='ref.slv', el='ref.ell',
            mt='ref.mlt')
# Mozilla's language codes (Cantonese: no model of its own - written Cantonese through Traditional Chinese, from it only)
MOZ = {k: k for k in FILE}
MOZ.update(zh='zh-Hans', yue='zh-Hant')
# three-letter codes for Opus-MT's multilingual models' target token (>>pol<<)
ISO3 = dict(pl='pol', hr='hrv', sl='slv', ja='jpn', pt='por', ro='ron', cs='ces', sk='slk', bg='bul', uk='ukr',
            ru='rus')
PIVOT_PAIRS = ['de-fr', 'fr-de', 'es-it', 'it-es', 'ru-uk', 'uk-ru', 'pl-cs', 'cs-pl', 'ja-ko', 'ko-ja', 'zh-ja',
               'ja-zh', 'de-ja', 'ja-de', 'ru-es', 'es-ru']


def opus_route(names, a, b):
    """Opus-MT's shortest route a -> b: [(model, target token or None)], or None. A bilingual model (tc-big first),
    else a family model (its target token when it has several targets), else through English."""
    # (models with several target languages or variants: the sentence-initial tag their vocabulary lists)
    tags = {'opus-mt-tc-big-en-pt': 'por', 'opus-mt-en-zh': 'cmn_Hans', 'opus-mt-tc-big-en-ro': 'ron',
            'opus-mt-tc-big-en-et': 'est', 'opus-mt-tc-big-en-lv': 'lav'}

    def one(x, y):
        for pre in ('opus-mt-tc-big-', 'opus-mt-'):
            if pre + f'{x}-{y}' in names:
                return pre + f'{x}-{y}', tags.get(pre + f'{x}-{y}')
        fam_src = dict(pt='ROMANCE', ro='ROMANCE', hr='zls', sl='zls')
        fam_tgt = dict(pl='zlw', hr='zls', sl='zls', ja='jap')
        if y == 'en' and x in fam_src:
            for m in (f'opus-mt-tc-big-{fam_src[x]}-en', f'opus-mt-{fam_src[x]}-en'):
                if m in names:
                    return m, None
        if x == 'en' and y in fam_tgt:
            m = f'opus-mt-en-{fam_tgt[y]}'
            if m in names:
                return m, (None if fam_tgt[y] == 'jap' else ISO3[y])
        return None
    direct = one(a, b)
    if direct:
        return [direct]
    if 'en' not in (a, b):
        r1, r2 = one(a, 'en'), one('en', b)
        if r1 and r2:
            return [r1, r2]
    return None


def main():
    os.makedirs(os.path.join(HERE, 'data'), exist_ok=True)
    for lang, f in FILE.items():
        path = os.path.join(HERE, 'data', f'{lang}.txt')
        if not os.path.exists(path):
            text = urllib.request.urlopen(NTREX.format(f)).read().decode('utf-8-sig')
            lines = [l.strip() for l in text.splitlines()][:N]
            assert len(lines) == N and all(lines), lang
            open(path, 'w', encoding='utf-8', newline='\n').write('\n'.join(lines) + '\n')
    names = set(json.load(open(os.path.join(HERE, 'opus_models.json'))))
    moz = json.load(open(os.path.join(HERE, 'tm.json'), encoding='utf-8'))['data']
    moz_pairs = {(r['fromLang'], r['toLang']) for r in moz}
    jobs = []
    pairs = [(x, 'en') for x in FILE if x != 'en'] + [('en', x) for x in FILE if x not in ('en', 'yue')]
    pairs += [tuple(p.split('-')) for p in PIVOT_PAIRS]
    for a, b in pairs:
        ma, mb = MOZ[a], MOZ[b]
        if 'en' in (a, b):
            moz_route = [[ma, mb]] if (ma, mb) in moz_pairs else None
        else:
            moz_route = [[ma, 'en'], ['en', mb]] if (ma, 'en') in moz_pairs and ('en', mb) in moz_pairs else None
        jobs.append(dict(src=a, tgt=b, kind='with English' if 'en' in (a, b) else 'between two others',
                         mozilla=moz_route, opus=opus_route(names, a, b)))
    json.dump(jobs, open(os.path.join(HERE, 'jobs.json'), 'w'), indent=1)
    for j in jobs:
        o = ' + '.join(m + (f' >>{t}<<' if t else '') for m, t in j['opus']) if j['opus'] else '-'
        m = ' + '.join('-'.join(r) for r in j['mozilla']) if j['mozilla'] else '-'
        print(f"{j['src']:>3}->{j['tgt']:<3}  mozilla: {m:<22} opus: {o}")


if __name__ == '__main__':
    main()
