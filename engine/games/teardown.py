"""Teardown, through the mod Proximity Babble Chat (its voice.lua). The link (PROTOCOL.md has the formats):
  game -> Koetama   savegame.xml: savegame.mod.pcvx.f, ~20 times a second - whom the player hears and how (volume,
                     direction, muffle), and what the game wants (the microphone, the language, a ping)
  Koetama -> game   small files next to the mod's folder: pcvx_on (running), pcvx_p<n> (the answer to ping n),
                     pcvx_t<n>.xml (message n: what the player said)
Teardown runs on Windows; on Linux (Steam Deck) through Proton - its files are then inside its Proton prefix.
"""
import glob
import os
import re
import subprocess
import sys
import threading
import time

import paths
import steam
from audio import load_wav
from games.base import Game

APPID = 1167630
TEXT_MAX = 400         # characters of one text file
TRANSLATION_MAX = 1000  # characters of one translation (kind "x": a 400-byte line can come back longer)
FEED = re.compile(rb'<pcvx>\s*<f\s+value="([^"]*)"\s*/>\s*</pcvx>')
MODTAG = re.compile(rb'<((?:local|steam)-[^\s/>]+)>')

# the three test voices of the mod's voice dummies (/dummy voice): (Windows voice, speaking rate -10..10, what it says)
VOICES = [
    ('Microsoft Zira Desktop', -1, 'I am the whisperer. Stay close, or you will not hear me at all. '
     'One, two, three, four, five, six, seven, eight, nine, ten.'),
    ('Microsoft David Desktop', 0, 'I am the speaker. This is my normal voice, and it carries a fair distance. '
     'Monday, Tuesday, Wednesday, Thursday, Friday, Saturday, Sunday.'),
    ('Microsoft Zira Desktop', 2, 'I am the yeller! You can hear me from far away! '
     'Red, orange, yellow, green, blue, purple, black and white!'),
]
NAMES = {1: 'whisperer', 2: 'speaker', 3: 'yeller'}
WORK = os.path.join(paths.DATA, 'voices') if paths.FROZEN else os.path.join(paths.APP_ROOT, 'export', 'voicehelper')


# ---------------------------------------------------------------- where Teardown's files are
def teardown_user():
    """the Windows user folder Teardown sees: this one on Windows; inside its Proton prefix on Linux"""
    if sys.platform == 'win32':
        return None
    return steam.proton_user(APPID)


def savegame_path():
    if os.environ.get('SAVEPROBE_DIR'):                     # (tests: a fake game's folder)
        return os.path.join(os.environ['SAVEPROBE_DIR'], 'savegame.xml')
    if sys.platform == 'win32':
        return os.path.join(os.environ.get('LOCALAPPDATA', ''), 'Teardown', 'savegame.xml')
    user = teardown_user()
    return os.path.join(user, 'AppData', 'Local', 'Teardown', 'savegame.xml') if user else ''


def io_dirs():
    """the folders a running copy of the mod looks in (MOD/../): the local mods folder, then the Workshop's"""
    if os.environ.get('HFP_MODS'):                          # (tests)
        return [os.environ['HFP_MODS']]
    if sys.platform == 'win32':
        docs = steam.documents_dir()
    else:
        user = teardown_user()
        docs = os.path.join(user, 'Documents') if user else os.path.join(os.path.expanduser('~'), 'Documents')
    dirs = [os.path.join(docs, 'Teardown', 'mods')]
    ws = steam.workshop_dir(APPID)
    if ws:
        dirs.append(ws)
    return dirs


# ---------------------------------------------------------------- the feed (game -> Koetama)
def voice_id(s):
    """a player id as the feed writes it: 1 to 5 ASCII digits, 1..65535 (else None)"""
    return int(s) if re.fullmatch(r'[0-9]{1,5}', s) and 1 <= int(s) <= 65535 else None


# the regions a voice room can be asked to live in (the relay's location hints)
REGIONS = ('wnam', 'enam', 'sam', 'weur', 'eeur', 'apac', 'apac-ne', 'apac-se', 'oc', 'afr', 'me')


# the feed versions this reads, each announced by a <prefix>v<n> file next to <prefix>on (Rust: files.rs FEED_VERSIONS)
FEED_VERSIONS = (5, 6)

# translation (version 6; Rust: kd_common::feed): at most 2 translations, 16 lines of at most 400 bytes, ids of 1-15 digits
MAX_RULES, MAX_REQUESTS, MAX_REQUEST_BYTES = 2, 16, 400
LANG_CODE = re.compile(r'[A-Za-z0-9_-]{1,16}')


def parse_translations(s):
    """'ja>en,ko>en' -> [('ja', 'en'), ('ko', 'en')]: good codes only, each rule once, the first MAX_RULES"""
    out = []
    for item in s.split(',') if s else ():
        a, sep, b = item.partition('>')
        if len(out) < MAX_RULES and sep and LANG_CODE.fullmatch(a) and LANG_CODE.fullmatch(b) and (a, b) not in out:
            out.append((a, b))
    return out


