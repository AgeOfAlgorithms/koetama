# The game link (Teardown), version 4

(Koetama's link with Teardown, and how to add another game's mod: see "Adding a game mod: profiles" at the end.)

How a game mod and the helper program talk. Both run on the same PC, one helper per player. A Teardown
mod's Lua cannot open sockets, write files or reach the network. It *can* write registry keys under
`savegame.mod.*`, which the game saves to `savegame.xml` within about a frame. It can also ask whether a
file exists (`HasFile`) and load a prefab file (`Spawn`). The link is built from those three things.

The first mod using it is Proximity Babble Chat; its `voice.lua` is the reference game side. Any mod that
writes the feed below is found the same way.

Measured in-game (`probes/`, results in PROJECT.md): a registry write reaches `savegame.xml` in about one
frame (~17 ms), and a file the helper writes is seen by `HasFile` within ~17 ms. Spawning a prefab and
reading its tags back takes about 28 ms (median).

## Game -> Koetama: the feed

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
| mic | 0: don't listen (the microphone is closed). 1: always on: listen and transcribe, the speech detector finds each line. 2 / 3: push to talk, the key up / held: the microphone stays open, but a line starts only while the key is held (the speech detector still finds where it begins, from the last second of audio, so the first word isn't lost to the feed's delay) and ends 0.25 s after the key is let go. Proximity Babble Chat sends 2 / 3 by default (its talk key, B) and 1 with "Always on" |
| lang | the language the player speaks (`en`, `ru`, `zh`, `yue`, `ja`, `ko`, `es`, ... or `auto`) |
| live | 1: live words while the player talks; 0: only the finished line (less CPU) |
| speaker | `id,src,talk,gain,azimuth,elevation,muffle`: a voice to play (src: a test voice 1..3, a real player later 0; talk 1 while talking; gain 0..1; azimuth degrees from where the camera looks, 0 ahead, 90 right; elevation degrees up; muffle 0..1 behind walls and in the buffer range) |

Versions 2 (no lang, no live) and 3 (no live) are still read.

## Koetama -> game: files next to the mod's folder

The mod reads them through `MOD/../pcvx_`. The helper writes them in `Documents/Teardown/mods/`, and in the
Workshop content folder when there is one.

| file | meaning |
|---|---|
| `pcvx_on` | the helper is running (removed when it stops). The mod looks for it once a second. |
| `pcvx_v<n>` | one per feed version this Koetama reads (now `pcvx_v5`), written with `pcvx_on`. A mod whose version is missing knows this Koetama is too old for its feed (it would never answer): the Teardown mod says "Your Koetama is too outdated. Please update it from the app." |
| `pcvx_vc` / `pcvx_vx` | the voice chat is in its room (`vc`) / can't reach the relay: the last tries failed, it keeps trying (`vx`); neither: no room, or connecting. The mod shows it and tells its player when voices can't arrive. |
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

## Real voices: rooms and the relay (version 5)

Players' voices travel between their Koetamas through **the relay**, a Cloudflare Worker
(`relay/` in this repo; `wss://koetama-relay.ageofalgorithms.workers.dev`, or `KOETAMA_RELAY`). The game never
carries audio. It tells its player's Koetama three things through the feed:

- **the room:** the game session's voice room (a name and a key every player in the session gets);
- **who should get my voice now:** the players within range of the speaker's current mode;
- **how loud each other player is to me:** the speakers list, as for the test voices (`src` 0, `id` = their player id).

### The feed, version 5

    5|<seq>|<vol>|<session>|<ack>|<ping>|<mic>|<lang>|<live>|<room>|<key>|<me>|<to>|<region>|<speaker>;<speaker>;...

The first nine fields are version 4's. New:

