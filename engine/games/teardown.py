"""Teardown, through the mod Proximity Babble Chat (its voice.lua). The link (PROTOCOL.md has the formats):
  game -> Koetama   savegame.xml: savegame.mod.pcvx.f, ~20 times a second - whom the player hears and how (volume,
                     direction, muffle), and what the game wants (the microphone, the language, a ping)
  Koetama -> game   small files next to the mod's folder: pcvx_on (running), pcvx_p<n> (the answer to ping n),
                     pcvx_t<n>.xml (message n: what the player said)
Teardown runs on Windows; on Linux (Steam Deck) through Proton - its files are then inside its Proton prefix.
"""
import glob
import hashlib
import hmac
import json
import math
import os
import re
import subprocess
import sys
import threading
import time
import unicodedata

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


# translation (Rust: kd_common::feed): at most 2 translations, 16 lines of at most 400 bytes, ids of 1-15 digits
MAX_RULES, MAX_REQUESTS, MAX_REQUEST_BYTES = 2, 16, 400
LANG_CODE = re.compile(r'[A-Za-z0-9_-]{1,16}')


def parse_translations(items):
    """[{"from", "to"}] -> [('ja', 'en'), ...]: good codes only, each pair once, the first MAX_RULES"""
    out = []
    for t in items:
        a, b = t.get('from'), t.get('to')
        if (len(out) < MAX_RULES and isinstance(a, str) and isinstance(b, str) and LANG_CODE.fullmatch(a)
                and LANG_CODE.fullmatch(b) and (a, b) not in out):
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


def parse_requests(items):
    """[{"id", "text"}] -> [(id, text)]: an item without a good id (1 to 15 digits' worth, at least 1) skipped; a text
    of more than MAX_REQUEST_BYTES keeps its id with ''; each id once (the first), the first MAX_REQUESTS"""
    out = []
    for r in items:
        rid = _whole(r.get('id'))
        if rid is None or not 1 <= rid <= 999999999999999:
            continue
        if len(out) == MAX_REQUESTS:
            break
        if rid in [i for i, _ in out]:
            continue
        out.append((rid, request_text(r['text'].encode('utf-8'))))
    return out


def _whole(v):
    """a whole number as JSON gives it - also as a decimal (1.0, 1.7e12: Lua JSON libraries write floats) when it is
    whole and fits; None otherwise (Rust: api::whole)"""
    if isinstance(v, bool):
        return None
    if isinstance(v, int):
        return v
    if isinstance(v, float) and v == v and abs(v) < 9.0e15 and v == int(v):
        return int(v)
    return None


def _num(o, k, default):
    v = o.get(k)
    if v is None:
        return default
    if isinstance(v, bool) or not isinstance(v, (int, float)) or v != v or v in (float('inf'), float('-inf')):
        raise ValueError(k)
    return float(v)


def _int(o, k):
    v = o.get(k)
    if v is None:
        return 0
    if _whole(v) is None:
        raise ValueError(k)
    return _whole(v)


def _flag(o, k, default):
    v = o.get(k)
    if v is None:
        return default
    if not isinstance(v, bool):
        raise ValueError(k)
    return v


def _str(o, k):
    v = o.get(k)
    if v is None:
        return ''
    if not isinstance(v, str):
        raise ValueError(k)
    return v


def _list(o, k, most):
    v = o.get(k)
    if v is None or v == {}:                 # ({}: a Lua JSON library's empty table)
        return []
    if not isinstance(v, list) or len(v) > most:
        raise ValueError(k)
    return v


INVISIBLE = ([0x00AD, 0x034F, 0x061C, 0x115F, 0x1160, 0x17B4, 0x17B5, 0x2800, 0x3164, 0xFEFF, 0xFFA0]
             + list(range(0x180B, 0x1810)) + list(range(0x200B, 0x2010)) + list(range(0x202A, 0x202F))
             + list(range(0x2060, 0x2070)) + list(range(0xFE00, 0xFE10)) + list(range(0xE0000, 0xE1000)))
