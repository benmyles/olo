//! An app's words in the person's language (Ben, 2026-11-18: "we should
//! add a language setting and also let's translate all olo apps and
//! prefs and ux to support all the major languages (defaulting to us
//! english)").
//!
//! The words are written in the code in English, as they always were;
//! each language has a catalog of them (`data/words/CODE.txt`,
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
    /// Whether it writes every noun with a capital (German): a phrase set
    /// into a sentence keeps its first letter then (`mid_sentence`).
    pub capital_nouns: bool,
}

/// A language's catalog as text (`en`: Olo's word list): the file
/// `words/CODE.txt` in Olo's data (`olo_xdg::data_dirs`: the tree's
/// `data/` beside a build, the installed share folder), read the first
/// time it's wanted and kept for good. Empty where there's none: the
/// words stay English.
///
/// They were compiled in (`include_str!`) until 2026-11-20: a word
/// changed in one catalog built olo-kit-base and the 47 crates on it
/// again, 2 min 35 s of release build here for the workspace, and each
/// of 38 programs carried all twelve (3.5 MB). Read at start, a catalog
/// costs a program in that language 0.2 ms more than it did (the read,
/// 268 KB; making its table is the same 1 ms: `what_a_catalog_costs`),
/// and one in English nothing, as before.
fn text_of(code: &str) -> &'static str {
    static READ: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut read = READ.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(text) = read.get(code) {
        return text;
    }
    let found = olo_xdg::data_dirs().iter().map(|d| d.join("words").join(format!("{code}.txt"))).find_map(|p| std::fs::read_to_string(p).ok());
    if found.is_none() {
        tracing::warn!("words: no catalog for {code} in Olo's data (words/{code}.txt): its words stay English");
    }
    let text: &'static str = found.map_or("", |t| Box::leak(t.into_boxed_str()));
    read.insert(code.to_owned(), text);
    text
}

/// English first (what the code is written in), then by their English
/// names.
pub static LANGUAGES: [Language; 13] = [
    Language { code: "en", name: "English", english: "English", locale: "en_US", counting: Counting::OneOther, capital_nouns: false },
    Language { code: "zh-Hans", name: "简体中文", english: "Chinese, Simplified", locale: "zh_CN", counting: Counting::Other, capital_nouns: false },
    Language { code: "zh-Hant", name: "繁體中文", english: "Chinese, Traditional", locale: "zh_TW", counting: Counting::Other, capital_nouns: false },
    Language { code: "nl", name: "Nederlands", english: "Dutch", locale: "nl_NL", counting: Counting::OneOther, capital_nouns: false },
    Language { code: "fr", name: "Français", english: "French", locale: "fr_FR", counting: Counting::UpToOneOther, capital_nouns: false },
    Language { code: "de", name: "Deutsch", english: "German", locale: "de_DE", counting: Counting::OneOther, capital_nouns: true },
    Language { code: "hi", name: "हिन्दी", english: "Hindi", locale: "hi_IN", counting: Counting::UpToOneOther, capital_nouns: false },
    Language { code: "it", name: "Italiano", english: "Italian", locale: "it_IT", counting: Counting::OneOther, capital_nouns: false },
    Language { code: "ja", name: "日本語", english: "Japanese", locale: "ja_JP", counting: Counting::Other, capital_nouns: false },
    Language { code: "ko", name: "한국어", english: "Korean", locale: "ko_KR", counting: Counting::Other, capital_nouns: false },
    Language { code: "pt-BR", name: "Português (Brasil)", english: "Portuguese, Brazil", locale: "pt_BR", counting: Counting::UpToOneOther, capital_nouns: false },
    Language { code: "ru", name: "Русский", english: "Russian", locale: "ru_RU", counting: Counting::Slavic, capital_nouns: false },
    Language { code: "es", name: "Español", english: "Spanish", locale: "es_ES", counting: Counting::OneOther, capital_nouns: false },
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
    /// Its sentences with parts, to find one another program said
    /// (`of_said`): made the first time one is looked for.
    patterns: OnceLock<Vec<Pattern>>,
    /// What `of_said` found for each text it was given, kept (a window
    /// shows the same few again and again).
    told: Mutex<HashMap<String, Option<String>>>,
}

