"""Offline test of the voice helper: the feed, the link to the game, the mixer, the speech detector, the
transcript filter (no sound device, no game). With faster-whisper installed it also transcribes the
speaker's test voice with the real model and prints how long that took.
    python engine/test_helper.py        (conda env "pcvoice"; "teardown" works without the last part)
"""
import math
import os
import sys
import tempfile

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import teardown_helper as H          # noqa: E402
import speech as S          # noqa: E402

FAILED = NCHECK = 0


def check(cond, msg):
    global FAILED, NCHECK
    NCHECK += 1
    if not cond:
        FAILED += 1
    print(('ok   ' if cond else 'FAIL ') + msg)


def db(x):
    return 20 * math.log10(max(1e-12, x))


def rms(x):
    return math.sqrt(float(np.mean(np.square(x))))


class Clock:
    def __init__(self):
        self.t = 0.0

    def __call__(self):
        return self.t


def feed(seq=1, vol=1.0, **sp):
    d = dict(src=1, talk=True, gain=1.0, az=0.0, el=0.0, muffle=0.0)
    d.update(sp)
    return dict(seq=seq, vol=vol, sid=1, ack=0, ping=1, mic=False, speakers={7: d})


def run(mixer, clock, seconds, block=480):
    """render `seconds` of audio in blocks, the clock moving with it"""
    out = []
    for _ in range(int(seconds * H.RATE / block)):
        out.append(mixer.render(block))
        clock.t += block / H.RATE
    return np.concatenate(out)


def settled(clips, f, seconds=0.5):
    """the output once the gains have settled on feed f (the feed kept fresh)"""
    clock = Clock()
    m = H.Mixer(clips, clock=clock)
    out = []
    for _ in range(int(seconds / 0.05) + 8):
        m.set_feed(f)
        out.append(run(m, clock, 0.05))
    return np.concatenate(out[8:])


rng = np.random.default_rng(1)
NOISE = {1: (rng.standard_normal(H.RATE) * 0.1).astype(np.float32)}
ONES = {1: np.full(H.RATE, 0.1, dtype=np.float32)}

# ---- the feed
f = H.parse_feed('2|42|0.50|7|3|12|1|2000,1,0,0.550,-39.8,0.0,0.00;2001,2,1,1.000,0.0,-3.5,0.25')
check(f and f['seq'] == 42 and f['vol'] == 0.5 and f['sid'] == 7 and f['ack'] == 3 and f['ping'] == 12 and f['mic'] is True
      and len(f['speakers']) == 2 and f['speakers'][2001] == dict(src=2, talk=True, gain=1.0, az=0.0, el=-3.5, muffle=0.25)
      and f['speakers'][2000]['talk'] is False,
      'a feed string parses: sequence, volume, session, ack, ping, mic, each speaker')
f = H.parse_feed('2|43|1.00|7|0|1|0|')
check(f and f['speakers'] == {} and f['mic'] is False, 'no speakers, mic off parses')
f4 = H.parse_feed('4|45|1.00|7|0|1|1|zh|0|')
check(f4 and f4['lang'] == 'zh' and f4['live'] is False and H.parse_feed('4|45|1.00|7|0|1|1|zh|1|')['live'] is True,
      'version 4 carries the live-words switch')
f = H.parse_feed('3|44|1.00|7|0|1|1|ru|2000,1,1,1.000,0.0,0.0,0.00')
check(f and f['lang'] == 'ru' and f['mic'] is True and len(f['speakers']) == 1 and H.parse_feed('2|43|1.00|7|0|1|0|')['lang'] == 'en',
      'version 3 carries the language the player speaks (version 2: English)')
check(H.parse_feed('1|1|1.00|') is None and H.parse_feed('garbage') is None and H.parse_feed('2|x|1|1|1|1|1|') is None,
      'another version or a broken string is refused')
A, B = '2|5|1.00|7|0|1|0|2000,1,1,1.000,0.0,0.0,0.00', '2|9|1.00|3|0|1|0|'
xml = ('<registry version="2.1.0">\n<savegame><mod>\n<local-proximity-chat>\n<pcmode value="s"/>\n<pcvx>\n\t<f value="%s"/>\n</pcvx>\n'
       '</local-proximity-chat>\n<steam-123>\n<pcvx>\n<f value="%s"/>\n</pcvx>\n</steam-123>\n</mod></savegame>\n</registry>\n' % (A, B)).encode()
