//! Two game mods may not share a message-file prefix: each removes its own message files, and with one prefix in one
//! folder they would remove each other's. And ids name the game and the mod: the old "teardown" is the built-in's.
use kd_games::*;

fn files_profile(id: &str, prefix: &str) -> String {
    format!(
        r#"{{"format": 1, "id": "{id}", "game": "Teardown", "mod": "Another Chat", "url": "https://example.com/other",
            "author": "Someone",
            "connector": {{"type": "files",
              "feed": {{"file": "{{localappdata}}/Teardown/savegame.xml"}},
              "out": {{"dirs": ["{{documents}}/Teardown/mods"], "prefix": "{prefix}"}}}}}}"#
    )
}

#[test]
fn a_prefix_in_use_is_refused_and_old_ids_are_the_builtins() {
    let dir = std::env::temp_dir().join(format!("kd-games-prefix-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("KOTODAMA_PROFILES_DIR", dir.join("games"));
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();

    // another Teardown mod with the built-in's prefix: refused, nothing installed
    let clash = src.join("clash.json");
    std::fs::write(&clash, files_profile("teardown-another-chat", "pcvx_")).unwrap();
    let e = install_profile(&clash).unwrap_err();
    assert!(e.contains("prefix is already used by Teardown (Proximity Babble Chat mod)"), "{e}");
    assert_eq!(games().len(), 1);

    // its own prefix: fine, next to the built-in
    let own = src.join("own.json");
    std::fs::write(&own, files_profile("teardown-another-chat", "achat_")).unwrap();
    let g = install_profile(&own).unwrap();
    assert_eq!(g.id, "teardown-another-chat");
    let ids: Vec<String> = games().iter().map(|g| g.id.clone()).collect();
    assert_eq!(ids, ["teardown-proximity-babble-chat", "teardown-another-chat"]);

    // a file dropped into the folder by hand with a taken prefix: skipped, and said why (of two files, the first by
    // name keeps it)
    std::fs::write(dir.join("games").join("zz-dropped.json"), files_profile("teardown-third-chat", "achat_")).unwrap();
    assert_eq!(games().len(), 2);
    assert!(bad_profiles().iter().any(|(p, why)| p.ends_with("zz-dropped.json") && why.contains("prefix is already used")));

    // the old id: the built-in, for settings that still hold it; no profile can take it
    assert_eq!(by_id("teardown").id, "teardown-proximity-babble-chat");
    let old = src.join("old.json");
    std::fs::write(&old, files_profile("teardown", "old_")).unwrap();
    assert!(install_profile(&old).unwrap_err().contains("built-in"));
    let _ = std::fs::remove_dir_all(&dir);
}