INVISIBLE = frozenset(INVISIBLE)


def id_char_ok(c):
    """a character a player id may hold: no control characters, nothing invisible (Rust: feed::id_char_ok)"""
    return unicodedata.category(c) != 'Cc' and ord(c) not in INVISIBLE


def player_id(v):
    """a player id as JSON gives it: (text, is a number) - a whole number, or a string of 1 to 64 characters without
    control characters; None otherwise (Rust: api::player_id)"""
    if isinstance(v, str):
        if 1 <= len(v) <= 64 and len(v.encode('utf-8')) <= 255 and all(id_char_ok(c) for c in v):
            return (v, False)
        return None
    w = _whole(v)
    return None if w is None else (str(w), True)


def relay_id(room, pid):
    """a player id's 16-bit number in a voice room (Rust: kd_common::feed::relay_id)"""
    h = hashlib.sha256(('koetama id:%s:%s' % (room, pid[0])).encode('utf-8')).digest()
    n = int.from_bytes(h[:2], 'big')
    return 65535 if n == 0 else n


_SEEDS = {}


def room_from_seed(seed):
    """the room and key every Koetama makes from a room_seed (Rust: kd_common::feed::room_from_seed): scrypt first
    (N 2^16, r 8, p 1: slow on purpose - a weak seed is not guessed quickly from the room's name)"""
    if seed not in _SEEDS:
        s = hashlib.scrypt(seed.encode('utf-8'), salt=b'koetama room seed', n=1 << 16, r=8, p=1, maxmem=1 << 27, dklen=32)
        room = hmac.new(s, b'koetama room', hashlib.sha256).hexdigest()[:32]
        _SEEDS[seed] = (room, hmac.new(s, b'koetama key', hashlib.sha256).hexdigest())
    return _SEEDS[seed]


def falloff(d, rng):
    near, far = rng
    if d != d or d >= far:
        return 0.0
    if d <= near or far <= near:
        return 1.0
    left = (far - d) / (far - near)
    return left * left


DEFAULT_RANGE = (10.0, 30.0)


def _vec3(v):
    if v is None:
        return None
    a = [] if v == {} else v
    if not isinstance(a, list) or len(a) != 3:
        return None
    n = [x for x in a if not isinstance(x, bool) and isinstance(x, (int, float)) and math.isfinite(x)]
    return [float(x) for x in n] if len(n) == 3 else None


def _unit(a):
    n = math.sqrt(a[0] * a[0] + a[1] * a[1] + a[2] * a[2])
    return [a[0] / n, a[1] / n, a[2] / n] if n > 1e-9 else None


def _dot(a, b):
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]


def _ears(v):
    """(position, forward, right, up) of a listener object, or None"""
    if not isinstance(v, dict):
        return None
    p = _vec3(v.get('position'))
    parts = [_vec3(v.get(k)) for k in ('forward', 'right', 'up')]
    if p is None or any(x is None for x in parts):
        return None
    units = [_unit(x) for x in parts]
    return None if any(u is None for u in units) else (p, units[0], units[1], units[2])


def _place(ears, p):
    pos, fwd, right, up = ears
    d = [p[0] - pos[0], p[1] - pos[1], p[2] - pos[2]]
    x, y, z = _dot(d, right), _dot(d, up), _dot(d, fwd)
    return math.degrees(math.atan2(x, z)), math.degrees(math.atan2(y, math.sqrt(x * x + z * z))), math.sqrt(_dot(d, d))


def _range(v):
    if v is None or v == {} or not isinstance(v, list) or len(v) != 2:
        return None
    n, f = v
    ok = all(not isinstance(x, bool) and isinstance(x, (int, float)) and math.isfinite(x) for x in (n, f))
    return (float(n), float(f)) if ok and 0 <= n < f else None


def _short(v):
    return ''.join(c for c in v if unicodedata.category(c) != 'Cc')[:64] if isinstance(v, str) else ''