check(H.find_feeds(xml) == [('local-proximity-chat', A), ('steam-123', B)], 'both feeds in a savegame.xml are found, each with its copy of the mod (local, Workshop)')

# ---- the reader: what is in the file at the start is not live; a change is
with tempfile.TemporaryDirectory() as tmp:
    path = os.path.join(tmp, 'savegame.xml')
    open(path, 'wb').write(xml)
    m = H.Mixer(NOISE)
    r = H.FeedReader(m, path=path)
    r.once()
    check(m.feed is None and r.updates == 0, 'a feed already in the file when the helper starts is not played (an old session)')
    open(path, 'wb').write(xml.replace(b'2|5|', b'2|6|'))
    r.once()
    check(m.feed is not None and m.feed['seq'] == 6 and r.updates == 1, 'a feed that changes is the live one')
    open(path, 'wb').write(b'<registry><pcvx><f value="2|7|1.00|7|0|1|0|"/></pcvx>')
    r.once()
    check(m.feed['seq'] == 6, 'a half-written file is skipped')


# ---- the link: the helper's files for the game
def names(d):
    return sorted(os.listdir(d))


with tempfile.TemporaryDirectory() as local, tempfile.TemporaryDirectory() as shop:
    open(os.path.join(local, 'pcvx_p77'), 'w').write('1')            # (left by a crash)
    open(os.path.join(local, 'other.txt'), 'w').write('1')
    link = H.Link([local, shop], log=lambda s: None)
    link.start()
    check(names(local) == ['other.txt', 'pcvx_on'] and names(shop) == ['pcvx_on'],
          'start: "on" in every folder a copy of the mod may look in; old files of mine swept, nothing else touched')
    check(link.send_text('too early') is False and names(local) == ['other.txt', 'pcvx_on'], 'no game yet: a text is not written')

    def fd(**kw):
        d = dict(seq=1, vol=1.0, sid=5, ack=0, ping=1, mic=False, speakers={})
        d.update(kw)
        return d
    link.on_feed(fd(), 'local-proximity-chat')
    check('pcvx_p1' in names(local) and link.dir == local, 'the first feed: the ping is answered where that copy of the mod looks')
    link.on_feed(fd(ping=2, mic=True), 'local-proximity-chat')
    check('pcvx_p2' in names(local) and 'pcvx_p1' not in names(local) and link.mic is True, 'the next ping: its answer, the last one removed; the game wants the microphone')
    link.on_feed(fd(ping=1002), 'local-proximity-chat')
    check('pcvx_p2' in names(local), 'ping 1002 is answered as p2 (numbers wrap at 1000)')
    check(link.send_text('open sesame, "quoted" & <ok>') and link.send_text('Привет, 你好'), 'two texts are written')
    t1 = open(os.path.join(local, 'pcvx_t1.xml'), encoding='utf-8').read()
    t2 = open(os.path.join(local, 'pcvx_t2.xml'), encoding='utf-8').read()
    hex1 = t1.split('t=')[1].split('"')[0]
    check(t1.startswith('<prefab') and '<body tags="pcvx k=f u=0 t=' in t1 and bytes.fromhex(hex1).decode() == 'open sesame, "quoted" & <ok>'
          and bytes.fromhex(t2.split('t=')[1].split('"')[0]).decode() == 'Привет, 你好' and not [n for n in names(local) if n.endswith('.tmp')],
          'numbered t1, t2: a prefab whose tag is the hex of the text (anything survives)')
    link.on_feed(fd(ping=1002, ack=1, lang='zh'), 'local-proximity-chat')
    check(link.lang == 'zh', 'the language from the feed')
    check(link.send_msg('l', 7, 'the words so') and link.send_msg('f', 7, '') and not link.send_msg('l', 7, '  '),
          'live words and an empty finished line are sent; empty live words are not')
    t3 = open(os.path.join(local, 'pcvx_t3.xml'), encoding='utf-8').read()
    t4 = open(os.path.join(local, 'pcvx_t4.xml'), encoding='utf-8').read()
    check('tags="pcvx k=l u=7 t=' in t3 and bytes.fromhex(t3.split('t=')[1].split('"')[0]).decode() == 'the words so'
          and 'tags="pcvx k=f u=7 t="' in t4, 'their kinds and utterance in the tags (k=l / k=f, u=7)')
    check('pcvx_t1.xml' not in names(local) and 'pcvx_t2.xml' in names(local) and 'pcvx_t4.xml' in names(local), 'the game acks 1: that file is deleted, 2 waits')
    link.on_feed(fd(ping=1, sid=6, ack=0), 'local-proximity-chat')
    check('pcvx_t2.xml' not in names(local) and 'pcvx_p1' in names(local) and link.n == 0, 'a new session (a level start): unread texts dropped, numbers start over')
    link.send_text('first of the new level')
    check('pcvx_t1.xml' in names(local), '... the next text is t1 again')
    link2 = H.Link([local, shop], log=lambda s: None)
    link2.sid = None
    link2.on_feed(fd(sid=6, ack=4, ping=9), 'local-proximity-chat')
    link2.send_text('helper restarted')
    check('pcvx_t5.xml' in names(local), 'a helper started in the middle of a session continues after the game\'s ack (t5)')
    link.on_feed(fd(sid=6, ping=3), 'steam-3812301496')
    check(link.dir == shop and 'pcvx_p3' in names(shop), 'the Workshop copy of the mod is answered in the Workshop folder')
    link.stop()
    check(names(local) == ['other.txt'] and names(shop) == [], 'stop: all my files gone')

