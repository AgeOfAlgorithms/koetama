"""Koetama on the command line, for Teardown (the window is koetama.py): the same runtime, a status line, and the
test modes that need no microphone.

    python engine/teardown_helper.py            # start it, then play (a level with Proximity Babble Chat)
    python engine/teardown_helper.py --demo     # no game needed: one voice walks a circle around you
    python engine/teardown_helper.py --list     # sound devices;  --device N / --mic-device N pick one
    python engine/teardown_helper.py --transcribe some.wav --lang ru   # a recording through the pipeline
    python engine/teardown_helper.py --auto-speech   # recorded lines as if spoken (needs the benchmark's export/)

Ctrl+C stops it. The pieces: audio.py (the mixer), games/teardown.py (the link), asr.py (speech to text),
runtime.py (all of it together).
"""
import argparse
import json
import os
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    sys.path.insert(0, HERE)

import numpy as np                                                   # noqa: E402

import paths                                                         # noqa: E402
from audio import RATE, STALE, Mixer, read_wav, load_wav, resample, open_output   # noqa: E402,F401
from games.teardown import (Teardown, FeedReader, Link, parse_feed, find_feeds, text_prefab, times_hex,   # noqa: E402,F401
                            make_voices, io_dirs, savegame_path, VOICES, NAMES, WORK, TEXT_MAX)
from runtime import Runtime                                          # noqa: E402

SAVE = savegame_path()


