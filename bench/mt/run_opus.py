"""The benchmark's Opus-MT run: Helsinki-NLP's Marian models converted to CTranslate2 (8-bit, CPU - what Koetama would
run them with), each job on its shortest route (a direct model when there is one, else through English), each
sentence translated on its own and timed. Greedy decoding and one thread, like the Mozilla engine. Writes
results/opus/<src>-<tgt>.json.
    python run_opus.py           (env mtbench; after prep.py)"""
import json
import os
import time

import ctranslate2
from huggingface_hub import snapshot_download
from transformers import MarianTokenizer

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, 'results', 'opus')
CT2 = os.path.join(HERE, 'opus_ct2')
BEAM = 1
THREADS = 1


def dir_bytes(d):
    return sum(os.path.getsize(os.path.join(r, f)) for r, _, fs in os.walk(d) for f in fs)


def get_model(name, cache):
    """(translator, tokenizer, bytes, load s) for a Helsinki-NLP model, converted once"""
    if name in cache:
        return cache[name]
    out = os.path.join(CT2, name)
    # (plain files in bench/mt/opus_src: Hugging Face's cache needs symbolic links, which Windows refuses without
    #  Developer Mode)
    src = snapshot_download(f'Helsinki-NLP/{name}', local_dir=os.path.join(HERE, 'opus_src', name),
                            allow_patterns=['*.json', '*.spm', 'pytorch_model.bin', 'vocab*', '*.txt'])
    if not os.path.exists(os.path.join(out, 'model.bin')):
        ctranslate2.converters.TransformersConverter(src).convert(out, quantization='int8', force=True)
    # (the original weights go once converted - hundreds of MB a model; 60 of them filled the disk: only the
    #  tokenizer's small files are needed from here)
    for f in os.listdir(src):
        if f.endswith(('.bin', '.safetensors', '.h5', '.msgpack', '.ot')):
            os.remove(os.path.join(src, f))
    t0 = time.perf_counter()
    tr = ctranslate2.Translator(out, device='cpu', inter_threads=1, intra_threads=THREADS)
    tok = MarianTokenizer.from_pretrained(src)
    cache[name] = (tr, tok, dir_bytes(out), time.perf_counter() - t0)
    return cache[name]


def hop(m, token, text):
    tr, tok, _, _ = m
    s = f'>>{token}<< {text}' if token else text
    toks = tok.convert_ids_to_tokens(tok.encode(s))
    res = tr.translate_batch([toks], beam_size=BEAM, max_decoding_length=256)
    return tok.decode(tok.convert_tokens_to_ids(res[0].hypotheses[0]), skip_special_tokens=True)


def main():
    os.makedirs(OUT, exist_ok=True)
    jobs = json.load(open(os.path.join(HERE, 'jobs.json')))
    cache = {}
    for j in jobs:
        if not j['opus']:
            continue
        path = os.path.join(OUT, f"{j['src']}-{j['tgt']}.json")
        if os.path.exists(path):
            continue
        ms = [(get_model(n, cache), t) for n, t in j['opus']]
        src = open(os.path.join(HERE, 'data', f"{j['src']}.txt"), encoding='utf-8').read().rstrip('\n').split('\n')
        for m, t in ms:
            hop(m, t, 'Warm up.')
        hyps, times = [], []
        for s in src:
            t0 = time.perf_counter()
            x = s
            for m, t in ms:
                x = hop(m, t, x)
            times.append((time.perf_counter() - t0) * 1000)
            hyps.append(x)
        json.dump(dict(route=[n + (f' >>{t}<<' if t else '') for n, t in j['opus']], hops=len(ms),
                       modelBytes=sum(m[2] for m, _ in ms), loadMs=[m[3] * 1000 for m, _ in ms], times=times,
                       hyps=hyps), open(path, 'w', encoding='utf-8'), ensure_ascii=False)
        print(f"{j['src']}->{j['tgt']} ({len(ms)} hop{'s' if len(ms) > 1 else ''}): {sum(times) / len(times):.0f} ms a "
              f"sentence", flush=True)
    print('done')


if __name__ == '__main__':
    main()
