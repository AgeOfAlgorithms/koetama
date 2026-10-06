"""Build the helper's spoken-language detector: SpeechBrain's lang-id-voxlingua107-ecapa (Apache-2.0; trained on
VoxLingua107, CC-BY-4.0) as one ONNX file - audio in, 107 language log-probabilities out - so the helper needs no
PyTorch. Replaces NVIDIA AmberNet (NGC terms: no redistribution).

    <conda>/envs/pclid/python.exe engine/export_lid.py

(env pclid: torch CPU + speechbrain + onnx + onnxruntime.) Writes export/lid/voxlingua107-ecapa.fp32.onnx (86 MB),
then voxlingua107-ecapa.onnx - the one the app ships (43 MB): the same model with its weights STORED as float16, cast
back to float32 when it loads, so it computes as the float32 one, at its speed (bench/lidquant.py, 2026-10-06: the
same language on every benchmark recording, 99.9 % of 1 s windows; int8 lost a third of the one-word callouts) - and
voxlingua107-ecapa.json (the language codes). Then it checks both files against SpeechBrain itself on real
recordings of several lengths.

torch.stft makes complex tensors, which the ONNX exporter cannot take: the STFT here is two convolutions (cosine and
sine kernels times SpeechBrain's own window, the same centring and padding); everything after it is SpeechBrain's own
modules (spectral_magnitude, its Filterbank, ECAPA_TDNN, the classifier) and the per-line mean normalisation.
"""
import json
import os
import sys

import numpy as np
import torch
import torch.nn.functional as F

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
OUT = os.path.join(ROOT, 'export', 'lid')
NAME = 'voxlingua107-ecapa'
SOURCE = 'speechbrain/lang-id-voxlingua107-ecapa'