def status(rt):
    """one line: the game, the microphone, each voice, what is being heard / was said"""
    st = rt.status()
    if st['state'] == 'waiting':
        return 'waiting for the game (a level with %s)' % rt.game.needs
    if st['state'] == 'paused':
        return 'the game stopped sending (paused, or the level ended)'
    parts = ['mic %s %4.0f dB' % (st['mic'], st['level']) if st['mic'] in ('listening', 'talking') else 'mic ' + st['mic']]
    for sp in st['speakers']:
        parts.append('%s%s vol %3d%% dir %4.0f muffle %3d%%' % (sp['name'], '*' if sp['talk'] else ' ', round(sp['gain'] * 100),
                                                               sp['az'], round(sp['muffle'] * 100)))
    if st['live']:
        parts.append('hearing: ' + st['live'][-40:])
    elif st['last']:
        parts.append('said: ' + st['last'][:50])
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
    versions): English lines, one-word callouts, Russian, Chinese, Spanish, German, and mixed-language lines"""
    lid = os.path.join(paths.APP_ROOT, 'export', 'asrbench', 'lid', 'items.json')
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


def transcribe(args):
    import asr
    audio, sr = read_wav(args.transcribe)
    audio = resample(audio, sr, asr.RATE)
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


def demo(args):
    mixer = Mixer(make_voices())
    mixer.volume = max(0.0, min(1.0, args.volume))
    stream = open_output(lambda out, frames, t, st: out.__setitem__(slice(None), mixer.render(frames)), args.device)
    print('demo: the speaker walks a circle around you, 4 m away (ahead, right, behind, left)')
    t0, seq = time.perf_counter(), 0
    with stream:
        try:
            while not args.seconds or time.perf_counter() - t0 < args.seconds:
                seq += 1
                az = ((time.perf_counter() - t0) * 45.0 + 180.0) % 360.0 - 180.0          # (a turn in 8 s)
                mixer.set_feed(dict(seq=seq, vol=1.0, speakers={1: dict(src=2, talk=True, gain=1.0, az=az, el=0.0, muffle=0.0)}))
                time.sleep(0.05)
                if seq % 10 == 0:
                    print('\r  direction %4.0f   ' % az, end='', flush=True)
        except KeyboardInterrupt:
            pass
    print()
    return 0


def main():
    ap = argparse.ArgumentParser(description='%s %s on the command line, for Teardown' % (paths.APP_NAME, paths.VERSION))
    ap.add_argument('--list', action='store_true', help='list the sound devices')
    ap.add_argument('--device', type=int, help='output device number (default: the system default)')
    ap.add_argument('--mic-device', type=int, help='microphone device number (default: the system default)')
    ap.add_argument('--mic-wav', metavar='WAV', help='play this recording as the microphone (real time; tests without one)')
    ap.add_argument('--auto-speech', action='store_true', help='no microphone: recorded lines (English, one-word callouts, '
                    'Russian, Chinese, Spanish, German, mixed) played through the REAL speech-to-text as if spoken, each in its '
                    'own language - watch the live words and lines in the game (needs export/asrbench from the benchmark)')
    ap.add_argument('--volume', type=float, default=1.0, help='Koetama\'s own volume, 0..1')
    ap.add_argument('--no-mic', action='store_true', help='never open the microphone')
    ap.add_argument('--lang', help='the language spoken (en, ru, zh, es, de, ...; auto) - default: the game\'s setting '
                    '"Language I speak"')
    ap.add_argument('--threads', type=int, default=4, help='CPU threads for the speech models')
    ap.add_argument('--io-dir', help='where the mod looks for Koetama\'s files (default: the mods folder / the Workshop folder)')
    ap.add_argument('--transcribe', metavar='WAV', help='run a recording through the speech pipeline as if it came from '
                    'the microphone: print the live words and the lines, and stop')
    ap.add_argument('--demo', action='store_true', help='no game: one voice walks a circle around you')
    ap.add_argument('--seconds', type=float, default=0, help='stop after this long (0 = until Ctrl+C)')
    ap.add_argument('--type', action='store_true', help='no microphone: each line piped in goes to the game as if said')
    ap.add_argument('--auto', action='store_true', help='no microphone: once the game is running, the test lines below are '
                    'sent one by one, %d s apart, as if you had said them (their words live first)' % AUTO_GAP)
    args = ap.parse_args()

    if args.transcribe:
        return transcribe(args)
    if args.list:
        import sounddevice as sd
        print(sd.query_devices())
        return 0
    if args.demo:
        return demo(args)

    lines = []                                    # (what to print above the status line)
    mic_source = None
    if args.auto_speech:                          # (recorded lines in several languages, the real pipeline)
        import asr
        items = auto_speech_items()
        mic_source = lambda lst: asr.PlaylistMicrophone(lst, items, log=lines.append)   # noqa: E731
    elif args.mic_wav:                            # (a recording instead of the microphone)
        import asr
        audio, sr = read_wav(args.mic_wav)
        audio = resample(audio, sr, asr.RATE)
        mic_source = lambda lst: asr.WavMicrophone(lst, audio, log=lines.append)        # noqa: E731
    rt = Runtime(Teardown, log=lines.append, threads=args.threads, out_device=args.device, mic_device=args.mic_device,
                 volume=args.volume, lang=args.lang or ('en' if args.auto_speech else None), mic_source=mic_source,
                 no_mic=args.no_mic or args.auto or args.type,
                 game_args=dict(dirs=[args.io_dir]) if args.io_dir else None)
    print('preparing...')
    rt.start()                                    # (--auto-speech: each recorded line sets its own language)
    game = rt.game

    def send_typed(line):
        line = line.strip()
        if line:
            sent = game.link.send_text(line)
            lines.append('typed%s: %s' % ('' if sent else ' [no game to tell - is a level running?]', line))
    if args.type:                                 # (lines piped in: a test)
        threading.Thread(target=lambda: [send_typed(line) for line in sys.stdin], daemon=True).start()

    if rt.stream is not None:
        import sounddevice as sd
        dev = sd.query_devices(rt.stream.device, 'output')
        print('playing on: %s (output delay %.0f ms)' % (dev['name'], rt.stream.latency * 1000))
    print('reading %s' % game.save)
    print('my files for the game go to: %s' % ', '.join(game.link.dirs))
    if args.auto:
        print('auto test: %d lines, %d s apart, once a level is running. Stay in the game and watch the chat.'
              % (len(AUTO_LINES), AUTO_GAP))
    t0 = time.perf_counter()
    auto = [0, None]                  # (--auto: lines sent, when the next one goes)
    auto_live = [None]                # (--auto: the line being "said" - utterance, words, how many sent, next time)
    try:
        while not args.seconds or time.perf_counter() - t0 < args.seconds:
            time.sleep(0.25)
            if args.auto and game.link.mic and game.connected():
                if auto_live[0]:                                       # (a line being "said": its words so far)
                    utt, words, k, nxt = auto_live[0]
                    if time.perf_counter() >= nxt:
                        k = min(len(words), k + 2)
                        if k < len(words):
                            game.link.send_msg('l', utt, ' '.join(words[:k]))
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
                        lines.append('  (auto line %d of %d: its words arrive live, then the line)' % (auto[0], len(AUTO_LINES)))
                        auto_live[0] = (auto[0], AUTO_LINES[auto[0] - 1].split(), 0, time.perf_counter())
            rt.tick()
            while lines:
                print('\r' + lines.pop(0).ljust(118))
            print('\r' + status(rt).ljust(118)[:118], end='', flush=True)
    except KeyboardInterrupt:
        pass
    print()
    rt.stop()
    return 0


if __name__ == '__main__':
    sys.exit(main())
