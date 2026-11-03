//! Spelling, natively: the system's Hunspell dictionaries
//! (`/usr/share/hunspell`, or `~/.local/share/hunspell`) read by
//! spellbook, in the person's language (`LANG`, else American English);
//! words learnt (Learn Spelling) are kept in `~/.config/olo/words`, one a
//! line, for every app. The kit loads the dictionary on a worker the
//! first time it's wanted ([`load`]) and draws again when it's ready
//! (`olo_ui::spell::checker`).

use olo_sync::RwLock;
use std::{collections::HashSet, ops::Range, path::PathBuf};

pub struct Checker {
    dict: spellbook::Dictionary,
    /// Words learnt (saved) and ignored (until the app quits).
    learnt: RwLock<HashSet<String>>,
    ignored: RwLock<HashSet<String>>,
    /// The dictionary's language ("en_US").
    pub language: String,
}

/// The language asked for: `LC_ALL`, `LC_MESSAGES` or `LANG` ("en_GB"
/// from "en_GB.UTF-8"), else American English.
fn language() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .map(|v| v.split(['.', '@']).next().unwrap_or("").to_owned())
        .find(|v| v.len() >= 2 && v != "C" && v != "POSIX")
        .unwrap_or_else(|| "en_US".into())
}

fn dirs() -> Vec<PathBuf> {
    let mut d = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        d.push(PathBuf::from(home).join(".local/share/hunspell"));
    }
    d.extend(["/usr/share/hunspell", "/usr/share/myspell", "/usr/share/myspell/dicts"].map(PathBuf::from));
    d
}

/// The dictionary for the language (exactly, else another of its
/// language, else American English).
pub fn load() -> Option<Checker> {
    let want = language();
    let base = want.split('_').next().unwrap_or("en").to_owned();
    let found: Vec<(PathBuf, String)> = dirs()
        .into_iter()
        .filter_map(|d| std::fs::read_dir(&d).ok().map(|r| (d, r)))
        .flat_map(|(d, r)| r.flatten().filter_map(move |e| Some((d.clone(), e.file_name().to_str()?.strip_suffix(".dic")?.to_owned()))))
        .collect();
    let pick = |f: &dyn Fn(&str) -> bool| found.iter().find(|(_, n)| f(n)).cloned();
    let (dir, name) = pick(&|n| n == want).or_else(|| pick(&|n| n == base || n.starts_with(&format!("{base}_")))).or_else(|| pick(&|n| n == "en_US"))?;
    let aff = std::fs::read_to_string(dir.join(format!("{name}.aff"))).ok()?;
    let dic = std::fs::read_to_string(dir.join(format!("{name}.dic"))).ok()?;
    let dict = match spellbook::Dictionary::new(&aff, &dic) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(dictionary = name, "can't read the dictionary: {e}");
            return None;
        }
    };
    let learnt = std::fs::read_to_string(words_file()).unwrap_or_default().lines().map(str::trim).filter(|w| !w.is_empty()).map(str::to_owned).collect();
    tracing::info!(dictionary = name, "spelling ready");
    Some(Checker { dict, learnt: RwLock::new(learnt), ignored: RwLock::new(HashSet::new()), language: name })
}

fn words_file() -> PathBuf {
    olo_xdg::config_dir().join("words")
}

impl Checker {
    /// Whether `word` is spelt right (or learnt, or ignored).
    pub fn check(&self, word: &str) -> bool {
        self.dict.check(word) || self.learnt.read().is_ok_and(|l| l.contains(word)) || self.ignored.read().is_ok_and(|l| l.contains(word))
    }

    /// The misspelled words of `text` (bytes).
    pub fn misspelled(&self, text: &str) -> Vec<Range<usize>> {
        words(text).filter(|r| !self.check(&text[r.clone()])).collect()
    }

    /// What `word` might have been meant to be, likeliest first (at most
    /// eight).
    pub fn suggest(&self, word: &str) -> Vec<String> {
        let mut out = Vec::new();
        self.dict.suggest(word, &mut out);
        out.truncate(8);
        out
    }