impl Catalog {
    fn new(language: &'static Language) -> Catalog {
        Catalog { language, words: read(text_of(language.code)), patterns: OnceLock::new(), told: Mutex::default() }
    }

    /// `text` in this language, if it's one of Olo's sentences: whole,
    /// or with parts put in.
    fn retell(&self, text: &str) -> Option<String> {
        if let Some(entry) = self.words.get(text) {
            return Some(entry.text).filter(|t| !t.is_empty()).map(str::to_owned);
        }
        let mut told = self.told.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(found) = told.get(text) {
            return found.clone();
        }
        let patterns = self.patterns.get_or_init(|| self.words.keys().filter_map(|k| Pattern::of(k)).collect());
        let found = best(patterns, text).and_then(|(p, parts)| self.with_parts(p, &parts));
        // (A few kept: a window's few, again and again; never many.)
        if told.len() >= 256 {
            told.clear();
        }
        told.insert(text.to_owned(), found.clone());
        found
    }

    /// The translation of `p` with `parts` put in its places.
    fn with_parts(&self, p: &Pattern, parts: &[&str]) -> Option<String> {
        let entry = self.words.get(p.key)?;
        if p.counted {
            let n: u64 = parts.first()?.parse().ok()?;
            let form = self.language.counting.form(n);
            let said = entry.forms.iter().find(|(f, _)| *f == form).or_else(|| entry.forms.iter().find(|(f, _)| *f == Form::Other)).map(|(_, t)| *t);
            return said.or(Some(entry.text)).filter(|t| !t.is_empty()).map(|t| t.replace("{n}", parts[0]));
        }
        let parts: Vec<&dyn Display> = parts.iter().map(|p| p as &dyn Display).collect();
        Some(entry.text).filter(|t| !t.is_empty()).map(|t| fill_in(t, &parts))
    }
}

/// A sentence with parts (`{}`, or a count's `{n}`), as its words
/// between them.
#[derive(Debug)]
struct Pattern {
    /// The English, as the catalog has it.
    key: &'static str,
    /// Its words before, between and after the parts: one more than it
    /// has parts.
    between: Vec<&'static str>,
    /// Its one part is a count's `{n}`.
    counted: bool,
    /// How many letters its own words have: of two that fit, the one
    /// that says more.
    letters: usize,
}

impl Pattern {
    fn of(key: &'static str) -> Option<Pattern> {
        if key.contains("{{") || key.contains("}}") {
            return None;
        }
        let (mut between, mut counted, mut rest) = (Vec::new(), false, key);
        while let Some(open) = rest.find('{') {
            let close = open + rest[open..].find('}')?;
            match &rest[open + 1..close] {
                "" => {}
                "n" => counted = true,
                _ => return None,
            }
            between.push(&rest[..open]);
            rest = &rest[close + 1..];
        }
        between.push(rest);
        // (Two parts side by side can't be told apart; a count has one.)
        if between.len() < 2 || between[1..between.len() - 1].iter().any(|b| b.is_empty()) || (counted && between.len() != 2) {
            return None;
        }
        let letters = between.iter().map(|b| b.chars().filter(|c| c.is_alphabetic()).count()).sum();
        // (Too few words of its own to be told from any other: "{}: {}".)
        (letters >= 3).then_some(Pattern { key, between, counted, letters })
    }

    /// `text`'s parts, if it's this sentence said with them: each part
    /// something, the words between found from the left.
    fn parts<'t>(&self, text: &'t str) -> Option<Vec<&'t str>> {
        let n = self.between.len();
        let mut rest = text.strip_prefix(self.between[0])?.strip_suffix(self.between[n - 1])?;
        let mut parts = Vec::with_capacity(n - 1);
        for words in &self.between[1..n - 1] {
            let skip = rest.chars().next()?.len_utf8();
            let at = skip + rest[skip..].find(words)?;
            parts.push(&rest[..at]);
            rest = &rest[at + words.len()..];
        }
        if rest.is_empty() || (self.counted && !rest.bytes().all(|b| b.is_ascii_digit())) {
            return None;
        }
        parts.push(rest);
        Some(parts)
    }
}

