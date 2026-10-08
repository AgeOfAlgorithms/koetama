"""The helper's speech-to-text (PROTOTYPE): what the player says, as live words while they talk and as a
finished line after. Chosen by the benchmarks in bench/ (see PROJECT.md):

  speech detector   Silero VAD v5 (2.3 MB, MIT): where a line starts and ends
  the words         the line so far, transcribed again every ROLL_EVERY s while the player talks (the live words),
                    then once more over the whole line (the finished line), by the language's own model:
                      Russian                                GigaAM v3 CTC (Sber, MIT)
                      Mandarin, Cantonese, Japanese, Korean  SenseVoice Small (Alibaba, FunASR model license)
                      the other European languages           Parakeet TDT 0.6B v3 int8 (NVIDIA, CC-BY-4.0)
  "auto" language   SpeechBrain's VoxLingua107 ECAPA language detector (Apache-2.0; our ONNX export,
                    engine/export_lid.py) cuts a line into stretches by language, each written by its
                    language's model
Everything runs on the CPU (sherpa-onnx / onnxruntime), on this machine; nothing is sent anywhere. The models are
downloaded once (Hugging Face) the first time they are needed.

    lst = Listener(on_live, on_final)       # on_live(utt, text, info), on_final(utt, text, info)
    lst.set_language('ru')                  # the game's "Language I speak" ('auto': found per stretch)
    lst.feed(samples_16k_float32)           # from the microphone, any block size; or in a thread: lst.start()
"""
import os
import queue
import json
import re
import threading
import time
import urllib.request

import numpy as np

RATE = 16000
PREROLL = 1.0              # s before the detected speech fed too
PTT_TAIL = 0.25            # push to talk: s of audio still taken after the key is let go, then the line ends
MAX_LINE = 15.0            # s: a longer line is cut
VAD_URL = 'https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad_v5.onnx'
import paths                                   # noqa: E402  (Kotodama's folders)
CACHE = paths.MODELS

MODELS = {   # name: (Hugging Face repo, the exact revision tested, the files used); the loader: Models._load_<name>
    # (pinned: a later change to a repo cannot change what the helper runs; only these files are downloaded - the
    #  SenseVoice repo also holds a 900 MB full-precision copy and test recordings the helper does not use)
    'gigaam': ('csukuangfj/sherpa-onnx-nemo-ctc-giga-am-v3-russian-2025-12-16', '32a4c7cc81809bd132e2d935ab99e9e6ab47fbec',
               ['model.int8.onnx', 'tokens.txt', 'LICENSE']),
    # (the 2024-07-17 release: the 2025-09-09 one is a Cantonese fine-tune that lost Japanese and Korean - 74 / 91 %
    #  of the characters wrong against 2.0 / 1.1 % here; bench/cjk.py)
    'sensevoice': ('csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17', '2365baeacb507f821a0c8120fcee3d484dba7a07',
                   ['model.int8.onnx', 'tokens.txt', 'LICENSE']),
    'parakeet': ('csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8', '2bda32ec70b097a55adaa07d9a7173915b43cc78',
                 ['encoder.int8.onnx', 'decoder.int8.onnx', 'joiner.int8.onnx', 'tokens.txt']),
}


# ---------------------------------------------------------------- words and when they were said
# A line's UNITS: runs of letters between spaces, and each CJK / kana character on its own (Chinese and Japanese
# have no spaces). The game splits the same way (voice.lua PC.voiceUnits; its PC.scriptOf "cjk" / "kana"
# ranges): each unit's start time travels with the text, so a listener who arrives (or leaves) mid-sentence
# gets only the words said while they were in reach.
WIDE = ((0x3040, 0x30FF), (0x31F0, 0x31FF), (0x1100, 0x11FF), (0x3000, 0x303F), (0x3130, 0x318F), (0x3400, 0x9FFF),
        (0xAC00, 0xD7AF), (0xF900, 0xFAFF), (0xFF00, 0xFFEF), (0x2B1A, 0x2B1A))
SPACE = ' \t\n\r\x0b\x0c'             # (Lua's %s: ASCII white space only)


def _wide(ch):
    c = ord(ch)
    return any(a <= c <= b for a, b in WIDE)