# devices and sound effects (PROTOCOL.md "Devices", "Sound effects"; kd_common::feed::Effects::preset - keep the same)
CLEAN = dict(band=None, drive=0.0, compress=0.0, hiss=0.0, crackle=0.0, squelch=0.0, horn=0.0, lofi=0.0, wobble=0.0,
             pitch=0.0, robot=0.0, echo=None, reverb=0.0, hum=0.0)
PRESETS = {
    'plain': CLEAN,
    'radio': dict(CLEAN, band=(300.0, 3000.0), drive=0.4, compress=0.35, hiss=0.2, crackle=0.1, squelch=0.8, lofi=0.2),
    'loudspeaker': dict(CLEAN, band=(400.0, 5000.0), drive=0.6, compress=0.4, horn=0.7),
    'pa': dict(CLEAN, band=(150.0, 7000.0), drive=0.2, compress=0.5, horn=0.2, reverb=0.5, hum=0.05),
}
DEVICE_RANGE = {'plain': (10.0, 30.0), 'radio': (1.0, 8.0), 'loudspeaker': (5.0, 40.0), 'pa': (10.0, 60.0)}
DEVICE_BACK = {'plain': 1.0, 'radio': 1.0, 'loudspeaker': 0.15, 'pa': 1.0}
SOUND_SPEED, MAX_DELAY = 343.0, 0.5


def directivity(facing, to, back):
    """a horn pointing along facing (unit), heard from `to` (horn -> listener): 1 ahead, back behind"""
    n = math.sqrt(_dot(to, to))
    if n < 1e-9:
        return 1.0
    h = (1.0 + min(1.0, max(-1.0, _dot(facing, to) / n))) / 2.0
    return back + (1.0 - back) * h * h
EFFECT_LIMITS = dict(drive=(0, 1), compress=(0, 1), hiss=(0, 1), crackle=(0, 1), squelch=(0, 1), horn=(0, 1),
                     lofi=(0, 1), wobble=(0, 1), pitch=(-12, 12), robot=(0, 2000), reverb=(0, 1), hum=(0, 1))


def _pair(v):
    if not isinstance(v, list) or len(v) != 2:
        return None
    ok = all(not isinstance(x, bool) and isinstance(x, (int, float)) and math.isfinite(x) for x in v)
    return (float(v[0]), float(v[1])) if ok else None


def _effects(v, base):
    """`effects` on top of a preset: each field given replaces the preset's (0, false or null: off)"""
    if v is None:
        return dict(base)
    if not isinstance(v, dict):
        raise ValueError('effects')
    out = dict(base)
    for k, (lo, hi) in EFFECT_LIMITS.items():
        name = 'static' if k == 'hiss' else k
        if name in v and (v[name] is False or v[name] is None):
            out[k] = 0.0
        elif name in v:
            out[k] = float(min(hi, max(lo, _num(v, name, base[k]))))
    if 'band' in v:
        if v['band'] is None or v['band'] is False:
            out['band'] = None
        else:
            p = _pair(v['band'])
            if p is None:
                raise ValueError('band')
            lo, hi = min(20000.0, max(20.0, p[0])), min(20000.0, max(20.0, p[1]))
            if lo >= hi:
                raise ValueError('band')
            out['band'] = (lo, hi)
    if 'echo' in v:
        e = v['echo']
        if e is None or e is False:
            out['echo'] = None
        elif not isinstance(e, bool) and isinstance(e, (int, float)):
            d = float(e) if math.isfinite(e) else 0.0
            out['echo'] = (min(1.0, max(0.02, d)), 0.35) if d > 0 else None
        else:
            p = _pair(e)
            if p is None:
                raise ValueError('echo')
            out['echo'] = (min(1.0, max(0.02, p[0])), min(0.9, max(0.0, p[1]))) if p[0] > 0 else None
    return out