| field | meaning |
|---|---|
| room | the voice room: 32 lower-case hex digits; empty = no room (no voice sent or received) |
| key | the room's key: 64 lower-case hex digits (32 bytes); empty when room is |
| me | this player's id in the game session, 1..65535 (Teardown: the player id) |
| to | the players who should get this player's voice right now, comma-separated ids; empty = nobody. The SPEAKER's game decides: in Teardown, the players within the voice mode's whole range (its buffer too) plus 5 m; every dead player too (the dead hear everyone; their own gain decides how loud); everyone in a lobby; nobody in Global; a dead speaker: the dead of their channel at any distance |
| region | where the room should live: `wnam`, `enam`, `sam`, `weur`, `eeur`, `apac`, `apac-ne`, `apac-se`, `oc`, `afr`, `me`; empty (or anything else) = Auto: wherever the relay creates it, near the first player to join. In Teardown the host picks it (Settings, "Voice server"); every player's feed carries the same one. Another region is another room on the relay, so a change moves everyone to a new room there |
| speaker | as version 4. A real player: `src` 0 and `id` = their player id; `talk` is ignored for them (Koetama plays what arrives). A player who is not in the list (or has gain 0) is not played even if their voice arrives |

Version 4 is still read (no room: no real voices).

The socket connector's feed object takes the same as `"room"`, `"key"`, `"me"` (numbers / strings as above),
`"to": [ids]` and `"region"`. It is told the voice chat's state with a line
`{"type":"voice","state":"off"|"connecting"|"connected"|"unreachable"}` at each change.

### Who makes the room: Koetama

A game script has no good random numbers, so Koetama makes the room. Once per game session (a new `session`
in the feed) it sends the game a message of kind **`r`** whose text is `<room>:<key>` (32 + 64 hex digits), freshly
random (OS randomness). The Teardown mod forwards it to the host (`server.pc_room`); the host keeps the FIRST one it
gets for the session and publishes it to every player (`shared.pcVoiceRoom = {id, key}`); every player's game puts
that one into its feed. So any player with Koetama can make the session's room, the host doesn't need Koetama,
and the key reaches only the players in the session. A game that does not know `r` reads it as an empty finished
line, which does nothing. (The socket connector: `{"type":"msg","kind":"r","text":"<room>:<key>"}`.)

### Sending and receiving (Koetama)

- Koetama connects to `<relay>/v1/room/<room>?me=<me>[&region=<region>]` (a WebSocket) while the feed names a room and the game is
  connected, reconnecting after a drop (1, 2, 4 ... 30 s). It sends the text `ping` every 20 s (answered `pong`).
- **When it sends:** while the microphone is open (mic 1, 2, 3) and the player talks: push to talk, while the key
  is held (from 0.15 s before the press arrived: the feed's delay) and PTT_TAIL (0.25 s) after; always on, while the speech detector hears speech (from 0.3 s before it
  noticed). Only to the feed's `to`; nothing when `to` is empty. Never in Global.
- **Audio:** Opus, 48 kHz mono, 20 ms frames, 24 kbit/s (VOIP), 3 frames (60 ms) per packet.
- **Frames to the relay** (binary): `[1][n][to_1 .. to_n as u16 big-endian][payload]`, n <= 64.
  **From the relay:** `[1][from as u16 big-endian][payload]`. The relay forwards each packet to the named players
  only, never back to the sender; it never looks inside the payload.
- **Payload** = `nonce (12 random bytes) | ChaCha20-Poly1305(key, nonce, plaintext, aad = from as u16 big-endian)`
  (the 16-byte tag at the end). A packet that does not decrypt is dropped. Plaintext:
  `[1][seq: u32 big-endian][flags: u8, 1 = the last packet of a stretch of talking][k][k x (len: u16 big-endian, Opus bytes)]`.
- **Playing:** per sender a jitter buffer (start at 60 ms buffered and 40 ms after the first packet arrived - a cushion for a packet that comes a little late -, at most 300 ms: older audio is dropped),
  Opus loss concealment for a missing packet, ended after 0.5 s without packets or after the last packet. Mixed
  like the test voices: the feed's gain, direction and muffle for that player id.

### The relay (`relay/`)

