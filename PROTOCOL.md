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
     "me":"76561198000000001","room_seed":"lobby 1234 + its password","range":[10,30],
     "listener":{"position":[0,1.7,0],"forward":[0,0,1],"right":[1,0,0],"up":[0,1,0]},
     "speakers":[{"id":"76561198000000002","name":"Ana","position":[3,1.7,8],"muffle":0.1}],
     "to_translate":[{"id":7,"text":"こんにちは"}]}

| field | default | meaning |
|---|---|---|
| `volume` | 1 | 0..1, how loud the other players' voices are |
| `listen` | `"off"` | the microphone: `"off"` (closed), `"always"` (the speech detector finds each line), `"push_to_talk"` (a line only while `talk_key` is held) |
| `talk_key` | false | push to talk: true while the talk key is held. Send a feed as soon as it changes. The microphone stays open, so a line starts from just before the key arrived (the feed's delay costs no first word) and ends 0.25 s after it is let go |
| `lang` | `"en"` | the language the player speaks (`en`, `ru`, `zh`, `yue`, `ja`, `ko`, `es`, ... or `auto`: Koetama finds it, even several in one line) |
| `live` | true | the words so far while the player talks (false: only the finished line; less CPU) |
| `speakers` | none | the other players this player can hear, and how: `id` (required: their player id), then EITHER `position` (with `listener`: Koetama works out the direction, and the loudness from the distance and that player's `range` - "Positions and ranges") OR `azimuth` (degrees from where the camera looks: 0 ahead, 90 right, ±180 behind) and `elevation` (degrees up); `gain` 0..1 (given: used as it is; 0: not heard), `muffle` 0..1 (0 clear, 1 the most muffled; behind a wall ~0.7, or where words are garbled), `name` (shown in Koetama's window), `range` (theirs, until their own Koetama announces one), `via` (the devices their voice also comes out of: "Devices"). A **test voice** instead of a player: `"test_voice": n` (one of the profile's `test_voices`), `"talking": true` while it should play, and optionally its own `range` |
| `me` | none | this player's id in the game session: a whole number or a string of 1 to 64 characters (at most 255 bytes of UTF-8; a Steam id, a name) - the same one the other players' games use for this player |
| `name` | `""` | this player's name (shown in the other players' Koetama windows) |
| `room_seed` | none | the session's voice room, as any string of 1 to 256 characters every player of the session has (a lobby id and its password, a server's address and world): each Koetama makes the same room and key from it. Anyone who knows the seed can listen: put something private in it ("Real voices") |
| `room`, `key` | none | instead of `room_seed`: the room and key themselves, 32 and 64 lower-case hex digits (the ones Koetama offers in a `room` object, shared by the game: "Real voices"). Neither: no voices sent or heard |
| `listener` | none | where this player hears from: `{"position": [x,y,z], "forward": [x,y,z], "right": [x,y,z], "up": [x,y,z]}` - position the character's head (not the camera), the directions the camera's in the game's own units and axes (the three directions settle which way is right; they need not be unit length) |
| `range` | none | how far this player's voice reaches now: `[near, far]` in the game's units - full loudness within `near`, nothing beyond `far` (a whisper `[1,4]`, talk `[8,25]`, a shout `[20,70]`). Other players' Koetamas use it for this player's loudness |
| `to` | (with `range` and positions: everyone within `far`) | the player ids who should get this player's voice right now; empty: nobody. Left out: with `range`, `listener` and speakers' positions, Koetama sends to the speakers within `far` (and 10 % more); else nobody |
| `transmit` | false | this player's voice is going into a device now ("Devices"): `true` - also to everyone in the voice room; a list of player ids - also to them |
| `region` | `""` | where the voice room should live: `wnam`, `enam`, `sam`, `weur`, `eeur`, `apac`, `apac-ne`, `apac-se`, `oc`, `afr`, `me`; `""` (or anything else): wherever the first player is. Every player of a session sends the same one |
| `translate` | true | false: stop translating for a while (Koetama answers nothing new until it is true again). What is translated into what is the player's setting in Koetama's window, not the game's: "Translation" |
| `to_translate` | none | the chat lines to translate: `{"id", "text"}`, at most 16, each at most 400 bytes of UTF-8. Ids are the game's (1 to 15 digits, unique in the session). Keep a line in every feed until its `translation` arrives (drop it after ~10 s without one) |

