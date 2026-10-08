"""The voice dummies' lines with the time each word is said, for the game (Proximity Babble Chat's voice.lua,
PC.VDUMMY_LINES): the dummies' bubbles fill with their words as the helper plays their clips, so the bubble
features can be tried alone (arriving mid-sentence, walking away, the buffer's garbling).

    <conda>/envs/pcvoice/python.exe engine/make_dummy_lines.py

The text is the script each clip was made from (teardown_helper.VOICES); the times come from Parakeet's token
timestamps on the clip, matched word by word to the script (a word Parakeet heard differently takes the time
between its neighbours). Prints the Lua table; paste it into voice.lua.
"""
import difflib
import math
import os
import sys

import numpy as np
from scipy.signal import resample_poly

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import asr                       # noqa: E402
import teardown_helper as H      # noqa: E402


def main():
    models = asr.Models(threads=4, log=lambda s: None)
    clips = H.make_voices()
    rows = []
    for i, (_, _, script) in enumerate(H.VOICES, 1):
        g = math.gcd(asr.RATE, H.RATE)
        audio = resample_poly(clips[i], asr.RATE // g, H.RATE // g).astype(np.float32)   # (the helper plays at H.RATE)
        heard, times, _ = models.offline_timed('parakeet', audio)
        a = [asr.unit_key(u) for _, u in asr.units(heard)]
        b = [asr.unit_key(u) for _, u in asr.units(script)]
        out = [None] * len(b)
        for blk in difflib.SequenceMatcher(None, a, b, autojunk=False).get_matching_blocks():
            for k in range(blk.size):
                out[blk.b + k] = times[blk.a + k]
        known = [k for k, t in enumerate(out) if t is not None]
        for k in range(len(out)):                            # (unmatched: between the neighbours)
            if out[k] is None:
                lo = max([j for j in known if j < k], default=None)
                hi = min([j for j in known if j > k], default=None)
                t_lo = out[lo] if lo is not None else 0.0
                t_hi = out[hi] if hi is not None else len(audio) / asr.RATE
                n_lo, n_hi = (lo if lo is not None else -1), (hi if hi is not None else len(out))
                out[k] = round(t_lo + (t_hi - t_lo) * (k - n_lo) / (n_hi - n_lo), 2)
        dur = len(audio) / asr.RATE
        rows.append((script, ''.join('%04x' % max(0, min(0xFFFF, int(round(t * 100)))) for t in out), dur))
        print('-- %d: %d of %d words matched; heard: %s' % (i, len(known), len(b), heard), file=sys.stderr)
    print('PC.VDUMMY_LINES = {   -- (engine/make_dummy_lines.py in the koetama repo: the clips\' scripts,')
    print('                      --  each word\'s start from Parakeet on the clip, 4 hex digits in 1/100 s; dur: the clip)')
    for script, w, dur in rows:
        print('\t{text = "%s",\n\t w = "%s", dur = %.2f},' % (script.replace('"', '\\"'), w, dur))
    print('}')


if __name__ == '__main__':
    sys.exit(main())
