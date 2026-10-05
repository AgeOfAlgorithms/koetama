"""Can a helper program pass TEXT into the game (for voice: what was said, into the chat history)?
Installs the "Text Probe (test)" mod and, while a level runs, writes small files the mod reads:
prefab files (the text as a description and as a hex tag) and images (two numbers as the size).
The mod reports what it read through savegame.xml.

    python probes/textprobe/probe.py      # start this first, then play any level with "Text Probe (test)" on

About 20 s. Report: export/textprobe_report.txt. It removes the files it wrote.
"""
import argparse
import os
import re
import shutil
import statistics
import struct
import sys
import time
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), 'saveprobe'))
from watch import SAVE, read_all          # noqa: E402  (shared-read open: never blocks the game)

EXPORT = os.path.join(os.path.dirname(os.path.dirname(HERE)), 'export')
MODS = os.environ.get('HFP_MODS') or os.path.join(os.environ['USERPROFILE'], 'OneDrive', 'Documents', 'Teardown', 'mods')
if not os.path.isdir(MODS):
    MODS = os.path.join(os.environ['USERPROFILE'], 'Documents', 'Teardown', 'mods')
MOD = os.path.join(MODS, 'text probe')
SIG = os.path.join(MOD, 'sig')
UP = os.path.join(MODS, 'txp_up.xml')

TXP = re.compile(rb'<txp>(.*?)</txp>', re.S)
KEY = re.compile(rb'<(\w+)\s+value="([^"]*)"')

# (label, the text, the file's form: "prefab" = inside <prefab>, "bare" = just the body)
MESSAGES = [
    ('plain', 'hello world', 'prefab'),
    ('plain, a bare body file', 'hello again', 'bare'),
    ('punctuation', 'Open sesame! It\'s "quoted" & <tagged>, 100% sure; ok?', 'prefab'),
    ('other alphabets', 'Привет, 你好, مرحبا, Γεια σου', 'prefab'),
    ('90 characters', ('the secret door is behind the painting ' * 3)[:90], 'prefab'),
    ('300 characters', ('this is a really long message to see how much text fits ' * 6)[:300], 'prefab'),
] + [('timing %d' % i, 'line number %d' % i, 'prefab') for i in range(1, 11)]


def xml_attr(s):
    return s.replace('&', '&amp;').replace('"', '&quot;').replace('<', '&lt;').replace('>', '&gt;')


def prefab(text, form='prefab'):
    body = '<body desc="%s" tags="txp t=%s"/>' % (xml_attr(text), text.encode('utf-8').hex())
    if form == 'bare':
        return body + '\n'
    return '<prefab version="1.5.2">\n\t%s\n</prefab>\n' % body


def png(w, h):
    def chunk(tag, data):
        return struct.pack('>I', len(data)) + tag + data + struct.pack('>I', zlib.crc32(tag + data) & 0xffffffff)
    raw = b''.join(b'\x00' + b'\x80\x80\x80' * w for _ in range(h))
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b''))


def put(path, data):
    """the file appears complete, never half-written"""
    tmp = path + '.tmp'
    with open(tmp, 'wb') as f:
        f.write(data if isinstance(data, bytes) else data.encode('utf-8'))
    os.replace(tmp, path)


def install():
    if os.path.isdir(MOD):
        shutil.rmtree(MOD)
    shutil.copytree(os.path.join(HERE, 'mod'), MOD)
    os.makedirs(SIG, exist_ok=True)
    if os.path.exists(UP):
        os.remove(UP)


class Game:
    def __init__(self, poll_ms=2.0):
        self.poll_s = poll_ms / 1000.0
        self.state, self.last = None, None
        self.frames = []

    def poll(self):
        data = read_all(SAVE)
        if data is not None and data != self.last and data.rstrip().endswith(b'</registry>'):
            self.last = data
            m = TXP.search(data)
            if m:
                kv = {k.decode(): v.decode() for k, v in KEY.findall(m.group(1))}
                if 'n' in kv:
                    if not self.frames or self.frames[-1][1] != int(kv['n']):
                        self.frames.append((time.perf_counter(), int(kv['n'])))
                    self.state = kv
        time.sleep(self.poll_s)

    def wait(self, cond, timeout):
        t0 = time.perf_counter()
        while time.perf_counter() - t0 < timeout:
            self.poll()
            if self.state and cond(self.state):
                return time.perf_counter() - t0
        return None


