//! An app's words in the person's language (Ben, 2026-11-18: "we should
//! add a language setting and also let's translate all olo apps and
//! prefs and ux to support all the major languages (defaulting to us
//! english)").
//!
//! The words are written in the code in English, as they always were;
//! each language has a catalog of them (`crates/kit-base/words/CODE.txt`,
//! in every program: a few pages, of which only the chosen language's
//! are ever touched), and [`tr`] gives a word's in the language chosen,
//! or the word itself where the catalog has none (and always, in
//! English: nothing is looked up, nothing is read). The kit asks for
//! the words an app hands its widgets as literals (a button's label, a
//! menu's items, a window's title: `olo_kit::words::of`), so most of an
//! app is translated without saying so; words made as it goes (a
//! sentence with a number or a name in it) are asked for with
//! [`tr!`](crate::tr) and [`count`], which put the parts where the
//! language wants them.
//!
//! A catalog is blocks apart by an empty line: the English, then the
//! translation; for a sentence that counts something, the English with
//! `{n}` for many, then a line for each form the language has (`one:`,
//! `few:`, `many:`, `other:`). `\n` is a new line. `#` begins a comment.
//!
//! The language is Olo's setting (`Settings::language`, applied by the
//! kit to every running app at once), `$OLO_LANGUAGE` for a test.

use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    sync::{
        Arc, Mutex, OnceLock, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

/// How a language counts: which of its forms a number takes (CLDR's
/// rules for whole numbers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counting {
    /// One, and anything else (English, German, Dutch, Spanish, Italian, Hindi…).
    OneOther,
    /// None or one, and anything else (French, Brazilian Portuguese).
    UpToOneOther,
    /// No forms (Japanese, Korean, Chinese).
    Other,
    /// One, few, many (Russian).
    Slavic,
}

/// A form a counted sentence has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Form {
    One,
    Few,
    Many,
    Other,
}

impl Counting {
    pub fn form(self, n: u64) -> Form {
        match self {
            Counting::OneOther => {
                if n == 1 {
                    Form::One
                } else {
                    Form::Other
                }
            }
            Counting::UpToOneOther => {
                if n <= 1 {
                    Form::One
                } else {
                    Form::Other
                }
            }
            Counting::Other => Form::Other,
            Counting::Slavic => match (n % 10, n % 100) {
                (1, h) if h != 11 => Form::One,
                (2..=4, h) if !(12..=14).contains(&h) => Form::Few,
                _ => Form::Many,
            },
        }
    }
}

/// A language Olo's words are in.
#[derive(Debug)]
pub struct Language {
    /// Its tag ("de", "pt-BR", "zh-Hans"): what the setting holds.
    pub code: &'static str,
    /// Its name in itself ("Deutsch"), and in English.
    pub name: &'static str,
    pub english: &'static str,
    /// The locale other programs are given for it ("de_DE").
    pub locale: &'static str,
    pub counting: Counting,
    /// Its catalog (empty: English).
    text: &'static str,
}

