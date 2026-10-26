//! Password generation (`PasswordRecipe`), from the system's random source.
//!
//! Memorable passwords use the EFF large word list (CC BY 3.0, see
//! `data/README.md`), pronounceable made-up syllables, or the list's
//! three-letter words.

use std::sync::OnceLock;

use crate::{Error, PasswordRecipe, SeparatorType, WordListType};

const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const DIGITS: &str = "0123456789";
/// Symbols every site accepts.
const SYMBOLS: &str = "!@#$%^&*-_=+.?";
const WORDS: &str = include_str!("../data/eff_large_wordlist.txt");

pub const RANDOM_LENGTH: std::ops::RangeInclusive<u32> = 8..=100;
pub const PIN_LENGTH: std::ops::RangeInclusive<u32> = 3..=64;
pub const WORD_COUNT: std::ops::RangeInclusive<u32> = 3..=15;

fn words() -> &'static [&'static str] {
    static LIST: OnceLock<Vec<&str>> = OnceLock::new();
    LIST.get_or_init(|| WORDS.lines().filter(|w| !w.is_empty()).collect())
}

fn short_words() -> &'static [&'static str] {
    static LIST: OnceLock<Vec<&str>> = OnceLock::new();
    LIST.get_or_init(|| words().iter().copied().filter(|w| w.len() == 3).collect())
}

/// A uniform number below `n` (rejection sampling, so no modulo bias).
pub fn below(n: usize) -> usize {
    assert!(n > 0 && n <= u32::MAX as usize);
    let n = n as u32;
    let zone = u32::MAX - (u32::MAX % n);
    loop {
        let mut b = [0u8; 4];
        getrandom::fill(&mut b).expect("the system's random source failed");
        let v = u32::from_le_bytes(b);
        if v < zone {
            return (v % n) as usize;
        }
    }
}

fn pick(set: &str) -> char {
    let chars: Vec<char> = set.chars().collect();
    chars[below(chars.len())]
}

fn check(range: std::ops::RangeInclusive<u32>, value: u32, what: &str) -> Result<usize, Error> {
    if range.contains(&value) {
        Ok(value as usize)
    } else {
        Err(Error::InvalidArgument(format!("{what} must be from {} to {}", range.start(), range.end())))
    }
}

/// Makes a password to `recipe`.
pub fn password(recipe: PasswordRecipe) -> Result<String, Error> {
    match recipe {
        PasswordRecipe::Pin { length } => {
            let length = check(PIN_LENGTH, length, "a PIN's length")?;
            Ok((0..length).map(|_| pick(DIGITS)).collect())
        }
        PasswordRecipe::Random { include_digits, include_symbols, length } => {
            let length = check(RANDOM_LENGTH, length, "a password's length")?;
            let mut pool = format!("{LOWER}{UPPER}");
            let mut required = vec![LOWER, UPPER];
            if include_digits {
                pool.push_str(DIGITS);
                required.push(DIGITS);
            }
            if include_symbols {
                pool.push_str(SYMBOLS);
                required.push(SYMBOLS);
            }
            // Draw until every asked-for kind is present: uniform over the
            // passwords that qualify.
            loop {
                let candidate: String = (0..length).map(|_| pick(&pool)).collect();
                if required.iter().all(|set| candidate.chars().any(|c| set.contains(c))) {
                    return Ok(candidate);
                }
            }
        }
        PasswordRecipe::Memorable { separator, capitalize, word_list, word_count } => {
            let count = check(WORD_COUNT, word_count, "the number of words")?;
            let mut out = String::new();
            for i in 0..count {
                if i > 0 {
                    out.push(match separator {
                        SeparatorType::Digits => pick(DIGITS),
                        SeparatorType::DigitsAndSymbols => pick(&format!("{DIGITS}{SYMBOLS}")),
                        SeparatorType::Spaces => ' ',
                        SeparatorType::Hyphens => '-',
                        SeparatorType::Underscores => '_',
                        SeparatorType::Periods => '.',
                        SeparatorType::Commas => ',',
                    });
                }
                let word = match word_list {
                    WordListType::FullWords => words()[below(words().len())].to_string(),
                    WordListType::ThreeLetters => short_words()[below(short_words().len())].to_string(),
                    WordListType::Syllables => syllable(),
                };
                if capitalize {
                    let mut chars = word.chars();
                    out.extend(chars.next().map(|c| c.to_ascii_uppercase()));
                    out.extend(chars);
                } else {
                    out.push_str(&word);
                }
            }
            Ok(out)
        }
    }
}

