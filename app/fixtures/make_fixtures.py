"""The Python engine's answers, written down for the Rust port's tests (app/): every piece the port must do the same
way gets cases here, run through the Python code (engine/) - the reference. Run it again after a change on the Python
side; the Rust tests read the JSON files next to this script.

    <conda>/envs/pcvoice/python.exe app/fixtures/make_fixtures.py          # text, audio, feed, link, segments
    <conda>/envs/pcvoice/python.exe app/fixtures/make_fixtures.py --real   # + the real models on benchmark clips

Files: text.json (units, unit_key, tidy, unit_times, LocalAgreement), audio.json (the low-pass, panning, the mixer),
feed.json (the feed, the savegame, the message files), link.json (the files Kotodama writes for the game, step by
step), segments.json (the language stitching with a made-up detector), real.json (models: transcripts, unit times,
detector outputs, stitched mixed-language lines).
"""
import json
import math
import os
import sys
import tempfile

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, os.path.join(ROOT, 'engine'))
import asr                                  # noqa: E402
import audio                                # noqa: E402
from games import teardown as td            # noqa: E402

BENCH = os.path.join(ROOT, 'export', 'asrbench')


def dump(name, obj):
    path = os.path.join(HERE, name)
    with open(path, 'w', encoding='utf-8') as f:
        json.dump(obj, f, ensure_ascii=False, indent=None, separators=(',', ':'))
    print('wrote %s (%.0f KB)' % (name, os.path.getsize(path) / 1024))


def floats(a, nd=7):
    return [round(float(v), nd) for v in np.asarray(a).reshape(-1)]


# ---------------------------------------------------------------- text
def text_cases():
    texts = ['I really like eating apples', 'Hello, world!  ok', '我们需要 a lever 才能打开这扇门。', 'すみません、ロープ',
             '한국어 말', 'Привет, мир', '', '  ', 'Run, 它就在!', 'tab\there\nnew line', 'ａｂｃ full width',
             'mixed日本語text', '¿Dónde está? Ça va, naïve café', 'x　y', 'emoji 🙂 ok', "don't stop-now"]
    units = [dict(text=t, units=[[i, u] for i, u in asr.units(t)]) for t in texts]
    keys = ['Hello,', 'world!', "don't", '它', '！', 'Привет,', 'Ça', 'naïve', '¿Dónde', '_under_', '123.', '…', 'a-b', 'ＡＢ']
    unit_key = [dict(u=u, key=asr.unit_key(u)) for u in keys]
    tidies = ['  hello   world ', 'привет мир', '...', '', 'Already Capital', '它在哪', 'élan vital', '  a', '1 two',
              'ñandú', '\tmixed\nspace  ']
    tidy = [dict(text=t, out=asr.tidy(t)) for t in tidies]
    ut = [
        dict(text='I really like', tokens=[' I', ' really', ' like'], stamps=[0.1, 0.4, 0.9], offset=0.0, dur=None),
        dict(text='I really like', tokens=['▁I', '▁re', 'ally', '▁like'], stamps=[0.1, 0.4, 0.6, 0.9], offset=0.0, dur=None),
        dict(text='我们需', tokens=['我', '们', '需'], stamps=[0.2, 0.3, 0.5], offset=1.0, dur=None),
        dict(text='Hello world', tokens=['hello', ' world'], stamps=[0.3, 0.8], offset=0.0, dur=None),
        dict(text='a b c', tokens=[], stamps=[], offset=0.0, dur=3.0),
        dict(text='a b c', tokens=['x'], stamps=[], offset=0.5, dur=1.5),
        dict(text='Hello. Can anyone hear me?', tokens=[' H', 'ello', '.', ' Can', ' any', 'one', ' hear', ' me', '?'],
             stamps=[0.32, 0.64, 1.04, 1.28, 1.68, 1.92, 2.2, 2.5, 2.7], offset=0.25, dur=None),
        dict(text='Totally different words', tokens=[' zzz', ' qqq'], stamps=[0.5, 1.0], offset=0.0, dur=None),
        dict(text='', tokens=[' a'], stamps=[0.1], offset=0.0, dur=None),
    ]
    for c in ut:
        c['out'] = asr.unit_times(c['text'], c['tokens'], c['stamps'], c['offset'], c['dur'])
    # LocalAgreement: a sequence of passes -> what is shown after each
    seqs = [
        [('I really like', [0.1, 0.4, 0.9]), ('I really like eating', [0.1, 0.4, 0.9, 1.3]),
         ('I really liked eating apples', [0.1, 0.4, 0.9, 1.3, 1.8]),
         ('I really liked eating apples now', [0.1, 0.4, 0.9, 1.3, 1.8, 2.4])],
        [('我们需要', [0.1, 0.2, 0.3, 0.4]), ('我们需要拉杆', [0.1, 0.2, 0.3, 0.4, 0.6, 0.7]),
         ('我们需要拉杆。Go', [0.1, 0.2, 0.3, 0.4, 0.6, 0.7, 0.8, 1.0]), ('我们需要拉杆。Go now', [0.1, 0.2, 0.3, 0.4, 0.6, 0.7, 0.8, 1.0, 1.2])],
        [('Run, run', [0.1, 0.5]), ('Run, run away', [0.1, 0.5, 0.8]), ('Run run away now', [0.1, 0.5, 0.8, 1.1]),
         ('Wrong length', [0.1])],
        [('Hello', [0.2]), ('Hello', [0.2]), ('Hello there', [0.2, 0.6]), ('Hello there friend', [0.2, 0.6, 1.0])],
        [('a.b', [0.0]), ('a.b c', [0.0, 0.3]), ('a.b c d', [0.0, 0.3, 0.6])],
    ]
    commit = []
    for seq in seqs:
        line = asr.RollingLine.__new__(asr.RollingLine)
        line.committed, line.prev = [], None
        steps = []
        for text, times in seq:
            grew = line._commit(text, times)
            steps.append(dict(text=text, times=times, grew=grew,
                              committed=[[sep, u, t] for sep, u, t in line.committed]))
        commit.append(steps)
    dump('text.json', dict(units=units, unit_key=unit_key, tidy=tidy, unit_times=ut, commit=commit,
                           wide=[list(r) for r in asr.WIDE]))