One Durable Object per room (by name), the WebSocket Hibernation API. Limits: 64 players in a room, 64
recipients and 4000 bytes of payload in a packet, 60 packets a second from one connection (more are dropped); a
second connection with the same `me` replaces the first (close code 4000). Paths: `/` and `/v1` say what it is;
`/v1/room/<32 hex>?me=<1..65535>[&region=<region>]` is the room: with a region it is named `<room>@<region>` and
created with that Durable Object location hint (best effort; only the first connection of a room places it). `npm test` (the frames), `node test/smoke.mjs <url>` (a live
room: delivery, nobody out of range, the keep-alive, the time of one hop), `npm run deploy` (from `relay/`, the
`relay` conda env: Node.js 22, Wrangler 4).

## Adding a game mod: profiles

Koetama's engine knows no game. Speech detection, speech to text and the language detector are the crate
`kd-speech`, and the voice mixer is `kd-audio` (in `app/crates/`). A game mod is linked to Koetama by a **profile**:
a small JSON file that names one of Koetama's built-in **connectors** and gives its settings. A profile is never
code, so anyone can write one for their game's mod without changing Koetama. A connector only does what this
section describes. Teardown's link is a profile too, built into Koetama (`app/crates/kd-games/src/profiles/teardown.json`).

Two connectors:
- **files**: the link above, for a game whose mod can only write data the game saves to a file and read files next to
  itself (Teardown). Koetama polls the file for the mod's feed string and writes small message files for the mod.
- **socket**: for a game whose mod can open a TCP connection. Koetama listens on `127.0.0.1:<port>` (this computer
  only), and both sides send one JSON object per line.

### Installing a profile (players)

- In Koetama's window: **Add game mod...**, then pick the `.json` file. Koetama shows what the profile reads,
  writes and listens on, with this PC's real folders. If you accept, it copies the file into its profiles folder as
  `<id>.json`, replacing an older profile with the same id.
- By hand: put the file into `%LOCALAPPDATA%\Koetama\games` (Linux: `~/.local/share/koetama/games`; macOS:
  `~/Library/Application Support/Koetama/games`). The window lists any file there that does not load, with the
  reason. A profile whose id is already taken (by a built-in or a file earlier in name order) is skipped.

The game then shows up in the window's game picker.

### The profile format (format 1)

A full example, using the files connector:

```json
{
  "format": 1,
  "id": "my-game-talky",
  "game": "My Game",
  "mod": "Talky",
  "url": "https://example.com/talky",
  "author": "Someone",
  "locate": {"steam_app": 4242},
  "uses": ["voices", "speech"],
  "test_voices": [{"src": 1, "voice": "Microsoft Zira Desktop", "rate": 0, "text": "I am the test speaker."}],
  "speaker_names": {"1": "tester"},
  "connector": {
    "type": "files",
    "feed": {
      "file": ["{localappdata}/My Game/save.xml", "{proton_user:4242}/AppData/Local/My Game/save.xml"],
      "pattern": "<talky>\\s*<f\\s+value=\"([^\"]*)\"\\s*/>",
      "tag_pattern": "<((?:local|steam)-[a-z0-9-]+)>",
      "complete": "</save>"
    },
    "out": {
      "dirs": [["{documents}/My Game/mods", "{proton_user:4242}/Documents/My Game/mods"], "{steam_workshop:4242}"],
      "tag_dirs": [{"tag_prefix": "steam-", "dir": 1}],
      "prefix": "talky_",
      "message": "json"
    }
  }
}
```

A profile using the socket connector only changes `connector`: `{"type": "socket", "port": 47120}`. See
`examples/profiles/example-socket.json`.

