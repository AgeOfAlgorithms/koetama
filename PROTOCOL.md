# Koetama's game protocol (protocol 2)

How a game mod talks to Koetama, the companion app that runs on the same PC (one per player): the player's speech
as text, other players' voices, and chat translation.

**One API, three transports.** The game and Koetama exchange a small set of JSON objects. How they travel depends on
what the game's mod can do, and a **profile** (a JSON file per game mod, "Adding a game mod: profiles" below) says
which:

- **socket**: the mod opens a TCP connection to Koetama (127.0.0.1); one object per line, both ways.
- **http**: the mod can only make HTTP requests (Tabletop Simulator, Garry's Mod, a browser game): it POSTs its feed
  and the answer holds Koetama's objects.
- **files**: for a mod that can only write files (or data its game saves) and read files next to itself (Teardown,
  Lua sandboxes): its feed in a file, Koetama's objects in small numbered files.

The objects are the same either way. Proximity Babble Chat (a Teardown mod, `voice.lua`) is the reference game side.

## The objects

### Game -> Koetama: the feed

The game's whole state for Koetama, sent again whenever something in it changes and at least once a second (Koetama
counts a game as gone after 1.5 s without one). Every field is optional; a missing one has its default.

    {"type":"feed","volume":1,"listen":"push_to_talk","talk_key":false,"lang":"en","live":true,
     "speakers":[{"id":2,"gain":0.8,"azimuth":30,"elevation":0,"muffle":0.1}],
     "room":"<32 hex>","key":"<64 hex>","me":1,"to":[2],"region":"",
     "translations":[{"from":"ja","to":"en"}],"to_translate":[{"id":7,"text":"こんにちは"}]}