def units(text):
    """[(index of its first character, the unit)] of a text"""
    out, cur = [], None
    for i, ch in enumerate(text):
        if ch in SPACE:
            cur = None
        elif _wide(ch):
            out.append([i, ch])
            cur = None
        elif cur is None:
            cur = [i, ch]
            out.append(cur)
        else:
            cur[1] += ch
    return [(i, u) for i, u in out]


def unit_times(text, tokens, stamps, offset=0.0, dur=None):
    """the start time (s, + offset) of each unit of text, from a model's tokens and their timestamps (sherpa-onnx
    result.tokens / .timestamps; SentencePiece's '▁' is a space). text may differ a little from the tokens
    (tidy's capital, spaces): each character is found in the tokens' text a few places ahead. No tokens: spread
    evenly over dur."""
    us = units(text)
    if not us:
        return []
    if not tokens or len(tokens) != len(stamps):
        d = dur or 0.0
        return [round(offset + d * k / len(us), 2) for k in range(len(us))]
    concat, ct = '', []
    for tok, t in zip(tokens, stamps):
        piece = tok.replace('\u2581', ' ')
        concat += piece
        ct += [float(t)] * len(piece)
    times, j, last = [], 0, float(stamps[0])
    for ch in text:
        for k in range(j, min(len(concat), j + 4)):
            if concat[k].lower() == ch.lower():
                last, j = ct[k], k + 1
                break
        times.append(last)
    out = [round(offset + times[i], 2) for i, _ in us]
    for k in range(1, len(out)):                      # (never earlier than the unit before)
        out[k] = max(out[k], out[k - 1])
    return out


def unit_key(u):
    """a unit compared between two passes: lower case, no punctuation"""
    return re.sub(r'[^\w]', '', u.lower())


def tidy(text):
    """a transcript as a chat line: spaces, a capital letter (GigaAM writes lower case without punctuation)"""
    text = re.sub(r'\s+', ' ', text).strip()
    if not re.search(r'\w', text):
        return ''
    return text[0].upper() + text[1:] if text[0].islower() else text


class Models:
    """loads each model once, on first use (downloading it the first time)"""
    def __init__(self, threads=4, log=print):
        self.threads, self.log = threads, log
        self.loaded = {}
        self.lock = threading.Lock()
        self.downloading = None                # (a download going on: (file, bytes done, bytes total), for the window)

    def _repo(self, repo, revision=None, files=None):
        """the folder of a model's files: pinned files through fetch.py (no Hugging Face library in the app); a whole
        repo (the benchmarks') through huggingface_hub"""
        if files:
            import fetch

            def progress(name, done, total):
                self.downloading = (name, done, total)
            try:
                return fetch.repo_files(repo, revision, files, log=self.log, progress=progress)
            finally:
                self.downloading = None
        from huggingface_hub import snapshot_download
        return snapshot_download(repo, revision=revision)

    def model_dir(self, name):
        """the folder of a model in MODELS: its pinned revision, only the files used (downloaded the first time)"""
        return self._repo(*MODELS[name])

    def vad(self):
        import sherpa_onnx as so
        path = os.path.join(CACHE, 'silero_vad_v5.onnx')
        if not os.path.exists(path):
            os.makedirs(CACHE, exist_ok=True)
            urllib.request.urlretrieve(VAD_URL, path + '.part')
            os.replace(path + '.part', path)
        cfg = so.VadModelConfig()
        cfg.silero_vad.model = path
        cfg.silero_vad.threshold = 0.5
        cfg.silero_vad.min_silence_duration = 0.5      # (a pause this long ends the line)
        cfg.silero_vad.min_speech_duration = 0.15
        cfg.silero_vad.max_speech_duration = MAX_LINE
        cfg.silero_vad.window_size = 512
        cfg.sample_rate = RATE
        cfg.num_threads = 1
        return so.VoiceActivityDetector(cfg, buffer_size_in_seconds=MAX_LINE + 5), cfg.silero_vad.window_size

    def get(self, name):
        with self.lock:
            if name not in self.loaded:
                t0 = time.perf_counter()
                self.loaded[name] = getattr(self, '_load_' + name)()
                self.log('speech model "%s" ready (%.1f s)' % (name, time.perf_counter() - t0))
            return self.loaded[name]

    def _load_gigaam(self):
        import sherpa_onnx as so
        d = self.model_dir('gigaam')
        return so.OfflineRecognizer.from_nemo_ctc(model=os.path.join(d, 'model.int8.onnx'), tokens=os.path.join(d, 'tokens.txt'),
                                                 num_threads=self.threads)

    def _load_sensevoice(self):
        import sherpa_onnx as so
        d = self.model_dir('sensevoice')
        return so.OfflineRecognizer.from_sense_voice(model=os.path.join(d, 'model.int8.onnx'), tokens=os.path.join(d, 'tokens.txt'),
                                                    num_threads=self.threads, language='auto', use_itn=True)   # (it tells zh / yue / ja / ko apart itself; a fixed language changed nothing measurable)

    def _load_parakeet(self):
        import sherpa_onnx as so
        d = self.model_dir('parakeet')
        j = lambda f: os.path.join(d, f)
        return so.OfflineRecognizer.from_transducer(encoder=j('encoder.int8.onnx'), decoder=j('decoder.int8.onnx'), joiner=j('joiner.int8.onnx'),
                                                   tokens=j('tokens.txt'), num_threads=self.threads, model_type='nemo_transducer')

    def offline(self, name, audio):
        """a whole line through an offline model: (text, seconds)"""
        text, _, _, took = self.offline_full(name, audio)
        return text, took

    def offline_full(self, name, audio):
        """... and its tokens with their timestamps: (text, tokens, timestamps, seconds)"""
        r = self.get(name)
        t0 = time.perf_counter()
        s = r.create_stream()
        s.accept_waveform(RATE, np.concatenate([audio, np.zeros(int(RATE * 0.3), np.float32)]))
        r.decode_stream(s)
        res = s.result
        return res.text.strip(), list(res.tokens), list(res.timestamps), time.perf_counter() - t0

    def offline_timed(self, name, audio, offset=0.0):
        """a whole line, tidied, with the start time of each unit: (text, [times], seconds)"""
        raw, tokens, stamps, took = self.offline_full(name, audio)
        text = tidy(raw)
        return text, unit_times(text, tokens, stamps, offset, len(audio) / RATE), took


