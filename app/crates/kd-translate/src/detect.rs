//! Which language each stretch of a chat line is in (PROTOCOL.md "Translation": mixed-language lines).
//!
//! A line is first cut by SCRIPT: Han, kana, Hangul, Latin, Cyrillic, Greek, other letters; punctuation, digits,
//! spaces and symbols belong to the stretch next to them. Some scripts name the language: Han in a line with kana is
//! Japanese, Hangul Korean, Greek Greek, Han without kana Chinese (Cantonese when the line holds a character only
//! written Cantonese uses: 嘅 咗 喺 哋 ...). Latin and Cyrillic text goes to a small detector (whatlang's trigram
//! profiles, restricted to Koetama's languages, helped by letters a close neighbour never uses, by the little words of
//! a short line, and a Croatian / Slovene word vote; Maltese, which whatlang does not know, by ħ ċ ġ, "għ" or its
//! hyphenated articles). Within one script a line that changes language at a clause or sentence boundary (". ", "! ",
//! "? ", ", ", ...) is split further when the parts clearly detect differently (part_language). Conservative: a
//! stretch under MIN_WORDS words (a name, "ok", "lol") - or, among other scripts, of mostly capitalised words - takes
//! the line's main language: the stretch around it is translated with it.
//!
//! The answer covers the whole line, in order: [Stretch { byte range, language or None (unknown) }].
use std::sync::LazyLock;
use whatlang::{Detector, Lang};

/// Koetama's languages (kd_speech::LANGS: the languages a player can speak).
pub const LANGS: [&str; 29] = [
    "en", "es", "fr", "de", "it", "pt", "nl", "pl", "uk", "ru", "zh", "yue", "ja", "ko", "cs", "sk", "ro", "hr", "bg",
    "fi", "sv", "hu", "da", "et", "lv", "lt", "sl", "el", "mt",
];

/// a Latin / Cyrillic stretch with fewer words takes the line's main language (too short to tell reliably)
pub const MIN_WORDS: usize = 3;
/// a clause is split off its run only when it has at least this many words not capitalised (names do not count) ...
pub const SPLIT_WORDS: usize = 5;
/// ... the detector is at least this sure of it (whatlang's confidence: 0..1, from the gap to the runner-up; measured
/// on the NTREX sentences: 0.35 lets ~10 of 2900 single-language lines split wrongly, where two languages of different
/// families meet) ...
pub const SPLIT_SURE: f64 = 0.35;
/// ... and its language is not KIN to the run's: close relatives are told apart on whole lines, never on clauses (the
/// trigrams of a few words of Slovene and Croatian, or Russian and Bulgarian, are too alike).
const KIN: [&[&str]; 8] = [
    &["es", "pt", "it", "fr", "ro", "mt"],
    &["cs", "sk", "pl"],
    &["hr", "sl"],
    &["ru", "uk", "bg"],
    &["da", "sv"],
    &["fi", "et"],
    &["lv", "lt"],
    &["de", "nl"],
];

fn kin(a: &str, b: &str) -> bool {
    KIN.iter().any(|k| k.contains(&a) && k.contains(&b))
}

/// One stretch of a line: line[start..end] (bytes), in `lang` (None: unknown - no letters, or a script Koetama has
/// no language for).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stretch {
    pub start: usize,
    pub end: usize,
    pub lang: Option<&'static str>,
}

impl Stretch {
    pub fn text<'a>(&self, line: &'a str) -> &'a str {
        &line[self.start..self.end]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Script {
    Han,
    Kana,
    Hangul,
    Latin,
    Cyrillic,
    Greek,
    /// letters of another script (Arabic, Thai, ...): no language of Koetama's
    Other,
    /// not a letter: punctuation, digits, spaces, symbols, emoji, combining marks
    Neutral,
}

fn in_ranges(c: u32, ranges: &[(u32, u32)]) -> bool {
    ranges.iter().any(|&(a, b)| a <= c && c <= b)
}