/// The sentence of `patterns` that `text` is, with its parts: of two
/// that fit, the one with more words of its own.
fn best<'p, 't>(patterns: &'p [Pattern], text: &'t str) -> Option<(&'p Pattern, Vec<&'t str>)> {
    patterns.iter().filter_map(|p| p.parts(text).map(|parts| (p, parts))).max_by_key(|(p, _)| p.letters)
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
    // (A line's spaces at its end are its own: "About " is a key, what a
    // menu's About item starts with, and its translations end in a space
    // too. They were trimmed, and it was found in no catalog: the agent
    // translating Olo's words, 2026-11-19.)
    for line in text.lines() {
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
    read_already.entry(language.code).or_insert_with(|| Arc::new(Catalog::new(language))).clone()
}

/// The language set: `code` ("de"; empty or one Olo hasn't: English).
/// Whether that changed anything (the windows are then drawn again).
pub fn set(code: &str) -> bool {
    let wanted = language(code).filter(|l| l.code != "en");
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
/// Whether every word asked for is being written down (`OLO_WORDS_SEEN`).
pub fn recording() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("OLO_WORDS_SEEN").is_some_and(|p| !p.is_empty()))
}

/// `text` written down as one of Olo's words, if they're being
/// (`recording`): for words handed on as plain text or kept as they are
/// in English (a tab view's tabs, a menu field's choices, a box's title).
pub fn note(text: &str) {
    if watched() {
        if recording() {
            seen(text);
        }
        said(text);
    }
}

#[inline]
fn seen(text: &str) {
    static SEEN: OnceLock<Option<Mutex<(HashSet<String>, std::fs::File)>>> = OnceLock::new();
    // (Not asked for, as nearly always: one load, and on.)
    let Some(seen) = SEEN.get_or_init(|| {
        let path = std::env::var_os("OLO_WORDS_SEEN").filter(|p| !p.is_empty())?;
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path).ok()?;
        Some(Mutex::new((HashSet::new(), file)))
    }) else {
        return;
    };
    let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
    if !seen.0.contains(text) && seen.0.insert(text.to_owned()) {
        use std::io::Write;
        // (One write: every app appends to the one file, and a word and
        // its newline written apart came out as "EditEdit".)
        let line = format!("{}\n", text.replace('\n', "\\n"));
        let _ = seen.1.write_all(line.as_bytes());
    }
}

/// Whether words shown but never asked for are being found
/// (`OLO_WORDS_UNSEEN` names a file): every word the layer hands back is
/// kept, and what a widget is handed, or the kit draws, that isn't one of
/// them is written down (`unasked`): words made at run time (a
/// `format!`, a literal copied into a `String`) that no language can
/// reach. `tools/dev/words unasked` runs the live lint so.
pub fn finding_unasked() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("OLO_WORDS_UNSEEN").is_some_and(|p| !p.is_empty()))
}

/// Whether anything watches the words (`OLO_WORDS_SEEN`,
/// `OLO_WORDS_UNSEEN`): one load, as nearly always nothing does.
#[inline]
fn watched() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| recording() || finding_unasked())
}

static SAID: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

/// `text`, handed back by the layer (`finding_unasked`).
#[inline]
pub fn said(text: &str) {
    if watched() && finding_unasked() {
        SAID.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()).insert(text.to_owned());
    }
}

/// Whether the layer handed `text` back (`finding_unasked`).
pub fn was_said(text: &str) -> bool {
    SAID.get().is_some_and(|s| s.lock().unwrap_or_else(|e| e.into_inner()).contains(text))
}

/// `text` shown but never asked for (`finding_unasked`), written down
/// once, with the program it's in, unless the layer handed it back or it
/// has no words (a number, a mark).
pub fn unasked(text: &str) {
    if !watched() || !finding_unasked() || was_said(text) || text.chars().filter(|c| c.is_alphabetic()).count() < 2 {
        return;
    }
    static FILE: OnceLock<Option<Mutex<(HashSet<String>, std::fs::File, String)>>> = OnceLock::new();
    let Some(file) = FILE.get_or_init(|| {
        let path = std::env::var_os("OLO_WORDS_UNSEEN").filter(|p| !p.is_empty())?;
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path).ok()?;
        let me = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
        Some(Mutex::new((HashSet::new(), file, me)))
    }) else {
        return;
    };
    let mut file = file.lock().unwrap_or_else(|e| e.into_inner());
    if file.0.insert(text.to_owned()) {
        use std::io::Write;
        let line = format!("{}\t{}\n", file.2, text.replace('\n', "\\n"));
        let _ = file.1.write_all(line.as_bytes());
    }
}