/// English first (what the code is written in), then by their English
/// names.
pub static LANGUAGES: [Language; 13] = [
    Language { code: "en", name: "English", english: "English", locale: "en_US", counting: Counting::OneOther, text: "" },
    Language { code: "zh-Hans", name: "简体中文", english: "Chinese, Simplified", locale: "zh_CN", counting: Counting::Other, text: include_str!("../words/zh-Hans.txt") },
    Language { code: "zh-Hant", name: "繁體中文", english: "Chinese, Traditional", locale: "zh_TW", counting: Counting::Other, text: include_str!("../words/zh-Hant.txt") },
    Language { code: "nl", name: "Nederlands", english: "Dutch", locale: "nl_NL", counting: Counting::OneOther, text: include_str!("../words/nl.txt") },
    Language { code: "fr", name: "Français", english: "French", locale: "fr_FR", counting: Counting::UpToOneOther, text: include_str!("../words/fr.txt") },
    Language { code: "de", name: "Deutsch", english: "German", locale: "de_DE", counting: Counting::OneOther, text: include_str!("../words/de.txt") },
    Language { code: "hi", name: "हिन्दी", english: "Hindi", locale: "hi_IN", counting: Counting::UpToOneOther, text: include_str!("../words/hi.txt") },
    Language { code: "it", name: "Italiano", english: "Italian", locale: "it_IT", counting: Counting::OneOther, text: include_str!("../words/it.txt") },
    Language { code: "ja", name: "日本語", english: "Japanese", locale: "ja_JP", counting: Counting::Other, text: include_str!("../words/ja.txt") },
    Language { code: "ko", name: "한국어", english: "Korean", locale: "ko_KR", counting: Counting::Other, text: include_str!("../words/ko.txt") },
    Language { code: "pt-BR", name: "Português (Brasil)", english: "Portuguese, Brazil", locale: "pt_BR", counting: Counting::UpToOneOther, text: include_str!("../words/pt-BR.txt") },
    Language { code: "ru", name: "Русский", english: "Russian", locale: "ru_RU", counting: Counting::Slavic, text: include_str!("../words/ru.txt") },
    Language { code: "es", name: "Español", english: "Spanish", locale: "es_ES", counting: Counting::OneOther, text: include_str!("../words/es.txt") },
];

/// The language with this tag (whatever its case; "de-DE" and "de_DE"
/// are "de"). None: not one of Olo's.
pub fn language(code: &str) -> Option<&'static Language> {
    let code = code.trim().replace('_', "-");
    let whole = LANGUAGES.iter().find(|l| l.code.eq_ignore_ascii_case(&code));
    whole.or_else(|| {
        let first = code.split('-').next().unwrap_or_default();
        // (Chinese without its script is the simplified.)
        LANGUAGES.iter().find(|l| l.code.split('-').next().is_some_and(|c| c.eq_ignore_ascii_case(first)))
    })
}

/// A word's translation: its own, and the forms of one that counts.
#[derive(Debug, Default, Clone, PartialEq)]
struct Entry {
    text: &'static str,
    forms: Vec<(Form, &'static str)>,
}

/// A language's words, by the English.
#[derive(Debug)]
struct Catalog {
    language: &'static Language,
    words: HashMap<&'static str, Entry>,
}

/// `text` with `\n` made a new line (kept as it is where there's none:
/// nearly always).
fn plain(text: &'static str) -> &'static str {
    if text.contains("\\n") { Box::leak(text.replace("\\n", "\n").into_boxed_str()) } else { text }
}

/// A catalog's text read: each block's English, and what it is here.
fn read(text: &'static str) -> HashMap<&'static str, Entry> {
    let mut words = HashMap::new();
    let mut block: Vec<&'static str> = Vec::new();
    let mut end = |block: &mut Vec<&'static str>| {
        if let [english, rest @ ..] = block.as_slice()
            && !rest.is_empty()
        {
            let mut entry = Entry::default();
            for line in rest {
                let form =
                    [("one:", Form::One), ("few:", Form::Few), ("many:", Form::Many), ("other:", Form::Other)].into_iter().find_map(|(mark, form)| line.strip_prefix(mark).map(|t| (form, t.trim())));
                match form {
                    Some((form, said)) => entry.forms.push((form, plain(said))),
                    None => entry.text = plain(line),
                }
            }
            words.insert(plain(english), entry);
        }
        block.clear();
    };
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            end(&mut block);
        } else if !line.starts_with('#') {
            block.push(line);
        }
    }
    end(&mut block);
    words
}

/// The language chosen, if it isn't English.
static NOW: RwLock<Option<Arc<Catalog>>> = RwLock::new(None);
/// Whether there is one: English asks nothing more than this.
static TRANSLATING: AtomicBool = AtomicBool::new(false);

