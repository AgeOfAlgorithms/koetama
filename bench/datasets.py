"""Real human speech for bench/denoise_stt.py, from two public datasets (downloaded into export/datasets/, never
committed; their licenses allow research use with attribution):
  - speechocean762 (OpenSLR 101, CC BY 4.0): English read by 250 Mandarin-speaking learners, half of them children, each
    utterance scored 0-10 for pronunciation by experts. The worst-pronounced that are still spoken: a "total" score of 3
    (scores 0-2 are 20 recordings, mostly near silence: -46 to -29 dB), drawn at random.
  - Golos far-field (Sber, OpenSLR 114; the 100 h far-field split's test set from Hugging Face,
    bond005/sberdevices_golos_100h_farfield): Russian spoken to a smart speaker from across a room.
  - EdAcc (the Edinburgh International Accents of English Corpus, CC BY-SA 4.0; test chunks from Hugging Face,
    edinburghcstr/edacc): conversations; an even number of whole sentences per accent (Kenyan, Indian, Ghanaian,
    Nigerian, Romanian, Latin American, Spanish in the chunks here).
  - LibriSpeech test-clean (OpenSLR 12, CC BY 4.0): native English read from audiobooks, one sentence per speaker.
  - Golos crowd (as Golos far-field, the 10 h crowd split): Russian read close to a phone's microphone.
Each becomes export/datasets/<name>/subset/: 16 kHz wav files and manifest.json ([{id, text, file, score?, accent?}]).

    python bench/datasets.py so762 [N]     (speechocean762: N of the worst spoken, default 40)
    python bench/datasets.py golos [N]     (needs pyarrow and soundfile: the pcbench env)
    python bench/datasets.py edacc [N]     (N per accent, default 8; pcbench)
    python bench/datasets.py libri [N]     (pcbench: FLAC)
    python bench/datasets.py golos_crowd [N]   (pcbench)
"""
import io, json, os, random, sys, wave

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(ROOT, 'export', 'datasets')


def write16(path, frames, sr):
    with wave.open(path, 'wb') as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(sr)
        w.writeframes(frames)


def so762(n):
    base = os.path.join(DATA, 'speechocean762', 'speechocean762')
    scores = json.load(open(os.path.join(base, 'resource', 'scores.json'), encoding='utf-8'))
    # (the test half: its wav.scp names each utterance's file)
    paths = {}
    for part in ('test', 'train'):
        scp = os.path.join(base, part, 'wav.scp')
        for line in open(scp, encoding='utf-8'):
            uid, rel = line.split(maxsplit=1)
            paths[uid] = os.path.join(base, rel.strip())
    items = sorted((v['total'], uid, v['text']) for uid, v in scores.items() if uid in paths and v['total'] == 3)
    random.Random(3).shuffle(items)
    out = os.path.join(DATA, 'speechocean762', 'subset')
    os.makedirs(out, exist_ok=True)
    man = []
    for total, uid, text in items[:n]:
        with wave.open(paths[uid]) as w:
            frames, sr = w.readframes(w.getnframes()), w.getframerate()
        f = os.path.join(out, f'{uid}.wav')
        write16(f, frames, sr)
        man.append({'id': uid, 'text': text, 'file': f, 'score': total})
    json.dump(man, open(os.path.join(out, 'manifest.json'), 'w', encoding='utf-8'), ensure_ascii=False, indent=1)
    print(f'speechocean762: {len(man)} utterances, total pronunciation scores {man[0]["score"]}..{man[-1]["score"]} of 10')