# ---- direction
o = settled(NOISE, feed(az=0))
check(abs(db(rms(o[:, 0])) - db(rms(o[:, 1]))) < 0.1 and rms(o) > 0.01, 'ahead: both ears the same')
o = settled(NOISE, feed(az=90))
right = db(rms(o[:, 1])) - db(rms(o[:, 0]))
check(right > 15, 'to the right: the right ear much louder (%.0f dB)' % right)
o = settled(NOISE, feed(az=-90))
check(db(rms(o[:, 0])) - db(rms(o[:, 1])) > 15, 'to the left: mirrored')
o = settled(NOISE, feed(az=30))
d30 = db(rms(o[:, 1])) - db(rms(o[:, 0]))
check(3 < d30 < right, 'a little to the right: a little louder there (%.1f dB)' % d30)
front, back = settled(NOISE, feed(az=0)), settled(NOISE, feed(az=180))
check(abs(db(rms(back[:, 0])) - db(rms(back[:, 1]))) < 0.1 and db(rms(front)) - db(rms(back)) > 1.5,
      'behind: centred, quieter and duller than ahead (%.1f dB)' % (db(rms(front)) - db(rms(back))))
up = settled(NOISE, feed(az=90, el=80))
check(db(rms(up[:, 1])) - db(rms(up[:, 0])) < 4, 'almost overhead: hardly to one side')
p_c, p_r = settled(NOISE, feed(az=0)), settled(NOISE, feed(az=90))
check(abs(db(rms(p_c) * math.sqrt(2)) - db(rms(p_r) * math.sqrt(2))) < 0.5, 'the same loudness in every direction (constant power)')


# ---- volume, talking, muffle
def high(x):
    """the share of the left channel's level above 3 kHz"""
    spec = np.abs(np.fft.rfft(x[:, 0]))
    freqs = np.fft.rfftfreq(len(x), 1 / H.RATE)
    return math.sqrt(float(np.sum(spec[freqs > 3000] ** 2)))


full = settled(NOISE, feed())
half = settled(NOISE, feed(gain=0.5))
check(abs(db(rms(full)) - db(rms(half)) - 6.02) < 0.3, 'half the gain: 6 dB quieter')
check(abs(db(rms(full)) - db(rms(settled(NOISE, feed(vol=0.5)))) - 6.02) < 0.3, 'half the voice volume (the Settings slider): 6 dB quieter')
check(rms(settled(NOISE, feed(gain=0))) < 1e-6 and rms(settled(NOISE, feed(talk=False))) < 1e-6, 'gain 0 or not talking: silence')
check(rms(settled(NOISE, feed(src=9))) < 1e-6, 'a speaker without audio (an unknown source): silence, no crash')
muf = settled(NOISE, feed(muffle=1))
check(db(high(full)) - db(high(muf)) > 30, 'fully muffled: above 3 kHz down %.0f dB' % (db(high(full)) - db(high(muf))))
mid = settled(NOISE, feed(muffle=0.5))
check(db(high(full)) - db(high(mid)) > 6 and db(high(mid)) - db(high(muf)) > 6, 'half muffled: in between')

