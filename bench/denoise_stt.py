"""Does noise suppression help Koetama's speech to text? (bench/denoise.py found the speech too clean and the noise too
quiet to tell: 0-1 % word errors with no suppression at all.) Harder here: every benchmark line of a language - the
one-word callouts too - with real keyboard typing, a vacuum cleaner and a synthetic fan at the speech's own level and
louder (5, 0 and -5 dB), each through no suppression, RNNoise and DeepFilterNet 3 (as bench/denoise runs them), then
Koetama's own speech to text (`koetama --cli --transcribe`, the language given). Word error rate (character error
rate for Chinese).

    python bench/denoise_stt.py              (en, ru, zh; ~20 min; heavy - not while someone plays)
    python bench/denoise_stt.py en           (one language)
    python bench/denoise_stt.py so762 golos  (real people: bench/datasets.py's speechocean762 - English from learners,
                                              the worst-pronounced - and Golos far-field Russian)

Needs what bench/denoise.py needs. Writes export/noise/stt-results.md and .json.
"""
import json, os, re, sys, time
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import denoise as D

LANGS = {'en': 'word', 'ru': 'word', 'zh': 'char'}
# (real speech, bench/datasets.py: name -> its language and manifest)
SETS = {'so762': ('en', 'export/datasets/speechocean762/subset/manifest.json'),
        'golos': ('ru', 'export/datasets/golos_farfield/subset/manifest.json')}
NOISES = ['keyboard', 'vacuum', 'fan']
SNRS = [5, 0, -5]
MODELS = ['raw', 'rnnoise', 'dfn3']
OUT = os.path.join(D.ROOT, 'export', 'noise', 'stt')


def track(lang):
    clips = [c for c in json.load(open(os.path.join(D.ROOT, 'export', 'asrbench', 'clips.json'), encoding='utf-8'))
             if c['lang'] == lang]
    parts, texts = [np.zeros(int(1.0 * D.RATE))], []
    for c in clips:
        x, sr = D.read(os.path.join(D.CLIPS, f'{c["id"]}_clean.wav'))
        parts += [D.to48(x, sr), np.zeros(int(1.0 * D.RATE))]
        texts.append(c['text'])
    s = np.concatenate(parts)
    return s * np.sqrt(0.01 / D.active_power(s)), texts


def track_set(name):
    man = json.load(open(os.path.join(D.ROOT, SETS[name][1]), encoding='utf-8'))
    parts, texts = [np.zeros(int(1.0 * D.RATE))], []
    for m in man:
        x, sr = D.read(m['file'])
        parts += [D.to48(x, sr), np.zeros(int(1.0 * D.RATE))]
        texts.append(m['text'])
    s = np.concatenate(parts)
    return s * np.sqrt(0.01 / D.active_power(s)), texts


def units(t, how):
    t = t.lower()
    if how == 'char':
        return [c for c in t if c.strip() and c.isalnum()]
    return re.findall(r"[\w']+", t)


def error_rate(refs, hyp, how):
    r, h = sum((units(t, how) for t in refs), []), units(hyp, how)
    d = list(range(len(h) + 1))
    for i in range(1, len(r) + 1):
        prev, d[0] = d[0], i
        for j in range(1, len(h) + 1):
            cur = min(d[j] + 1, d[j - 1] + 1, prev + (r[i - 1] != h[j - 1]))
            prev, d[j] = d[j], cur
    return d[len(h)] / max(1, len(r))


def transcribe(path, lang):
    import subprocess
    out = subprocess.run([D.KOETAMA, '--cli', '--transcribe', path, '--lang', lang], env=D.env(), capture_output=True,
                         text=True, encoding='utf-8', errors='replace').stdout
    return ' '.join(m.group(1) for m in re.finditer(r'^LINE \d+: (.*?)   \[', out, re.M) if m.group(1) != '(nothing)')


def main():
    langs = [a for a in sys.argv[1:] if a in LANGS or a in SETS] or list(LANGS)
    os.makedirs(OUT, exist_ok=True)
    rng = np.random.default_rng(11)
    res = {}
    for lang in langs:
        name = lang
        speech, refs = track_set(name) if name in SETS else track(name)
        lang = SETS[name][0] if name in SETS else name
        n = len(speech)
        how = LANGS[lang]
        res[name] = {'lines': len(refs), 'unit': how, 'clean': {}}
        pc = os.path.join(OUT, f'{name}-clean.wav')
        D.write(pc, speech)
        res[name]['clean'] = error_rate(refs, transcribe(pc, lang), how)
        print(f'{name}: {len(refs)} lines, clean speech {res[name]["clean"]:.1%}', flush=True)
        for kind in NOISES:
            base = D.noise_track(kind, n, rng)
            for snr in SNRS:
                key = f'{kind} {snr:+d} dB'
                noisy = speech + base * np.sqrt(D.active_power(speech) / 10 ** (snr / 10))
                pm = os.path.join(OUT, f'{name}-{kind}{snr:+d}.wav')
                D.write(pm, noisy)
                row = {}
                for m in MODELS:
                    if m == 'raw':
                        p = pm
                    else:
                        o = os.path.join(OUT, f'{name}-{kind}{snr:+d}-{m}.f32.wav')
                        D.run_model(m, pm, o)
                        p = o.replace('.f32.wav', '.wav')
                        D.write(p, D.read_f32(o))
                    row[m] = error_rate(refs, transcribe(p, lang), how)
                res[name][key] = row
                print(f'  {key:<16} ' + '  '.join(f'{m} {v:6.1%}' for m, v in row.items()), flush=True)
    res['when'] = time.strftime('%Y-%m-%d %H:%M')
    tag = '-'.join(langs)
    with open(os.path.join(D.ROOT, 'export', 'noise', f'stt-results-{tag}.json'), 'w') as f:
        json.dump(res, f, indent=1)
    L = [f'# Speech to text with noise suppression ({res["when"]})', '',
         'Every benchmark line of the language (computer voices), noise at the speech\'s level and louder; error rate '
         '(words; characters for Chinese) of Koetama\'s own speech to text, the language given.', '']
    for lang in langs:
        r = res[lang]
        L += [f'**{lang}** ({r["lines"]} lines; clean speech {r["clean"]:.1%})', '',
              '| noise | no suppression | RNNoise | DeepFilterNet 3 |', '|---|---|---|---|']
        for k, row in r.items():
            if isinstance(row, dict):
                best = min(row.values())
                L.append(f'| {k} | ' + ' | '.join(('**%.1f %%**' if v == best else '%.1f %%') % (100 * v) for v in row.values()) + ' |')
        L.append('')
    with open(os.path.join(D.ROOT, 'export', 'noise', f'stt-results-{tag}.md'), 'w', encoding='utf-8') as f:
        f.write('\n'.join(L))
    print('\n'.join(L))


if __name__ == '__main__':
    main()