const ONSETS: &[&str] = &[
    "b", "c", "d", "f", "g", "h", "j", "k", "l", "m", "n", "p", "r", "s", "t", "v", "w", "z", "br", "ch", "cl", "dr",
    "fl", "fr", "gl", "gr", "pl", "pr", "sh", "sk", "sl", "sm", "sn", "sp", "st", "th", "tr", "tw",
];
const VOWELS: &[&str] = &["a", "e", "i", "o", "u", "ai", "ea", "ee", "oa", "oo", "ou"];
const CODAS: &[&str] = &["", "", "b", "d", "g", "k", "l", "m", "n", "p", "r", "s", "t", "x", "nd", "nt", "st"];

/// A pronounceable made-up syllable ("blee", "stand", "kou").
fn syllable() -> String {
    format!("{}{}{}", ONSETS[below(ONSETS.len())], VOWELS[below(VOWELS.len())], CODAS[below(CODAS.len())])
}

/// How many bits of randomness a recipe's passwords have.
pub fn entropy_bits(recipe: PasswordRecipe) -> f64 {
    match recipe {
        PasswordRecipe::Pin { length } => length as f64 * 10f64.log2(),
        PasswordRecipe::Random { include_digits, include_symbols, length } => {
            let mut pool = 52;
            if include_digits {
                pool += DIGITS.len();
            }
            if include_symbols {
                pool += SYMBOLS.len();
            }
            length as f64 * (pool as f64).log2()
        }
        PasswordRecipe::Memorable { separator, word_list, word_count, .. } => {
            let per_word = match word_list {
                WordListType::FullWords => words().len() as f64,
                WordListType::ThreeLetters => short_words().len() as f64,
                WordListType::Syllables => (ONSETS.len() * VOWELS.len() * CODAS.len()) as f64,
            }
            .log2();
            let per_separator = match separator {
                SeparatorType::Digits => (DIGITS.len() as f64).log2(),
                SeparatorType::DigitsAndSymbols => ((DIGITS.len() + SYMBOLS.len()) as f64).log2(),
                _ => 0.0,
            };
            word_count as f64 * per_word + word_count.saturating_sub(1) as f64 * per_separator
        }
    }
}

/// A rough strength estimate for any password, in bits: the alphabet it
/// draws from times its length, discounting repeats, sequences and words
/// from the list.
pub fn strength_bits(password: &str) -> f64 {
    if password.is_empty() {
        return 0.0;
    }
    let mut pool = 0usize;
    if password.chars().any(|c| c.is_ascii_lowercase()) {
        pool += 26;
    }
    if password.chars().any(|c| c.is_ascii_uppercase()) {
        pool += 26;
    }
    if password.chars().any(|c| c.is_ascii_digit()) {
        pool += 10;
    }
    if password.chars().any(|c| c.is_ascii_punctuation() || c == ' ') {
        pool += 33;
    }
    if password.chars().any(|c| !c.is_ascii()) {
        pool += 100;
    }
    let chars: Vec<char> = password.chars().collect();
    // Characters that repeat or continue a run (aaa, abc, 123) add little.
    let mut effective = 1.0;
    for w in chars.windows(2) {
        let step = w[1] as i64 - w[0] as i64;
        effective += if step == 0 || step.abs() == 1 { 0.25 } else { 1.0 };
    }
    let mut bits = effective * (pool as f64).log2();
    // A dictionary word, perhaps capitalised or with a digit or two.
    let core: String = password.trim_end_matches(|c: char| c.is_ascii_digit() || c.is_ascii_punctuation()).to_lowercase();
    if words().binary_search(&core.as_str()).is_ok() || COMMON.contains(&core.as_str()) {
        bits = bits.min(16.0);
    }
    bits
}