# ---- smooth: a jump in the feed becomes a glide, no click
clock = Clock()
m = H.Mixer(ONES, clock=clock)
m.set_feed(feed(gain=0))
run(m, clock, 0.1)
m.set_feed(feed(gain=1.0))
o = run(m, clock, 0.5)
step = float(np.max(np.abs(np.diff(o[:, 0]))))
check(step < 0.002 and o[-1, 0] > 0.05, 'gain 0 -> 1 in one feed: a glide (largest step between samples %.5f)' % step)
m.set_feed(feed(gain=1.0, az=90))
o2 = run(m, clock, 0.5)
check(float(np.max(np.abs(np.diff(o2[:, 0])))) < 0.002, 'ahead -> right in one feed: a glide')

# ---- the game stops sending: fade out; back again: back
clock = Clock()
m = H.Mixer(NOISE, clock=clock)
for _ in range(10):
    m.set_feed(feed())
    run(m, clock, 0.05)
loud = rms(run(m, clock, 0.05))
quiet = rms(run(m, clock, H.STALE + 0.5)[-4800:])
check(loud > 0.01 and quiet < 1e-4 and not m.fresh(), 'no new feed for %.1f s (the game paused or gone): the voices fade out' % H.STALE)
for _ in range(10):
    m.set_feed(feed())
    run(m, clock, 0.05)
check(rms(run(m, clock, 0.05)) > 0.01, '... and come back with the feed')
m.set_feed(dict(seq=99, vol=1.0, speakers={}))
run(m, clock, 0.6)
check(rms(run(m, clock, 0.05)) < 1e-6 and not m.voices, 'an empty feed (voice off): silence, the speaker forgotten')

# ---- a voice keeps its place while it is not talking; the limiter
SAW = {1: (np.arange(H.RATE, dtype=np.float32) / H.RATE * 0.1)}
clock = Clock()
m = H.Mixer(SAW, clock=clock)
m.set_feed(feed())
run(m, clock, 0.3)
m.set_feed(feed(talk=False))
run(m, clock, 1.0)
pos = m.voices[7].pos
m.set_feed(feed(talk=False))
run(m, clock, 0.5)
check(m.voices[7].pos == pos and 0.3 * H.RATE <= pos < 0.9 * H.RATE, 'a voice that stops talking keeps its place in its recording (%.2f s)' % (pos / H.RATE))
LOUD = {1: np.full(H.RATE, 5.0, dtype=np.float32)}
check(float(np.max(np.abs(settled(LOUD, feed())))) <= 1.0, 'too loud: limited to full scale, never beyond')

# ---- the speech detector: utterances out of a microphone stream
SR = 16000


def sound(seconds, level_db, kind='noise'):
    n = int(seconds * SR)
    amp = 10 ** (level_db / 20)
    if kind == 'tone':                                # (speech-like: a tone going up and down 4 times a second)
        t = np.arange(n) / SR
        env = 0.25 + 0.75 * np.abs(np.sin(2 * np.pi * 2 * t))
        return (np.sin(2 * np.pi * 220 * t) * env * amp * math.sqrt(2)).astype(np.float32)
    return (rng.standard_normal(n) * amp).astype(np.float32)


def detect(parts, **kw):
    det = S.SpeechDetector(SR, **kw)
    out = []
    x = np.concatenate(parts)
    for i in range(0, len(x), 480):
        out += det.feed(x[i:i + 480])
    return out, det