ROLL_EVERY = 1.0           # s: a rolling line is transcribed again this often...
ROLL_SLOW = 0.5            # ...unless a pass takes more than this share of it: then twice as long (up to ROLL_MAX),
ROLL_MAX = 4.0             #    remembered for the session (Models.every): a slow PC gets fewer, later live words
ROLL_MODEL = {'ru': 'gigaam', 'zh': 'sensevoice', 'yue': 'sensevoice', 'ja': 'sensevoice', 'ko': 'sensevoice'}   # (else Parakeet)


def roll_model(lang):
    """the one model of a language in the rolling design: its own recogniser, else Parakeet v3"""
    return ROLL_MODEL.get(lang, 'parakeet')


# ---------------------------------------------------------------- mixed languages ("auto": the language decided per stretch)
LID_NAME = 'voxlingua107-ecapa'   # SpeechBrain lang-id-voxlingua107-ecapa (Apache-2.0) as ONNX: export_lid.py builds it
# (the languages "auto" chooses between; Cantonese is found as zh and SenseVoice writes it. Not every supported one:
#  more candidates, more wrong stretches - the 13 supported ones made Russian lines 18 % wrong against 8 % (taken for
#  Ukrainian), English 5 % against 3 %; bench/autolangs.py. A player of another language picks it.)
MIXED_LANGS = ['en', 'ru', 'zh', 'es', 'de', 'fr', 'it', 'pt', 'ja', 'ko']
LID_WIN, LID_MIN = 1.0, 0.5   # s: windows, the shortest stretch kept (bench/lid.py)
LID_HOP = 0.25                # s between two windows (0.5: half the detector's cost - bench/budget.py)
LID_QUIET = 25.0              # dB under the line's loudest 0.25 s (or under -50 dBFS): quiet, it does not vote
LID_SWITCH = 4.0              # what a change of language costs (log-probability) - bench/lidtune.py
LID_SURE = 0.8                # a short line: the detector's language only when this sure, else the fallback


def lid_dir():
    """where the language detector is: Kotodama's model folder (downloaded), the install's models folder (shipped
    with it), else this repo's export/lid (where export_lid.py writes it)"""
    for d in (CACHE, os.path.join(paths.APP_ROOT, 'models'), os.path.join(paths.APP_ROOT, 'export', 'lid')):
        if os.path.exists(os.path.join(d, LID_NAME + '.onnx')) and os.path.exists(os.path.join(d, LID_NAME + '.json')):
            return d
    raise RuntimeError('the language detector (%s.onnx) is missing: build it with engine/export_lid.py' % LID_NAME)


