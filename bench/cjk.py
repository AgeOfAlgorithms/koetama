"""SenseVoice's language setting for Japanese, Korean and Cantonese (and Mandarin): the helper loaded it once with
language 'zh'. Lines said by Microsoft's online neural voices (edge-tts: no Windows voices or Piper voices for these),
transcribed with the setting fixed at 'zh' (as before), 'auto', and the line's own language; characters wrong.

    C:/Users/user/miniconda3/envs/pcbench/python.exe bench/cjk.py make
    C:/Users/user/miniconda3/envs/pcvoice/python.exe bench/cjk.py test [Hugging Face repo]

Report: export/asrbench/cjk_<repo>.md
"""
import json
import os
import re
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(os.path.dirname(HERE), 'export', 'asrbench', 'cjk')

LINES = {
    'ja': ['すみません、ロープを投げてもらえますか。', '左の扉の後ろに宝物があります。', 'みんな、こっちに来て、早く。',
           'あの怪物はまだ地下室にいると思う。', '鍵はどこに置いたの？', '今夜は城の塔に登ろう。',
           '静かにして、何か聞こえる。', '私のライトが消えた、助けて。'],
    'ko': ['죄송한데 밧줄 좀 던져 줄 수 있어요?', '왼쪽 문 뒤에 보물이 있어요.', '다들 이쪽으로 빨리 와.',
           '그 괴물은 아직 지하실에 있는 것 같아.', '열쇠를 어디에 뒀어?', '오늘 밤에 성의 탑에 올라가자.',
           '조용히 해, 뭔가 들려.', '내 손전등이 꺼졌어, 도와줘.'],
    'yue': ['唔該你可唔可以掉條繩過嚟？', '左邊嗰度門後面有寶物。', '大家快啲過嚟呢邊。',
            '我覺得嗰隻怪物仲喺地庫度。', '你將條鎖匙放咗喺邊度？', '今晚我哋去爬城堡個塔啦。',
            '靜啲，我聽到啲嘢。', '我支電筒熄咗，幫下我。'],
    'zh': ['不好意思，你能把绳子扔给我吗？', '左边那扇门后面有宝物。', '大家快到这边来。',
           '我觉得那个怪物还在地下室里。', '你把钥匙放在哪里了？', '今晚我们去爬城堡的塔吧。',
           '安静，我听到了什么声音。', '我的手电筒灭了，帮帮我。'],
}
VOICES = {'ja': ['ja-JP-NanamiNeural', 'ja-JP-KeitaNeural'], 'ko': ['ko-KR-SunHiNeural', 'ko-KR-InJoonNeural'],
          'yue': ['zh-HK-HiuGaaiNeural', 'zh-HK-WanLungNeural'], 'zh': ['zh-CN-XiaoxiaoNeural', 'zh-CN-YunxiNeural']}


def make():
    import asyncio
    import edge_tts
    os.makedirs(OUT, exist_ok=True)
    items = []
    for lang, lines in LINES.items():
        for i, text in enumerate(lines):
            mp3 = os.path.join(OUT, '%s_%d.mp3' % (lang, i))
            wav = mp3[:-4] + '.wav'
            if not os.path.exists(wav):
                asyncio.run(edge_tts.Communicate(text, VOICES[lang][i % 2]).save(mp3))
                subprocess.run(['ffmpeg', '-y', '-loglevel', 'error', '-i', mp3, '-ac', '1', '-ar', '16000', wav], check=True)
                os.remove(mp3)
            items.append(dict(lang=lang, text=text, path=wav))
    json.dump(items, open(os.path.join(OUT, 'items.json'), 'w', encoding='utf-8'), ensure_ascii=False, indent=1)
    print(len(items), 'lines')


def chars(t, cc):
    t = cc.convert(t)
    return [c for c in t if not re.match(r'[\s\W_]', c)]


def edits(a, b):
    d = list(range(len(b) + 1))
    for i in range(1, len(a) + 1):
        prev, d[0] = d[0], i
        for j in range(1, len(b) + 1):
            prev, d[j] = d[j], min(d[j] + 1, d[j - 1] + 1, prev + (a[i - 1] != b[j - 1]))
    return d[len(b)]


def test():
    import numpy as np
    import sherpa_onnx as so
    import wave
    import opencc
    sys.path.insert(0, os.path.join(os.path.dirname(HERE), 'engine'))
    import asr
    cc = opencc.OpenCC('t2s')
    items = json.load(open(os.path.join(OUT, 'items.json'), encoding='utf-8'))
    repo = sys.argv[2] if len(sys.argv) > 2 else asr.MODELS['sensevoice'][0]
    d = asr.Models(threads=4, log=lambda s: None)._repo(repo)

    def rec(lang):
        return so.OfflineRecognizer.from_sense_voice(model=os.path.join(d, 'model.int8.onnx'), tokens=os.path.join(d, 'tokens.txt'),
                                                    num_threads=4, language=lang, use_itn=True)

    def read(p):
        w = wave.open(p)
        return np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(np.float32) / 32768

    def run(r, x, opt=None):
        s = r.create_stream()
        if opt:
            s.set_option('language', opt)
        s.accept_waveform(16000, np.concatenate([x, np.zeros(4800, np.float32)]))
        r.decode_stream(s)
        return s.result.text.strip()

    recs = {l: rec(l) for l in ('zh', 'auto', 'ja', 'ko', 'yue')}
    setups = [('fixed zh (before)', lambda it, x: run(recs['zh'], x)),
              ('auto', lambda it, x: run(recs['auto'], x)),
              ('own language (a recogniser each)', lambda it, x: run(recs[it['lang']], x)),
              ('own language (per line, set_option on one recogniser)', lambda it, x: run(recs['auto'], x, it['lang']))]
    res, samples = {}, []
    for name, fn in setups:
        for it in items:
            x = read(it['path'])
            t0 = time.perf_counter()
            out = fn(it, x)
            e, n = edits(chars(it['text'], cc), chars(out, cc)), len(chars(it['text'], cc))
            r = res.setdefault((name, it['lang']), [0, 0])
            r[0] += e
            r[1] += n
            if it is items[0] or it['path'].endswith('_3.wav'):
                samples.append((name, it['lang'], it['text'], out))
        print(name, {l: '%.1f' % (100.0 * res[(name, l)][0] / res[(name, l)][1]) for l in LINES}, flush=True)
    L = ['# SenseVoice: the language setting (%s, %s)' % (repo.split('/')[-1], time.strftime('%Y-%m-%d %H:%M')), '',
         '8 lines per language, Microsoft neural voices (edge-tts), clean. Characters wrong (%, traditional -> simplified '
         'before comparing, punctuation ignored).', '',
         '| setting | ' + ' | '.join(LINES) + ' |', '|---|' + '---|' * len(LINES)]
    for name, _ in setups:
        L.append('| %s | %s |' % (name, ' | '.join('%.1f' % (100.0 * res[(name, l)][0] / res[(name, l)][1]) for l in LINES)))
    L += ['', '| setting | language | said | written |', '|---|---|---|---|']
    L += ['| %s | %s | %s | %s |' % s for s in samples]
    open(os.path.join(os.path.dirname(OUT), 'cjk_%s.md' % repo.split('/')[-1]), 'w', encoding='utf-8').write('\n'.join(L) + '\n')
    print('\n'.join(L))


if __name__ == '__main__':
    {'make': make, 'test': test}[sys.argv[1]]()
