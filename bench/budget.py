"""The voice helper's compute budget, as it runs now (rolling design, asr.RollingLine): per language and with "auto"
(language detection + stitching), with 4 and 2 threads, while another core is kept busy (Teardown's main thread).

    C:/Users/user/miniconda3/envs/pcvoice/python.exe bench/budget.py

Per configuration: CPU while talking (cores busy on average: process CPU time / seconds of speech), the longest
single pass (how far behind the live words can fall), the delay of the finished line after the speech ends,
memory. Report: export/asrbench/budget.md
"""
import json
import os
import sys
import time

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(os.path.dirname(HERE), 'engine'))
import bench as B           # noqa: E402
import asr                  # noqa: E402


def lines(lang):
    clips = [c for c in json.load(open(os.path.join(B.OUT, 'clips.json'), encoding='utf-8')) if c['text'] and c['lang'] == lang]
    out = []
    for c in clips:
        x = B.read(os.path.join(B.OUT, 'clips', '%s_room.wav' % c['id']))
        a0 = max(0.0, c['speech_start'] - 1.0)
        out.append((x[int(a0 * 16000):int((c['speech_end'] + 0.5) * 16000)], c['speech_end'] - c['speech_start']))
    return out


def mixed():
    items = [i for i in json.load(open(os.path.join(B.OUT, 'lid', 'items.json'), encoding='utf-8')) if i['kind'] == 'mixed']
    out = []
    for it in items:
        x = B.read(it['path'])
        out.append((np.concatenate([np.zeros(16000, np.float32), x]), it['dur'] - 0.6))
    return out


def run(models, lang, data, live=True):
    import psutil
    proc = psutil.Process()
    cpu0 = sum(proc.cpu_times()[:2])
    speech, longest, finals = 0.0, 0.0, []
    for x, sp in data:
        line = asr.RollingLine(models, 1, lang, x[:16000], live=live)
        body = x[16000:]
        n = int(16000 * 0.05)
        for k in range(0, len(body), n):
            t0 = time.perf_counter()
            line.feed(body[k:k + n])
            longest = max(longest, time.perf_counter() - t0)
        t0 = time.perf_counter()
        line.finish()
        finals.append(time.perf_counter() - t0)
        speech += sp
    cores = (sum(proc.cpu_times()[:2]) - cpu0) / speech
    return cores, longest, float(np.median(finals))


GAME = r"""
import time, sys
# a fake game: 16.7 ms frames of busy work; counts frames that end more than 25 ms after they began
n = late = 0
t_end = time.perf_counter() + float(sys.argv[1])
while time.perf_counter() < t_end:
    t0 = time.perf_counter()
    while time.perf_counter() - t0 < 0.012:
        pass
    left = 0.0167 - (time.perf_counter() - t0)
    if left > 0:
        time.sleep(left)
    n += 1
    if time.perf_counter() - t0 > 0.025:
        late += 1
print(n, late)
"""


def contention(low, seconds=40):
    """the fake game at normal priority on a full processor (a spinner on every other core), while this process
    transcribes English lines with 4 threads at normal or below-normal priority: late game frames"""
    import ctypes
    import subprocess
    k32 = ctypes.windll.kernel32
    k32.SetPriorityClass(k32.GetCurrentProcess(), 0x00004000 if low else 0x00000020)
    spinners = [B.spinner() for _ in range(os.cpu_count() - 2)]
    game = subprocess.Popen([sys.executable, '-c', GAME, str(seconds)], stdout=subprocess.PIPE, text=True)
    models = asr.Models(threads=4, log=lambda s: None)
    models.get('parakeet')
    data = lines('en')
    t0 = time.time()
    while time.time() - t0 < seconds:
        run(models, 'en', data[:4])
    out = game.communicate()[0].split()
    for sp in spinners:
        sp.kill()
    k32.SetPriorityClass(k32.GetCurrentProcess(), 0x00000020)
    return int(out[0]), int(out[1])


def stitch_error(models, hop):
    import re
    asr.LID_HOP = hop
    items = [i for i in json.load(open(os.path.join(B.OUT, 'lid', 'items.json'), encoding='utf-8')) if i['kind'] == 'mixed']

    def toks(t):
        out = []
        for w in B.norm(t, 'en'):
            out += B.norm(w, 'zh') if re.search(r'[\u3400-\u9fff]', w) else [w]
        return out
    e = n = 0
    for it in items:
        text, _, _ = asr.transcribe_mixed(models, B.read(it['path']))
        e += B.edits(toks(it['text']), toks(text))
        n += len(toks(it['text']))
    asr.LID_HOP = 0.25
    return 100.0 * e / n


