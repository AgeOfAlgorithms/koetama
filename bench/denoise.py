"""The noise suppressors Koetama might use (docs/noise-suppression.md), on the same speech and noises:
RNNoise (nnnoiseless), DPDFNet 48 kHz (sherpa-onnx's streaming denoiser) and DeepFilterNet 3 (libDF), each run by
bench/denoise (Rust, as the app would run it: one thread, its own frame size).

For each noise (real keyboard typing, mouse clicks and a vacuum cleaner from ESC-50, and a synthetic fan: pink noise
and a hum) at two signal-to-noise ratios it measures:
  - noise removed: the noise alone, before and after (dB);
  - the speech kept: clean speech alone through the model, against itself (SI-SDR, and its level change);
  - the result: the noisy mix through the model against the clean speech (SI-SDR), and what speech to text
    (Koetama's own, Parakeet) makes of it (word error rate);
  - the delay the model adds (the clean speech's cross-correlation);
and, paced in real time (each 10 ms frame waits for its moment, as a microphone delivers), each frame's processing
time: mean, p99, max, and the share of one core. Listening files for each model: export/noise/listen/.

    python bench/denoise.py              (everything; ~15 min; heavy - not while someone plays)
    python bench/denoise.py --cpu-only   (only the paced CPU runs: e.g. again with a game open)

Needs: export/asrbench/clips (bench/make_clips.py), export/noise/esc50 (ESC-50's keyboard_typing, mouse_click,
vacuum_cleaner: CC BY-NC 3.0 as a whole - local tests only, never committed), export/noise/models
(dpdfnet2_48khz_hr.onnx, DeepFilterNet3_onnx.tar.gz), the app built (app/target/debug/koetama for speech to text).
Writes export/noise/results.md and .json.
"""
import glob, json, os, random, re, shutil, subprocess, sys, time, wave
import numpy as np
from scipy.signal import resample_poly

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = '.exe' if os.name == 'nt' else ''
BENCH = os.path.join(ROOT, 'bench', 'denoise')
TOOL = os.path.join(BENCH, 'target', 'release', 'denoise_bench' + EXE)
KOETAMA = os.path.join(ROOT, 'app', 'target', 'debug', 'koetama' + EXE)
CLIPS = os.path.join(ROOT, 'export', 'asrbench', 'clips')
ESC = os.path.join(ROOT, 'export', 'noise', 'esc50')
MODELS = os.path.join(ROOT, 'export', 'noise', 'models')
OUT = os.path.join(ROOT, 'export', 'noise', 'run')
LISTEN = os.path.join(ROOT, 'export', 'noise', 'listen')
RATE = 48000
MODEL_NAMES = ['rnnoise', 'dpdfnet', 'dfn3']
LINES = ['en01', 'en02', 'en03', 'en04', 'en06', 'en08', 'en09', 'en10']  # (sentences: one-word lines are unstable in STT)
NOISES = ['keyboard', 'mouse', 'vacuum', 'fan']
SNRS = [5, 15]


def env():
    e = dict(os.environ)
    if not e.get('SHERPA_ONNX_LIB_DIR'):
        found = glob.glob(os.path.join(ROOT, 'app', 'target', 'sherpa-onnx-prebuilt', '*', 'lib'))
        if found:
            e['SHERPA_ONNX_LIB_DIR'] = found[0]
    return e


def build():
    print('building bench/denoise (release)...', flush=True)
    subprocess.run(['cargo', 'build', '--release', '-q'], cwd=BENCH, env=env(), check=True)
    lib = env().get('SHERPA_ONNX_LIB_DIR')
    if lib and os.name == 'nt':
        for dll in glob.glob(os.path.join(lib, '*.dll')):
            shutil.copy(dll, os.path.dirname(TOOL))


def read(path):
    with wave.open(path) as w:
        n, sr, ch, sw = w.getnframes(), w.getframerate(), w.getnchannels(), w.getsampwidth()
        raw = w.readframes(n)
    if sw == 2:
        x = np.frombuffer(raw, np.int16).astype(np.float64) / 32768
    elif sw == 4:
        x = np.frombuffer(raw, np.float32).astype(np.float64) if path.endswith('.f32.wav') else np.frombuffer(raw, np.int32) / 2**31
    else:
        raise ValueError(path)
    return x.reshape(-1, ch).mean(axis=1), sr


