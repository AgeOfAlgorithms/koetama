# Which games Koetama should target next: a ranked survey

*2026-10-12; updated 2026-10-13: the criterion "what a mod alone could do", one point less integration fit where
players install mods by hand, the Valheim, DST and Project Zomboid findings, Big Walk, and links to every mod named.
Replaces the 2026-10-09 survey (transport per game, five demos, the API assessment; its demos and transport
notes are kept, condensed, in "Kept from the previous survey"). About 60 games and tools looked at. Player numbers are
Steam concurrents: SteamCharts pages and the Steam Web API (`GetNumberOfCurrentPlayers`), snapshots taken 2026-10-12/13
UTC for the sandbox, co-op and RPG games; the mil-sim, roleplay, social-VR and tabletop figures are third-party
trackers from **Jun-Sep 2026** (no October numbers found). SteamDB could not be fetched. Mod counts: Thunderstore's
package API (downloads summed over versions), the Modrinth API, Steam's `GetPublishedFileDetails`, GitHub stars; Nexus
returned 403, so co-op RPG mods have few counts. International audience = the language of Steam reviews (appreviews
API), a proxy that under-counts Chinese players. Steam Deck = Steam's compatibility endpoint (3 Verified, 2 Playable,
1 Unsupported). "[i]" = an inference, not verified. Koetama's capabilities as in PROTOCOL.md (protocol 2) and
PROJECT.md.*

## Summary