_LID_LABELS = []   # (the detector's 107 labels, once it is loaded)


def detector_label(lang):
    """the detector's label for a language (Cantonese is found as Chinese; SenseVoice writes both)"""
    return 'zh' if lang == 'yue' else lang


def _lid_load(models):
    import onnxruntime as ort
    d = lid_dir()
    so = ort.SessionOptions()
    so.intra_op_num_threads = models.threads
    sess = ort.InferenceSession(os.path.join(d, LID_NAME + '.onnx'), so, providers=['CPUExecutionProvider'])
    labels = json.load(open(os.path.join(d, LID_NAME + '.json'), encoding='utf-8'))['labels']
    _LID_LABELS[:] = labels
    return sess, [labels.index(l) for l in MIXED_LANGS]


Models._load_langid = _lid_load


def lid_probs(models, x, langs=None):
    """the language detector's probabilities over MIXED_LANGS (or langs: the ones a player speaks) for a stretch of
    audio"""
    sess, idx = models.get('langid')
    if langs is not None:
        idx = [_LID_LABELS.index(detector_label(l)) for l in langs]
    if len(x) < 1600:
        x = np.concatenate([x, np.zeros(1600 - len(x), np.float32)])
    z = sess.run(None, {'audio': x[None].astype(np.float32)})[0][0][idx]
    p = np.exp(z - z.max())
    return p / p.sum()


def quiet_point(x, t, span=0.3):
    """the quietest 20 ms within +-span s of t: a cut there rarely splits a word"""
    a, b = int(max(0, t - span) * RATE), int(min(len(x) / RATE, t + span) * RATE)
    best, bt = 1e9, t
    for k in range(a, max(a + 1, b - 320), 80):
        e = float(np.mean(x[k:k + 320] ** 2))
        if e < best:
            best, bt = e, (k + 160) / RATE
    return bt