def read_f32(path):
    """a wav the Rust tool wrote (32-bit float)"""
    import struct
    with open(path, 'rb') as f:
        data = f.read()
    i = data.index(b'data') + 8
    return np.frombuffer(data[i:], np.float32).astype(np.float64)


def write(path, x):
    y = np.clip(x, -1, 1)
    with wave.open(path, 'wb') as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes((y * 32767).astype(np.int16).tobytes())


def to48(x, sr):
    if sr == RATE:
        return x
    from math import gcd
    g = gcd(RATE, sr)
    return resample_poly(x, RATE // g, sr // g)


def active_power(x):
    """the power of the 20 ms windows with sound in them (the speech's level, as Koetama measures it)"""
    w = np.array([np.mean(x[i:i + 960] ** 2) for i in range(0, len(x) - 959, 960)])
    on = w[w > w.max() * 1e-3]
    return float(np.mean(on))


def db(p):
    return 10 * np.log10(max(p, 1e-20))


def speech_track(texts):
    clips = json.load(open(os.path.join(ROOT, 'export', 'asrbench', 'clips.json'), encoding='utf-8'))
    ref = {c['id']: c['text'] for c in clips}
    parts, words = [np.zeros(int(1.5 * RATE))], []
    for cid in LINES:
        x, sr = read(os.path.join(CLIPS, f'{cid}_clean.wav'))
        parts += [to48(x, sr), np.zeros(int(0.8 * RATE))]
        words.append(ref[cid])
    parts.append(np.zeros(int(1.5 * RATE)))
    s = np.concatenate(parts)
    return s * np.sqrt(0.01 / active_power(s)), ' '.join(words)  # (speech at -20 dBFS, as Koetama's gain aims)


def noise_track(kind, n, rng):
    if kind == 'fan':
        # (pink noise and a 120 Hz hum with harmonics, as bench/make_clips.py's room)
        w = rng.standard_normal(n + RATE)
        f = np.fft.rfft(w)
        f[1:] /= np.sqrt(np.arange(1, len(f)))
        pink = np.fft.irfft(f)[:n]
        t = np.arange(n) / RATE
        hum = sum(np.sin(2 * np.pi * 120 * k * t) / k for k in (1, 2, 3))
        x = pink / pink.std() + 0.5 * hum / hum.std()
    else:
        folder = {'keyboard': 'keyboard_typing', 'mouse': 'mouse_click', 'vacuum': 'vacuum_cleaner'}[kind]
        files = sorted(glob.glob(os.path.join(ESC, folder, '*.wav')))
        rng.shuffle(files)
        parts, total = [], 0
        for f in files:
            y, sr = read(f)
            y = to48(y, sr)
            y = y - y.mean()
            if np.abs(y).max() < 1e-4:
                continue
            parts.append(y / np.sqrt(np.mean(y ** 2) + 1e-12))
            total += len(y)
            if total >= n:
                break
        x = np.concatenate(parts)[:n]
    return x / np.sqrt(np.mean(x ** 2))


def run_model(model, wav_in, wav_out, paced=False):
    args = [TOOL, model, wav_in, wav_out, '--models', MODELS] + (['--paced'] if paced else [])
    p = subprocess.run(args, env=env(), capture_output=True, text=True)
    if p.returncode != 0:
        raise SystemExit(f'{model} failed:\n{p.stdout}\n{p.stderr}')
    return json.loads(p.stdout.strip().splitlines()[-1])


def delay(ref, out):
    """the lag (samples, 0..200 ms) where out best matches ref"""
    n = min(len(ref), len(out))
    a, b = ref[:n], out[:n]
    best, lag = -1.0, 0
    for k in range(0, int(0.2 * RATE), 48):
        c = np.dot(a[:n - k], b[k:n]) / (np.linalg.norm(a[:n - k]) * np.linalg.norm(b[k:n]) + 1e-12)
        if c > best:
            best, lag = c, k
    for k in range(max(0, lag - 48), lag + 48):
        c = np.dot(a[:n - k], b[k:n]) / (np.linalg.norm(a[:n - k]) * np.linalg.norm(b[k:n]) + 1e-12)
        if c > best:
            best, lag = c, k
    return lag


def align(out, lag, n):
    y = out[lag:lag + n]
    return np.pad(y, (0, n - len(y)))


def si_sdr(ref, est):
    a = np.dot(est, ref) / (np.dot(ref, ref) + 1e-12)
    t = a * ref
    return 10 * np.log10(np.dot(t, t) / (np.dot(est - t, est - t) + 1e-12))


def transcribe(path):
    out = subprocess.run([KOETAMA, '--cli', '--transcribe', path, '--lang', 'en'], env=env(), capture_output=True,
                         text=True, encoding='utf-8', errors='replace').stdout
    return ' '.join(m.group(1) for m in re.finditer(r'^LINE \d+: (.*?)   \[', out, re.M) if m.group(1) != '(nothing)')


def wer(ref, hyp):
    r, h = re.findall(r"[a-z']+", ref.lower()), re.findall(r"[a-z']+", hyp.lower())
    d = list(range(len(h) + 1))
    for i in range(1, len(r) + 1):
        prev, d[0] = d[0], i
        for j in range(1, len(h) + 1):
            cur = min(d[j] + 1, d[j - 1] + 1, prev + (r[i - 1] != h[j - 1]))
            prev, d[j] = d[j], cur
    return d[len(h)] / max(1, len(r))


def cpu_runs(track_path):
    rows = {}
    for m in MODEL_NAMES:
        print(f'  paced: {m}', flush=True)
        rows[m] = run_model(m, track_path, os.path.join(OUT, f'paced-{m}.f32.wav'), paced=True)
    return rows


def main():
    cpu_only = '--cpu-only' in sys.argv
    os.makedirs(OUT, exist_ok=True)
    os.makedirs(LISTEN, exist_ok=True)
    build()
    rng = np.random.default_rng(7)
    speech, text = speech_track(LINES)
    n = len(speech)
    clean_path = os.path.join(OUT, 'clean.wav')
    write(clean_path, speech)
    results = {'speech_s': n / RATE, 'lines': LINES, 'conditions': {}, 'models': {}}
    if not cpu_only:
        # (the clean speech through each model: what it does to speech, and its delay)
        for m in MODEL_NAMES:
            st = run_model(m, clean_path, os.path.join(OUT, f'clean-{m}.f32.wav'))
            out = read_f32(os.path.join(OUT, f'clean-{m}.f32.wav'))
            lag = delay(speech, out)
            y = align(out, lag, n)
            results['models'][m] = {'delay_ms': lag / RATE * 1000, 'speech_si_sdr': si_sdr(speech, y),
                                    'speech_level_db': db(active_power(y)) - db(active_power(speech)), 'unpaced': st}
            print(f'  {m}: delay {lag / RATE * 1000:.1f} ms, clean speech SI-SDR {si_sdr(speech, y):.1f} dB', flush=True)
        for kind in NOISES:
            base = noise_track(kind, n, rng)
            for snr in SNRS:
                key = f'{kind}-{snr}dB'
                noise = base * np.sqrt(active_power(speech) / 10 ** (snr / 10))
                noisy = speech + noise
                pn, pm = os.path.join(OUT, f'{key}-noise.wav'), os.path.join(OUT, f'{key}-noisy.wav')
                write(pn, noise)
                write(pm, noisy)
                row = {'raw': {'si_sdr': si_sdr(speech, noisy), 'wer': wer(text, transcribe(pm))}}
                print(f'{key}: raw SI-SDR {row["raw"]["si_sdr"]:.1f}, WER {row["raw"]["wer"]:.0%}', flush=True)
                for m in MODEL_NAMES:
                    lag = int(results['models'][m]['delay_ms'] * RATE / 1000)
                    on = os.path.join(OUT, f'{key}-noise-{m}.f32.wav')
                    run_model(m, pn, on)
                    out_noise = align(read_f32(on), lag, n)
                    om = os.path.join(OUT, f'{key}-{m}.f32.wav')
                    run_model(m, pm, om)
                    out = align(read_f32(om), lag, n)
                    om16 = os.path.join(OUT, f'{key}-{m}.wav')
                    write(om16, out)
                    row[m] = {'noise_removed_db': db(np.mean(noise ** 2)) - db(np.mean(out_noise ** 2)),
                              'si_sdr': si_sdr(speech, out), 'wer': wer(text, transcribe(om16))}
                    print(f'  {m}: noise -{row[m]["noise_removed_db"]:.1f} dB, SI-SDR {row[m]["si_sdr"]:.1f}, '
                          f'WER {row[m]["wer"]:.0%}', flush=True)
                    if snr == SNRS[0]:
                        shutil.copy(om16, os.path.join(LISTEN, f'{key}-{m}.wav'))
                if snr == SNRS[0]:
                    shutil.copy(pm, os.path.join(LISTEN, f'{key}-noisy.wav'))
                results['conditions'][key] = row
        shutil.copy(clean_path, os.path.join(LISTEN, 'clean.wav'))
    # (paced in real time, on the keyboard mix: the cost at a microphone's pace)
    print('paced CPU runs...', flush=True)
    track = os.path.join(OUT, f'keyboard-{SNRS[0]}dB-noisy.wav')
    if not os.path.exists(track):
        noise = noise_track('keyboard', n, rng) * np.sqrt(active_power(speech) / 10 ** (SNRS[0] / 10))
        write(track, speech + noise)
    results['paced'] = cpu_runs(track)
    results['when'] = time.strftime('%Y-%m-%d %H:%M')
    tag = 'cpu' if cpu_only else 'all'
    with open(os.path.join(ROOT, 'export', 'noise', f'results-{tag}.json'), 'w') as f:
        json.dump(results, f, indent=1)
    report(results, os.path.join(ROOT, 'export', 'noise', f'results-{tag}.md'))


def report(r, path):
    L = [f'# Noise suppression benchmark ({r["when"]})', '',
         f'{len(r["lines"])} English sentences ({r["speech_s"]:.0f} s, computer voices recorded at 16 kHz: nothing above '
         '8 kHz), speech at -20 dBFS. Noise level against the speech\'s active level.', '']
    if r['models']:
        L += ['| model | delay (ms) | clean speech kept (SI-SDR dB) | clean speech level change (dB) |', '|---|---|---|---|']
        for m, v in r['models'].items():
            L.append(f'| {m} | {v["delay_ms"]:.1f} | {v["speech_si_sdr"]:.1f} | {v["speech_level_db"]:+.1f} |')
        L += ['', '| noise | raw SI-SDR / WER | ' + ' | '.join(f'{m}: noise removed / SI-SDR / WER' for m in MODEL_NAMES) + ' |',
              '|---|---|' + '---|' * len(MODEL_NAMES)]
        for key, row in r['conditions'].items():
            cells = [f'{row["raw"]["si_sdr"]:.1f} / {row["raw"]["wer"]:.0%}']
            cells += [f'-{row[m]["noise_removed_db"]:.1f} dB / {row[m]["si_sdr"]:.1f} / {row[m]["wer"]:.0%}' for m in MODEL_NAMES]
            L.append(f'| {key} | ' + ' | '.join(cells) + ' |')
    L += ['', '**Paced in real time** (one thread, each frame at its moment):', '',
          '| model | frame (ms) | mean (ms) | p99 (ms) | max (ms) | share of one core | load (s) |', '|---|---|---|---|---|---|---|']
    for m, v in r['paced'].items():
        L.append(f'| {m} | {v["frame_ms"]:.1f} | {v["mean_ms"]:.3f} | {v["p99_ms"]:.3f} | {v["max_ms"]:.2f} | '
                 f'{v["core_share"]:.1%} | {v["load_s"]:.2f} |')
    with open(path, 'w', encoding='utf-8') as f:
        f.write('\n'.join(L) + '\n')
    print('\n'.join(L))


if __name__ == '__main__':
    main()