fn script(ch: char) -> Script {
    let c = ch as u32;
    const HAN: [(u32, u32); 7] = [
        (0x3400, 0x4DBF),
        (0x4E00, 0x9FFF),
        (0xF900, 0xFAFF),
        (0x20000, 0x2A6DF),
        (0x2A700, 0x2EBEF),
        (0x2F800, 0x2FA1F),
        (0x30000, 0x3134F),
    ];
    // (々 the iteration mark and 〆: Japanese)
    const KANA: [(u32, u32); 6] =
        [(0x3040, 0x309F), (0x30A0, 0x30FF), (0x31F0, 0x31FF), (0xFF66, 0xFF9F), (0x1B000, 0x1B16F), (0x3005, 0x3006)];
    const HANGUL: [(u32, u32); 6] =
        [(0xAC00, 0xD7AF), (0x1100, 0x11FF), (0x3130, 0x318F), (0xA960, 0xA97F), (0xD7B0, 0xD7FF), (0xFFA0, 0xFFDC)];
    const CYRILLIC: [(u32, u32); 4] = [(0x0400, 0x052F), (0x1C80, 0x1C8F), (0x2DE0, 0x2DFF), (0xA640, 0xA69F)];
    const GREEK: [(u32, u32); 2] = [(0x0370, 0x03FF), (0x1F00, 0x1FFF)];
    const LATIN: [(u32, u32); 8] = [
        (0x0041, 0x024F),
        (0x0250, 0x02AF),
        (0x1D00, 0x1DBF),
        (0x1E00, 0x1EFF),
        (0x2C60, 0x2C7F),
        (0xA720, 0xA7FF),
        (0xAB30, 0xAB6F),
        (0xFF21, 0xFF5A),
    ];
    if in_ranges(c, &KANA) {
        return Script::Kana;
    }
    if c == 0x3007 || in_ranges(c, &HAN) {
        return Script::Han;
    }
    if !ch.is_alphabetic() {
        return Script::Neutral;
    }
    if in_ranges(c, &HANGUL) {
        Script::Hangul
    } else if in_ranges(c, &LATIN) {
        Script::Latin
    } else if in_ranges(c, &CYRILLIC) {
        Script::Cyrillic
    } else if in_ranges(c, &GREEK) {
        Script::Greek
    } else {
        Script::Other
    }
}

/// Scripts that make one run together (Han and kana: Japanese mixes them in every sentence).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Group {
    Cjk,
    Hangul,
    Latin,
    Cyrillic,
    Greek,
    Other,
}

fn group(s: Script) -> Option<Group> {
    match s {
        Script::Han | Script::Kana => Some(Group::Cjk),
        Script::Hangul => Some(Group::Hangul),
        Script::Latin => Some(Group::Latin),
        Script::Cyrillic => Some(Group::Cyrillic),
        Script::Greek => Some(Group::Greek),
        Script::Other => Some(Group::Other),
        Script::Neutral => None,
    }
}

/// Characters only written Cantonese uses (standard written Chinese does not): one makes a Han stretch Cantonese.
const CANTONESE: &str = "嘅咗喺哋嘢啲冇嚟佢唔乜嘞啱瞓搵揾噉㗎喎囉咩嗰睇嘥攰諗啩嗮冚唞嚿揸㖭嚡乸冧氹畀俾";

/// Maltese letters no other language of Koetama's uses.
const MALTESE_LETTERS: &str = "ħĦċĊġĠ";

/// An opening mark: it belongs to the text after it ("(", "「", "¿", ...).
fn opening(c: char) -> bool {
    "([{«“‘„‚‹「『【〔〈《（［｛〖〘〚¿¡".contains(c)
}

/// The languages the detector chooses among: (Koetama's code, whatlang's, the letters that rule it out - letters its
/// close neighbours use and it never does). Maltese, which whatlang does not know: by its letters (maltese()).
const LATIN: [(&str, Lang, &str); 20] = [
    ("en", Lang::Eng, ""),
    ("es", Lang::Spa, "ãõçâêôàèășț"),
    ("fr", Lang::Fra, "ñășț"),
    ("de", Lang::Deu, ""),
    ("it", Lang::Ita, "ñășț"),
    ("pt", Lang::Por, "ñ¿¡ășț"),
    ("nl", Lang::Nld, ""),
    ("pl", Lang::Pol, ""),
    ("cs", Lang::Ces, "äľĺŕôą"),
    ("sk", Lang::Slk, "ěřů"),
    ("ro", Lang::Ron, ""),
    ("hr", Lang::Hrv, ""),
    ("fi", Lang::Fin, ""),
    ("sv", Lang::Swe, "æø"),
    ("hu", Lang::Hun, "ășțâîãõ"),
    ("da", Lang::Dan, "äö"),
    ("et", Lang::Est, ""),
    ("lv", Lang::Lav, ""),
    ("lt", Lang::Lit, ""),
    ("sl", Lang::Slv, "ćđ"),
];
/// (Ukrainian has і ї є ґ and no ы э ъ ё; Bulgarian none of ы э ё і ї є ґ)
const CYRILLIC: [(&str, Lang, &str); 3] =
    [("ru", Lang::Rus, "іїєґ"), ("uk", Lang::Ukr, "ыэъё"), ("bg", Lang::Bul, "ыэёіїєґ")];