def segments(models, x, fallback='en', cache=None, langs=None):
    """[(lang, start s, end s)] of a line that may change language. The detector on LID_WIN s windows every LID_HOP s;
    each LID_HOP s frame scores each language by the mean log-probability of the windows covering it. Quiet frames
    (LID_QUIET) and windows mostly quiet do not vote: before this, the quiet around a word (and the helper's 1 s of
    audio from before the speech) got a language of its own - "Okay." came out French + Russian. The frames' languages
    are then the best path where each change costs LID_SWITCH (Viterbi): a line keeps its language unless the evidence
    for a change is strong. Stretches under LID_MIN s join a neighbour; each cut goes to the quietest point near the
    change. Less than LID_WIN + 2 hops of SPEECH (one word, a short call): one stretch, in the detector's language if
    it is LID_SURE, else `fallback` (the helper passes the language of the player's line before).
    (Measured with SpeechBrain, bench/lidtune.py: single-language lines and one-word lines better than the
    plain 3-frame vote and than AmberNet had them; mixed lines 11 % words wrong against AmberNet's 7.)
    (Cut by LANGUAGE, not by the model that writes it: Parakeet decides one language per clip, so English and German
    handed to it as one piece lose one of them - grouping by model tried 2026-10-05: 17 % words wrong against 7 %.)
    cache: {window start: probabilities} kept by a growing line - its earlier windows never change.
    langs: the candidate languages (the ones a player speaks: fewer candidates, fewer wrong stretches); None:
    MIXED_LANGS."""
    names = list(langs) if langs is not None else MIXED_LANGS
    probs = (lambda w: lid_probs(models, w)) if langs is None else (lambda w: lid_probs(models, w, names))
    dur = len(x) / RATE
    L = len(names)
    hop = int(LID_HOP * RATE)
    n = max(1, int(np.ceil(len(x) / hop)))
    db = np.array([10 * np.log10(np.mean(x[i * hop:(i + 1) * hop] ** 2) + 1e-12) if len(x[i * hop:(i + 1) * hop]) else -120.0
                   for i in range(n)])
    voiced = db > max(db.max() - LID_QUIET, -50.0)
    if not voiced.any():
        return [(fallback, 0.0, dur)]
    v0, v1 = np.flatnonzero(voiced)[[0, -1]]
    if (v1 + 1 - v0) * LID_HOP < LID_WIN + LID_HOP * 2:   # (short speech: one stretch)
        p = probs(x[v0 * hop:(v1 + 1) * hop])
        lang = names[int(p.argmax())] if p.max() > LID_SURE else fallback
        return [(lang, 0.0, dur)]
    score, cnt = np.zeros((n, L)), np.zeros(n)
    t = 0.0
    while t + LID_WIN <= dur + 1e-6:
        f0, f1 = int(round(t / LID_HOP)), min(n, int(np.ceil((t + LID_WIN) / LID_HOP)))
        if voiced[f0:f1].mean() >= 0.3:                   # (a mostly quiet window does not vote)
            k = round(t / LID_HOP)
            if cache is not None and k in cache:
                p = cache[k]
            else:
                p = probs(x[int(t * RATE):int((t + LID_WIN) * RATE)])
                if cache is not None:
                    cache[k] = p
            score[f0:f1] += np.log(p + 1e-9)
            cnt[f0:f1] += 1
        t += LID_HOP
    has = (cnt > 0) & voiced
    score[has] /= cnt[has, None]
    score[~has] = 0.0                                     # (no evidence there: the path carries its language through)
    cost, back = np.zeros(L), np.zeros((n, L), int)
    for f in range(n):                                    # (Viterbi: staying is free, a change costs LID_SWITCH)
        move = cost.max() - LID_SWITCH
        back[f] = np.where(cost >= move, np.arange(L), int(cost.argmax()))
        cost = np.maximum(cost, move) + score[f]
    lab = np.zeros(n, int)
    lab[-1] = int(cost.argmax())
    for f in range(n - 1, 0, -1):
        lab[f - 1] = back[f, lab[f]]
    segs = []
    for f in range(n):
        l = names[lab[f]]
        if segs and segs[-1][0] == l:
            segs[-1][2] = (f + 1) * LID_HOP
        else:
            segs.append([l, f * LID_HOP, (f + 1) * LID_HOP])
    while len(segs) > 1:
        short = [i for i, s in enumerate(segs) if s[2] - s[1] < LID_MIN]
        if not short:
            break
        i = short[0]
        j = i - 1 if i > 0 else i + 1
        segs[j][1], segs[j][2] = min(segs[j][1], segs[i][1]), max(segs[j][2], segs[i][2])
        segs.pop(i)
        k = 0
        while k < len(segs) - 1:
            if segs[k][0] == segs[k + 1][0]:
                segs[k][2] = segs[k + 1][2]
                segs.pop(k + 1)
            else:
                k += 1
    segs[0][1], segs[-1][2] = 0.0, dur
    for i in range(1, len(segs)):
        cut = quiet_point(x, segs[i][1])
        segs[i - 1][2] = segs[i][1] = cut
    return [tuple(s) for s in segs]


def transcribe_mixed(models, x, fallback='en', cache=None, segs_out=None, times_out=None, langs=None):
    """a line in any of MIXED_LANGS, even several: cut into stretches, each written by its language's model.
    -> (text, [langs], seconds); segs_out (a list): gets the stretches [(lang, start, end)]; times_out (a list):
    the start time of each unit of the text"""
    t0 = time.perf_counter()
    segs = segments(models, x, fallback, cache, langs)
    if segs_out is not None:
        segs_out[:] = segs
    parts, times = [], []
    for lang, a, b in segs:
        text, tt, _ = models.offline_timed(roll_model(lang), x[int(a * RATE):int(b * RATE)], offset=a)
        if text:
            parts.append(text)
            times += tt
    if times_out is not None:
        times_out[:] = times
    return ' '.join(parts), [s[0] for s in segs], time.perf_counter() - t0


