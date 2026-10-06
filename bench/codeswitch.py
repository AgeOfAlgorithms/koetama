"""Language detection and switching languages inside one utterance: Whisper small / medium and Nemotron 3.5
(560 ms, int8) with NO language given.

    <conda>/envs/pcbench/python.exe bench/codeswitch.py

1. Detection: every single-language line of the benchmark (clean): which language each model decides on
   (Whisper: its detected language; Nemotron: the script of the words it writes).
2. Switching: utterances glued from pieces said by voices of two languages with no pause (the voice
   changes too - a real speaker's would not), e.g. "Я нашёл | the golden key | за картиной в зале".
Report: export/asrbench/codeswitch.md (run after make_clips.py, needs its voices in export/asrbench/tts).
"""
import json
import os
import re
import sys
import time

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import bench as B           # noqa: E402
import make_clips as M      # noqa: E402

PIPER = {'en': 'vits-piper-en_US-ryan-medium', 'ru': 'vits-piper-ru_RU-dmitri-medium', 'zh': 'vits-piper-zh_CN-huayan-medium',
         'es': 'vits-piper-es_ES-davefx-medium', 'de': 'vits-piper-de_DE-thorsten-medium'}
MIXED = [
    [('ru', 'Я нашёл'), ('en', 'the golden key'), ('ru', 'за картиной в зале.')],
    [('en', 'Wait for me,'), ('ru', 'я спускаюсь по лестнице.')],
    [('ru', 'Беги, оно прямо за тобой,'), ('en', 'run run run!')],
    [('zh', '我们需要'), ('en', 'a lever'), ('zh', '才能打开这扇门。')],
    [('en', 'Run,'), ('zh', '它就在你后面！')],
    [('es', 'Corre,'), ('en', 'it is right behind you!')],
    [('en', 'Okay guys,'), ('de', 'lauf, es ist direkt hinter dir!')],
    [('de', 'Ich habe den'), ('en', 'flashlight'), ('de', 'im Keller gefunden.')],
]
CJK = re.compile(r'[\u3400-\u9fff]')
CYR = re.compile(r'[\u0400-\u04ff]')


def tokens(text):
    """words, with every Chinese character a token of its own (traditional counted as simplified)"""
    out = []
    for w in B.norm(text, 'en'):
        if CJK.search(w):
            out += B.norm(w, 'zh')
        else:
            out.append(w)
    return out


def script_of(text):
    """the language a transcript is written in, guessed from its letters"""
    if CJK.search(text):
        return 'zh'
    if CYR.search(text):
        return 'ru'
    low = text.lower()
    if re.search(r'[äöüß]', low) or re.search(r'\b(ich|ist|der|die|das|nicht|und|es)\b', low):
        return 'de'
    if re.search(r'[ñ¿¡áéíóú]', low) or re.search(r'\b(el|la|que|está|los|una)\b', low):
        return 'es'
    return 'en' if low.strip() else '-'


def mixed_audio(parts):
    pieces = []
    for lang, text in parts:
        x, sr = M.piper(PIPER[lang], text)
        x = M.to16k(x, sr)
        s0, s1 = M.speech_span(x / (np.abs(x).max() + 1e-9), 0)
        pieces.append(x[int(s0 * 16000):int(s1 * 16000)] / (np.abs(x).max() + 1e-9) * 0.5)
        pieces.append(np.zeros(int(16000 * 0.06), np.float32))      # (no pause: 60 ms between the pieces)
    speech = np.concatenate(pieces[:-1])
    x = np.concatenate([np.zeros(8000, np.float32), speech, np.zeros(16000, np.float32)])
    return x, 0.5 + len(speech) / 16000


class WhisperAuto(B.Whisper):
    def run(self, x, clip):
        t0 = time.perf_counter()
        segs, info = self.m.transcribe(x, language=None, beam_size=5, temperature=0.0, condition_on_previous_text=False,
                                       vad_filter=False)
        segs = list(segs)
        return dict(text=''.join(s.text for s in segs).strip(), lang=info.language, p=info.language_probability,
                    compute=time.perf_counter() - t0)


def main():
    clips = [c for c in json.load(open(os.path.join(B.OUT, 'clips.json'), encoding='utf-8')) if c['text']]
    mixed = []
    for parts in MIXED:
        x, end = mixed_audio(parts)
        mixed.append((parts, x, end))
    models = [('whisper-small', lambda: WhisperAuto(B.CONFIGS['whisper-small'])),
              ('whisper-medium', lambda: WhisperAuto(B.CONFIGS['whisper-medium'])),
              ('nemotron-560-int8 (auto)', lambda: B.Nemotron(B.CONFIGS['nemotron-560-int8-auto']))]
    L = ['# Language detection and switching (%s)' % time.strftime('%Y-%m-%d %H:%M'), '',
         'No language given to any model. Clean clips, 4 threads.', '']
    L += ['## 1. Detection on single-language lines', '', '| model | ' + ' | '.join(B.LANGS if hasattr(B, 'LANGS') else
          ['en', 'ru', 'zh', 'es', 'de']) + ' | wrong guesses |', '|---|---|---|---|---|---|---|']
    det_rows, mix_rows = [], []
    for name, make in models:
        m = make()
        right, total, wrong = {}, {}, []
        for c in clips:
            x = B.read(os.path.join(B.OUT, 'clips', '%s_clean.wav' % c['id']))
            r = m.run(x, c)
            got = r.get('lang') or script_of(r['text'])
            got = {'zh': 'zh', 'yue': 'zh'}.get(got, got)
            total[c['lang']] = total.get(c['lang'], 0) + 1
            if got == c['lang']:
                right[c['lang']] = right.get(c['lang'], 0) + 1
            else:
                wrong.append('%s->%s' % (c['id'], got))
            print(name, c['id'], got, r['text'][:60], flush=True)
        det_rows.append('| %s | %s | %s |' % (name, ' | '.join('%d/%d' % (right.get(l, 0), total[l]) for l in ['en', 'ru', 'zh', 'es', 'de']),
                                              ', '.join(wrong) or 'none'))
        for parts, x, end in mixed:
            ref = ' '.join(t for _, t in parts)
            clip = dict(lang='en', speech_end=end)
            r = m.run(x, clip)
            rt, ht = tokens(ref), tokens(r['text'])
            mix_rows.append((name, ' | '.join('%s: %s' % (l, t) for l, t in parts), r['text'], B.edits(rt, ht), len(rt)))
            print(name, 'MIX', r['text'], flush=True)
        del m
    L += det_rows
    L += ['', '## 2. Switching inside one utterance', '']
    for name, _ in models:
        rows = [r for r in mix_rows if r[0] == name]
        err = 100.0 * sum(r[3] for r in rows) / sum(r[4] for r in rows)
        L += ['', '**%s**: %.0f %% of the words wrong' % (name, err)]
        for _, said, got, e, n in rows:
            L.append('- %s -> **%s** (%d of %d wrong)' % (said, got or '(nothing)', e, n))
    path = os.path.join(B.OUT, 'codeswitch.md')
    open(path, 'w', encoding='utf-8').write('\n'.join(L) + '\n')
    print('\n'.join(L))


if __name__ == '__main__':
    sys.exit(main())
