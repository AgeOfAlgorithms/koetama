//! Game mod profiles: a good one of each connector, each kind of bad one with its message, the placeholders and the
//! candidates, the summary the import preview shows.
use kd_games::profile::{Connector, MessageFormat, PathSpec, PathTemplate, Place, Profile};
use serde_json::{json, Value};
use std::path::PathBuf;

fn files_profile() -> Value {
    json!({
        "format": 1,
        "id": "my-game",
        "game": "My Game",
        "mod": "Talky",
        "url": "https://example.com/talky",
        "author": "Someone",
        "locate": {"steam_app": 4242},
        "test_voices": [{"id": 1, "voice": "Microsoft Zira Desktop", "rate": 1, "text": "Hello, I am a test."}],
        "speaker_names": {"1": "tester"},
        "connector": {
            "type": "files",
            "feed": {"file": ["{steam_app:4242}/save/state.txt", "{env:MY_GAME_SAVE}/state.txt"],
                     "pattern": "FEED\\[([^\\]]*)\\]", "tag_pattern": "", "complete": ""},
            "out": {"dirs": ["{documents}/My Game/mods", ["{steam_workshop:4242}", "{home}/ws"]],
                    "tag_dirs": [{"tag_prefix": "ws-", "dir": 1}], "prefix": "talky_", "message": "json"}
        }
    })
}

fn socket_profile() -> Value {
    json!({"format": 1, "id": "example-game", "game": "Example Game", "mod": "Example Voice", "url": "https://example.com",
           "author": "Someone", "connector": {"type": "socket", "port": 47120}})
}

fn parse(v: &Value) -> Result<Profile, String> {
    Profile::parse(&v.to_string())
}

/// the profile with one change at a JSON pointer (None: removed)
fn with(mut v: Value, pointer: &str, new: Option<Value>) -> Value {
    let (parent, key) = pointer.rsplit_once('/').unwrap();
    let p = v.pointer_mut(parent).unwrap_or_else(|| panic!("{parent}"));
    match (p, new) {
        (Value::Object(m), Some(n)) => {
            m.insert(key.to_string(), n);
        }
        (Value::Object(m), None) => {
            m.remove(key);
        }
        (Value::Array(a), Some(n)) => a[key.parse::<usize>().unwrap()] = n,
        _ => panic!("{pointer}"),
    }
    v
}

fn err(v: &Value) -> String {
    match parse(v) {
        Ok(_) => panic!("should be refused: {v}"),
        Err(e) => e,
    }
}

#[test]
fn good_profiles() {
    let p = parse(&files_profile()).unwrap();
    assert_eq!((p.id.as_str(), p.game.as_str(), p.mod_name.as_str(), p.author.as_str()), ("my-game", "My Game", "Talky", "Someone"));
    assert_eq!(p.needs, "the Talky mod", "needs: by default from the mod's name");
    assert_eq!(p.steam_app, Some(4242));
    assert!(p.voices && p.speech, "uses: both by default");
    assert_eq!((p.speaker_name(1), p.speaker_name(2)), ("tester".to_string(), "2".to_string()));
    assert_eq!(p.test_voices.len(), 1);
    assert_eq!(p.test_voices[0].rate, 1);
    let Connector::Files(c) = &p.connector else { panic!("files") };
    assert_eq!((c.prefix.as_str(), c.message, c.complete.as_str()), ("talky_", MessageFormat::Json, ""));
    assert_eq!(c.tag_dirs, vec![("ws-".to_string(), 1)]);
    assert_eq!((c.feed_file.0.len(), c.dirs.len(), c.dirs[1].0.len()), (2, 2, 2));
    let s = parse(&socket_profile()).unwrap();
    assert!(matches!(s.connector, Connector::Socket(ref c) if c.port == 47120));
    assert!(s.test_voices.is_empty() && s.speaker_names.is_empty() && s.steam_app.is_none());
    // the files connector's defaults: Teardown's
    let d = parse(&with(with(files_profile(), "/connector/feed/pattern", None), "/connector/out/prefix", None)).unwrap();
    let Connector::Files(c) = &d.connector else { panic!() };
    assert_eq!((c.pattern.as_str(), c.prefix.as_str()), (kd_games::teardown::FEED, "pcvx_"));
    let d = parse(&with(with(files_profile(), "/connector/feed/complete", None), "/connector/out/message", None)).unwrap();
    let Connector::Files(c) = &d.connector else { panic!() };
    assert_eq!((c.complete.as_str(), c.message), ("</registry>", MessageFormat::TeardownPrefab));
    // (Notepad's byte order mark, nulls as missing; what the player needs comes from the mod's name)
    let t = format!("\u{FEFF}{}", with(socket_profile(), "/locate", Some(Value::Null)));
    let p = Profile::parse(&t).unwrap();
    assert_eq!(p.needs, format!("the {} mod", p.mod_name));
    // (there is no "needs" field: it only repeated the mod's name)
    let e = Profile::parse(&with(socket_profile(), "/needs", Some(json!("the Example mod"))).to_string()).unwrap_err();
    assert!(e.contains("unknown field \"needs\""), "{e}");
}