class RollingLine:
    """the rolling design (bench/rolling.py; no streaming model): the line so far transcribed again every
    ROLL_EVERY s by the language's own model, shown at once; at the end one pass over the whole line. Lines are
    at most MAX_LINE s, where a whole-line pass still costs well under real time (no window needed).
    lang "auto": the line may change language - each pass is cut into stretches by language (the detector) and each
    stretch written by its language's model (transcribe_mixed)."""
    def __init__(self, models, utt, lang, preroll, fallback='en', live=True):
        self.models, self.utt, self.lang, self.fallback, self.live = models, utt, lang, fallback, live
        self.model = roll_model(lang)
        self.audio = [preroll]
        self.t0 = time.perf_counter() - len(preroll) / RATE   # (when the line's audio begins: word times count from here)
        self.committed = []                                  # (the live words so far: [(space before, unit, time)])
        self.prev = None                                     # (the last pass's units, compared: unit_key)
        self.times = []
        self.text = ''
        self.speech = 0.0
        self.next = getattr(models, 'every', ROLL_EVERY)
        self.compute = 0.0
        self.passes = 0
        self.langs = [lang]
        self.segs = []
        self.lid_cache = {}

    def _pass(self, audio):
        """(text, [unit times], seconds)"""
        if self.lang == 'auto':
            times = []
            text, self.langs, took = transcribe_mixed(self.models, audio, self.fallback, self.lid_cache, self.segs, times)
            return tidy(text), times, took
        return self.models.offline_timed(self.model, audio)

    def _commit(self, text, times):
        """the live words: only units two passes in a row agree on, never the last one (it may be cut off), and
        once shown never taken back - the bubble fills chunk by chunk (LocalAgreement). True if it grew."""
        us = units(text)
        keys = [unit_key(u) for _, u in us]
        grew = False
        if self.prev is not None and len(times) == len(us):
            k = 0
            while k < min(len(keys), len(self.prev)) and keys[k] == self.prev[k]:
                k += 1
            k = min(k, len(us) - 1)
            for i in range(len(self.committed), k):
                start, u = us[i]
                end = us[i - 1][0] + len(us[i - 1][1]) if i > 0 else start
                sep = text[end:start] if i > 0 else ''
                if i > 0 and not sep and not (_wide(u[0]) or _wide(us[i - 1][1][-1])):
                    sep = ' '                                # (two units that were apart stay apart)
                self.committed.append((sep, u, times[i]))
                grew = True
        self.prev = keys
        return grew

    def feed(self, x):
        self.audio.append(x)
        self.speech += len(x) / RATE
        if not self.live or self.speech < self.next:            # (live words off: only the finished line)
            return None
        every = getattr(self.models, 'every', ROLL_EVERY)
        text, times, took = self._pass(np.concatenate(self.audio))
        if took > ROLL_SLOW * every and every < ROLL_MAX:         # (this PC is slow for it: pass less often)
            self.models.every = every = min(ROLL_MAX, every * 2)
            self.models.log('live words every %.0f s (a pass took %.2f s)' % (every, took))
        self.next = self.speech + every
        self.compute += took
        self.passes += 1
        if text and self._commit(text, times):
            self.text = ''.join(sep + u for sep, u, _ in self.committed)
            self.times = [t for _, _, t in self.committed]
            return self.text
        return None

    def finish(self):
        t0 = time.perf_counter()
        text, times, took = self._pass(np.concatenate(self.audio + [np.zeros(int(RATE * 0.2), np.float32)]))
        used = '+'.join(dict.fromkeys(self.langs)) if self.lang == 'auto' else self.model
        main = self.lang                                      # (auto: the language it was mostly in)
        if self.lang == 'auto' and self.segs:
            span = {}
            for l, a, b in self.segs:
                span[l] = span.get(l, 0.0) + b - a
            main = max(span, key=span.get)
        return text, dict(live=self.text, used=used, lang=main, speech=self.speech, second_s=took, times=times, t0=self.t0,
                          finish_s=time.perf_counter() - t0, live_cpu=self.compute, passes=self.passes)


def low_priority():
    """the speech work yields to the game: this process below normal priority (its threads, the speech models'
    worker threads too), so when they compete the live words fall behind instead of Teardown stuttering. The audio
    output keeps its own (raised) priority - PortAudio's WASAPI thread uses Windows' multimedia class (MMCSS)."""
    try:
        import ctypes
        k32 = ctypes.windll.kernel32
        k32.SetPriorityClass(k32.GetCurrentProcess(), 0x00004000)   # BELOW_NORMAL_PRIORITY_CLASS
    except Exception:
        pass