def _via(v, ears):
    """a speaker's `via`: each device's places as this player hears them"""
    if v is None:
        return []
    a = [] if v == {} else v
    if not isinstance(a, list) or len(a) > 8:
        raise ValueError('via')
    out = []
    for d in a:
        if not isinstance(d, dict):
            raise ValueError('via')
        dev = d['device'] if d.get('device') is not None else 'plain'
        if not isinstance(dev, str) or dev not in PRESETS:
            raise ValueError('device')
        places = [_vec3(d['position'])] if _vec3(d.get('position')) else []
        if d.get('positions') is not None:
            ps = [] if d['positions'] == {} else d['positions']
            if not isinstance(ps, list) or len(ps) > 16:
                raise ValueError('positions')
            for p in ps:
                if _vec3(p) is None:
                    raise ValueError('positions')
                places.append(_vec3(p))
        places = places[:16]
        rng = _range(d.get('range')) or DEVICE_RANGE[dev]
        given = d.get('gain') is not None
        g = min(1.0, max(0.0, _num(d, 'gain', 1.0)))
        facing = _unit(_vec3(d.get('facing'))) if _vec3(d.get('facing')) else None
        back = min(1.0, max(0.0, _num(d, 'back', DEVICE_BACK[dev])))
        if ears and places:
            placed = [_place(ears, p) for p in places]
            nearest = min(p[2] for p in placed)
            outs = []
            for (az, el, dist), p in zip(placed, places):
                to = [ears[0][0] - p[0], ears[0][1] - p[1], ears[0][2] - p[2]]
                dg = directivity(facing, to, back) if facing else 1.0
                outs.append(dict(az=az, el=el, gain=g if given else falloff(dist, rng) * dg,
                                 delay=min(MAX_DELAY, max(0.0, (dist - nearest) / SOUND_SPEED))))
        else:
            outs = [dict(az=_num(d, 'azimuth', 0.0), el=_num(d, 'elevation', 0.0),
                         gain=g if (given or not places) else 0.0, delay=0.0)]
        out.append(dict(device=dev, outs=outs, muffle=min(1.0, max(0.0, _num(d, 'muffle', 0.0))),
                        signal=min(1.0, max(0.0, _num(d, 'signal', 1.0))),
                        effects=_effects(d.get('effects'), PRESETS[dev])))
    return out


