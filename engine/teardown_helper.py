"""Voice helper (PROTOTYPE; no network yet: you hear the voice dummies, and what YOU say becomes chat text).

The game cannot play a live voice or hear a microphone, so this program does. It talks with a game mod (the
first: Proximity Babble Chat, Teardown) through files on this machine - PROTOCOL.md has the formats:
  game -> helper   savegame.xml: savegame.mod.pcvx.f, ~20 times a second - whom the player hears and how
                   (volume, direction, muffle), and what the game wants (the microphone on, a ping)
  helper -> game   small files next to the mod's folder: pcvx_on (I am running), pcvx_p<n> (the answer to
                   ping n), pcvx_t<n>.xml (text number n: what the player said)
What it does:
  - plays the voice dummies (/dummy voice): three recorded voices, mixed by direction, distance and walls
  - when the game asks (Settings: "Write what I say in the chat"): listens to the microphone and, ON THIS
    MACHINE (asr.py: Silero speech detector, then the language's own model every second and over the whole line),
    sends the game the live words while you talk and the finished line after; the game shows the live words
    over your head and says the line in the chat. The language: the game's "Language I speak" (or --lang)

    python engine/teardown_helper.py            # start it, then play; in the game: Enter, Voice
    python engine/teardown_helper.py --demo     # no game needed: one voice walks a circle around you
    python engine/teardown_helper.py --list     # sound devices;  --device N / --mic-device N pick one
    python engine/teardown_helper.py --transcribe some.wav --lang ru   # a recording through the pipeline

Needs numpy, scipy, sounddevice and sherpa-onnx (conda env "pcvoice", pip only). Ctrl+C stops it.
"""
import argparse
import atexit
import ctypes
import glob
import json
import math
import msvcrt
import os
import re
import subprocess
import sys
import threading
import time
import wave
from ctypes import wintypes

import numpy as np
from scipy.signal import lfilter, resample_poly

RATE = 48000
SMOOTH = 0.05          # s: gains, direction and muffle glide to each new feed value
STALE = 1.5            # s without a new feed (the game paused or gone): the voices fade out
CUT_CLEAR, CUT_MUFFLED = 16000.0, 400.0   # low-pass cutoff (Hz) at muffle 0 and 1
PAN = 0.9              # how far to one side a voice goes at 90 degrees (1 = that ear only)
BEHIND_MUFFLE, BEHIND_QUIET = 0.25, 0.2   # a voice straight behind: this much duller and quieter
HEADROOM = 1.3         # gain before the soft limiter
TEXT_MAX = 400         # characters of one text file

HERE = os.path.dirname(os.path.abspath(__file__))
WORK = os.path.join(os.path.dirname(HERE), 'export', 'voicehelper')
TD = os.environ.get('SAVEPROBE_DIR') or os.path.join(os.environ.get('LOCALAPPDATA', ''), 'Teardown')
SAVE = os.path.join(TD, 'savegame.xml')
WORKSHOP = r'C:\Program Files (x86)\Steam\steamapps\workshop\content\1167630'

# the three test voices: (Windows voice, speaking rate -10..10, what it says)
VOICES = [
    ('Microsoft Zira Desktop', -1, 'I am the whisperer. Stay close, or you will not hear me at all. '
     'One, two, three, four, five, six, seven, eight, nine, ten.'),
    ('Microsoft David Desktop', 0, 'I am the speaker. This is my normal voice, and it carries a fair distance. '
     'Monday, Tuesday, Wednesday, Thursday, Friday, Saturday, Sunday.'),
    ('Microsoft Zira Desktop', 2, 'I am the yeller! You can hear me from far away! '
     'Red, orange, yellow, green, blue, purple, black and white!'),
]
NAMES = {1: 'whisperer', 2: 'speaker', 3: 'yeller'}

FEED = re.compile(rb'<pcvx>\s*<f\s+value="([^"]*)"\s*/>\s*</pcvx>')
MODTAG = re.compile(rb'<((?:local|steam)-[^\s/>]+)>')