class Listener:
    """the microphone's audio in (16 kHz float32, any block size), lines out: on_live(utt, words, info) while the
    player talks (only ever growing; info: times = each unit's start, s from t0 = when the line's audio began,
    time.perf_counter()), on_final(utt, text, info) after (text may be '': nothing made out; info has times, t0)"""
    def __init__(self, on_live, on_final, threads=4, log=print, models=None, live=True, on_start=None):
        self.on_live, self.on_final, self.log = on_live, on_final, log
        self.on_start = on_start                     # (on_start(utt): the speech detector heard a line begin - at once)
        self.live = live                             # (False: no live words, only the finished line - less CPU)
        self.models = models or Models(threads, log)
        self.vad, self.win = self.models.vad()
        self.lang = 'en'
        self.last_lang = 'en'                        # (auto: the language of the line before - a short line's fallback)
        self.ring = np.zeros(0, np.float32)          # the last PREROLL s (before speech is detected)
        self.pending = np.zeros(0, np.float32)       # not yet a whole VAD window
        self.line = None
        self.utt = 0
        self.ptt = None                              # push to talk: None (always on), False (the key up), True (held)
        self.tail = 0                                # (push to talk: samples taken since the key was let go)
        self.q = queue.Queue(maxsize=200)
        self.thread = None
        self.running = False

    def set_language(self, lang):
        self.lang = lang or 'en'

    def set_push_to_talk(self, held):
        """push to talk: held True / False - a line starts only while the key is held (the speech detector still finds
        where the speech begins, from the last PREROLL s), and ends PTT_TAIL s after it is let go; None - always on"""
        self.ptt = held

    def warm(self, lang=None):
        """load the models a language needs now (the first line would wait for them otherwise)"""
        lang = lang or self.lang
        if lang == 'auto':                           # (mixed: the detector and every language's model)
            for name in ['langid'] + list(dict.fromkeys(roll_model(l) for l in MIXED_LANGS)):
                self.models.get(name)
        else:
            self.models.get(roll_model(lang))

    def feed(self, x):
        x = np.asarray(x, dtype=np.float32).reshape(-1)
        self.ring = np.concatenate([self.ring, x])[-int(RATE * PREROLL):]
        if self.line is not None:
            words = self.line.feed(x)
            if words is not None:
                self.on_live(self.utt, words, dict(times=list(self.line.times), t0=self.line.t0))
        self.pending = np.concatenate([self.pending, x])
        while len(self.pending) >= self.win:
            w, self.pending = self.pending[:self.win], self.pending[self.win:]
            self.vad.accept_waveform(w)
            if self.line is None and self.vad.is_speech_detected() and self.ptt is not False:
                self.utt += 1
                self.line = RollingLine(self.models, self.utt, self.lang, self.ring.copy(), fallback=self.last_lang, live=self.live)
                if self.on_start:
                    self.on_start(self.utt)
            while not self.vad.empty():                   # (a finished segment: the line ends)
                self.vad.pop()
                self.end()
        if self.line is not None and self.line.speech > MAX_LINE + 1:
            self.end()
        # push to talk, the key let go: the line ends a moment later, and the speech detector starts over
        if self.ptt is False:
            if self.line is not None:
                self.tail += len(x)
                if self.tail >= int(RATE * PTT_TAIL):
                    self.end()
                    self.vad.reset()
                    self.pending = np.zeros(0, np.float32)
        else:
            self.tail = 0

    def end(self):
        if self.line is None:
            return
        line, self.line = self.line, None
        text, info = line.finish()
        if text and info.get('lang') in MIXED_LANGS:
            self.last_lang = info['lang']
        self.on_final(line.utt, text, info)

    def flush(self):
        """the end of the input: a line still going is finished"""
        self.vad.flush()
        while not self.vad.empty():
            self.vad.pop()
        self.end()

    # (in a thread: the microphone callback only queues)
    def push(self, x):
        try:
            self.q.put_nowait(np.asarray(x, dtype=np.float32).copy())
        except queue.Full:
            pass                                          # (far behind: drop a block rather than lag forever)

    def start(self):
        self.running = True
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()

    def _run(self):
        low_priority()
        while self.running:
            try:
                x = self.q.get(timeout=0.2)
            except queue.Empty:
                continue
            try:
                self.feed(x)
            except Exception as e:
                self.log('speech: %s' % e)

    def stop(self):
        self.running = False
        self.flush()