**Build next, in this order** (scores out of 70; the same five games since the first ranking):
- **Don't Starve Together** (61; 24k Sep average; the most international audience at 24 % English; no voice). **The
  user's pick, started 2026-10-13:** a DST Proximity Comms (hand-off: [handoff-dst-proximity-comms.md](handoff-dst-proximity-comms.md)).
  Its Lua sandbox can carry no audio and run no speech to text, so no mod-only rival can exist. Klei explicitly
  re-allowed mods to call localhost in Feb 2025, so the HTTP transport is a sanctioned route. Joiners download the mod
  automatically. The one proximity text mod, [Proximity Chat](https://steamcommunity.com/sharedfiles/filedetails/?id=2531534743),
  is minimal and has 306 subscribers: text alone does not pull DST players, so lead with voice, captions and translation.
- **Project Zomboid** (59; 53k now, all-time peak in Aug 2026 after Build 42 multiplayer; voice and radios built in).
  Speech and translation only, beside the game's voice: captions and spoken lines that keep its ranges, translation for
  its EN/ES/RU/PT servers. Kahlua could do none of it alone. Not voice: the game has working proximity voice, and
  [Enhanced Proximity Voice Chat](https://steamcommunity.com/sharedfiles/filedetails/?id=3799877828), which adds
  Koetama-like modes, walls and radios to it, has 246 subscribers.
- **Stardew Valley** (57; 43k now, 49k Sep average; no voice; Deck Verified; 21 % Chinese reviews). A SMAPI mod with
  voice by location, speech to chat for pad players, and translation. No Workshop: every player installs SMAPI, the mod
  and Koetama by hand.
- **PEAK** (57; 21k Sep average; Photon proximity voice built in, but no text chat). A speech-and-translate-only mod
  (`uses: ["speech","translate"]`): spoken lines become text, teammates' speech gets captions, chat is translated offline.
- **Valheim** (53; 100,650 playing now, 1.0 on 2026-09-09; no built-in voice; the BepInEx demo already exists). Four
  mod-only voice mods appeared in 2026, one over Steam P2P (a good path), so Koetama's mod should lead with what a mod
  can't carry (speech to chat, live captions, offline translation) and leave voice to another mod when one is installed.

**Lesson from Teardown.** The barrier is "install a separate program", not the code.
[TeardownVC](https://steamcommunity.com/sharedfiles/filedetails/?id=3653434619) (voice through an exe) has 2,344
current subscribers; the top text-chat mod, [Easy Chat](https://steamcommunity.com/sharedfiles/filedetails/?id=3621818416),
has 136,043, and the user's [Proximity Comms](https://steamcommunity.com/sharedfiles/filedetails/?id=3812301496) 108.
Other external-app voice mods are small too: [Factorio](https://mods.factorio.com/mod/proximity-voice-chat) 186 users,
[Satisfactory](https://www.mado-hosting.com/satisfactory-mods/ProxVoiceChat) 707 downloads. And where a mod alone can
carry voice, a mod-only rival wins on friction ("How candidates are scored": M). Every Koetama mod should therefore be useful without
Koetama (text chat, bubbles, the quick-chat wheel), with Koetama as the upgrade.

**Must a game be moddable?** For **positional voice**, almost always yes. Koetama has to know where every player is and
which session they share. Only four routes give it that:
- a **mod**;
- an **official API**: VRChat OSC (chatbox input only, no positions), the ETS2/ATS telemetry SDK (only your own
  position), MSFS SimConnect;
- **Mumble Link** shared memory: 27 games write it natively (mostly Source-era shooters, ArmA, Guild Wars 2), plus
  7 Days to Die since Alpha 21 ([Mumble](https://www.mumble.info/documentation/user/positional-audio/supported-games/));
- **memory reading**: BetterCrewLink and Proximity Core do this, and it carries an anti-cheat ban risk.

Telemetry and Mumble Link give each PC only its **own** position. Koetama would have to carry each sender's position in
its voice packets to use them, which protocol 2 does not do [i]. **Captions, translation and speech-to-chat need
less.** Koetama's own window can caption and translate what the player says in any game. VRChat's OSC chatbox is an
official speech-to-chat route. Typing into a game's chat by simulated keys works but is fragile and risky under
anti-cheat. Other players' speech still needs either a shared channel or their own Koetama.

**Do some games forbid mods that link to external programs?** Yes, but few, and the rule is usually about the file,
not about a companion app:
- **tModLoader's Workshop** is the only explicit ban found: "Do not launch external programs", "Do not instruct users
  to download external software". Exemptions are granted on request.
- **Garry's Mod's Workshop** bans addons "that require user to download something from description".
- **Nexus, Modrinth, CurseForge and Thunderstore** police the uploaded file: bundled executables that phone home,
  off-site download links, undisclosed uploads, self-updating, obfuscation. None of them bans a mod that talks to an
  optional local app.
- **Klei (Don't Starve Together)** went the other way and explicitly allows localhost.

So a Koetama mod should ship no binaries, never launch or download Koetama, link to the project page, disclose the
localhost connection and the voice relay, and work without Koetama. Details are in "Rules on mods that use external
programs".

## What changed in Koetama since the last survey

All five gaps found on 2026-10-09 were closed in protocol 2. The Teardown mod gained controller support. Each change
widens the field:

| Feature | What it does | Kinds of games it opens |
|---|---|---|
| **Hub** (`uses: "hosted"`, join codes) | The host's script lists every player; each player types an 8-character code into their own Koetama | Host-only scripts (Tabletop Simulator); server-only plugins (Rust Oxide/Carbon, Unturned OpenMod, ARK server API, Bedrock BDS scripts, FiveM server resources); games where joiners won't install mods |
| **`room_seed`** | Any shared string becomes the room and key (scrypt), with no mod networking | Games without mod networking: MH Rise/World (lobby + passphrase), Elden Ring Seamless Co-op (its password), Valheim dedicated servers (address + world + password), Among Us (lobby code + a private part) |
| **String player ids** | Steam ids or names; a clash is reported as `id_taken` | Every game with 64-bit ids |
| **Positions and ranges** | `listener` plus speakers' `position` and `range`; Koetama does the direction, loudness and `to` | Lua sandboxes (DST, Zomboid, Stormworks, TTS): no vector maths in the script, and the same ranges on every PC |
| **Devices** (`transmit`, `via`: radio, loudspeaker, PA) | A voice also comes out of a walkie, a megaphone or PA speakers, or as an item-less "radio chat" | Games with radio items or modes, or demand for them: Zomboid, Lethal Company walkies, R.E.P.O. walkies, PEAK conches, Schedule I phones, Stormworks, RP servers |
| **Translation set once in Koetama** | The player picks languages and a target in Koetama; the game only sends lines | Any game with text chat: a mod needs no translation UI. Multilingual lobbies: DST, R.E.P.O., Zomboid, PEAK |
| **`talking`, `status`, `voice.players`** | Speaking icons, "Koetama ready", who in the session has Koetama | Games without voice UI of their own |
| **Per-player `volume`** (0..2) | One player's voice louder or quieter, for this listener only | Any game; no per-mod mixing |
| **`max_range`** (2026-10-12) | The farthest a voice can reach in the game; a range another player's Koetama announces beyond it is ignored (a changed Koetama can no longer be heard across the map) | Every game that gives positions without gains (Valheim, Stardew, DST): give it |
| **`uses`** (`speech` / `translate` / `voices`) | A profile can skip voices and only do speech to text or translation | Games with good built-in voice (PEAK, Zomboid, Rust, FiveM, Minecraft with Simple Voice Chat) |
| **Controller support** (in the Teardown mod) | L3 twice opens the quick-chat wheel; a controller player's mic is always on, so they speak instead of type | Pad-first and Deck games: Stardew, Valheim (1.0 has console crossplay), PEAK, DST, Core Keeper, Elden Ring, Monster Hunter |

The controller audience is real. Valve's 2024 figures: 48M players use controllers on Steam, and controller sessions
rose from 5 % of daily sessions (2018) to a 15 % peak (2024) ([PC Gamer](https://www.pcgamer.com/48-million-players-use-controllers-on-steam);
[GIGAZINE](https://gigazine.net/gsc_news/en/20240704-steam-gamer-controller-usage)). No newer figure was found.

Protocol 2 has been tested in Rust and Python suites and over the live relay, but **not yet in a real game** other
than Teardown (PROJECT.md).

## How candidates are scored

Each criterion is scored 0-5, then weighted. The maximum is 70.

| Criterion | Weight | 5 means | 0 means |
|---|---|---|---|
| **P: players now and trend** | ×4 | ≥50k average concurrent, stable or rising | <500 |
| **F: integration fit** | ×2 | Real code reaches a socket, the game has mod networking or a seed, and joiners get the mod automatically (Workshop sync, server push); one point less when each player installs it by hand (Thunderstore, Nexus) | No route to a local app |
| **G: gap left by built-in voice and existing mods** | ×2 | No voice, no STT or translation mods | Voice, STT and translation all well served |
| **R: policy and anti-cheat risk** | ×1 | Sanctioned or no rule; no anti-cheat | Mods banned, or ban risk for users |
| **C: PC share** | ×1 | PC only, or the modded slice is PC only | Mostly mobile or console |
| **K: Koetama-specific value** | ×1 | Pad or Deck play, a multilingual audience, radios or voice commands all apply | None apply |
| **E: effort** (inverse) | ×1 | A demo exists or the transport is already proven | New transport plus reverse engineering |
| **M: what a mod alone could do** | ×2 | A mod can't carry audio or run speech to text there (a script sandbox, no mod networking, server-only plugins) | A mod-only rival already does the job well (its own voice path, in-mod STT) |

**Why M (added 2026-10-13).** A player asked to install a mod *and* Koetama will pick a rival that needs only the mod,
if it does the same job. Valheim shows the case: four mods carry voice with no relay and no app, among them
[ProximityVoice](https://thunderstore.io/c/valheim/p/Nubedeu/ProximityVoice/) through the game's own messages
(`ZRoutedRpc`) and [ValheimVoiceChat](https://thunderstore.io/c/valheim/p/BORTY/ValheimVoiceChat/) over Steam P2P. So M asks what Koetama would lead with in that game, and whether a mod could
do it alone, and how well:
- **Voice:** whether the game's own networking can carry it, and how well. A good path (Steam P2P unreliable messages,
  a mod's own UDP server like Simple Voice Chat's, Photon) scores low. A path that technically works but is poor for
  audio scores in the middle. Poor here means it goes through the host's upload, is reliable and ordered (a lost
  packet stalls the stream), shares a send cap with world sync, or is tied to the game's tick. Valheim's routed RPCs
  are all of these (but its mods can also reach Steam P2P, so Valheim scores 2): vanilla caps each connection at
  roughly 60-64 KB/s, and heavy servers run
  [BetterNetworking](https://thunderstore.io/c/valheim/p/CW_Jesse/BetterNetworking_Valheim/) to raise it
  ([Supercraft](https://supercraft.host/wiki/valheim/better-networking)). Not measured with voice [i]. No path at all
  (a Lua sandbox, a closed co-op mod without networking) scores 5: there Koetama's relay is the only way.
- **Speech to text and translation:** the models (0.2-0.7 GB per language, plus a native runtime) are too big to ship
  inside a mod, so M is high unless an in-mod STT already serves the game (Lethal Company and Content Warning's Windows
  speech mods).

Koetama's relay is not better than every game path: it is a WebSocket (TCP) through Cloudflare's edge, with a 60-300 ms
jitter buffer and Opus loss concealment. Against Steam's unreliable P2P relays it has no quality edge [i]. Its edge is
over host-relayed, reliable or throttled game messages, and over no path at all.

How P is set:
- **Bands** (by Steam monthly average): 5 ≥50k; 4 for 20-50k; 3 for 8-20k; 2 for 3-8k; 1 for 0.5-3k; 0 below that.
- **Trend**: one point off for a sharp decline or a decaying launch spike.
- **Modded slice**: one point off when only a modded or multiplayer slice can be reached (Skyrim Together, Seamless
  Co-op, BG3 Script Extender).

The scores are judgements on the cited facts, so they can be redone when the numbers change.

## The ranking

Sorted by score. "Now" is the snapshot, "avg" the Sep-2026 monthly average (mid-2026 where marked). Score: the eight criterion scores P/F/G/R/C/K/E/M, then the total P×4 + F×2 + G×2 + R + C + K + E + M×2. Ties go
to more players now (P), then less effort (E), then the bigger gap (G).

| # | Game | Now / avg (trend) | Mod route, transport | Built-in voice | Existing mods/tools doing Koetama's job | Policy note | What a mod alone could do (M) | Score |
|---|---|---|---|---|---|---|---|---|
| 1 | **Don't Starve Together** | 11,901 / 24,183 (24h peak 48,120) | Lua 5.1, HTTP to localhost; mod RPC | none; a chat line shows over the speaker's head; whisper reaches nearby players | [Proximity Chat](https://steamcommunity.com/sharedfiles/filedetails/?id=2531534743) 306 (overhead text near players only; 2021, never updated); [Chat Binds](https://steamcommunity.com/sharedfiles/filedetails/?id=921637415) 1,163; [Chat Machine Translation](https://koale.eu/chat) (cloud + companion exe) | Klei allows localhost (hotfix 653007) | Lua sandbox: no audio, no STT | 4/4/5/5/4/5/3/5 = **61** |
| 2 | **Project Zomboid** | 52,971 / 54,454 (all-time peak Aug 2026) | Kahlua, files; `sendClientCommand` | proximity + radios; `/say`, `/yell` ranges with text over heads | [Enhanced Proximity Voice Chat](https://steamcommunity.com/sharedfiles/filedetails/?id=3799877828) 246 (improves the built-in voice; a Java agent on every PC); [TICS](https://steamcommunity.com/sharedfiles/filedetails/?id=3366438318) 29,692 (ranged RP chat, B41 tag); [buffy's roleplay chat](https://steamcommunity.com/sharedfiles/filedetails/?id=2688851521) 43,589 (B41, end of support); no STT or translation | Workshop; none found | Kahlua: no audio, no STT | 5/4/2/4/5/4/4/5 = **59** |
| 3 | **Stardew Valley** | 42,994 / 48,648 (slow decline) | SMAPI C#, socket; `Helper.Multiplayer` | none | [Location Voice Chat](https://www.nexusmods.com/stardewvalley/mods/22550) (Discord bot, zones; count not gathered) | Nexus: disclose requirement | SMAPI messages through the host [i]; no mod-only voice yet | 4/4/5/5/4/4/4/3 = **57** |
| 4 | **PEAK** | 11,352 / 21,022 (stable, update spikes) | BepInEx C#, socket; Photon properties | Photon proximity; no text chat | [PeakTextChat](https://thunderstore.io/c/peak/p/borealityy/PeakTextChat/) 176,600; [VoiceCurse](https://thunderstore.io/c/peak/p/Ryocery/VoiceCurse/) 10,679; [VoiceBox](https://thunderstore.io/c/peak/p/Tim_Shaw/VoiceBox/) 53,719 (cloud); [PeakChatTranslator](https://github.com/gustavogordoni/PeakChatTranslator) 454 (Google) | Thunderstore | Photon voice built in; STT and translation models too big for a mod | 4/4/3/5/5/5/4/4 = **57** |
| 5 | **Valheim** | 100,650 / 110,174 (1.0 surge; pre-1.0 ~16-21k) | BepInEx C#, socket; ZRoutedRpc or seed | none | [ProximityVoiceChat](https://thunderstore.io/c/valheim/p/Azumatt/ProximityVoiceChat/) 4,731 (no external program); [ProximityVoice](https://thunderstore.io/c/valheim/p/Nubedeu/ProximityVoice/) 2,522; BreakoutMods-VOIP 1,209; [ValheimVoiceChat](https://thunderstore.io/c/valheim/p/BORTY/ValheimVoiceChat/) 96 (Steam P2P); no STT or translation | Thunderstore rules; no dev statement | a good path exists: ValheimVoiceChat sends voice over Steam P2P; ProximityVoice uses ZRoutedRpc (through the host, reliable, sharing the ~60 KB/s per-peer cap with world sync) [i] | 4/4/4/4/4/4/5/2 = **53** |
| 6 | **Minecraft Java** | not on Steam; ~212M MAU (aggregators) | Fabric/NeoForge, socket; speech + translate only | none (mods) | [Simple Voice Chat](https://modrinth.com/plugin/simple-voice-chat) 73.8M; [Plasmo Voice](https://modrinth.com/mod/plasmo-voice) 4.08M; [PlasmoWords](https://modrinth.com/mod/plasmowords) 117; [Slip of a Tongue](https://modrinth.com/mod/slip-of-a-tongue) 124; cloud translators | Modrinth 1.11: disclose uploads | SVC's own UDP voice is excellent; in-mod STT exists (Vosk) but is barely used | 5/4/2/4/4/3/3/3 = **52** |
| 7 | **Terraria (tModLoader)** | tML 13,195 / 21,158 (Terraria 37k) | tML C#, socket; ModPacket; mods sync on join | none | none found | **tML Workshop bans external programs**; exemption needed | ModPackets through the server [i]; no mod-only voice yet | 3/5/5/1/5/4/4/3 = **52** |
| 8 | **Rust** (Oxide/Carbon) | 61,396 / 81,168 | server-only C# plugin, hub | proximity + megaphone/phone props | none found for captions/translation | Facepunch: UI plugins list as "Modded"; EAC on clients | server-only plugins, EAC on clients: no client STT | 5/2/2/3/5/3/2/5 = **51** |
| 9 | **R.E.P.O.** | 14,860 / 16,858 (down from May) | BepInEx C#, socket; Photon | proximity | [WalkieTalkies](https://thunderstore.io/c/repo/p/Omniscye/WalkieTalkies/) 347,751; [Speech2TTS](https://thunderstore.io/c/repo/p/NocturnalLyfe/Speech2TTS/) 2,974; [DeadTTS](https://thunderstore.io/c/repo/p/flipf17/DeadTTS/) 4.2M (chat to speech) | Thunderstore | Photon voice built in; in-mod STT exists (Vosk, 2,974) | 3/4/3/5/5/4/4/3 = **50** |
| 10 | **FiveM** (GTA V RP) | ~126k avg, Jun 2026 (peak 221,062, May) | server + client Lua/JS, NUI; hub or client resource | pma-voice (built-in Mumble) | [pma-voice](https://github.com/AvarianKnight/pma-voice), SaltyChat, [TokoVOIP](https://github.com/Itokoyamato/TokoVOIP_TS3) (radios, phones); no captions found | Cfx PLA §5.2: Rockstar may ban "unauthorized service providers" | Mumble voice built in; client Lua/JS cannot run STT [i] | 5/3/2/2/5/2/2/4 = **49** |
| 11 | **Garry's Mod** | ~21-23k avg (Jul 2026) | GLua `http.Fetch`, HTTP; binary modules | engine voice | none found | Workshop bans "download something from description" | engine voice built in; GLua cannot run STT | 4/4/2/3/5/2/3/4 = **49** |
| 12 | **Baldur's Gate 3** | 44,331 / 42.9k (multiplayer slice smaller) | Script Extender Lua, files; Nexus only, not synced | none [i] | none found | Larian syncs only mod.io mods | Script Extender Lua: no audio, no STT | 3/3/4/4/4/3/2/5 = **49** |
| 13 | **VRChat** (OSC, no mod) | ~100k daily avg (record 158,192, Feb 2026) | OSC chatbox (UDP); Koetama has no OSC connector | proximity | [TTS Voice Wizard](https://github.com/VRCWizard/TTS-Voice-Wizard) (810★), [VRCOSC](https://github.com/VolcanicArts/VRCOSC) (573★), [Whispering Tiger](https://github.com/Sharrnah/whispering) (549★), [VRCT](https://github.com/misyaguziya/VRCT) (448★), TeaTalk, Booth tools | OSC official; EAC blocks modded clients | rivals are companion apps too (local Whisper, cloud) | 5/3/1/5/3/5/3/2 = **48** |
| 14 | **Satisfactory** | 14,237 / 13,132 (declining) | SML C++, socket | not verified | [ProxVoiceChat](https://www.mado-hosting.com/satisfactory-mods/ProxVoiceChat) 707 (proximity + radio) | ficsit.app; none found | UE replication (unreliable RPCs) [i]; ProxVoiceChat is mod-only | 3/4/4/5/5/3/3/2 = **48** |
| 15 | **Tabletop Simulator** | ~5-6k avg (Jun-Jul 2026) | host-only Lua, HTTP; hub | Steam voice, global | none found | none found | host-only Lua: nothing runs on the other PCs | 2/4/3/5/5/2/4/5 = **48** |
| 16 | **Schedule I** | 17,361 / 8,998 (declining) | MelonLoader, IL2CPP (Mono on a beta branch), socket | none | none ([TextYourFriends](https://thunderstore.io/c/schedule-i/p/ifBars/TextYourFriends_Il2Cpp/) ~3.6k, texting) | none found | mod code can reach Steam P2P directly [i] | 3/3/5/4/5/3/3/2 = **47** |
| 17 | **Factorio** | 12,399 / 15,580 (flat) | Lua, files out + UDP in (`--enable-lua-udp`) | none | [Proximity Voice Chat](https://mods.factorio.com/mod/proximity-voice-chat) via Mumble, 186 users | No network by design | lockstep Lua: no audio | 3/2/4/4/5/2/2/5 = **47** |
| 18 | **Elden Ring** (Seamless Co-op) | ER 21,568 / 30.8k; Seamless slice unknown | closed-source mod; separate DLL, seed from the password | none reliable | none found | EAC off; separate saves | no mod networking: a mod-only voice would need its own server | 3/2/4/3/5/4/1/5 = **47** |
| 19 | **Stormworks** | 1,206 / 1,327 (flat) | Lua `server.httpGet` to localhost only, HTTP | radio voice, no proximity | Workshop radio vehicles | localhost API made for companion apps | Lua sandbox: no audio, no STT | 1/4/4/5/5/3/4/5 = **47** |
| 20 | **Palworld** | 35,278 / 77,110 (1.0 spike decaying) | UE4SS, mostly client; Workshop loader | proximity since 1.0 | none found | anti-cheat not verified; Xbox/PS5/Game Pass | voice built in; UE4SS Lua cannot run STT [i] | 4/3/2/3/3/4/2/4 = **46** |
| 21 | **Monster Hunter** World / Rise / Wilds | World 8,949 / 14.4k (rising); Rise 6.7k; Wilds 25.7k (spiky) | REFramework Lua, files (Rise demo); `room_seed` | lobby voice | none found | Capcom: vague ban warning (leaderboards) | lobby voice built in; REFramework Lua cannot run STT | 3/3/3/3/3/4/3/4 = **45** |
| 22 | **7 Days to Die** | 17,995 / 23,591 (stable) | DLL mods with EAC off, socket; Mumble Link built in | proximity (old sources) | none found | EAC must be off | voice built in; C# mods could bundle STT [i] | 4/3/2/2/4/3/3/3 = **44** |
| 23 | **Core Keeper** | 9,031 / 4,466 | official Unity SDK via mod.io; "limits access to system functions" | none [i] | none found | sandbox may block sockets | sandboxed SDK [i] | 2/2/5/4/4/4/2/4 = **44** |
| 24 | **Skyrim Together Reborn** | Skyrim SE 29,954 / 23.2k (+23 % y/y); STR slice unknown | SKSE plugin, socket; seed from the server address | none (README) | [Mantella](https://github.com/art-from-the-machine/Mantella), [CHIM](https://github.com/abeiro/HerikaServer) (AI NPCs, they hold the mic) | Nexus | no scripting API yet: a mod-only voice needs its own server [i] | 2/3/4/4/5/3/2/4 = **44** |
| 25 | **Content Warning** | 1,113 / 733 | BepInEx / Workshop, socket | proximity; no text chat | [VoiceRecognitionAPI](https://thunderstore.io/c/content-warning/p/Notest/VoiceRecognitionAPI/) 152,420 (Windows speech, 2024); [SimpleChat](https://thunderstore.io/c/content-warning/p/maysunrise/SimpleChat/) 163,033 | Landfall supports mods | mod-only STT exists (Windows speech, 152,420) | 1/5/3/5/5/4/4/2 = **42** |
| 26 | **Big Walk** | 5,010 / 6,044 (launch 2026-08-04, peak 46,224; decaying) | BepInEx 6 IL2CPP, socket; mods sync between modded players (BigChoppa) | proximity voice + text chat with the same limits (walls, distance, walkies); crossplay with PS5 / Switch 2 | none for voice, STT or translation ([Thunderstore](https://thunderstore.io/c/big-walk/): 97 packages) | FAQ: talking outside the game is "NOT ALLOWED" (design rule): speech and translation only, through its text chat | built-in voice; no in-mod STT yet | 1/4/3/4/3/4/3/4 = **40** |
| 27 | **Lethal Company** | 3,888 / 2,654 (declining) | BepInEx C#, socket; LethalNetworkAPI | Dissonance proximity + walkies | [ReservedWalkieSlot](https://thunderstore.io/c/lethal-company/p/FlipMods/ReservedWalkieSlot/) 12.4M; [VoiceRecognitionAPI](https://thunderstore.io/c/lethal-company/p/loaforc/VoiceRecognitionAPI/) 144,238; [LC_VEGA](https://thunderstore.io/c/lethal-company/p/JS03/LC_VEGA/) 115,109; [SpeechRecognitionAPI](https://thunderstore.io/c/lethal-company/p/JS03/SpeechRecognitionAPI/) 19,820 (Vosk) | Thunderstore | mod-only STT exists (Windows speech 144,238; Vosk 19,820) | 1/4/3/5/5/3/4/2 = **39** |
| 28 | **Arma Reforger** | ~10.7k avg, Jun 2026 (169k all platforms, 5 Jan) | Enforce script REST (GET/POST; localhost unverified) | VoN + radios | radio mods on the native VoN | Workshop; console crossplay | VoN radios built in; Enforce script cannot run STT | 3/2/2/3/2/3/2/4 = **38** |
| 29 | **Vintage Story** | no Steam data | C# Universal mods (pushed on join), socket | none | [RP Voice Chat](https://mods.vintagestory.at/rpvoicechat) ~33,687 | ModDB | RP Voice Chat works as a mod (~33,687) | 1/5/3/5/5/2/4/1 = **38** |
| 30 | **Among Us** (Steam) | 10,098 / 6,769 (most players on mobile) | BepInEx 6 IL2CPP; seed from the lobby code | none | [BetterCrewLink](https://github.com/OhMyGuus/BetterCrewLink) (memory reading, 1.1k★) | Innersloth: register mods on official servers | BetterCrewLink (a companion app with its own servers) | 2/3/3/3/1/3/3/3 = **36** |
| 31 | **Arma 3** | ~7.2k avg, Jul 2026 | `callExtension` DLL | VON | [TFAR](https://github.com/michail-nikolaev/task-force-arma-3-radio) (320★), [ACRE2](https://github.com/IDI-Systems/acre2) (215★), [GameVox GameLink](https://gamevox.com/integrations/arma3) | BattlEye blocks non-whitelisted DLLs | TFAR and ACRE2 (TeamSpeak companions) entrenched | 2/3/1/2/5/3/2/2 = **32** |
| 32 | **Phasmophobia** | 13,200 / 8,758 | MelonLoader, unofficial | proximity + Vosk voice recognition | — | "No mods"; own anti-cheat | built-in voice recognition | 3/1/2/0/4/3/1/2 = **30** |

Sources for the rows:
- **Players**: [SteamCharts](https://steamcharts.com/app/892970) (each game's app page); [Steambase](https://steambase.io/apps/286160);
  [Raijin](https://raijin.gg/app/1874880/Arma_Reforger); FiveM [SteamCharts](https://steamcharts.com/app/2676230); VRChat
  [Road to VR](https://roadtovr.com/vrchat-key-stats-japan-growth-concurrents/); Minecraft [searchlogistics](https://www.searchlogistics.com/learn/statistics/minecraft-user-statistics/);
  Valheim 1.0 [PCGH](https://www.pcgameshardware.de/Valheim-Spiel-73202/News/1-0-Update-Deep-North-1553323/); Zomboid B42
  [Shockbyte](https://shockbyte.com/blog/project-zomboid-build-42-multiplayer).
- **Mod counts**: Thunderstore package pages, e.g. [ProximityVoice](https://thunderstore.io/c/valheim/p/Nubedeu/ProximityVoice/),
  [PeakTextChat](https://thunderstore.io/c/peak/p/borealityy/PeakTextChat/),
  [PeakChatTranslator](https://github.com/gustavogordoni/PeakChatTranslator),
  [WalkieTalkies](https://thunderstore.io/c/repo/p/Omniscye/WalkieTalkies/),
  [Speech2TTS](https://thunderstore.io/c/repo/p/NocturnalLyfe/Speech2TTS/),
  [ReservedWalkieSlot](https://thunderstore.io/c/lethal-company/p/FlipMods/ReservedWalkieSlot/),
  [SpeechRecognitionAPI](https://thunderstore.io/c/lethal-company/p/JS03/SpeechRecognitionAPI/).
- **Other mods and tools**: Modrinth [Simple Voice Chat](https://api.modrinth.com/v2/project/simple-voice-chat) and
  [PlasmoWords](https://modrinth.com/mod/plasmowords); Stardew [Location Voice Chat](https://www.nexusmods.com/stardewvalley/mods/22550);
  [Chat Machine Translation](https://koale.eu/chat); Factorio [Proximity Voice Chat](https://mods.factorio.com/mod/proximity-voice-chat);
  Satisfactory [ProxVoiceChat](https://www.mado-hosting.com/satisfactory-mods/ProxVoiceChat);
  [RP Voice Chat](https://mods.vintagestory.at/rpvoicechat); [Mantella](https://github.com/art-from-the-machine/Mantella);
  VRChat [TTS Voice Wizard](https://github.com/VRCWizard/TTS-Voice-Wizard) and [VRCT](https://github.com/misyaguziya/VRCT).
- **Built-in voice**: Zomboid [guildorder](https://guildorder.com/games/project_zomboid/wiki/voice-radio-and-chat-settings);
  Palworld [MintServers](https://mintservers.com/knowledgebase/palworld-server-voice-chat); PEAK
  [RejoinVoiceFix](https://thunderstore.io/c/peak/p/PatchNote/RejoinVoiceFix/); Phasmophobia
  [Kinetic Games](https://kineticgames.co.uk/blog/phasmophobia-voice-recognition-update).

**Also looked at, all below 30 or ruled out:** Sons of the Forest, Raft, Subnautica 1/2, Escape the Backrooms,
Supermarket Together, ARK: Survival Ascended, Conan Exiles, DayZ, Unturned, Barotrauma, Space Engineers, Deep Rock
Galactic, Ready or Not, Cyberpunk 2077, FFXIV, Gorilla Tag, MSFS 2024, ETS2, s&box, Resonite. These either have
built-in voice plus anti-cheat (DayZ, Conan: BattlEye on both sides), console-shared mods (ARK DevKit mods must run on
Xbox and PS5), no official mod support (Sons of the Forest, Subnautica 2), or a ToS against all tools (FFXIV).

### Build next

**1. Don't Starve Together (61). The user's pick (2026-10-13); hand-off for the agent building it:
[handoff-dst-proximity-comms.md](handoff-dst-proximity-comms.md).** It has the most mixed-language audience: 24 %
English, 31 % Simplified Chinese, 19 % Russian. It has no voice. The existing translator,
[Chat Machine Translation](https://koale.eu/chat), needs a cloud API plus a 14 MB companion exe since Klei's 2025
lockdown. Klei's hotfix 653007 (2025-02-05) states that `TheSim:QueryServer` "may now query to localhost or 127.0.0.1"
([Klei](https://kleiforums.com/game-updates/dst/653007-r2465/)). That is exactly Koetama's HTTP transport.

What the game's own scripts show (read from its `scripts.zip`, 2026-10-13; details and line numbers in the hand-off):
- **The base chat is half proximity already.** A chat line shows over the speaker's head as well as in the chat log
  (`Networking_Say` calls `talker:Say`), and a whisper mode reaches only nearby players. No Speak / Yell ranges, no
  fading at the edges, no per-player history.
- **It can be replaced.** The input screen and chat log are Lua widgets, and `Networking_Say` is a global a mod can
  wrap. Real ranges go through the mod's own RPCs: the server sends each line only to the players in range, as
  Proximity Comms does in Teardown.
- **Babble comes free:** a talking character plays its own voice sound from where it stands.
- **The word filter runs on the receiving PC** (`ApplyLocalWordFilter`): a mod's own lines must apply it themselves.
- **Joiners get the mod automatically** (`all_clients_require_mod`): the only extra install is Koetama.
- **The world is two shards** with caves (the surface and the caves are separate server processes).

**Prior mods.** [Proximity Chat](https://steamcommunity.com/sharedfiles/filedetails/?id=2531534743) (Peterwander,
2021, never updated) hides chat from players who aren't near and shows it over heads only. It has no modes or settings
and conflicts with the Status Announcements mod. It has 306 subscribers (499 lifetime, 24 favourites) after five years.
[Chat Binds](https://steamcommunity.com/sharedfiles/filedetails/?id=921637415) (1,163) is a quick-chat key mod with a
whisper tag. So the idea is not taken, but proximity text alone does not pull DST players: lead with what DST has never
had (voice, captions, offline translation), with proximity text as the part that works without Koetama.

Effort is higher than for a C# game: Lua 5.1 HTTP polling (whether `QueryServer` sends `Content-Length` with a POST,
and how often it can be called, are the hand-off's first check), and server and client mod halves.

*First mod*: a DST Proximity Comms. Proximity text chat in its own design (Whisper / Speak / Yell / Global), then
Koetama: speech to chat (pad and Deck players: Verified), live captions, offline translation, proximity voice from
positions.

**2. Project Zomboid (59).** It is the fastest-growing big game in the set: the August 2026 average was 76,861, and
Build 42 multiplayer went stable on 2026-07-29 (one source). Its audience is 43 % English, 14 % Russian, 13 % Spanish,
8 % Portuguese. Kahlua can only write files under `~/Zomboid/Lua`, which is the same pattern as Teardown's files
transport. Server mods download on join.
- **Voice is not the gap.** The game has directional proximity voice (10-100 tiles) and radios on frequencies
  ([guildorder](https://guildorder.com/games/project_zomboid/wiki/voice-radio-and-chat-settings)). No sign was found
  that Build 42's voice is broken: the guides' problems are setup (devices, push to talk, the server switch), though
  "many groups use Discord for reliability" ([pzfans](https://pzfans.com/ProjectZomboidB42Voice/),
  [low.ms](https://low.ms/blog/project-zomboid-build-42-multiplayer-survival-guide)).
- **The voice mod there improves the game's own voice.**
  [Enhanced Proximity Voice Chat](https://steamcommunity.com/sharedfiles/filedetails/?id=3799877828) (2026-09-11,
  Build 42) adds whisper / talk / shout modes (3 / 15 / 40 tiles), muffling through walls, doors, windows and floors,
  walkie-talkies and HAM radios that break up with distance, and a craftable broadcast station. It needs the
  ZombieBuddy mod's Java agent on every player's game, so it has install friction like Koetama's. It has 246
  subscribers (342 lifetime, 26 favourites). That is Koetama's voice feature set, and few want it: don't compete on
  voice here.
- **Ranged text chat exists too.** `/say` and `/yell` carry different distances, lines show over heads, and the chat UI
  (`ISChat`) is Lua. [TICS: Total Immersive Chat System](https://steamcommunity.com/sharedfiles/filedetails/?id=3366438318)
  (29,692 subscribers, 75,007 lifetime; tagged Build 41, last updated 2025-01-09) rebuilt chat and radio with whisper,
  low, say and yell ranges for roleplay servers. It succeeds
  [buffy's roleplay chat](https://steamcommunity.com/sharedfiles/filedetails/?id=2688851521) (43,589; Build 41, end of
  support). Whether TICS runs on Build 42 was not confirmed.

*First mod* (`uses: ["speech","translate"]`, beside the game's voice): speech to chat and captions that keep the
game's ranges, offline translation, and radio messages written out in the radio's text. Nobody offers these, and
Kahlua cannot.

**3. Stardew Valley (57).** It has 43k players now and no voice. The only voice option is
[Location Voice Chat](https://www.nexusmods.com/stardewvalley/mods/22550), a Discord-bot mod that moves players between
per-area channels. It is Deck Verified, typing on a pad is slow, and 55 % of reviews are not English (21 % Simplified
Chinese, 9 % Brazilian Portuguese). SMAPI C# reaches a socket, and `Helper.Multiplayer` gives the secure room/key path.
**Stardew has no Workshop:** mods come from Nexus through SMAPI, and every player installs SMAPI and the mod by hand;
nothing is pushed on join (F 4). That is the normal way to mod Stardew, not a niche: it was Nexus's fifth game by
downloads (296.4M by early 2024,
[Destructoid](https://www.destructoid.com/nexus-mods-10-billion-downloads-almost-are-skyrim-mods/)). Still, a farm's
voice works only among the players who installed all three (SMAPI, the mod, Koetama).

*First mod*: voice within the same location by position, nothing heard across maps (later a "phone" device); speech
to chat with an always-on mic for controller players; offline chat translation; speaking icons over farmers.

**4. PEAK (57).** Photon proximity voice is core to the game, so Koetama should not replace it. The base game has no
text chat; text-chat mods have 176,600 ([PeakTextChat](https://thunderstore.io/c/peak/p/borealityy/PeakTextChat/)) +
35,491 downloads. PEAK has full controller support and is Deck Verified, and 18 % of reviews are Russian. The one chat
translator, [PeakChatTranslator](https://github.com/gustavogordoni/PeakChatTranslator) (454 downloads), sends text to
Google.

*First mod* (`uses: ["speech","translate"]`):
- each player's Koetama turns their speech into chat lines and bubbles, shared over Photon, which gives captions of
  teammates' speech for deaf and hard-of-hearing players;
- offline translation of every line;
- a `status` "Koetama ready" hint.

Devices (radio static on teammates far away, as
[Conchversations](https://thunderstore.io/c/peak/p/tony4twenty/Conchversations_Beta/) does with 29,259 downloads) can
come later.

**5. Valheim (53).** It has the biggest audience in the set right now, and no built-in voice: 1.0 coverage mentions
none. Non-English reviews are 44 % (13 % Russian). The game has full controller support and is Deck Verified, and 1.0
added PS5 and Switch 2 crossplay, so there are more pad players, though console players cannot run Koetama. The numbers
will fall toward the pre-1.0 16-21k [i], which is still more than every Thunderstore co-op game except R.E.P.O. and
PEAK. Effort is lowest here because `examples/mods/valheim` already passes 15 end-to-end tests against the real
Koetama.

**But a mod alone can carry voice here, and four do** (Thunderstore downloads, 2026-10-13; none does speech to text
or translation):
- [ProximityVoiceChat](https://thunderstore.io/c/valheim/p/Azumatt/ProximityVoiceChat/) (Azumatt, 1.0.2,
  2026-09-11): 4,731, "client side, no external program".
- [ProximityVoice](https://thunderstore.io/c/valheim/p/Nubedeu/ProximityVoice/) (Nubedeu, 0.2.1): 2,522. Opus at
  ~24 kbit/s, push to talk, full volume to 10 m and silent at 50 m, no walls; Windows microphone capture (others
  untested). It sends voice through Valheim's routed RPCs, relayed by the host, which is weak for audio: everything
  goes through the host, it is reliable and ordered, and it shares the ~60 KB/s per-peer send cap with world sync [i].
- BreakoutMods-VOIP: 1,209 (experimental).
- [ValheimVoiceChat](https://thunderstore.io/c/valheim/p/BORTY/ValheimVoiceChat/) (BORTY, 2.1.0, 2026-10-09): 96.
  Voice over **Steam P2P**, the good path: push to talk, voice activation or open mic, 3D audio, per-player volume.

So a player can have voice with no app, and Koetama can't win on voice alone (G 4, M 2). Lead with speech to chat,
live captions and translation, which no Valheim mod has.

*First mod*: finish the demo into a Thunderstore mod, pitched on what a mod can't do alone (speech to chat, live
captions, offline translation), with voice as the extra.
- **Voice**: proximity voice from `listener` and positions, with Valheim's whisper/talk/shout ranges as `range` and
  the shout's far as `max_range`. When another voice mod is installed, the mod leaves voice to it and keeps speech
  and translation, so players never have to choose and no voice goes out twice.
- **Room**: the host shares the room and key over ZRoutedRpc; dedicated servers use `room_seed` (server address,
  world and password), which replaces the old "earliest claim" fallback.
- **Speech**: spoken lines go to chat and bubbles; a controller player's mic is always on.
- **HUD**: `talking` icons.
- **Translation**: chat is translated.
- **Without Koetama**: the mod still works as better chat.

### Good later
- **Minecraft Java (52)**: the largest audience, but voice is taken by Simple Voice Chat (73.8M downloads). The open
  part is captions, speech to chat and translation. The existing STT mods have about 120 downloads each and need a
  manual Vosk download, and the translators are all cloud ([Modrinth](https://modrinth.com/mod/plasmowords)). A
  speech-and-translate Fabric mod that sits beside SVC is cheap [i].
- **R.E.P.O. (50)**: 25 % Russian reviews, an English-only game, and Chinese translation mods with over 1.6M downloads,
  so translation demand is clear. It also has walkie demand (347,751). Its audience is falling from a May spike.
- **Terraria (52)**: no voice and no voice mod, and tModLoader syncs mods on join. Ask tModLoader's Workshop moderators
  for the exemption first; a refusal ends it.
- **Satisfactory (48)** and **Schedule I (47)**: Schedule I has no voice at all, and players ask for proximity chat and
  phone calls ([Steam](https://steamcommunity.com/app/3164500/discussions/1/599648251203859526)). But it is IL2CPP, with
  separate Mono and IL2CPP mod builds.
- **VRChat (48)**: a big, Japanese-growing audience where typing is hardest, but at least four maintained STT and
  translation tools already exist. It would need a new OSC connector, and voice is not needed.
- **Rust (51)**: rose with the "what a mod alone could do" criterion. Plugins run only on the server and EAC guards the
  clients, so captions and translation can only come from outside the game, through the hub. It has 61k players now,
  but the work is large (F 2, E 2) and servers often have more than the relay's 64 players per room.

### Niche (worth it only for a community that asks)
- **Hub fits, with a big audience but crowded voice**: Rust, FiveM, Garry's Mod, Tabletop Simulator. Rust and FiveM
  servers often exceed the relay's 64 players per room.
- **Co-op RPGs**: BG3, Monster Hunter, Elden Ring Seamless, Skyrim Together. Each covers only a modded slice, and
  Seamless is closed source.
- **Small but cheap**: Content Warning, Lethal Company, Stormworks, Vintage Story.
- **Big Walk (40)** (House House, published by Panic; out 2026-08-04, over a million copies sold;
  [SteamCharts](https://steamcharts.com/app/1478500): 46,224 peak, 6,044 Sep average, ~5k now and falling). Moddable
  without a Workshop: BepInEx 6 (IL2CPP) mods on [Thunderstore](https://thunderstore.io/c/big-walk/) (97 packages, the
  BepInEx pack 855k downloads) and [Nexus](https://www.nexusmods.com/bigwalk/mods/4), including multiplayer ones
  (BigChoppa needs everyone in the lobby to have it) and a [VR mod](https://github.com/CircuitLord/BigWalkVRInstaller).
  No anti-cheat or mod policy found, and nothing for voice, speech or translation. But its proximity voice *is* the
  game: voices fade, echo and are muffled by walls, walkie-talkies carry them, challenges cut talking off, and the text
  chat has "the same features (and intentional limitations)". The [FAQ](https://bigwalk.game/faq/) says talking
  outside the game is "NOT ALLOWED". So no Koetama voice: speech to the game's own text chat (which keeps its rules,
  reaches console players with no mod, and spares Deck and pad players typing) and offline translation. Crossplay with
  PS5 and Switch 2, whose players can't run Koetama.
- **Awkward**: Factorio (UDP in), Core Keeper (sandbox), 7 Days to Die (EAC off), Palworld (console share, built-in
  voice).

### Avoid
- **Phasmophobia**: the developers say "no mods" and it has its own anti-cheat.
- **Arma 3**: BattlEye blocks unlisted DLLs, and TFAR/ACRE2 own the space.
- **Arma Reforger**: localhost is unverified, and much of the audience is on console.
- **Among Us on official servers**: mod registration, and a mostly mobile audience.
- **Gorilla Tag**: modded lobbies only, with ban reports.
- **FFXIV**: Square Enix says all third-party tools violate the ToS
  ([Automaton](https://automaton-media.com/en/news/20220511-12419/)).

## Ruled out or dropped since the last survey
- **Rec Room**: shut down 2026-06-01 ([Road to VR](https://roadtovr.com/rec-room-shutdown-snap-acquisition-2026/)).
- **alt:V and RAGE:MP**: closed at Take-Two's demand (alt:V support ended 2026-07-06; RAGE:MP servers by 2026-08-31),
  leaving FiveM as the only authorised GTA V multiplayer ([GTABoom](https://www.gtaboom.com/popular-gta-v-mod-platform-given-shutdown-notice-after-9-years-of-operation-33dc)).
- **Mage Arena**: the last survey's speech-cast-spells candidate; 26 average concurrent in Sep 2026 against a
  17,126 all-time peak ([SteamCharts](https://steamcharts.com/app/3716600)).
- **Webfishing**: about 216 average, and its loader GDWeave was archived in Aug 2026 ([GitHub](https://github.com/NotNite/GDWeave)).
- **Roblox**: in-experience HTTP is HTTPS-only from servers; only Studio plugins reach localhost
  ([Roblox docs](https://create.roblox.com/docs/cloud-services/http-service)).
- **Space Engineers 2**: data-only modding since 2025-06-24, with no scripting
  ([Keen](https://support.keenswh.com/spaceengineers2/pc/announcement/space-engineers-2-alpha-vs-1-5-modding)).
  **Space Engineers 1** has fallen to 2.7k.
- **Cyberpunk 2077**: CyberMP is still in closed beta (sixth beta, Jan 2026), and it is a sandbox, not co-op
  ([Outlook Respawn](https://respawn.outlookindia.com/gaming/gaming-news/cyberpunks-multiplayer-mod-cybermp-had-its-most-successful-beta)).
- **Fallout 4 multiplayer**: Fallout Together was never finished.
- **Minecraft Bedrock**: networking only on dedicated servers (beta module), and players on phones and consoles
  cannot run Koetama; VoiceCraft already uses the companion-app design.
- **s&box, Resonite, ChilloutVR**: too small (s&box ~519 online after its April launch; Resonite ~200).

## Rules on mods that use external programs

| Hub / game | Rule (brief quote) | Effect on a Koetama mod |
|---|---|---|
| **tModLoader Workshop** | "Do not launch external programs"; "Do not instruct users to download external software"; native libraries only with an exception ([EULA](https://store.steampowered.com/workshopeula/1281930/)) | Needs an exemption: ask in the tModLoader Discord before publishing |
| **Garry's Mod Workshop** | "Addons that require user to download something from description (an external link) are not allowed" ([Facepunch wiki](https://wiki.facepunch.com/gmod/Steam_Workshop_Rules)) | Koetama must be optional; the addon must work without it |
| **Nexus Mods** | Files "that connect to the internet… are prohibited… unless where it is crucial"; caveats and requirements must be detailed ([guidelines](https://help.nexusmods.com/article/28-file-submission-guidelines)) | The mod only talks to localhost; state Koetama as a requirement |
| **Modrinth** | 1.11: no upload "to a remote server… without clear disclosure"; 5.6: list dependencies ([rules](https://modrinth.com/legal/rules)) | Disclose that voice goes to the relay (end-to-end encrypted) |
| **CurseForge** | "External download links for files are not allowed"; cross-links go at the bottom of the page; automated checks reject .exe/.sh/.bat ([policy](https://support.curseforge.com/en/support/solutions/articles/9000197279)) | Link Koetama's project page, not a file; no binaries |
| **Thunderstore** | No obfuscated code, no executing downloaded code, no self-updating; packages that "exist primarily to advertise an outside platform" are disallowed ([global rules](https://wiki.thunderstore.io/moderation/global-rules)) | The mod must do something on its own; precedent: Super_Soundboard openly sends voice to a server |
| **mod.io** | Automated scans; game owners can reject file types; on consoles UGC "cannot run scripts" ([docs](https://docs.mod.io/moderation)) | PC mods only |
| **Klei (DST)** | Hotfix 653007: QueryServer "may now query to localhost or 127.0.0.1" ([Klei](https://kleiforums.com/game-updates/dst/653007-r2465/)) | Sanctioned |
| **Factorio** | No network access by design; `send_udp` "if enabled… `--enable-lua-udp`" ([API](https://lua-api.factorio.com/latest/classes/LuaHelpers.html)) | Players must add a launch flag |
| **Rust (Facepunch)** | Plugins that affect gameplay or show UI must be listed as Modded ([Facepunch](https://support.facepunchstudios.com/hc/en-us/articles/360009062817-Guidelines-for-community-servers-using-plugins-mods)) | A Koetama server is a "Modded" server |
| **FiveM (Cfx.re PLA, 2026-09-10)** | §5.2: Rockstar "may identify… unauthorized service providers" and require "an Authorized Service" ([PLA](https://static.cfx.re/platform-license-agreement-10-sept-2026.pdf)) | Allowed today; revocable on 30 days' notice |
| **Among Us** | Mods on official servers "must be registered on lobby creation"; mod stamp; voice chat must be free ([policy](https://innersloth.com/modpolicy.php)) | Register; Koetama is free |
| **Big Walk** (a design rule, not a mod policy) | "Talking to players outside the game is ❌ NOT ALLOWED ❌" ([FAQ](https://bigwalk.game/faq/)); no mod policy found | No Koetama voice; speech and translation only through the game's own text chat |
| **Mojang** | Mods may be distributed, not sold; Mojang decides what a mod is ([EULA](https://www.minecraft.net/en-us/eula)) | Fine |
| **FFXIV, Phasmophobia, Gorilla Tag** | Third-party tools against the ToS / "no mods" / modded lobbies only | Avoid |
| **Anti-cheat** | BattlEye blocks injection and vulnerable drivers; overlays "generally allowed" ([Wikipedia](https://en.wikipedia.org/wiki/BattlEye)); Proximity Core warns its memory-reading bridges can bring bans ([Steam](https://store.steampowered.com/app/4728470/)) | Koetama reads no memory and injects nothing; no vendor promises a separate mic-only process is safe |

**Checklist for publishing a Koetama-linked mod:**
1. Ship no executables or native DLLs, and never launch, download or update Koetama from the mod.
2. Make the mod useful without Koetama: text chat, bubbles, the quick-chat wheel. When Koetama is missing, show a quiet
   hint, never an error.
3. Link Koetama's project page, not a file (on CurseForge, at the bottom of the page). List it as an optional
   requirement in the description and in Modrinth's dependency or disclosure fields.
4. Disclose what goes where. The mod talks only to 127.0.0.1 (or Koetama's files). Koetama sends voice, end-to-end
   encrypted, through a relay the player chooses to use. Speech to text and translation stay on the PC.
5. Keep the mod open source and unobfuscated, with no self-update (Thunderstore, Nexus).
6. Where a game needs permission (tModLoader exemption, Among Us lobby registration), get it before publishing.
7. Never read game memory or inject input. That is the line between Koetama and the tools anti-cheat flags.

## Competitors and adjacent tools that are game-agnostic

| Tool | What it does | Where Koetama differs |
|---|---|---|
| **Proximity Core** (Steam EA 2026-09-15, CA$7.19, 2 reviews) | Game-agnostic proximity voice; positions from "bridges", mostly memory reading, plus Workshop community bridges; Steam Audio HRTF; radio zones ([Steam](https://store.steampowered.com/app/4728470/)) | Koetama is free and MIT, runs on Windows and Linux, uses a cooperative mod API (no memory reading), an end-to-end encrypted relay, offline STT and translation, and devices. Proximity Core shows no STT or translation |
| **Mumble positional audio** | 27 native games and 3 engines via Link; 43 memory-reading plugins that break on updates ([Mumble](https://www.mumble.info/documentation/user/positional-audio/supported-games/)) | Mostly old shooters. A Koetama Link reader would also need senders' positions in voice packets [i] |
| **BetterCrewLink** (Among Us, 1.1k★) | Proximity voice by hooking the game process; antivirus warnings ([GitHub](https://github.com/OhMyGuus/BetterCrewLink)) | Proves an external app can win when the game lacks voice; also shows the memory-reading risk |
| **Simple Voice Chat, Plasmo Voice; VoiceCraft, VCMC, Bedrock Voice Chat** | Per-game proximity voice (Java mods; Bedrock companion apps fed by server scripts) | One game each; no STT or translation (PlasmoWords is a 117-download add-on) |
| **Discord bridges** (Proxcord, Skoice, Location Voice Chat) | Bots move or fade players by coordinates or zones | Server-wide and coarse; no captions |
| **VRChat STT tools** (TTS Voice Wizard, VRCT, Whispering Tiger, VRCOSC, TeaTalk, ぶいとら!) | Mic to chatbox over OSC, translation (often cloud: DeepL, Edge, Azure) | VRChat only; Koetama is offline and general |
| **Windows 11 Live Captions** | On-device captions of any PC audio; translation into English on Copilot+ PCs ([Microsoft](https://support.microsoft.com/accessibility/windows/use-live-captions-to-better-understand-audio)) | Captions what you hear, not your speech into game chat; nothing goes to other players |
| **OBS LocalVocal** | Local Whisper captions and translation for streams ([OBS](https://obsproject.com/forum/resources/localvocal-local-live-captions-translation-on-the-go.1769)) | Aimed at viewers, not at the game |
| **Built-in transcription** (Xbox Game Chat Transcription, Fortnite "Voice Dictation", Roblox chat translation) | First-party STT or translation ([Xbox Wire](https://news.xbox.com/en-us/?p=153862), [Epic](https://www.epicgames.com/help/c-202300000001639/c-202300000001735/how-do-i-enable-speech-to-text-voice-dictation-a202300000083793)) | Proves demand; only in their own games |
| **Translumo, Overwolf chat translators** | OCR screen translation; per-game chat translators (cloud) | Cloud, not tied to the game's chat |
| **VoiceAttack** | Voice to keystrokes; some anti-cheats block virtual input ([Steam](https://steamcommunity.com/app/583010/discussions/0/1319962329029569220)) | Koetama's commands go through the mod, not keystrokes |
| **Vivox, EOS Voice, Photon Voice, ODIN** | Voice SDKs billed to game developers per CCU | None aimed at modders (none found) |
| **GameVox GameLink** | Commercial voice for Arma 3 via `callExtension` and for GTA RP ([GameVox](https://gamevox.com/integrations/arma3)) | Paid, voice only |

No single tool found combines proximity voice, offline speech to game chat and offline translation behind one mod API
[i]. The closest overlaps are per game: VRChat for STT and translation, Minecraft for voice, Teardown's
[TeardownVC](https://steamcommunity.com/sharedfiles/filedetails/?id=3653434619) for voice through an exe.

## Kept from the previous survey

**The demos** (2026-10-09, all against the real Koetama; files in `examples/mods/`):

| Game | Mod platform | Transport | Result |
|---|---|---|---|
| Teardown | Lua sandbox | files | the reference mod (now [Proximity Comms](https://steamcommunity.com/sharedfiles/filedetails/?id=3812301496), with Koetama 0.5.4) |
| Valheim | BepInEx 5, C# | socket | 78 unit + 15 end-to-end tests; reusable `KoetamaClient` |
| Godot 4 (Webfishing via GDWeave) | GDScript | socket | headless; typed to signal ~50 ms |
| Tabletop Simulator | MoonSharp Lua, host only | HTTP | 17 ms (one frame); now a hub case |
| Monster Hunter Rise | REFramework Lua | files (whole-file JSON) | 17 ms; now a `room_seed` case |

Speech to text arrives ~1 s after the speaker stops (the model, not the transport); translations take 30-60 ms once
the models are loaded; an HTTP round trip is under 1 ms.

**Transport notes, still true:**
- **Real-code loaders** reach a 127.0.0.1 TCP socket: BepInEx, SMAPI, tModLoader, Fabric, SKSE, UE4SS C++, MelonLoader,
  Dalamud. On Unity Mono, avoid `ClientWebSocket` and named pipes.
- **Lua sandboxes** split three ways:
  - HTTP only: DST (localhost only), TTS, Stormworks, Northstar, Garry's Mod;
  - files only: Teardown, Zomboid, BG3SE, CET, REFramework, Space Engineers mods;
  - files out + UDP in: Factorio.
- **Prior art** for a game DLL or sandbox talking to a voice app: TFAR and ACRE2 (named pipe to a TeamSpeak plugin),
  TokoVOIP (NUI WebSocket), FFXIV's TextToTalk (a Dalamud plugin serving a WebSocket).

**The API gaps it found are closed.** The five gaps the old survey ranked (room, player ids, positions, who is
talking and readiness, host-only games) were all closed in protocol 2, as was its translation `from`/`to` item and the
per-speaker `name`.

**Still open from that list:**
- a Mumble Link reader;
- a UDP sender for Factorio's inbound side;
- WebSocket, if FiveM's NUI cannot reach HTTP on localhost.

## Gaps in this survey
- **No October numbers** for FiveM, VRChat, GMod, Arma, Squad, TTS, MSFS or ETS2 (Jun-Sep 2026 tracker snapshots).
  Vintage Story, Minecraft, RedM and TruckersMP have no concurrency figures at all. "Now" is one snapshot at an unknown
  hour; DST swings 12k to 48k in a day.
- **Co-op RPG mods**: Nexus returned 403, so there are no download counts for Mantella, Seamless Co-op, Skyrim
  Together, Stardew's Location Voice Chat, the Minecraft translators or DST's Chat Machine Translation. Built-in voice
  in BG3, DRG, MH Wilds, Satisfactory, Core Keeper, Terraria and Stardew was not checked against primary sources.
- **Policy texts not retrieved**: Roblox ToS, mod.io Acceptable Use Policy, Bohemia's Workshop terms, the Rockstar Mod
  Guidelines, VRChat's stance on OSC apps, BattlEye extension whitelisting, and the developer policies of Iron Gate,
  Landfall, semiwork, TVGS and ConcernedApe. No evidence of anyone banned for running a voice or caption app beside a
  game was found either way.
- **Unverified technical points**: whether a FiveM NUI or an Arma Reforger REST call can reach localhost; Core Keeper's
  SDK sandbox; whether Zomboid Workshop mods and Factorio mods download on join (general knowledge); DayZ script HTTP;
  whether DST's `QueryServer` sends `Content-Length` with a POST body (the DST hand-off checks it first); whether
  [TICS](https://steamcommunity.com/sharedfiles/filedetails/?id=3366438318) runs on Zomboid Build 42; how Azumatt's
  Valheim [ProximityVoiceChat](https://thunderstore.io/c/valheim/p/Azumatt/ProximityVoiceChat/) carries voice; Big
  Walk's text-chat internals and PC/console split.
- **Controller and console share**: no per-game controller share exists. Valve's latest figure is from July 2024.
  Review language under-counts Chinese players (R.E.P.O.'s >1.6M Chinese-translation-mod downloads against 3.8 %
  Chinese reviews).
- **Scores are judgements.** A different weight on P (×3 instead of ×4) keeps the same top five. The "what a mod alone
  could do" scores (M, added 2026-10-13) rest mostly on what each game's scripting allows; how well a game's own
  network path carries voice (Valheim's, Stardew's, Terraria's) was judged from its design, not measured.
- **Untried claims**: none of the protocol-2 features has run in a real game other than Teardown, and the hub has not
  been tried on a headless dedicated server.