# ---------------------------------------------------------------- the feed (game -> helper)
def parse_feed(text):
    """'3|seq|volume|session|ack|ping|mic|lang|id,src,talk,gain,az,el,muffle;...' (version 2: no lang) -> a dict,
    or None"""
    try:
        parts = text.split('|')
        live = '1'
        if parts[0] == '4' and len(parts) == 10:
            _, seq, vol, sid, ack, ping, mic, lang, live, rest = parts
        elif parts[0] == '3' and len(parts) == 9:
            _, seq, vol, sid, ack, ping, mic, lang, rest = parts
        elif parts[0] == '2' and len(parts) == 8:
            _, seq, vol, sid, ack, ping, mic, rest = parts
            lang = 'en'
        else:
            return None
        speakers = {}
        for item in rest.split(';'):
            if not item:
                continue
            spid, src, talk, gain, az, el, muffle = item.split(',')
            speakers[int(spid)] = dict(src=int(src), talk=talk == '1', gain=float(gain), az=float(az),
                                       el=float(el), muffle=float(muffle))
        return dict(seq=int(seq), vol=float(vol), sid=int(sid), ack=int(ack), ping=int(ping), mic=mic == '1',
                    lang=lang or 'en', live=live != '0', speakers=speakers)
    except ValueError:
        return None


def find_feeds(data):
    """[(the mod's tag, the feed string)] for every copy of the mod in a savegame.xml: 'local-proximity-chat'
    (the mods folder) and 'steam-<id>' (the Workshop) each have their own"""
    out = []
    for m in FEED.finditer(data):
        tags = MODTAG.findall(data, 0, m.start())
        out.append((tags[-1].decode('ascii', 'replace') if tags else '', m.group(1).decode('ascii', 'replace')))
    return out


_k32 = ctypes.WinDLL('kernel32', use_last_error=True) if os.name == 'nt' else None
if _k32:
    _k32.CreateFileW.restype = wintypes.HANDLE
    _k32.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID,
                                 wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]


def read_shared(path):
    """the file's bytes, opened so that the game can write, replace or delete it meanwhile; None if not there"""
    h = _k32.CreateFileW(path, 0x80000000, 0x1 | 0x2 | 0x4, None, 3, 0, None)
    if h is None or h == wintypes.HANDLE(-1).value:
        return None
    with os.fdopen(msvcrt.open_osfhandle(h, os.O_RDONLY | os.O_BINARY), 'rb') as f:
        return f.read()


class FeedReader(threading.Thread):
    """polls savegame.xml; a feed that changes is live: it goes to the mixer and the link"""
    def __init__(self, mixer, link=None, path=SAVE, poll=0.01):
        super().__init__(daemon=True)
        self.mixer, self.link, self.path, self.poll = mixer, link, path, poll
        self.last = {}            # the mod's tag -> its last string
        self.updates = 0
        self.running = True

    def once(self):
        data = read_shared(self.path)
        if not data or not data.rstrip().endswith(b'</registry>'):
            return
        for tag, text in find_feeds(data):
            if self.last.get(tag) != text:
                first = tag not in self.last
                self.last[tag] = text
                feed = parse_feed(text)
                if feed and not first:          # (what was in the file before we started is not live)
                    self.updates += 1
                    self.mixer.set_feed(feed)
                    if self.link:
                        self.link.on_feed(feed, tag)

    def run(self):
        while self.running:
            try:
                self.once()
            except OSError:
                pass
            time.sleep(self.poll)


# ---------------------------------------------------------------- the link (helper -> game)
def io_dirs():
    """the folders a running copy of the mod looks in (MOD/../): the local mods folder, then the Workshop's"""
    mods = os.environ.get('HFP_MODS') or os.path.join(os.environ.get('USERPROFILE', ''), 'OneDrive', 'Documents', 'Teardown', 'mods')
    if not os.path.isdir(mods):
        mods = os.path.join(os.environ.get('USERPROFILE', ''), 'Documents', 'Teardown', 'mods')
    dirs = [mods]
    if os.path.isdir(WORKSHOP) and not os.environ.get('HFP_MODS'):
        dirs.append(WORKSHOP)
    return dirs


def times_hex(times):
    """unit start times (s) as the tag w: 4 hex digits each, in 1/100 s (up to 655 s)"""
    return ''.join('%04x' % max(0, min(0xFFFF, int(round(t * 100)))) for t in times)


def text_prefab(text, kind='f', utt=0, times=None, ago=None):
    """the file the game Spawns to read a message: a body whose tags are its kind ("l" the live words so far, "f"
    the finished line), the utterance it belongs to, t = the hex of the UTF-8 text, and when there are word times:
    w = each unit's start (times_hex; s after the line's audio began) and a = how long ago that was, in 1/100 s,
    when the file was written (the game turns it into a moment on its own clock)"""
    extra = ''
    if times is not None and ago is not None:
        extra = ' w=%s a=%d' % (times_hex(times), max(0, int(round(ago * 100))))
    return '<prefab version="1.5.2">\n\t<body tags="pcvx k=%s u=%d t=%s%s"/>\n</prefab>\n' % (
        kind, utt, text.encode('utf-8').hex(), extra)


