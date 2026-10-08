"""The machine-translation benchmark's scores: chrF (sacrebleu; character n-grams - fair across scripts, Chinese and
Japanese included) against NTREX's human translations, and the time per sentence (mean, 90th percentile) and per
source word, for each system, direction and route. Writes RESULTS.md and results.json.
    python score.py              (env mtbench; after the runs)"""
import json
import os
import statistics

import sacrebleu

HERE = os.path.dirname(os.path.abspath(__file__))
SYSTEMS = ['mozilla', 'opus']
NAMES = dict(mozilla='Mozilla', opus='Opus-MT')


def read(lang):
    return open(os.path.join(HERE, 'data', f'{lang}.txt'), encoding='utf-8').read().rstrip('\n').split('\n')


def words(lines):
    return sum(len(l.split()) for l in lines)


def main():
    jobs = json.load(open(os.path.join(HERE, 'jobs.json')))
    rows = []
    for j in jobs:
        row = dict(src=j['src'], tgt=j['tgt'], kind=j['kind'])
        refs = read(j['tgt'])
        src_words = max(1, words(read(j['src'])))
        for s in SYSTEMS:
            p = os.path.join(HERE, 'results', s, f"{j['src']}-{j['tgt']}.json")
            if not os.path.exists(p):
                continue
            r = json.load(open(p, encoding='utf-8'))
            t = sorted(r['times'])
            row[s] = dict(chrf=sacrebleu.corpus_chrf(r['hyps'], [refs]).score, hops=r['hops'], route=r['route'],
                          mean_ms=statistics.mean(t), p90_ms=t[int(0.9 * (len(t) - 1))],
                          ms_per_word=sum(t) / src_words, mb=r['modelBytes'] / 1e6)
        rows.append(row)
    json.dump(rows, open(os.path.join(HERE, 'results.json'), 'w'), indent=1)

    def avg(rs, s, k):
        v = [r[s][k] for r in rs if s in r]
        return statistics.mean(v) if v else float('nan')

    out = ['# Machine translation benchmark: Mozilla (Firefox Translations) vs Opus-MT', '',
           'NTREX-128, the first 100 sentences of each language (news, ~25 words a sentence); each sentence '
           'translated on its own; chrF against the human translation (higher is better). Mozilla: the Firefox '
           'engine (WebAssembly, one thread) with its newest models; Opus-MT: CTranslate2 8-bit, one thread, greedy, '
           "on each pair's shortest route (a direct model when there is one). Two hops: through English.", '']
    for kind in ('with English', 'between two others'):
        rs = [r for r in rows if r['kind'] == kind]
        out += [f'## {kind}', '',
                '| pair | Mozilla chrF | route | ms/sentence | Opus-MT chrF | route | ms/sentence |',
                '|---|---|---|---|---|---|---|']
        for r in rs:
            cells = [f"{r['src']}→{r['tgt']}"]
            for s in SYSTEMS:
                if s in r:
                    x = r[s]
                    cells += [f"{x['chrf']:.1f}", f"{x['hops']} hop{'s' if x['hops'] > 1 else ''}",
                              f"{x['mean_ms']:.0f} (p90 {x['p90_ms']:.0f})"]
                else:
                    cells += ['-', '-', '-']
            out.append('| ' + ' | '.join(cells) + ' |')
        both = [r for r in rs if all(s in r for s in SYSTEMS)]
        out += ['', f'Average over the {len(both)} pairs both have: '
                + ', '.join(f"{NAMES[s]} chrF {avg(both, s, 'chrf'):.1f}, {avg(both, s, 'mean_ms'):.0f} ms a sentence "
                            f"({avg(both, s, 'ms_per_word'):.1f} ms a word)" for s in SYSTEMS), '']
    open(os.path.join(HERE, 'RESULTS.md'), 'w', encoding='utf-8').write('\n'.join(out) + '\n')
    print('\n'.join(out))


if __name__ == '__main__':
    main()
