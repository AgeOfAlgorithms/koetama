//! The profiles folder (KOTODAMA_PROFILES_DIR: a temp one): games() lists built-ins then the valid files, a clash
//! of ids or a bad file goes to bad_profiles(); load / install (replacing the same id) / remove (never a built-in).
//! One test: the folder is an environment variable, shared by the whole process.
use kd_games::{bad_profiles, by_id, games, install_profile, load_profile, profiles_dir, remove_profile};
use serde_json::json;
use std::path::{Path, PathBuf};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("kd-games-profiles-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn socket(id: &str, game: &str, port: u16) -> String {
    json!({"format": 1, "id": id, "game": game, "mod": "Voice", "url": "https://example.com", "author": "me",
           "connector": {"type": "socket", "port": port}})
    .to_string()
}

fn ids() -> Vec<String> {
    games().into_iter().map(|g| g.id).collect()
}

fn file_names(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

#[test]
fn the_profiles_folder() {
    let root = tmp("root");
    let dir = root.join("games");
    let outside = tmp("outside");
    std::env::set_var("KOTODAMA_PROFILES_DIR", &dir);
    assert_eq!(profiles_dir(), dir);
    // no folder yet: the built-ins
    assert_eq!(ids(), ["teardown-proximity-babble-chat"]);
    assert!(bad_profiles().is_empty());
    let td = by_id("teardown-proximity-babble-chat");
    assert!(td.builtin && td.source.is_none() && td.voices && td.speech && td.name == "Teardown" && td.author == "AgeOfAlgorithms");
    assert!(td.summary.iter().any(|l| l.contains("pcvx_*")), "{:?}", td.summary);

    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.json"), socket("example-game", "Example Game", 47120)).unwrap();
    std::fs::write(dir.join("b.json"), "{ not json").unwrap();
    std::fs::write(dir.join("c.json"), socket("teardown", "Fake Teardown", 47121)).unwrap();
    std::fs::write(dir.join("d.JSON"), socket("example-game", "Example Again", 47122)).unwrap();
    std::fs::write(dir.join("e.json"), socket("zz-game", "ZZ", 47123)).unwrap();
    std::fs::write(dir.join("notes.txt"), "not a profile").unwrap();
    assert_eq!(ids(), ["teardown-proximity-babble-chat", "example-game", "zz-game"]);
    let ex = by_id("example-game");
    assert!(!ex.builtin && ex.source.as_deref() == Some(dir.join("a.json").as_path()) && ex.name == "Example Game");
    assert_eq!(ex.summary[0], "listens on 127.0.0.1:47120 (this computer only)");
    assert_eq!(by_id("nope").id, "teardown-proximity-babble-chat", "unknown: the first");
    let bad = bad_profiles();
    let why = |n: &str| bad.iter().find(|(p, _)| p.file_name().unwrap() == n).map(|(_, w)| w.clone()).unwrap_or_default();
    assert_eq!(bad.len(), 3, "{bad:?}");
    assert!(why("b.json").starts_with("b.json: not valid JSON"), "{bad:?}");
    assert_eq!(why("c.json"), "the id \"teardown\" is already used by Kotodama's built-in Teardown: skipped");
    assert_eq!(why("d.JSON"), "the id \"example-game\" is already used by a.json: skipped");

    // load: the preview, nothing copied
    let new = outside.join("my new game.json");
    std::fs::write(&new, socket("other-game", "Other Game", 47124)).unwrap();
    let g = load_profile(&new).unwrap();
    assert!(g.id == "other-game" && g.source.as_deref() == Some(new.as_path()) && !dir.join("other-game.json").exists());
    assert!(load_profile(&outside.join("missing.json")).unwrap_err().starts_with("missing.json: cannot read it"));
    std::fs::write(outside.join("big.json"), vec![b' '; 70 * 1024]).unwrap();
    assert!(load_profile(&outside.join("big.json")).unwrap_err().contains("too big"));
    // install: copied as <id>.json
    let g = install_profile(&new).unwrap();
    assert_eq!(g.source.as_deref(), Some(dir.join("other-game.json").as_path()));
    assert!(new.exists(), "the user's own copy stays");
    assert_eq!(ids(), ["teardown-proximity-babble-chat", "example-game", "zz-game", "other-game"], "sorted by file name: a, e, other-game");
    // a built-in's id: refused
    let fake = outside.join("teardown.json");
    std::fs::write(&fake, socket("teardown", "Fake", 47125)).unwrap();
    assert_eq!(install_profile(&fake).unwrap_err(), "the id \"teardown\" is Kotodama's built-in Teardown: a profile cannot replace it");
    // a bad file: refused with its reason, nothing copied
    std::fs::write(outside.join("bad.json"), socket("x", "X", 47126)).unwrap();
    assert!(install_profile(&outside.join("bad.json")).unwrap_err().starts_with("bad.json: \"id\": \"x\""));
    // a new version of example-game replaces every file with that id (a.json, d.JSON)
    let v2 = outside.join("example v2.json");
    std::fs::write(&v2, socket("example-game", "Example Game 2", 47127)).unwrap();
    install_profile(&v2).unwrap();
    assert_eq!(file_names(&dir), ["b.json", "c.json", "e.json", "example-game.json", "notes.txt", "other-game.json"]);
    assert_eq!(by_id("example-game").name, "Example Game 2");
    assert_eq!(by_id("example-game").summary[0], "listens on 127.0.0.1:47127 (this computer only)");
    // installing a file already in the folder under another name: moved to <id>.json
    install_profile(&dir.join("e.json")).unwrap();
    assert!(!dir.join("e.json").exists() && dir.join("zz-game.json").exists());
    assert_eq!(ids(), ["teardown-proximity-babble-chat", "example-game", "other-game", "zz-game"]);
    // reinstalling a profile from its own place: still there
    install_profile(&dir.join("zz-game.json")).unwrap();
    assert!(dir.join("zz-game.json").exists());

    // remove: only profile files, never a built-in
    assert_eq!(remove_profile("teardown-proximity-babble-chat").unwrap_err(), "Teardown is built into Kotodama: it cannot be removed");
    remove_profile("other-game").unwrap();
    assert!(!dir.join("other-game.json").exists());
    assert_eq!(remove_profile("other-game").unwrap_err(), "no game mod profile with the id \"other-game\"");
    assert_eq!(ids(), ["teardown-proximity-babble-chat", "example-game", "zz-game"]);
    assert!(dir.join("notes.txt").exists() && dir.join("b.json").exists() && dir.join("c.json").exists(), "other files untouched");
    // a profile's module
    let sink = std::sync::Arc::new(NoSink);
    let g = by_id("zz-game").make(sink, kd_common::null_log(), None);
    assert_eq!((g.id(), g.name()), ("zz-game", "ZZ"));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside);
}

struct NoSink;
impl kd_common::feed::FeedSink for NoSink {
    fn set_feed(&self, _: kd_common::feed::Feed) {}
    fn fresh(&self) -> bool {
        false
    }
}
