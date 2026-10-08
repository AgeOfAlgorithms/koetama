# Privacy

Kotodama has no accounts, no ads and no telemetry. It never sends anything about you or your PC to its author.

## What stays on your PC

- **Your speech and what it says.** Speech to text runs entirely on your own computer. Your microphone's audio is
  never sent anywhere to be transcribed.
- **Your settings, your languages, the speech models and the game mod profiles** are stored in
  `%LOCALAPPDATA%\Kotodama` (Windows) or `~/.local/share/kotodama` (Linux).

## What goes over the internet, and only when it is needed

| What | Where to | When |
|---|---|---|
| **Your voice**, compressed and **end-to-end encrypted** | Kotodama's voice relay (a Cloudflare Worker, [`relay/`](relay/)), which passes it on to the other players | Only while you talk in a game session with voice chat, and only to the players close enough to hear you. The key is made on a player's PC and shared only with the players in that game session: the relay cannot decrypt or listen to anything, and it stores nothing. Like any web server, it sees your IP address, the session's room name and your player number in that session. |
| **Speech model downloads** | Hugging Face (`huggingface.co`) and GitHub (`github.com`) | The first time a language you picked needs its model (one download per model). |
| **Update checks and updates** | GitHub (`api.github.com`, `github.com`) | When you click **Check for updates**, or when Kotodama checks at start. |

These services see your IP address when Kotodama connects to them, as they would for any download. Their own
privacy policies apply: [Cloudflare](https://www.cloudflare.com/privacypolicy/),
[Hugging Face](https://huggingface.co/privacy), [GitHub](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement).

## The game

Kotodama talks to your game only on your own PC (files the game's mod reads and writes, or a local network port that
only programs on your PC can reach). What the game mod then does with the words you say (for example show them to
other players) is up to that mod.

Questions: open an issue at [github.com/AgeOfAlgorithms/kotodama](https://github.com/AgeOfAlgorithms/kotodama/issues).
