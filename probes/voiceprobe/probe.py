"""How does a voice sound when the GAME plays it from short sound files written while it is spoken?
Installs the "Voice Probe (test)" mod and feeds it a spoken sentence in real time, clip by clip, cut and
stitched in several ways; the mod plays the clips (LoadSound / PlaySound) and reports through savegame.xml.

    python probes/voiceprobe/probe.py               # start this first, then play any level with "Voice Probe (test)" on
    python probes/voiceprobe/probe.py --wav my.wav  # your own recording (6-10 s) instead of the computer voice

Needs ffmpeg (env FFMPEG, PATH, or the WinGet install). It measures: said -> heard delay, frame hitches,
late or missing clips. How it SOUNDS only you can tell: note each run (A, B, C...) while it plays.
Stand still for all runs but the last; in the last one the voice is at a point 4 m ahead: walk around it.
Report: export/voiceprobe_report.txt. Clips are prepared in export/voiceprobe/ (gitignored).
"""
import argparse
import array
import glob
import math
import os
import re
import shutil
import statistics
import subprocess
import sys
import time
import wave
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), 'saveprobe'))
from watch import SAVE, read_all          # noqa: E402  (shared-read open: never blocks the game)

EXPORT = os.path.join(os.path.dirname(os.path.dirname(HERE)), 'export')
WORK = os.path.join(EXPORT, 'voiceprobe')
MODS = os.environ.get('HFP_MODS') or os.path.join(os.environ['USERPROFILE'], 'OneDrive', 'Documents', 'Teardown', 'mods')
if not os.path.isdir(MODS):
    MODS = os.path.join(os.environ['USERPROFILE'], 'Documents', 'Teardown', 'mods')
MOD = os.path.join(MODS, 'voice probe')
SIG = os.path.join(MOD, 'sig')
RATE = 44100
FADE, PRE = 0.024, 0.025      # crossfade between clips; silence at a clip's start (skipped when a frame is late)
TEXT = "Hello, can you hear me? I am standing right behind the wall, next to the lazy dog."

VP = re.compile(rb'<vp>(.*?)</vp>', re.S)
KEY = re.compile(rb'<(\w+)\s+value="([^"]*)"')


def ffmpeg():
    cands = [os.environ.get('FFMPEG'), shutil.which('ffmpeg')] + glob.glob(os.path.join(
        os.environ.get('LOCALAPPDATA', ''), 'Microsoft', 'WinGet', 'Packages', 'Gyan.FFmpeg*', '*', 'bin', 'ffmpeg.exe'))
    for c in cands:
        if c and os.path.isfile(c):
            return c
    sys.exit('ffmpeg not found (set FFMPEG=path to ffmpeg.exe)')


def speech(src):
    """the sentence as 44.1 kHz mono samples"""
    os.makedirs(WORK, exist_ok=True)
    if not src:
        src = os.path.join(WORK, 'speech_tts.wav')
        if not os.path.exists(src):                     # (the computer voice that ships with Windows)
            ps = ("Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; "
                  "$s.SetOutputToWaveFile('%s'); $s.Speak('%s'); $s.Dispose()" % (src, TEXT.replace("'", "''")))
            subprocess.run(['powershell', '-NoProfile', '-Command', ps], check=True)
    out = os.path.join(WORK, 'speech_44k.wav')
    subprocess.run([ffmpeg(), '-y', '-loglevel', 'error', '-i', src, '-ac', '1', '-ar', str(RATE),
                    '-af', 'loudnorm=I=-16:TP=-2', '-c:a', 'pcm_s16le', out], check=True)
    with wave.open(out, 'rb') as w:
        x = array.array('h')
        x.frombytes(w.readframes(w.getnframes()))
    return x


def plan():
    """the runs: (id, label, clip seconds, fade, pre-roll, sentence repeats, spatial)"""
    cf = 'crossfaded, lined up'
    return [
        ('A', 'the whole sentence as one file (the best it can sound)', None, 0, 0, 1, False),
        ('B', '0.4 s clips, ' + cf, 0.4, FADE, PRE, 1, False),
        ('C', '0.2 s clips, ' + cf, 0.2, FADE, PRE, 1, False),
        ('D', '0.1 s clips, ' + cf, 0.1, FADE, PRE, 1, False),
        ('E', '0.2 s clips, crossfaded, started on the frame', 0.2, FADE, 0, 1, False),
        ('F', '0.2 s clips, plain cuts', 0.2, 0, 0, 1, False),
        ('G', '0.2 s clips as C, from the VOICE point: walk around it', 0.2, FADE, PRE, 2, True),
    ]