#[test]
fn uses() {
    let p = parse(&with(socket_profile(), "/uses", Some(json!(["speech"])))).unwrap();
    assert!(p.speech && !p.voices);
    let p = parse(&with(socket_profile(), "/uses", Some(json!(["voices"])))).unwrap();
    assert!(p.voices && !p.speech);
    let p = parse(&with(socket_profile(), "/uses", Some(json!(["voices", "speech", "voices"])))).unwrap();
    assert!(p.voices && p.speech);
    assert!(err(&with(socket_profile(), "/uses", Some(json!([])))).contains("at least one"));
    assert!(err(&with(socket_profile(), "/uses", Some(json!("voices")))).contains("\"uses\": must be a list"));
    let e = err(&with(socket_profile(), "/uses", Some(json!(["voices", "video"]))));
    assert!(e.contains("\"uses\": unknown \"video\""), "{e}");
    let sum = parse(&with(socket_profile(), "/uses", Some(json!(["speech"])))).unwrap().summary_with(&|_| None);
    assert!(sum.iter().any(|l| l.contains("speech to text")) && !sum.iter().any(|l| l.contains("voices")), "{sum:?}");
}

#[test]
fn bad_profiles_say_why() {
    let f = files_profile;
    let s = socket_profile;
    let cases: Vec<(Value, &str)> = vec![
        (with(s(), "/format", Some(json!(2))), "profile format 2 is newer than this Koetama reads (1): update Koetama"),
        (with(s(), "/format", Some(json!("1"))), "\"format\": must be 1"),
        (with(s(), "/format", None), "\"format\" is missing"),
        (with(s(), "/id", None), "\"id\" is missing"),
        (with(s(), "/id", Some(json!("My Game"))), "\"id\": \"My Game\" must be 3 to 40 of a-z, 0-9 and -"),
        (with(s(), "/id", Some(json!("ab"))), "\"id\": \"ab\" must be 3 to 40"),
        (with(s(), "/id", Some(json!("../evil"))), "\"id\": \"../evil\""),
        (with(s(), "/game", None), "\"game\" is missing"),
        (with(s(), "/game", Some(json!(""))), "\"game\": must have 1 to 80 characters"),
        (with(s(), "/mod", Some(json!("two\nlines"))), "\"mod\": may not hold control characters"),
        (with(s(), "/url", Some(json!("file:///C:/Windows/system32/calc.exe"))), "\"url\": must be a web page (https://...)"),
        (with(s(), "/author", Some(json!(3))), "\"author\": must be a string"),
        (with(s(), "/colour", Some(json!("red"))), "unknown field \"colour\""),
        (with(s(), "/connector", None), "\"connector\" is missing"),
        (with(s(), "/connector/type", Some(json!("pipe"))), "\"connector.type\": \"pipe\" is not a connector (known: \"files\", \"socket\", \"http\")"),
        (with(s(), "/connector/type", None), "\"connector.type\" is missing"),
        (with(s(), "/connector/port", Some(json!(80))), "\"connector.port\": must be a port from 1024 to 65535 (got 80)"),
        (with(s(), "/connector/port", Some(json!(70000))), "(got 70000)"),
        (with(s(), "/connector/port", Some(json!("47120"))), "\"connector.port\": must be a port"),
        (with(s(), "/connector/host", Some(json!("0.0.0.0"))), "unknown field \"connector.host\""),
        (with(s(), "/locate", Some(json!({"steam_app": -1}))), "\"locate.steam_app\": must be a Steam app id"),
        (with(s(), "/test_voices", Some(json!([{"id": 1, "voice": "V", "rate": 20, "text": "t"}]))), "\"test_voices[0].rate\": must be a whole number from -10 to 10"),
        (with(s(), "/test_voices", Some(json!([{"id": 1, "voice": "V", "text": "a\u{2019}; calc"}, {"id": 1, "voice": "V", "text": "t"}]))), "\"test_voices[1].id\": 1 is there twice"),
        (with(s(), "/test_voices", Some(json!([{"id": 1, "voice": "V", "text": "a\r\nb"}]))), "\"test_voices[0].text\": may not hold control characters"),
        (with(s(), "/test_voices", Some(json!([{"id": 0, "voice": "V", "text": "t"}]))), "\"test_voices[0].id\": must be a whole number from 1 to 999"),
        (with(s(), "/speaker_names", Some(json!({"one": "x"}))), "\"speaker_names\": the key \"one\" must be a test voice's id"),
        (with(f(), "/connector/out/prefix", Some(json!("x"))), "\"connector.out.prefix\": \"x\" must be 3 to 32 letters, digits or _ and end in _"),
        (with(f(), "/connector/out/prefix", Some(json!("pcvx"))), "\"connector.out.prefix\": \"pcvx\""),
        (with(f(), "/connector/out/prefix", Some(json!("../x_"))), "\"connector.out.prefix\": \"../x_\""),
        (with(f(), "/connector/out/prefix", Some(json!("a*_"))), "\"connector.out.prefix\": \"a*_\""),
        (with(f(), "/connector/out/prefix", Some(json!(""))), "\"connector.out.prefix\": \"\""),
        (with(f(), "/connector/out/message", Some(json!("xml"))), "\"connector.out.message\": \"xml\" is not a message format"),
        (with(f(), "/connector/out/dirs", Some(json!([]))), "\"connector.out.dirs\": must be a list of 1 to 8 folders"),
        (with(f(), "/connector/out/dirs/0", Some(json!("{appdata}/x"))), "\"connector.out.dirs[0]\": unknown placeholder {appdata} (known: {documents}"),
        (with(f(), "/connector/out/dirs/0", Some(json!("{documents}/../../Windows"))), "no \".\" or \"..\" in a path"),
        (with(f(), "/connector/out/dirs/0", Some(json!("mods/here"))), "must start with a placeholder ({documents}, {steam_app:ID}, ...) or be a full path"),
        (with(f(), "/connector/out/dirs/0", Some(json!("{documents}/x{home}"))), "a placeholder can only start a path"),
        (with(f(), "/connector/out/dirs/0", Some(json!("{steam_app}/x"))), "{steam_app}: steam_app needs a Steam app id"),
        (with(f(), "/connector/out/dirs/0", Some(json!("{steam_app:abc}/x"))), "needs a Steam app id"),
        (with(f(), "/connector/out/dirs/0", Some(json!("{env:A B}/x"))), "env needs a variable name"),
        (with(f(), "/connector/out/dirs/0", Some(json!("{documents}/a:b"))), "may not hold : * ?"),
        (with(f(), "/connector/out/dirs/0", Some(json!("\\\\server\\share"))), "must start with a placeholder"),
        (with(f(), "/connector/out/dirs/0", Some(json!(["{documents}", 5]))), "\"connector.out.dirs[0]\": candidates must all be paths"),
        (with(f(), "/connector/out/tag_dirs/0/dir", Some(json!(2))), "\"connector.out.tag_dirs[0].dir\": must be an index into \"connector.out.dirs\" (0 to 1)"),
        (with(f(), "/connector/out/prefx", Some(json!("a_b_"))), "unknown field \"connector.out.prefx\""),
        (with(f(), "/connector/feed/file", Some(json!("{documents}"))), "\"connector.feed.file\": must end in the file's name"),
        (with(f(), "/connector/feed/file", None), "\"connector.feed.file\" is missing"),
        (with(f(), "/connector/feed/pattern", Some(json!("(a)(b)"))), "\"connector.feed.pattern\": the regex must have exactly one group ( ... ), it has 2"),
        (with(f(), "/connector/feed/pattern", Some(json!("abc"))), "it has 0"),
        (with(f(), "/connector/feed/pattern", Some(json!("(unclosed"))), "\"connector.feed.pattern\": not a valid regex"),
        (with(f(), "/connector/feed/tag_pattern", Some(json!("x"))), "\"connector.feed.tag_pattern\": the regex must have exactly one group"),
        (with(f(), "/connector", Some(json!("files"))), "\"connector\": must be an object"),
    ];
    for (v, want) in cases {
        let e = err(&v);
        assert!(e.contains(want), "{v}\n  got:  {e}\n  want: {want}");
    }
    assert!(Profile::parse("{nope").unwrap_err().starts_with("not valid JSON: "));
    assert_eq!(Profile::parse("[1]").unwrap_err(), "a profile must be a JSON object { ... }");
    let big = format!("{{\"x\": \"{}\"}}", "a".repeat(70_000));
    assert!(Profile::parse(&big).unwrap_err().starts_with("too big for a profile"));
}

