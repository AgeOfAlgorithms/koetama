"""Rounds 2 and 3 of the benchmark (run after bench.py and make_clips.py):

  2. language specialists, as a second pass over the finished line (or, if they stream, instead of Nemotron):
     Russian  GigaAM v3 (CTC and transducer, Sber, MIT), T-one (streaming, T-Bank, Apache-2.0)
     Chinese  SenseVoice Small (int8), streaming Paraformer zh-en (int8), streaming Zipformer multi-zh-hans (int8)
  3. noise suppression before Nemotron 3.5 (560 ms, int8): GTCRN (0.5 MB) and DPDFNet (2 and baseline)

    <conda>/envs/pcbench/python.exe bench/second.py

Same clips, 4 threads, one other core kept busy. Report: export/asrbench/second.md
"""
import json
import os
import statistics
import subprocess
import sys
import time
import urllib.request

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import bench as B           # noqa: E402

DENOISE_URL = 'https://github.com/k2-fsa/sherpa-onnx/releases/download/speech-enhancement-models/%s'
DENOISERS = {'gtcrn': 'gtcrn_simple.onnx', 'dpdfnet2': 'dpdfnet2.onnx', 'dpdfnet-baseline': 'dpdfnet_baseline.onnx'}


def get(repo):
    from huggingface_hub import snapshot_download
    return snapshot_download(repo)


def size_mb(d, names):
    return sum(os.path.getsize(os.path.join(d, n)) for n in names if os.path.exists(os.path.join(d, n))) / 1e6


# ---------------------------------------------------------------- the specialists
def specialists():
    """(name, language, kind, loader) - kind "offline" (a second pass) or "online" (streams)"""
    import sherpa_onnx as so
    T = B.THREADS

    def gigaam_ctc():
        d = get('csukuangfj/sherpa-onnx-nemo-ctc-giga-am-v3-russian-2025-12-16')
        return so.OfflineRecognizer.from_nemo_ctc(model=os.path.join(d, 'model.int8.onnx'), tokens=os.path.join(d, 'tokens.txt'),
                                                 num_threads=T), size_mb(d, ['model.int8.onnx'])

    def gigaam_rnnt():
        d = get('csukuangfj/sherpa-onnx-nemo-transducer-giga-am-v3-russian-2025-12-16')
        return so.OfflineRecognizer.from_transducer(encoder=os.path.join(d, 'encoder.int8.onnx'), decoder=os.path.join(d, 'decoder.onnx'),
                                                   joiner=os.path.join(d, 'joiner.onnx'), tokens=os.path.join(d, 'tokens.txt'),
                                                   num_threads=T, model_type='nemo_transducer'), \
            size_mb(d, ['encoder.int8.onnx', 'decoder.onnx', 'joiner.onnx'])

    def tone():
        d = get('csukuangfj/sherpa-onnx-streaming-t-one-russian-2025-09-08')
        return so.OnlineRecognizer.from_t_one_ctc(model=os.path.join(d, 'model.onnx'), tokens=os.path.join(d, 'tokens.txt'),
                                                  num_threads=T), size_mb(d, ['model.onnx'])

    def sensevoice():
        d = get('csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09')
        return so.OfflineRecognizer.from_sense_voice(model=os.path.join(d, 'model.int8.onnx'), tokens=os.path.join(d, 'tokens.txt'),
                                                    num_threads=T, language='zh', use_itn=True), size_mb(d, ['model.int8.onnx'])

    def paraformer():
        d = get('csukuangfj/sherpa-onnx-streaming-paraformer-bilingual-zh-en')
        return so.OnlineRecognizer.from_paraformer(tokens=os.path.join(d, 'tokens.txt'), encoder=os.path.join(d, 'encoder.int8.onnx'),
                                                   decoder=os.path.join(d, 'decoder.int8.onnx'), num_threads=T), \
            size_mb(d, ['encoder.int8.onnx', 'decoder.int8.onnx'])

    def zipformer():
        d = get('csukuangfj/sherpa-onnx-streaming-zipformer-multi-zh-hans-int8-2023-12-13')
        stem = 'epoch-20-avg-1-chunk-16-left-128'
        return so.OnlineRecognizer.from_transducer(tokens=os.path.join(d, 'tokens.txt'),
                                                   encoder=os.path.join(d, 'encoder-%s.int8.onnx' % stem),
                                                   decoder=os.path.join(d, 'decoder-%s.onnx' % stem),
                                                   joiner=os.path.join(d, 'joiner-%s.int8.onnx' % stem), num_threads=T), \
            size_mb(d, ['encoder-%s.int8.onnx' % stem, 'decoder-%s.onnx' % stem, 'joiner-%s.int8.onnx' % stem])
    return [('GigaAM v3 CTC', 'ru', 'offline', gigaam_ctc), ('GigaAM v3 transducer', 'ru', 'offline', gigaam_rnnt),
            ('T-one (streams)', 'ru', 'online', tone),
            ('SenseVoice Small int8', 'zh', 'offline', sensevoice), ('Paraformer zh-en (streams)', 'zh', 'online', paraformer),
            ('Zipformer multi-zh-hans (streams)', 'zh', 'online', zipformer)]


