"""Single-word callouts ("Okay.", "Yes.", "Run!"...): does Nemotron drop short utterances, and does a blank
penalty (the decoder's "say nothing" made less attractive) bring them back? Whisper small for comparison.

    C:/Users/user/miniconda3/envs/pcbench/python.exe bench/shortwords.py

Each word is said by several voices (0.5 s before, 1 s after), clean and as through a webcam in a room.
Report: export/asrbench/shortwords.md
"""
import os
import sys
import time

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import bench as B           # noqa: E402
import make_clips as M      # noqa: E402

WORDS = {
    'en': ['Okay.', 'Yes.', 'No.', 'Run!', 'Go.', 'Help!', 'Wait.', 'Stop!', 'Over here!', 'Thanks.'],
    'ru': ['Да.', 'Нет.', 'Беги!', 'Стой!', 'Помогите!'],
    'zh': ['好。', '是的。', '快跑！', '等等。', '救命！'],
}
VOICES = {
    'en': [('sapi', 'Microsoft David Desktop'), ('sapi', 'Microsoft Zira Desktop'), ('sapi', 'Microsoft Mark'),
           ('piper', 'vits-piper-en_US-ryan-medium'), ('piper', 'vits-piper-en_US-amy-medium')],
    'ru': [('piper', 'vits-piper-ru_RU-dmitri-medium'), ('piper', 'vits-piper-ru_RU-irina-medium')],
    'zh': [('piper', 'vits-piper-zh_CN-huayan-medium'), ('piper', 'vits-piper-zh_CN-chaowen-medium')],
}
PENALTIES = [0.0, 1.0, 2.0]


def make():
    clips = []
    tmp = os.path.join(B.OUT, 'sw_tmp.wav')
    for lang, words in WORDS.items():
        for w in words:
            for kind, voice in VOICES[lang]:
                if kind == 'sapi':
                    M.sapi(voice, w, tmp)
                    x, sr = M.read_wav(tmp)
                else:
                    x, sr = M.piper(voice, w)
                x = M.to16k(x, sr)
                x = x / (np.abs(x).max() + 1e-9) * 0.5
                s0, s1 = M.speech_span(x, 0)
                x = np.concatenate([np.zeros(8000, np.float32), x[int(s0 * 16000):int(s1 * 16000) + 1600], np.zeros(16000, np.float32)])
                rms = np.sqrt((x ** 2).mean() * len(x) / max(1, int((s1 - s0) * 16000)))
                room = M.room(x) + M.noise(len(x), rms / 10 ** (15 / 20))
                room = (room / (np.abs(room).max() + 1e-9) * 0.5).astype(np.float32)
                end = 0.5 + (s1 - s0)
                clips.append(dict(lang=lang, text=w, voice=voice.split('-')[-2] if kind == 'piper' else voice.split()[1],
                                  clean=x.astype(np.float32), room=room, speech_end=end))
    if os.path.exists(tmp):
        os.remove(tmp)
    return clips


def hit(ref, hyp, lang):
    r, h = B.norm(ref, lang), B.norm(hyp, lang)
    return bool(h) and B.edits(r, h) == 0


def main():
    clips = make()
    print('%d clips' % len(clips), flush=True)
    import sherpa_onnx
    from huggingface_hub import snapshot_download
    d = snapshot_download(B.NEMO % '560ms-int8')
    results = {}
    for pen in PENALTIES:
        r = sherpa_onnx.OnlineRecognizer.from_transducer(
            tokens=os.path.join(d, 'tokens.txt'), encoder=os.path.join(d, 'encoder.int8.onnx'), decoder=os.path.join(d, 'decoder.int8.onnx'),
            joiner=os.path.join(d, 'joiner.int8.onnx'), num_threads=B.THREADS, blank_penalty=pen)
        m = B.Nemotron.__new__(B.Nemotron)
        m.cfg, m.r = dict(lang=True), r
        for c in clips:
            for cond in ('clean', 'room'):
                out = m.run(c[cond], dict(lang=c['lang'], speech_end=c['speech_end']))
                results[('nemotron, blank penalty %g' % pen, cond, id(c))] = out['text']
        print('nemotron penalty', pen, 'done', flush=True)
    w = B.Whisper(dict(model='Systran/faster-whisper-small', words=False))
    for c in clips:
        for cond in ('clean', 'room'):
            results[('whisper-small', cond, id(c))] = w.run(c[cond], dict(lang=c['lang']))['text']
    print('whisper done', flush=True)
    names = ['nemotron, blank penalty %g' % p for p in PENALTIES] + ['whisper-small']
    L = ['# Single-word callouts (%s)' % time.strftime('%Y-%m-%d %H:%M'), '',
         '%d clips: %s. A hit = exactly the word (case and punctuation ignored).' %
         (len(clips), ', '.join('%s %d words x %d voices' % (l, len(WORDS[l]), len(VOICES[l])) for l in WORDS)), '',
         '| model | ' + ' | '.join('%s %s' % (l, cond) for l in WORDS for cond in ('clean', 'room')) + ' | nothing at all |',
         '|---|' + '---|' * (2 * len(WORDS) + 1)]
    for n in names:
        cells, empty = [], 0
        for l in WORDS:
            for cond in ('clean', 'room'):
                cc = [c for c in clips if c['lang'] == l]
                hits = sum(hit(c['text'], results[(n, cond, id(c))], l) for c in cc)
                empty += sum(1 for c in cc if not results[(n, cond, id(c))].strip())
                cells.append('%d/%d' % (hits, len(cc)))
        L.append('| %s | %s | %d of %d |' % (n, ' | '.join(cells), empty, 2 * len(clips)))
    L += ['', '## Per word (clean / room; - = nothing)', '']
    for l in WORDS:
        for word in WORDS[l]:
            row = []
            for n in names:
                outs = []
                for c in [c for c in clips if c['lang'] == l and c['text'] == word]:
                    for cond in ('clean', 'room'):
                        t = results[(n, cond, id(c))].strip()
                        outs.append(t if t else '-')
                miss = sum(1 for o, c in zip(outs, [c for c in clips if c['lang'] == l and c['text'] == word for _ in (0, 1)])
                           if not hit(word, o if o != '-' else '', l))
                row.append('%s: %d/%d wrong (%s)' % (n, miss, len(outs), ', '.join(sorted(set(outs)))[:60]))
            L.append('- **%s** %s' % (word, ' ; '.join(row)))
    path = os.path.join(B.OUT, 'shortwords.md')
    open(path, 'w', encoding='utf-8').write('\n'.join(L) + '\n')
    print('\n'.join(L))


if __name__ == '__main__':
    sys.exit(main())
