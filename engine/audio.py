"""Audio for the helper, game-independent: the voice mixer (each voice placed by the game's gain, direction and muffle),
the low-pass that muffles, resampling, wav files, and opening the output device. numpy only (no scipy: the app is
smaller and builds faster).
"""
import math
import threading
import time
import wave

import numpy as np

RATE = 48000
SMOOTH = 0.05          # s: gains, direction and muffle glide to each new feed value
STALE = 1.5            # s without a new feed (the game paused or gone): the voices fade out
CUT_CLEAR, CUT_MUFFLED = 16000.0, 400.0   # low-pass cutoff (Hz) at muffle 0 and 1
PAN = 0.9              # how far to one side a voice goes at 90 degrees (1 = that ear only)
BEHIND_MUFFLE, BEHIND_QUIET = 0.25, 0.2   # a voice straight behind: this much duller and quieter
HEADROOM = 1.3         # gain before the soft limiter
LP_TAPS = 400          # longest low-pass impulse response kept (samples; the most muffled ~ 350)


# ---------------------------------------------------------------- the muffle: two one-pole low-passes in a row
def lowpass_ir(a):
    """the impulse response of two one-pole low-passes in a row (y = a x + (1 - a) y'), cut where it is spent"""
    n = np.arange(LP_TAPS)
    h = a * (1 - a) ** n
    hh = np.convolve(h, h)[:LP_TAPS]
    keep = np.nonzero(hh > hh.max() * 1e-4)[0]
    return hh[:keep[-1] + 1] if len(keep) else hh[:1]


def lowpass(x, hist, a):
    """filter block x (float32) with the cutoff a (the one-pole coefficient); hist: the last LP_TAPS input samples
    before x (the filter's memory, as an FIR over the input - the cutoff may change from block to block).
    -> (y, new hist)"""
    h = lowpass_ir(a)
    full = np.concatenate([hist, x])
    y = np.convolve(full, h)[len(hist):len(hist) + len(x)]
    return y.astype(np.float32), full[-LP_TAPS:]


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
        self.hist = np.zeros(LP_TAPS, np.float32)
        self.src_clip = None      # the clip it last played (to fade out on after it left the feed)


class Mixer:
    """clips: {src: mono float32 array at RATE}. A feed (the game's): {vol, speakers: {id: {src, talk, gain, az,
    el, muffle}}}. render(frames) -> (frames, 2) float32"""
    def __init__(self, clips, rate=RATE, clock=time.perf_counter):
        self.clips, self.rate, self.clock = clips, rate, clock
        self.feed, self.feed_t = None, -1e9
        self.voices = {}
        self.lock = threading.Lock()
        self.volume = 1.0         # the helper's own volume, on top of the game's
        self.levels = {}          # id -> the level last rendered (for the status)

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
            x, v.hist = lowpass(x, v.hist, a)
            nl, nr = v.gl + (tl - v.gl) * k, v.gr + (tr - v.gr) * k
            out[:, 0] += x * (v.gl + (nl - v.gl) * ramp)
            out[:, 1] += x * (v.gr + (nr - v.gr) * ramp)
            v.gl, v.gr = nl, nr
            self.levels[sid] = max(nl, nr)
        return np.tanh(out)


# ---------------------------------------------------------------- resampling, wav files
def resample(x, sr_from, sr_to):
    """x (mono float32) from one sample rate to another: sherpa-onnx's resampler (windowed sinc), else linear"""
    if sr_from == sr_to:
        return np.asarray(x, np.float32)
    try:
        import sherpa_onnx as so
        r = so.LinearResample(orig_sample_rate=int(sr_from), new_sample_rate=int(sr_to))
        return np.asarray(r.resample(np.asarray(x, np.float32), flush=True), np.float32)
    except Exception:
        n = int(round(len(x) * sr_to / sr_from))
        return np.interp(np.arange(n) * (sr_from / sr_to), np.arange(len(x)), x).astype(np.float32)


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
    """a voice clip for the mixer: at RATE, every clip equally loud (peaks at half scale), 0.5 s of quiet after"""
    x, sr = read_wav(path)
    x = resample(x, sr, RATE)
    loud = float(np.percentile(np.abs(x), 99.9)) or 1.0
    x = x * (0.5 / loud)
    return np.concatenate([x, np.zeros(int(RATE * 0.5), dtype=np.float32)]).astype(np.float32)


# ---------------------------------------------------------------- devices
def _sounddevice():
    """sounddevice, finding PortAudio: a packaged Linux build ships libportaudio.so.2 next to the program (a Steam Deck
    has none installed), and sounddevice only looks for the system's"""
    import sys
    if sys.platform.startswith('linux'):
        import os
        import ctypes.util
        import paths
        mine = os.path.join(paths.APP_ROOT, 'lib', 'libportaudio.so.2')
        if os.path.exists(mine) and not getattr(ctypes.util, '_kotodama', False):
            find = ctypes.util.find_library
            ctypes.util.find_library = lambda name: mine if 'portaudio' in name else find(name)
            ctypes.util._kotodama = True
    import sounddevice
    return sounddevice


def output_devices():
    """[(index, name)] of the output devices, the WASAPI ones on Windows (the short delay) when there are any"""
    sd = _sounddevice()
    return _devices(sd, 'max_output_channels')


def input_devices():
    sd = _sounddevice()
    return _devices(sd, 'max_input_channels')


def _devices(sd, key):
    apis = sd.query_hostapis()
    wasapi = [i for i, a in enumerate(apis) if 'WASAPI' in a['name']]
    out = []
    for i, d in enumerate(sd.query_devices()):
        if d[key] > 0 and (not wasapi or d['hostapi'] in wasapi):
            out.append((i, d['name']))
    return out


def open_output(callback, device=None, log=print):
    """an OutputStream at RATE, stereo, small blocks: on Windows through WASAPI (the default API, MME, adds ~100 ms)"""
    sd = _sounddevice()
    extra = None
    if device is None:
        try:
            for api in sd.query_hostapis():
                if 'WASAPI' in api['name'] and api['default_output_device'] >= 0:
                    device = api['default_output_device']
        except Exception:
            device = None
    try:
        if device is not None and 'WASAPI' in sd.query_hostapis(sd.query_devices(device)['hostapi'])['name']:
            extra = sd.WasapiSettings(auto_convert=True)
    except Exception:
        extra = None
    try:
        return sd.OutputStream(samplerate=RATE, channels=2, dtype='float32', blocksize=480, latency='low',
                               device=device, extra_settings=extra, callback=callback)
    except Exception as e:             # (that device would not open this way: the system default)
        log('output device failed (%s); using the default output' % e)
        return sd.OutputStream(samplerate=RATE, channels=2, dtype='float32', blocksize=480, latency='low',
                               callback=callback)