class Link:
    """the helper's files for the game: pcvx_on, the answer to each ping, numbered text files"""
    PREFIX = 'pcvx_'

    def __init__(self, dirs, log=print):
        self.dirs, self.log = dirs, log
        self.dir = None           # where the live copy of the mod looks (known from its first feed)
        self.sid = self.ping = None
        self.n = 0                # the last text number written
        self.pending = {}         # number -> path, until the game acks it
        self.mic = False
        self.lang = 'en'
        self.live = True
        self.lock = threading.Lock()

    def _path(self, d, name):
        return os.path.join(d, self.PREFIX + name)

    def _remove(self, path):
        try:
            os.remove(path)
        except OSError:
            pass

    def _sweep(self, d, keep_on=False):
        for path in glob.glob(self._path(d, '*')):
            if not (keep_on and os.path.basename(path) == self.PREFIX + 'on'):
                self._remove(path)

    def start(self):
        for d in self.dirs:
            if os.path.isdir(d):
                self._sweep(d)
                with open(self._path(d, 'on'), 'w') as f:
                    f.write('1')

    def stop(self):
        for d in self.dirs:
            if os.path.isdir(d):
                self._sweep(d)

    def dir_for(self, tag):
        if tag.startswith('steam-') and len(self.dirs) > 1:
            return self.dirs[1]
        return self.dirs[0]

    def on_feed(self, feed, tag):
        d = self.dir_for(tag)
        with self.lock:
            if feed['sid'] != self.sid or d != self.dir:            # (a new level, or another copy of the mod)
                for path in self.pending.values():
                    self._remove(path)
                self.pending = {}
                self.sid, self.dir, self.ping, self.n = feed['sid'], d, None, feed['ack']
            if feed['ping'] != self.ping:                           # (alive: the answer to this ping, the last one gone)
                with open(self._path(d, 'p%d' % (feed['ping'] % 1000)), 'w') as f:
                    f.write('1')
                if self.ping is not None and self.ping % 1000 != feed['ping'] % 1000:
                    self._remove(self._path(d, 'p%d' % (self.ping % 1000)))
                self.ping = feed['ping']
            for n in [n for n in self.pending if n <= feed['ack']]:   # (read by the game)
                self._remove(self.pending.pop(n))
            self.mic = feed['mic']
            self.lang = feed.get('lang', 'en')
            self.live = feed.get('live', True)

    def send_text(self, text):
        """hand a finished line to the game; False if no game is listening"""
        text = text.strip()
        return bool(text) and self.send_msg('f', 0, text)

    def send_msg(self, kind, utt, text, times=None, t0=None):
        """hand a message to the game: kind "s" (the player started talking: no text yet), "l" (the live words so far)
        or "f" (the finished line; "" = nothing made out: the live words go); times: each unit's start (s after t0, time.perf_counter() when the line's
        audio began) - written with how long ago t0 is now. False if no game is listening"""
        import asr
        text = text.strip()[:TEXT_MAX]
        if times is not None:                                # (cut with the text: the first n units keep their times)
            n = len(asr.units(text))
            times = list(times)[:n] if len(times) >= n else None
        with self.lock:
            if self.dir is None or (kind == 'l' and not text):
                return False
            self.n += 1
            path = self._path(self.dir, 't%d.xml' % self.n)
            tmp = self._path(self.dir, 'w%d.tmp' % self.n)
            ago = (time.perf_counter() - t0) if (times is not None and t0 is not None) else None
            with open(tmp, 'w', encoding='utf-8') as f:
                f.write(text_prefab(text, kind, utt, times if ago is not None else None, ago))
            os.replace(tmp, path)                                   # (appears complete, never half-written)
            self.pending[self.n] = path
            return True


# ---------------------------------------------------------------- the mixer
def pan_gains(az, el):
    """left, right gain for a direction (degrees; az 0 ahead, 90 right; el up): constant power"""
    p = max(-PAN, min(PAN, math.sin(math.radians(az)) * math.cos(math.radians(el)) * PAN))
    th = (p + 1) * math.pi / 4
    return math.cos(th), math.sin(th)


