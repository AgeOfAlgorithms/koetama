"""Make the speech-to-text benchmark's clips with computer voices (no microphone needed).

    C:/Users/user/miniconda3/envs/pcbench/python.exe bench/make_clips.py

Voices: the Windows voices (David, Mark, Zira: English, with each word's position in the audio, for
checking word timestamps) and Piper voices through sherpa-onnx (English, Russian, Chinese, Spanish,
German; downloaded once to export/asrbench/tts/). Each line is said by one voice (rotating), with 0.5 s
before and 1 s after. Two versions of every clip:
  clean   the voice as made (16 kHz)
  room    as through a webcam mic in a room: band-limited (100 Hz - 7 kHz), a small room's echo, and
          noise (pink + a fan hum) 15 dB under the voice
plus a few clips with no speech at all (room noise only: anything transcribed there was made up).
Writes export/asrbench/clips/*.wav and export/asrbench/clips.json.
"""
import json
import math
import os
import subprocess
import sys
import tarfile
import urllib.request
import wave

import numpy as np
from scipy.signal import butter, resample_poly, sosfilt, fftconvolve

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(os.path.dirname(HERE), 'export', 'asrbench')
CLIPS = os.path.join(OUT, 'clips')
TTS = os.path.join(OUT, 'tts')
RATE = 16000
PIPER_URL = 'https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/%s.tar.bz2'

LINES = {
    'en': [
        "Hello, can anyone hear me?",
        "I found the key behind the painting in the great hall.",
        "Wait for me, I am coming down the stairs.",
        "There is something in the basement, do not go down there alone.",
        "Open sesame.",
        "Who took the gold chalice from the table?",
        "Run, it is right behind you!",
        "Let's split up, you take the left corridor and I will check the tower.",
        "My flashlight battery is almost dead.",
        "Can you throw me a rope? I am stuck on the ledge.",
        "The door will not open, maybe we need a lever.",
        "Okay.",
        "No, the other way, turn around.",
        "How much loot do we have so far?",
        "I think we should head back to the ship before it gets dark.",
        "Did you hear that noise upstairs?",
        "Quiet, it can hear us.",
        "Nice job, that was close.",
        "Meet me at the bridge near the windmill.",
        "Somebody fell through the floor in the kitchen.",
    ],
    'ru': [
        "Привет, вы меня слышите?",
        "Я нашёл ключ за картиной в большом зале.",
        "Подожди меня, я спускаюсь по лестнице.",
        "Не ходи в подвал один.",
        "Беги, оно прямо за тобой!",
        "Дверь не открывается, нужен рычаг.",
        "Кто взял золотую чашу со стола?",
        "Давай вернёмся на корабль, пока не стемнело.",
        "Тихо, оно нас слышит.",
        "Встретимся у моста возле мельницы.",
    ],
    'zh': [
        "你好，有人能听到我吗？",
        "我在大厅的画后面找到了钥匙。",
        "等等我，我正在下楼梯。",
        "不要一个人去地下室。",
        "快跑，它就在你后面！",
        "门打不开，也许我们需要一个拉杆。",
        "谁把桌上的金杯拿走了？",
        "天黑之前我们回船上吧。",
        "安静，它能听到我们。",
        "我们在风车旁边的桥上见面。",
    ],
    'es': [
        "Hola, ¿alguien me escucha?",
        "Encontré la llave detrás del cuadro en el salón principal.",
        "Espérame, estoy bajando las escaleras.",
        "Corre, está justo detrás de ti.",
        "La puerta no se abre, tal vez necesitamos una palanca.",
        "Silencio, nos puede oír.",
    ],
    'de': [
        "Hallo, kann mich jemand hören?",
        "Ich habe den Schlüssel hinter dem Gemälde in der großen Halle gefunden.",
        "Warte auf mich, ich komme die Treppe herunter.",
        "Lauf, es ist direkt hinter dir!",
        "Die Tür geht nicht auf, vielleicht brauchen wir einen Hebel.",
        "Leise, es kann uns hören.",
    ],
}
VOICES = {      # rotating per line
    'en': [('sapi', 'Microsoft David Desktop'), ('piper', 'vits-piper-en_US-ryan-medium'), ('sapi', 'Microsoft Zira Desktop'),
           ('piper', 'vits-piper-en_US-amy-medium'), ('sapi', 'Microsoft Mark')],
    'ru': [('piper', 'vits-piper-ru_RU-dmitri-medium'), ('piper', 'vits-piper-ru_RU-irina-medium')],
    'zh': [('piper', 'vits-piper-zh_CN-huayan-medium'), ('piper', 'vits-piper-zh_CN-chaowen-medium')],
    'es': [('piper', 'vits-piper-es_ES-davefx-medium')],
    'de': [('piper', 'vits-piper-de_DE-thorsten-medium')],
}