# ---------------------------------------------------------------- audio
class Clock:
    def __init__(self):
        self.t = 0.0

    def __call__(self):
        return self.t


def audio_cases():
    ir = [dict(a=a, h=floats(audio.lowpass_ir(a), 9)) for a in (0.02, 0.05, 0.2, 0.5, 0.9, 0.999)]
    rng = np.random.default_rng(7)
    hist = np.zeros(audio.LP_TAPS, np.float32)
    blocks = []
    for k, a in enumerate((0.9, 0.9, 0.1, 0.05, 0.5, 0.02)):
        x = rng.standard_normal(480).astype(np.float32) * 0.3
        y, hist = audio.lowpass(x, hist, a)
        blocks.append(dict(a=a, x=floats(x), y=floats(y)))
    pan = [dict(az=az, el=el, lr=list(audio.pan_gains(az, el)), behind=audio.behind(az))
           for az in (-180, -135, -90, -30, 0, 30, 90, 120, 180) for el in (0, 45)]
    # the mixer: two short clips, a scripted run of feeds and renders on a fake clock
    t = np.arange(4800) / audio.RATE
    clips = {1: (0.4 * np.sin(2 * math.pi * 220 * t)).astype(np.float32),
             2: (0.3 * np.sign(np.sin(2 * math.pi * 330 * t))).astype(np.float32)}
    clock = Clock()
    m = audio.Mixer(clips, clock=clock)
    m.volume = 0.8

    def feed(vol, sp):
        return dict(seq=0, vol=vol, speakers={k: dict(src=v[0], talk=v[1], gain=v[2], az=v[3], el=v[4], muffle=v[5])
                                              for k, v in sp.items()})
    script = [
        ('feed', feed(1.0, {7: (1, True, 1.0, 0.0, 0.0, 0.0)})), ('render', 480), ('render', 480),
        ('feed', feed(0.9, {7: (1, True, 0.6, 60.0, 10.0, 0.3), 8: (2, True, 0.5, -120.0, 0.0, 0.8)})),
        ('render', 480), ('render', 512), ('render', 480),
        ('feed', feed(0.9, {7: (1, False, 0.6, 60.0, 10.0, 0.3), 8: (2, True, 0.5, 170.0, 0.0, 1.0)})),
        ('render', 480), ('render', 480), ('render', 1024),
        ('feed', feed(1.0, {8: (2, True, 1.0, 0.0, 0.0, 0.0)})), ('render', 480),
        ('feed', feed(1.0, {7: (1, True, 1.0, 0.0, 0.0, 0.0), 8: (2, True, 1.0, 0.0, 0.0, 0.0)})), ('render', 480),
        ('wait', 2.0), ('render', 480), ('render', 480),
    ]
    steps = []
    for op, arg in script:
        if op == 'feed':
            m.set_feed(arg)
            steps.append(dict(op='feed', feed=dict(vol=arg['vol'], speakers={str(k): v for k, v in arg['speakers'].items()})))
        elif op == 'wait':
            clock.t += arg
            steps.append(dict(op='wait', seconds=arg))
        else:
            out = m.render(arg)
            clock.t += arg / audio.RATE
            steps.append(dict(op='render', frames=arg, out=floats(out, 6),
                              pos={str(k): v.pos for k, v in m.voices.items()}))
    mixer = dict(clips={str(k): floats(v) for k, v in clips.items()}, volume=0.8, steps=steps)
    consts = dict(RATE=audio.RATE, SMOOTH=audio.SMOOTH, STALE=audio.STALE, CUT_CLEAR=audio.CUT_CLEAR,
                  CUT_MUFFLED=audio.CUT_MUFFLED, PAN=audio.PAN, BEHIND_MUFFLE=audio.BEHIND_MUFFLE,
                  BEHIND_QUIET=audio.BEHIND_QUIET, HEADROOM=audio.HEADROOM, LP_TAPS=audio.LP_TAPS)
    # load_wav's level: the 99.9th percentile (numpy's linear interpolation)
    x = rng.standard_normal(9001).astype(np.float32)
    pct = dict(x=floats(x), p999=float(np.percentile(np.abs(x), 99.9)))
    dump('audio.json', dict(consts=consts, lowpass_ir=ir, lowpass=blocks, pan=pan, mixer=mixer, percentile=pct))