/// A menu's item named in a sentence ("choose {} in the window menu"):
/// `path`'s parts, the menu's own words as its items say them ("Float
/// Window", "Always"), each in the language chosen as the menu shows it,
/// joined by "▸", with no-break spaces inside and between them, so a
/// sentence's line never breaks inside a menu's name in any language (a
/// translator writing the path into a sentence wrote names the menu
/// didn't have, and the wrapper broke inside them: „Fenster / schweben
/// lassen ▸ Immer"; the reviews of the panels, 2026-11-20).
pub fn menu_path(path: &[&'static str]) -> String {
    path.iter().map(|part| tr(part).replace(' ', "\u{a0}")).collect::<Vec<_>>().join("\u{a0}▸\u{a0}")
}

/// `text` (as the code writes it, in English) in the language chosen:
/// itself in English, and where the language has no word for it.
pub fn tr(text: &'static str) -> &'static str {
    let out = if TRANSLATING.load(Ordering::Acquire) {
        let now = NOW.read().unwrap_or_else(|e| e.into_inner());
        now.as_ref().and_then(|c| c.words.get(text)).map(|e| e.text).filter(|t| !t.is_empty()).unwrap_or(text)
    } else {
        text
    };
    // (One load when nothing watches, as before it was `seen`'s.)
    if watched() {
        seen(text);
        said(out);
    }
    out
}

/// The translation of words given by what they say, not as a literal
/// (an alert's message, handed on as `&str`): None in English, and
/// where the language has none.
pub fn lookup(text: &str) -> Option<&'static str> {
    let out = if TRANSLATING.load(Ordering::Acquire) {
        let now = NOW.read().unwrap_or_else(|e| e.into_inner());
        now.as_ref().and_then(|c| c.words.get(text)).map(|e| e.text).filter(|t| !t.is_empty())
    } else {
        None
    };
    if watched() {
        note(text);
        said(out.unwrap_or(text));
    }
    out
}

/// `pattern` with its `{}` filled from `parts` in turn, and `{0}`,
/// `{1}`… by their place (a translation puts them in its own order).
/// `{{` and `}}` are braces.
pub fn fill(pattern: &str, parts: &[&dyn Display]) -> String {
    let out = fill_in(pattern, parts);
    said(&out);
    out
}

fn fill_in(pattern: &str, parts: &[&dyn Display]) -> String {
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

/// Words for the person that another program shows (a service's state,
/// an error it keeps, what it answers): kept in English here, where
/// programs compare them and agents and logs read them, and gathered
/// into the catalogs, so the window showing them gives them in the
/// person's language ([`of_said`]). With parts: [`en!`](crate::en).
pub fn en(text: &'static str) -> &'static str {
    if recording() {
        seen(text);
    }
    text
}

/// [`en!`](crate::en)'s sentence with its parts put in, in English.
pub fn en_with(pattern: &'static str, parts: &[&dyn Display]) -> String {
    fill_in(en(pattern), parts)
}

/// What another program said in English for the person ([`en`],
/// [`en!`](crate::en): a service's state, an error it kept), in the
/// language chosen: the sentence found by its words, whole or around
/// the parts put in it (a path, a number, a system's own message), and
/// the parts put in the translation's places as they are. Itself in
/// English, and where it's none of Olo's sentences.
pub fn of_said(text: &str) -> String {
    let found = if TRANSLATING.load(Ordering::Acquire) {
        let now = NOW.read().unwrap_or_else(|e| e.into_inner()).clone();
        now.and_then(|c| c.retell(text))
    } else {
        None
    };
    if watched() && finding_unasked() {
        match &found {
            Some(out) => said(out),
            // (In English: handed back as one of Olo's, if it is.)
            None if is_english(text) => said(text),
            None => {}
        }
    }
    found.unwrap_or_else(|| text.to_owned())
}

/// Whether `text` is one of Olo's English sentences, whole or with its
/// parts put in (for finding words never asked for: `finding_unasked`).
fn is_english(text: &str) -> bool {
    static PATTERNS: OnceLock<Vec<Pattern>> = OnceLock::new();
    english().iter().any(|e| *e == text) || best(PATTERNS.get_or_init(|| english().iter().filter_map(|k| Pattern::of(k)).collect()), text).is_some()
}

/// A phrase said on its own ("Partly cloudy") set into the middle of a
/// sentence ("44°, partly cloudy"): its first letter made small, as the
/// language has it, but kept in a language that writes every noun with a
/// capital ("44°, Regen", not "regen").
pub fn mid_sentence(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) if !current().capital_nouns => first.to_lowercase().chain(chars).collect(),
        _ => text.to_owned(),
    }
}

/// A sentence that counts something, in the language's form for `n`:
/// `one` and `other` are the English ("1 window", "{n} windows"), and
/// `{n}` in whichever is used is the number. A language's forms are in
/// its catalog under `other`.
pub fn count(n: u64, one: &'static str, other: &'static str) -> String {
    let out = count_in(n, one, other);
    said(&out);
    out
}

fn count_in(n: u64, one: &'static str, other: &'static str) -> String {
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

/// Olo's words, in English: every literal the kit was asked for across
/// the layout lint's windows (`data/words/en.txt`, made with
/// `OLO_WORDS_SEEN`: HACKING.md, "Words in the person's language"),
/// for saying how far a translation has got and as what's to translate.
pub fn english() -> &'static [&'static str] {
    static LIST: OnceLock<Vec<&'static str>> = OnceLock::new();
    LIST.get_or_init(|| text_of("en").lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')).map(plain).collect())
}

/// How many of a language's words `asked` (the English) has: for
/// saying how far a translation has got.
pub fn covered(language: &'static Language, asked: impl Iterator<Item = &'static str>) -> (usize, usize) {
    if language.code == "en" {
        let n = asked.count();
        return (n, n);
    }
    let catalog = catalog(language);
    asked.fold((0, 0), |(have, all), text| (have + usize::from(catalog.words.contains_key(text)), all + 1))
}

/// Words for the person that another program shows, kept in English
/// with their parts put in: `en!("the keyring can't be read: {}", e)`
/// (`format!` as the catalogs know it). The window showing them gives
/// them in the person's language with [`of_said`](crate::words::of_said).
#[macro_export]
macro_rules! en {
    ($text:literal) => {
        $crate::words::en($text)
    };
    ($text:literal, $($part:expr),+ $(,)?) => {
        $crate::words::en_with($text, &[$(&$part as &dyn ::std::fmt::Display),+])
    };
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

    /// What a catalog costs a program that starts in its language
    /// (`cargo test --release -p olo-kit-base --lib what_a_catalog_costs
    /// -- --ignored --nocapture`): its file read, and its table made.
    /// (Compiled in, the table alone: 0.96 ms; 2026-11-20.)
    #[test]
    #[ignore]
    fn what_a_catalog_costs() {
        let file = olo_xdg::data_dirs().iter().map(|d| d.join("words/de.txt")).find(|p| p.is_file()).expect("data/words/de.txt");
        for round in 0..3 {
            let t = std::time::Instant::now();
            let text: &'static str = Box::leak(std::fs::read_to_string(&file).unwrap().into_boxed_str());
            let was_read = t.elapsed();
            let n = std::hint::black_box(read(text)).len();
            eprintln!("round {round}: {n} words read and made a table in {:.2?} (the read {was_read:.2?}, {} KB)", t.elapsed(), text.len() / 1024);
        }
    }

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

    /// What another program said, found in a catalog: whole; by its
    /// words around its parts, which go into the translation's places
    /// (in its own order); a count's in the form for its number; of two
    /// that fit, the one that says more; and nothing for what isn't one
    /// of Olo's, or has too few words of its own to tell.
    #[test]
    fn what_another_program_said() {
        let c = Catalog {
            language: language("de").unwrap(),
            words: read(
                "Paused\nPausiert\n\nthe keyring can't be read: {}\nder Schlüsselbund ist nicht lesbar: {}\n\nDeleting {} of {}\nVon {1} gelöscht: {0}\n\nStarted {} ago\nVor {} begonnen\n\nStarted {} ago, at {}\nVor {} begonnen, um {}\n\n{n} backups and the unfinished one\none: {n} Sicherung und die unfertige\nother: {n} Sicherungen und die unfertige\n\n{}: {}\n{}: {}\n\n{}{} more\n{}{} mehr\n",
            ),
            patterns: OnceLock::new(),
            told: Mutex::default(),
        };
        assert_eq!(c.retell("Paused").as_deref(), Some("Pausiert"));
        assert_eq!(c.retell("the keyring can't be read: it's locked").as_deref(), Some("der Schlüsselbund ist nicht lesbar: it's locked"));
        assert_eq!(c.retell("Deleting 3 of 12").as_deref(), Some("Von 12 gelöscht: 3"));
        assert_eq!(c.retell("Started 5 min ago, at 9:00").as_deref(), Some("Vor 5 min begonnen, um 9:00"), "the one that says more");
        assert_eq!(c.retell("Started 5 min ago").as_deref(), Some("Vor 5 min begonnen"));
        assert_eq!(c.retell("1 backups and the unfinished one").as_deref(), Some("1 Sicherung und die unfertige"));
        assert_eq!(c.retell("4 backups and the unfinished one").as_deref(), Some("4 Sicherungen und die unfertige"));
        assert_eq!(c.retell("some backups and the unfinished one"), None, "a count's part is a number");
        assert_eq!(c.retell("Disk: full"), None, "too few words of its own");
        assert_eq!(c.retell("ab more"), None, "two parts side by side");
        assert_eq!(c.retell("Started  ago"), None, "a part is something");
        assert_eq!(c.retell("A name"), None);
        assert_eq!(c.retell("the keyring can't be read: it's locked").as_deref(), Some("der Schlüsselbund ist nicht lesbar: it's locked"), "kept");
    }

    /// A phrase in a sentence: small in English, its noun's capital kept
    /// in German.
    #[test]
    fn a_phrase_mid_sentence() {
        assert_eq!(mid_sentence("Partly cloudy"), "partly cloudy");
        assert_eq!(mid_sentence(""), "");
        assert!(language("de").unwrap().capital_nouns && !language("fr").unwrap().capital_nouns);
    }

    /// `en!`: English, its parts put in, whatever the language.
    #[test]
    fn words_for_another_program() {
        assert_eq!(crate::en!("not set up yet"), "not set up yet");
        assert_eq!(crate::en!("the keyring can't be read: {}", "locked"), "the keyring can't be read: locked");
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
        assert!(!english().is_empty(), "Olo's word list wasn't found (data/words/en.txt)");
        for language in LANGUAGES.iter().filter(|l| l.code != "en") {
            let words = read(text_of(language.code));
            assert!(!words.is_empty(), "{}: its catalog wasn't found (data/words/{}.txt)", language.code, language.code);
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
                // (A count's form may say it without its number: "Jeden
                // Tag" for "Every {n} days".)
                let numberless: Vec<String> = wanted.iter().filter(|x| *x != "{n}").cloned().collect();
                for text in std::iter::once(entry.text).chain(entry.forms.iter().map(|(_, t)| *t)).filter(|t| !t.is_empty()) {
                    let got = own(text);
                    assert!(got == wanted || (english.contains("{n}") && got == numberless), "{}: “{english}” is “{text}”", language.code);
                }
                assert!(!entry.text.is_empty() || !entry.forms.is_empty(), "{}: “{english}” has no translation", language.code);
                // (A space at the end is a translation's only where it's
                // its English's: one by chance would be read as its own.)
                let spaced = |t: &str| t.ends_with(char::is_whitespace);
                assert!(!spaced(entry.text) || spaced(english), "{}: “{}” ends in a space “{english}” doesn't", language.code, entry.text);
            }
            if words.len() > 4000 {
                assert!(words.get("About ").is_some_and(|e| e.text.ends_with(' ')), "{}: “About ” not found as itself", language.code);
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