def to16k(x, sr):
    if sr == RATE:
        return x.astype(np.float32)
    g = math.gcd(RATE, sr)
    return resample_poly(x, RATE // g, sr // g).astype(np.float32)


def read_wav(path):
    with wave.open(path, 'rb') as w:
        sr, ch = w.getframerate(), w.getnchannels()
        x = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float32) / 32768.0
    if ch > 1:
        x = x.reshape(-1, ch).mean(axis=1)
    return x, sr


def write_wav(path, x):
    x = np.clip(x, -1, 1)
    with wave.open(path, 'wb') as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes((x * 32767).astype(np.int16).tobytes())


SAPI_PS = r'''
Add-Type -AssemblyName System.Speech
$s = New-Object System.Speech.Synthesis.SpeechSynthesizer
$s.SelectVoice('%s')
$script:w = @()
$s.add_SpeakProgress({ param($sender, $e) $script:w += ("{0}`t{1}`t{2}" -f $e.AudioPosition.TotalSeconds, $e.CharacterPosition, $e.Text) })
$fmt = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)
$s.SetOutputToWaveFile('%s', $fmt)
$s.Speak('%s')
$s.SetOutputToNull()
$s.Dispose()
$script:w | ForEach-Object { $_ }
'''


def sapi(voice, text, path):
    """speak with a Windows voice; returns [(word, start seconds)] from its progress events"""
    out = subprocess.run(['powershell', '-NoProfile', '-Command', SAPI_PS % (voice, path, text.replace("'", "''"))],
                         capture_output=True, text=True, check=True).stdout
    words = []
    for line in out.splitlines():
        parts = line.split('\t')
        if len(parts) == 3:
            words.append((parts[2], float(parts[0])))
    return words


_piper = {}


def piper(name, text):
    import sherpa_onnx
    d = os.path.join(TTS, name)
    if not os.path.isdir(d):
        os.makedirs(TTS, exist_ok=True)
        tar = os.path.join(TTS, name + '.tar.bz2')
        print('  downloading voice', name)
        urllib.request.urlretrieve(PIPER_URL % name, tar)
        with tarfile.open(tar) as t:
            t.extractall(TTS, filter='data')
        os.remove(tar)
    if name not in _piper:
        onnx = [f for f in os.listdir(d) if f.endswith('.onnx')][0]
        has = lambda f: os.path.exists(os.path.join(d, f))
        vits = sherpa_onnx.OfflineTtsVitsModelConfig(               # (a voice with a lexicon - some Chinese ones - has no espeak data)
            model=os.path.join(d, onnx), tokens=os.path.join(d, 'tokens.txt'),
            data_dir=os.path.join(d, 'espeak-ng-data') if has('espeak-ng-data') else '',
            lexicon=os.path.join(d, 'lexicon.txt') if has('lexicon.txt') else '')
        fsts = ','.join(os.path.join(d, f) for f in ('date.fst', 'phone.fst', 'number.fst') if has(f))
        cfg = sherpa_onnx.OfflineTtsConfig(model=sherpa_onnx.OfflineTtsModelConfig(vits=vits, num_threads=4), rule_fsts=fsts)
        _piper[name] = sherpa_onnx.OfflineTts(cfg)
    audio = _piper[name].generate(text, sid=0, speed=1.0)
    return np.array(audio.samples, dtype=np.float32), audio.sample_rate


