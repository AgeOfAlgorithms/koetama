"""Watch savegame.xml and log.txt while the Save Probe mod runs, and report how fast the game puts
savegame.mod keys on disk. That decides whether a voice-chat sidecar can take the game's state (who you
hear, from where, how muffled; push-to-talk) from the savegame.

    python probes/saveprobe/watch.py            # start this first, then play any level with "Save Probe (test)" on
    python probes/saveprobe/watch.py --poll 1   # poll every 1 ms (default 2)

It backs up savegame.xml to export/ first and opens both files with full sharing (read, write, delete),
so it never blocks the game from saving or replacing them. It stops when the probe says "done" (or
Ctrl+C, or --timeout) and writes export/saveprobe_report.txt.
"""
import argparse
import ctypes
import datetime
import msvcrt
import os
import re
import shutil
import statistics
import sys
import time
from ctypes import wintypes

TD = os.environ.get('SAVEPROBE_DIR') or os.path.join(os.environ['LOCALAPPDATA'], 'Teardown')   # (the env var: offline test)
SAVE = os.path.join(TD, 'savegame.xml')
LOG = os.path.join(TD, 'log.txt')
HERE = os.path.dirname(os.path.abspath(__file__))
EXPORT = os.path.join(os.path.dirname(os.path.dirname(HERE)), 'export')

_k32 = ctypes.WinDLL('kernel32', use_last_error=True)
_k32.CreateFileW.restype = wintypes.HANDLE
_k32.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID,
                             wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
GENERIC_READ, SHARE_ALL, OPEN_EXISTING = 0x80000000, 0x1 | 0x2 | 0x4, 3
INVALID = wintypes.HANDLE(-1).value


def open_shared(path):
    """A read-only file object that lets the game write, replace or delete the file meanwhile."""
    h = _k32.CreateFileW(path, GENERIC_READ, SHARE_ALL, None, OPEN_EXISTING, 0, None)
    if h == INVALID or h is None:
        return None
    return os.fdopen(msvcrt.open_osfhandle(h, os.O_RDONLY | os.O_BINARY), 'rb')


def read_all(path):
    f = open_shared(path)
    if f is None:
        return None
    with f:
        return f.read()


class Clock:
    """Wall-clock seconds since local midnight, from the high-resolution counter."""
    def __init__(self):
        now = datetime.datetime.now()
        midnight = now.replace(hour=0, minute=0, second=0, microsecond=0)
        self.base = (now - midnight).total_seconds()
        self.p0 = time.perf_counter()

    def sod(self):
        return self.base + time.perf_counter() - self.p0


SVP = re.compile(rb'<svp>(.*?)</svp>', re.S)
KEY = re.compile(rb'<(n|t|ph)\s+value="([^"]*)"')
LOGTIME = re.compile(rb'(\d\d):(\d\d):(\d\d(?:\.\d+)?)')
LOGMARK = re.compile(rb'svp n=(\d+) t=(\d+) ph=(\w+)')


def parse_save(data):
    """(n, t_ms, ph) from the probe's keys, 'none' if they are not there, None if unreadable."""
    if not data or not data.rstrip().endswith(b'</registry>'):
        return None
    m = SVP.search(data)
    if not m:
        return 'none'
    kv = {k.decode(): v.decode() for k, v in KEY.findall(m.group(1))}
    try:
        return int(kv['n']), int(kv['t']), kv['ph']
    except (KeyError, ValueError):
        return None


def stats(xs, unit=1000.0):
    if not xs:
        return 'n/a'
    s = sorted(xs)
    p = lambda q: s[min(len(s) - 1, int(q * (len(s) - 1) + 0.5))]
    return 'median %.0f  p90 %.0f  max %.0f  (min %.0f, %d samples)' % (
        statistics.median(s) * unit, p(0.9) * unit, s[-1] * unit, s[0] * unit, len(s))