# ---------------------------------------------------------------- the feed and the message files
def feed_cases():
    feeds = ['4|12|0.80|5|3|7|1|ru|0|1,2,1,0.5,-30.0,5.0,0.25;9,3,0,1,180,-10,1',
             '4|1|1.00|5|0|1|0|auto|1|', '3|99|1|11|4|2|1|en|', '2|1|0.5|3|0|1|1|7,1,1,1,0,0,0',
             '4|1|1|5|0|1|0||1|', '4|x|1|5|0|1|0|en|1|', '5|1|1|1|1|1|1|en|1|', '4|1|1|5|0|1|0|en|1|1,2,3', '',
             '3|1|1|1|1|1|1|en|1,1,1,nan,0,0,0', '4|1|1|5|0|1|0|en|1|;;3,1,1,1,0,0,0;']
    parse = []
    for f in feeds:
        p = td.parse_feed(f)
        if p is not None:
            p['speakers'] = {str(k): v for k, v in p['speakers'].items()}
        parse.append(dict(text=f, feed=p))
    xml = ('<registry version="2.1.0">\n<savegame><mod><local-proximity-chat>\n<pcvx>\n<f value="4|1|1|5|0|1|1|en|1|"/>\n'
           '</pcvx>\n</local-proximity-chat><steam-3812301496><other value="1"/><pcvx> <f   value="4|2|0.5|6|1|2|0|ru|0|1,1,1,1,0,0,0"/>\n'
           '</pcvx></steam-3812301496></mod></savegame>\n</registry>\n')
    finds = [dict(xml=xml, feeds=[list(x) for x in td.find_feeds(xml.encode())]),
             dict(xml='<registry><pcvx><f value="3|1|1|1|1|1|1|en|"/></pcvx></registry>',
                  feeds=[list(x) for x in td.find_feeds(b'<registry><pcvx><f value="3|1|1|1|1|1|1|en|"/></pcvx></registry>')])]
    prefabs = [dict(text=t, kind=k, utt=u, times=tm, ago=ago, out=td.text_prefab(t, k, u, tm, ago))
               for t, k, u, tm, ago in [('Hello there', 'f', 3, None, None), ('', 's', 4, None, None),
                                        ('Привет 我们', 'l', 5, [0.0, 0.5, 0.75, 1.0], 1.234),
                                        ('x', 'f', 0, [700.0], 0.004), ('a b', 'l', 1, [-1.0, 0.126], -2.0)]]
    hexes = [dict(times=t, out=td.times_hex(t)) for t in ([], [0.0, 0.01, 1.5, 655.35, 700.0, -3.0], [0.005, 0.015, 0.025])]
    dump('feed.json', dict(parse=parse, find=finds, prefab=prefabs, times_hex=hexes, TEXT_MAX=td.TEXT_MAX))


