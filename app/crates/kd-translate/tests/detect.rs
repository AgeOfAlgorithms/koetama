//! detect.rs on real sentences: the NTREX-128 test sentences of bench/mt/data (the same news sentences in all 29 of
//! Koetama's languages; skipped when the folder is not there), single-language lines a player could type, and
//! mixed-language lines (each stretch in its own language, in order).
//!     cargo test -p kd-translate --test detect -- --nocapture     (prints the accuracy per language)
use kd_translate::detect::{detect, stretches, LANGS};
use std::path::PathBuf;

fn ntrex_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../bench/mt/data")
}

/// (language, lines) of the NTREX sentences there are.
fn ntrex() -> Vec<(&'static str, Vec<String>)> {
    let mut out = Vec::new();
    for l in LANGS {
        if let Ok(text) = std::fs::read_to_string(ntrex_dir().join(format!("{l}.txt"))) {
            out.push((l, text.lines().filter(|s| !s.trim().is_empty()).map(str::to_string).collect()));
        }
    }
    out
}

/// A line is right when it comes out as ONE stretch in its language (a split single-language line is wrong too).
fn right(line: &str, lang: &str) -> bool {
    let st = stretches(line);
    st.len() == 1 && st[0].lang == Some(lang)
}

#[test]
fn ntrex_single_language_lines() {
    let data = ntrex();
    if data.is_empty() {
        eprintln!("bench/mt/data is not here: skipped");
        return;
    }
    assert_eq!(data.len(), 29, "every language has its file");
    let (mut all, mut ok_all, mut main_all) = (0, 0, 0);
    let mut report = Vec::new();
    for (lang, lines) in &data {
        let n = lines.len();
        let ok = lines.iter().filter(|s| right(s, lang)).count();
        // (the line's main language right, even when a part of it was split off)
        let main = lines.iter().filter(|s| detect(s) == Some(lang)).count();
        let wrong: Vec<String> = lines
            .iter()
            .filter(|s| !right(s, lang))
            .take(3)
            .map(|s| {
                let parts: Vec<String> =
                    stretches(s).iter().map(|x| format!("{}:{:?}", x.lang.unwrap_or("?"), x.text(s))).collect();
                parts.join(" | ")
            })
            .collect();
        report.push(format!(
            "{lang:>3}: {ok:>3}/{n} one stretch right, main language {main:>3}/{n}  {}",
            wrong.join("  //  ")
        ));
        all += n;
        ok_all += ok;
        main_all += main;
        // (Cantonese written with few Cantonese-only characters reads as Chinese: no floor for it)
        if *lang != "yue" {
            assert!(ok * 100 >= n * 85, "{lang}: only {ok}/{n} lines right\n{}", report.last().unwrap());
        }
    }
    println!("{}", report.join("\n"));
    println!(
        "all: {ok_all}/{all} ({:.1} %) lines one stretch in the right language; main language right {main_all}/{all} ({:.1} %)",
        ok_all as f64 * 100.0 / all as f64,
        main_all as f64 * 100.0 / all as f64
    );
    assert!(ok_all * 100 >= all * 95, "overall {ok_all}/{all}");
}

/// Chat-like lines, one language each.
#[test]
fn chat_lines() {
    let cases = [
        ("en", "Does anyone know where the key for the red door is?"),
        ("es", "¿Alguien sabe dónde está la llave de la puerta roja?"),
        ("fr", "Quelqu'un sait où est la clé de la porte rouge ?"),
        ("de", "Weiß jemand, wo der Schlüssel für die rote Tür ist?"),
        ("it", "Qualcuno sa dove si trova la chiave della porta rossa?"),
        ("pt", "Alguém sabe onde está a chave da porta vermelha?"),
        ("nl", "Weet iemand waar de sleutel van de rode deur is?"),
        ("pl", "Czy ktoś wie, gdzie jest klucz do czerwonych drzwi?"),
        ("uk", "Хтось знає, де ключ від червоних дверей?"),
        ("ru", "Кто-нибудь знает, где ключ от красной двери?"),
        ("zh", "有人知道红门的钥匙在哪里吗？"),
        ("yue", "有冇人知道紅色嗰度門嘅鎖匙喺邊度？"),
        ("ja", "赤いドアの鍵がどこにあるか知っている人はいますか？"),
        ("ko", "빨간 문 열쇠가 어디 있는지 아는 사람 있어요?"),
        ("cs", "Neví někdo, kde je klíč od červených dveří?"),
        ("sk", "Nevie niekto, kde je kľúč od červených dverí?"),
        ("ro", "Știe cineva unde este cheia de la ușa roșie?"),
        ("hr", "Zna li netko gdje je ključ od crvenih vrata?"),
        ("bg", "Някой знае ли къде е ключът за червената врата?"),
        ("fi", "Tietääkö kukaan, missä punaisen oven avain on?"),
        ("sv", "Vet någon var nyckeln till den röda dörren är?"),
        ("hu", "Tudja valaki, hol van a piros ajtó kulcsa?"),
        ("da", "Er der nogen, der ved, hvor nøglen til den røde dør er?"),
        ("et", "Kas keegi teab, kus on punase ukse võti?"),
        ("lv", "Vai kāds zina, kur ir sarkano durvju atslēga?"),
        ("lt", "Ar kas nors žino, kur yra raudonų durų raktas?"),
        ("sl", "Ali kdo ve, kje je ključ od rdečih vrat?"),
        ("el", "Ξέρει κανείς πού είναι το κλειδί για την κόκκινη πόρτα;"),
        ("mt", "Xi ħadd jaf fejn hu ċ-ċavetta tal-bieb l-aħmar?"),
    ];
    let mut wrong = Vec::new();
    for (lang, line) in cases {
        if !right(line, lang) {
            wrong.push(format!(
                "{lang}: {:?}",
                stretches(line).iter().map(|s| (s.lang, s.text(line))).collect::<Vec<_>>()
            ));
        }
    }
    println!("chat lines: {}/{} right", cases.len() - wrong.len(), cases.len());
    // (one or two close neighbours may swap on one short line; more is a regression)
    assert!(wrong.len() <= 2, "{}", wrong.join("\n"));
}

