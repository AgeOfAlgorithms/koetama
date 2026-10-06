# Claude Code
Read PROJECT.md first (Kotodama: the layout, environments, commands, the release plan, open items, and the full
research history), and PROTOCOL.md for the game link and how a game module works. Games are modules in
`engine/games/`; the engine (asr.py, audio.py, runtime.py) knows no game. The Teardown side lives in the
teardown-mods repo (`proxchat/`, the mod Proximity Babble Chat, `mods/proximity chat/voice.lua`); a protocol change
needs both sides and both test suites (`engine/test_*.py` here, `tools/test_proxchat.lua` there).
Python: conda env `pcvoice` (pip packages only). No heavy jobs (benchmarks, builds) while the user is playing.