def clip(x, start, length, fade, pre):
    """samples [start - fade/2, start + length + fade/2) of x (zeros outside), faded at both ends, after
    `pre` samples of silence"""
    a, b = start - fade // 2, start + length + fade - fade // 2
    seg = array.array('h', [0]) * (b - a)
    lo, hi = max(a, 0), min(b, len(x))
    if hi > lo:
        seg[lo - a:hi - a] = x[lo:hi]
    for i in range(fade):
        g = (i + 0.5) / fade
        seg[i] = int(seg[i] * g)
        seg[-1 - i] = int(seg[-1 - i] * g)
    return array.array('h', [0]) * pre + seg


def prepare(x, runs):
    """encode every clip of every run to WORK/r<run>_<k>.ogg; returns a dict per run"""
    ff = ffmpeg()
    jobs, out = [], []
    for r, (rid, label, L, fade, pre, reps, spatial) in enumerate(runs, 1):
        src = x * reps
        total = len(src) / RATE
        Ls = len(src) if L is None else int(round(L * RATE))
        n = int(math.ceil(len(src) / Ls))
        fs, ps = int(round(fade * RATE)), int(round(pre * RATE))
        files = []
        for k in range(n):
            wav = os.path.join(WORK, 'r%d_%d.wav' % (r, k))
            with wave.open(wav, 'wb') as w:
                w.setnchannels(1)
                w.setsampwidth(2)
                w.setframerate(RATE)
                w.writeframes(clip(src, k * Ls, Ls, fs, ps).tobytes())
            files.append(wav[:-4] + '.ogg')
            jobs.append(wav)
        out.append(dict(id=rid, label=label, L=Ls / RATE, n=n, lead=(fs // 2 + ps) / RATE, pre=ps / RATE,
                        half=(fs - fs // 2) / RATE, total=total, files=files, spatial=spatial))

    def enc(wav):
        subprocess.run([ff, '-y', '-loglevel', 'error', '-i', wav, '-c:a', 'libvorbis', '-q:a', '5', wav[:-4] + '.ogg'], check=True)
        os.remove(wav)
    with ThreadPoolExecutor(8) as ex:
        list(ex.map(enc, jobs))
    return out


def install(runs):
    if os.path.isdir(MOD):
        shutil.rmtree(MOD)
    shutil.copytree(os.path.join(HERE, 'mod'), MOD)
    os.makedirs(SIG, exist_ok=True)
    rows = ['\t{id = "%s", label = "%s", L = %.6f, n = %d, lead = %.6f, pre = %.6f, spatial = %s},'
            % (r['id'], r['label'], r['L'], r['n'], r['lead'], r['pre'], 'true' if r['spatial'] else 'false') for r in runs]
    with open(os.path.join(MOD, 'plan.lua'), 'w', encoding='utf-8') as f:
        f.write('VP_RUNS = {\n' + '\n'.join(rows) + '\n}\n')


class Game:
    def __init__(self, poll_ms):
        self.poll_s = poll_ms / 1000.0
        self.state, self.last = None, None
        self.frames = []          # (time, n)
        self.plays = {}           # "run.k" -> time first seen

    def poll(self):
        data = read_all(SAVE)
        if data is not None and data != self.last and data.rstrip().endswith(b'</registry>'):
            self.last = data
            m = VP.search(data)
            if m:
                kv = {k.decode(): v.decode() for k, v in KEY.findall(m.group(1))}
                if 'n' in kv:
                    now = time.perf_counter()
                    if not self.frames or self.frames[-1][1] != int(kv['n']):
                        self.frames.append((now, int(kv['n'])))
                    if 'play' in kv:
                        self.plays.setdefault(kv['play'], now)
                    self.state = kv
        time.sleep(self.poll_s)

    def wait(self, cond, timeout):
        t0 = time.perf_counter()
        while time.perf_counter() - t0 < timeout:
            self.poll()
            if self.state and cond(self.state):
                return True
        return False


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--wav', help='a recording to use instead of the computer voice')
    ap.add_argument('--poll', type=float, default=2.0)
    ap.add_argument('--timeout', type=float, default=900.0, help='seconds to wait for the level to start')
    args = ap.parse_args()

    print('preparing the clips...')
    x = speech(args.wav)
    runs = prepare(x, plan())
    install(runs)
    print('sentence %.1f s, %d clips; test mod installed in %s' % (len(x) / RATE, sum(r['n'] for r in runs), MOD))
    print('now start any level with "Voice Probe (test)" enabled (restart Teardown if it is not in the list)')
    for r in runs:
        print('   %s  %s' % (r['id'], r['label']))

    g = Game(args.poll)
    out = []

    def say(s):
        out.append(s)
        print(s)
    n0 = [None]

    def fresh(st):                 # (a level started now, not what an earlier session left in the savegame)
        if n0[0] is None:
            n0[0] = st['n']
        return st['n'] != n0[0] and st.get('run') == '1' and st.get('st') == 'wait'
    if not g.wait(fresh, args.timeout):
        print('no level with the mod started: nothing measured')
        return 1
    say('Voice probe report  %s' % time.strftime('%Y-%m-%d %H:%M:%S'))
    say('%-2s %-52s %11s %9s %8s %6s %11s' % ('', 'run', 'said->heard', 'late max', 'missing', 'fps', 'worst frame'))
    for i, r in enumerate(runs, 1):
        if not g.wait(lambda st: st.get('run') == str(i) and st.get('st') == 'wait', 30):
            say('%s: the mod never asked for this run' % r['id'])
            break
        print('  streaming %s ...' % r['id'])
        t0 = time.perf_counter()
        f0 = len(g.frames)
        for k, src in enumerate(r['files']):
            ready = t0 + min((k + 1) * r['L'], r['total']) + r['half']      # (when this clip's audio has been said)
            while time.perf_counter() < ready:
                g.poll()
            tmp = os.path.join(SIG, 'tmp_%d_%d' % (i, k))
            shutil.copyfile(src, tmp)
            os.replace(tmp, os.path.join(SIG, os.path.basename(src)))     # (appears complete, never half-written)
        got = g.wait(lambda st: ('r%d' % i) in st, r['total'] + 15)
        fr = g.frames[f0:]
        delays = [g.plays['%d.%d' % (i, k)] - (t0 + k * r['L']) + r['lead']
                  for k in range(r['n']) if '%d.%d' % (i, k) in g.plays]
        gaps = [b[0] - a[0] for a, b in zip(fr, fr[1:])]
        fps = (fr[-1][1] - fr[0][1]) / (fr[-1][0] - fr[0][0]) if len(fr) > 10 else 0
        if got:
            under, late_max, late_mean, played = g.state['r%d' % i].split(',')
            say('%-2s %-52s %8.0f ms %6.0f ms %8s %6.1f %8.0f ms' % (
                r['id'], r['label'][:52], statistics.median(delays) * 1000 if delays else float('nan'),
                float(late_max), under, fps, max(gaps) * 1000 if gaps else 0))
        else:
            say('%-2s %-52s no result from the mod (%d of %d clips started)' % (r['id'], r['label'][:52], len(delays), r['n']))
    g.wait(lambda st: st.get('st') == 'done', 10)

    say('')
    say('said->heard: from when a clip\'s first sound was "spoken" to the game starting it (median; the save-file')
    say('  leg, ~10 ms, is in it; a real helper adds the network, 30-80 ms). Run A waits for the whole sentence.')
    say('late max: the latest a clip started after its time (a frame is 17 ms at 60 fps). missing: clips not there in time.')
    stuck = 0
    for name in os.listdir(SIG):
        try:
            os.remove(os.path.join(SIG, name))
        except OSError:
            stuck += 1
    say('clip files the game still held open at the end: %d' % stuck)
    os.makedirs(EXPORT, exist_ok=True)
    path = os.path.join(EXPORT, 'voiceprobe_report.txt')
    with open(path, 'w', encoding='utf-8') as f:
        f.write('\n'.join(out) + '\n')
    print('\nreport written to', path)
    print('Now: how did each run sound (A-G)? Clicks, stutter, robot voice, or fine?')
    return 0


if __name__ == '__main__':
    sys.exit(main())
