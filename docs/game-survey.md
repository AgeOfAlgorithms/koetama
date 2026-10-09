# Can game mods connect to Koetama? A survey, five demos, and an assessment of the API

*2026-10-09. Research: ~40 games and platforms (sources linked in each finding; "[i]" = inference, not verified).
Demos built and tested against the real Koetama: Teardown (the reference mod), Valheim, Godot, Tabletop Simulator,
Monster Hunter Rise. API as of PROTOCOL.md "protocol 2".*

## Summary

Every game whose mods can run real code reaches Koetama through the **socket** transport with no trouble. Two more
transports cover the sandboxes: **HTTP** (Lua games that can only make web requests) and **files** (games that can only
write and read files). Across the survey, only anti-cheat games, single-player games and console-only features are out
of reach.

The API's weak spots are not the transports but four things every mod had to solve on its own:
1. **The voice room**: getting one room and key to every player of a session. This is easy with a host and mod
   networking, and hard or impossible without (dedicated servers, games with no mod networking).
2. **Player ids of 1..65535**: games have 64-bit ids or names, and a hash collision silently kicks a player off voice.
3. **Spatial maths in every mod**: azimuth, elevation and gain from positions, with the same ranges on every PC.
4. **No per-player voice feedback**: nothing says who is talking, so there are no speaking icons over heads.

## The demos

| Game | Mod platform | Transport | Built | Tested | Line said → in the game |
|---|---|---|---|---|---|
| Teardown | Lua (sandbox: registry + prefab files) | files (hex in savegame.xml, prefab objects) | Proximity Babble Chat | 654 offline checks; in-game pending | ~0.1-0.3 s |
| Valheim | BepInEx 5, C# (Mono) | socket | `examples/mods/valheim` (reusable `KoetamaClient` + plugin) | 78 unit + 15 end-to-end vs real Koetama; installed, not run in-game | speech end → final ~1.0 s |
| Godot 4 (any Godot game; Webfishing via GDWeave) | GDScript | socket | `examples/mods/godot/koetama.gd` | headless vs real Koetama | typed → signal ~50 ms |
| Tabletop Simulator | MoonSharp Lua (host only) | HTTP | `examples/mods/tabletop-simulator/Global.lua` | LuaJIT harness vs real Koetama | 17 ms (one frame) |
| Monster Hunter Rise | REFramework Lua | files (whole-file JSON) | `examples/mods/monster-hunter-rise/` | LuaJIT harness vs real Koetama | 17 ms (one frame) |

Translation replies took 30-60 ms once the models were loaded; speech to text ~1 s after the speaker stops (the model,
not the transport). An HTTP round trip to Koetama is under 1 ms.

## The survey

### Transport per game