/// A detector for a set of candidates.
type Kept = (Vec<Lang>, Detector);

/// One detector per set of candidates (most texts rule out none: the full set).
static DETECTORS: LazyLock<std::sync::Mutex<Vec<Kept>>> = LazyLock::new(Default::default);

fn detect_among(text: &str, langs: Vec<Lang>) -> Option<whatlang::Info> {
    let mut cache = DETECTORS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, d)) = cache.iter().find(|(l, _)| *l == langs) {
        return d.detect(text);
    }
    let d = Detector::with_allowlist(langs.clone());
    let info = d.detect(text);
    if cache.len() < 64 {
        cache.push((langs, d));
    }
    info
}

/// Croatian and Slovenian are close: a few words only one of them writes decide when they disagree with the trigrams.
const CROATIAN_WORDS: [&str; 16] = [
    "i", "koji", "koja", "koje", "kao", "također", "su", "nije", "bio", "zbog", "što", "će", "vrlo", "sam", "samo",
    "kod",
];
const SLOVENIAN_WORDS: [&str; 16] =
    ["in", "ki", "kot", "tudi", "so", "ni", "bil", "zaradi", "kar", "kaj", "bo", "zelo", "sem", "samo", "pri", "pa"];

fn croatian_or_slovenian(text: &str, guess: &'static str) -> &'static str {
    let lower = text.to_lowercase();
    let ws: Vec<&str> = lower.split(|c: char| !c.is_alphabetic()).filter(|w| !w.is_empty()).collect();
    let hr = ws.iter().filter(|w| CROATIAN_WORDS.contains(w) && !SLOVENIAN_WORDS.contains(w)).count();
    let sl = ws.iter().filter(|w| SLOVENIAN_WORDS.contains(w) && !CROATIAN_WORDS.contains(w)).count();
    match hr.cmp(&sl) {
        std::cmp::Ordering::Greater => "hr",
        std::cmp::Ordering::Less => "sl",
        std::cmp::Ordering::Equal => guess,
    }
}

/// Maltese articles and prepositions joined to the next word with a hyphen ("il-kamra", "tal-bieb", "fit-triq").
const MALTESE_ARTICLES: [&str; 30] = [
    "il", "l", "it", "id", "ir", "is", "ix", "iż", "iz", "in", "iċ", "tal", "tat", "tad", "tar", "tas", "tax", "taż",
    "fil", "fit", "fl", "bil", "bit", "mill", "mit", "lill", "sal", "mal", "għall", "għat",
];

/// Maltese: one of its own letters (ħ ċ ġ), its "għ", or two of its hyphenated articles.
fn maltese(text: &str) -> bool {
    if text.chars().any(|c| MALTESE_LETTERS.contains(c)) {
        return true;
    }
    let lower = text.to_lowercase();
    if lower.contains("għ") {
        return true;
    }
    let mut articles = 0;
    for w in lower.split(|c: char| c.is_whitespace() || "\"'“”‘’„«»(),.;:!?".contains(c)) {
        if let Some((head, tail)) = w.split_once('-') {
            if MALTESE_ARTICLES.contains(&head) && tail.chars().next().is_some_and(char::is_alphabetic) {
                articles += 1;
            }
        }
    }
    articles >= 2
}

/// Words in a text: runs of letters (digits and punctuation do not count).
fn words(text: &str) -> usize {
    let mut n = 0;
    let mut inside = false;
    for c in text.chars() {
        let letter = c.is_alphabetic();
        if letter && !inside {
            n += 1;
        }
        inside = letter;
    }
    n
}

/// Words that do not start with a capital letter (names and titles do).
fn lower_words(text: &str) -> usize {
    text.split(|c: char| !c.is_alphabetic()).filter(|w| w.chars().next().is_some_and(|c| !c.is_uppercase())).count()
}

/// The language of a Latin or Cyrillic text, and how sure (0..1).
fn detect_alphabetic(text: &str, g: Group) -> (Option<&'static str>, f64) {
    detect_alphabetic_with(text, g, true)
}

/// detect_alphabetic, the function words used (or not: a clause is split off only on the trigrams' word).
fn detect_alphabetic_with(text: &str, g: Group, function_words: bool) -> (Option<&'static str>, f64) {
    if g == Group::Latin && maltese(text) {
        return (Some("mt"), 1.0);
    }
    let table: &[(&'static str, Lang, &str)] = if g == Group::Cyrillic { &CYRILLIC } else { &LATIN };
    let lower = text.to_lowercase();
    let mut allowed: Vec<&(&'static str, Lang, &str)> =
        table.iter().filter(|(_, _, no)| !lower.chars().any(|c| no.contains(c))).collect();
    if allowed.is_empty() {
        allowed = table.iter().collect();
    }
    if allowed.len() == 1 {
        return (Some(allowed[0].0), 1.0);
    }
    let Some(info) = detect_among(text, allowed.iter().map(|t| t.1).collect()) else {
        return (None, 0.0);
    };
    let mut code = table.iter().find(|t| t.1 == info.lang()).map(|t| t.0);
    if function_words && g == Group::Latin && info.confidence() < FUNCTION_WORDS_BELOW {
        // (the trigrams unsure - a short chat line: the little words decide when they clearly point elsewhere)
        let allowed_codes: Vec<&str> = allowed.iter().map(|t| t.0).collect();
        let hits = |l: &str| function_word_hits(&lower, l);
        let best = allowed_codes.iter().map(|&l| (l, hits(l))).max_by_key(|x| x.1);
        if let Some((l, n)) = best {
            if n >= 2 && code.is_none_or(|c| n >= hits(c) + 2) {
                code = LATIN.iter().find(|t| t.0 == l).map(|t| t.0);
            }
        }
    }
    let code = match code {
        Some(c @ ("hr" | "sl")) if allowed.iter().any(|t| t.0 == "hr") && allowed.iter().any(|t| t.0 == "sl") => {
            Some(croatian_or_slovenian(text, c))
        }
        c => c,
    };
    // (a short or unsure text whose language is not one the line is likely in - the player's own, the one chat is
    //  translated into: those first - "ok" or "lol" is the player's, not a language picked from three letters. The
    //  letters still rule a language out: one with a letter it never writes ("¿Dónde estás?" is not English), or
    //  whose little words are clearly fewer than the detected language's ("Buenas noches a todos los jugadores")
    let preferred = PREFERRED.with(|p| p.borrow().clone());
    if !preferred.is_empty()
        && code.is_none_or(|c| !preferred.contains(&c))
        && (words(text) < PREFER_WORDS || info.confidence() < PREFER_BELOW)
    {
        // (the detected language's little words in it - counted from MIN_WORDS words: "no" is English too)
        let most = match code {
            Some(c) if g == Group::Latin && words(text) >= MIN_WORDS => function_word_hits(&lower, c),
            _ => 0,
        };
        let plausible = |l: &str| g != Group::Latin || (writes_all(l, &lower) && most <= function_word_hits(&lower, l));
        let likely: Vec<&(&'static str, Lang, &str)> =
            allowed.iter().copied().filter(|t| preferred.contains(&t.0) && plausible(t.0)).collect();
        match likely.len() {
            0 => {}
            1 => return (Some(likely[0].0), info.confidence()),
            _ => {
                if let Some(again) = detect_among(text, likely.iter().map(|t| t.1).collect()) {
                    return (table.iter().find(|t| t.1 == again.lang()).map(|t| t.0), again.confidence());
                }
            }
        }
    }
    (code, info.confidence())
}

/// a Latin / Cyrillic text shorter than this (words), or detected less surely than PREFER_BELOW, is detected again
/// among the PREFERRED languages when its language is not one of them (stretches_preferring)
pub const PREFER_WORDS: usize = 6;
pub const PREFER_BELOW: f64 = 0.5;

thread_local! {
    /// the languages a line is most likely in (the player's own and the one chat is translated into; empty: none) - set by
    /// stretches_preferring for the time of one line
    static PREFERRED: std::cell::RefCell<Vec<&'static str>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// below this confidence of the trigrams, the function words get a say
const FUNCTION_WORDS_BELOW: f64 = 0.5;

/// The commonest little words of each Latin language (a short chat line has too few trigrams; it has these).
const FUNCTION_WORDS: [(&str, &str); 20] = [
    ("en", "the you your and are is was what this that with have get here there just don't can't i'm it's let's it we they he she my me to for not but out how why where when will would can do go i of"),
    ("es", "el la los las que de y en un una es no por para con lo se del al como pero más está qué yo tú muy hay eso a todos hola"),
    ("fr", "le la les de des et est un une je tu il elle nous vous ne pas que qui dans pour avec ce ça c'est sur mais au du on j'ai"),
    ("de", "der die das und ist nicht ich du er sie wir ihr ein eine zu mit auf für den dem auch es was wie aber noch hier mal bin"),
    ("it", "il lo la gli le di e è che non un una per con del della sono ho io tu ma come questo qui anche cosa perché"),
    ("pt", "o a os as de e é que não um uma em para com do da eu você mas está isso muito por se no na"),
    ("nl", "de het een en is niet ik je jij wij we dat van op te met voor maar ook er zijn wat hoe hier nog naar dit kan"),
    ("pl", "i w na nie się to jest że z do jak co ja ty ale tak mi czy jestem już tylko"),
    ("cs", "a je se na v to že s z do jak ale jsem není co já ty tak už jen by ve"),
    ("sk", "a je sa na v to že s z do ako ale som nie čo ja ty tak už len by vo"),
    ("ro", "și de la în nu este un o cu pe că ce eu tu dar mai sunt pentru ai e a au care din fost acest"),
    ("hr", "i je u na da se za su ne sam što to ali ja ti kako koji"),
    ("fi", "ja on ei se että minä sinä hän me te he mutta tämä oli olen kun niin mitä nyt"),
    ("sv", "och är att det en ett jag du han hon vi inte på med för som har till den vad men här"),
    ("hu", "a az és hogy nem egy van is de meg én te ez azt mi már csak"),
    ("da", "og er at det en et jeg du han hun vi ikke på med for som har til den hvad men her"),
    ("et", "ja on ei see et mina sina ta me te nad aga kui oli olen mis nüüd siin"),
    ("lv", "un ir ka ar uz no es tu viņš mēs nav bet kas to par arī"),
    ("lt", "ir kad su į ne aš tu jis mes yra bet kas tai iš kaip labai"),
    ("sl", "in je v na da se za so ne sem kaj to ali jaz ti kako ki"),
];

/// The letters beyond a-z each Latin language writes (a text with another one is not in it; the preference for the
/// player's languages uses this: detect_alphabetic).
const OWN_LETTERS: [(&str, &str); 21] = [
    ("en", ""),
    ("es", "áéíóúüñ"),
    ("fr", "àâæçéèêëîïôœùûüÿ"),
    ("de", "äöüß"),
    ("it", "àèéìíîòóùú"),
    ("pt", "áâãàçéêíóôõúü"),
    ("nl", "éëïöüáèóú"),
    ("pl", "ąćęłńóśźż"),
    ("cs", "áčďéěíňóřšťúůýž"),
    ("sk", "áäčďéíĺľňóôŕšťúýž"),
    ("ro", "ăâîșțşţ"),
    ("hr", "čćđšž"),
    ("fi", "äöåšž"),
    ("sv", "åäöé"),
    ("hu", "áéíóöőúüű"),
    ("da", "æøåé"),
    ("et", "äöõüšž"),
    ("lv", "āčēģīķļņšūž"),
    ("lt", "ąčęėįšųūž"),
    ("sl", "čšžćđ"),
    ("mt", "ċġħżàèìòù"),
];

/// Every letter of a (lower-case) text is one `lang` writes (a-z, or its OWN_LETTERS).
fn writes_all(lang: &str, lower: &str) -> bool {
    let own = OWN_LETTERS.iter().find(|(l, _)| *l == lang).map_or("", |(_, o)| *o);
    lower.chars().filter(|c| c.is_alphabetic() && !c.is_ascii_alphabetic()).all(|c| own.contains(c))
}

/// How many of a (lower-case) text's words are among a language's FUNCTION_WORDS.
fn function_word_hits(lower: &str, lang: &str) -> usize {
    let Some((_, list)) = FUNCTION_WORDS.iter().find(|(l, _)| *l == lang) else {
        return 0;
    };
    lower
        .split(|c: char| !(c.is_alphabetic() || c == '\'' || c == '’'))
        .filter(|w| !w.is_empty())
        .filter(|w| {
            let w = w.replace('’', "'");
            list.split(' ').any(|x| x == w)
        })
        .count()
}

/// The language of a piece of text in one script group. Han: Japanese when the line has kana, else Cantonese when
/// the line has a character only Cantonese writes, else Chinese (the line decides: a Han stretch between two Latin
/// names is as Cantonese as the rest).
fn detect_group(text: &str, g: Group, line: &str) -> Option<&'static str> {
    match g {
        Group::Cjk => Some(if line.chars().any(|c| script(c) == Script::Kana) {
            "ja"
        } else if line.chars().any(|c| CANTONESE.contains(c)) {
            "yue"
        } else {
            "zh"
        }),
        Group::Hangul => Some("ko"),
        Group::Greek => Some("el"),
        Group::Other => None,
        Group::Latin | Group::Cyrillic => detect_alphabetic(text, g).0,
    }
}

/// The language of a whole text (its main language when it mixes some): the stretches' languages weighed by their
/// letters (a CJK character as two).
pub fn detect(text: &str) -> Option<&'static str> {
    main_language(text, &stretches(text))
}

fn weight(text: &str) -> usize {
    text.chars()
        .map(|c| match script(c) {
            Script::Neutral => 0,
            Script::Han | Script::Kana | Script::Hangul => 2,
            _ => 1,
        })
        .sum()
}

fn main_language(line: &str, parts: &[Stretch]) -> Option<&'static str> {
    let mut tally: Vec<(&'static str, usize)> = Vec::new();
    for p in parts {
        if let Some(l) = p.lang {
            let w = weight(p.text(line));
            match tally.iter_mut().find(|(x, _)| *x == l) {
                Some(t) => t.1 += w,
                None => tally.push((l, w)),
            }
        }
    }
    // (the heaviest; a tie: the first)
    let mut best: Option<(&'static str, usize)> = None;
    for t in tally {
        if best.is_none_or(|b| t.1 > b.1) {
            best = Some(t);
        }
    }
    best.map(|b| b.0)
}

/// A run of one script group: line[start..end], from its first letter to its last.
#[derive(Clone, Copy, Debug)]
struct Run {
    group: Group,
    start: usize,
    end: usize,
}

/// The line's runs of one script group each (the neutral characters between two letters of one group belong to it).
fn runs(line: &str) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    for (i, ch) in line.char_indices() {
        let Some(g) = group(script(ch)) else {
            continue;
        };
        let end = i + ch.len_utf8();
        match out.last_mut() {
            Some(r) if r.group == g => r.end = end,
            _ => out.push(Run { group: g, start: i, end }),
        }
    }
    out
}

/// Where the neutral gap line[a..b] between two runs is cut: before its first white space (what comes before it -
/// a full stop, a comma, a closing quote - ends the run before), opening marks going with the run after.
fn cut_gap(line: &str, a: usize, b: usize) -> usize {
    let gap = &line[a..b];
    let mut s = gap.char_indices().find(|(_, c)| c.is_whitespace()).map_or(gap.len(), |(i, _)| i);
    while let Some(c) = gap[..s].chars().next_back() {
        if !opening(c) {
            break;
        }
        s -= c.len_utf8();
    }
    a + s
}

/// Clause boundaries in a run: after a sentence or clause mark followed by white space, and at line breaks.
fn clauses(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev: Option<char> = None;
    for (i, c) in text.char_indices() {
        if let Some(p) = prev {
            let boundary = (c.is_whitespace() && ".!?;:,…".contains(p)) || p == '\n';
            if boundary && i > start {
                out.push((start, i));
                start = i;
            }
        }
        prev = Some(c);
    }
    if start < text.len() {
        out.push((start, text.len()));
    }
    out
}

/// The language of a part of a text whose whole is in `main`: its own when it clearly is another (SPLIT_WORDS,
/// SPLIT_SURE on the trigrams alone, the function words agreeing, not KIN), else main.
fn part_language(part: &str, g: Group, main: Option<&'static str>) -> Option<&'static str> {
    if lower_words(part) >= SPLIT_WORDS {
        if let (Some(l), sure) = detect_alphabetic_with(part, g, false) {
            let agreed = || detect_alphabetic(part, g).0 == Some(l);
            if Some(l) != main && sure >= SPLIT_SURE && !main.is_some_and(|m| kin(m, l)) && agreed() {
                return Some(l);
            }
        }
    }
    main
}

/// A Latin / Cyrillic run's parts: [(start, end, language)] relative to the run - one part in `main`, unless clauses
/// clearly detect as another language (part_language).
fn split_run(text: &str, g: Group, main: Option<&'static str>) -> Vec<(usize, usize, Option<&'static str>)> {
    let parts = clauses(text);
    if parts.len() < 2 {
        return vec![(0, text.len(), main)];
    }
    let mut out: Vec<(usize, usize, Option<&'static str>)> = Vec::new();
    for (a, b) in parts {
        let lang = part_language(&text[a..b], g, main);
        match out.last_mut() {
            Some(last) if last.2 == lang => last.1 = b,
            _ => out.push((a, b, lang)),
        }
    }
    out
}

/// stretches(), with the languages the line is most likely in (the player's own, the one chat is translated into): a
/// short or unsure stretch is told among those first (PREFER_WORDS, PREFER_BELOW).
pub fn stretches_preferring(line: &str, likely: &[&str]) -> Vec<Stretch> {
    let codes: Vec<&'static str> = LANGS.iter().copied().filter(|l| likely.contains(l)).collect();
    PREFERRED.with(|p| *p.borrow_mut() = codes);
    let out = stretches(line);
    PREFERRED.with(|p| p.borrow_mut().clear());
    out
}

/// The stretches of a line, in order, covering all of it (an empty line: none).
pub fn stretches(line: &str) -> Vec<Stretch> {
    let rs = runs(line);
    if rs.is_empty() {
        return if line.is_empty() { Vec::new() } else { vec![Stretch { start: 0, end: line.len(), lang: None }] };
    }
    // each run's span with the neutral characters around it (cut_gap), so the spans cover the line
    let mut spans = Vec::with_capacity(rs.len());
    for (i, r) in rs.iter().enumerate() {
        let start = if i == 0 { 0 } else { cut_gap(line, rs[i - 1].end, r.start) };
        let end = if i + 1 == rs.len() { line.len() } else { cut_gap(line, r.end, rs[i + 1].start) };
        spans.push((start, end));
    }
    // (a Latin or Cyrillic run cut by another script - a Russian sentence around a Latin letter - is detected with the
    // rest of its script's text in the line, and keeps its own language only when it clearly has another)
    let mut group_lang: Vec<(Group, Option<&'static str>)> = Vec::new();
    for g in [Group::Latin, Group::Cyrillic] {
        let all: Vec<&str> =
            rs.iter().zip(&spans).filter(|(r, _)| r.group == g).map(|(_, &(a, b))| &line[a..b]).collect();
        if all.len() > 1 {
            group_lang.push((g, detect_alphabetic(&all.join(" "), g).0));
        }
    }
    // (stretch, sure: false when it is too short to tell - it takes the line's main language)
    let mut parts: Vec<(Stretch, bool)> = Vec::new();
    for (r, &(start, end)) in rs.iter().zip(&spans) {
        let text = &line[start..end];
        match r.group {
            Group::Latin | Group::Cyrillic => {
                // (with other scripts in the line: a few words, or mostly capitalised ones - names, a brand list, a
                // title - go with the line's language)
                let short = words(text) < MIN_WORDS || (rs.len() > 1 && lower_words(text) < MIN_WORDS);
                let main = match group_lang.iter().find(|(g, _)| *g == r.group) {
                    Some(&(_, all)) => part_language(text, r.group, all),
                    None => detect_alphabetic(text, r.group).0,
                };
                for (a, b, lang) in split_run(text, r.group, main) {
                    parts.push((Stretch { start: start + a, end: start + b, lang }, !short));
                }
            }
            g => {
                // (a lone Han character among other scripts: too short to tell Chinese from a Japanese word)
                let short = g == Group::Cjk
                    && line[r.start..r.end].chars().filter(|&c| script(c) != Script::Neutral).count() < 2;
                parts.push((Stretch { start, end, lang: detect_group(text, g, line) }, !short));
            }
        }
    }
    if parts.len() > 1 {
        let sure: Vec<Stretch> = parts.iter().filter(|p| p.1).map(|p| p.0.clone()).collect();
        let main = if sure.is_empty() {
            main_language(line, &parts.iter().map(|p| p.0.clone()).collect::<Vec<_>>())
        } else {
            main_language(line, &sure)
        };
        for p in parts.iter_mut() {
            if !p.1 {
                p.0.lang = main;
            }
        }
    }
    let mut out: Vec<Stretch> = Vec::new();
    for (s, _) in parts {
        match out.last_mut() {
            Some(last) if last.lang == s.lang => last.end = s.end,
            _ => out.push(s),
        }
    }
    out
}

/// No space is put between a stretch ending (or starting) with this character and its neighbour when they are joined
/// again (Chinese and Japanese write no spaces; Korean does).
pub fn no_space(c: char) -> bool {
    matches!(script(c), Script::Han | Script::Kana) || in_ranges(c as u32, &[(0x3000, 0x303F), (0xFF00, 0xFF65)])
}

/// One of Koetama's languages written in the Latin alphabet (the detector chooses among these for a Latin stretch).
pub fn latin(lang: &str) -> bool {
    lang == "mt" || LATIN.iter().any(|t| t.0 == lang)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn langs(line: &str) -> Vec<(String, Option<&'static str>)> {
        stretches(line).iter().map(|s| (s.text(line).to_string(), s.lang)).collect()
    }

    #[test]
    fn scripts() {
        assert_eq!(script('あ'), Script::Kana);
        assert_eq!(script('ー'), Script::Kana);
        assert_eq!(script('漢'), Script::Han);
        assert_eq!(script('한'), Script::Hangul);
        assert_eq!(script('é'), Script::Latin);
        assert_eq!(script('ж'), Script::Cyrillic);
        assert_eq!(script('λ'), Script::Greek);
        assert_eq!(script('ب'), Script::Other);
        for c in [' ', '7', '!', '。', '「', '😀', '\u{301}'] {
            assert_eq!(script(c), Script::Neutral, "{c:?}");
        }
    }

    #[test]
    fn covers_the_line() {
        for line in ["", " ", "123 !!", "Hello 世界", "  a  ", "「こんにちは」と言った。Hello there my friend!"]
        {
            let st = stretches(line);
            let mut at = 0;
            for s in &st {
                assert_eq!(s.start, at, "{line:?}: {st:?}");
                assert!(s.end > s.start);
                at = s.end;
            }
            assert_eq!(at, line.len(), "{line:?}: {st:?}");
        }
        assert!(stretches("").is_empty());
        assert_eq!(langs("12:30 :)"), [("12:30 :)".to_string(), None)]);
    }

    #[test]
    fn gaps() {
        // (the full stop ends the English; the space and the opening bracket go with the Japanese)
        let l = langs("This is my favourite song. 「さくら」が好きです");
        assert_eq!(l.len(), 2, "{l:?}");
        assert_eq!(l[0].0, "This is my favourite song.");
        assert_eq!(l[1].0, " 「さくら」が好きです");
        assert_eq!(l[1].1, Some("ja"));
    }

    #[test]
    fn clause_cuts() {
        assert_eq!(clauses("a, b. c"), [(0, 2), (2, 5), (5, 7)]);
        assert_eq!(clauses("3.5 kg"), [(0, 6)]);
        assert_eq!(words("it's 3 o'clock, ok?"), 5);
    }
}