def speech_span(x, pad_before):
    """(start, end) of the speech in seconds: where the level is within 40 dB of the loudest 20 ms"""
    n = int(RATE * 0.02)
    frames = x[:len(x) // n * n].reshape(-1, n)
    db = 20 * np.log10(np.sqrt((frames ** 2).mean(axis=1)) + 1e-9)
    on = np.where(db > db.max() - 40)[0]
    return float(on[0] * n / RATE), float((on[-1] + 1) * n / RATE)


rng = np.random.default_rng(7)
ROOM_IR = None


def room(x):
    """as through a webcam mic in a small room"""
    global ROOM_IR
    if ROOM_IR is None:
        n = int(RATE * 0.35)
        t = np.arange(n) / RATE
        ir = rng.standard_normal(n) * np.exp(-6.9 * t / 0.35) * 0.25        # (RT60 0.35 s)
        ir[0] = 1.0
        ROOM_IR = ir / np.sqrt((ir ** 2).sum())
    y = fftconvolve(x, ROOM_IR)[:len(x)]
    y = sosfilt(butter(4, [100, 7000], btype='band', fs=RATE, output='sos'), y)
    return y


def noise(n, level_rms):
    white = rng.standard_normal(n)
    pink = np.cumsum(white)                                   # (red-ish, then high-passed: a soft room hiss)
    pink = sosfilt(butter(2, 60, btype='high', fs=RATE, output='sos'), pink)
    pink /= pink.std() + 1e-9
    hum = np.sin(2 * np.pi * 120 * np.arange(n) / RATE) * 0.3 + np.sin(2 * np.pi * 240 * np.arange(n) / RATE) * 0.15
    z = pink + hum + 0.3 * white
    return z / (z.std() + 1e-9) * level_rms


def main():
    os.makedirs(CLIPS, exist_ok=True)
    meta = []
    k = 0
    for lang, lines in LINES.items():
        for i, text in enumerate(lines):
            kind, voice = VOICES[lang][i % len(VOICES[lang])]
            k += 1
            cid = '%s%02d' % (lang, i + 1)
            raw_path = os.path.join(OUT, 'raw_%s.wav' % cid)
            words = None
            if kind == 'sapi':
                words = sapi(voice, text, raw_path)
                x, sr = read_wav(raw_path)
                os.remove(raw_path)
            else:
                x, sr = piper(voice, text)
            x = to16k(x, sr)
            x = x / (np.abs(x).max() + 1e-9) * 0.5
            pad0, pad1 = 0.5, 1.0
            s0, s1 = speech_span(x, 0)
            x = np.concatenate([np.zeros(int(RATE * pad0), np.float32), x, np.zeros(int(RATE * pad1), np.float32)])
            clean = x
            speech_rms = np.sqrt((clean[int((pad0 + s0) * RATE):int((pad0 + s1) * RATE)] ** 2).mean())
            noisy = room(clean) + noise(len(clean), speech_rms / 10 ** (15 / 20))
            noisy = noisy / (np.abs(noisy).max() + 1e-9) * 0.5
            for cond, y in (('clean', clean), ('room', noisy)):
                write_wav(os.path.join(CLIPS, '%s_%s.wav' % (cid, cond)), y)
            meta.append(dict(id=cid, lang=lang, text=text, voice=voice, speech_start=pad0 + s0, speech_end=pad0 + s1,
                             dur=len(x) / RATE, words=[(w, pad0 + t) for w, t in words] if words else None))
            print('%-5s %-32s %4.1f s  %s' % (cid, voice[:32], len(x) / RATE, text))
    for j in range(4):                       # (no speech: room noise only)
        cid = 'none%02d' % (j + 1)
        n = int(RATE * 3.0)
        write_wav(os.path.join(CLIPS, '%s_room.wav' % cid), noise(n, 0.02 * (j + 1)))
        write_wav(os.path.join(CLIPS, '%s_clean.wav' % cid), noise(n, 0.002))
        meta.append(dict(id=cid, lang='en', text='', voice='', speech_start=0, speech_end=0, dur=3.0, words=None))
    with open(os.path.join(OUT, 'clips.json'), 'w', encoding='utf-8') as f:
        json.dump(meta, f, ensure_ascii=False, indent=1)
    total = sum(m['dur'] for m in meta)
    print('%d clips x 2 conditions, %.0f s of audio each' % (len(meta), total))


if __name__ == '__main__':
    sys.exit(main())