def _parse_one(v, inherit=None):
    """one feed (the top one, or a hub's player's: inherit = the host's (room, key, region)) -> a dict; raises
    ValueError for a value of the wrong kind"""
    if not isinstance(v, dict):
        raise ValueError('not an object')
    listen = _str(v, 'listen')
    if listen not in ('', 'off', 'always', 'push_to_talk'):
        raise ValueError('listen')
    lang = _str(v, 'lang') or 'en'
    if not LANG_CODE.fullmatch(lang):
        raise ValueError('lang')
    seed = _str(v, 'room_seed')
    if seed and len(seed) <= 256:
        room, key = room_from_seed(seed)
    elif 'room' in v or 'key' in v or inherit is None:
        room, key = _str(v, 'room'), _str(v, 'key')
    else:
        room, key = inherit[0], inherit[1]
    region = _str(v, 'region') if 'region' in v else (inherit[2] if inherit else '')
    raw_me = v.get('me') if v.get('me') is not None else (v.get('id') if inherit is not None else None)
    me_id = player_id(raw_me) if raw_me is not None else None
    if not (re.fullmatch(r'[0-9a-f]{32}', room) and re.fullmatch(r'[0-9a-f]{64}', key) and me_id):
        room, key, me, me_id = '', '', 0, None
    else:
        me = relay_id(room, me_id)
    if not room or region not in REGIONS:            # (a room's region: one of the relay's, else wherever)
        region = ''
    ears = None
    if v.get('listener') is not None:
        ears = _ears(v['listener'])
        if ears is None:
            raise ValueError('listener')
    my_range = _range(v.get('range'))
    speakers, clashes = {}, []
    for sp in _list(v, 'speakers', 256):
        if not isinstance(sp, dict):
            raise ValueError('speaker')
        pid = player_id(sp.get('id')) if sp.get('id') is not None else None
        if pid is None:
            raise ValueError('speaker id')
        tv = sp.get('test_voice')
        if tv is not None and (_whole(tv) is None or _whole(tv) < 1):
            raise ValueError('test_voice')
        rid = relay_id(room, pid)
        if (rid in speakers and speakers[rid]['id'] != pid) or (me and rid == me and me_id != pid):
            clashes.append(pid)
            continue
        placed = _place(ears, _vec3(sp['position'])) if ears and _vec3(sp.get('position')) else None
        own = _range(sp.get('range'))
        via = _via(sp.get('via'), ears)
        given = sp.get('gain') is not None
        directed = any(sp.get(k) is not None for k in ('position', 'azimuth', 'elevation'))
        if given:
            gain = min(1.0, max(0.0, _num(sp, 'gain', 1.0)))
        elif placed:
            gain = falloff(placed[2], own or my_range or DEFAULT_RANGE)
        elif via and not directed:
            gain = 0.0                                  # (heard only through their devices)
        else:
            gain = 1.0
        az, el = (placed[0], placed[1]) if placed else (_num(sp, 'azimuth', 0.0), _num(sp, 'elevation', 0.0))
        speakers[rid] = dict(src=_whole(tv) if tv is not None else 0, talk=_flag(sp, 'talking', False), gain=gain,
                             az=az, el=el, muffle=min(1.0, max(0.0, _num(sp, 'muffle', 0.0))), id=pid,
                             name=_short(sp.get('name')), distance=placed[2] if placed else None, gain_given=given,
                             range=own, via=via, effects=_effects(sp.get('effects'), CLEAN))
    ids = []
    if v.get('to') is None:
        if my_range and ears:
            for rid, s in sorted(speakers.items()):
                if s['src'] == 0 and s['distance'] is not None and s['distance'] <= my_range[1] * 1.1 and rid != me \
                        and rid not in ids and len(ids) < 64:
                    ids.append(rid)
    else:
        for i in _list(v, 'to', 256):
            pid = player_id(i)
            if pid is None:
                raise ValueError('to')
            r = relay_id(room, pid)
            if r != me and r not in ids and len(ids) < 64:
                ids.append(r)
    transmit_all = False                                # (into a device: to these players too, or to everyone)
    t = v.get('transmit')
    if t is True:
        transmit_all = True
    elif t is not None and t is not False:
        for i in _list(v, 'transmit', 256):
            pid = player_id(i)
            if pid is None:
                raise ValueError('transmit')
            r = relay_id(room, pid)
            if r != me and r not in ids and len(ids) < 64:
                ids.append(r)
    pairs = _list(v, 'translations', 16)
    if not all(isinstance(t, dict) and isinstance(t.get('from'), str) and isinstance(t.get('to'), str) for t in pairs):
        raise ValueError('translations')
    lines = _list(v, 'to_translate', 64)
    if not all(isinstance(r, dict) and isinstance(r.get('text'), str) for r in lines):
        raise ValueError('to_translate')
    return dict(seq=_int(v, 'seq'), vol=min(1.0, max(0.0, _num(v, 'volume', 1.0))), sid=_int(v, 'session'),
                ack=_int(v, 'ack'), ping=_int(v, 'ping'), mic=listen in ('always', 'push_to_talk'),
                ptt=_flag(v, 'talk_key', False) if listen == 'push_to_talk' else None, lang=lang,
                live=_flag(v, 'live', True), speakers=speakers, room=room, key=key, me=me, to=ids, region=region,
                translations=parse_translations(pairs), to_translate=parse_requests(lines), me_id=me_id,
                name=_short(v.get('name')), range=my_range, clashes=clashes, transmit_all=transmit_all, players=[])


