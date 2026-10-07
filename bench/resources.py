"""What Kotodama costs a player's PC: the built app (dist/Kotodama) run against a fake game (as engine/test_e2e.py: a
savegame feed that wants the microphone) with a recording as the microphone, its CPU and memory sampled 4 times a
second. Phases: idle (connected, models not loaded yet), loading, talking (the recording: lines with pauses), after.

    <conda>/envs/pcvoice/python.exe bench/resources.py [en|auto|ru|zh ...] [--no-live]

CPU is in "% of one core" (100 = one core busy) and as a share of the whole CPU; memory is the private bytes and the
working set. Report: printed, and appended to export/asrbench/resources.md.
"""
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import wave

import numpy as np
import psutil

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
EXE = os.environ.get('KOTODAMA_EXE') or os.path.join(ROOT, 'dist', 'Kotodama', 'Kotodama.exe')
BENCH = os.path.join(ROOT, 'export', 'asrbench')
XML = ('<registry version="2.1.0">\n<savegame><mod><local-proximity-chat>\n<pcvx>\n<f value="%s"/>\n</pcvx>\n'
       '</local-proximity-chat></mod></savegame>\n</registry>\n')
LINES = {'en': ['en01', 'en02', 'en05', 'en07', 'en04', 'en13'], 'ru': ['ru01', 'ru02', 'ru05', 'ru10'],
         'zh': ['zh03', 'zh07', 'zh01', 'zh05']}


def recording(path, lang):
    clips = {c['id']: c for c in json.load(open(os.path.join(BENCH, 'clips.json'), encoding='utf-8'))}
    ids = LINES.get(lang) or (LINES['en'][:3] + LINES['ru'][:2] + LINES['zh'][:2])     # (auto: three languages)
    parts = [np.zeros(16000, np.float32)]
    for cid in ids:
        c = clips[cid]
        with wave.open(os.path.join(BENCH, 'clips', '%s_room.wav' % cid), 'rb') as w:
            x = np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(np.float32) / 32768
        parts += [x[int((c['speech_start'] - 0.05) * 16000):int((c['speech_end'] + 0.05) * 16000)], np.zeros(int(16000 * 2.0), np.float32)]
    y = np.concatenate(parts + [np.zeros(16000 * 3, np.float32)])
    with wave.open(path, 'wb') as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(16000)
        w.writeframes((np.clip(y, -1, 1) * 32767).astype(np.int16).tobytes())
    return len(y) / 16000


def main():
    args = [a for a in sys.argv[1:] if not a.startswith('--')]
    lang = args[0] if args else 'en'
    live = '--no-live' not in sys.argv
    cores = psutil.cpu_count()
    with tempfile.TemporaryDirectory() as td, tempfile.TemporaryDirectory() as mods:
        wav = os.path.join(td, 'mic.wav')
        dur = recording(wav, lang)
        save = os.path.join(td, 'savegame.xml')
        stop = threading.Event()
        state = dict(seq=0)

        def game():                                   # (the fake game: a feed 20 times a second, the mic wanted)
            while not stop.is_set():
                state['seq'] += 1
                feed = '4|%d|1.00|11|0|1|1|%s|%d|' % (state['seq'], lang, 1 if live else 0)
                try:
                    with open(save, 'w') as f:
                        f.write(XML % feed)
                except PermissionError:
                    pass
                time.sleep(0.05)
        threading.Thread(target=game, daemon=True).start()
        time.sleep(0.3)
        env = dict(os.environ, SAVEPROBE_DIR=td, HFP_MODS=mods)
        p = subprocess.Popen([EXE, '--cli', '--volume', '0', '--mic-wav', wav, '--seconds', str(int(dur + 30))], env=env,
                             stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding='utf-8', errors='replace')
        out = []
        threading.Thread(target=lambda: out.extend(p.stdout), daemon=True).start()
        proc = psutil.Process(p.pid)
        limit = next((int(a.split('=')[1]) for a in sys.argv if a.startswith('--cores=')), 0)
        if limit:                                     # (a smaller CPU: the app on `limit` physical cores, no SMT twins)
            proc.cpu_affinity(list(range(0, 2 * limit, 2)))
        proc.cpu_percent(None)
        samples = []                                  # (t, cpu % of one core, private MB, working set MB)
        t0 = time.time()
        talk_start = None
        while p.poll() is None and time.time() - t0 < dur + 40:
            time.sleep(0.25)
            try:
                cpu = proc.cpu_percent(None)
                mi = proc.memory_info()
            except psutil.NoSuchProcess:
                break
            samples.append((time.time() - t0, cpu, getattr(mi, 'private', mi.vms) / 1e6, mi.rss / 1e6))
            text = ''.join(out)
            if talk_start is None and 'playing the recording' in text:
                talk_start = time.time() - t0
            if talk_start is not None and time.time() - t0 > talk_start + dur + 12:
                break                                 # (after the recording: 12 s of silence with the mic open)
        p.kill()
        stop.set()
    text = ''.join(out)
    said = [l.split('you said', 1)[1].strip(': ') for l in text.replace('\r', '\n').split('\n') if 'you said' in l]
    a = np.array(samples)
    ts = talk_start or 0
    load = a[a[:, 0] < ts] if talk_start else a[:0]
    talk = a[(a[:, 0] >= ts) & (a[:, 0] <= ts + dur)] if talk_start else a[:0]
    quiet = a[(a[:, 0] >= ts + dur + 2) & (a[:, 0] <= ts + dur + 12)] if talk_start else a[:0]

    def row(name, s):
        if not len(s):
            return '| %s | - | - | - | - |' % name
        return '| %s | %.0f %% (%.1f %% of the CPU) | %.0f %% | %.0f MB | %.0f MB |' % (
            name, s[:, 1].mean(), s[:, 1].mean() / cores, s[:, 1].max(), s[:, 2].max(), s[:, 3].max())
    slow = [l.strip() for l in text.splitlines() if 'live words every' in l]
    limit = next((int(a.split('=')[1]) for a in sys.argv if a.startswith('--cores=')), 0)
    L = ['', '## %s, live words %s (%s, %s)' % (lang, 'on' if live else 'off', time.strftime('%Y-%m-%d %H:%M'),
                                                 '%d cores only' % limit if limit else '%d logical cores' % cores), '',
         'slow-PC adaptation: %s' % ('; '.join(slow) or 'none'), '',
         '%.0f s of recording (%d lines); the app said %d lines.' % (dur, len(LINES.get(lang, [0] * 7)), len(said)), '',
         '| phase | CPU mean (one core = 100 %) | CPU peak (0.25 s) | private memory peak | working set peak |',
         '|---|---|---|---|---|', row('start: the models load', load), row('talking (lines with 2 s pauses)', talk),
         row('mic open, player silent', quiet)]
    print('\n'.join(L))
    for s in said:
        print('   said:', s)
    with open(os.path.join(BENCH, 'resources.md'), 'a', encoding='utf-8') as f:
        f.write('\n'.join(L) + '\n')


if __name__ == '__main__':
    main()
