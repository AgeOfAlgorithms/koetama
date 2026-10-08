"""Scores Koetama's Rust engine (results_rust.json, from `cargo run --release -p kd-translate --example mt_bench`)
next to Mozilla's own build (results.json, run_mozilla.mjs): chrF on the same sentences, time per sentence, memory.
    python score_rust.py        (env mtbench)  -> prints the table, writes RUST.md"""
import json
import os

import sacrebleu

HERE = os.path.dirname(os.path.abspath(__file__))


def refs(lang):
    return open(os.path.join(HERE, 'data', f'{lang}.txt'), encoding='utf-8').read().rstrip('\n').split('\n')


def main():
    rust = json.load(open(os.path.join(HERE, 'results_rust.json'), encoding='utf-8'))
    moz = {(r['src'], r['tgt']): r.get('mozilla') for r in json.load(open(os.path.join(HERE, 'results.json'), encoding='utf-8'))}
    rows = []
    for r in rust:
        ref = refs(r['tgt'])[:len(r['hyps'])]
        chrf = sacrebleu.corpus_chrf(r['hyps'], [ref]).score
        m = moz.get((r['src'], r['tgt']))
        rows.append(dict(pair=f"{r['src']}-{r['tgt']}", hops=len(r['route']), chrf=chrf, moz_chrf=m and m['chrf'],
                         ms=r['mean_ms'], p90=r['p90_ms'], moz_ms=m and m['mean_ms'], load=r['load_ms'],
                         mb=r['model_mb'], ws=r['ws_load_mb']))
    lines = ['| pair | hops | chrF Rust | chrF Mozilla (WASM) | ms Rust (p90) | ms Mozilla | load ms | MB |',
             '|---|---|---|---|---|---|---|---|']
    for x in rows:
        mc = f"{x['moz_chrf']:.1f}" if x['moz_chrf'] is not None else '-'
        mm = f"{x['moz_ms']:.0f}" if x['moz_ms'] is not None else '-'
        lines.append(f"| {x['pair']} | {x['hops']} | {x['chrf']:.1f} | {mc} | {x['ms']:.0f} ({x['p90']:.0f}) | {mm} | "
                     f"{x['load']:.0f} | {x['ws']:.0f} |")
    summary = []
    for hops in (1, 2):
        rs = [x for x in rows if x['hops'] == hops and x['moz_chrf'] is not None]
        if not rs:
            continue
        avg = lambda k: sum(x[k] for x in rs) / len(rs)  # noqa: E731
        summary.append(f"{hops} hop{'s' if hops > 1 else ''} ({len(rs)} pairs): Rust chrF {avg('chrf'):.1f} vs Mozilla "
                       f"{avg('moz_chrf'):.1f}; {avg('ms'):.0f} ms a sentence (p90 {avg('p90'):.0f}) vs {avg('moz_ms'):.0f}; "
                       f"load {avg('load'):.0f} ms; {avg('ws'):.0f} MB")
    text = '\n'.join(['# Koetama\'s Rust translation engine vs Mozilla\'s build', '',
                      'NTREX-128, the first 100 sentences; one sentence at a time, one thread (Ryzen 7 3700X). '
                      'MB = the working set the models add.', ''] + [f'- {s}' for s in summary] + [''] + lines) + '\n'
    open(os.path.join(HERE, 'RUST.md'), 'w', encoding='utf-8').write(text)
    print('\n'.join(summary))
    worst = sorted(rows, key=lambda x: (x['chrf'] - (x['moz_chrf'] or x['chrf'])))[:6]
    print('largest drops:', [(x['pair'], round(x['chrf'], 1), round(x['moz_chrf'] or 0, 1)) for x in worst])
    slow = sorted(rows, key=lambda x: -x['ms'])[:6]
    print('slowest:', [(x['pair'], round(x['ms']), round(x['moz_ms'] or 0)) for x in slow])


if __name__ == '__main__':
    main()
