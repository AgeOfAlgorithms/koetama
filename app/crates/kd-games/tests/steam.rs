//! Finding a Steam game (test_app.py): a stand-in Steam folder with two libraries, the game in the second, its
//! Workshop folder and Proton prefix. And the game list.
use kd_games::{by_id, games, steam};
use std::path::{Path, PathBuf};

fn normcase(p: &Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(windows) {
        s.to_lowercase().replace('/', "\\")
    } else {
        s.into_owned()
    }
}

#[test]
fn a_stand_in_steam() {
    let td = std::env::temp_dir().join(format!("kd-games-steam-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&td);
    let root = td.join("Steam");
    let lib2 = td.join("Games");
    let mk = |p: PathBuf| std::fs::create_dir_all(p).unwrap();
    mk(root.join("steamapps"));
    mk(lib2.join("steamapps").join("common").join("Teardown"));
    mk(lib2.join("steamapps").join("workshop").join("content").join("1167630"));
    mk(lib2.join("steamapps").join("compatdata").join("1167630").join("pfx").join("drive_c").join("users").join("steamuser"));
    let esc = |p: &Path| p.display().to_string().replace('\\', "\\\\");
    // (a third entry that does not exist, and the second again in other case and slashes: both skipped)
    let again = if cfg!(windows) { esc(&lib2).to_uppercase().replace("\\\\", "/") } else { esc(&lib2) };
    std::fs::write(
        root.join("steamapps").join("libraryfolders.vdf"),
        format!(
            "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\
             \t\"2\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"3\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
            esc(&root),
            esc(&lib2),
            esc(&td.join("Gone")),
            again
        ),
    )
    .unwrap();
    std::fs::write(
        lib2.join("steamapps").join("appmanifest_1167630.acf"),
        "\"AppState\"\n{\n\t\"installdir\"\t\t\"Teardown\"\n}\n",
    )
    .unwrap();
    let libs = steam::libraries(Some(&root));
    assert_eq!(libs.len(), 2, "Steam's library folders read: {libs:?}");
    assert_eq!(normcase(&libs[1]), normcase(&lib2));
    assert_eq!(normcase(&steam::app_library(1167630, Some(&root)).unwrap()), normcase(&lib2));
    assert!(steam::install_dir(1167630, Some(&root)).unwrap().ends_with(Path::new("common").join("Teardown")));
    assert!(steam::workshop_dir(1167630, Some(&root)).unwrap().ends_with(Path::new("content").join("1167630")));
    assert!(steam::proton_user(1167630, Some(&root)).unwrap().ends_with(Path::new("users").join("steamuser")));
    assert!(steam::install_dir(4242, Some(&root)).is_none(), "a game not installed: None");
    assert!(steam::workshop_dir(4242, Some(&root)).is_none());
    // (no libraryfolders.vdf: Steam's own folder only)
    assert_eq!(steam::libraries(Some(&lib2)), vec![lib2.clone()]);
    let _ = std::fs::remove_dir_all(&td);
}

#[test]
fn this_pc() {
    // (whatever is installed here: no panic; Documents is a path)
    let _ = steam::steam_root();
    let docs = steam::documents_dir();
    assert!(!docs.as_os_str().is_empty());
    println!("steam: {:?}, documents: {}", steam::steam_root(), docs.display());
}

/// By hand: what this PC has (compare with the Python's steam.py / teardown.py). cargo test -- --ignored --nocapture
#[test]
#[ignore]
fn print_this_pc() {
    use kd_games::teardown;
    println!("{:?}", steam::steam_root());
    println!("{:?}", steam::libraries(None));
    println!("{:?}", steam::install_dir(teardown::APPID, None));
    println!("{:?}", steam::workshop_dir(teardown::APPID, None));
    println!("{:?}", steam::documents_dir());
    println!("{:?}", teardown::savegame_path());
    println!("{:?}", teardown::io_dirs());
    println!("{:?}", teardown::work_dir());
}

/// By hand (Windows, runs PowerShell ~10 s): the three test voices made in a temp folder are the same files as the
/// ones the Python made (export/voicehelper, when there).
#[test]
#[ignore]
fn voices_as_python() {
    use kd_games::teardown;
    let d = std::env::temp_dir().join(format!("kd-games-voices-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let got = teardown::make_voices_in(&d);
    assert_eq!(got.len(), 3, "{got:?}");
    let py = teardown::work_dir();
    for (src, path) in &got {
        let mine = std::fs::read(path).unwrap();
        println!("voice {src}: {} bytes", mine.len());
        if let Ok(theirs) = std::fs::read(py.join(format!("voice{src}.wav"))) {
            assert!(mine == theirs, "voice {src} differs from the Python's ({} vs {} bytes)", mine.len(), theirs.len());
        }
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_game_list() {
    let all = games();
    // (built in: Teardown alone; profiles this PC has installed come after it)
    assert_eq!(all.iter().filter(|g| g.builtin).count(), 1);
    assert!(all[0].builtin);
    assert_eq!(by_id("teardown-proximity-babble-chat").name, "Teardown");
    assert_eq!(by_id("no such game").id, all[0].id);
    for g in &all {
        assert!(!g.id.is_empty() && !g.name.is_empty() && !g.needs.is_empty());
    }
}