/// Each language's catalog, read once.
fn catalog(language: &'static Language) -> Arc<Catalog> {
    static READ: OnceLock<Mutex<HashMap<&'static str, Arc<Catalog>>>> = OnceLock::new();
    let mut read_already = READ.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    read_already.entry(language.code).or_insert_with(|| Arc::new(Catalog { language, words: read(language.text) })).clone()
}

/// The language set: `code` ("de"; empty or one Olo hasn't: English).
/// Whether that changed anything (the windows are then drawn again).
pub fn set(code: &str) -> bool {
    let wanted = language(code).filter(|l| !l.text.is_empty());
    let mut now = NOW.write().unwrap_or_else(|e| e.into_inner());
    if now.as_ref().map(|c| c.language.code) == wanted.map(|l| l.code) {
        return false;
    }
    *now = wanted.map(catalog);
    TRANSLATING.store(now.is_some(), Ordering::Release);
    true
}

/// The language as a program starts: `$OLO_LANGUAGE` (tests, pictures),
/// else what `setting` says.
pub fn start(setting: &str) {
    let named = std::env::var("OLO_LANGUAGE").ok().filter(|v| !v.is_empty());
    set(named.as_deref().unwrap_or(setting));
}

/// The language chosen.
pub fn current() -> &'static Language {
    if !TRANSLATING.load(Ordering::Acquire) {
        return &LANGUAGES[0];
    }
    NOW.read().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(&LANGUAGES[0], |c| c.language)
}

/// Every word asked for is written to the file `$OLO_WORDS_SEEN` names,
/// each once (the list of what there is to translate: run the layout
/// lint with it, which opens every window of every app).
#[inline]
fn seen(text: &'static str) {
    static SEEN: OnceLock<Option<Mutex<(HashSet<&'static str>, std::fs::File)>>> = OnceLock::new();
    // (Not asked for, as nearly always: one load, and on.)
    let Some(seen) = SEEN.get_or_init(|| {
        let path = std::env::var_os("OLO_WORDS_SEEN").filter(|p| !p.is_empty())?;
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path).ok()?;
        Some(Mutex::new((HashSet::new(), file)))
    }) else {
        return;
    };
    let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
    if seen.0.insert(text) {
        use std::io::Write;
        let _ = writeln!(seen.1, "{}", text.replace('\n', "\\n"));
    }
}

/// `text` (as the code writes it, in English) in the language chosen:
/// itself in English, and where the language has no word for it.
pub fn tr(text: &'static str) -> &'static str {
    seen(text);
    if !TRANSLATING.load(Ordering::Acquire) {
        return text;
    }
    let now = NOW.read().unwrap_or_else(|e| e.into_inner());
    now.as_ref().and_then(|c| c.words.get(text)).map(|e| e.text).filter(|t| !t.is_empty()).unwrap_or(text)
}

/// The translation of words given by what they say, not as a literal
/// (an alert's message, handed on as `&str`): None in English, and
/// where the language has none.
pub fn lookup(text: &str) -> Option<&'static str> {
    if !TRANSLATING.load(Ordering::Acquire) {
        return None;
    }
    let now = NOW.read().unwrap_or_else(|e| e.into_inner());
    now.as_ref().and_then(|c| c.words.get(text)).map(|e| e.text).filter(|t| !t.is_empty())
}