def decode(r):
    """the mod's 'ok:<n>:<types>:<hex desc>:<tag>' -> (entities, types, description, tag text) or the error"""
    if r.startswith('err:'):
        return None, bytes.fromhex(r[4:]).decode('utf-8', 'replace'), None, None
    _, n, types, hdesc, tag = r.split(':')
    try:
        tagtext = bytes.fromhex(tag).decode('utf-8', 'replace')
    except ValueError:
        tagtext = '(not hex: %s)' % tag
    return int(n), types, bytes.fromhex(hdesc).decode('utf-8', 'replace'), tagtext


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--timeout', type=float, default=900.0, help='seconds to wait for the level to start')
    args = ap.parse_args()
    install()
    print('test mod installed in', MOD)
    print('now start any level with "Text Probe (test)" enabled (restart Teardown if it is not in the list)')
    g = Game()
    n0 = [None]

    def fresh(st):                 # (a level started now, not what an earlier session left in the savegame)
        if n0[0] is None:
            n0[0] = st['n']
        return st['n'] != n0[0] and 'r1' not in st
    if g.wait(fresh, args.timeout) is None:
        print('no level with the mod started: nothing measured')
        return 1
    out = []

    def say(s):
        out.append(s)
        print(s)
    say('Text probe report  %s' % time.strftime('%Y-%m-%d %H:%M:%S'))
    say('Lua file functions in a script (dofile, loadfile, loadstring, io, os, require): %s' % g.state.get('g', '?'))
    say('')
    say('A. text in a prefab file, written while the level runs (description / hex tag):')
    lat, good_desc, good_tag = [], 0, 0
    for k, (label, text, form) in enumerate(MESSAGES, 1):
        f0 = len(g.frames)
        put(os.path.join(SIG, 'm%d.xml' % k), prefab(text, form))
        d = g.wait(lambda st: ('r%d' % k) in st, 2.0)
        if d is None:
            say('  %-26s NOT READ (the mod never reported it)' % label)
            continue
        n, types, desc, tag = decode(g.state['r%d' % k])
        if n is None:
            say('  %-26s Spawn failed: %s' % (label, types))
            continue
        ok_d, ok_t = desc == text, tag == text
        good_desc += ok_d
        good_tag += ok_t
        if label.startswith('timing'):
            lat.append(d)
        fr = g.frames[f0:]
        worst = max((b[0] - a[0] for a, b in zip(fr, fr[1:])), default=0) * 1000
        say('  %-26s description %s  tag %s   %3.0f ms  (%d entities: %s; worst frame %.0f ms)' % (
            label, 'OK ' if ok_d else 'NO ', 'OK ' if ok_t else 'NO ', d * 1000, n, types, worst))
        if not ok_d:
            say('      description read: %r' % desc[:120])
        if not ok_t:
            say('      tag read: %r' % tag[:120])
    if lat:
        say('  written -> read by the game -> seen here: median %.0f ms, max %.0f ms' % (statistics.median(lat) * 1000, max(lat) * 1000))

    say('')
    put(UP, prefab('one folder up'))
    d = g.wait(lambda st: 'rup' in st, 2.0)
    if d is None:
        say('a file one folder up (MOD/../): NOT READ')
    else:
        n, types, desc, tag = decode(g.state['rup'])
        say('a file one folder up (MOD/../): %s' % ('OK' if desc == 'one folder up' or tag == 'one folder up' else 'read, but: %r / %r / %r' % (types, desc, tag)))

    put(os.path.join(SIG, 'same.xml'), prefab('first version'))
    if g.wait(lambda st: 'rs1' in st, 2.0) is not None:
        put(os.path.join(SIG, 'same.xml'), prefab('second version'))
        put(os.path.join(SIG, 'same2.go'), 'x')
        if g.wait(lambda st: 'rs2' in st, 2.0) is not None:
            a, b = decode(g.state['rs1']), decode(g.state['rs2'])
            first = a[2] or a[3]
            second = b[2] or b[3]
            say('the same file name written again: first read %r, then %r -> %s' % (
                first, second, 'read fresh each time' if second == 'second version' else 'REMEMBERED (a new name is needed per message)'))
        else:
            say('the same file name written again: the second read never came')
    else:
        say('same.xml: NOT READ')

    say('')
    say('B. numbers as an image size (UiGetImageSize):')
    put(os.path.join(SIG, 'img1.png'), png(37, 53))
    d = g.wait(lambda st: 'i1' in st, 2.0)
    say('  a 37 x 53 image written while the level runs: %s' % (g.state.get('i1') if d is not None else 'NOT READ'))
    put(os.path.join(SIG, 'imgsame.png'), png(41, 59))
    if g.wait(lambda st: 'is1' in st, 2.0) is not None:
        put(os.path.join(SIG, 'imgsame.png'), png(43, 61))
        put(os.path.join(SIG, 'img2.go'), 'x')
        d = g.wait(lambda st: 'is2' in st, 2.0)
        say('  the same name, 41 x 59 then 43 x 61: %s then %s' % (g.state.get('is1'), g.state.get('is2') if d is not None else '(no second read)'))
    else:
        say('  imgsame.png: NOT READ')

    say('')
    total = len(MESSAGES)
    say('Verdict: text through a prefab file: description %d of %d, hex tag %d of %d' % (good_desc, total, good_tag, total))
    for name in os.listdir(SIG):
        try:
            os.remove(os.path.join(SIG, name))
        except OSError:
            pass
    if os.path.exists(UP):
        os.remove(UP)
    os.makedirs(EXPORT, exist_ok=True)
    path = os.path.join(EXPORT, 'textprobe_report.txt')
    with open(path, 'w', encoding='utf-8') as f:
        f.write('\n'.join(out) + '\n')
    print('\nreport written to', path, '- you can quit the level')
    return 0


if __name__ == '__main__':
    sys.exit(main())
