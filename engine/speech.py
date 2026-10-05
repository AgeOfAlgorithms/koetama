"""REPLACED by asr.py (2026-10-04); kept for test_helper.py's detector tests and the Whisper comparison.
The helper's ears (first PROTOTYPE): the microphone, a speech detector that cuts what is said into
utterances, and a transcriber (a small Whisper model through faster-whisper, on the CPU, on this machine;
nothing is sent anywhere). helper.py turns each transcript into a chat line.
"""
import math
import queue
import re
import threading
import time

import numpy as np
from scipy.signal import resample_poly

ASR_RATE = 16000


def dbfs(x):
    """level of a block of samples in dB below full scale"""
    if len(x) == 0:
        return -120.0
    return 20 * math.log10(max(1e-6, math.sqrt(float(np.mean(np.square(x))))))


class SpeechDetector:
    """Cuts a stream into utterances by loudness against the room's noise.
    feed(block) -> list of finished utterances (float32 arrays at `rate`).
    Speech starts when `start_frames` of the last `start_frames + 1` frames (30 ms each) are `margin` dB above
    the noise floor (and above `min_db`), reaching back `pre` s; it ends after `hang` s below; an utterance
    longer than `max_len` s is cut; one with less than `min_voiced` s of loud frames is dropped.
    The noise floor: the first `warm` s after opening set it (nothing is speech yet); then it follows the
    quiet frames. A sound that holds one level for `steady` s (within `steady_db` dB: a fan, a hum - speech
    goes up and down with every syllable) is noise: the utterance is dropped and the floor moves there."""
    FRAME = 0.03

    def __init__(self, rate, margin=12.0, min_db=-45.0, pre=0.3, hang=0.7, max_len=15.0, min_voiced=0.25, start_frames=3,
                 warm=0.45, steady=1.2, steady_db=5.0):
        self.rate = rate
        self.n = int(rate * self.FRAME)
        self.margin, self.min_db = margin, min_db
        self.pre_frames = int(round(pre / self.FRAME))
        self.hang_frames = int(round(hang / self.FRAME))
        self.max_frames = int(round(max_len / self.FRAME))
        self.min_voiced = int(round(min_voiced / self.FRAME))
        self.start_frames = start_frames
        self.warm_frames = int(round(warm / self.FRAME))
        self.steady_frames = int(round(steady / self.FRAME))
        self.steady_db = steady_db
        self.warm = []                 # the first frames' levels (they set the floor)
        self.levels = []               # the last steady_frames levels of the utterance being said
        self.floor = -60.0             # the room's noise, dB
        self.buf = np.zeros(0, dtype=np.float32)
        self.recent = []               # the last frames while silent (the pre-roll)
        self.flags = []                # loud / not, the last start_frames + 1 frames
        self.cur = None                # frames of the utterance being said
        self.voiced = 0
        self.quiet = 0
        self.level = -120.0            # the last frame's level (for a meter)
        self.talking = False

    def threshold(self):
        return max(self.floor + self.margin, self.min_db)

    def feed(self, block):
        out = []
        self.buf = np.concatenate([self.buf, np.asarray(block, dtype=np.float32).reshape(-1)])
        while len(self.buf) >= self.n:
            frame, self.buf = self.buf[:self.n], self.buf[self.n:]
            done = self._frame(frame)
            if done is not None:
                out.append(done)
        return out

    def _frame(self, frame):
        level = dbfs(frame)
        self.level = level
        if len(self.warm) < self.warm_frames:                       # (just opened: learn the room first)
            self.warm.append(level)
            if len(self.warm) == self.warm_frames:
                self.floor = max(-80.0, min(-30.0, float(np.median(self.warm))))
            return None
        loud = level > self.threshold()
        if self.cur is None:
            # the floor follows the room: quickly down, slowly up (so speech does not raise it much)
            self.floor += (level - self.floor) * (0.2 if level < self.floor else 0.02)
            self.floor = max(-80.0, min(-30.0, self.floor))
            self.recent.append(frame)
            if len(self.recent) > self.pre_frames:
                self.recent.pop(0)
            self.flags.append(loud)
            if len(self.flags) > self.start_frames + 1:
                self.flags.pop(0)
            if sum(self.flags) >= self.start_frames:
                self.cur, self.recent, self.flags = list(self.recent), [], []
                self.voiced, self.quiet, self.talking = self.start_frames, 0, True
                self.levels = []
            return None
        self.cur.append(frame)
        self.levels.append(level)
        if len(self.levels) > self.steady_frames:
            self.levels.pop(0)
        if len(self.levels) == self.steady_frames and max(self.levels) - min(self.levels) < self.steady_db:
            self.floor = max(-80.0, min(-30.0, float(np.mean(self.levels))))   # (one level held: noise, not speech)
            self.cur, self.talking, self.levels = None, False, []
            return None
        if loud:
            self.voiced += 1
            self.quiet = 0
        else:
            self.quiet += 1
        if self.quiet >= self.hang_frames or len(self.cur) >= self.max_frames:
            frames, voiced = self.cur, self.voiced
            self.cur, self.talking = None, False
            if voiced >= self.min_voiced:
                return np.concatenate(frames)
        return None


