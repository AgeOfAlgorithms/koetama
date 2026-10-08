"""The running app, game-independent: the chosen game's link, the voice mixer and its output, the speech-to-text and
the microphone. The window (koetama.py) and the command line (teardown_helper.py) both drive one of these: start(),
then tick() a few times a second, stop() at the end; status() says what is going on.

The microphone is open only while the game wants it (its feed's mic flag) and is running; the speech models load the
first time it is wanted (downloaded once), and again for another language.
"""
import threading
import time

from audio import Mixer, open_output


class Runtime:
    def __init__(self, game_cls, log=print, threads=4, out_device=None, mic_device=None, volume=1.0, lang=None,
                 mic_source=None, no_mic=False, game_args=None):
        self.game_cls, self.log, self.threads = game_cls, log, threads
        self.out_device, self.mic_device, self.volume = out_device, mic_device, volume
        self.lang_override = lang              # (None: the game's setting; "auto speech" sets its own per line)
        self.mic_source = mic_source           # (None: the real microphone; else make(listener) -> a microphone-like)
        self.no_mic = no_mic
        self.game_args = game_args or {}
        self.mixer = self.game = self.stream = self.listener = self.mic = None
        self.ready = None                      # (the speech models: None not asked yet, False loading, True ready)
        self.loaded_lang = None
        self.live_now = ''                     # (the words so far of the line being said)
        self.last_said = ''
        self.error = ''

    # ---- start / stop
    def start(self):
        self.mixer = Mixer({})
        self.mixer.volume = self.volume
        self.game = self.game_cls(self.mixer, self.log, **self.game_args)
        try:
            self.mixer.clips = self.game.test_clips()
        except Exception as e:                 # (no test voices: the game's test speakers are silent)
            self.log('test voices: %s' % e)
        self._open_output()
        self.game.start()
        if not self.no_mic:
            self._make_listener()

    def _open_output(self):
        def callback(outdata, frames, t, st):
            outdata[:] = self.mixer.render(frames)
        try:
            self.stream = open_output(callback, self.out_device, self.log)
            self.stream.start()
        except Exception as e:
            self.stream = None
            self.error = 'no sound output: %s' % e
            self.log(self.error)

    def _make_listener(self):
        import asr
        g = self.game

        def on_live(utt, words, info=None):
            self.live_now = words
            g.send('l', utt, words, (info or {}).get('times'), (info or {}).get('t0'))

        def on_final(utt, text, info):
            self.live_now = ''
            sent = g.send('f', utt, text, info.get('times'), info.get('t0'))
            if text:
                self.last_said = text
                self.log('you said%s: %s' % ('' if sent else ' [no game to tell]', text))
        self.listener = asr.Listener(on_live, on_final, threads=self.threads, log=self.log,
                                     on_start=lambda utt: g.send('s', utt, ''))   # (talking: their head bobs at once)
        self.mic = self.mic_source(self.listener) if self.mic_source else asr.Microphone(self.listener, device=self.mic_device, log=self.log)

    def stop(self):
        if self.mic is not None:
            self.mic.close()
        if self.listener is not None and self.listener.thread is not None:
            self.listener.stop()
        if self.stream is not None:
            try:
                self.stream.stop()
                self.stream.close()
            except Exception:
                pass
        if self.game is not None:
            self.game.stop()

    # ---- settings while running
    def set_volume(self, v):
        self.volume = max(0.0, min(1.0, float(v)))
        if self.mixer:
            self.mixer.volume = self.volume

    def set_output(self, device):
        self.out_device = device
        if self.stream is not None:
            try:
                self.stream.stop()
                self.stream.close()
            except Exception:
                pass
        self._open_output()

    def set_mic(self, device):
        self.mic_device = device
        if self.listener is None or self.mic_source:
            return
        import asr
        was = self.mic.is_open()
        self.mic.close()
        self.mic = asr.Microphone(self.listener, device=device, log=self.log)
        if was:
            self.mic.open()

    # ---- a few times a second
    def language(self):
        return self.lang_override or self.game.language()

    def _warm(self, lang):
        try:
            self.listener.set_language(lang)
            self.listener.warm()
            self.ready = True
        except Exception as e:
            self.error = 'the speech models could not be loaded: %s' % e
            self.log(self.error)
            self.ready = None

    def tick(self):
        if self.mic is None:
            return
        want = self.game.wants_mic() and self.game.connected()
        lang = self.language()
        if want and self.ready is None:                     # (load the models first: downloaded the first time)
            self.ready, self.loaded_lang = False, lang
            self.log('loading the speech models for "%s" (the first time they are downloaded)...' % lang)
            threading.Thread(target=self._warm, args=(lang,), daemon=True).start()
        elif self.ready and lang != self.loaded_lang:       # (another language: its model in the background)
            self.loaded_lang = lang
            self.log('language: %s' % lang)
            threading.Thread(target=self._warm, args=(lang,), daemon=True).start()
        self.listener.live = self.game.live_words()
        self.listener.set_push_to_talk(self.game.push_to_talk())
        if want and self.ready and not self.mic.is_open():
            self.listener.set_language(lang)
            if self.listener.thread is None:
                self.listener.start()
            self.mic.open()
        elif not want and self.mic.is_open():
            self.mic.close()

    # ---- what is going on
    def status(self):
        """a dict for the window / the status line"""
        mixer = self.mixer
        with mixer.lock:
            feed, age = mixer.feed, mixer.clock() - mixer.feed_t
        from audio import STALE
        if feed is None:
            state = 'waiting'                                # (no feed yet: the game is not running the mod)
        elif age > STALE:
            state = 'paused'
        else:
            state = 'connected'
        mic = 'off'
        if self.mic is not None and self.mic.is_open():
            mic = 'talking' if self.listener.line is not None else 'listening'
        elif self.ready is False:
            mic = 'loading'
        elif self.game.wants_mic() and state == 'connected':
            mic = 'wanted'
        speakers = []
        if feed and state == 'connected':
            for sid, sp in sorted(feed['speakers'].items()):
                speakers.append(dict(name=self.game.speaker_name(sp['src']), talk=sp['talk'], gain=sp['gain'],
                                     az=sp['az'], muffle=sp['muffle']))
        down = getattr(getattr(self.listener, 'models', None), 'downloading', None)
        return dict(state=state, mic=mic, download=down, level=getattr(self.mic, 'level', -120.0) if self.mic else -120.0,
                    lang=self.language(), live=self.live_now, last=self.last_said, speakers=speakers, error=self.error,
                    updates=getattr(getattr(self.game, 'reader', None), 'updates', 0))
