"""The Python Listener's answers on real clips, for kd-speech's end-to-end tests (app/crates/kd-speech/tests/listener.rs):
each clip fed as teardown_helper.py --transcribe does (1 s of quiet after, 800-sample blocks, synchronous feed + flush),
every callback written down. Live words depend only on the audio (a pass every Models.every s of speech), so the port
must give the same lines. Also the models' load times and a pass, for comparing speeds.

    <conda>/envs/pcvoice/python.exe app/fixtures/make_speech_fixtures.py      # -> speech.json (needs the models)
"""
import json
import os
import sys
import time

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, os.path.join(ROOT, 'engine'))
import asr                                  # noqa: E402
from audio import read_wav                  # noqa: E402

CLIPS = os.path.join(ROOT, 'export', 'asrbench', 'clips')
LID = os.path.join(ROOT, 'export', 'asrbench', 'lid')


def clip(name):
    x, sr = read_wav(os.path.join(CLIPS, name + '.wav'))
    assert sr == asr.RATE
    return x


def run(models, lang, x):
    starts, lives, finals = [], [], []
    lst = asr.Listener(lambda u, t, i: lives.append(dict(utt=u, text=t, times=list(i['times']))),
                       lambda u, t, i: finals.append(dict(utt=u, text=t, used=i['used'], lang=i['lang'], times=list(i['times']),
                                                          speech=i['speech'], passes=i['passes'], live=i['live'])),
                       log=lambda s: None, models=models, on_start=starts.append)
    lst.set_language(lang)
    lst.warm()
    audio = np.concatenate([x, np.zeros(asr.RATE, np.float32)])
    for k in range(0, len(audio), 800):
        lst.feed(audio[k:k + 800])
    lst.flush()
    return dict(starts=starts, lives=lives, finals=finals, every=models.every if hasattr(models, 'every') else asr.ROLL_EVERY)


def main():
    models = asr.Models(threads=4, log=lambda m: None)
    load = {}
    for name in ('parakeet', 'gigaam', 'sensevoice', 'langid'):
        t0 = time.perf_counter()
        models.get(name)
        load[name] = round(time.perf_counter() - t0, 2)
    meta = {c['id']: c for c in json.load(open(os.path.join(ROOT, 'export', 'asrbench', 'clips.json'), encoding='utf-8'))}
    en04, en02 = clip('en04_room'), clip('en02_room')
    a = en04[:int((meta['en04']['speech_end'] + 0.1) * asr.RATE)]
    b = en02[int((meta['en02']['speech_start'] - 0.1) * asr.RATE):]
    mixed08, _ = read_wav(os.path.join(LID, 'mixed_08.wav'))
    cases = [
        dict(name='en01', lang='en', parts=[['clips/en01_room.wav', 0.0, None]], x=clip('en01_room')),
        dict(name='en04+en02 (one long line)', lang='en',
             parts=[['clips/en04_room.wav', 0.0, meta['en04']['speech_end'] + 0.1], ['clips/en02_room.wav', meta['en02']['speech_start'] - 0.1, None]],
             x=np.concatenate([a, b])),
        dict(name='ru02', lang='ru', parts=[['clips/ru02_room.wav', 0.0, None]], x=clip('ru02_room')),
        dict(name='zh03', lang='zh', parts=[['clips/zh03_room.wav', 0.0, None]], x=clip('zh03_room')),
        dict(name='mixed_08 auto', lang='auto', parts=[['lid/mixed_08.wav', 0.0, None]], x=mixed08),
    ]
    out = []
    for c in cases:
        models.every = asr.ROLL_EVERY
        t0 = time.perf_counter()
        r = run(models, c['lang'], c['x'])
        r.update(name=c['name'], lang=c['lang'], parts=c['parts'], took=round(time.perf_counter() - t0, 2),
                 dur=round(len(c['x']) / asr.RATE + 1.0, 2))
        out.append(r)
        print(c['name'], r['finals'], len(r['lives']), 'lives', r['took'], 's')
    # one pass's cost: a 6 s line through Parakeet (the live words' pass and the final pass are the same thing)
    x = cases[1]['x']
    models.offline_full('parakeet', x)
    t0 = time.perf_counter()
    for _ in range(3):
        models.offline_full('parakeet', x)
    pass_s = (time.perf_counter() - t0) / 3
    t0 = time.perf_counter()
    for k in range(10):
        asr.lid_probs(models, x[k * 1600:k * 1600 + 16000])
    lid_s = (time.perf_counter() - t0) / 10
    timing = dict(load=load, parakeet_pass_6s=round(pass_s, 3), lid_window=round(lid_s, 4), audio_s=round(len(x) / asr.RATE, 2))
    print(timing)
    path = os.path.join(HERE, 'speech.json')
    with open(path, 'w', encoding='utf-8') as f:
        json.dump(dict(cases=out, timing=timing), f, ensure_ascii=False, indent=1)
    print('wrote', path)


if __name__ == '__main__':
    main()
