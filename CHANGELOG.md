# What changed in each version

The three newest versions are in [README.md](README.md#whats-new); each release moves the oldest of them here.

### 0.5.5: safer voice chat

- The game list shows who made each game mod.
- Safer voice chat: a changed Koetama can no longer make its voice heard across the whole map (a range beyond the
  game's farthest is ignored), and a web page can no longer talk to a game's connection on this PC.
- Games that send big requests over HTTP no longer get their data a second late.
- **For mod makers** (see [PROTOCOL.md](PROTOCOL.md)):
  - The feed's new **`max_range`**: the farthest any player's voice can reach in your game (your loudest mode's
    `far`). A range another player's Koetama announces beyond it is ignored. Without it the limit is the farthest
    `range` your feed gives, else 30, so a game whose players whisper and shout should send it. A hub passes the
    host's on to its players.
  - The **HTTP connector** answers `Expect: 100-continue` with `100 Continue` (libcurl sends it with any body over
    1 KB). A client that sends that header must accept the interim reply; nothing changes for one that doesn't.
  - The **socket connector** closes a connection that speaks HTTP (a request line or a `Host:` header): any web page
    can make the browser connect to 127.0.0.1. A mod sends JSON lines, so it is not affected.
  - A profile's **`author`** now shows in Koetama's window.

### 0.5.4: Proximity Comms

- Teardown's **Proximity Babble Chat** mod is now **Proximity Comms (Text and Voice Chat)**: the game list shows the
  new name (your choice of game is kept).
- The game list: a game mod's page is a link symbol right of its row.
- The update button no longer turns a see-through colour while the update downloads.

### 0.5.3: test your microphone

- **Test microphone**, under Microphone in the window: the microphone opens for 30 s with no game needed - watch the
  level bar and see what the speech to text makes of you. **Hear yourself** plays your voice back as the other players
  will hear it (with the automatic gain and Mic boost; use headphones). Nothing is sent anywhere.
- **Per-player volume** for games: a speaker's `volume` (0..2) in the feed scales that player's voice and their
  devices ([PROTOCOL.md](PROTOCOL.md)); Proximity Babble Chat uses it for its new per-player sliders.

### 0.5.2: live words keep up

- **Live words while you talk again.** After one slow moment (a game loading, say) Koetama read the line less and
  less often, up to every 4 s, for the rest of the session: words then showed up only after you finished. It now
  follows the average, speeds back up once the PC has room, and never waits more than 2 s.

### 0.5.1: louder quiet microphones

- **Quiet microphones fixed.** Many headset and USB mics are offered to Windows as two channels with the voice in only
  one; Koetama averaged them, which halved those voices. It now uses the channel that carries the voice.
- **Automatic gain** on the voice you send, as other games' voice chat has: quiet voices are raised toward a normal
  level, shouts held back, and a soft limiter keeps peaks clean. The new **Mic boost** slider (under Microphone) adds
  up to +20 dB on top.
- **Translate your own languages too:** the Translation card's new "Don't translate" row lists the languages you speak;
  untick one to have it translated like any other.
- **Korean, Japanese or Chinese speakers** who speak only one of them: SenseVoice is told that language instead of
  guessing, so lines no longer drift into another one.
- "Join a hosted game" only shows for games whose mod needs it.

### 0.5.0: translation, set once in Koetama

- **Translate chat into: one setting in Koetama's window**, the same in every game. Lines in the languages you
  speak are left alone; anything else is translated into the language you picked. Off until you choose one.
- **No pairs to set up.** The model for a language downloads the first time someone writes in it (or turn
  downloads off and use only what is on your PC). Up to four languages stay loaded at once.
- **Better on short lines.** Accented letters and common little words now decide what language a short line is in,
  so "¿Dónde estás?" is no longer taken for English, while "ok" and "gg" stay yours.
- **For mod makers:** games no longer send translation settings. They send the chat lines they show (`to_translate`)
  and may pause translation with `"translate": false`; `translations_status` says which language the player reads
  (`into`). See [PROTOCOL.md](PROTOCOL.md). Joined players in hosted games use their own setting.

### 0.4.0: translation, more games, walkie-talkies

- **Chat translation.** Lines in another language are translated on your own PC (Mozilla's offline models,
  downloaded the first time a language needs them), so you can read players you share no language with.
- **Built for more games.** One simple JSON API for every game, over a local socket, HTTP (for games whose mods can
  only make web requests) or files. Example mods for Valheim, Godot and Tabletop Simulator show how
  ([PROTOCOL.md](PROTOCOL.md)).
- **Positions instead of maths.** A game can give player positions and voice ranges; Koetama works out direction,
  loudness and who hears whom. Player ids can be Steam ids or names, and a shared `room_seed` makes the voice room.
- **Hosted games.** For games whose mods run only on the host, each player joins with a short code the game shows
  them, and their Koetama works through the host's.
- **Walkie-talkies, loudspeakers and PA systems.** A voice can also come out of devices, each with a realistic sound
  (static and squelch, a horn's ring, a hall's echo), and games choose their own effects: pitch, wobble, robot,
  echo, reverb and more.
- **For game screens.** Games are told who is talking, who is in the voice chat, and whether speech is ready.
- **Safer.** Updates must carry a signature from the release key; nobody can push a player out of a voice room by
  taking their number; join codes work once; plus the fixes from a full security audit.
- **Fixed:** on Windows, a voice connection could drop for a second at random.

### 0.3.1: Koetama

The app is now called **Koetama** (声魂, "voice spirit"). It was Kotodama ("word spirit"), a name other voice apps
already use.

- **Smoother voices.** Each voice starts playing with a small cushion, so a packet that arrives a little late no
  longer leaves a gap, and your voice is sent in smaller steps (20 ms instead of 50), so it leaves sooner.
- **Check for updates** says when it finds no releases at all, instead of "you have the latest version".

### 0.3.0: real voice chat

You can now hear the other players, not just their words. Your voice goes to the players close enough to hear you,
through a small relay server, encrypted end to end.

- **Real voices.** Koetama sends your voice (Opus, about 3 KB/s while you talk) to the players in range of your
  speaking mode, and plays theirs where they stand. Whispers reach only the players near you.
- **Push to talk.** Hold the game's talk key to talk (the Teardown mod: B, rebindable), or choose always on. The
  first syllable is not cut off, and a line ends a quarter second after you let go.
- **Voice rooms.** Each game session gets its own room and key, made by Koetama from your PC's secure random
  numbers. The host can choose the region the room lives in (Auto: near the first player to join).
- **Status for the game.** Koetama tells the game which feed versions it reads, so a mod can ask an outdated
  Koetama to update, and whether the voice server can be reached.
- **Uninstalling** asks whether to remove the downloaded speech models and your settings too (yes by default).
- **Self-test** now also sends a packet through the voice relay.

### 0.2.0: the Rust app

A rewrite of the whole app in Rust: a smaller download, no Python, and a window that works on any PC.

- **Languages I speak.** Tick the languages you speak; only their speech models load (about 0.9 GB of memory for
  one language). The list shows which languages are fully supported, in beta or experimental.
- **Game mod profiles.** Games connect through profile files anyone can write (files or a local socket), added from
  the window after a safety preview. Mods can use voices, speech to text, or both.
- **A smaller language detector:** 43 MB instead of 86, with the same results.
- **The Ember look:** a dark theme, the voice-wave icon, and an installer to match.

### 0.1.0: the first version

The first app, in Python, made for Teardown's Proximity Babble Chat mod: a window with a game picker, live words
while you talk and the finished line when you stop, word times so a player who walks up mid-sentence sees only what
they heard, test voices placed around you in the game, and a Windows installer with an updater.
