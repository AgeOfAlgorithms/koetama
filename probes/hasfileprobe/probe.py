"""Can a helper program signal the game by creating and deleting files (HasFile), and does LoadSound
read a sound file that appeared after the level loaded? Installs the "HasFile Probe (test)" mod, toggles
signal files while it runs and reads what the mod saw from savegame.xml (which follows the game within
~20 ms: probes/saveprobe).

    python probes/hasfileprobe/probe.py     # start this first, then play any level with "HasFile Probe (test)" on

It only reads savegame.xml. It writes: the installed test mod (its sig/ folder), one file next to the mod
folders (hfp_up.txt) and one in %LOCALAPPDATA%/Teardown/hfp/; all removed at the end.
Report: export/hasfileprobe_report.txt. Then tell which beeps you heard in the SOUND phase (1, 2, 3).
"""
import argparse
import os
import re
import shutil
import statistics
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), 'saveprobe'))
from watch import TD, SAVE, read_all          # noqa: E402  (shared-read open: never blocks the game)

EXPORT = os.path.join(os.path.dirname(os.path.dirname(HERE)), 'export')
MODS = os.environ.get('HFP_MODS') or os.path.join(os.environ['USERPROFILE'], 'OneDrive', 'Documents', 'Teardown', 'mods')
if not os.path.isdir(MODS):
    MODS = os.path.join(os.environ['USERPROFILE'], 'Documents', 'Teardown', 'mods')
MOD = os.path.join(MODS, 'hasfile probe')
SIG = os.path.join(MOD, 'sig')
ABS = os.path.join(TD, 'hfp', 'abs.txt')

# the mod's FILES, in its order: (label, the path here, what it tells)
CHANNELS = [
    ('pre', os.path.join(SIG, 'pre.txt'), 'in the mod folder, there before the level loaded'),
    ('a.txt', os.path.join(SIG, 'a.txt'), 'in the mod folder, created while the level runs'),
    ('b', os.path.join(SIG, 'b'), 'the same, a name without an extension'),
    ('up', os.path.join(MODS, 'hfp_up.txt'), 'outside the mod folder by MOD/../'),
    ('abs', ABS, 'outside the mod folder by an absolute path (AppData/Local/Teardown/hfp)'),
]

HFP = re.compile(rb'<hfp>(.*?)</hfp>', re.S)
KEY = re.compile(rb'<(\w+)\s+value="([^"]*)"')


def install():
    if os.path.isdir(MOD):
        shutil.rmtree(MOD)
    shutil.copytree(os.path.join(HERE, 'mod'), MOD)
    with open(os.path.join(MOD, 'abs.lua'), 'w', encoding='utf-8') as f:
        f.write('HFP_ABS = "%s"\n' % ABS.replace('\\', '/'))
    os.makedirs(SIG, exist_ok=True)
    os.makedirs(os.path.dirname(ABS), exist_ok=True)
    for _, path, _ in CHANNELS[1:]:
        remove(path)
    touch(CHANNELS[0][1])


def touch(path):
    with open(path, 'w') as f:
        f.write('x')


def remove(path):
    try:
        os.remove(path)
    except FileNotFoundError:
        pass


class Probe:
    def __init__(self, poll_ms):
        self.poll_s = poll_ms / 1000.0
        self.state = None            # the mod's keys, newest
        self.frames = []             # (time, n, ph)
        self.dropped = False
        self.last = None

    def poll(self):
        data = read_all(SAVE)
        if data is not None and data != self.last and data.rstrip().endswith(b'</registry>'):
            self.last = data
            m = HFP.search(data)
            if m:
                kv = {k.decode(): v.decode() for k, v in KEY.findall(m.group(1))}
                if 'n' in kv and 's' in kv:
                    self.state = kv
                    ph = kv.get('ph', '')
                    self.frames.append((time.perf_counter(), int(kv['n']), ph))
                    if ph == 'sound' and not self.dropped:      # the two sounds: new since the level loaded
                        self.dropped = True
                        shutil.copyfile(os.path.join(HERE, 'live1.ogg'), os.path.join(SIG, 'live1.ogg'))
                        shutil.copyfile(os.path.join(HERE, 'live2.ogg'), os.path.join(SIG, 'live2.ogg'))
        time.sleep(self.poll_s)

    def bit(self, i):
        s = self.state['s'] if self.state else ''
        return s[i] if i < len(s) else '?'

    def wait_bit(self, i, want, timeout):
        """seconds until the mod reports `want` for file i, or None"""
        t0 = time.perf_counter()
        while time.perf_counter() - t0 < timeout:
            self.poll()
            if self.bit(i) == want:
                return time.perf_counter() - t0
        return None

    def running(self, timeout):
        """wait until the frame counter moves (a level with the mod is running)"""
        t0 = time.perf_counter()
        n0 = None
        while time.perf_counter() - t0 < timeout:
            self.poll()
            if self.state:
                n = int(self.state['n'])
                if n0 is None:
                    n0 = n
                elif n != n0 and self.state.get('ph') != 'done':
                    return True
        return False