room = -60
utt, det = detect([sound(2, room), sound(1.5, -25, 'tone'), sound(2, room)])
check(len(utt) == 1 and 1.9 < len(utt[0]) / SR < 2.7, 'one utterance in a quiet room: cut out with a little before and the pause after (%.2f s)' % (len(utt[0]) / SR if utt else 0))
check(abs(det.floor - room) < 4, 'the room\'s noise is learned (%.0f dB)' % det.floor)
utt, _ = detect([sound(2, room), sound(1.0, -25, 'tone'), sound(0.3, room), sound(1.0, -25, 'tone'), sound(2, room)])
check(len(utt) == 1, 'a short pause inside a sentence does not cut it')
utt, _ = detect([sound(2, room), sound(1.0, -25, 'tone'), sound(1.5, room), sound(1.0, -25, 'tone'), sound(2, room)])
check(len(utt) == 2, 'a long pause: two utterances')
utt, _ = detect([sound(2, room), sound(0.09, -20, 'tone'), sound(2, room)])
check(len(utt) == 0, 'a click (90 ms) is not speech')
utt, _ = detect([sound(6, -40)])
check(len(utt) == 0, 'steady noise, even loud (a fan at -40 dB): never speech')
utt, det = detect([sound(2, room), sound(6, -40)])
check(len(utt) == 0 and not det.talking and abs(det.floor + 40) < 3, 'a fan that starts later: not speech either, the floor moves up to it (%.0f dB)' % det.floor)
utt, _ = detect([sound(3, -40), sound(1.5, -22, 'tone'), sound(2, -40)])
check(len(utt) == 1, '... and speech over that noise is still found')
utt, _ = detect([sound(1, room), sound(40, -25, 'tone'), sound(2, room)])
check(len(utt) >= 2 and max(len(u) for u in utt) / SR <= 15.1, 'talking without a pause: cut every 15 s')
utt, det = detect([sound(1, room), sound(1.0, -25, 'tone')])
check(len(utt) == 0 and det.talking, 'still talking at the end of the stream: nothing out yet, "talking" is set')

# ---- the transcript filter
check(S.clean_transcript('  [Music]  Hello   there (laughs) ') == 'Hello there' and S.clean_transcript('[BLANK_AUDIO]') == '',
      'non-speech marks are taken out of a transcript')
check(S.keep_transcript('Open the door.', -0.3, 0.02) and not S.keep_transcript('', -0.1, 0.0) and not S.keep_transcript('...', -0.1, 0.0),
      'a clear sentence is kept; nothing or only dots is not')
check(not S.keep_transcript('Thank you.', -0.8, 0.5) and S.keep_transcript('Thank you.', -0.2, 0.05),
      '"Thank you." from noise (the model unsure) is dropped; said clearly it is kept')
check(not S.keep_transcript('asdf qwer', -1.5, 0.1) and not S.keep_transcript('Hello there', -0.7, 0.8) and not S.keep_transcript('la la la la', -0.3, 0.1, 3.0),
      'a wild guess, a probably-silent segment and a repetition loop are dropped')

# ---- the real model on the speaker's test voice (only where faster-whisper is installed)
try:
    import faster_whisper  # noqa: F401
    have = os.path.exists(os.path.join(H.WORK, 'voice2.wav'))
except ImportError:
    have = False
if have:
    tr = S.Transcriber(None, model='base.en', language='en', threads=4, log=lambda s: None)
    tr.load()
    audio, sr = H.read_wav(os.path.join(H.WORK, 'voice2.wav'))
    text, took = tr.transcribe(audio, sr)
    text, took = tr.transcribe(audio, sr)
    low = text.lower()
    check('speaker' in low and 'normal voice' in low and 'wednesday' in low and 'sunday' in low,
          'the real model (base.en) transcribes the speaker\'s test voice: %.1f s of audio in %.2f s - "%s"' % (len(audio) / sr, took, text))
    quiet = (rng.standard_normal(SR * 3) * 0.003).astype(np.float32)
    text, took = tr.transcribe(quiet, SR)
    check(text == '', 'three seconds of room noise: no words (%.2f s)%s' % (took, '' if text == '' else ' - got "%s"' % text))
else:
    print('skip the real model (faster-whisper or export/voicehelper/voice2.wav not here)')

print('\n%d checks, %d failed' % (NCHECK, FAILED))
sys.exit(1 if FAILED else 0)