Bad values are skipped or replaced by the default (a bad room, key or `me`: no room). Numbers may be written as
decimals (`1.0`) and an empty list as `{}` (Lua's JSON libraries do both).

A **hub** (a game whose script runs only on the host, "Hub") adds `players`: one feed per other player.

### Koetama -> game

Each object has a `"type"` first.

| object | when |
|---|---|
| `{"type":"hello","app":"Koetama","version":"0.4.0","protocol":2,"features":["speech","voices","rooms","translate"]}` | first, each session (socket: each connection). `features`: what Koetama does for this game - what its profile uses, and `rooms` (real voices) with `voices`. Ignore a feature you do not know |
| `{"type":"speech","kind":"start","utt":4}` | the player started talking (no words yet: the moment the speech detector hears a line begin, so the game can show them talking) |
| `{"type":"speech","kind":"live","utt":4,"text":"hello there","times":[0.1,0.55],"ago":1.02}` | the words so far, while they talk (only with `live`) |
| `{"type":"speech","kind":"final","utt":4,"text":"hello there everyone","times":[0.1,0.55,0.9],"ago":2.4}` | the finished line (`text` may be `""`: nothing made out; the live words go) |
| `{"type":"room","room":"<32 hex>","key":"<64 hex>"}` | a new voice room, once per session: "Real voices" |
| `{"type":"voice","state":"connected","players":["7656...02"]}` | the voice chat changed: `state` `off`, `connecting`, `connected`, `unreachable` (the last tries failed; it keeps trying) or `id_taken` (another player's id clashes with this one in this room: no voice this session - "Real voices"); `players`: the other players whose Koetama is in the room (their ids as the game gives them) |
| `{"type":"talking","id":"7656...02","talking":true}` | a player's voice started (`true`) or stopped (`false`) being heard here - and this player's own (`id` = `me`) being sent. For speaking icons over heads |
| `{"type":"status","speech":"ready","microphone":"open"}` | after the hello, and when it changes: `speech` `off` (the game does not ask for it, or this Koetama listens to no microphone: started to take typed lines only), `loading` (the models load; the first time they are downloaded), `ready` or `error`; `microphone` `closed`, `open` or `none` (no microphone). Lines said before `ready` and `open` are not heard |
| `{"type":"translation","id":7,"text":"Hello","from":"es","to":"en"}` | the translation of line `id`, exactly one per id; `from` / `to`: the pair that was used (left out with `""`). `""`: nothing to show (translation off, nothing in a language the player doesn't speak, no model for it, the same as the line, or a bad line). A line that comes while its language's models are downloading or loading waits for them (up to 2 minutes). At most 1000 characters |
| `{"type":"translations_status","into":"en","translations":[{"from":"ja","to":"en","state":"downloading","progress":0.42}]}` | the player's translation: `into`, the language their chat is translated into (`""`: off), and each pair in use this session with its state - after the hello and when one changes (at most every 0.5 s while downloading): `ready`, `downloading` (with `progress` 0..1), `loading`, `unavailable` (no model for it, or not on this PC with downloads off) or `error` (tried again after a minute) |
| `{"type":"join_code","player":"7656...02","code":"K7QF-4MXA"}` | a hub only ("Hub"): the code that player types into their Koetama to join |
| `{"type":"player","player":"7656...02","joined":true}` | a hub only: that player's Koetama joined (or left) |

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
player's Koetama the session's room, this player's id (`me`), and where everyone is ("Positions and ranges") or how
loud and from where each other player is heard (`speakers`) and who should get this player's voice (`to`).

**One room for the session**, two ways:
- **`room_seed`**: a string every player's game already has - a lobby or match id, a server's address and world name,
  a co-op password. Each Koetama makes the room and key from it: s = scrypt(seed, salt "koetama room seed",
  N = 2^16, r = 8, p = 1, 32 bytes) - slow on purpose (~0.1 s, once per seed), so whoever sees a room's name cannot
  try guesses at the seed quickly -, then room = the first 32 hex digits of HMAC-SHA256(s, "koetama room") and key =
  HMAC-SHA256(s, "koetama key"). No mod networking needed. Whoever knows the seed can listen, so mix in something
  private: a lobby password, a key the host's mod shares, the session's start time and the players' ids together -
  and the game's name, so two games' "lobby 42" are not one room.
- **`room` and `key`**: once per session Koetama offers a fresh random room in a `room` object (a game script has no
  good random numbers). The game gives ONE room to every player (in Teardown: each player's game forwards its offer to
  the host, the host keeps the first and shares it), and every player's feed names it. Private by construction.

**Player ids** are the game's own (`me`, speakers' `id`, `to`): numbers or strings. Inside the room each one is a
16-bit number, the first two bytes of SHA-256("koetama id:" + room + ":" + id) (1..65535; a number id as its decimal
digits), so every Koetama names every player alike. **A number belongs to the Koetama that connects with it first**:
each connection carries an `owner` (32 hex digits: HMAC-SHA256 of a secret made once per install, kept in Koetama's
data folder, with the room and the number - the same every time that install connects as that player, nobody
else's). The relay lets the same owner reconnect (its old connection is closed with 4000) and refuses anyone else
(409): nobody can push a player out of the room by taking their number. Two ids of one room may meet on one number
(about 1 in 500 with 12 players): the second player's Koetama is refused and told `id_taken`, and stays out of that
room (`voice` object); the next session has another room, and other numbers.

**Sending.** Koetama connects to `<relay>/v1/room/<room>?me=<me>&owner=<owner>[&region=<region>]` (a WebSocket) while the feed
names a room, reconnecting after a drop (1, 2, 4 ... 30 s), with a text `ping` every 20 s. It sends while the player
talks (push to talk: from 0.15 s before the press arrived until 0.25 s after the release; always: while the speech
detector hears speech, from 0.3 s before it noticed), only to `to`. Audio: Opus, 48 kHz mono, 20 ms frames, 24 kbit/s,
3 frames (60 ms) per packet.

- **Frames to the relay** (binary): `[1][n][to_1 .. to_n as u16 big-endian][payload]`, n <= 64. **From the relay:**
  `[1][from as u16 big-endian][payload]`. The relay forwards a packet to the named players only, never back to the
  sender, and never looks inside.
- **Payload** = `nonce (12 random bytes) | ChaCha20-Poly1305(key, nonce, plaintext, aad = from as u16 big-endian)`;
  a packet that does not decrypt is dropped. Plaintext (version 2): `[2][seq: u32 BE][flags: u8: 1 = the last packet
  of a stretch of talking, 2 = presence (no audio), 4 = the sender's id is a number][near: f32 BE][far: f32 BE][n: u8][the sender's id: n bytes UTF-8]
  [k][k x (len: u16 BE, Opus bytes)]` - the sender's `range` (0, 0: none given) and its game id travel with its voice.
  **Presence:** every 5 s a packet without audio goes to every player in the feed's `speakers` and `to`, so the
  others know who is in the room (`voice` `players`: heard from in the last 15 s).
- **Playing:** per sender a jitter buffer (starts at 60 ms buffered and 40 ms after the first packet; at most
  300 ms), Opus loss concealment for a missing packet, ended 0.5 s after the last packet. Mixed with that player's
  loudness, direction and muffle (the feed's, or worked out from positions).

**The relay** (`relay/`): one Durable Object per room (by name; with a region: `<room>@<region>`, created with that
location hint), the WebSocket Hibernation API. Limits: 64 players in a room, 64 recipients and 4000 bytes of
payload in a packet, 60 packets a second from one connection (more are dropped; twice that and the connection is
closed: 4008), 60 new connections a minute from one address (429); a second connection with the same `me` replaces the
first (close code 4000) only with the same `owner` - else it is refused (409). It keeps no logs. `/` and `/v1` say
what it is. `npm test`, `node test/smoke.mjs <url>` (a live room),
`npm run deploy` (from `relay/`, the `relay` conda env).

## Positions and ranges

A game can give Koetama positions instead of doing the sound's maths itself:
- **`listener`**: this player's ears - `position`: the character's head (not the camera: in third person it can be
  metres away, and distances would not match what the game shows), and the directions `forward`, `right` and `up`
  (the camera's, so a voice pans with where the player looks), in the game's units and axes. Giving all three settles left-handed against right-handed axes.
- each speaker's **`position`**: Koetama works out the direction (azimuth, elevation) from the listener, and the
  loudness from the distance: 1 within that player's `near`, falling to 0 at their `far` (the square of the way
  left: `((far - d) / (far - near))²`). Their range is the one their own Koetama announces (their feed's `range`,
  carried in their voice packets), else the speaker's `range` in this feed, else this player's own `range`, else
  `[10, 30]`.
- a speaker's **`gain`**, when given, wins over the distance (a radio, a phone call, the dead hearing the living).
- **`range`**: how far this player's voice reaches right now (whispering, talking, shouting), in the game's units.
  With it, positions and no `to`, Koetama sends this player's voice to the speakers within `far` (+10 %).
- `muffle` stays the game's (it knows the walls: a raycast between the two players).

Azimuth and elevation (and `gain`, `to`) still work as before for a game that works them out itself.

## Devices: walkie-talkies, loudspeakers, a PA

A voice can also come out of things: a walkie-talkie on someone's belt or on the floor, a loudspeaker (a megaphone,
an intercom), a PA system's speakers around a building. Each sounds like one: a walkie-talkie narrow, hissing and
crackling, with a click when the talk button goes down and a burst of static (the squelch tail) when it comes up; a
loudspeaker narrow, hard and ringing; a PA fuller, through several speakers at once - the farther ones arriving later
(echo) - and the hall's reverb. The direct voice still plays where the player stands, as usual.

These are ways a voice can sound, nothing more: whether a game has walkie-talkie items, needs one to use the radio,
or just offers a "radio" chat mode is the game's business. Koetama plays what the feed asks for. A device with no
position (`{"device": "radio"}`) plays in this player's own ear: a radio chat with no items at all.

**The talker's game** says when their voice goes into a device (the walkie's talk button held, at the PA's
microphone, a megaphone up): **`transmit`** in the feed - `true` (to everyone in the voice room) or a list of player
ids (the ones whose games might play it: the walkie channel's players, everyone near the PA's speakers). Their voice
then goes to those players as well as to `to`. Left out or `false`: no device.

**Each listener's game** says where that voice comes out for them: the speaker's entry gets **`via`**, a list of
devices (at most 8), each

    {"device":"radio","position":[4,1,2],"signal":0.7}
    {"device":"loudspeaker","position":[0,3,10],"range":[5,40]}
    {"device":"pa","positions":[[0,5,0],[30,5,0],[60,5,0]],"effects":{"reverb":0.8}}
    {"position":[0,2,0],"effects":{"pitch":5,"wobble":0.4,"static":0.3}}

| field | default | meaning |
|---|---|---|
| `device` | `plain` | the preset: `radio` (a walkie-talkie, a radio set), `loudspeaker` (a megaphone, an intercom, one horn), `pa` (a PA system: several speakers, a hall), `plain` (no effects but the ones given) |
| `position` / `positions` | none | where it is (with `listener`), or several places that play together (at most 16; a PA's speakers): direction and loudness from each, and a speaker farther than the nearest one is heard later by the difference in distance (sound at 343 units a second: the game's units are taken as metres) |
| `azimuth`, `elevation` | 0 | instead of positions, for a game that works out directions itself |
| `range` | radio `[1,8]`, loudspeaker `[5,40]`, pa `[10,60]`, plain `[10,30]` | how far the device is heard: loudness by distance as for voices ("Positions and ranges") - the device's own, not the talker's |
| `gain` | (by distance) | given: used as it is (0: not heard) |
| `facing` | none | the direction it points ([x, y, z]): a horn is loud in front and quiet behind - by the angle to the listener, 1 straight ahead down to `back` straight behind (back + (1 - back) ((1 + cos) / 2)²). A megaphone: where its holder looks |
| `back` | loudspeaker 0.15, others 1 | the loudness straight behind, 0..1, when it has a `facing` (1: the same all round) |
| `muffle` | 0 | as for a voice: the walls between the listener and the device |
| `signal` | 1 | reception, 0..1: lower is more static and crackle, words breaking up below ~0.3 |
| `effects` | the preset's | how it sounds: "Sound effects" |

A speaker with `via` and no `position`, `azimuth` or `gain` is heard only through its devices (a walkie-talkie
across the map). A speaker's `gain`, `position` and `effects` are its direct voice only: `gain: 0` silences that, never
its devices (each has its own `gain` and `effects`). A talker's loudness carries through (a whisper into a walkie-talkie comes out quiet, a shout
distorts). Test voices take `via` too, so a game can try its devices with no second player. Devices are the game's to
show: `talking` says when a player is heard, and the text bubbles of what they said (`speech`, shared by the game as
usual) belong at the devices as well as over the talker.

## Sound effects

How a device - or a player's own voice - sounds is a chain of effects, each set on its own. A device starts from
its preset; `effects` changes any of them (0, `false` or `null`: off) and leaves the rest. A speaker's own
`effects` (on their entry in `speakers`, no preset) change their direct voice: a helmet, a robot, a ghost. So the
game decides how each of its voice modes sounds, and gets a sensible sound when it says nothing.

    "effects": {"band": [300, 3000], "drive": 0.4, "static": 0.2, "pitch": 4, "echo": [0.25, 0.4], "reverb": 0.3}

| effect | value | what it does | radio | loudspeaker | pa |
|---|---|---|---|---|---|
| `band` | [low Hz, high Hz] | keeps only that band (a small speaker, a phone line) | [300, 3000] | [400, 5000] | [150, 7000] |
| `drive` | 0..1 | saturation and clipping (louder words break up more) | 0.4 | 0.6 | 0.2 |
| `compress` | 0..1 | evens out loudness (a whisper and a shout come closer) | 0.35 | 0.4 | 0.5 |
| `static` | 0..1 | hiss under the voice | 0.2 | | |
| `crackle` | 0..1 | crackles and dropouts | 0.1 | | |
| `squelch` | 0..1 | a click when the voice starts, a burst of static when it ends (a walkie-talkie's talk button) | 0.8 | | |
| `horn` | 0..1 | a horn's resonances and metallic ring | | 0.7 | 0.2 |
| `lofi` | 0..1 | lower sample rate and bits (a cheap digital link) | 0.2 | | |
| `wobble` | 0..1 | the pitch and loudness waver (tape, a fading signal, underwater) | | | |
| `pitch` | -12..12 | semitones up (a higher voice) or down | | | |
| `robot` | Hz, 0..2000 | ring modulation (a robot; ~30-100 Hz) | | | |
| `echo` | seconds, or [seconds, feedback 0..0.9] | a repeating echo (a canyon; feedback 0.35 when not given) | | | |
| `reverb` | 0..1 | the room: a small room .. a hangar | | | 0.5 |
| `hum` | 0..1 | mains hum | | | 0.05 |

(The presets' numbers may still be tuned; a game that wants a sound exactly should give its effects.) A device's
`signal` adds static and crackle on top. Effects never make the voice much louder or quieter: the
distance does that.

## Hub: games whose script runs only on the host

Some games run mod scripts on one machine only (Tabletop Simulator: the host; a server-side mod): the other players'
games tell their Koetamas nothing. The host's Koetama then works as a **hub** for them:

1. The host's feed adds **`players`**: a list of feeds, one per other player, each with that player's `id` (as in
   `me`) and anything a feed holds for them - `listen`, `talk_key`, `lang`, `name`, `listener`, `speakers`, `range`,
   `to`, `translate`, `to_translate`. The room (`room_seed`, or `room` and `key`) and `region` are the host's
   unless a player's feed gives its own. The host's own fields stay at the top, as usual.
2. For each player the hub answers a **`join_code`** (`K7QF-4MXA`: 8 characters, about 40 bits), once per game
   session. The game shows each player their own code, privately (TTS: `broadcastToColor`). **A code works once.**
3. The player types it into their Koetama ("Join a hosted game" in the window; `koetama --cli --join K7QF-4MXA`).
   Their Koetama and the hub pair through the relay (below) and from then on it works as if that player's feed came
   from a game on their own PC: their microphone, speech to text, their voice in the session's room, their
   translation (with their own Koetama's setting: nothing of it comes from the host). The hub sends it that player's feed whenever it changes (at least once a second).
4. Whatever that player's Koetama would tell a game, the hub tells the host's game, with **`"player": <id>`** added:
   `speech`, `talking`, `translation`, `translations_status`, `status`, `voice`. A `player` object says when a
   player's Koetama joins (only once the paired link is up) or leaves.
5. If a paired player's Koetama is silent for 30 s (closed), the hub drops that link and makes the player a **new
   code**: the game gets a new `join_code` object for them (and `player` left). A player whose connection merely
   dropped comes back over the link with the key their Koetama still holds - no new code.

**Pairing.** The code makes a room and a key: `HMAC-SHA256(code, "koetama hub room")` (its first 32 hex digits) and
`HMAC-SHA256(code, "koetama hub key")` (the code's 8 characters, upper case, no dash). In that room, sealed with that
key, the player's Koetama says `{"hello": "<its X25519 public key, 64 hex>"}` (again every second until answered)
and the hub answers `{"welcome": "<the hub's X25519 public key>"}`. Both then derive the link:

    link_key  = HMAC-SHA256(code_key, "koetama hub link" | X25519 shared secret | hub_pub | player_pub)
    link_room = first 32 hex digits of HMAC-SHA256(link_key, "koetama hub link room")

and move to `link_room` (hub relay id 1, player 2, as before), sealing everything with `link_key`. After its first
hello the hub leaves the code's room and never listens there again (a later hello is ignored). Feeds and objects go
only over the link, never in the code's room; a player's Koetama takes a feed only from the link. Each pairing makes
fresh X25519 keys (a low-order public key is refused).

What this protects: someone who sees a code after it was used (a stream, a screenshot), or who sees the relay's
traffic and guesses the code from the room's name, gets nothing - no feed (with the session's voice room key), no
way to send objects to the host's game or to steer the player's microphone. What it cannot: someone who has the code
**before** the player uses it can race them - pair with the hub as that player, or pose as the hub to the player (an
active man in the middle). Short codes cannot prevent that without a PAKE; the window and the host's game show a
player as joined only once the link is up, so a player who never gets there, or a join nobody made, calls for a new
code (the game leaves that player out of `players` once, then lists them again: a new player gets a new code).

Between the hub and a player's Koetama: plaintext type 3 = a JSON message (`{"feed": {...}}` one way, `{"objects":
[...]}` the other), cut into parts of at most 3500 bytes: `[3][message: u32 BE][part: u8][parts: u8][bytes]`, hub
relay id 1, player 2.

## Translation

Translation is the **player's setting, in Koetama's window**: "Translate chat into" (Off - the default, so nothing
downloads until the player chooses - or a language) and "Download translation models when needed" (on by default).
The game only sends the full chat lines it shows (typed lines, and spoken lines once finished - never live words) in
`to_translate`; Koetama translates them on the player's PC with Mozilla's Firefox Translations models (MPL-2.0, ~20-55
MB a direction) and answers each with a `translation`. A game sends `"translate": false` to stop it for a while.

- **What is translated**: every stretch of a line in a language the player does not speak (the languages they
  ticked under "Languages I speak", else the game's `lang`) and that is not the target is translated into the target;
  stretches in the player's own languages and in the target are left as they are, the order kept. Short or unsure
  stretches are told among the player's languages and the target first, so "ok", "lol" or "gg" stay as they are.
  Chinese and Cantonese count as one language here.
- **Pairs**: one per foreign language seen ("from" that language "to" the target), made the first time the
  language shows up: its models are fetched (downloaded if downloads are on, else only models already on this PC are
  used) and loaded, and that line waits for them (up to 2 minutes). A pair without English goes through English (two
  models). At most four pairs keep their models in memory; another lets go of the least recently used. The pairs in
  use and their states are in `translations_status`.
- A language Mozilla has no model for is `unavailable` (left untranslated). Cantonese (`yue`) only as a source
  (through the Traditional Chinese model); Maltese only into English.
- A hub's players (below) each use their own Koetama's setting; nothing of it comes from the host.

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
| `uses` | optional | Any of `"voices"`, `"speech"`, `"translate"`; the default is `["voices", "speech"]`. `voices`: Koetama plays the speakers in the feed (other players' voices). `speech`: Koetama listens to the microphone and sends what the player said (speech to text). A speech-only mod doesn't need to send speakers (Koetama drops them). A voices-only mod's `listen` is ignored, so the microphone never opens. `translate`: Koetama translates the chat lines the game sends, into the language the player chose in Koetama ("Translation"); without it the feed's `translate` and `to_translate` are ignored. The `hello`'s `features` tell the mod which. |
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