def request_text(b):
    """a line to translate: good UTF-8 of at most MAX_REQUEST_BYTES, else '' (answered '')"""
    if len(b) > MAX_REQUEST_BYTES:
        return ''
    try:
        return b.decode('utf-8')
    except UnicodeDecodeError:
        return ''


def parse_requests(s):
    """'<id>:<hex of the UTF-8 text>;...' -> [(id, text)]: an item without a good id (1 to 15 ASCII digits, at least
    1) skipped; a bad hex or text keeps its id with ''; each id once (the first), the first MAX_REQUESTS"""
    out = []
    for item in s.split(';'):
        rid, sep, hx = item.partition(':')
        if not item or not sep or not re.fullmatch(r'[0-9]{1,15}', rid) or int(rid) < 1:
            continue
        if len(out) == MAX_REQUESTS:
            break
        if int(rid) in [i for i, _ in out]:
            continue
        good = len(hx) % 2 == 0 and re.fullmatch(r'[0-9a-fA-F]*', hx)
        out.append((int(rid), request_text(bytes.fromhex(hx)) if good else ''))
    return out


def parse_feed(text):
    """'6|seq|volume|session|ack|ping|mic|lang|live|room|key|me|to|region|translations|requests|id,src,talk,gain,az,el,muffle;...'
    (version 5: no translations or requests; 4, 3 and 2: no room either) -> a dict, or None. A bad room, key or id: no room
    ('', '', 0); bad ids in `to` are skipped (each kept once, at most 64); malformed translations and requests skipped
    (parse_translations, parse_requests)"""
    try:
        parts = text.split('|')
        live = '1'
        room = key = me = to = region = translations = requests = ''
        if parts[0] == '6' and len(parts) == 17:
            _, seq, vol, sid, ack, ping, mic, lang, live, room, key, me, to, region, translations, requests, rest = parts
        elif parts[0] == '5' and len(parts) == 15:
            _, seq, vol, sid, ack, ping, mic, lang, live, room, key, me, to, region, rest = parts
        elif parts[0] == '4' and len(parts) == 10:
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
        # the voice room (version 5): the room, its key and my id all good, or no room
        me = voice_id(me)
        if not (re.fullmatch(r'[0-9a-f]{32}', room) and re.fullmatch(r'[0-9a-f]{64}', key) and me):
            room, key, me = '', '', 0
        if not room or region not in REGIONS:            # (a room's region: one of the relay's, else wherever)
            region = ''
        ids = []
        for i in (voice_id(x) for x in to.split(',')) if to else ():
            if i and i not in ids and len(ids) < 64:
                ids.append(i)
        # mic: 0 off, 1 listen (voice detection), 2 / 3 push to talk with the key up / held (ptt; None: no push to talk)
        return dict(seq=int(seq), vol=float(vol), sid=int(sid), ack=int(ack), ping=int(ping), mic=mic in ('1', '2', '3'),
                    ptt={'2': False, '3': True}.get(mic), lang=lang or 'en', live=live != '0', speakers=speakers,
                    room=room, key=key, me=me, to=ids, region=region, translations=parse_translations(translations),
                    to_translate=parse_requests(requests))
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


if sys.platform == 'win32':
    import ctypes
    import msvcrt
    from ctypes import wintypes
    _k32 = ctypes.WinDLL('kernel32', use_last_error=True)
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
else:
    def read_shared(path):
        try:
            with open(path, 'rb') as f:
                return f.read()
        except OSError:
            return None


class FeedReader(threading.Thread):
    """polls savegame.xml; a feed that changes is live: on_feed(feed, tag)"""
    def __init__(self, on_feed, path, poll=0.01):
        super().__init__(daemon=True)
        self.on_feed, self.path, self.poll = on_feed, path, poll
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
                    self.on_feed(feed, tag)

    def run(self):
        while self.running:
            try:
                self.once()
            except OSError:
                pass
            time.sleep(self.poll)


# ---------------------------------------------------------------- the link (Koetama -> game)
def times_hex(times):
    """unit start times (s) as the tag w: 4 hex digits each, in 1/100 s (up to 655 s)"""
    return ''.join('%04x' % max(0, min(0xFFFF, int(round(t * 100)))) for t in times)