def link_cases():
    """the files the Link leaves after each step (times without word times: those depend on the clock)"""
    with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
        open(os.path.join(a, 'pcvx_t9.xml'), 'w').write('old')
        open(os.path.join(a, 'other.txt'), 'w').write('keep')
        link = td.Link([a, b], log=lambda m: None)

        def files():
            out = {}
            for name, d in (('local', a), ('workshop', b)):
                out[name] = {f: open(os.path.join(d, f), encoding='utf-8').read() for f in sorted(os.listdir(d))}
            return out

        def feed(sid, ack, ping, mic=True, lang='en', live=True):
            return dict(seq=1, vol=1.0, sid=sid, ack=ack, ping=ping, mic=mic, lang=lang, live=live, speakers={})
        steps = []

        def step(op, **kw):
            r = None
            if op == 'start':
                link.start()
            elif op == 'stop':
                link.stop()
            elif op == 'feed':
                link.on_feed(feed(*kw['args']), kw['tag'])
            elif op == 'send':
                r = link.send_msg(kw['kind'], kw['utt'], kw['text'])
            elif op == 'send_text':
                r = link.send_text(kw['text'])
            steps.append(dict(op=op, args=kw, result=r, files=files(), mic=link.mic, lang=link.lang, live=link.live))
        step('send', kind='f', utt=1, text='nobody listens')
        step('start')
        step('feed', args=[11, 0, 1], tag='local-proximity-chat')
        step('send', kind='s', utt=1, text='')
        step('send', kind='l', utt=1, text='')
        step('send', kind='l', utt=1, text='  hello  ')
        step('send', kind='f', utt=1, text='hello there ' + 'x' * 500)
        step('feed', args=[11, 2, 2, False, 'ru', False], tag='local-proximity-chat')
        step('feed', args=[11, 3, 1002], tag='local-proximity-chat')
        step('send_text', text='typed line')
        step('feed', args=[12, 7, 5], tag='steam-3812301496')
        step('send', kind='f', utt=2, text='to the workshop copy')
        step('feed', args=[12, 7, 5], tag='steam-3812301496')
        step('stop')
    dump('link.json', dict(steps=steps))


# ---------------------------------------------------------------- language stitching, with a made-up detector
def fake_probs(x, langs=None):
    """a stand-in detector: language k is how much of the window's energy sits on every 10th sample from k (the test
    audio puts a "language" there). -> probabilities over the 10 MIXED_LANGS (or over langs, some of them: the real
    detector's softmax over just their logits)"""
    x = np.asarray(x, np.float64)
    if len(x) < 1600:
        x = np.concatenate([x, np.zeros(1600 - len(x))])
    e = np.array([np.mean(x[k::10] ** 2) for k in range(10)]) + 1e-12
    z = np.log(e) * 3.0
    if langs is not None:
        z = z[[asr.MIXED_LANGS.index(l) for l in langs]]
    p = np.exp(z - z.max())
    return p / p.sum()


def voice(lang, dur, rng, level=0.3):
    """noise whose energy sits on every 10th sample from lang (fake_probs reads that as language lang)"""
    n = int(dur * asr.RATE)
    x = rng.standard_normal(n) * level * 0.15
    x[lang::10] += rng.standard_normal(len(x[lang::10])) * level
    return x.astype(np.float32)


def quiet(dur, rng):
    return (rng.standard_normal(int(dur * asr.RATE)) * 0.0005).astype(np.float32)