# what a Whisper model tends to "hear" in noise or silence: dropped unless the model is sure of it
PHANTOMS = {'thank you', 'thanks for watching', 'thank you for watching', 'thanks', 'you', 'bye', 'bye bye',
            'so', 'okay', 'the end', 'subtitles by the amara org community', 'please subscribe', 'i', 'oh', 'uh', 'um', 'hmm', 'mm'}
NONSPEECH = re.compile(r'\[[^\]]*\]|\([^)]*\)|\*[^*]*\*|♪')


def clean_transcript(text):
    """a transcript as a chat line: non-speech marks ([Music], (laughs)) out, spaces tidied"""
    text = NONSPEECH.sub(' ', text)
    text = re.sub(r'\s+', ' ', text).strip().strip('"').strip()
    return text


def keep_transcript(text, avg_logprob, no_speech_prob, compression_ratio=1.0):
    """is this a real utterance? (text already cleaned)"""
    if not text or not re.search(r'\w', text):
        return False
    if avg_logprob < -1.2 or compression_ratio > 2.4:
        return False
    if no_speech_prob > 0.6 and avg_logprob < -0.5:
        return False
    key = re.sub(r'[^\w ]', '', text.lower()).strip()
    if key in PHANTOMS and (avg_logprob < -0.6 or no_speech_prob > 0.3):
        return False
    return True


class Transcriber(threading.Thread):
    """utterances in (submit), text out (on_text(text, seconds of speech, seconds it took)); one at a time"""
    def __init__(self, on_text, model='base.en', language='en', threads=4, beam=5, log=print):
        super().__init__(daemon=True)
        self.on_text, self.model_name, self.language, self.threads, self.beam, self.log = on_text, model, language, threads, beam, log
        self.q = queue.Queue()
        self.model = None
        self.ready = threading.Event()
        self.busy = False
        self.stats = []                # (seconds of speech, seconds to transcribe)

    def load(self):
        from faster_whisper import WhisperModel
        t0 = time.perf_counter()
        self.model = WhisperModel(self.model_name, device='cpu', compute_type='int8', cpu_threads=self.threads)
        self.log('speech model "%s" loaded in %.1f s (%d threads)' % (self.model_name, time.perf_counter() - t0, self.threads))
        self.ready.set()

    def submit(self, audio, rate):
        if self.q.qsize() >= 4:        # (far behind: drop the oldest - a chat line minutes late is worse than none)
            try:
                self.q.get_nowait()
            except queue.Empty:
                pass
        self.q.put((audio, rate))

    def transcribe(self, audio, rate):
        """-> (text or '', seconds it took)"""
        if rate != ASR_RATE:
            g = math.gcd(ASR_RATE, int(rate))
            audio = resample_poly(audio, ASR_RATE // g, int(rate) // g)
        audio = np.asarray(audio, dtype=np.float32)
        peak = float(np.max(np.abs(audio))) if len(audio) else 0.0
        if peak > 0:
            audio = audio * min(20.0, 0.7 / peak)           # (a quiet microphone: bring it up)
        t0 = time.perf_counter()
        segments, _ = self.model.transcribe(audio, language=self.language, beam_size=self.beam, temperature=0.0,
                                            condition_on_previous_text=False, without_timestamps=True, vad_filter=False)
        parts = []
        for s in segments:
            text = clean_transcript(s.text)
            if keep_transcript(text, s.avg_logprob, s.no_speech_prob, s.compression_ratio):
                parts.append(text)
        return ' '.join(parts), time.perf_counter() - t0

    def run(self):
        try:
            self.load()
        except Exception as e:
            self.log('the speech model could not be loaded: %s' % e)
            return
        while True:
            audio, rate = self.q.get()
            self.busy = True
            try:
                text, took = self.transcribe(audio, rate)
                self.stats.append((len(audio) / rate, took))
                self.on_text(text, len(audio) / rate, took)
            except Exception as e:
                self.log('transcription failed: %s' % e)
            self.busy = False


class Microphone:
    """the default microphone (WASAPI), open only while wanted; blocks go to a SpeechDetector, finished
    utterances to on_utterance(audio, rate)"""
    def __init__(self, on_utterance, device=None, log=print):
        self.on_utterance, self.device, self.log = on_utterance, device, log
        self.stream = None
        self.detector = None
        self.rate = 0
        self.lock = threading.Lock()

    def is_open(self):
        return self.stream is not None

    def open(self):
        if self.stream is not None:
            return True
        import sounddevice as sd
        device = self.device
        if device is None:
            try:
                for api in sd.query_hostapis():
                    if 'WASAPI' in api['name'] and api['default_input_device'] >= 0:
                        device = api['default_input_device']
            except Exception:
                device = None
        try:
            info = sd.query_devices(device, 'input')
            self.rate = int(info['default_samplerate'])
            self.detector = SpeechDetector(self.rate)

            def callback(indata, frames, t, st):
                for utt in self.detector.feed(indata[:, 0]):
                    self.on_utterance(utt, self.rate)
            self.stream = sd.InputStream(samplerate=self.rate, channels=1, dtype='float32', blocksize=int(self.rate * 0.03),
                                         device=device, callback=callback)
            self.stream.start()
            self.log('microphone on: %s (%d Hz)' % (info['name'], self.rate))
            return True
        except Exception as e:
            self.stream = None
            self.log('the microphone could not be opened: %s' % e)
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
