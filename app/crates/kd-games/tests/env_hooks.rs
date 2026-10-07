//! The test hooks engine/test_e2e.py relies on: SAVEPROBE_DIR (the feed file's folder; the profile's file name) and
//! HFP_MODS (the only output folder), and make()'s io_dir (the only output folder, before HFP_MODS). One test: they
//! are environment variables, shared by the whole process.
use kd_common::feed::{Feed, FeedSink};
use kd_games::profile::Profile;
use kd_games::{by_id, teardown, GameKind};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

struct NoSink;
impl FeedSink for NoSink {
    fn set_feed(&self, _: Feed) {}
    fn fresh(&self) -> bool {
        false
    }
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("kd-games-env-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn saveprobe_dir_and_hfp_mods() {
    let (game, mods, io) = (tmp("game"), tmp("mods"), tmp("io"));
    std::env::set_var("SAVEPROBE_DIR", &game);
    std::env::set_var("HFP_MODS", &mods);
    assert_eq!(teardown::savegame_path(), game.join("savegame.xml"));
    assert_eq!(teardown::io_dirs(), vec![mods.clone()]);
    let mut g = by_id("teardown-proximity-babble-chat").make(Arc::new(NoSink), kd_common::null_log(), None);
    assert_eq!(
        g.describe(),
        vec![format!("reading {}", game.join("savegame.xml").display()), format!("my files for the game go to: {}", mods.display())]
    );
    g.start();
    assert!(mods.join("pcvx_on").exists());
    g.stop();
    assert!(!mods.join("pcvx_on").exists());
    // --io-dir: the only folder, before HFP_MODS
    let g = by_id("teardown-proximity-babble-chat").make(Arc::new(NoSink), kd_common::null_log(), Some(io.clone()));
    assert_eq!(g.describe()[1], format!("my files for the game go to: {}", io.display()));
    // another files profile: its own file name in SAVEPROBE_DIR
    let p = json!({"format": 1, "id": "talky-game", "game": "Talky", "mod": "T", "url": "https://example.com", "author": "me",
                   "connector": {"type": "files", "feed": {"file": "{steam_app:99999999}/data/state.txt"},
                                 "out": {"dirs": ["{steam_app:99999999}/mods"], "prefix": "talky_"}}});
    let kind = GameKind::from_profile(Profile::parse(&p.to_string()).unwrap(), false, None);
    let g = kind.make(Arc::new(NoSink), kd_common::null_log(), None);
    assert_eq!(g.describe()[0], format!("reading {}", game.join("state.txt").display()));
    assert_eq!(g.describe()[1], format!("my files for the game go to: {}", mods.display()));
    std::env::remove_var("SAVEPROBE_DIR");
    std::env::remove_var("HFP_MODS");
    let g = kind.make(Arc::new(NoSink), kd_common::null_log(), None);
    assert_eq!(g.describe(), vec!["reading ".to_string(), "my files for the game go to: ".to_string()], "not on this PC: nothing");
    assert_eq!(g.locate(), (false, String::new()), "no locate in the profile: not looked for");
    for d in [game, mods, io] {
        let _ = std::fs::remove_dir_all(d);
    }
}