/// Passwords people use most, which a guesser tries first.
const COMMON: &[&str] = &[
    "password", "passw0rd", "qwerty", "letmein", "welcome", "admin", "iloveyou", "monkey", "dragon", "sunshine",
    "princess", "football", "baseball", "master", "trustno1", "abc123", "123456", "12345678", "111111", "qwertyuiop",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_list_is_sorted_and_clean() {
        assert_eq!(words().len(), 7772);
        assert!(words().windows(2).all(|w| w[0] < w[1]));
        assert!(words().iter().all(|w| w.chars().all(|c| c.is_ascii_lowercase())));
        assert!(short_words().len() > 50);
    }

    #[test]
    fn random_has_what_was_asked() {
        for _ in 0..200 {
            let p = password(PasswordRecipe::Random { include_digits: true, include_symbols: true, length: 12 }).unwrap();
            assert_eq!(p.chars().count(), 12);
            assert!(p.chars().any(|c| c.is_ascii_digit()));
            assert!(p.chars().any(|c| SYMBOLS.contains(c)));
            assert!(p.chars().any(|c| c.is_ascii_uppercase()) && p.chars().any(|c| c.is_ascii_lowercase()));
            let p = password(PasswordRecipe::Random { include_digits: false, include_symbols: false, length: 8 }).unwrap();
            assert!(p.chars().all(|c| c.is_ascii_alphabetic()));
        }
        assert!(password(PasswordRecipe::Random { include_digits: true, include_symbols: true, length: 4 }).is_err());
    }

    #[test]
    fn pins_and_memorable() {
        let pin = password(PasswordRecipe::Pin { length: 6 }).unwrap();
        assert!(pin.len() == 6 && pin.chars().all(|c| c.is_ascii_digit()));
        let m = password(PasswordRecipe::Memorable {
            separator: SeparatorType::Hyphens,
            capitalize: true,
            word_list: WordListType::FullWords,
            word_count: 4,
        })
        .unwrap();
        let parts: Vec<&str> = m.split('-').collect();
        assert_eq!(parts.len(), 4);
        for part in parts {
            assert!(part.chars().next().unwrap().is_ascii_uppercase());
            assert!(words().contains(&part.to_lowercase().as_str()));
        }
        let m = password(PasswordRecipe::Memorable {
            separator: SeparatorType::Digits,
            capitalize: false,
            word_list: WordListType::ThreeLetters,
            word_count: 3,
        })
        .unwrap();
        assert_eq!(m.len(), 11);
    }

    #[test]
    fn uniform_enough() {
        let mut counts = [0usize; 10];
        for _ in 0..20_000 {
            counts[below(10)] += 1;
        }
        assert!(counts.iter().all(|&c| (1700..2300).contains(&c)), "{counts:?}");
    }

    #[test]
    fn entropy_and_strength() {
        let four_words = PasswordRecipe::Memorable {
            separator: SeparatorType::Hyphens,
            capitalize: false,
            word_list: WordListType::FullWords,
            word_count: 4,
        };
        assert!((entropy_bits(four_words) - 4.0 * 7772f64.log2()).abs() < 1e-9);
        assert!(strength_bits("password1") <= 16.0);
        assert!(strength_bits("aaaaaaaaaaaa") < strength_bits("q7#Kd9!mZp2x"));
        assert!(strength_bits("q7#Kd9!mZp2x") > 60.0);
    }
}