class WavMicrophone:
    """a recording played into the listener in real time as if it were the microphone (tests without one):
    from the start each time it is opened, then quiet"""
    def __init__(self, listener, audio, log=print):
        self.listener, self.audio, self.log = listener, audio, log
        self.thread = None
        self.level = -120.0

    def is_open(self):
        return self.thread is not None

    def open(self):
        if self.thread is None:
            self.thread = threading.Thread(target=self._run, daemon=True)
            self.thread.start()
            self.log('microphone: playing the recording (%.1f s) as the microphone' % (len(self.audio) / RATE))
        return True

    def _run(self):
        n = int(RATE * 0.05)
        t0 = time.perf_counter()
        k = 0
        while self.thread is not None:
            x = self.audio[k:k + n] if k < len(self.audio) else np.zeros(n, np.float32)
            self.level = 20 * np.log10(max(1e-6, float(np.sqrt(np.mean(x * x)))))
            self.listener.push(x)
            k += n
            delay = t0 + k / RATE - time.perf_counter()
            if delay > 0:
                time.sleep(delay)

    def close(self):
        self.thread = None


class PlaylistMicrophone(WavMicrophone):
    """recorded lines played into the listener in real time, one after another, each in its own language (the
    listener is switched to it first): the whole pipeline on real audio, for watching it in the game without a
    microphone. items: [(lang, audio 16 kHz float32, what is said)]"""
    def __init__(self, listener, items, gap=2.5, log=print):
        super().__init__(listener, np.zeros(0, np.float32), log)
        self.items, self.gap = items, gap
        self.done = False

    def open(self):
        if self.thread is None:
            self.thread = threading.Thread(target=self._run, daemon=True)
            self.thread.start()
            self.log('auto speech: %d recorded lines through the real speech-to-text, %.1f s apart' % (len(self.items), self.gap))
        return True

    def _run(self):
        n = int(RATE * 0.05)
        rng = np.random.default_rng(4)
        for k, (lang, audio, said) in enumerate(self.items):
            if self.thread is None:
                return
            self.listener.warm(lang)
            self.listener.set_language(lang)
            self.log('  playing %d of %d [%s]: %s' % (k + 1, len(self.items), lang, said))
            x = np.concatenate([audio, (rng.standard_normal(int(RATE * self.gap)) * 0.002).astype(np.float32)])
            t0 = time.perf_counter()
            for i in range(0, len(x), n):
                if self.thread is None:
                    return
                blk = x[i:i + n]
                self.level = 20 * np.log10(max(1e-6, float(np.sqrt(np.mean(blk * blk)))))
                self.listener.push(blk)
                delay = t0 + (i + n) / RATE - time.perf_counter()
                if delay > 0:
                    time.sleep(delay)
        self.done = True
        self.log('auto speech: all lines played')
        while self.thread is not None:                   # (then quiet)
            self.listener.push(np.zeros(n, np.float32))
            time.sleep(0.05)


class Microphone:
    """the default microphone at 16 kHz (Windows converts), blocks to listener.push; open only while wanted"""
    def __init__(self, listener, device=None, log=print):
        self.listener, self.device, self.log = listener, device, log
        self.stream = None
        self.level = -120.0

    def is_open(self):
        return self.stream is not None

    def open(self):
        if self.stream is not None:
            return True
        from audio import _sounddevice   # (finds a packaged Linux build's own PortAudio)
        sd = _sounddevice()
        device, extra = self.device, None
        if device is None:
            try:
                for api in sd.query_hostapis():
                    if 'WASAPI' in api['name'] and api['default_input_device'] >= 0:
                        device, extra = api['default_input_device'], sd.WasapiSettings(auto_convert=True)
            except Exception:
                device, extra = None, None

        def callback(indata, frames, t, st):
            x = indata[:, 0]
            self.level = 20 * np.log10(max(1e-6, float(np.sqrt(np.mean(x * x)))))
            self.listener.push(x)
        for dev, ex in ((device, extra), (self.device, None)):
            try:
                self.stream = sd.InputStream(samplerate=RATE, channels=1, dtype='float32', blocksize=int(RATE * 0.05),
                                             device=dev, extra_settings=ex, callback=callback)
                self.stream.start()
                self.log('microphone on: %s' % sd.query_devices(self.stream.device, 'input')['name'])
                return True
            except Exception as e:
                self.stream = None
                err = e
        self.log('the microphone could not be opened: %s' % err)
        return False

    def close(self):
        if self.stream is not None:
            try:
                self.stream.stop()
                self.stream.close()
            except Exception:
                pass
            self.stream = None
            self.log('microphone off')