def golos(n, folder='golos_farfield', parquet='test.parquet'):
    import pyarrow.parquet as pq
    import soundfile as sf
    t = pq.read_table(os.path.join(DATA, folder, parquet)).to_pylist()
    random.Random(5).shuffle(t)
    out = os.path.join(DATA, folder, 'subset')
    os.makedirs(out, exist_ok=True)
    man = []
    for i, r in enumerate(t):
        if len(man) >= n:
            break
        text = (r['transcription'] or '').strip()
        if len(text.split()) < 3:
            continue  # (sentences: one-word commands say little about errors)
        x, sr = sf.read(io.BytesIO(r['audio']['bytes']), dtype='int16')
        f = os.path.join(out, f'golos{i:04d}.wav')
        write16(f, x.tobytes(), sr)
        man.append({'id': f'golos{i:04d}', 'text': text, 'file': f})
    json.dump(man, open(os.path.join(out, 'manifest.json'), 'w', encoding='utf-8'), ensure_ascii=False, indent=1)
    print(f'{folder}: {len(man)} utterances')


def edacc(n):
    import glob, re
    import pyarrow.parquet as pq
    import soundfile as sf
    rows = []
    for f in sorted(glob.glob(os.path.join(DATA, 'edacc', '*.parquet'))):
        rows += pq.read_table(f).to_pylist()
    random.Random(7).shuffle(rows)
    out = os.path.join(DATA, 'edacc', 'subset')
    os.makedirs(out, exist_ok=True)
    man, per = [], {}
    for i, r in enumerate(rows):
        text = re.sub(r'[<\[][^>\]]*[>\]]', ' ', r['text'] or '')  # (no <laugh>, [noise] and the like)
        text = ' '.join(text.split())
        if len(text.split()) < 6 or per.get(r['accent'], 0) >= n or not re.fullmatch(r"[A-Za-z' ,.?!-]+", text):
            continue
        x, sr = sf.read(io.BytesIO(r['audio']['bytes']), dtype='int16')
        if x.ndim > 1:
            x = x[:, 0]
        if not 2 <= len(x) / sr <= 12:
            continue
        uid = f'edacc{i:05d}'
        f = os.path.join(out, f'{uid}.wav')
        write16(f, x.tobytes(), sr)
        man.append({'id': uid, 'text': text, 'file': f, 'accent': r['accent'], 'speaker': r['speaker']})
        per[r['accent']] = per.get(r['accent'], 0) + 1
    json.dump(man, open(os.path.join(out, 'manifest.json'), 'w', encoding='utf-8'), ensure_ascii=False, indent=1)
    print(f'edacc: {len(man)} sentences: ' + ', '.join(f'{a} {c}' for a, c in sorted(per.items())))


def libri(n):
    import glob
    import soundfile as sf
    base = os.path.join(DATA, 'librispeech', 'LibriSpeech', 'test-clean')
    out = os.path.join(DATA, 'librispeech', 'subset')
    os.makedirs(out, exist_ok=True)
    man = []
    for spk in sorted(os.listdir(base)):
        if len(man) >= n:
            break
        # (one sentence of 3-12 s per speaker: as many voices as there are lines)
        for trans in sorted(glob.glob(os.path.join(base, spk, '*', '*.trans.txt'))):
            done = False
            for line in open(trans, encoding='utf-8'):
                uid, text = line.strip().split(' ', 1)
                x, sr = sf.read(os.path.join(os.path.dirname(trans), uid + '.flac'), dtype='int16')
                if 3 <= len(x) / sr <= 12:
                    f = os.path.join(out, f'{uid}.wav')
                    write16(f, x.tobytes(), sr)
                    man.append({'id': uid, 'text': text, 'file': f})
                    done = True
                    break
            if done:
                break
    json.dump(man, open(os.path.join(out, 'manifest.json'), 'w', encoding='utf-8'), ensure_ascii=False, indent=1)
    print(f'librispeech: {len(man)} sentences, one per speaker')


if __name__ == '__main__':
    which = sys.argv[1]
    n = int(sys.argv[2]) if len(sys.argv) > 2 else (8 if which == 'edacc' else 40)
    {'so762': so762, 'golos': golos, 'edacc': edacc, 'libri': libri,
     'golos_crowd': lambda n: golos(n, 'golos_crowd', 'test0.parquet')}[which](n)