def parse_feed(text):
    """the feed (PROTOCOL.md "Game -> Koetama: the feed"): the JSON object, or its hex (Teardown's registry string)
    -> a dict, or None. Missing fields: their defaults; a value of the wrong kind: None; a bad room, key or me: no
    room ('', '', 0); bad ids in `to` skipped; bad translations and lines skipped; a hub's players: a dict each"""
    try:
        t = text.strip()
        v = json.loads(t if t.startswith('{') else bytes.fromhex(t).decode('utf-8'))
        top = _parse_one(v)
        for p in _list(v, 'players', 32):
            if not isinstance(p, dict):
                return None
            sub = _parse_one(p, (top['room'], top['key'], top['region']))
            pid = player_id(p['id']) if p.get('id') is not None else None
            if pid:
                sub['me'], sub['me_id'] = (relay_id(sub['room'], pid) if sub['room'] else 0), pid
            if sub['me_id'] and not any(q['me_id'] == sub['me_id'] for q in top['players']):
                top['players'].append(sub)
        return top
    except (ValueError, UnicodeDecodeError, AttributeError):
        return None


def find_feeds(data):
    """[(the mod's tag, the feed string)] for every copy of the mod in a savegame.xml: 'local-proximity-chat'
    (the mods folder) and 'steam-<id>' (the Workshop) each have their own"""
    out = []
    for m in FEED.finditer(data):
        tags = MODTAG.findall(data, 0, m.start())
        out.append((tags[-1].decode('ascii', 'replace') if tags else '', m.group(1).decode('utf-8', 'replace')))
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
PROTOCOL = 2


def _js(o):
    return json.dumps(o, ensure_ascii=False, separators=(',', ':'))


def json_secs(x):
    """seconds as JSON, to 1/100 s, halves away from zero (Rust's f64::round), no ".0" (never NaN or infinite: 0)"""
    if x != x or x in (float('inf'), float('-inf')):
        return '0'
    r = math.copysign(math.floor(abs(x) * 100 + 0.5), x) / 100
    t = repr(r + 0.0)
    return t[:-2] if t.endswith('.0') else t


def hello(features):
    return '{"type":"hello","app":%s,"version":%s,"protocol":%d,"features":%s}' % (
        _js(paths.APP_NAME), _js(paths.VERSION), PROTOCOL, _js(list(features)))


def speech(kind, utt, text, times=None, ago=None):
    """what the player said: kind "s" start, "l" live, "f" final; "r": the session's voice room (text room:key)"""
    if kind == 'r':
        r, _, k = text.partition(':')
        return room(r, k)
    if kind == 's':
        return '{"type":"speech","kind":"start","utt":%d}' % utt
    line = '{"type":"speech","kind":"%s","utt":%d,"text":%s' % ('live' if kind == 'l' else 'final', utt, _js(text))
    if times is not None and ago is not None:
        line += ',"times":[%s],"ago":%s' % (','.join(json_secs(t) for t in times), json_secs(max(0.0, ago)))
    return line + '}'


def room(r, k):
    return '{"type":"room","room":%s,"key":%s}' % (_js(r), _js(k))


def _pid(p):
    """a player id (text, is a number) as JSON: as the game gave it"""
    return p[0] if p[1] else _js(p[0])


def voice(state, players=()):
    return '{"type":"voice","state":%s,"players":[%s]}' % (_js(state), ','.join(_pid(p) for p in players))


def talking(pid, on):
    return '{"type":"talking","id":%s,"talking":%s}' % (_pid(pid), 'true' if on else 'false')


def status(speech, microphone):
    return '{"type":"status","speech":%s,"microphone":%s}' % (_js(speech), _js(microphone))


def translation(rid, text, rule=None):
    if rule and text:
        return '{"type":"translation","id":%d,"text":%s,"from":%s,"to":%s}' % (rid, _js(text), _js(rule[0]), _js(rule[1]))
    return '{"type":"translation","id":%d,"text":%s}' % (rid, _js(text))


