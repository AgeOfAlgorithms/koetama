//! The feed and the message files against the Python answers (app/fixtures/feed.json, make_fixtures.py feed_cases):
//! parse_feed (versions 4/3/2, what is refused), find_feeds (each feed with its copy of the mod), text_prefab
//! byte for byte, times_hex. Plus test_helper.py's feed checks.
use kd_games::teardown::{find_feeds, parse_feed, text_prefab, times_hex, TEXT_MAX};
use serde_json::Value;

fn fixture() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/feed.json");
    let text = std::fs::read_to_string(path).unwrap();
    // (Python's json writes NaN, serde_json reads no such thing: null stands for it here)
    serde_json::from_str(&text.replace(":NaN", ":null")).unwrap()
}

fn same_f64(want: &Value, got: f64) -> bool {
    match want.as_f64() {
        Some(w) => w == got,
        None => got.is_nan(),
    }
}

#[test]
fn parse_as_python() {
    let fx = fixture();
    let cases = fx["parse"].as_array().unwrap();
    assert_eq!(cases.len(), 11);
    for c in cases {
        let text = c["text"].as_str().unwrap();
        let got = parse_feed(text);
        let want = &c["feed"];
        if want.is_null() {
            assert!(got.is_none(), "{text:?} should be refused: {got:?}");
            continue;
        }
        let f = got.unwrap_or_else(|| panic!("{text:?} should parse"));
        assert_eq!(f.seq, want["seq"].as_i64().unwrap(), "{text}");
        assert!(same_f64(&want["vol"], f.vol), "{text}");
        assert_eq!(f.sid, want["sid"].as_i64().unwrap(), "{text}");
        assert_eq!(f.ack, want["ack"].as_i64().unwrap(), "{text}");
        assert_eq!(f.ping, want["ping"].as_i64().unwrap(), "{text}");
        assert_eq!(f.mic, want["mic"].as_bool().unwrap(), "{text}");
        assert_eq!(f.lang, want["lang"].as_str().unwrap(), "{text}");
        assert_eq!(f.live, want["live"].as_bool().unwrap(), "{text}");
        let sp = want["speakers"].as_object().unwrap();
        assert_eq!(f.speakers.len(), sp.len(), "{text}");
        for (k, v) in sp {
            let s = &f.speakers[&k.parse::<i64>().unwrap()];
            assert_eq!(s.src, v["src"].as_i64().unwrap());
            assert_eq!(s.talk, v["talk"].as_bool().unwrap());
            assert!(same_f64(&v["gain"], s.gain), "{text}: gain {}", s.gain);
            assert!(same_f64(&v["az"], s.az));
            assert!(same_f64(&v["el"], s.el));
            assert!(same_f64(&v["muffle"], s.muffle));
        }
    }
}

#[test]
fn find_as_python() {
    let fx = fixture();
    for c in fx["find"].as_array().unwrap() {
        let got = find_feeds(c["xml"].as_str().unwrap().as_bytes());
        let want: Vec<(String, String)> = c["feeds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p[0].as_str().unwrap().to_string(), p[1].as_str().unwrap().to_string()))
            .collect();
        assert_eq!(got, want);
    }
}

#[test]
fn prefabs_byte_identical() {
    let fx = fixture();
    for c in fx["prefab"].as_array().unwrap() {
        let times: Option<Vec<f64>> = c["times"].as_array().map(|a| a.iter().map(|v| v.as_f64().unwrap()).collect());
        let got = text_prefab(
            c["text"].as_str().unwrap(),
            c["kind"].as_str().unwrap().chars().next().unwrap(),
            c["utt"].as_u64().unwrap() as u32,
            times.as_deref(),
            c["ago"].as_f64(),
        );
        assert_eq!(got, c["out"].as_str().unwrap());
    }
    assert_eq!(fx["TEXT_MAX"].as_u64().unwrap() as usize, TEXT_MAX);
}

#[test]
fn times_hex_as_python() {
    let fx = fixture();
    for c in fx["times_hex"].as_array().unwrap() {
        let t: Vec<f64> = c["times"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert_eq!(times_hex(&t), c["out"].as_str().unwrap(), "{t:?}");
    }
}

/// test_helper.py's feed checks
#[test]
fn helper_feed_checks() {
    let f = parse_feed("2|42|0.50|7|3|12|1|2000,1,0,0.550,-39.8,0.0,0.00;2001,2,1,1.000,0.0,-3.5,0.25").unwrap();
    assert!(f.seq == 42 && f.vol == 0.5 && f.sid == 7 && f.ack == 3 && f.ping == 12 && f.mic && f.speakers.len() == 2);
    let s = &f.speakers[&2001];
    assert!(s.src == 2 && s.talk && s.gain == 1.0 && s.az == 0.0 && s.el == -3.5 && s.muffle == 0.25);
    assert!(!f.speakers[&2000].talk);
    let f = parse_feed("2|43|1.00|7|0|1|0|").unwrap();
    assert!(f.speakers.is_empty() && !f.mic && f.lang == "en");
    let f4 = parse_feed("4|45|1.00|7|0|1|1|zh|0|").unwrap();
    assert!(f4.lang == "zh" && !f4.live && parse_feed("4|45|1.00|7|0|1|1|zh|1|").unwrap().live);
    let f = parse_feed("3|44|1.00|7|0|1|1|ru|2000,1,1,1.000,0.0,0.0,0.00").unwrap();
    assert!(f.lang == "ru" && f.mic && f.speakers.len() == 1);
    assert!(parse_feed("1|1|1.00|").is_none() && parse_feed("garbage").is_none() && parse_feed("2|x|1|1|1|1|1|").is_none());
    let (a, b) = ("2|5|1.00|7|0|1|0|2000,1,1,1.000,0.0,0.0,0.00", "2|9|1.00|3|0|1|0|");
    let xml = format!(
        "<registry version=\"2.1.0\">\n<savegame><mod>\n<local-proximity-chat>\n<pcmode value=\"s\"/>\n<pcvx>\n\t<f value=\"{a}\"/>\n</pcvx>\n\
         </local-proximity-chat>\n<steam-123>\n<pcvx>\n<f value=\"{b}\"/>\n</pcvx>\n</steam-123>\n</mod></savegame>\n</registry>\n"
    );
    assert_eq!(
        find_feeds(xml.as_bytes()),
        vec![("local-proximity-chat".to_string(), a.to_string()), ("steam-123".to_string(), b.to_string())]
    );
    // (bytes past ASCII read as U+FFFD, as Python's decode('ascii', 'replace'); never a panic)
    let odd = b"<steam-\xff1><pcvx><f value=\"2|1|1|1|0|1|0|\xe4\"/></pcvx>";
    let got = find_feeds(odd);
    assert_eq!(got, vec![("steam-\u{FFFD}1".to_string(), "2|1|1|1|0|1|0|\u{FFFD}".to_string())]);
    assert!(parse_feed(&got[0].1).is_none());
}