| field | default | meaning |
|---|---|---|
| `volume` | 1 | 0..1, how loud the other players' voices are |
| `listen` | `"off"` | the microphone: `"off"` (closed), `"always"` (the speech detector finds each line), `"push_to_talk"` (a line only while `talk_key` is held) |
| `talk_key` | false | push to talk: true while the talk key is held. Send a feed as soon as it changes. The microphone stays open, so a line starts from just before the key arrived (the feed's delay costs no first word) and ends 0.25 s after it is let go |
| `lang` | `"en"` | the language the player speaks (`en`, `ru`, `zh`, `yue`, `ja`, `ko`, `es`, ... or `auto`: Koetama finds it, even several in one line) |
| `live` | true | the words so far while the player talks (false: only the finished line; less CPU) |
| `speakers` | none | the other players this player hears now, and how: `id` (required: their player id), `gain` 0..1 (0 or not listed: not heard), `azimuth` (degrees from where the camera looks: 0 ahead, 90 right, ±180 behind), `elevation` (degrees up), `muffle` 0..1 (0 clear; behind walls, or where words are garbled). A **test voice** instead of a player: `"test_voice": n` (one of the profile's `test_voices`) and `"talking": true` while it should play |
| `room`, `key` | none | the session's voice room: 32 and 64 lower-case hex digits (Koetama makes them: "Real voices"); none: no voices sent or heard |
| `me` | none | this player's id in the game session, 1..65535 |
| `to` | none | the player ids who should get this player's voice right now (the game decides who is in range); empty: nobody |
| `region` | `""` | where the voice room should live: `wnam`, `enam`, `sam`, `weur`, `eeur`, `apac`, `apac-ne`, `apac-se`, `oc`, `afr`, `me`; `""` (or anything else): wherever the first player is. Every player of a session sends the same one |
| `translations` | none | up to two `{"from", "to"}` (language codes): "Translation" |
| `to_translate` | none | the chat lines to translate: `{"id", "text"}`, at most 16, each at most 400 bytes of UTF-8. Ids are the game's (1 to 15 digits, unique in the session). Keep a line in every feed until its `translation` arrives (drop it after ~10 s without one) |

Bad values are skipped or replaced by the default (a bad room, key or `me`: no room).

### Koetama -> game

Each object has a `"type"` first.

| object | when |
|---|---|
| `{"type":"hello","app":"Koetama","version":"0.4.0","protocol":2,"features":["speech","voices","rooms","translate"]}` | first, each session (socket: each connection). `features`: what Koetama does for this game - what its profile uses, and `rooms` (real voices) with `voices`. Ignore a feature you do not know |
| `{"type":"speech","kind":"start","utt":4}` | the player started talking (no words yet: the moment the speech detector hears a line begin, so the game can show them talking) |
| `{"type":"speech","kind":"live","utt":4,"text":"hello there","times":[0.1,0.55],"ago":1.02}` | the words so far, while they talk (only with `live`) |
| `{"type":"speech","kind":"final","utt":4,"text":"hello there everyone","times":[0.1,0.55,0.9],"ago":2.4}` | the finished line (`text` may be `""`: nothing made out; the live words go) |
| `{"type":"room","room":"<32 hex>","key":"<64 hex>"}` | a new voice room, once per session: "Real voices" |
| `{"type":"voice","state":"connected"}` | the voice chat's link changed: `off`, `connecting`, `connected` or `unreachable` (the last tries failed; it keeps trying) |
| `{"type":"translation","id":7,"text":"Hello"}` | the translation of line `id`, exactly one per id. `""`: nothing to show (nothing in a source language, the same as the line, or a bad line). A line that comes while a translation's models are downloading or loading waits for them (up to 2 minutes). At most 1000 characters |
| `{"type":"translations_status","translations":[{"from":"ja","to":"en","state":"downloading","progress":0.42}]}` | each translation's state, when one changes (at most every 0.5 s while downloading): `ready`, `downloading` (with `progress` 0..1), `loading`, `unavailable` (no model for it) or `error` (tried again after a minute) |

**Speech.** `utt` numbers a line (its `start`, `live` and `final` share it). **Live words only grow**: about once a
second Koetama reads the line so far again and sends only the words two reads agree on, never the newest one, and
never takes a shown word back; the `final` replaces them. **Word times**: a line is split into units (runs of letters
between spaces, and each CJK, kana or Hangul character on its own; `engine/asr.py` `units()`, the mod's
`PC.voiceUnits`); `times` holds each unit's start in seconds after the line's audio began, and `ago` how long ago that
was when Koetama sent it. Both or neither. With them a listener can show only what was said while they were in reach
of the speaker ("... the rest" arriving mid-sentence, "the start ..." walking away).

## Real voices

Players' voices travel between their Koetamas through **the relay**, a Cloudflare Worker (`relay/`;
`wss://koetama-relay.ageofalgorithms.workers.dev`, or `KOETAMA_RELAY`). The game never carries audio. It tells its
player's Koetama the room (`room`, `key`, `me`, `region`), who should get this player's voice now (`to`), and how loud
each other player is (`speakers`).

**Who makes the room.** A game script has no good random numbers, so Koetama makes the room: once per session it
sends a `room` object with a fresh random room and key. The game gives ONE room to every player of the session (in
Teardown: each player's game forwards its offer to the host, the host keeps the first and shares it), and every
player's feed names it. Any player with Koetama can make the room; the key reaches only the players in the session.

**Sending.** Koetama connects to `<relay>/v1/room/<room>?me=<me>[&region=<region>]` (a WebSocket) while the feed
names a room, reconnecting after a drop (1, 2, 4 ... 30 s), with a text `ping` every 20 s. It sends while the player
talks (push to talk: from 0.15 s before the press arrived until 0.25 s after the release; always: while the speech
detector hears speech, from 0.3 s before it noticed), only to `to`. Audio: Opus, 48 kHz mono, 20 ms frames, 24 kbit/s,
3 frames (60 ms) per packet.

- **Frames to the relay** (binary): `[1][n][to_1 .. to_n as u16 big-endian][payload]`, n <= 64. **From the relay:**
  `[1][from as u16 big-endian][payload]`. The relay forwards a packet to the named players only, never back to the
  sender, and never looks inside.
- **Payload** = `nonce (12 random bytes) | ChaCha20-Poly1305(key, nonce, plaintext, aad = from as u16 big-endian)`;
  a packet that does not decrypt is dropped. Plaintext: `[1][seq: u32 BE][flags: u8, 1 = the last packet of a
  stretch of talking][k][k x (len: u16 BE, Opus bytes)]`.
- **Playing:** per sender a jitter buffer (starts at 60 ms buffered and 40 ms after the first packet; at most
  300 ms), Opus loss concealment for a missing packet, ended 0.5 s after the last packet. Mixed with the feed's
  `gain`, `azimuth`, `elevation` and `muffle` for that player.

**The relay** (`relay/`): one Durable Object per room (by name; with a region: `<room>@<region>`, created with that
location hint), the WebSocket Hibernation API. Limits: 64 players in a room, 64 recipients and 4000 bytes of
payload in a packet, 60 packets a second from one connection; a second connection with the same `me` replaces the
first (close code 4000). `/` and `/v1` say what it is. `npm test`, `node test/smoke.mjs <url>` (a live room),
`npm run deploy` (from `relay/`, the `relay` conda env).

## Translation

A player has up to two **translations**, each "from language A into language B" (Japanese → English, Korean →
English). The game sends Koetama the full chat lines it shows (typed lines, and spoken lines once finished - never
live words) in `to_translate`; Koetama translates them on the player's PC with Mozilla's Firefox Translations models
(MPL-2.0, ~20-55 MB a direction, downloaded the first time a translation needs them; a pair without English goes
through English: two models) and answers each with a `translation`.

- **Mixed-language lines** are split into stretches by script and language; the stretches in a translation's source
  language are translated, the rest kept, the order kept. A line with nothing in a source language: `""`.
- A translation whose two languages are the same, or with a language Mozilla has no model for, is `unavailable`.
  Cantonese (`yue`) only as a source (through the Traditional Chinese model); Maltese only into English. Two with
  the same source language: the first is used.

## Transport: socket

`{"type": "socket", "port": 47120}` in the profile. Koetama listens on `127.0.0.1:<port>` only (nothing from another
computer can connect), one client at a time (a new connection replaces the old one), one JSON object per line (`\n`)
both ways. On connect Koetama sends its `hello`; the mod may send `{"type":"hello","protocol":2,"game":..,"mod":..}`,
then feeds. Each connection is a session (a new `room` for it). No acks or pings: the connection shows the game is
there. Koetama skips a line that is not JSON or a feed it cannot read (logging the first per connection), ignores an
unknown `type`, and drops a connection whose line is longer than 64 KB. A busy port is tried again every 2 s.
`examples/socket_client.py` is a working client (Python 3, no packages) and a manual test.

## Transport: HTTP

`{"type": "http", "port": 47140}` in the profile. Koetama answers HTTP/1.1 on `127.0.0.1:<port>` only. The mod POSTs
its feed object (`Content-Length`, no chunked bodies; at most 64 KB) to `/`, and the answer is what Koetama has for it:

    {"objects": [{"type":"hello",...}, {"type":"speech",...}], "first": 1, "last": 2}

- **Numbers.** Koetama numbers its objects per session (`session` in the feed; 1 is the `hello`); `first` and `last`
  are the numbers of the first and last ones in the answer (none: `first` is `last` + 1). The next feed's `ack` says
  the last one the mod has: Koetama keeps every object until it is acked, so an answer that gets lost loses nothing
  (the next answer repeats it). A mod with two requests out at once may get an object twice: skip numbers up to the
  last it has.
- **`wait`** (in the feed, seconds, at most 1): with nothing to send, the answer waits that long for an object. A mod
  that sends its next feed as soon as an answer arrives (one request at a time) then gets each object the moment it
  exists, without a busy loop; with `wait` 0 it polls (each poll is a feed). A newer feed ends a waiting answer at
  once, so a mod can send a change (the talk key) right away without waiting for its poll to come back. Either way, a feed at least once a second
  (Koetama counts a game as gone after 1.5 s without one).
- `session`, `ack` and `wait` are this transport's own fields; the rest of the feed is as above.
- `GET /` answers `{"app":"Koetama","version":..,"protocol":2,"game":"<profile id>"}`: is Koetama there?
- Errors: 400 (not JSON, or a feed it cannot read: `{"error": ".."}`), 403, 404, 405, 411, 413, 503 (16 requests
  open at once).
- **Browsers.** A request with an `Origin` header (a web page) is refused (403) unless the profile lists that origin:
  `"allow_origins": ["https://my-game.example"]`; for those, Koetama sends the CORS and Private Network Access headers
  (`Access-Control-Allow-Origin`, `Access-Control-Allow-Private-Network`) and answers preflights. A game's own HTTP
  client sends no `Origin` and needs nothing. So no web page the player happens to open can read what they say.

`examples/http_client.py` is a working client (Python 3, no packages).

## Transport: files

For a mod that can only write data its game saves to a file, and see files next to itself (Teardown: a mod's Lua
cannot open sockets, write files or reach the network; it can write `savegame.mod.*` registry keys, which the game
saves to `savegame.xml` within a frame, ask whether a file exists, `HasFile`, and load a prefab file, `Spawn`).

**Game -> Koetama.** The mod writes its feed as one string into a file: the JSON object, or its hex (lower or upper
case) when the file cannot hold quotes - Teardown's registry string is hex. Koetama reads the file every 10 ms (only
when it is complete: it ends with the profile's `complete` text), finds each copy of the mod's feed with the
profile's `pattern`, and reads the newest. A mod that writes its feed object as a file of its own (a Lua `json.dump`,
`io.open(...):write`) gives `"pattern": ""` (the whole file is the feed) and `"complete": ""` (a half-written file is
simply not JSON yet, and skipped). Four fields carry the link itself, only here:

| field | meaning |
|---|---|
| `seq` | counts up with every write (a changed string = the game is there; a paused game stops writing) |
| `session` | a new number for each game session (Teardown: each level); Koetama starts its message numbers over, with a `hello` |
| `ack` | the number of the last message the mod has read; Koetama deletes that file |
| `ping` | counts up every ~2 s; Koetama answers it |

**Koetama -> game: files in the profile's folders**, each name starting with the profile's prefix (`pcvx_` for
Proximity Babble Chat; Teardown: `Documents/Teardown/mods/` and the Workshop folder, read by the mod as
`MOD/../pcvx_...`):

| file | meaning |
|---|---|
| `<prefix>on` | Koetama is running (removed when it stops) |
| `<prefix>p<n % 1000>` | the answer to ping n. No answer for ~5 s: Koetama is gone (a crash leaves `on` behind) |
| `<prefix>t<n>.<ext>` | object n (1, 2, ... per session; 1 is the `hello`), written whole (through `<prefix>w<n>.tmp`). The mod reads it, acks n in the feed, and Koetama deletes it. The profile's `message` format: `json` (`.json`: the object, one line) or `teardown-prefab` (`.xml`: `<prefab version="1.5.2"><body tags="pcvx j=<hex of the object>"/></prefab>`; the mod `Spawn`s it, reads the `j` tag, `Delete`s what it made) |

A mod knows its Koetama is too old for this protocol when `<prefix>on` is there but no `hello` comes: a Koetama
before 0.4.0 also writes `<prefix>v5` / `<prefix>v6`, which the Teardown mod looks for ("Your Koetama is too
outdated. Please update it from the app."). Koetama 0.4.0 removes those old files.

Measured in Teardown (`probes/`, PROJECT.md): a registry write reaches `savegame.xml` in about one frame (~17 ms), a
file Koetama writes is seen by `HasFile` within ~17 ms, and spawning a prefab and reading its tags takes ~28 ms.

**Teardown (Proximity Babble Chat).** The feed is `savegame.mod.pcvx.f` (hex), written 20 times a second while there
are voices or a voice room, 5 times otherwise, at once when a line is queued for translation or the talk key changes,
and once more (`listen` off, no speakers) when nothing is left. It sits in `savegame.xml` under the mod's tag
(`local-<folder>`, `steam-<id>`).

## Adding a game mod: profiles

Koetama's engine knows no game. Speech detection, speech to text and the language detector are the crate
`kd-speech`, and the voice mixer is `kd-audio` (in `app/crates/`). A game mod is linked to Koetama by a **profile**:
a small JSON file that names one of Koetama's built-in **connectors** and gives its settings. A profile is never
code, so anyone can write one for their game's mod without changing Koetama. A connector only does what this
section describes. Teardown's link is a profile too, built into Koetama (`app/crates/kd-games/src/profiles/teardown.json`).

Three connectors, the transports above: **socket** (the mod opens a TCP connection), **http** (the mod makes HTTP
requests) and **files** (the mod writes its feed into a file, and reads Koetama's objects from files next to
itself).

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
  "test_voices": [{"id": 1, "voice": "Microsoft Zira Desktop", "rate": 0, "text": "I am the test speaker."}],
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
| `uses` | optional | Any of `"voices"`, `"speech"`, `"translate"`; the default is `["voices", "speech"]`. `voices`: Koetama plays the speakers in the feed (other players' voices). `speech`: Koetama listens to the microphone and sends what the player said (speech to text). A speech-only mod doesn't need to send speakers (Koetama drops them). A voices-only mod's `listen` is ignored, so the microphone never opens. `translate`: Koetama translates the chat lines the game sends ("Translation"); without it the feed's `translations` and `to_translate` are ignored. The `hello`'s `features` tell the mod which. |
| `test_voices` | optional | Up to 16 recorded voices for the mod's test speakers: `{"id": 1..999, "voice": "<Windows voice>", "rate": -10..10, "text": "..."}`. They're made once with the Windows speech voices (none on other systems), and a speaker with `"test_voice": <id>` plays one. |
| `speaker_names` | optional | Names for the test voices in Koetama's window, by their `id`, e.g. `{"1": "the whisperer"}`. Real players need nothing: they show as "player <id>". |
| `connector` | required | `{"type": "files", ...}`, `{"type": "socket", ...}` or `{"type": "http", ...}`, described below. |

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
| `feed.pattern` | default: Teardown's | A regex with exactly one group, which captures the feed string. Each match is one copy of the mod. It runs over the file's bytes, and `(?-u)` lets it match any bytes. `""`: the whole file is the feed. |
| `feed.tag_pattern` | default: Teardown's | A regex with one group: the tag of the mod copy that wrote a feed (the last match before it). `""` means no tags. |
| `feed.complete` | default `</registry>` | The file is read only when it ends with this text, ignoring trailing white space, so a half-written file is skipped. `""` reads it every time. |
| `out.dirs` | required | 1 to 8 folders the mod looks in (each a path or candidates). Folders not on this PC are skipped. |
| `out.tag_dirs` | optional | `[{"tag_prefix": "steam-", "dir": 1}]`: a mod copy whose tag starts with the prefix gets the folder at that index of `out.dirs`. Any other copy gets folder 0. |
| `out.prefix` | default `pcvx_` | The start of every file name Koetama writes. Unique among game mods: a profile whose prefix another one uses is refused (the two would remove each other's files), so pick one from your mod's name (`pcvx_` is Proximity Babble Chat's). |
| `out.message` | default `teardown-prefab` | How an object file is written: `json` (`<prefix>t<n>.json`, the object as one line) or `teardown-prefab` (`<prefix>t<n>.xml`, the object's hex in a prefab's tag): "Transport: files". |

The feed and the files are as in "Transport: files". A feed string that was in the file when Koetama started does not
count as live until it changes.

### The socket connector

`{"type": "socket", "port": 47120}`: the port is 1024 to 65535. Everything else is in "Transport: socket". To start
from: `examples/socket_client.py` (Python 3, no packages); install `examples/profiles/example-socket.json`, pick
"Example Game" in Koetama, run the script and talk.

### The HTTP connector

`{"type": "http", "port": 47140, "allow_origins": ["https://my-game.example"]}`: the port is 1024 to 65535;
`allow_origins` (optional, at most 16) lists the web pages that may use it from a browser, each exactly
`http(s)://host[:port]` (no path, no `*`). Everything else is in "Transport: HTTP".

### What a profile can and cannot make Koetama do

- It never runs code. A test voice's text and voice name reach the Windows speech voice as data, never inside a
  command.
- The files connector writes only into folders that already exist (it never creates one). Every file it writes
  starts with the prefix. The prefix must be 3 or more letters, digits or `_`, ending in `_`. The connector deletes
  only files whose whole names are exactly `<prefix>on`, `<prefix>p<digits>`, `<prefix>t<digits>.<xml or json>` or
  `<prefix>w<digits>.tmp` (and the `<prefix>v<digits>`, `<prefix>vc`, `<prefix>vx` an older Koetama left), and
  nothing else, not even another file starting with the prefix.
- The socket and HTTP connectors listen only on 127.0.0.1, on the profile's port (1024 to 65535); the HTTP one
  refuses web pages unless the profile names them. Every connector reads only the feed and sends only the objects
  above: the hello, what the player said, the voice room and the voice chat's
  state, and translations of the lines the game sent.
- The window shows players all of this, with real paths, before a profile is installed.