def main():
    rows, notes = [], []
    spin = B.spinner()
    try:
        for threads in (4, 2):
            models = asr.Models(threads=threads, log=lambda s: None)
            for name in ('parakeet', 'gigaam', 'sensevoice', 'langid'):
                models.get(name)
            for label, lang, data, live in (('English', 'en', lines('en'), True), ('Russian', 'ru', lines('ru'), True),
                                            ('Chinese', 'zh', lines('zh'), True),
                                            ('English, live words off', 'en', lines('en'), False),
                                            ('Russian, live words off', 'ru', lines('ru'), False),
                                            ('Chinese, live words off', 'zh', lines('zh'), False)):
                cores, longest, fin = run(models, lang, data, live)
                rows.append((threads, label, cores, longest, fin))
                print(threads, label, '%.2f cores' % cores, flush=True)
            for hop in (0.25, 0.5):
                asr.LID_HOP = hop
                cores, longest, fin = run(models, 'auto', mixed())
                asr.LID_HOP = 0.25
                rows.append((threads, 'Auto language, mixed lines, detector every %.2f s' % hop, cores, longest, fin))
                print(threads, 'auto hop', hop, '%.2f cores' % cores, flush=True)
            if threads == 4:
                for hop in (0.25, 0.5):
                    notes.append('Auto language, detector every %.2f s: %.0f %% of the words wrong on the 20 mixed lines' % (hop, stitch_error(models, hop)))
            del models
        # a slow PC: one thread (the pass interval stretches by itself)
        models = asr.Models(threads=1, log=lambda s: None)
        models.get('parakeet')
        cores, longest, fin = run(models, 'en', lines('en'))
        rows.append((1, 'English, ONE thread (a slow PC stand-in)', cores, longest, fin))
        notes.append('One thread: the live-word interval stretched itself to %.0f s (a pass took more than half of 1 s)' % getattr(models, 'every', 1.0))
    finally:
        spin.kill()
    for low in (False, True):
        n, late = contention(low)
        notes.append('Full processor (a busy loop on every core but two), a fake game at normal priority, the helper transcribing English '
                     'with 4 threads at %s priority: %d of %d game frames late (%.1f %%)' % ('BELOW-NORMAL' if low else 'normal', late, n, 100.0 * late / max(1, n)))
        print(notes[-1], flush=True)
    mem = {}
    import subprocess
    for name in ('parakeet', 'gigaam', 'sensevoice', 'langid'):
        code = ('import sys, psutil; sys.path.insert(0, %r); import asr; m = asr.Models(log=lambda s: None); b = psutil.Process().memory_info().rss; '
                'm.get(%r); print(round((psutil.Process().memory_info().rss - b) / 1e6))' % (os.path.join(os.path.dirname(HERE), 'engine'), name))
        out = subprocess.run([sys.executable, '-c', code], capture_output=True, text=True, env=dict(os.environ, HF_HUB_DISABLE_SYMLINKS_WARNING='1')).stdout.split()
        mem[name] = int(out[-1]) if out else None
    L = ['# Voice helper compute budget (%s)' % time.strftime('%Y-%m-%d %H:%M'), '',
         'Rolling design (a pass every %.0f s, stretching on a slow PC, + the final pass), room clips, one other core kept busy. '
         '"Cores while talking" = the CPU time of the helper / seconds of speech (1.0 = one core fully busy while someone talks; '
         'nothing while quiet).' % asr.ROLL_EVERY, '',
         '| threads | what | cores while talking | longest single pass | finished line after the speech ends (+0.5 s pause) |',
         '|---|---|---|---|---|']
    for t, label, cores, longest, fin in rows:
        L.append('| %d | %s | %.2f | %.2f s | %.2f s |' % (t, label, cores, longest, fin))
    L += [''] + ['- ' + n for n in notes]
    L += ['', '| model | memory (MB) |', '|---|---|'] + ['| %s | %s |' % kv for kv in mem.items()]
    path = os.path.join(B.OUT, 'budget.md')
    open(path, 'w', encoding='utf-8').write('\n'.join(L) + '\n')
    print('\n'.join(L))


if __name__ == '__main__':
    sys.exit(main())