def ms(xs):
    if not xs:
        return 'n/a'
    s = sorted(xs)
    return 'median %.0f  max %.0f  (min %.0f, %d)' % (statistics.median(s) * 1000, s[-1] * 1000, s[0] * 1000, len(s))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--poll', type=float, default=2.0, help='ms between polls')
    ap.add_argument('--timeout', type=float, default=900.0, help='seconds to wait for the level to start')
    ap.add_argument('--cycles', type=int, default=8)
    args = ap.parse_args()

    install()
    print('installed the test mod in', MOD)
    print('now start any level with "HasFile Probe (test)" enabled (restart Teardown if it is not in the list)')
    p = Probe(args.poll)
    if not p.running(args.timeout):
        print('no level with the mod started: nothing measured')
        return 1
    print('the mod is running (%d files checked a frame)' % len(p.state['s']))

    out = []
    say = lambda s: (out.append(s), print(s))
    say('HasFile probe report  %s' % time.strftime('%Y-%m-%d %H:%M:%S'))
    say('at the start the mod saw: %s   (1 there, 0 not, e = the call failed; order: %s)'
        % (p.state['s'], ', '.join(c[0] for c in CHANNELS)))
    results = {}
    for i in list(range(1, len(CHANNELS))) + [0]:              # (the file that was there from the start: last)
        label, path, what = CHANNELS[i]
        ups, downs, missed = [], [], 0
        start = p.bit(i)
        order = (('0', remove), ('1', touch)) if start == '1' else (('1', touch), ('0', remove))
        for _ in range(args.cycles):
            for want, act in order:
                act(path)
                d = p.wait_bit(i, want, 1.5)
                if d is None:
                    missed += 1
                else:
                    (ups if want == '1' else downs).append(d)
            if missed >= 2 and not ups:
                break
        results[label] = (ups, downs, missed)
        say('')
        say('%s: %s' % (label, what))
        if start == 'e':
            say('  HasFile fails for this path (an error)')
        elif not ups:
            say('  NOT SEEN: the game never noticed the file change (it stayed "%s")' % p.bit(i))
        else:
            say('  created -> seen (ms): ' + ms(ups))
            say('  deleted -> seen (ms): ' + ms(downs))
            if missed:
                say('  %d changes were not seen within 1.5 s' % missed)

    print('file tests done; waiting for the cost and sound phases (listen for the beeps)...')
    t0 = time.perf_counter()
    while (not p.state or p.state.get('ph') != 'done') and time.perf_counter() - t0 < 120:
        p.poll()
    for _ in range(100):
        p.poll()

    say('')
    for ph, title in (('base', 'frame rate, %d HasFile calls a frame' % len(p.state['s'])), ('heavy', 'frame rate, 60 more calls a frame')):
        rows = [f for f in p.frames if f[2] == ph]
        if len(rows) > 10:
            say('%s: %.1f fps' % (title, (rows[-1][1] - rows[0][1]) / (rows[-1][0] - rows[0][0])))
    st = p.state
    say('')
    say('sounds: control handle %s; new file 1: HasFile %s, handle %s; new file 2: HasFile %s, handle %s'
        % (st.get('h0', '-'), st.get('f1', '-'), st.get('h1', '-'), st.get('f2', '-'), st.get('h2', '-')))
    say('  (whether they PLAYED only you can tell: 1 beep = control, 2 beeps = new file 1, 3 beeps = new file 2)')

    say('')
    live = [k for k, (u, d, m) in results.items() if u]
    say('Verdict: a helper can signal the game through files: %s' % (('YES, via ' + ', '.join(live)) if live else 'NO'))

    for _, path, _ in CHANNELS[1:]:
        remove(path)
    shutil.rmtree(os.path.dirname(ABS), ignore_errors=True)
    for name in ('live1.ogg', 'live2.ogg'):
        remove(os.path.join(SIG, name))
    os.makedirs(EXPORT, exist_ok=True)
    path = os.path.join(EXPORT, 'hasfileprobe_report.txt')
    with open(path, 'w', encoding='utf-8') as f:
        f.write('\n'.join(out) + '\n')
    print('\nreport written to', path)
    return 0


if __name__ == '__main__':
    sys.exit(main())