def behind(az):
    """0 in front .. 1 straight behind"""
    return max(0.0, (abs(az) - 90.0) / 90.0)


class Voice:
    def __init__(self):
        self.pos = 0
        self.talking = False      # (a new turn starts the clip from its beginning: the game times the words to it)
        self.gl = self.gr = 0.0
        self.muffle = 0.0
        self.z1 = np.zeros(1)
        self.z2 = np.zeros(1)
        self.src_clip = None      # the clip it last played (to fade out on after it left the feed)


class Mixer:
    """clips: {src: mono float32 array at RATE}. render(frames) -> (frames, 2) float32"""
    def __init__(self, clips, rate=RATE, clock=time.perf_counter):
        self.clips, self.rate, self.clock = clips, rate, clock
        self.feed, self.feed_t = None, -1e9
        self.voices = {}
        self.lock = threading.Lock()
        self.volume = 1.0         # the helper's own volume (the --volume option), on top of the game's
        self.levels = {}          # id -> the level last rendered (for the status line)

    def set_feed(self, feed):
        with self.lock:
            self.feed, self.feed_t = feed, self.clock()

    def fresh(self):
        with self.lock:
            return self.feed is not None and self.clock() - self.feed_t <= STALE

    def render(self, frames):
        with self.lock:
            feed, age = self.feed, self.clock() - self.feed_t
        live = feed is not None and age <= STALE
        speakers = feed['speakers'] if live else {}
        master = (feed['vol'] if live else 0.0) * self.volume * HEADROOM
        out = np.zeros((frames, 2), dtype=np.float32)
        k = 1.0 - math.exp(-frames / self.rate / SMOOTH)
        ramp = np.arange(1, frames + 1, dtype=np.float32) / frames
        for sid in set(self.voices) | set(speakers):
            sp = speakers.get(sid)
            v = self.voices.get(sid)
            if v is None:
                v = self.voices[sid] = Voice()
            clip = self.clips.get(sp['src']) if sp else None
            talking = bool(sp and sp['talk'] and clip is not None)
            if talking and not v.talking:
                v.pos = 0
            v.talking = talking
            if talking:
                b = behind(sp['az'])
                g = sp['gain'] * master * (1 - BEHIND_QUIET * b)
                l, r = pan_gains(sp['az'], sp['el'])
                tl, tr, tm = g * l, g * r, min(1.0, sp['muffle'] + BEHIND_MUFFLE * b)
            else:
                tl, tr, tm = 0.0, 0.0, v.muffle
            if not talking and v.gl < 1e-4 and v.gr < 1e-4:
                v.gl = v.gr = 0.0
                self.levels[sid] = 0.0
                if sp is None:
                    del self.voices[sid]
                continue
            src = v.src_clip if clip is None else clip      # (fading out after it left the feed: its last clip)
            v.src_clip = src
            idx = (v.pos + np.arange(frames)) % len(src)
            x = src[idx]
            v.pos = (v.pos + frames) % len(src)
            v.muffle += (tm - v.muffle) * k
            fc = CUT_CLEAR * (CUT_MUFFLED / CUT_CLEAR) ** v.muffle
            a = 1.0 - math.exp(-2 * math.pi * fc / self.rate)
            x, v.z1 = lfilter([a], [1.0, a - 1.0], x, zi=v.z1)
            x, v.z2 = lfilter([a], [1.0, a - 1.0], x, zi=v.z2)
            nl, nr = v.gl + (tl - v.gl) * k, v.gr + (tr - v.gr) * k
            out[:, 0] += x * (v.gl + (nl - v.gl) * ramp)
            out[:, 1] += x * (v.gr + (nr - v.gr) * ramp)
            v.gl, v.gr = nl, nr
            self.levels[sid] = max(nl, nr)
        return np.tanh(out)


# ---------------------------------------------------------------- the test voices
def make_voices():
    """the three test voices as mono float32 at RATE (made once with the Windows computer voices)"""
    os.makedirs(WORK, exist_ok=True)
    clips = {}
    for i, (voice, rate, text) in enumerate(VOICES, 1):
        path = os.path.join(WORK, 'voice%d.wav' % i)
        if not os.path.exists(path):
            ps = ("Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; "
                  "try { $s.SelectVoice('%s') } catch {}; $s.Rate = %d; $s.SetOutputToWaveFile('%s'); $s.Speak('%s'); $s.Dispose()"
                  % (voice, rate, path, text.replace("'", "''")))
            subprocess.run(['powershell', '-NoProfile', '-Command', ps], check=True)
        clips[i] = load_wav(path)
    return clips