def text_prefab(text, kind='f', utt=0, times=None, ago=None):
    """the file the game Spawns to read a message: a body whose tags are its kind ("s" started talking, "l" the live
    words so far, "f" the finished line), the utterance it belongs to, t = the hex of the UTF-8 text, and when there
    are word times: w = each unit's start (times_hex; s after the line's audio began) and a = how long ago that was,
    in 1/100 s, when the file was written (the game turns it into a moment on its own clock)"""
    extra = ''
    if times is not None and ago is not None:
        extra = ' w=%s a=%d' % (times_hex(times), max(0, int(round(ago * 100))))
    return '<prefab version="1.5.2">\n\t<body tags="pcvx k=%s u=%d t=%s%s"/>\n</prefab>\n' % (
        kind, utt, text.encode('utf-8').hex(), extra)


class Link:
    """Koetama's files for the game: pcvx_on, the answer to each ping, numbered text files"""
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

    MARKS = ['on'] + ['v%d' % v for v in FEED_VERSIONS]

    def _path(self, d, name):
        return os.path.join(d, self.PREFIX + name)

    def _remove(self, path):
        try:
            os.remove(path)
        except OSError:
            pass

    def _sweep(self, d, keep_on=False):
        marks = [self.PREFIX + n for n in self.MARKS]
        for path in glob.glob(self._path(d, '*')):
            if not (keep_on and os.path.basename(path) in marks):
                self._remove(path)

    def start(self):
        """old files swept; <prefix>on (running) and <prefix>v<n> (each feed version this reads) written"""
        for d in self.dirs:
            if os.path.isdir(d):
                self._sweep(d)
                for name in self.MARKS:
                    with open(self._path(d, name), 'w') as f:
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

    def send_translation(self, rid, text):
        """the translation of request rid (version 6, kind "x"; "" = nothing to show); False if no game is listening"""
        return self._write('x', rid, text.strip()[:TRANSLATION_MAX])

    def send_translations_state(self, text):
        """the translations' states (kind "d": "ja>en=ready,ko>en=downloading 42"); False if no game is listening"""
        return self._write('d', 0, text)

    def send_msg(self, kind, utt, text, times=None, t0=None):
        """hand a message to the game: kind "s" (the player started talking: no text yet), "l" (the live words so far)
        or "f" (the finished line; "" = nothing made out: the live words go); times: each unit's start (s after t0,
        time.perf_counter() when the line's audio began) - written with how long ago t0 is now. False if no game is
        listening"""
        import asr
        text = text.strip()[:TEXT_MAX]
        if times is not None:                                # (cut with the text: the first n units keep their times)
            n = len(asr.units(text))
            times = list(times)[:n] if len(times) >= n else None
        if kind == 'l' and not text:
            return False
        return self._write(kind, utt, text, times, t0)

    def _write(self, kind, utt, text, times=None, t0=None):
        """one numbered message file (written whole), kept until the game acks it"""
        with self.lock:
            if self.dir is None:
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


# ---------------------------------------------------------------- the test voices
def make_voices():
    """the three test voices as mono float32 at audio.RATE: made once with the Windows computer voices (none on other
    systems - the dummies are then silent)"""
    if sys.platform != 'win32':
        return {}
    os.makedirs(WORK, exist_ok=True)
    clips = {}
    for i, (voice, rate, text) in enumerate(VOICES, 1):
        path = os.path.join(WORK, 'voice%d.wav' % i)
        if not os.path.exists(path):
            ps = ("Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; "
                  "try { $s.SelectVoice('%s') } catch {}; $s.Rate = %d; $s.SetOutputToWaveFile('%s'); $s.Speak('%s'); $s.Dispose()"
                  % (voice, rate, path, text.replace("'", "''")))
            try:
                subprocess.run(['powershell', '-NoProfile', '-Command', ps], check=True,
                               creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
            except (OSError, subprocess.CalledProcessError):
                continue
        try:
            clips[i] = load_wav(path)
        except (OSError, ValueError):
            pass
    return clips


# ---------------------------------------------------------------- the game module
class Teardown(Game):
    id = 'teardown'
    name = 'Teardown'
    needs = 'the Proximity Babble Chat mod (Steam Workshop)'

    def __init__(self, mixer, log=print, save=None, dirs=None):
        super().__init__(mixer, log)
        self.save = save or savegame_path()
        self.link = Link(dirs or io_dirs(), log)
        self.reader = None

    def locate(self):
        inst = steam.install_dir(APPID)
        if inst:
            return True, inst
        if self.save and os.path.exists(self.save):
            return True, os.path.dirname(self.save)
        return False, 'Teardown was not found in your Steam libraries'

    def start(self):
        self.link.start()
        self.reader = FeedReader(self._feed, self.save)
        self.reader.start()

    def _feed(self, feed, tag):
        self.on_feed(feed)
        self.link.on_feed(feed, tag)

    def stop(self):
        if self.reader:
            self.reader.running = False
        self.link.stop()

    def send(self, kind, utt, text, times=None, t0=None):
        return self.link.send_msg(kind, utt, text, times, t0)

    def test_clips(self):
        return make_voices()

    def speaker_name(self, src):
        return NAMES.get(src, str(src))
