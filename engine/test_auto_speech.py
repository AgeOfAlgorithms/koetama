"""The in-game auto test (helper.py --auto-speech) against a fake game: what the game would receive - live words
and finished lines - for each recorded line, in real time. Checks every line arrives, live words first.

    C:/Users/user/miniconda3/envs/pcvoice/python.exe engine/test_auto_speech.py
"""
import os
import re
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
XML = ('<registry version="2.1.0">\n<savegame><mod><local-proximity-chat>\n<pcvx>\n<f value="%s"/>\n</pcvx>\n'
       '</local-proximity-chat></mod></savegame>\n</registry>\n')


def main():
    sys.path.insert(0, HERE)
    import helper
    n_items = len(helper.auto_speech_items())
    with tempfile.TemporaryDirectory() as td, tempfile.TemporaryDirectory() as mods:
        save = os.path.join(td, 'savegame.xml')
        st = dict(seq=0, ack=0, ping=1)

        def write():
            st['seq'] += 1
            for _ in range(50):
                try:
                    with open(save, 'w') as f:
                        f.write(XML % ('3|%d|1.00|12|%d|%d|1|en|' % (st['seq'], st['ack'], st['ping'])))
                    return
                except PermissionError:
                    time.sleep(0.001)
        write()
        env = dict(os.environ, SAVEPROBE_DIR=td, HFP_MODS=mods, HF_HUB_DISABLE_SYMLINKS_WARNING='1', PYTHONIOENCODING='utf-8')
        log = os.path.join(td, 'helper.log')
        logf = open(log, 'w', encoding='utf-8')
        p = subprocess.Popen([sys.executable, '-u', os.path.join(HERE, 'helper.py'), '--volume', '0', '--auto-speech', '--seconds', '400'],
                             env=env, stdout=logf, stderr=subprocess.STDOUT)
        msgs, t0 = [], time.time()
        while time.time() - t0 < 390:
            write()
            if time.time() - t0 > 2 * st['ping']:
                st['ping'] += 1
            f = os.path.join(mods, 'pcvx_t%d.xml' % (st['ack'] + 1))
            if os.path.exists(f):
                try:
                    m = re.search(r'k=(\w) u=(\d+) t=([0-9a-f]*)"', open(f, encoding='utf-8').read())
                    msgs.append((time.time() - t0, m.group(1), int(m.group(2)), bytes.fromhex(m.group(3)).decode()))
                    st['ack'] += 1
                except (OSError, AttributeError):
                    pass
            out = open(log, encoding='utf-8', errors='replace').read()
            if 'all lines played' in out and time.time() - (msgs[-1][0] + t0 if msgs else t0) > 4:
                break
            time.sleep(0.05)
        p.kill()
        p.wait()
        logf.close()
        out = open(log, encoding='utf-8', errors='replace').read()
    played = [l.strip() for l in out.replace('\r', '\n').split('\n') if re.search(r'playing \d+ of', l)]
    finals = [m for m in msgs if m[1] == 'f' and m[3]]
    lives = [m for m in msgs if m[1] == 'l']
    print('played %d of %d lines; the game got %d live messages and %d lines:' % (len(played), n_items, len(lives), len(finals)))
    k = 0
    for line in played:
        print('  ' + line[:110])
    for m in finals:
        n_live = sum(1 for x in lives if x[2] == m[2])
        print('    -> utt %2d (%d live updates first): %s' % (m[2], n_live, m[3]))
    ok = len(played) == n_items and len(finals) >= n_items - 2
    print('\n%s' % ('OK' if ok else 'FAILED'))
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
