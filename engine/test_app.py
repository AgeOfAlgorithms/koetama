"""Offline tests of the app's parts around the engine: the updater, finding Steam games, the game list, the runtime's
states with a stand-in game, and the mixer's low-pass (numpy) against a plain one-pole filter. No network, no sound
device.
    python engine/test_app.py        (conda env "pcvoice")
"""
import json
import math
import os
import sys
import tempfile

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import audio      # noqa: E402
import paths      # noqa: E402
import runtime    # noqa: E402
import steam      # noqa: E402
import updater    # noqa: E402
from games import GAMES, by_id            # noqa: E402
from games.base import Game               # noqa: E402

FAILED = NCHECK = 0


def check(cond, msg):
    global FAILED, NCHECK
    NCHECK += 1
    if not cond:
        FAILED += 1
    print(('ok   ' if cond else 'FAIL ') + msg)


# ---- the updater
check(updater.version_tuple('v1.2.10') == (1, 2, 10) and updater.newer('v0.2.0', '0.1.9') and not updater.newer('v0.1.0', '0.1.0')
      and updater.newer('v0.10.0', '0.9.9'), 'versions compare as numbers (0.10 after 0.9), a tag\'s v ignored')
REL = {'tag_name': 'v9.9.9', 'body': 'notes', 'html_url': 'https://example/rel', 'assets': [
    {'name': 'Kotodama-Setup-9.9.9.exe', 'browser_download_url': 'URL_EXE'},
    {'name': 'Kotodama-9.9.9-linux.tar.gz', 'browser_download_url': 'URL_TGZ'},
    {'name': 'SHA256SUMS.txt', 'browser_download_url': 'URL_SUMS'}]}
real_get = updater._get
updater._get = lambda url, timeout=10: json.dumps(REL).encode()
info = updater.check()
check(info and info['version'] == '9.9.9' and info['installer'] == 'Kotodama-Setup-9.9.9.exe' and info['installer_url'] == 'URL_EXE'
      and info['sums_url'] == 'URL_SUMS', 'a newer release: its installer and checksums found')
REL['tag_name'] = 'v' + paths.VERSION
check(updater.check() is None, 'the same version: no update')
REL['tag_name'], REL['draft'] = 'v9.9.9', True
check(updater.check() is None, 'a draft release: no update')
REL['draft'] = False
with tempfile.TemporaryDirectory() as td:
    exe = os.path.join(td, 'Kotodama-Setup-9.9.9.exe')
    open(exe, 'wb').write(b'installer bytes')
    good = updater.sha256(exe)
    sums = os.path.join(td, 'SHA256SUMS.txt')
    open(sums, 'w').write('%s  Kotodama-Setup-9.9.9.exe\n%s  other.tar.gz\n' % (good, '0' * 64))
    check(updater.expected_sha(open(sums).read(), 'Kotodama-Setup-9.9.9.exe') == good, 'SHA256SUMS.txt read')
    url = lambda p: 'file:///' + p.replace('\\', '/')                     # noqa: E731
    updater._get = real_get
    info = dict(installer='Kotodama-Setup-9.9.9.exe', installer_url=url(exe), sums_url=url(sums))
    path = updater.download(info)
    check(open(path, 'rb').read() == b'installer bytes', 'the installer downloads and matches its checksum')
    open(sums, 'w').write('%s  Kotodama-Setup-9.9.9.exe\n' % ('f' * 64))
    try:
        updater.download(info)
        bad = False
    except ValueError:
        bad = True
    check(bad, 'a download that does not match its checksum is refused')

# ---- finding a Steam game (a stand-in Steam folder: two libraries, the game in the second)
with tempfile.TemporaryDirectory() as td:
    root = os.path.join(td, 'Steam')
    lib2 = os.path.join(td, 'Games')
    os.makedirs(os.path.join(root, 'steamapps'))
    os.makedirs(os.path.join(lib2, 'steamapps', 'common', 'Teardown'))
    os.makedirs(os.path.join(lib2, 'steamapps', 'workshop', 'content', '1167630'))
    os.makedirs(os.path.join(lib2, 'steamapps', 'compatdata', '1167630', 'pfx', 'drive_c', 'users', 'steamuser'))
    open(os.path.join(root, 'steamapps', 'libraryfolders.vdf'), 'w').write(
        '"libraryfolders"\n{\n\t"0"\n\t{\n\t\t"path"\t\t"%s"\n\t}\n\t"1"\n\t{\n\t\t"path"\t\t"%s"\n\t}\n}\n'
        % (root.replace('\\', '\\\\'), lib2.replace('\\', '\\\\')))
    open(os.path.join(lib2, 'steamapps', 'appmanifest_1167630.acf'), 'w').write('"AppState"\n{\n\t"installdir"\t\t"Teardown"\n}\n')
    libs = steam.libraries(root)
    check(len(libs) == 2 and os.path.normcase(libs[1]) == os.path.normcase(lib2), 'Steam\'s library folders read (%d)' % len(libs))
    check(os.path.normcase(steam.app_library(1167630, root)) == os.path.normcase(lib2)
          and steam.install_dir(1167630, root).endswith(os.path.join('common', 'Teardown')), 'the game found in the second library')
    check(steam.workshop_dir(1167630, root).endswith(os.path.join('content', '1167630')), '... its Workshop folder')
    check(steam.proton_user(1167630, root).endswith(os.path.join('users', 'steamuser')), '... its Proton prefix (Linux: its Documents and AppData)')
    check(steam.install_dir(4242, root) is None, 'a game not installed: None')

