# Claude Code
Read PROJECT.md first (Kotodama: the layout, environments, commands, the release plan, open items, and the full
research history), and PROTOCOL.md for the game link and how a game module works. The app is Rust, in `app/`
(app/DESIGN.md: the crates and their interfaces); `engine/` is the first, Python version, kept as the REFERENCE the
Rust tests compare against (app/fixtures/make_fixtures.py writes its answers) and for the benchmarks. A behaviour
change goes into both, with new fixtures. Games are modules in `app/crates/kd-games`; the engine (kd-speech, kd-audio,
the runtime) knows no game. The Teardown side lives in the teardown-mods repo (`proxchat/`, the mod Proximity Babble
Chat, `mods/proximity chat/voice.lua`); a protocol change needs both sides and both test suites (`cargo test
--workspace` here, `tools/test_proxchat.lua` there).
Rust: `export PATH="$HOME/.cargo/bin:$PATH"`, run cargo from `app/`. Python: conda env `pcvoice` (pip packages only).
No heavy jobs (benchmarks, builds) while the user is playing.