fn parts(line: &str) -> Vec<(Option<&'static str>, String)> {
    stretches(line).iter().map(|s| (s.lang, s.text(line).trim().to_string())).collect()
}

/// Mixed lines: each stretch in its language, in order.
#[test]
fn mixed_lines() {
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "I think we should go upstairs now. 上の階に宝物があるはずです。",
            vec![("en", "I think we should go upstairs now."), ("ja", "上の階に宝物があるはずです。")],
        ),
        (
            "こんにちは！ How is everyone doing today?",
            vec![("ja", "こんにちは！"), ("en", "How is everyone doing today?")],
        ),
        (
            "Vamos a la cocina ahora mismo, and then we can look for the key upstairs.",
            vec![("es", "Vamos a la cocina ahora mismo,"), ("en", "and then we can look for the key upstairs.")],
        ),
        (
            "오늘 같이 게임할 사람? Anyone want to play together tonight?",
            vec![("ko", "오늘 같이 게임할 사람?"), ("en", "Anyone want to play together tonight?")],
        ),
        (
            "Привет всем, как у вас дела? I just joined the server.",
            vec![("ru", "Привет всем, как у вас дела?"), ("en", "I just joined the server.")],
        ),
        (
            "这个房间太危险了 let's get out of here quickly",
            vec![("zh", "这个房间太危险了"), ("en", "let's get out of here quickly")],
        ),
        (
            "Ich habe den Schlüssel gefunden. Je vais ouvrir la porte maintenant.",
            vec![("de", "Ich habe den Schlüssel gefunden."), ("fr", "Je vais ouvrir la porte maintenant.")],
        ),
        (
            "Καλησπέρα σε όλους! Good evening everyone, welcome back.",
            vec![("el", "Καλησπέρα σε όλους!"), ("en", "Good evening everyone, welcome back.")],
        ),
        // (short Latin inside another script: the line's language - translated with it)
        ("このiPhoneは高いです", vec![("ja", "このiPhoneは高いです")]),
        ("有人提议应将 AM 的头衔改为MWP。", vec![("zh", "有人提议应将 AM 的头衔改为MWP。")]),
        ("Привет, John! Как дела у тебя сегодня?", vec![("ru", "Привет, John! Как дела у тебя сегодня?")]),
        // (a short clause keeps the line's language: too short to tell)
        (
            "ok, I will go and check the basement right now",
            vec![("en", "ok, I will go and check the basement right now")],
        ),
    ];
    for (line, want) in cases {
        let got = parts(line);
        let want: Vec<(Option<&str>, String)> = want.iter().map(|(l, t)| (Some(*l), t.to_string())).collect();
        assert_eq!(got, want, "{line}");
    }
}

/// Short chat lines are where trigrams fail ("Vamos a jugar otra vez" read as Hungarian): told among the languages
/// the player translates first; a line in none of them stays out of them.
#[test]
fn short_lines_prefer_the_translations_languages() {
    let lang = |line: &str, prefer: &[&str]| {
        let s = kd_translate::detect::stretches_preferring(line, prefer);
        assert_eq!(s.len(), 1, "{line}: {s:?}");
        s[0].lang
    };
    for line in ["Vamos a jugar otra vez", "Tengo dos ovejas", "Me voy a dormir", "Ven aquí rápido", "Tengo hambre"] {
        assert_eq!(lang(line, &["es", "en"]), Some("es"), "{line}");
    }
    assert_eq!(lang("Attenti al drago.", &["it", "en"]), Some("it"));
    assert_ne!(lang("Je suis là, derrière toi", &["es", "en"]), Some("es"), "French is not made Spanish");
    assert_eq!(lang("ok let's go then", &["es", "en"]), Some("en"));
    // (a long, sure line keeps its own language)
    assert_eq!(lang("Ich habe den Schlüssel im Keller gefunden, aber die Tür ist immer noch zu.", &["es", "en"]), Some("de"));
}