def join_code(pid, code):
    return '{"type":"join_code","player":%s,"code":%s}' % (_pid(pid), _js(code))


def player(pid, joined):
    return '{"type":"player","player":%s,"joined":%s}' % (_pid(pid), 'true' if joined else 'false')


def translations_status(states):
    """states: [(from, to, state, progress 0..1)]"""
    items = []
    for a, b, st, prog in states:
        extra = ',"progress":%s' % json_secs(min(1.0, max(0.0, prog))) if st == 'downloading' else ''
        items.append('{"from":%s,"to":%s,"state":%s%s}' % (_js(a), _js(b), _js(st), extra))
    return '{"type":"translations_status","translations":[%s]}' % ','.join(items)


def object_prefab(obj):
    """the file the game Spawns to read an object: a body whose tag j holds the object's hex"""
    return '<prefab version="1.5.2">\n\t<body tags="pcvx j=%s"/>\n</prefab>\n' % obj.encode('utf-8').hex()


class Link:
    """Koetama's files for the game: pcvx_on, the answer to each ping, numbered object files"""
    PREFIX = 'pcvx_'

    def __init__(self, dirs, log=print, features=('speech', 'voices')):
        self.dirs, self.log = dirs, log
        self.dir = None           # where the live copy of the mod looks (known from its first feed)
        self.sid = self.ping = None
        self.n = 0                # the last text number written
        self.pending = {}         # number -> path, until the game acks it
        self.mic = False
        self.lang = 'en'
        self.live = True
        self.voice = 'off'        # the voice chat's state as last told (a new session hears it after its hello)
        self.features = list(features)
        self.lock = threading.Lock()

    MARKS = ['on']

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
        """old files swept (an older Koetama's v<n>, vc, vx too); <prefix>on (running) written"""
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
            new_session = feed['sid'] != self.sid or d != self.dir
            if new_session:                                         # (a new level, or another copy of the mod)
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
        if new_session:                         # (the session's first object; then the voice chat's state, if any)
            self._write(hello(self.features))
            if self.voice != 'off':
                self._write(voice(self.voice))

    def set_voice(self, state):
        """the voice chat's state for the game (a voice object when it changes)"""
        if state != self.voice:
            self.voice = state
            self._write(voice(state))

    def send_text(self, text):
        """hand a finished line to the game; False if no game is listening"""
        text = text.strip()
        return bool(text) and self.send_msg('f', 0, text)

    def send_translation(self, rid, text):
        """the translation of line rid ("" = nothing to show); False if no game is listening"""
        return self._write(translation(rid, text.strip()[:TRANSLATION_MAX]))

    def send_translations_state(self, states):
        """the translations' states, [(from, to, state, progress)]; False if no game is listening"""
        return self._write(translations_status(states))

    def send_msg(self, kind, utt, text, times=None, t0=None):
        """hand what the player said to the game: kind "s" (they started talking), "l" (the live words so far), "f"
        (the finished line; "" = nothing made out) or "r" (a voice room, "<room>:<key>"); times: each unit's start (s
        after t0, time.perf_counter() when the line's audio began) - sent with how long ago t0 is now. False if no game
        is listening"""
        import asr
        text = text.strip()[:TEXT_MAX]
        if times is not None:                                # (cut with the text: the first n units keep their times)
            n = len(asr.units(text))
            times = list(times)[:n] if len(times) >= n else None
        if kind == 'l' and not text:
            return False
        ago = (time.perf_counter() - t0) if (times is not None and t0 is not None) else None
        return self._write(speech(kind, utt, text, times if ago is not None else None, ago))

    def _write(self, obj):
        """object n of the session (written whole), kept until the game acks it"""
        with self.lock:
            if self.dir is None:
                return False
            self.n += 1
            path = self._path(self.dir, 't%d.xml' % self.n)
            tmp = self._path(self.dir, 'w%d.tmp' % self.n)
            with open(tmp, 'w', encoding='utf-8') as f:
                f.write(object_prefab(obj))
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
