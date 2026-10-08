"""What every game module gives the app (runtime.Runtime). The engine (speech to text, the voice mixer) knows no game;
a game module is only its LINK: how the game says whom the player hears and what it wants, and how Kotodama hands the
game what the player said.

The game's state, as the module reads it, is a FEED (a dict) given to the mixer and kept here:
    vol       0..1 the player's voice volume in the game
    speakers  {id: {src, talk, gain, az, el, muffle}}: the voices to play (src: a test voice for now; gain 0..1;
              az degrees from where the camera looks, 0 ahead, 90 right; el degrees up; muffle 0..1)
    mic       True: the player's speech should be heard and written
    lang      the language the player speaks ("en", "ru", ... or "auto")
    live      True: live words while they talk; False: only the finished line (less CPU)
"""


class Game:
    id = ''                 # short, for settings: "teardown"
    name = ''               # shown in the game picker: "Teardown"
    needs = ''              # what the player needs in the game, shown while waiting: "the Proximity Babble Chat mod"

    def __init__(self, mixer, log=print):
        self.mixer, self.log = mixer, log
        self.feed = None

    # ---- the app calls these
    def locate(self):
        """(found, where): is the game installed here, and where its files are (shown in the app)"""
        return False, ''

    def start(self):
        """start listening to the game (a thread of its own); tell it Kotodama runs"""

    def stop(self):
        """stop; tell the game Kotodama is gone"""

    def send(self, kind, utt, text, times=None, t0=None):
        """hand the game what the player said: kind "s" (they started talking), "l" (the words so far, only ever
        growing), "f" (the finished line; "" = nothing made out). times: each unit's start in s after t0 (when the
        line's audio began, time.perf_counter()). False if no game is listening"""
        return False

    def test_clips(self):
        """{src: mono float32 clip at audio.RATE}: the recorded voices the game's test speakers play"""
        return {}

    def speaker_name(self, src):
        return str(src)

    # ---- what the game wants (from its feed)
    def connected(self):
        return self.mixer.fresh()

    def wants_mic(self):
        return bool(self.feed and self.feed.get('mic'))

    def push_to_talk(self):
        """push to talk: True / False (the key is held or not); None - always on (the speech detector decides)"""
        return self.feed.get('ptt') if self.wants_mic() else None

    def language(self):
        return (self.feed or {}).get('lang', 'en') or 'en'

    def live_words(self):
        return (self.feed or {}).get('live', True)

    def on_feed(self, feed):
        """a module calls this with each new feed"""
        self.feed = feed
        self.mixer.set_feed(feed)
