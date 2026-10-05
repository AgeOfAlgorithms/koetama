"""End to end without the game or a microphone: a fake game (it writes the feed into a temporary savegame.xml,
reads the helper's message files in order and acks them, as voice.lua does) and the real helper with a
recording as its microphone (--mic-wav, real time). The recording: four benchmark lines and two one-word
callouts with pauses (export/asrbench, from bench/make_clips.py).

    C:/Users/user/miniconda3/envs/pcvoice/python.exe engine/test_e2e.py
"""
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import wave

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(os.path.dirname(HERE), 'export', 'asrbench')
XML = ('<registry version="2.1.0">\n<savegame><mod><local-proximity-chat>\n<pcvx>\n<f value="%s"/>\n</pcvx>\n'
       '</local-proximity-chat></mod></savegame>\n</registry>\n')
LINES = ['en01', 'en02', 'en05', 'en07']
FAILED = NCHECK = 0


def check(cond, msg):
    global FAILED, NCHECK
    NCHECK += 1
    if not cond:
        FAILED += 1
    print(('ok   ' if cond else 'FAIL ') + msg, flush=True)


def recording(path):
    clips = {c['id']: c for c in json.load(open(os.path.join(OUT, 'clips.json'), encoding='utf-8'))}
    parts, refs = [np.zeros(16000, np.float32)], []
    for cid in LINES:
        c = clips[cid]
        with wave.open(os.path.join(OUT, 'clips', '%s_room.wav' % cid), 'rb') as w:
            x = np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(np.float32) / 32768
        parts += [x[int((c['speech_start'] - 0.05) * 16000):int((c['speech_end'] + 0.05) * 16000)], np.zeros(int(16000 * 1.5), np.float32)]
        refs.append(c['text'])
    y = np.concatenate(parts + [np.zeros(16000 * 2, np.float32)])
    with wave.open(path, 'wb') as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(16000)
        w.writeframes((np.clip(y, -1, 1) * 32767).astype(np.int16).tobytes())
    return refs, len(y) / 16000


def words(t):
    return re.sub(r'[^\w ]', '', t.lower()).split()


def main():
    with tempfile.TemporaryDirectory() as td, tempfile.TemporaryDirectory() as mods:
        wav = os.path.join(td, 'mic.wav')
        refs, dur = recording(wav)
        save = os.path.join(td, 'savegame.xml')
        state = dict(seq=0, ack=0, ping=1)

        def write():
            state['seq'] += 1
            feed = '3|%d|1.00|11|%d|%d|1|en|' % (state['seq'], state['ack'], state['ping'])
            for _ in range(50):
                try:
                    with open(save, 'w') as f:
                        f.write(XML % feed)
                    return
                except PermissionError:
                    time.sleep(0.001)
        write()
        env = dict(os.environ, SAVEPROBE_DIR=td, HFP_MODS=mods, HF_HUB_DISABLE_SYMLINKS_WARNING='1', PYTHONIOENCODING='utf-8')
        p = subprocess.Popen([sys.executable, '-u', os.path.join(HERE, 'teardown_helper.py'), '--volume', '0', '--mic-wav', wav,
                              '--seconds', str(int(dur + 40))], env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                             text=True, encoding='utf-8', errors='replace')
        msgs = []                                   # (time, kind, utt, text)
        t0 = time.time()
        first = None
        while time.time() - t0 < dur + 38:
            write()
            if time.time() - t0 > 2 * (state['ping']):
                state['ping'] += 1
            f = os.path.join(mods, 'pcvx_t%d.xml' % (state['ack'] + 1))
            if os.path.exists(f):
                try:
                    body = open(f, encoding='utf-8').read()
                    m = re.search(r'k=(\w) u=(\d+) t=([0-9a-f]*)"', body)
                    msgs.append((time.time() - t0, m.group(1), int(m.group(2)), bytes.fromhex(m.group(3)).decode()))
                    state['ack'] += 1
                    if first is None:
                        first = time.time() - t0
                except (OSError, AttributeError):
                    pass
            finals = [x for x in msgs if x[1] == 'f']
            if len(finals) >= len(refs) and time.time() - t0 > 5:
                break
            time.sleep(0.05)
        p.kill()
        out = p.communicate()[0]
    print('helper said:')
    for line in out.replace('\r', '\n').split('\n'):
        if line.strip() and ('you said' in line or 'speech model' in line or 'microphone' in line or 'loading' in line or 'Error' in line or 'Traceback' in line):
            print('   ' + line.strip()[:150])
    finals = [x for x in msgs if x[1] == 'f']
    lives = [x for x in msgs if x[1] == 'l']
    print('messages: %d live, %d finished' % (len(lives), len(finals)))
    for x in finals:
        print('   %5.1f s  utt %d: %s' % (x[0], x[2], x[3]))
    check(len(finals) == len(refs), 'one finished line per spoken line (%d of %d)' % (len(finals), len(refs)))
    utts = {x[2] for x in finals}
    check(all(any(l[2] == u and l[0] < f[0] for l in lives) for u in utts for f in finals if f[2] == u),
          'live words came before each finished line')
    errs = sum(len(set(words(r)) ^ set(words(h[3]))) for r, h in zip(refs, finals))
    check(errs <= 3, 'the lines are right (%d words differ)' % errs)
    order = [x[2] for x in msgs]
    check(order == sorted(order), 'the messages come in utterance order')
    print('\n%d checks, %d failed' % (NCHECK, FAILED))
    return 1 if FAILED else 0


if __name__ == '__main__':
    sys.exit(main())