def run_offline(r, x):
    s = r.create_stream()
    t0 = time.perf_counter()
    s.accept_waveform(16000, x)
    r.decode_stream(s)
    return s.result.text.strip(), time.perf_counter() - t0


def run_online(r, x, speech_end):
    """fed 0.1 s at a time; returns (text, compute, delay: from the end of the speech until the text was complete)"""
    s = r.create_stream()
    n = int(16000 * B.STEP)
    busy, compute, timeline = 0.0, 0.0, []
    for k in range(0, len(x), n):
        s.accept_waveform(16000, x[k:k + n])
        if not r.is_ready(s):
            continue
        t0 = time.perf_counter()
        while r.is_ready(s):
            r.decode_stream(s)
        dt = time.perf_counter() - t0
        compute += dt
        busy = max((k + n) / 16000.0, busy) + dt
        timeline.append((busy, r.get_result(s)))
    t0 = time.perf_counter()
    s.accept_waveform(16000, np.zeros(int(16000 * 0.5), np.float32))
    s.input_finished()
    while r.is_ready(s):
        r.decode_stream(s)
    dt = time.perf_counter() - t0
    compute += dt
    busy = max(len(x) / 16000.0, busy) + dt
    final = r.get_result(s).strip()
    timeline.append((busy, final))
    return final, compute, timeline


# ---------------------------------------------------------------- the denoisers
def denoiser(name):
    import sherpa_onnx as so
    path = os.path.join(B.OUT, 'denoise', DENOISERS[name])
    if not os.path.exists(path):
        os.makedirs(os.path.dirname(path), exist_ok=True)
        urllib.request.urlretrieve(DENOISE_URL % DENOISERS[name], path)
    if name == 'gtcrn':
        mc = so.OfflineSpeechDenoiserModelConfig(gtcrn=so.OfflineSpeechDenoiserGtcrnModelConfig(model=path), num_threads=1)
    else:
        mc = so.OfflineSpeechDenoiserModelConfig(dpdfnet=so.OfflineSpeechDenoiserDpdfNetModelConfig(model=path), num_threads=1)
    return so.OfflineSpeechDenoiser(so.OfflineSpeechDenoiserConfig(model=mc)), os.path.getsize(path) / 1e6


def denoise(d, x):
    t0 = time.perf_counter()
    out = d.run(x.tolist(), 16000)
    y = np.array(out.samples, dtype=np.float32)
    if out.sample_rate != 16000:
        import make_clips as M
        y = M.to16k(y, out.sample_rate)
    return y, time.perf_counter() - t0


# ---------------------------------------------------------------- running it
def main():
    clips = [c for c in json.load(open(os.path.join(B.OUT, 'clips.json'), encoding='utf-8')) if c['text']]
    spin = B.spinner()
    rows = []                      # (round, name, lang, cond, errors, n, compute, delay, size)
    examples = {}
    try:
        for name, lang, kind, load in specialists():
            t0 = time.time()
            r, mb = load()
            for cond in ('clean', 'room'):
                for c in [c for c in clips if c['lang'] == lang]:
                    x = B.read(os.path.join(B.OUT, 'clips', '%s_%s.wav' % (c['id'], cond)))
                    if kind == 'offline':
                        text, took = run_offline(r, x)
                        delay = took
                    else:
                        text, took, timeline = run_online(r, x, c['speech_end'])
                        target = B.norm(text, lang)
                        done = next((t for t, txt in timeline if B.norm(txt, lang) == target), timeline[-1][0])
                        delay = max(0.0, done - c['speech_end'])
                    e = B.edits(B.norm(c['text'], lang), B.norm(text, lang))
                    rows.append(('2', name, lang, cond, e, len(B.norm(c['text'], lang)), took, delay, mb, c['dur']))
                    if e and cond == 'room':
                        examples.setdefault(name, []).append('%s -> %s' % (c['text'], text or '(nothing)'))
            print('%-34s %4.0f s' % (name, time.time() - t0), flush=True)
            del r
        nemo = B.Nemotron(B.CONFIGS['nemotron-560-int8'])
        for dn in DENOISERS:
            t0 = time.time()
            d, mb = denoiser(dn)
            for cond in ('clean', 'room'):
                for c in clips:
                    x = B.read(os.path.join(B.OUT, 'clips', '%s_%s.wav' % (c['id'], cond)))
                    y, dt = denoise(d, x)
                    out = nemo.run(y, c)
                    e = B.edits(B.norm(c['text'], c['lang']), B.norm(out['text'], c['lang']))
                    rows.append(('3', dn + ' + Nemotron', c['lang'], cond, e, len(B.norm(c['text'], c['lang'])), dt, out['delay'], mb, c['dur']))
            print('%-34s %4.0f s' % (dn, time.time() - t0), flush=True)
    finally:
        spin.kill()
    json.dump(rows, open(os.path.join(B.OUT, 'second.json'), 'w', encoding='utf-8'), ensure_ascii=False)
    report(rows, examples)