/// `pattern` with its `{}` filled from `parts` in turn, and `{0}`,
/// `{1}`… by their place (a translation puts them in its own order).
/// `{{` and `}}` are braces.
pub fn fill(pattern: &str, parts: &[&dyn Display]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut next = 0;
    let mut rest = pattern;
    while let Some(at) = rest.find(['{', '}']) {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
        } else if let Some(close) = tail.find('}').filter(|_| tail.starts_with('{')) {
            let name = tail[1..close].trim();
            let which = if name.is_empty() {
                next += 1;
                Some(next - 1)
            } else {
                name.parse::<usize>().ok()
            };
            match which.and_then(|i| parts.get(i)) {
                Some(part) => {
                    let _ = write!(out, "{part}");
                }
                // (Not one of the parts: written as it is.)
                None => out.push_str(&tail[..=close]),
            }
            rest = &tail[close + 1..];
        } else {
            out.push_str(&tail[..1]);
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// A sentence that counts something, in the language's form for `n`:
/// `one` and `other` are the English ("1 window", "{n} windows"), and
/// `{n}` in whichever is used is the number. A language's forms are in
/// its catalog under `other`.
pub fn count(n: u64, one: &'static str, other: &'static str) -> String {
    seen(other);
    let said = |pattern: &str| pattern.replace("{n}", &n.to_string());
    if !TRANSLATING.load(Ordering::Acquire) {
        return said(if n == 1 { one } else { other });
    }
    let now = NOW.read().unwrap_or_else(|e| e.into_inner());
    let Some(catalog) = now.as_ref() else { return said(if n == 1 { one } else { other }) };
    let Some(entry) = catalog.words.get(other) else { return said(if n == 1 { one } else { other }) };
    let form = catalog.language.counting.form(n);
    let pattern = entry
        .forms
        .iter()
        .find(|(f, _)| *f == form)
        .or_else(|| entry.forms.iter().find(|(f, _)| *f == Form::Other))
        .map(|(_, t)| *t)
        .filter(|t| !t.is_empty())
        .or(Some(entry.text).filter(|t| !t.is_empty()));
    match pattern {
        Some(pattern) => said(pattern),
        None => said(if n == 1 { one } else { other }),
    }
}

/// How many of a language's words `asked` (the English) has: for
/// saying how far a translation has got.
pub fn covered(language: &'static Language, asked: impl Iterator<Item = &'static str>) -> (usize, usize) {
    if language.text.is_empty() {
        let n = asked.count();
        return (n, n);
    }
    let catalog = catalog(language);
    asked.fold((0, 0), |(have, all), text| (have + usize::from(catalog.words.contains_key(text)), all + 1))
}

/// A sentence in the language chosen with its parts put in:
/// `tr!("Updated {} ago", when)`. The first is the English, as a
/// literal; alone, `tr!("Save")` is [`words::tr`](crate::words::tr).
#[macro_export]
macro_rules! tr {
    ($text:literal) => {
        $crate::words::tr($text)
    };
    ($text:literal, $($part:expr),+ $(,)?) => {
        $crate::words::fill($crate::words::tr($text), &[$(&$part as &dyn ::std::fmt::Display),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A catalog read: a word, a sentence with a new line, one that
    /// counts with its forms; comments and stray lines passed over.
    #[test]
    fn a_catalog_read() {
        let words = read("# German\nSave\nSichern\n\n\nNot saved.\\nTry again?\nNicht gesichert.\\nNoch einmal?\n\n{n} windows\none: {n} Fenster\nother: {n} Fenster\n\nAlone\n\n# end\n");
        assert_eq!(words.get("Save").map(|e| e.text), Some("Sichern"));
        assert_eq!(words.get("Not saved.\nTry again?").map(|e| e.text), Some("Nicht gesichert.\nNoch einmal?"));
        assert_eq!(words.get("{n} windows").map(|e| e.forms.clone()), Some(vec![(Form::One, "{n} Fenster"), (Form::Other, "{n} Fenster")]));
        assert_eq!(words.len(), 3, "a block with no translation is no entry");
    }

    /// Parts put in: in turn, by their place, braces themselves, and
    /// what isn't a part left as written.
    #[test]
    fn a_sentence_filled() {
        assert_eq!(fill("Updated {} ago by {}", &[&"5 minutes", &"Kim"]), "Updated 5 minutes ago by Kim");
        assert_eq!(fill("{1} hat vor {0} aktualisiert", &[&"5 Minuten", &"Kim"]), "Kim hat vor 5 Minuten aktualisiert");
        assert_eq!(fill("{{}} and {name} and {7} and {", &[&1]), "{} and {name} and {7} and {");
        assert_eq!(fill("{}%", &[&50]), "50%");
        assert_eq!(fill("", &[]), "");
    }

    /// How each kind of language counts.
    #[test]
    fn counting() {
        use Form::*;
        assert_eq!([0, 1, 2, 11].map(|n| Counting::OneOther.form(n)), [Other, One, Other, Other]);
        assert_eq!([0, 1, 2].map(|n| Counting::UpToOneOther.form(n)), [One, One, Other]);
        assert_eq!([1, 5].map(|n| Counting::Other.form(n)), [Other, Other]);
        assert_eq!([1, 2, 5, 11, 12, 21, 22, 25, 111, 101].map(|n| Counting::Slavic.form(n)), [One, Few, Many, Many, Many, One, Few, Many, Many, One]);
    }

    /// Every catalog reads, every translation keeps its English's parts
    /// (`{}`, `{0}`, `{n}`: a sentence can't lose its number), and a
    /// language is found by its tag however it's written.
    #[test]
    fn the_catalogs() {
        for language in LANGUAGES.iter().filter(|l| !l.text.is_empty()) {
            let words = read(language.text);
            assert!(!words.is_empty(), "{}: no words", language.code);
            let parts = |text: &str| {
                let mut found: Vec<String> = Vec::new();
                let mut rest = text;
                while let Some(open) = rest.find('{') {
                    let Some(close) = rest[open..].find('}') else { break };
                    found.push(rest[open..open + close + 1].to_owned());
                    rest = &rest[open + close + 1..];
                }
                found.sort();
                found
            };
            for (english, entry) in &words {
                let wanted = parts(english);
                // (By place, a translation may name its parts: {0} for the first {}.)
                let own = |text: &str| {
                    let mut p = parts(text);
                    if p.iter().any(|x| x.len() > 2 && x != "{n}") && wanted.iter().all(|x| x == "{}") {
                        p = vec!["{}".to_owned(); p.len()];
                    }
                    p
                };
                for text in std::iter::once(entry.text).chain(entry.forms.iter().map(|(_, t)| *t)).filter(|t| !t.is_empty()) {
                    assert_eq!(own(text), wanted, "{}: “{english}” is “{text}”", language.code);
                }
                assert!(!entry.text.is_empty() || !entry.forms.is_empty(), "{}: “{english}” has no translation", language.code);
            }
        }
        assert_eq!(language("de_DE").map(|l| l.code), Some("de"));
        assert_eq!(language("PT-br").map(|l| l.code), Some("pt-BR"));
        assert_eq!(language("zh").map(|l| l.code), Some("zh-Hans"));
        assert_eq!(language("zh-Hant").map(|l| l.code), Some("zh-Hant"));
        assert!(language("tlh").is_none());
        assert_eq!(language("").map(|l| l.code), None);
    }

    /// In a language: its word, its form for a number, and English for
    /// what it hasn't; back in English, the words as written. (One test
    /// sets the language: it's the program's.)
    #[test]
    fn words_in_a_language_and_back() {
        assert_eq!((tr("Cancel"), count(1, "{n} item", "{n} items"), count(3, "{n} item", "{n} items")), ("Cancel", "1 item".to_owned(), "3 items".to_owned()));
        assert!(set("de") && !set("de_DE"));
        assert_eq!(current().code, "de");
        assert_eq!(tr("Cancel"), "Abbrechen");
        assert_eq!(tr("A word no catalog has"), "A word no catalog has");
        assert_eq!(crate::tr!("A word no catalog has, {} times", 3), "A word no catalog has, 3 times");
        assert!(set("") && current().code == "en");
        assert_eq!(tr("Cancel"), "Cancel");
    }
}