| field | | meaning |
|---|---|---|
| `format` | required | `1`. Koetama refuses a newer format and asks the player to update. |
| `id` | required | 3 to 40 of `a-z`, `0-9`, `-`. The settings key; unique. Name the game AND the mod (`<game>-<mod>`, like the built-in `teardown-proximity-babble-chat`): one game can have several mods made for Koetama. |
| `game` | required | The game's name, shown in the picker (1 to 80 characters). |
| `mod` | required | The mod's name (1 to 80 characters). |
| `url` | required | The mod's page, `https://...` or `http://...` (the window opens it in the browser). |
| `author` | required | Who made the mod and the profile. |
| `locate` | optional | `{"steam_app": N}`: the game's Steam app id. Koetama shows where it's installed, or that it's missing. |
| `uses` | optional | `["voices", "speech"]` (the default), or just one of them. `voices`: Koetama plays the speakers in the feed (other players' voices). `speech`: Koetama listens to the microphone and sends what the player said (speech to text). A speech-only mod doesn't need to send speakers (Koetama drops them). A voices-only mod's `mic` is ignored, so the microphone never opens. |
| `test_voices` | optional | Up to 16 recorded voices for the mod's test speakers: `{"src": 1..999, "voice": "<Windows voice>", "rate": -10..10, "text": "..."}`. They're made once with the Windows speech voices (none on other systems), and a speaker with that `src` plays them. |
| `speaker_names` | optional | Names for the mod's TEST speakers (the sample voices of `test_voices`, by `src`) in Koetama's window, e.g. `{"1": "the whisperer"}`. Real players need nothing: they show as "player <id>". Only useful with `test_voices`. |
| `connector` | required | `{"type": "files", ...}` or `{"type": "socket", ...}`, described below. |

Unknown fields are errors, so a typo doesn't get silently ignored. Text may not contain control characters. A profile
file can be at most 64 KB.

### Paths and placeholders

The files connector's paths use `/` (or `\`) between folder names. A path starts with a placeholder or is a full path
(`C:/...`, `/...`). It may not contain `.` or `..`, and a placeholder can only be at the start:

| placeholder | is |
|---|---|
| `{documents}` | the user's Documents folder (on Windows, wherever it really is: OneDrive moves it) |
| `{localappdata}` | `%LOCALAPPDATA%` (Windows) |
| `{home}` | the user's home folder |
| `{steam_app:ID}` | the Steam app's install folder (`steamapps/common/<game>`, in any Steam library) |
| `{steam_workshop:ID}` | the app's Workshop content folder (`steamapps/workshop/content/<ID>`) |
| `{proton_user:ID}` | Linux: the Windows user folder in the app's Proton prefix (`.../pfx/drive_c/users/steamuser`) |
| `{env:NAME}` | an environment variable (set and not empty) |

A path entry can also be a list of candidates. Koetama uses the first one whose placeholder resolves on this PC, and
for a folder, the first one that also exists. One profile can therefore cover Windows and Linux (Proton), as
Teardown's does.

### The files connector

| field | | meaning |
|---|---|---|
| `feed.file` | required | The file the game writes and Koetama reads (a path or candidates). It must end in a file name. |
| `feed.pattern` | default: Teardown's | A regex with exactly one group, which captures the feed string. Each match is one copy of the mod. It runs over the file's bytes, and `(?-u)` lets it match any bytes. |
| `feed.tag_pattern` | default: Teardown's | A regex with one group: the tag of the mod copy that wrote a feed (the last match before it). `""` means no tags. |
| `feed.complete` | default `</registry>` | The file is read only when it ends with this text, ignoring trailing white space, so a half-written file is skipped. `""` reads it every time. |
| `out.dirs` | required | 1 to 8 folders the mod looks in (each a path or candidates). Folders not on this PC are skipped. |
| `out.tag_dirs` | optional | `[{"tag_prefix": "steam-", "dir": 1}]`: a mod copy whose tag starts with the prefix gets the folder at that index of `out.dirs`. Any other copy gets folder 0. |
| `out.prefix` | default `pcvx_` | The start of every file name Koetama writes. Unique among game mods: a profile whose prefix another one uses is refused (the two would remove each other's files), so pick one from your mod's name (`pcvx_` is Proximity Babble Chat's). |
| `out.message` | default `teardown-prefab` | How a message file is written: `teardown-prefab` (`<prefix>t<n>.xml`, exactly the prefab described above) or `json` (`<prefix>t<n>.json`). |

The feed string is the one described above (`4|seq|volume|session|ack|ping|mic|lang|live|speakers`). Koetama reads the
file every 10 ms. A feed string that changes counts as live, but whatever was in the file when Koetama started does
not. The files Koetama writes and the way acks, pings and sessions work are also as described above, with the
profile's prefix. A `json` message is one line:

    {"k":"l","u":7,"t":"hello there","w":[0.1,0.55],"a":2.03}

`k` is the kind (`s`, `l`, `f`), `u` the utterance and `t` the plain UTF-8 text. `w` holds each unit's start in seconds
after the line's audio began, and `a` how many seconds ago that was when the file was written. `w` and `a` come
together or not at all.

### The socket connector

`{"type": "socket", "port": 47120}`: the port is 1024 to 65535. Koetama listens on `127.0.0.1` only, so nothing
from another computer can connect. It serves one client at a time, and a new connection replaces the old one. Both
sides send JSON objects, one per line (`\n` ends a line).

When a mod connects, Koetama sends:

    {"type":"hello","app":"Koetama","version":"0.3.1","protocol":1}

The mod sends, first (optional):

    {"type":"hello","protocol":1,"game":"Example Game","mod":"Example Mod"}

then its feed, whenever it changes and at least once a second:

    {"type":"feed","vol":1.0,"mic":true,"lang":"en","live":true,
     "speakers":[{"id":7,"src":1,"talk":true,"gain":0.8,"az":90,"el":0,"muffle":0.25}]}

(The example is split over two lines here, but it is sent as one.)

| feed field | default | meaning |
|---|---|---|
| `vol` | 1 | 0..1, the player's voice volume |
| `mic` | false | the player's speech should be heard and written (the microphone opens only then) |
| `ptt` | none | push to talk: `true` while the talk key is held, `false` while it is up (a line starts only while it is held and ends 0.25 s after it is let go); left out or `null`: always on (the speech detector decides). Send a feed as soon as the key changes |
| `lang` | `en` | the language the player speaks (`en`, `ru`, `zh`, ... or `auto`) |
| `live` | true | live words while the player talks; false: only the finished line |
| `speakers` | none | the voices to play: `id` (required, a whole number), `src` (0 = a real player, 1..: a test voice), `talk`, `gain` 0..1, `az` (degrees from where the camera looks, 90 = right), `el` (degrees up), `muffle` 0..1 |

The game counts as connected while feeds arrive. If none comes for 1.5 s, the game shows as paused and its voices
stop. There are no acks or pings, because the connection itself shows the game is there.

Koetama sends what the player said:

    {"type":"msg","kind":"s","utt":4,"text":""}
    {"type":"msg","kind":"l","utt":4,"text":"hello there","times":[0.1,0.55],"ago":1.02}
    {"type":"msg","kind":"f","utt":4,"text":"hello there everyone","times":[0.1,0.55,0.9],"ago":2.4}

The kinds are the same as for the files connector. `times` holds each unit's start, in seconds after the line's audio
began, and `ago` how long ago that was when the line was sent. They come together, and only when known.

Koetama skips a line that isn't JSON, or a feed it can't read, and logs the first one per connection. It ignores
messages with an unknown `type`, which leaves room for later versions. A line longer than 64 KB drops the connection.
If the port is busy, Koetama says so and tries again every 2 s.

`examples/socket_client.py` is a working client (Python 3, no packages) to start from. It also serves as a manual
test: install `examples/profiles/example-socket.json`, pick "Example Game" in Koetama, run the script and talk.

### What a profile can and cannot make Koetama do

- It never runs code. A test voice's text and voice name reach the Windows speech voice as data, never inside a
  command.
- The files connector writes only into folders that already exist (it never creates one). Every file it writes
  starts with the prefix. The prefix must be 3 or more letters, digits or `_`, ending in `_`. The connector deletes
  only files whose whole names are exactly `<prefix>on`, `<prefix>p<digits>`, `<prefix>t<digits>.<xml or json>` or
  `<prefix>w<digits>.tmp`, and nothing else, not even another file starting with the prefix.
- The socket connector listens only on 127.0.0.1, on the profile's port (1024 to 65535). It reads only the messages
  above and sends only what the player said.
- The window shows players all of this, with real paths, before a profile is installed.