def day_diff(a, b):
    """a - b in seconds, both seconds-of-day (handles midnight)."""
    d = a - b
    if d < -43200:
        d += 86400
    elif d > 43200:
        d -= 86400
    return d


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--poll', type=float, default=2.0, help='ms between polls')
    ap.add_argument('--timeout', type=float, default=600.0, help='give up after this many seconds')
    args = ap.parse_args()

    os.makedirs(EXPORT, exist_ok=True)
    if os.path.exists(SAVE):
        bak = os.path.join(EXPORT, 'savegame_backup_%s.xml' % time.strftime('%Y%m%d_%H%M%S'))
        shutil.copy2(SAVE, bak)
        print('savegame.xml backed up to', bak)

    clock = Clock()
    last_save = read_all(SAVE)
    log_pos = os.path.getsize(LOG) if os.path.exists(LOG) else 0
    log_buf = b''

    saves = []        # (seen_sod, n, t_ms, ph) for each new probe value seen
    changes = []      # (seen_sod, parsed) for every content change of savegame.xml
    bad_reads = 0
    marks = {}        # n -> (log_sod, t_ms, ph, seen_sod)
    first_mark_line = None
    polls = 0
    started = time.perf_counter()
    last_print = 0
    done_at = None
    print('watching %s and %s - now play any level with "Save Probe (test)" enabled' % (SAVE, LOG))
    try:
        while True:
            polls += 1
            now = clock.sod()
            data = read_all(SAVE)
            if data is not None and data != last_save:
                last_save = data
                p = parse_save(data)
                changes.append((now, p))
                if p is None:
                    bad_reads += 1
                elif p != 'none' and (not saves or saves[-1][1] != p[0]):
                    saves.append((now, p[0], p[1], p[2]))
                    if p[2] == 'done' and done_at is None:
                        done_at = time.perf_counter()

            if os.path.exists(LOG):
                size = os.path.getsize(LOG)
                if size < log_pos:          # the game restarted: a new log
                    log_pos, log_buf = 0, b''
                if size > log_pos:
                    f = open_shared(LOG)
                    if f:
                        with f:
                            f.seek(log_pos)
                            chunk = f.read(size - log_pos)
                        log_pos += len(chunk)
                        log_buf += chunk
                        *lines, log_buf = log_buf.split(b'\n')
                        for line in lines:
                            mm = LOGMARK.search(line)
                            tm = LOGTIME.search(line)
                            if mm and tm:
                                first_mark_line = first_mark_line or line.strip()
                                lsod = int(tm.group(1)) * 3600 + int(tm.group(2)) * 60 + float(tm.group(3))
                                marks[int(mm.group(1))] = (lsod, int(mm.group(2)), mm.group(3).decode(), now)

            if time.perf_counter() - last_print > 1:
                last_print = time.perf_counter()
                ph = saves[-1][3] if saves else '-'
                print('\r  phase %-5s  savegame changes %5d  probe values seen %5d  log marks %4d  unreadable %d   '
                      % (ph, len(changes), len(saves), len(marks), bad_reads), end='', flush=True)
            if done_at and time.perf_counter() - done_at > 2:
                break
            if time.perf_counter() - started > args.timeout:
                print('\ntimeout')
                break
            time.sleep(args.poll / 1000.0)
    except KeyboardInterrupt:
        print('\nstopped')
    elapsed = time.perf_counter() - started
    print()
    report(saves, changes, bad_reads, marks, first_mark_line, polls, elapsed)


