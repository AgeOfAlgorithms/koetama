"""Voice to voice on one PC, with no game and no second player: two real Koetamas (`koetama --cli`) as players A and B
of a fake game, through a stand-in relay on 127.0.0.1 (kd-voice's example standin_relay). A's microphone is a
simulated one (`--sim-mic`, kd_audio::simmic: the same capture as a real device) playing a recording; B plays into a
wav file (`--out-wav`), so nothing is heard and no sound device is needed. For each microphone it reports how loud B
hears A against the reference microphone ("mono"), and what speech to text makes of what B heard.

    python tools/voice_e2e.py                          (the reference, then the microphones in MICS)
    python tools/voice_e2e.py stereo-inverted quiet    (the reference, then these)
    python tools/voice_e2e.py --list                   (every simulated microphone)
    python tools/voice_e2e.py --noise strong quiet     (A's noise suppression: off, standard - the default - or strong)

Needs: Python 3 with numpy, cargo (it builds koetama and the stand-in relay; SHERPA_ONNX_LIB_DIR is found in
app/target when unset), the English speech model in Koetama's data folder (the first run downloads it, 670 MB), and
the benchmark's clip export/asrbench/clips/en01_clean.wav (without it: a voice-like signal, and no speech-to-text
check). Heavy (it loads the speech model several times): not while someone plays. Exit 1 when a microphone comes
through more than 3 dB off the reference, or its words are not understood.
"""
import glob, json, os, re, socket, subprocess, sys, tempfile, threading, time, wave
import numpy as np

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
APP = os.path.join(ROOT, 'app')
EXE = '.exe' if os.name == 'nt' else ''
KOETAMA = os.path.join(APP, 'target', 'debug', 'koetama' + EXE)
RELAY = os.path.join(APP, 'target', 'debug', 'examples', 'standin_relay' + EXE)
CLIP = os.path.join(ROOT, 'export', 'asrbench', 'clips', 'en01_clean.wav')
MICS = ['stereo-inverted', 'stereo-silent-right', 'array-4-one-inverted', 'quiet', 'hot', '44k1-i16']
SPEAK = 10.0          # s that A talks once its microphone is open
TOLERANCE = 4.0       # dB against the reference (noise suppression on a hissy microphone takes the quiet parts of
                      #  its speech with the hiss and the automatic gain lifts the rest: it comes through 2-3 dB louder)
RECALL = 0.6          # of the recording's words found in what B heard


def env():
    e = dict(os.environ)
    if not e.get('SHERPA_ONNX_LIB_DIR'):
        found = glob.glob(os.path.join(APP, 'target', 'sherpa-onnx-prebuilt', '*', 'lib'))
        if found:
            e['SHERPA_ONNX_LIB_DIR'] = found[0]
    return e


def build():
    print('building koetama and the stand-in relay...', flush=True)
    subprocess.run(['cargo', 'build', '-q', '-p', 'koetama', '-p', 'kd-voice', '--example', 'standin_relay'],
                   cwd=APP, env=env(), check=True)


def free_port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


class Player:
    """One Koetama and the fake game's connection to it (the socket connector: one JSON object per line)."""

    def __init__(self, name, args, profiles, relay_url, out_wav):
        self.name, self.port = name, free_port()
        self.lines, self.objects = [], []
        with open(os.path.join(profiles, f'voice-e2e-{name}.json'), 'w') as f:
            json.dump({'format': 1, 'id': f'voice-e2e-{name}', 'game': 'Voice test', 'mod': f'Player {name.upper()}',
                       'url': 'https://example.com', 'author': 'voice_e2e.py', 'uses': ['voices', 'speech'],
                       'connector': {'type': 'socket', 'port': self.port}}, f)
        e = env()
        e.update(KOETAMA_PROFILES_DIR=profiles, KOETAMA_RELAY=relay_url)
        self.proc = subprocess.Popen([KOETAMA, '--cli', '--game', f'voice-e2e-{name}', '--out-wav', out_wav, '--lang', 'en'] + args,
                                     env=e, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                                     encoding='utf-8', errors='replace')
        threading.Thread(target=self._read_out, daemon=True).start()
        self.sock = None

    def _read_out(self):
        for line in self.proc.stdout:
            self.lines.append(line.rstrip())

    def said(self, text):
        return any(text in l for l in self.lines)

    def connect(self, secs=60):
        t0 = time.time()
        while time.time() - t0 < secs:
            try:
                self.sock = socket.create_connection(('127.0.0.1', self.port), timeout=1)
                self.sock.settimeout(None)
                threading.Thread(target=self._read_objects, daemon=True).start()
                return
            except OSError:
                time.sleep(0.2)
        raise SystemExit(f'player {self.name}: Koetama never listened on {self.port}:\n' + '\n'.join(self.lines[-20:]))

    def _read_objects(self):
        f = self.sock.makefile('r', encoding='utf-8', errors='replace')
        try:
            for line in f:
                try:
                    self.objects.append(json.loads(line))
                except ValueError:
                    pass
        except OSError:
            pass  # (the player stopped)

    def feed(self, obj):
        self.sock.sendall((json.dumps(dict(obj, type='feed')) + '\n').encode())

    def stop(self):
        if self.sock:
            self.sock.close()
        self.proc.terminate()
        self.proc.wait(10)


def speech_db(x):
    """the level of the speech (dBFS): the power of the 20 ms windows within 20 dB of the loudest ones (the 95th
    percentile) - the speech proper, not its quiet tails or what is left between words (counting every window with
    sound made a microphone with noise suppression look ~3 dB louder than it was: its tails are quieter)"""
    w = np.array([np.mean(x[i:i + 960] ** 2) for i in range(0, len(x) - 959, 960)]) if len(x) >= 960 else np.zeros(1)
    if w.max() <= 0:
        return -240.0
    on = w[w > np.percentile(w, 95) * 1e-2]
    return 10 * np.log10(np.mean(on) + 1e-24)