| Game / platform | Mod code | Best transport | Room key via | Built-in proximity voice | Fit |
|---|---|---|---|---|---|
| Valheim | BepInEx C# | socket | ZRoutedRpc | no | **good** (demo) |
| Lethal Company | BepInEx C# | socket | LethalNetworkAPI | yes (Dissonance) | text/translation; replace voice = work |
| PEAK | BepInEx C# | socket | Photon custom properties | yes (Photon) | text/translation |
| Mage Arena | BepInEx C# | socket | (Steam lobby) | yes; speech-cast spells | translation, STT spells |
| Super Battle Golf | BepInEx C# | socket | P2P mod messages | yes | text/translation |
| Among Us | BepInEx 6 IL2CPP (x86) | socket | Reactor RPC (modded servers only) | no | official servers: key sharing blocked |
| Stardew Valley | SMAPI C# | socket | `Helper.Multiplayer` | no | **good** |
| Terraria | tModLoader C# | socket | ModPacket | no | Workshop rules forbid "external software" |
| RimWorld | Harmony C# | socket | - | - | no avatars: poor fit |
| Space Engineers | Workshop C# (whitelist) / Pulsar plugins | files (mods) / socket (plugins) | MyAPIGateway.Multiplayer | yes (antenna range) | ok |
| Webfishing | GDWeave (.NET 8 + GDScript) | socket | Steam P2P | no (ReelChat mod) | **good** |
| Godot games | GDScript | socket (StreamPeerTCP) | game's own | varies | **good** (demo) |
| Minecraft Java | Fabric/Forge Java | socket | plugin channels | no (Simple Voice Chat mod) | good [i] |
| Skyrim / Fallout 4 | SKSE/F4SE C++, Papyrus | socket (plugin) / files (Papyrus JsonUtil) | Skyrim Together: STRPM chat envelopes | no | good with a plugin |
| Cyberpunk 2077 | CET Lua / RED4ext | files (CET, own folder) / socket (RED4ext) | - | - | single player (CyberMP not out) |
| Baldur's Gate 3 | Script Extender Lua 5.4 | files (Script Extender folder) | Ext.Net | no voice, no chat | good |
| Project Zomboid | Kahlua | files (~/Zomboid/Lua) | sendClientCommand | yes | text/translation |
| Don't Starve Together | Lua 5.1 (HTTP to localhost only since Jan 2025) | **HTTP** | mod RPC | no | good, ~10 Hz polling |
| Factorio | Lua 5.2 lockstep | files out + **UDP in** (`--enable-lua-udp`) | (host's Koetama injects) | no | inbound needs a UDP sender |
| Tabletop Simulator | MoonSharp, host only | **HTTP** (or Editor API TCP 39998/39999) | none: no client scripts | global voice | host only (demo) |
| Monster Hunter Rise | REFramework Lua (files in reframework/data) / native plugin | files / socket via plugin | **none** (no mod networking) | lobby voice | room needs another way |
| Monster Hunter World | Stracker C++ | socket | none | voice | room needs another way |
| Elden Ring | DLL mods + Seamless Co-op (no EAC) | socket | none (Seamless password) | none on PC | room from the password |
| Titanfall 2 / Northstar | Squirrel (HTTP; localhost needs `-allowlocalhttp`) / plugins | HTTP or socket (plugin) | remote functions | yes (vanilla) | ok |
| Arma 3 | SQF + callExtension DLL | socket / named pipe (TFAR, ACRE do this) | remoteExec | yes (VON) | BattlEye needs whitelisting |
| FiveM / RedM | server resources; NUI = Chromium | **HTTP or WebSocket from NUI** (TokoVOIP did WS) | server events | yes (pma-voice) | server owner installs |
| GTA V single player | ScriptHookVDotNet | - | - | - | single player only |
| Unreal games (UE4SS) | Lua 5.4 with io/package, C++ mods | files (Lua io) / socket (Lua C module or C++) | game RPC hooks | varies | ok |
| Garry's Mod | GLua (HTTP; binary modules) | HTTP / socket (module) | net library | yes | ok [i] |
| Roblox | Luau, HttpService server-only, no localhost | - | - | - | **no** |
| Browser games | JS fetch / WebSocket | **HTTP** with `allow_origins` (CORS + Private Network Access) | game's own | - | ok |
| Outlast Trials, Marvel Rivals, The Finals, PoE 2 | anti-cheat / no mods | - | - | - | **no** |
| Hardspace: Shipbreaker, TowerFall, Portal Knights | single player / local / no code mods | - | - | - | no fit |

### What the mod platforms have in common
- **Real code (C#, C++, Java, GDScript):** TCP to 127.0.0.1 always works. Unity Mono: avoid `ClientWebSocket` (old
  Mono bugs) and `NamedPipeServerStream` (not implemented); plain TCP is safest. IL2CPP loaders run mod code on .NET 6.
- **Lua sandboxes** split three ways: HTTP only (DST, TTS, Northstar), files only (Teardown, Zomboid, BG3SE, CET,
  REFramework, Space Engineers mods), or full libraries (UE4SS).
- **Prior art proves the model:** Arma's TFAR/ACRE (game DLL → named pipe → TeamSpeak plugin), FiveM's TokoVOIP
  (sandbox → in-game browser → WebSocket → TeamSpeak plugin), BetterCrewLink for Among Us (reads game memory, no mod),
  and the Mumble Link shared memory many games and mods already write (Valheim PositionalAudio, Factorio).
- **Many multiplayer games already have proximity voice** (Lethal Company, PEAK, Mage Arena, Subnautica 2, Escape the
  Backrooms, Super Battle Golf, Project Zomboid, Space Engineers, FiveM). There, Koetama's value is speech to text,
  live captions (deaf and hard-of-hearing players) and translation, not its voice chat.

## Assessment of the API

### What held up
- **One set of objects over every transport.** Five clients in five languages each needed a few hundred lines; four
  of the five platforms had JSON built in, and the Lua sandboxes used a small vendored one.
- `hello` with `features`, live words that only grow, `times`/`ago`, the `!` hint, speech → chat with no helper.
- The files transport works even in very restricted sandboxes (one frame of latency in MH Rise).

### Fixed during this survey
- **HTTP transport** added (DST, TTS, Northstar, FiveM's in-game browser, browser games): objects kept until acked,
  `wait` long-poll that a newer feed ends at once, `first`/`last` numbers, under 1 ms a round trip, browsers only
  from the profile's `allow_origins`.
- **Whole-file feeds** (`"pattern": ""`): a Lua mod writes its feed object as a JSON file.
- **Lua's numbers**: `1.0`, `1.7e12` where a whole number belongs, and `{}` for an empty list, were rejected (the whole
  feed, silently over files). Now accepted; an unreadable feed over files is told in Koetama's log.
- **A mod's folder made after Koetama started** is found (it was never written to).
- **Translation**: a line sent while models downloaded got `""` and was lost; it now waits for them. Short chat lines
  were often detected as the wrong language ("Vamos a jugar otra vez" as Hungarian, "Me voy a dormir" as Slovene); short
  or unsure stretches are now told among the player's translation languages and English first.
- `--game <id>` for Koetama's command line (headless tests of any profile).

### Still to decide (ranked)
1. **The voice room (the hardest part for every mod).** Today Koetama makes a room and the game must hand the same one
   to every player through its own networking. That fails without mod networking (MH Rise/World, Elden Ring Seamless,
   Among Us on official servers), with no client scripts (TTS) and is awkward on dedicated servers (Valheim needed an
   "earliest claim" fallback). Proposal: also accept **`room_seed`**, any string every player of the session shares
   (lobby id + a passphrase, the server's address and world, the Seamless password), from which each Koetama derives
   the same room and key. A game with mod networking keeps the secure `room`/`key` path; a seed made only of public
   facts would let anyone who knows them listen, so the docs must say to mix in something private.
2. **Player ids.** Accept any id string (Steam ids, 64-bit ids, names) as `me`, speaker `id` and `to`; Koetama maps them
   to the relay's 16-bit ids and the relay refuses a duplicate with a message the game sees (today the second
   connection silently replaces the first).
3. **Positions instead of angles (optional).** Let a feed give `listener: {position, forward, up, right}` and each
   speaker's `position` (any units, any handedness: the three vectors settle it) plus the mode's `range`; Koetama
   computes azimuth, elevation and distance gain and smooths between feeds. Every mod so far re-implemented this, and
   each PC must agree on ranges. Keep `azimuth`/`elevation`/`gain` for games that already compute them.
4. **Who is talking.** A `talking` object (player id, start/stop, maybe level) for other players' voices and the
   local one, so games can show speaking icons; and whether each player is in the voice room at all ("this friend has
   no Koetama").
5. **Readiness.** A `status` object (speech model loading/ready, microphone open/none) so a game can say "Koetama
   ready" - lines spoken in the first seconds are lost today without a hint.
6. **Host-only and server-scripted games** (TTS, FiveM, DST server mods, Factorio's host): one script knows every
   player, but only the host's Koetama hears from it. A hub mode - the host's Koetama sends each paired player's
   Koetama its own feed through the relay - would cover them. Large; only worth it for a game we want.
7. **Translation answers** could say which translation applied (`from`, `to`), so a game can label them.
8. **Compatibility transports** worth considering later: a **Mumble Link reader** (shared memory many mods already
   write: instant positional voice for them, no Koetama mod), a **UDP sender** for Factorio's inbound side,
   **WebSocket** if FiveM's NUI or a browser cannot use HTTP to localhost.
9. Small: an optional `name` per speaker (Koetama's window shows "player 7"), define `muffle`'s scale, the command
   line's Teardown wording ("a level with the ... mod").

### Not worth changing
- Three transports is enough: every surveyed game with a way out fits one of them (UDP-only Factorio aside).
- `to_translate` riding in the feed is slightly odd over the socket, but one way to ask for translations on every
  transport is simpler than two.
