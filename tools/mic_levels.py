"""How loud each microphone's speech really is: records every input device at once for SECONDS (talk normally into
the one you use), then prints per device the level of its speech (RMS over 10 ms pieces, dBFS), its noise floor, and
how much of the speech Koetama's automatic gain would see (kd-voice agc.rs: SPEECH, a 10 ms piece under -54 dBFS is
never taken for speech). Nothing is saved or sent.

    python tools/mic_levels.py [SECONDS] [DEVICE-NAME-PART ...]     (needs sounddevice and numpy: the pcvoice env)
"""
import sys
import numpy as np
import sounddevice as sd

SPEECH_FLOOR_DB = -54.0


def pieces_db(x, rate):
    n = int(rate * 0.01)
    k = len(x) // n
    p = x[: k * n].reshape(k, n)
    return 20 * np.log10(np.sqrt((p.astype(np.float64) ** 2).mean(axis=1)) + 1e-9)


def main():
    secs = float(sys.argv[1]) if len(sys.argv) > 1 else 20.0
    want = [a.lower() for a in sys.argv[2:]]
    wasapi = next((i for i, h in enumerate(sd.query_hostapis()) if 'WASAPI' in h['name']), None)
    devs = [(i, d) for i, d in enumerate(sd.query_devices()) if d['max_input_channels'] > 0
            and (wasapi is None or d['hostapi'] == wasapi)
            and (not want or any(w in d['name'].lower() for w in want))]
    got, streams = {}, []
    # (a stream of its own per device: sd.rec keeps one recording at a time)
    for i, d in devs:
        rate, ch = int(d['default_samplerate']), min(2, d['max_input_channels'])
        blocks = []
        try:
            s = sd.InputStream(device=i, samplerate=rate, channels=ch, dtype='float32',
                               callback=lambda x, n, t, st, blocks=blocks: blocks.append(x.copy()))
            s.start()
            streams.append(s)
            got[i] = (d['name'], rate, blocks)
        except Exception as e:
            got[i] = (d['name'], rate, e)
    print(f'recording {len(streams)} inputs for {secs:.0f} s - talk normally, then yell...', flush=True)
    sd.sleep(int(secs * 1000))
    for s in streams:
        s.stop()
        s.close()
    got = {i: (n, r, np.concatenate(b) if isinstance(b, list) and b else b) for i, (n, r, b) in got.items()}
    for i, (name, rate, x) in got.items():
        if isinstance(x, Exception):
            print(f'{name}: could not record ({x})')
            continue
        # (each channel; the louder one, as Koetama picks when they are not the same signal)
        per = [pieces_db(x[:, c], rate) for c in range(x.shape[1])]
        db = max(per, key=lambda v: np.percentile(v, 90))
        floor = np.percentile(db, 10)
        speech = db[db > floor + 15]          # (pieces well above the room: the talking)
        if len(speech) < 20:
            print(f'{name}: no speech heard (noise floor {floor:.0f} dBFS, loudest {db.max():.0f})')
            continue
        level = 10 * np.log10(np.mean(10 ** (speech / 10)))
        seen = np.mean(speech > SPEECH_FLOOR_DB)
        print(f'{name}: speech {level:.1f} dBFS (loudest 10 ms {db.max():.0f}), noise floor {floor:.0f} dBFS, '
              f'{seen:.0%} of the speech above the AGC floor (-54)')


if __name__ == '__main__':
    main()