/// A stand-in for this PC: Documents and the Steam app are there, the Workshop and env var are not.
fn stand_in(root: PathBuf) -> impl Fn(&Place) -> Option<PathBuf> {
    move |p| match p {
        Place::Documents => Some(root.join("Docs")),
        Place::Home => Some(root.join("Home")),
        Place::SteamApp(4242) => Some(root.join("Steam").join("common").join("My Game")),
        Place::Env(n) if n == "REL" => Some(PathBuf::from("relative")),
        _ => None,
    }
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("kd-games-profile-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn placeholders_and_candidates() {
    let root = tmp("places");
    let look = stand_in(root.clone());
    let t = PathTemplate::parse("{documents}/My Game\\mods/").unwrap();
    assert_eq!(t.start, Some(Place::Documents));
    assert_eq!(t.parts, vec!["My Game", "mods"]);
    assert_eq!(t.resolve_with(&look), Some(root.join("Docs").join("My Game").join("mods")));
    assert_eq!(PathTemplate::parse("{steam_workshop:4242}").unwrap().resolve_with(&look), None, "not on this PC");
    assert_eq!(PathTemplate::parse("{env:REL}/x").unwrap().resolve_with(&look), None, "a relative result is no path");
    for (text, place) in [
        ("{localappdata}/x", Place::LocalAppData),
        ("{home}/x", Place::Home),
        ("{steam_app:7}/x", Place::SteamApp(7)),
        ("{steam_workshop:7}/x", Place::SteamWorkshop(7)),
        ("{proton_user:7}/x", Place::ProtonUser(7)),
        ("{env:MY_VAR}/x", Place::Env("MY_VAR".into())),
    ] {
        assert_eq!(PathTemplate::parse(text).unwrap().start, Some(place), "{text}");
    }
    // full paths: absolute on this system only
    let win = PathTemplate::parse("C:/Games/x").unwrap();
    let unix = PathTemplate::parse("/opt/games").unwrap();
    if cfg!(windows) {
        assert_eq!(win.resolve_with(&look), Some(PathBuf::from(r"C:\Games\x")));
        assert_eq!(unix.resolve_with(&look), None);
    } else {
        assert_eq!(unix.resolve_with(&look), Some(PathBuf::from("/opt/games")));
        assert_eq!(win.resolve_with(&look), None);
    }
    // the real lookup: an environment variable, home, Documents
    std::env::set_var("KD_GAMES_TEST_PLACE", &root);
    let t = PathTemplate::parse("{env:KD_GAMES_TEST_PLACE}/a/b.txt").unwrap();
    assert_eq!(t.resolve_with(&kd_games::profile::this_pc), Some(root.join("a").join("b.txt")));
    assert_eq!(t.file_name(), Some("b.txt"));
    assert_eq!(PathTemplate::parse("{env:KD_GAMES_TEST_UNSET}/a").unwrap().resolve_with(&kd_games::profile::this_pc), None);
    assert!(PathTemplate::parse("{home}").unwrap().resolve_with(&kd_games::profile::this_pc).is_some());
    assert!(PathTemplate::parse("{documents}").unwrap().resolve_with(&kd_games::profile::this_pc).is_some());

    // candidates: a file takes the first that resolves; a folder the first that resolves AND exists
    let spec = |v: Value| -> PathSpec {
        let p = with(files_profile(), "/connector/out/dirs/0", Some(v));
        let Connector::Files(c) = parse(&p).unwrap().connector else { panic!() };
        c.dirs[0].clone()
    };
    let s = spec(json!(["{steam_workshop:4242}/a.txt", "{documents}/b.txt", "{home}/c.txt"]));
    assert_eq!(s.resolve_file(&look), Some(root.join("Docs").join("b.txt")));
    assert_eq!(s.text(), "{steam_workshop:4242}/a.txt or {documents}/b.txt or {home}/c.txt");
    std::fs::create_dir_all(root.join("Home").join("mods")).unwrap();
    let s = spec(json!(["{steam_workshop:4242}/mods", "{documents}/mods", "{home}/mods"]));
    assert_eq!(s.resolve_dir(&look), Some(root.join("Home").join("mods")), "Docs/mods does not exist: skipped");
    std::fs::create_dir_all(root.join("Docs").join("mods")).unwrap();
    assert_eq!(s.resolve_dir(&look), Some(root.join("Docs").join("mods")));
    assert_eq!(spec(json!("{steam_workshop:4242}")).resolve_dir(&look), None);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn summary_lines() {
    let root = tmp("summary");
    let look = stand_in(root.clone());
    std::fs::create_dir_all(root.join("Docs").join("My Game").join("mods")).unwrap();
    let p = parse(&files_profile()).unwrap();
    let game = root.join("Steam").join("common").join("My Game");
    assert_eq!(
        p.summary_with(&look),
        vec![
            format!("finds My Game through Steam (app 4242): {}", game.display()),
            format!("reads {}", game.join("save").join("state.txt").display()),
            format!("writes its message files (talky_*) into {}", root.join("Docs").join("My Game").join("mods").display()),
            "writes its message files (talky_*) into {steam_workshop:4242} or {home}/ws (not found on this PC)".to_string(),
            "deletes only its own files there: talky_on, talky_p<n>, talky_t<n>.json, talky_w<n>.tmp".to_string(),
            "plays other players' voices".to_string(),
            "writes what you say (speech to text), while the game asks for the microphone".to_string(),
            "makes 1 test voices with Windows' speech voices".to_string(),
        ]
    );
    let none = p.summary_with(&|_| None);
    assert_eq!(none[0], "finds My Game through Steam (app 4242) (not found on this PC)");
    assert_eq!(none[1], "reads {steam_app:4242}/save/state.txt or {env:MY_GAME_SAVE}/state.txt (not found on this PC)");
    let s = parse(&socket_profile()).unwrap();
    assert_eq!(
        s.summary_with(&look),
        vec![
            "listens on 127.0.0.1:47120 (this computer only)".to_string(),
            "plays other players' voices".to_string(),
            "writes what you say (speech to text), while the game asks for the microphone".to_string(),
        ]
    );
    // (the real one: Teardown's, whatever this PC has - no panic, each line says something)
    for line in kd_games::teardown::profile().summary() {
        println!("{line}");
        assert!(!line.is_empty());
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A test voice's text and voice name reach PowerShell only as data: quotes (also the curly ones PowerShell takes
/// as quotes) and $(...) run nothing. Windows (other systems make no voices).
#[test]
fn test_voice_text_is_not_code() {
    let d = tmp("voices");
    let pwned = d.join("pwned.txt");
    let p = pwned.display().to_string();
    let evil = format!("hi'; New-Item -ItemType File -Path '{p}'; ' \u{2019}; New-Item -ItemType File -Path \u{2019}{p}\u{2019}; \u{2019} $(New-Item -ItemType File -Path '{p}')");
    let v = kd_games::profile::TestVoice { src: 1, voice: format!("x'; New-Item -ItemType File -Path '{p}'; '"), rate: 0, text: evil };
    let got = kd_games::voices::make_in(&d, &[v], &|v| format!("voice{}.wav", v.src));
    assert!(!pwned.exists(), "the text ran as code");
    if cfg!(windows) {
        assert!(got.contains_key(&1), "the voice is made (it reads the text out)");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// The example profile shipped for mod authors (examples/profiles/) and the one in PROTOCOL.md are valid.
#[test]
fn the_example_profile() {
    let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../PROTOCOL.md")).unwrap().replace("\r\n", "\n");
    let section = doc.split("## Adding a game mod: profiles").nth(1).expect("PROTOCOL.md has the profiles section");
    let block = section.split("```json\n").nth(1).unwrap().split("```").next().unwrap();
    let p = Profile::parse(block).unwrap_or_else(|e| panic!("PROTOCOL.md's example: {e}"));
    assert_eq!(p.id, "my-game-talky");
    let Connector::Files(c) = &p.connector else { panic!() };
    let save = b"<save><mods><steam-77><talky>\n<f value=\"4|1|1|1|0|1|1|en|1|\"/></talky></steam-77></mods></save>\n";
    let found = kd_games::files::FeedRules::from_config(c).find(save);
    assert_eq!(found, vec![("steam-77".to_string(), "4|1|1|1|0|1|1|en|1|".to_string())]);
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../examples/profiles/example-socket.json");
    let p = Profile::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!((p.id.as_str(), p.game.as_str()), ("example-game-example-mod", "Example Game"));
    assert!(matches!(p.connector, Connector::Socket(ref c) if c.port == 47120));
}
