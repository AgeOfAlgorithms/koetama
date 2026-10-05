# The game link (Teardown), version 4

How a game mod and the helper program talk. Both run on the same PC, one helper per player. A Teardown
mod's Lua cannot open sockets, write files or reach the network. It *can* write registry keys under
`savegame.mod.*`, which the game saves to `savegame.xml` within about a frame. It can also ask whether a
file exists (`HasFile`) and load a prefab file (`Spawn`). The link is built from those three things.

The first mod using it is Proximity Babble Chat; its `voice.lua` is the reference game side. Any mod that
writes the feed below is found the same way.

Measured in-game (`probes/`, results in PROJECT.md): a registry write reaches `savegame.xml` in about one
frame (~17 ms), and a file the helper writes is seen by `HasFile` within ~17 ms. Spawning a prefab and
reading its tags back takes about 28 ms (median).

## Game -> helper: the feed

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
| mic | 1: the player wants what they say transcribed (the helper opens the microphone only then) |
| lang | the language the player speaks (`en`, `ru`, `zh`, `yue`, `ja`, `ko`, `es`, ... or `auto`) |
| live | 1: live words while the player talks; 0: only the finished line (less CPU) |
| speaker | `id,src,talk,gain,azimuth,elevation,muffle`: a voice to play (src: a test voice 1..3, a real player later 0; talk 1 while talking; gain 0..1; azimuth degrees from where the camera looks, 0 ahead, 90 right; elevation degrees up; muffle 0..1 behind walls and in the buffer range) |

Versions 2 (no lang, no live) and 3 (no live) are still read.

## Helper -> game: files next to the mod's folder

The mod reads them through `MOD/../pcvx_`. The helper writes them in `Documents/Teardown/mods/`, and in the
Workshop content folder when there is one.

| file | meaning |
|---|---|
| `pcvx_on` | the helper is running (removed when it stops). The mod looks for it once a second. |
| `pcvx_p<n % 1000>` | the answer to ping n. If no answer comes for ~5 s, the helper counts as gone (a crashed helper leaves `pcvx_on` behind). |
| `pcvx_t<n>.xml` | message n (1, 2, ... per session): a prefab `<body tags="pcvx k=<kind> u=<utterance> t=<hex of the UTF-8 text>"/>`. The mod `Spawn`s it, reads the tags, `Delete`s what it made, and acks n in the feed. |

Message kinds: `l` = the words so far while the player still talks (live), `f` = the finished line (it may be
empty: nothing made out; the live words go).

Planned (in progress): each message also carries the start time of every word and how long ago the speech
began, so a listener who arrives (or leaves) mid-sentence gets only the words said while they were in range.

## Another game

The engine (`engine/asr.py`: speech detection, speech to text, language detection) knows nothing about the
game. Everything game-specific is in `engine/teardown_helper.py`: the feed reader, the `Link` class that writes the
files, and the mixer. Another game would need its own link (a localhost socket, a named pipe, shared
memory, ...) speaking the same messages.
