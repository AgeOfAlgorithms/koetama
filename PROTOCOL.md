# The game link (Teardown), version 4

(Kotodama's link with Teardown, and how to add another game: see "Adding a game" at the end.)

How a game mod and the helper program talk. Both run on the same PC, one helper per player. A Teardown
mod's Lua cannot open sockets, write files or reach the network. It *can* write registry keys under
`savegame.mod.*`, which the game saves to `savegame.xml` within about a frame. It can also ask whether a
file exists (`HasFile`) and load a prefab file (`Spawn`). The link is built from those three things.

The first mod using it is Proximity Babble Chat; its `voice.lua` is the reference game side. Any mod that
writes the feed below is found the same way.

Measured in-game (`probes/`, results in PROJECT.md): a registry write reaches `savegame.xml` in about one
frame (~17 ms), and a file the helper writes is seen by `HasFile` within ~17 ms. Spawning a prefab and
reading its tags back takes about 28 ms (median).

## Game -> Kotodama: the feed

The mod writes one string, `savegame.mod.pcvx.f`. That's 20 times a second while there are voices to
hear, 5 times while only the helper is there, and once more (mic 0, no speakers) when there's nothing left.
The helper finds every `<pcvx><f value="..."/></pcvx>` in `Documents/Teardown/savegame.xml`, under the
mod's own tag: `local-<folder>` for a local mod, `steam-<id>` for a Workshop one.

    4|<seq>|<volume>|<session>|<ack>|<ping>|<mic>|<lang>|<live>|<speaker>;<speaker>;...

| field | meaning |
|---|---|
| seq | counts up with every write |
| volume | 0..1, the player's voice volume |
| session | new on every level start; the helper starts its message numbers over |
| ack | the number of the last message the mod has read; the helper deletes that file |
| ping | counts up every 2 s; the helper answers it (below) |
| mic | 1: the helper should listen and transcribe (the microphone is open only then). Proximity Babble Chat sends 1 whenever the helper is connected: what a player says is always written, so everyone gets the same (no opting out while your voice is heard) |
| lang | the language the player speaks (`en`, `ru`, `zh`, `yue`, `ja`, `ko`, `es`, ... or `auto`) |
| live | 1: live words while the player talks; 0: only the finished line (less CPU) |
| speaker | `id,src,talk,gain,azimuth,elevation,muffle`: a voice to play (src: a test voice 1..3, a real player later 0; talk 1 while talking; gain 0..1; azimuth degrees from where the camera looks, 0 ahead, 90 right; elevation degrees up; muffle 0..1 behind walls and in the buffer range) |

Versions 2 (no lang, no live) and 3 (no live) are still read.

## Kotodama -> game: files next to the mod's folder

The mod reads them through `MOD/../pcvx_`. The helper writes them in `Documents/Teardown/mods/`, and in the
Workshop content folder when there is one.

| file | meaning |
|---|---|
| `pcvx_on` | the helper is running (removed when it stops). The mod looks for it once a second. |
| `pcvx_p<n % 1000>` | the answer to ping n. If no answer comes for ~5 s, the helper counts as gone (a crashed helper leaves `pcvx_on` behind). |
| `pcvx_t<n>.xml` | message n (1, 2, ... per session): a prefab `<body tags="pcvx k=<kind> u=<utterance> t=<hex of the UTF-8 text> [w=<times> a=<ago>]"/>`. The mod `Spawn`s it, reads the tags, `Delete`s what it made, and acks n in the feed. |

Message kinds: `s` = the player started talking (no text yet: sent the moment the speech detector hears a line
begin, so the game can show them talking - their head bobs - before any words), `l` = the words so far while the
player still talks (live), `f` = the finished line (it may be empty: nothing made out; the live words go). A game
that does not know `s` reads it as an empty finished line, which does nothing.

**Live words only grow.** About once a second the helper reads the line so far again. It sends only the words
two reads in a row agree on, never the newest one, and never takes a shown word back. The game's bubble
therefore fills chunk by chunk; the finished line replaces it.

**Word times** (`w`, `a`; both or neither):
- **Units:** a line is split into units: runs of letters between spaces, and each CJK, kana or Hangul character
  on its own (`engine/asr.py` `units()`; the mod's `PC.voiceUnits` splits the same way, and both test suites
  share the same cases).
- **`w`:** each unit's start, as 4 hex digits in 1/100 s after the line's audio began.
- **`a`:** how long ago that was, in 1/100 s, at the moment the file was written. The mod turns it into a
  moment on its own clock; the server stamps it on the server's clock.
- **What a listener gets:** only the units said while they were in reach of the speaker, with "..." for each
  stretch missed: arriving mid-sentence "... the rest", walking away "the start ...". In the buffer zone the
  words are garbled like typed text, and walking closer reveals letters.
- **Without the tags:** the whole text, as before.

## Adding a game

Kotodama's engine knows no game. Speech detection, speech to text and the language detector are in `engine/asr.py`,
and the voice mixer is in `engine/audio.py`. Each game is a module in `engine/games/`: a class derived from
`games.base.Game`, listed in `games/__init__.py` `GAMES`, which puts it in the window's game picker. A module is only
the game's link:

| Part | What a game module does |
|---|---|
| `id`, `name`, `needs` | Its settings key, the name in the picker, and what players need in the game ("the X mod"). |
| `locate()` | Is the game installed here, and where (`engine/steam.py` finds Steam games, their Workshop folders and, on Linux, their Proton prefix). |
| `start()` / `stop()` | Begin and end the link: tell the game Kotodama runs, read its state in a thread of its own. |
| `on_feed(feed)` | Call it with each new state from the game: `vol`, `speakers` ({id: src, talk, gain, az, el, muffle}), `mic`, `lang`, `live`, as in Teardown's feed above. The mixer plays the speakers from it. |
| `send(kind, utt, text, times, t0)` | Hand the game what the player said: kinds `s` (started talking), `l` (live words, only growing), `f` (the finished line), with each unit's start time. |
| `test_clips()`, `speaker_name()` | Recorded voices for the game's test speakers (optional). |

How the game and Kotodama talk is up to the game: Teardown's mods can only use the save file and files next to
the mod, but a game that can open a localhost socket, a named pipe or shared memory can use that. The messages stay
the same. `engine/runtime.py` drives any module: the mixer, the output, the microphone (open only while the
module's feed asks for it) and the speech to text.