def baseline():
    """Nemotron 560 int8 (and Whisper) from bench.py's results: {(name, lang, cond): error %}"""
    out = {}
    for name in ('nemotron-560-int8', 'nemotron-560-int8-bp2', 'nemotron-1120-int8', 'whisper-small', 'whisper-medium'):
        for cond in ('clean', 'room'):
            p = os.path.join(B.RES, '%s_%s.json' % (name, cond))
            if os.path.exists(p):
                d = json.load(open(p, encoding='utf-8'))
                for lang in ('en', 'ru', 'zh', 'es', 'de'):
                    rr = [r for r in d['rows'] if r['lang'] == lang and r['n']]
                    if rr:
                        out[(name, lang, cond)] = 100.0 * sum(r['errors'] for r in rr) / sum(r['n'] for r in rr)
    return out


def report(rows, examples):
    base = baseline()
    L = ['# Rounds 2 and 3 (%s)' % time.strftime('%Y-%m-%d %H:%M'), '',
         'Error % (Chinese: characters). Same clips and conditions as report.md; 4 threads, one other core busy.', '',
         '## 2. Language specialists', '',
         '| model | language | clean err % | room err % | time per line (second pass) / delay (streaming) | size |',
         '|---|---|---|---|---|---|']
    for lang in ('ru', 'zh'):
        for ref in ('nemotron-560-int8', 'whisper-small', 'whisper-medium'):
            if (ref, lang, 'clean') in base:
                L.append('| *%s (for comparison)* | %s | %.1f | %.1f | | |' % (ref, lang, base[(ref, lang, 'clean')], base.get((ref, lang, 'room'), float('nan'))))
        names = []
        for r in rows:
            if r[0] == '2' and r[2] == lang and r[1] not in names:
                names.append(r[1])
        for n in names:
            cell = {}
            for cond in ('clean', 'room'):
                rr = [r for r in rows if r[1] == n and r[3] == cond]
                cell[cond] = 100.0 * sum(r[4] for r in rr) / max(1, sum(r[5] for r in rr))
            rr = [r for r in rows if r[1] == n]
            L.append('| **%s** | %s | %.1f | %.1f | %.2f s median | %.0f MB |' % (
                n, lang, cell['clean'], cell['room'], statistics.median(r[7] for r in rr), rr[0][8]))
    L += ['', '## 3. Noise suppression before Nemotron (560 ms, int8)', '',
          '| | ' + ' | '.join('%s clean / room' % l for l in ('en', 'ru', 'zh', 'es', 'de')) + ' | denoise time per s of audio | size |',
          '|---|' + '---|' * 7]
    langs = ('en', 'ru', 'zh', 'es', 'de')
    L.append('| *no suppression* | ' + ' | '.join('%.1f / %.1f' % (base.get(('nemotron-560-int8', l, 'clean'), float('nan')),
                                                                 base.get(('nemotron-560-int8', l, 'room'), float('nan'))) for l in langs) + ' | | |')
    for dn in DENOISERS:
        n = dn + ' + Nemotron'
        cells = []
        for l in langs:
            v = []
            for cond in ('clean', 'room'):
                rr = [r for r in rows if r[1] == n and r[2] == l and r[3] == cond]
                v.append(100.0 * sum(r[4] for r in rr) / max(1, sum(r[5] for r in rr)))
            cells.append('%.1f / %.1f' % tuple(v))
        rr = [r for r in rows if r[1] == n]
        per_s = sum(r[6] for r in rr) / sum(r[9] for r in rr)
        L.append('| **%s** | %s | %.3f s | %.1f MB |' % (dn, ' | '.join(cells), per_s, rr[0][8]))
    L += ['', '## Specialists: what they got wrong (room)']
    for n, ex in examples.items():
        L += ['', '**%s**' % n] + ['- %s' % e for e in ex[:8]]
    path = os.path.join(B.OUT, 'second.md')
    open(path, 'w', encoding='utf-8').write('\n'.join(L) + '\n')
    print('\n'.join(L))


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--report':
        report(json.load(open(os.path.join(B.OUT, 'second.json'), encoding='utf-8')), {})
    else:
        sys.exit(main())