def read_wav(path):
    with wave.open(path) as w:
        x = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float32) / 32768
        return x.reshape(-1, w.getnchannels()).mean(axis=1), w.getframerate()


def transcribe(path):
    out = subprocess.run([KOETAMA, '--cli', '--transcribe', path, '--lang', 'en'], env=env(), capture_output=True,
                         text=True, encoding='utf-8', errors='replace').stdout
    return ' '.join(m.group(1) for m in re.finditer(r'^LINE \d+: (.*?)   \[', out, re.M) if m.group(1) != '(nothing)')


def words(t):
    return re.findall(r"[a-z']+", t.lower())


def run(mic, relay_url, tmp, use_clip):
    """A says the recording into `mic`; returns (B's speech level dBFS, what speech to text made of it)."""
    profiles = tempfile.mkdtemp(dir=tmp)
    b_wav, a_wav = os.path.join(tmp, f'{mic}-heard-by-b.wav'), os.path.join(tmp, f'{mic}-a.wav')
    a_args = ['--sim-mic', mic, '--noise', NOISE] + (['--mic-wav', CLIP] if use_clip else [])
    a = Player('a', a_args, profiles, relay_url, a_wav)
    b = Player('b', ['--no-mic'], profiles, relay_url, b_wav)
    seed = f'voice-e2e {time.time()} {mic}'
    ears = lambda pos, fwd, right: {'position': pos, 'forward': fwd, 'right': right, 'up': [0, 1, 0]}
    fa = {'listen': 'push_to_talk', 'talk_key': True, 'lang': 'en', 'live': False, 'me': 'a', 'name': 'A',
          'room_seed': seed, 'listener': ears([0, 1.7, 0], [0, 0, 1], [1, 0, 0]),
          'speakers': [{'id': 'b', 'name': 'B', 'position': [0, 1.7, 3]}], 'range': [10, 30], 'max_range': 30}
    fb = {'listen': 'off', 'lang': 'en', 'me': 'b', 'name': 'B', 'room_seed': seed,
          'listener': ears([0, 1.7, 3], [0, 0, -1], [-1, 0, 0]),
          'speakers': [{'id': 'a', 'name': 'A', 'position': [0, 1.7, 0]}], 'range': [10, 30], 'max_range': 30}
    try:
        a.connect()
        b.connect()
        t0, t_on = time.time(), None
        while True:
            a.feed(fa)
            b.feed(fb)
            if t_on is None and a.said('microphone on:'):
                t_on = time.time()
                print(f'  A: {next(l for l in a.lines if "microphone on:" in l).strip()}', flush=True)
            if t_on and time.time() - t_on > SPEAK:
                break
            if time.time() - t0 > 240:
                raise SystemExit('A\'s microphone never opened (the speech model?):\n' + '\n'.join(a.lines[-20:]))
            time.sleep(0.1)
        for l in a.lines:
            if 'microphone: ' in l and 'channel' in l:
                print(f'  A: {l.strip()}')
        time.sleep(1.5)  # (the last packets play out, B's file is written)
    finally:
        a.stop()
        b.stop()
    x, sr = read_wav(b_wav)
    heard = x[-int((SPEAK + 1.5) * sr):]
    return speech_db(heard), transcribe(b_wav), b_wav


NOISE = 'standard'


def main():
    global NOISE
    args = sys.argv[1:]
    if '--noise' in args:
        i = args.index('--noise')
        NOISE = args[i + 1]
        del args[i:i + 2]
    names = [a for a in args if not a.startswith('-')]
    build()
    if '--list' in sys.argv:
        subprocess.run([KOETAMA, '--cli', '--list-sim-mics'], env=env())
        return 0
    use_clip = os.path.exists(CLIP)
    want = []
    if use_clip:
        with open(os.path.join(ROOT, 'export', 'asrbench', 'clips.json'), encoding='utf-8') as f:
            want = words(next(c['text'] for c in json.load(f) if c['id'] == 'en01'))
    else:
        print(f'(no {CLIP}: a voice-like signal, no speech-to-text check)')
    relay = subprocess.Popen([RELAY], stdout=subprocess.PIPE, text=True)
    relay_url = relay.stdout.readline().strip()
    print(f'stand-in relay: {relay_url}')
    tmp = tempfile.mkdtemp(prefix='voice_e2e_')
    rows, bad = [], []
    try:
        for mic in ['mono'] + (names or MICS):
            print(f'{mic}...', flush=True)
            level, text, path = run(mic, relay_url, tmp, use_clip)
            found = set(words(text))
            recall = sum(w in found for w in want) / len(want) if want else None
            rows.append((mic, level, recall, text, path))
    finally:
        relay.terminate()
    ref = rows[0][1]
    print(f'\n{"microphone":<24} {"B hears":>8} {"vs mono":>8} {"words":>6}  what speech to text made of it')
    for mic, level, recall, text, path in rows:
        d = level - ref
        rc = '-' if recall is None else f'{recall:.0%}'
        print(f'{mic:<24} {level:>8.1f} {d:>+8.1f} {rc:>6}  {text[:70]}')
        if abs(d) > TOLERANCE:
            bad.append(f'{mic}: {d:+.1f} dB against the reference')
        if recall is not None and recall < RECALL:
            bad.append(f'{mic}: only {recall:.0%} of the words understood')
    print(f'\n(what B heard: {tmp})')
    if bad:
        print('\nFAILED:\n  ' + '\n  '.join(bad))
        return 1
    print('\nall microphones came through')
    return 0


if __name__ == '__main__':
    sys.exit(main())
