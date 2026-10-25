# Privacy

Koetama has no accounts, no ads and no telemetry. It never sends anything about you or your PC to its author.

## What stays on your PC

- **Your speech and what it says.** Speech to text runs entirely on your own computer. Your microphone's audio is
  never sent anywhere to be transcribed. (A game mod that uses a large language model may send the words it is given
  on: below.)
- **Your large language model key**, if you are a host who uses one: kept sealed by Windows (only your account on
  your PC can open it) and sent only to the service it is for.
- **Your settings, your languages, the speech models and the game mod profiles** are stored in
  `%LOCALAPPDATA%\Koetama` (Windows) or `~/.local/share/koetama` (Linux).

## What goes over the internet, and only when it is needed

| What | Where to | When |
|---|---|---|
| **Your voice**, compressed and **end-to-end encrypted** | Koetama's voice relay (a Cloudflare Worker, [`relay/`](relay/)), which passes it on to the other players | Only while you talk in a game session with voice chat, and only to the players close enough to hear you. The key is made on a player's PC and shared only with the players in that game session: the relay cannot decrypt or listen to anything, and it stores nothing. Like any web server, it sees your IP address, the session's room name and your player number in that session. |
| **Speech model downloads** | Hugging Face (`huggingface.co`) and GitHub (`github.com`) | The first time a language you picked needs its model (one download per model). |
| **Translation model downloads** | Mozilla (`firefox-settings-attachments.cdn.mozilla.net`, `firefox.settings.services.mozilla.com`) | When you turned translation on and a chat line comes in a language whose model is not on your PC yet. Your chat itself is translated on your PC. |
| **What a game mod sends to a large language model**: lines said to its characters (by voice or typed), who said them, what the mod says is going on | The AI service the **host** chose in Koetama ("Large Language Models"), with the host's own key | Only when the host turned it on for that game and its mod uses it. That service's own privacy policy applies; it is not Koetama's. Koetama keeps no copy of what is sent or answered. If you join someone else's game, what you say near its characters may go to the service the host chose: the game should tell you. |
| **Large language model prices** (a public list; nothing about you or your game is sent) | OpenRouter (`openrouter.ai/api/v1/models`) | At most once a day, and only once a host has set a model in "Large Language Models" (or uses `--llm-model` without `--llm-price`): Koetama counts what the model costs with that list. |
| **Update checks and updates** | GitHub (`api.github.com`, `github.com`) | When you click **Check for updates**, or when Koetama checks at start. |

These services see your IP address when Koetama connects to them, as they would for any download. Their own
privacy policies apply: [Cloudflare](https://www.cloudflare.com/privacypolicy/),
[Hugging Face](https://huggingface.co/privacy), [OpenRouter](https://openrouter.ai/privacy), [GitHub](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement).

## The game

Koetama talks to your game only on your own PC (files the game's mod reads and writes, or a local network port that
only programs on your PC can reach). What the game mod then does with the words you say (for example show them to
other players) is up to that mod.

Questions: open an issue at [github.com/AgeOfAlgorithms/koetama](https://github.com/AgeOfAlgorithms/koetama/issues).