def segment_cases():
    rng = np.random.default_rng(11)
    real_probs = asr.lid_probs
    asr.lid_probs = lambda models, x, langs=None: fake_probs(x, langs)
    lines = {
        'one language': [quiet(0.5, rng), voice(0, 3.0, rng), quiet(0.5, rng)],
        'two languages': [quiet(0.3, rng), voice(1, 1.6, rng), voice(0, 1.8, rng), quiet(0.3, rng)],
        'three, a short blip': [voice(2, 1.5, rng), voice(5, 0.3, rng), voice(2, 1.2, rng), voice(3, 1.5, rng)],
        'short call, sure': [quiet(0.4, rng), voice(4, 0.9, rng), quiet(0.4, rng)],
        'short call, unsure': [quiet(0.4, rng), (voice(4, 0.9, rng) + voice(6, 0.9, rng)) / 2, quiet(0.4, rng)],
        'all quiet': [quiet(2.0, rng)],
        'loud and soft': [voice(7, 1.5, rng, 0.5), voice(8, 1.5, rng, 0.02), voice(7, 1.5, rng, 0.5)],
        'tiny': [voice(1, 0.05, rng)],
    }
    cases = []
    for name, parts in lines.items():
        x = np.concatenate(parts)
        cache = {}
        segs = asr.segments(None, x, fallback='ko', cache=cache)
        cases.append(dict(name=name, x=floats(x, 6), fallback='ko', segs=[list(s) for s in segs],
                          cache_keys=sorted(cache)))
    # (a player's own languages: the candidates are only those - the same audio, other answers)
    for name, langs in [('two languages', ['en', 'ru']), ('two languages', ['ru', 'de']), ('three, a short blip', ['zh', 'es']),
                        ('three, a short blip', ['ja', 'ko', 'zh']), ('short call, sure', ['de', 'fr']),
                        ('short call, sure', ['en', 'ru']), ('loud and soft', ['pt', 'ja'])]:
        x = np.array(next(c['x'] for c in cases if c['name'] == name and 'langs' not in c), np.float32)
        cache = {}
        segs = asr.segments(None, x, fallback='ko', cache=cache, langs=langs)
        cases.append(dict(name=name, langs=langs, x_from=name, fallback='ko', segs=[list(s) for s in segs],
                          cache_keys=sorted(cache)))
    qp = []
    x = np.concatenate([voice(0, 1.0, rng), quiet(0.2, rng), voice(1, 1.0, rng)])
    for t in (0.5, 1.0, 1.1, 1.25, 2.0, 0.05):
        qp.append(dict(t=t, out=asr.quiet_point(x, t)))
    probe = [dict(x=floats(v, 6), p=floats(fake_probs(v), 9)) for v in (voice(3, 1.0, rng), voice(9, 0.05, rng))]
    asr.lid_probs = real_probs
    consts = dict(RATE=asr.RATE, LID_WIN=asr.LID_WIN, LID_MIN=asr.LID_MIN, LID_HOP=asr.LID_HOP, LID_QUIET=asr.LID_QUIET,
                  LID_SWITCH=asr.LID_SWITCH, LID_SURE=asr.LID_SURE, MIXED_LANGS=asr.MIXED_LANGS)
    dump('segments.json', dict(consts=consts, cases=cases, quiet_point=dict(x=floats(x, 6), cases=qp), fake_probs=probe))


# ---------------------------------------------------------------- the real models (slow; needs the benchmark clips)
def real_cases():
    models = asr.Models(threads=4, log=lambda m: None)
    items = {it['id']: it for it in json.load(open(os.path.join(BENCH, 'lid', 'items.json'), encoding='utf-8'))}
    out = dict(offline=[], lid=[], mixed=[])
    for cid, model in [('en01_room', 'parakeet'), ('en13_room', 'parakeet'), ('de03_room', 'parakeet'),
                       ('es02_room', 'parakeet'), ('ru02_room', 'gigaam'), ('ru05_room', 'gigaam'),
                       ('zh03_room', 'sensevoice'), ('zh07_room', 'sensevoice')]:
        x, sr = audio.read_wav(items[cid]['path'])
        text, times, _ = models.offline_timed(model, x, offset=0.25)
        raw, tokens, stamps, _ = models.offline_full(model, x)
        out['offline'].append(dict(id=cid, path=os.path.relpath(items[cid]['path'], ROOT).replace('\\', '/'), model=model,
                                   raw=raw, tokens=tokens, stamps=floats(stamps, 4), text=text, times=times))
    for cid in ('en01_room', 'ru02_room', 'zh03_room', 'de03_room', 'm08'):
        x, sr = audio.read_wav(items[cid]['path'])
        for a, b in ((0.0, 1.0), (0.5, 1.5), (0.0, 0.08)):
            seg = x[int(a * sr):int(b * sr)]
            out['lid'].append(dict(id=cid, path=os.path.relpath(items[cid]['path'], ROOT).replace('\\', '/'), a=a, b=b,
                                   p=floats(asr.lid_probs(models, seg), 7)))
    for cid in ('m08', 'm10', 'm12', 'm14', 'm00', 'en04_room', 'w005_room'):
        if cid not in items:
            continue
        x, sr = audio.read_wav(items[cid]['path'])
        segs, times = [], []
        text, langs, _ = asr.transcribe_mixed(models, x, fallback='en', segs_out=segs, times_out=times)
        out['mixed'].append(dict(id=cid, path=os.path.relpath(items[cid]['path'], ROOT).replace('\\', '/'), text=text,
                                 segs=[list(s) for s in segs], times=times))
    dump('real.json', out)


if __name__ == '__main__':
    text_cases()
    audio_cases()
    feed_cases()
    link_cases()
    segment_cases()
    if '--real' in sys.argv:
        real_cases()