class LangId(torch.nn.Module):
    def __init__(self, sb):
        super().__init__()
        from speechbrain.processing.features import spectral_magnitude
        self.mag = spectral_magnitude
        st = sb.mods.compute_features.compute_STFT
        assert st.center and not st.normalized_stft and st.onesided, 'an STFT setting this export does not copy'
        self.n_fft, self.hop, self.pad_mode = st.n_fft, st.hop_length, st.pad_mode
        win = st.window.float()
        if len(win) < self.n_fft:                         # (torch.stft centres a shorter window in n_fft)
            left = (self.n_fft - len(win)) // 2
            win = F.pad(win, (left, self.n_fft - len(win) - left))
        n = torch.arange(self.n_fft, dtype=torch.float64)
        k = torch.arange(self.n_fft // 2 + 1, dtype=torch.float64)[:, None]
        ang = 2 * np.pi * k * n / self.n_fft
        self.register_buffer('cos', (torch.cos(ang) * win.double()).float()[:, None, :])
        self.register_buffer('sin', (-torch.sin(ang) * win.double()).float()[:, None, :])
        self.fbanks = sb.mods.compute_features.compute_fbanks
        self.embed = sb.mods.embedding_model
        self.classify = sb.mods.classifier

    def forward(self, audio):                             # audio: [1, samples] float32 at 16 kHz
        x = audio[:, None, :]
        x = F.pad(x, (self.n_fft // 2, self.n_fft // 2), mode=self.pad_mode)
        re = F.conv1d(x, self.cos, stride=self.hop)
        im = F.conv1d(x, self.sin, stride=self.hop)
        stft = torch.stack([re, im], -1).transpose(1, 2)  # [1, frames, bins, 2] as SpeechBrain's STFT returns
        feats = self.fbanks(self.mag(stft))
        feats = feats - feats.mean(dim=1, keepdim=True)  # (InputNormalization, sentence, no std)
        emb = self.embed(feats)
        return self.classify(emb).squeeze(1)              # [1, 107] log-probabilities


def fp16_storage(src, dst):
    """the ONNX model at src, its float32 weights (1024+ values) stored as float16 with a Cast back to float32 in
    front of their use: half the file; onnxruntime folds the casts when it loads, so it runs in float32 as before"""
    import onnx
    from onnx import helper, numpy_helper, TensorProto
    m = onnx.load(src)
    g = m.graph
    casts = []
    for t in list(g.initializer):
        if t.data_type == TensorProto.FLOAT and int(np.prod(t.dims)) >= 1024:
            h = numpy_helper.from_array(numpy_helper.to_array(t).astype(np.float16), t.name + '_fp16')
            g.initializer.remove(t)
            g.initializer.append(h)
            casts.append(helper.make_node('Cast', [h.name], [t.name], to=TensorProto.FLOAT, name=t.name + '_cast'))
    for c in reversed(casts):
        g.node.insert(0, c)
    onnx.save(m, dst)
    return len(casts)


def main():
    from speechbrain.inference.classifiers import EncoderClassifier
    from speechbrain.utils.fetching import LocalStrategy
    import onnxruntime as ort
    torch.set_num_threads(4)
    os.makedirs(OUT, exist_ok=True)
    sb = EncoderClassifier.from_hparams(source=SOURCE, savedir=os.path.join(OUT, 'speechbrain'), run_opts={'device': 'cpu'},
                                        local_strategy=LocalStrategy.COPY)
    sb.eval()
    codes = [lab.split(':')[0].strip() for lab in sb.hparams.label_encoder.decode_ndim(list(range(107)))]
    model = LangId(sb).eval()
    full = os.path.join(OUT, NAME + '.fp32.onnx')
    path = os.path.join(OUT, NAME + '.onnx')
    with torch.no_grad():
        torch.onnx.export(model, (torch.zeros(1, 16000),), full, input_names=['audio'], output_names=['logprobs'],
                          dynamic_axes={'audio': {1: 'samples'}}, opset_version=17, dynamo=False)
    n = fp16_storage(full, path)
    json.dump(dict(source=SOURCE, license='Apache-2.0 (model), CC-BY-4.0 (VoxLingua107 data)', rate=16000, labels=codes),
              open(os.path.join(OUT, NAME + '.json'), 'w', encoding='utf-8'), indent=1)
    print('wrote %s (%.0f MB) and %s (%.0f MB: %d weights stored as float16)' % (
        full, os.path.getsize(full) / 1e6, path, os.path.getsize(path) / 1e6, n))

    # the check: real recordings (the benchmark's), cut to several lengths; SpeechBrain itself against both ONNX files
    sess = ort.InferenceSession(full, providers=['CPUExecutionProvider'])
    half = ort.InferenceSession(path, providers=['CPUExecutionProvider'])
    y = np.random.default_rng(0).standard_normal((1, 16000)).astype(np.float32) * 0.1
    d = float(np.abs(sess.run(None, {'audio': y})[0] - half.run(None, {'audio': y})[0]).max())
    print('the float16-stored file runs (largest log-probability difference from float32 on noise: %.4f)' % d)
    items_path = os.path.join(ROOT, 'export', 'asrbench', 'lid', 'items.json')
    if not os.path.exists(items_path):                  # (CI, a fresh clone: the benchmark's recordings are not here)
        print('no benchmark recordings (export/asrbench/lid): the check against SpeechBrain is skipped')
        return 0
    items = json.load(open(items_path, encoding='utf-8'))
    import wave
    worst, same, n = 0.0, 0, 0
    worst16, same16 = 0.0, 0
    for it in items[::9][:24]:
        w = wave.open(it['path'])
        x = np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(np.float32) / 32768
        for sec in (0.5, 1.0, 2.7):
            y = x[:int(sec * 16000)]
            with torch.no_grad():
                ref = sb.classify_batch(torch.from_numpy(y)[None])[0][0].numpy()
            got = sess.run(None, {'audio': y[None]})[0][0]
            worst = max(worst, float(np.abs(ref - got).max()))
            same += int(ref.argmax() == got.argmax())
            g16 = half.run(None, {'audio': y[None]})[0][0]
            worst16 = max(worst16, float(np.abs(ref - g16).max()))
            top2 = np.sort(ref)[-2:]
            # (a near tie in SpeechBrain itself - its top two within 0.05 - may go either way in float16)
            same16 += int(ref.argmax() == g16.argmax() or top2[1] - top2[0] < 0.05)
            n += 1
    print('checked %d cuts: the same top language in %d, largest log-probability difference %.5f' % (n, same, worst))
    print('  the float16-stored file: the same top language in %d, largest difference %.4f' % (same16, worst16))
    ok = same == n and worst < 1e-3 and same16 == n and worst16 < 0.1
    print('OK' if ok else 'MISMATCH')
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