# ---- the game list
check(GAMES and by_id('teardown').name == 'Teardown' and by_id('no such game') is GAMES[0], 'the game list: Teardown; an unknown id falls back')
for g in GAMES:
    check(g.id and g.name and g.needs and issubclass(g, Game), '%s: a game module with an id, a name and what it needs' % g.name)


# ---- the runtime with a stand-in game (no sound device)
class FakeGame(Game):
    id, name, needs = 'fake', 'Fake', 'nothing'

    def __init__(self, mixer, log=print):
        super().__init__(mixer, log)
        self.sent = []

    def send(self, kind, utt, text, times=None, t0=None):
        self.sent.append((kind, utt, text))
        return True


class NoStream:
    def start(self):
        pass

    def stop(self):
        pass

    def close(self):
        pass


runtime.open_output = lambda cb, device=None, log=print: NoStream()
rt = runtime.Runtime(FakeGame, log=lambda s: None, no_mic=True, volume=0.5)
rt.start()
check(rt.status()['state'] == 'waiting' and rt.mixer.volume == 0.5, 'the runtime: waiting for the game first')
rt.game.on_feed(dict(vol=1.0, speakers={1: dict(src=1, talk=True, gain=1.0, az=0, el=0, muffle=0)}, mic=False, lang='ru', live=True))
st = rt.status()
check(st['state'] == 'connected' and st['lang'] == 'ru' and len(st['speakers']) == 1 and st['mic'] == 'off',
      'a feed from the game: connected, its language, its speakers')
rt.mixer.feed_t -= audio.STALE + 1
check(rt.status()['state'] == 'paused', 'no news from the game: paused')
rt.set_volume(2)
check(rt.mixer.volume == 1.0, 'the volume is held to 0..1')
rt.stop()

# ---- the low-pass: the same as two one-pole filters in a row, block after block
rng = np.random.default_rng(3)
x = rng.standard_normal(1440).astype(np.float32)
for fc in (400.0, 4000.0):
    a = 1 - math.exp(-2 * math.pi * fc / audio.RATE)
    y1 = y2 = 0.0
    ref = []
    for s in x:
        y1 = a * s + (1 - a) * y1
        y2 = a * y1 + (1 - a) * y2
        ref.append(y2)
    hist = np.zeros(audio.LP_TAPS, np.float32)
    got = []
    for k in range(0, len(x), 480):
        y, hist = audio.lowpass(x[k:k + 480], hist, a)
        got.append(y)
    err = float(np.abs(np.concatenate(got) - np.array(ref)).max())
    check(err < 1e-3, 'the low-pass at %.0f Hz matches two one-pole filters (largest difference %.6f)' % (fc, err))

# ---- model downloads (fetch.py) from a local web server laid out like Hugging Face: <repo>/resolve/<rev>/<file>
import functools, http.server, tempfile, threading
import fetch
import kotodama
web, home = tempfile.mkdtemp(), tempfile.mkdtemp()
os.makedirs(os.path.join(web, 'own', 'model', 'resolve', 'abc'))
blob = os.urandom(3 << 20)
open(os.path.join(web, 'own', 'model', 'resolve', 'abc', 'm.onnx'), 'wb').write(blob)
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), functools.partial(http.server.SimpleHTTPRequestHandler, directory=web))
srv.RequestHandlerClass.log_message = lambda *a: None
threading.Thread(target=srv.serve_forever, daemon=True).start()
old = fetch.BASE, paths.MODELS, os.environ.get('HF_HUB_CACHE')
fetch.BASE, paths.MODELS, os.environ['HF_HUB_CACHE'] = 'http://127.0.0.1:%d' % srv.server_address[1], home, os.path.join(home, 'nohf')
want = os.path.join(home, 'own__model', 'abc', 'm.onnx')
os.makedirs(os.path.dirname(want))
open(want + '.part', 'wb').write(b'half a file from before')     # (a broken download; this server cannot resume)
seen = []
d = fetch.repo_files('own/model', 'abc', ['m.onnx'], log=lambda m: None, progress=lambda f, a, b: seen.append((f, a, b)))
check(open(os.path.join(d, 'm.onnx'), 'rb').read() == blob and not os.path.exists(want + '.part'),
      'a model file downloads whole, over a broken earlier try')
check(seen and seen[-1] == ('m.onnx', len(blob), len(blob)), 'the download reports its progress')
check(fetch.repo_files('own/model', 'abc', ['m.onnx'], log=lambda m: 1 / 0) == d, 'a downloaded file is not downloaded again')
try:
    fetch.download(fetch.BASE + '/own/model/resolve/abc/none.onnx', os.path.join(home, 'none.onnx'), tries=1)
    check(False, 'a missing file is an error')
except OSError:
    check(not os.path.exists(os.path.join(home, 'none.onnx')), 'a missing file is an error, and leaves nothing behind')
check(kotodama.download_text(('m', 336e6, 640e6)) == '52 % of 640 MB' and kotodama.download_text(('m', 5e6, 0)) == '5 MB',
      'the window shows the download')
srv.shutdown()
fetch.BASE, paths.MODELS = old[0], old[1]
if old[2] is None:
    os.environ.pop('HF_HUB_CACHE')
else:
    os.environ['HF_HUB_CACHE'] = old[2]

print('\n%d checks, %d failed' % (NCHECK, FAILED))
sys.exit(1 if FAILED else 0)