    /// Never mark `word` again, in any app (saved, on a thread).
    pub fn learn(&self, word: &str) {
        if let Ok(mut l) = self.learnt.write() {
            l.insert(word.to_owned());
            let mut all: Vec<String> = l.iter().cloned().collect();
            all.sort();
            let _ = std::thread::Builder::new().name("spelling-learn".into()).spawn(move || {
                let path = words_file();
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(path, all.join("\n") + "\n");
            });
        }
    }

    /// Don't mark `word` again while this app runs.
    pub fn ignore(&self, word: &str) {
        if let Ok(mut l) = self.ignored.write() {
            l.insert(word.to_owned());
        }
    }
}

/// The words of `text` to check (bytes): runs of letters, with
/// apostrophes inside; tokens that look like addresses, paths, code or
/// numbers (with `://`, `@`, `/`, `\`, `_`, a digit, a dot between letters
/// or a capital after a lower-case letter) and words in capitals (an
/// acronym) are left alone.
pub fn words(text: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    let mut tokens = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(i),
            (true, Some(s)) => {
                tokens.push(s..i);
                start = None;
            }
            _ => {}
        }
    }
    tokens.into_iter().filter(|t| plain(&text[t.clone()])).flat_map(move |t| {
        let token = &text[t.clone()];
        let mut out = Vec::new();
        let mut w: Option<usize> = None;
        let chars: Vec<(usize, char)> = token.char_indices().collect();
        for (k, &(i, c)) in chars.iter().enumerate() {
            let apostrophe = matches!(c, '\'' | '’') && w.is_some() && chars.get(k + 1).is_some_and(|(_, n)| n.is_alphabetic());
            if c.is_alphabetic() || apostrophe {
                w.get_or_insert(i);
            } else if let Some(s) = w.take() {
                out.push(t.start + s..t.start + i);
            }
        }
        if let Some(s) = w {
            out.push(t.start + s..t.end);
        }
        out.into_iter().filter(move |r| {
            let word = &text[r.clone()];
            // An acronym (all capitals) or a single letter: left alone.
            word.chars().count() > 1 && !word.chars().all(|c| !c.is_lowercase())
        })
    })
}

/// A token of prose (not an address, path, number or identifier).
fn plain(token: &str) -> bool {
    if token.contains("://") || token.contains(['@', '/', '\\', '_']) || token.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    let chars: Vec<char> = token.chars().collect();
    !chars.windows(3).any(|w| w[0].is_alphabetic() && w[1] == '.' && w[2].is_alphabetic()) && !chars.windows(2).any(|w| w[0].is_lowercase() && w[1].is_uppercase())
}

/// The word (checkable, as [`words`] finds them) holding or touching byte
/// `at` of `text`.
pub fn word_at(text: &str, at: usize) -> Option<Range<usize>> {
    // Only the line around it is looked at.
    let start = text[..at.min(text.len())].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at.min(text.len())..].find('\n').map_or(text.len(), |i| at + i);
    words(&text[start..end]).map(|r| start + r.start..start + r.end).find(|r| r.start <= at && at <= r.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(t: &str) -> Vec<&str> {
        words(t).map(|r| &t[r]).collect()
    }

    #[test]
    fn words_of_prose_only() {
        assert_eq!(found("Hello, world! It's fine."), ["Hello", "world", "It's", "fine"]);
        assert_eq!(found("see https://example.com or a@b.c and src/main.rs"), ["see", "or", "and"]);
        assert_eq!(found("NASA uses camelCase and v2 x"), ["uses", "and"]);
        assert_eq!(found("‘quoted’ words"), ["quoted", "words"]);
    }

    #[test]
    fn word_at_the_caret() {
        let t = "one two\nthree";
        assert_eq!(word_at(t, 5), Some(4..7));
        assert_eq!(word_at(t, 7), Some(4..7));
        assert_eq!(word_at(t, 9), Some(8..13));
    }
}