def report(saves, changes, bad_reads, marks, first_mark_line, polls, elapsed):
    out = []
    say = out.append
    say('Save probe report  %s' % time.strftime('%Y-%m-%d %H:%M:%S'))
    say('polls: %d in %.1f s (%.0f per second)' % (polls, elapsed, polls / max(elapsed, 1e-9)))
    say('savegame.xml content changes: %d, unreadable/partial reads: %d' % (len(changes), bad_reads))
    say('log marks found: %d%s' % (len(marks), ('  e.g. ' + first_mark_line.decode(errors='replace')[:160])
                                   if first_mark_line else '  (the Spawn trick did not reach log.txt)'))

    # anchors by game time: wall time of a write = log time of the nearest mark + game-time difference
    anchors = sorted((t, lsod) for (lsod, t, ph, seen) in marks.values())

    def wall_of(t_ms):
        if not anchors:
            return None
        best = min(anchors, key=lambda a: abs(a[0] - t_ms))
        return best[1] + (t_ms - best[0]) / 1000.0

    for ph, title in (('a', 'A: written every frame'), ('b', 'B: written at 10 Hz')):
        rows = [s for s in saves if s[3] == ph]
        say('')
        say('Phase ' + title)
        if not rows:
            say('  no probe value of this phase ever reached savegame.xml')
            continue
        frames = [m for m, v in marks.items() if v[2] == ph]
        ns = [r[1] for r in rows]
        gtime = (rows[-1][2] - rows[0][2]) / 1000.0
        if ph == 'a':
            written = ns[-1] - ns[0] + 1
            say('  frames written %d over %.1f s game time (%.0f fps), distinct values seen %d (%.0f %%)'
                % (written, gtime, written / max(gtime, 1e-9), len(ns), 100.0 * len(ns) / max(written, 1)))
        else:
            say('  writes marked in the log %d, distinct values seen %d' % (len(frames), len(ns)))
        gaps = [b[0] - a[0] for a, b in zip(rows, rows[1:])]
        say('  time between file updates (ms): ' + stats(gaps))
        lat = []
        for seen, n, t, _ in rows:
            w = marks[n][0] if n in marks else wall_of(t)
            if w is not None:
                lat.append(day_diff(seen, w))
        say('  write -> on disk -> read here (ms): ' + stats(lat))
        if lat and min(lat) < -0.005:
            say('  (negative values: the log clock and this clock disagree by that much; read the spread, not the level)')

    c_rows = [s for s in saves if s[3] == 'c']
    if c_rows:
        c_start = c_rows[0][0]
        d_rows = [s for s in saves if s[3] == 'done']
        c_end = d_rows[0][0] if d_rows else changes[-1][0]
        other = [ch for ch in changes if c_start < ch[0] < c_end]
        say('')
        say('Phase C (no writes): savegame.xml changed %d times anyway' % len(other))

    lag = [day_diff(v[3], v[0]) for v in marks.values()]
    say('')
    say('log.txt: line time -> read here (ms): ' + stats(lag))

    say('')
    say('Verdict (needs: feed >= 10 Hz with p90 <= 150 ms; push-to-talk p90 <= 80 ms):')
    b = [s for s in saves if s[3] == 'b']
    sent = len([1 for v in marks.values() if v[2] == 'b']) or 200
    b_gaps = sorted(y[0] - x[0] for x, y in zip(b, b[1:]))
    p90 = b_gaps[int(0.9 * (len(b_gaps) - 1) + 0.5)] * 1000 if b_gaps else None
    if len(b) >= 0.9 * sent and p90 is not None and p90 <= 150:
        say('  10 Hz feed: YES - %d of %d writes seen, p90 %.0f ms between updates' % (len(b), sent, p90))
    else:
        say('  10 Hz feed: NO - %d of %d writes seen%s' % (len(b), sent,
            ', p90 %.0f ms between updates' % p90 if p90 is not None else ''))
    if p90 is not None:
        say('  push-to-talk through this file: %s' % ('fine' if p90 <= 80 else
            'late by up to ~%.0f ms (the sidecar would need its own key hook, or voice activation)' % p90))
    text = '\n'.join(out)
    print(text)
    os.makedirs(EXPORT, exist_ok=True)
    path = os.path.join(EXPORT, 'saveprobe_report.txt')
    with open(path, 'w', encoding='utf-8') as f:
        f.write(text + '\n')
    print('\nreport written to', path)


if __name__ == '__main__':
    sys.exit(main())