def read_wav(path):
    """(mono float32 samples, rate) of a 16-bit wav"""
    with wave.open(path, 'rb') as w:
        sr, ch, sw = w.getframerate(), w.getnchannels(), w.getsampwidth()
        raw = w.readframes(w.getnframes())
    if sw != 2:
        raise ValueError('%s: 16-bit wav expected' % path)
    x = np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768.0
    if ch > 1:
        x = x.reshape(-1, ch).mean(axis=1)
    return x, sr


def load_wav(path):
    x, sr = read_wav(path)
    if sr != RATE:
        g = math.gcd(RATE, sr)
        x = resample_poly(x, RATE // g, sr // g).astype(np.float32)
    loud = float(np.percentile(np.abs(x), 99.9)) or 1.0
    x = x * (0.5 / loud)                                        # (every voice equally loud, peaks at half scale)
    return np.concatenate([x, np.zeros(int(RATE * 0.5), dtype=np.float32)]).astype(np.float32)


# ---------------------------------------------------------------- running it
def status(mixer, rate, mic=None, last='', live=''):
    with mixer.lock:
        feed, age = mixer.feed, mixer.clock() - mixer.feed_t
    if feed is None:
        return 'waiting for the game (play a level with a mod that uses this engine, e.g. Proximity Babble Chat)'
    if age > STALE:
        return 'the game stopped sending (paused, or the level ended)'
    parts = ['%2d/s' % rate]
    if mic is not None and mic.is_open():
        talking = mic.listener.line is not None
        parts.append('mic %s %4.0f dB' % ('TALKING' if talking else 'quiet  ', mic.level))
    elif feed.get('mic'):
        parts.append('mic wanted')
    else:
        parts.append('mic off')
    for sid, sp in sorted(feed['speakers'].items()):
        parts.append('%s%s vol %3d%% dir %4.0f muffle %3d%%' % (
            NAMES.get(sp['src'], sid), '*' if sp['talk'] else ' ', round(sp['gain'] * 100), sp['az'], round(sp['muffle'] * 100)))
    if live:
        parts.append('hearing: ' + live[-40:])
    elif last:
        parts.append('said: ' + last)
    return ' | '.join(parts)


AUTO_GAP = 8          # s between two --auto lines
AUTO_LINES = [
    "open sesame, it's me",
    "To test the chat modes, change the chat's mode now: Enter, click Whisper or Yell on the input line, then Enter "
    "on the empty line.",
    "This line is said in whatever mode the chat is in right now: whispered, spoken or yelled.",
    "Привет, как дела? Это русский текст.",
    "你好，有人能听到我吗？",
    "This is a long line to see how the chat cuts it into pieces between words: the secret door is behind the painting "
    "in the great hall, the key is under the third stone of the fireplace, and the monster only comes out when the lights "
    "are off, so keep your flashlight charged and stay together.",
    "Last test line. Now stop the helper with Ctrl+C: about five seconds later the chat should say it disconnected.",
]


def auto_speech_items():
    """--auto-speech: [(lang, audio, what is said)] from the benchmark's recordings (computer voices, the webcam-in-a-room
    versions): English lines, one-word callouts, Russian, Chinese, Spanish, German, and two mixed-language lines"""
    lid = os.path.join(os.path.dirname(HERE), 'export', 'asrbench', 'lid', 'items.json')
    if not os.path.exists(lid):
        sys.exit('--auto-speech needs the benchmark recordings: run bench/make_clips.py and lid.py prep first')
    items = {it['id']: it for it in json.load(open(lid, encoding='utf-8'))}
    pick = ['en01_room', 'en04_room', 'en08_room', 'en13_room', 'w000_room', 'w005_room', 'w010_room', 'w015_room',
            'ru02_room', 'ru05_room', 'ru10_room', 'zh03_room', 'zh07_room', 'es02_room', 'de03_room',
            'm08', 'm10', 'm12', 'm14', 'm15', 'm16', 'm19']                 # (mixed: two languages, one word inside, three)
    out = []
    for k in pick:
        it = items.get(k)
        if it:
            x, sr = read_wav(it['path'])
            mixed = it.get('segs')                   # (a mixed line: the "auto" language - detected per stretch)
            out.append(('auto' if mixed else it['lang'], x, it['text'] + ('  (MIXED: %s)' % '+'.join(s['lang'] for s in mixed) if mixed else '')))
    return out


def prompt(st, typing):
    """the status line, shortened, then what is being typed"""
    return (st[:60].ljust(60) + ' | type> ' + ''.join(typing)[-45:]).ljust(118)


def main():
    ap = argparse.ArgumentParser(description='Proximity voice chat STT engine: the helper program for the game (prototype)')
    ap.add_argument('--list', action='store_true', help='list the sound devices')
    ap.add_argument('--device', type=int, help='output device number (default: the Windows default)')
    ap.add_argument('--mic-device', type=int, help='microphone device number (default: the Windows default)')
    ap.add_argument('--mic-wav', metavar='WAV', help='play this recording as the microphone (real time; tests without one)')
    ap.add_argument('--auto-speech', action='store_true', help='no microphone: recorded lines (English, one-word callouts, '
                    'Russian, Chinese, Spanish, German, mixed) played through the REAL speech-to-text as if spoken, each in its '
                    'own language - watch the live words and lines in the game (needs export/asrbench from the benchmark)')
    ap.add_argument('--volume', type=float, default=1.0, help='the helper\'s own volume, 0..1')
    ap.add_argument('--no-mic', action='store_true', help='never open the microphone')
    ap.add_argument('--lang', help='the language spoken (en, ru, zh, es, de, ...; auto) - default: the game\'s setting '
                    '"Language I speak"')
    ap.add_argument('--threads', type=int, default=4, help='CPU threads for the speech models')
    ap.add_argument('--io-dir', help='where the mod looks for the helper\'s files (default: the mods folder / the Workshop folder)')
    ap.add_argument('--transcribe', metavar='WAV', help='run a recording through the speech pipeline as if it came from '
                    'the microphone: print the live words and the lines, and stop')
    ap.add_argument('--demo', action='store_true', help='no game: one voice walks a circle around you')
    ap.add_argument('--seconds', type=float, default=0, help='stop after this long (0 = until Ctrl+C)')
    ap.add_argument('--type', action='store_true', help='no microphone: each line you type here (Enter) goes to the game '
                    'as if you had said it (tests the whole text path without a microphone)')
    ap.add_argument('--auto', action='store_true', help='no microphone, no typing: once the game is running with "Write what '
                    'I say in the chat" On, the test lines below are sent one by one, %d s apart, as if you had said them' % AUTO_GAP)
    args = ap.parse_args()

    if args.transcribe:
        import asr
        audio, sr = read_wav(args.transcribe)
        if sr != asr.RATE:
            g = math.gcd(asr.RATE, sr)
            audio = resample_poly(audio, asr.RATE // g, sr // g).astype(np.float32)
        lst = asr.Listener(lambda u, t, i=None: print('  live %d: %s' % (u, t)),
                           lambda u, t, i: print('LINE %d: %s   [%s; %.1f s of speech; final pass %.2f s]' % (u, t or '(nothing)', i['used'], i['speech'], i['second_s'])),
                           threads=args.threads)
        lst.set_language(args.lang or 'en')
        t0 = time.perf_counter()
        lst.warm()
        print('models ready in %.1f s' % (time.perf_counter() - t0))
        t0 = time.perf_counter()
        audio = np.concatenate([audio, np.zeros(asr.RATE, np.float32)])
        for k in range(0, len(audio), 800):
            lst.feed(audio[k:k + 800])
        lst.flush()
        print('%.1f s of audio in %.1f s' % (len(audio) / asr.RATE, time.perf_counter() - t0))
        return 0

    import sounddevice as sd
    if args.list:
        print(sd.query_devices())
        return 0

    print('preparing the test voices...')
    mixer = Mixer(make_voices())
    mixer.volume = max(0.0, min(1.0, args.volume))

    def callback(outdata, frames, t, st):
        outdata[:] = mixer.render(frames)
    device, extra = args.device, None
    if device is None:                 # (WASAPI: the short output delay; the Windows default API, MME, adds ~100 ms)
        try:
            for api in sd.query_hostapis():
                if 'WASAPI' in api['name'] and api['default_output_device'] >= 0:
                    device, extra = api['default_output_device'], sd.WasapiSettings(auto_convert=True)
        except Exception:
            device, extra = None, None
    try:
        stream = sd.OutputStream(samplerate=RATE, channels=2, dtype='float32', blocksize=480, latency='low',
                                 device=device, extra_settings=extra, callback=callback)
    except Exception as e:             # (that device would not open this way: the Windows default)
        print('WASAPI output failed (%s); using the default output' % e)
        stream = sd.OutputStream(samplerate=RATE, channels=2, dtype='float32', blocksize=480, latency='low',
                                 device=args.device, callback=callback)

    lines = []                         # what to print above the status line

    def log(s):
        lines.append(s)
    link = reader = mic = listener = None
    last_said = ['']
    live_now = ['']                    # (the live words of the line being said)
    ready = [None]                     # (the speech models: None not asked yet, False loading, True ready)
    loaded_lang = [None]
    typing = []                        # (--type: the line being typed)
    keys = args.type and sys.stdin.isatty()

    def send_typed(line):
        line = line.strip()
        if line:
            sent = link.send_text(line)
            last_said[0] = line[:50]
            log('typed%s: %s' % ('' if sent else ' [no game to tell - is a level running?]', line))

    def read_keys():
        """--type in a console: the keys as they come (a thread blocked on reading stdin never got the lines
        while the status line was redrawn); Enter sends, Backspace deletes, Esc clears"""
        changed = False
        while msvcrt.kbhit():
            ch = msvcrt.getwch()
            changed = True
            if ch in ('\x00', '\xe0'):                    # (arrows, function keys: their second half too)
                msvcrt.getwch()
            elif ch in ('\r', '\n'):
                send_typed(''.join(typing))
                typing.clear()
            elif ch == '\x08':
                if typing:
                    typing.pop()
            elif ch == '\x1b':
                typing.clear()
            elif ch == '\x03':
                raise KeyboardInterrupt
            elif ch >= ' ':
                typing.append(ch)
        return changed
    if not args.demo:
        link = Link([args.io_dir] if args.io_dir else io_dirs(), log=log)
        link.start()
        atexit.register(link.stop)
        reader = FeedReader(mixer, link)
        if args.type and not keys:                        # (lines piped in: a test)
            def piped():
                for line in sys.stdin:
                    send_typed(line)
            threading.Thread(target=piped, daemon=True).start()
        elif args.type:
            pass                                          # (keys from the console: read in the loop below)
        elif not args.no_mic and not args.auto:           # (--auto, --type: the lines stand in for the microphone)
            import asr

            def on_live(utt, words, info=None):
                live_now[0] = words
                link.send_msg('l', utt, words, (info or {}).get('times'), (info or {}).get('t0'))

            def on_final(utt, text, info):
                live_now[0] = ''
                sent = link.send_msg('f', utt, text, info.get('times'), info.get('t0'))
                if text:
                    last_said[0] = text[:50]
                    log('you said (%.1f s; %s, final pass %.2f s)%s: %s' % (info['speech'], info['used'], info['second_s'],
                                                                         '' if sent else ' [no game to tell]', text))
                else:
                    log('(%.1f s of sound, no words made out)' % info['speech'])
            listener = asr.Listener(on_live, on_final, threads=args.threads, log=log,
                                    on_start=lambda utt: link.send_msg('s', utt, ''))   # (talking: their head bobs at once)
            if args.auto_speech:                          # (recorded lines in several languages, the real pipeline)
                mic = asr.PlaylistMicrophone(listener, auto_speech_items(), log=log)
            elif args.mic_wav:                              # (a recording instead of the microphone)
                audio, sr = read_wav(args.mic_wav)
                if sr != asr.RATE:
                    g = math.gcd(asr.RATE, sr)
                    audio = resample_poly(audio, asr.RATE // g, sr // g).astype(np.float32)
                mic = asr.WavMicrophone(listener, audio, log=log)
            else:
                mic = asr.Microphone(listener, device=args.mic_device, log=log)

            def warm(lang):
                try:
                    listener.set_language(lang)
                    listener.warm()
                    ready[0] = True
                except Exception as e:
                    log('the speech models could not be loaded: %s' % e)
                    ready[0] = None

    with stream:
        dev = sd.query_devices(stream.device, 'output')
        print('playing on: %s (output delay %.0f ms)' % (dev['name'], stream.latency * 1000))
        if reader:
            print('reading %s' % SAVE)
            print('my files for the game go to: %s' % ', '.join(link.dirs))
            if args.auto:
                print('auto test: %d lines, %d s apart, once a level is running and "Write what I say in the chat" is On '
                      '(in the game: Enter, Settings). Stay in the game and watch the chat.' % (len(AUTO_LINES), AUTO_GAP))
            elif args.type:
                print('type a line here and press Enter: the game says it as if you had spoken it (in the game, '
                      'Settings: "Write what I say in the chat" must be On). The game may pause while this window '
                      'has the focus: click back into the game to see the line arrive.')
            reader.start()
        else:
            print('demo: the speaker walks a circle around you, 4 m away (ahead, right, behind, left)')
        t0 = time.perf_counter()
        last_n, last_t, seq = 0, t0, 0
        last_rate = [0.0]
        auto = [0, None]                  # (--auto: lines sent, when the next one goes)
        auto_live = [None]                # (--auto: the line being "said" - utterance, words, how many sent, next time)
        try:
            while not args.seconds or time.perf_counter() - t0 < args.seconds:
                now = time.perf_counter()
                if args.demo:
                    seq += 1
                    az = ((now - t0) * 45.0 + 180.0) % 360.0 - 180.0          # (a turn in 8 s)
                    mixer.set_feed(dict(seq=seq, vol=1.0, speakers={1: dict(src=2, talk=True, gain=1.0, az=az, el=0.0, muffle=0.0)}))
                    time.sleep(0.05)
                    if seq % 10 == 0:
                        print('\r  direction %4.0f   ' % az, end='', flush=True)
                    continue
                if keys:                   # (typing: keys every 30 ms, the status four times a second)
                    for _ in range(8):
                        time.sleep(0.03)
                        if read_keys():
                            print('\r' + prompt(status(mixer, last_rate[0], mic, last_said[0], live_now[0]), typing), end='', flush=True)
                else:
                    time.sleep(0.25)
                if args.auto and link.mic and mixer.fresh():
                    if auto_live[0]:                                       # (a line being "said": its words so far)
                        utt, words, k, nxt = auto_live[0]
                        if time.perf_counter() >= nxt:
                            k = min(len(words), k + 2)
                            if k < len(words):
                                link.send_msg('l', utt, ' '.join(words[:k]))
                                auto_live[0] = (utt, words, k, time.perf_counter() + 0.35)
                            else:
                                send_typed(' '.join(words))               # (the finished line)
                                auto_live[0] = None
                    elif auto[0] < len(AUTO_LINES):
                        if auto[1] is None:
                            auto[1] = time.perf_counter() + 3              # (the first line 3 s after it is wanted)
                        elif time.perf_counter() >= auto[1]:
                            auto[0] += 1
                            auto[1] = time.perf_counter() + AUTO_GAP
                            log('  (auto line %d of %d: its words arrive live, then the line)' % (auto[0], len(AUTO_LINES)))
                            auto_live[0] = (auto[0], AUTO_LINES[auto[0] - 1].split(), 0, time.perf_counter())
                if mic is not None:    # (the microphone is open only while the game wants it and is running)
                    want = link.mic and mixer.fresh()
                    lang = args.lang or ('en' if args.auto_speech else link.lang)   # (auto speech: each line sets its own)
                    if want and ready[0] is None:                          # (load the models first: downloaded once)
                        ready[0] = False
                        loaded_lang[0] = lang
                        log('loading the speech models for "%s" (the first time they are downloaded, ~1.5 GB)...' % lang)
                        threading.Thread(target=warm, args=(lang,), daemon=True).start()
                    elif ready[0] and lang != loaded_lang[0]:              # (another language: its model in the background)
                        loaded_lang[0] = lang
                        log('language: %s' % lang)
                        threading.Thread(target=warm, args=(lang,), daemon=True).start()
                    listener.live = link.live                             # (the game's "Live words while I talk")
                    if want and ready[0] and not mic.is_open():
                        listener.set_language(lang)
                        if listener.thread is None:
                            listener.start()
                        mic.open()
                    elif not want and mic.is_open():
                        mic.close()
                n = reader.updates
                rate = (n - last_n) / max(1e-6, time.perf_counter() - last_t)
                last_n, last_t = n, time.perf_counter()
                last_rate[0] = rate
                while lines:
                    print('\r' + lines.pop(0).ljust(118))
                st = status(mixer, rate, mic, last_said[0], live_now[0])
                print('\r' + (prompt(st, typing) if keys else st.ljust(118)[:118]), end='', flush=True)
        except KeyboardInterrupt:
            pass
        print()
        if mic is not None:
            mic.close()
        if listener is not None and listener.thread is not None:
            listener.stop()
    if link:
        link.stop()
    return 0


if __name__ == '__main__':
    sys.path.insert(0, HERE)
    sys.exit(main())
