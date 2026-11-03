//! The emoji: every one the emoji font draws, in Unicode's order, with
//! its name, CLDR's keywords and GitHub's shortcodes to find it by; what
//! was picked lately; and the search, ranked as you type.

use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

/// The groups, in Unicode's order, each with the emoji its tab shows.
pub const GROUPS: [(&str, &str); 9] = [
    ("Smileys & Emotion", "😀"),
    ("People & Body", "👋"),
    ("Animals & Nature", "🐻"),
    ("Food & Drink", "🍔"),
    ("Travel & Places", "✈️"),
    ("Activities", "⚽"),
    ("Objects", "💡"),
    ("Symbols", "🔣"),
    ("Flags", "🏁"),
];

/// The most used, most first (Unicode's emoji frequency, 2021): among
/// matches as good, these come first.
const COMMON: &[&str] = &[
    "😂", "❤️", "🤣", "👍", "😭", "🙏", "😘", "🥰", "😍", "😊", "🎉", "😁", "💕", "🥺", "😅", "🔥", "☺️", "🤦", "♥️", "🤷",
    "🙄", "😆", "🤗", "😉", "🎂", "🤔", "👏", "🙂", "😳", "🥳", "😎", "👌", "💜", "😔", "💪", "✨", "💖", "👀", "😋", "😏",
    "😢", "👉", "💗", "😩", "💯", "🌹", "💞", "🎈", "💙", "😃",
];

pub struct Emoji {
    pub e: &'static emojis::Emoji,
    pub group: usize,
    /// Lowercase: the name's words (the first `own`), the keywords, the
    /// shortcodes.
    words: Vec<String>,
    own: usize,
    name: String,
    /// How much more it counts for being used a lot (`COMMON`).
    common: i32,
}

impl Emoji {
    pub fn name(&self) -> &'static str {
        self.e.name()
    }

    /// As shown, in `tone` if it comes in tones.
    pub fn glyph(&self, tone: Tone) -> &'static str {
        match tone.0.and_then(|t| self.e.with_skin_tone(t)) {
            Some(toned) => toned.as_str(),
            None => self.e.as_str(),
        }
    }

    pub fn shortcode(&self) -> Option<&'static str> {
        self.e.shortcode()
    }

    /// The keywords, for the status line.
    pub fn keywords(&self) -> &[String] {
        &self.words
    }
}

/// A skin tone (None: the yellow of no tone).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tone(pub Option<emojis::SkinTone>);

impl Tone {
    pub const ALL: [Tone; 6] = [
        Tone(None),
        Tone(Some(emojis::SkinTone::Light)),
        Tone(Some(emojis::SkinTone::MediumLight)),
        Tone(Some(emojis::SkinTone::Medium)),
        Tone(Some(emojis::SkinTone::MediumDark)),
        Tone(Some(emojis::SkinTone::Dark)),
    ];

    pub fn name(self) -> &'static str {
        match self.0 {
            None => "No skin tone",
            Some(emojis::SkinTone::Light) => "Light skin tone",
            Some(emojis::SkinTone::MediumLight) => "Medium-light skin tone",
            Some(emojis::SkinTone::Medium) => "Medium skin tone",
            Some(emojis::SkinTone::MediumDark) => "Medium-dark skin tone",
            Some(emojis::SkinTone::Dark) => "Dark skin tone",
            Some(_) => "Skin tone",
        }
    }

    fn index(self) -> usize {
        Tone::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

fn group_of(g: emojis::Group) -> usize {
    use emojis::Group as G;
    match g {
        G::SmileysAndEmotion => 0,
        G::PeopleAndBody => 1,
        G::AnimalsAndNature => 2,
        G::FoodAndDrink => 3,
        G::TravelAndPlaces => 4,
        G::Activities => 5,
        G::Objects => 6,
        G::Symbols => 7,
        G::Flags => 8,
    }
}

/// Every emoji `draws` says the font draws whole (off the UI thread:
/// ~1,900 shaped).
pub fn load(draws: impl Fn(&str) -> bool) -> Vec<Emoji> {
    emojis::iter()
        .filter(|e| e.skin_tone().is_none_or(|t| t == emojis::SkinTone::Default))
        .filter_map(|e| {
            let group = group_of(e.group());
            if !draws(e.as_str()) {
                return None;
            }
            let name = e.name().to_lowercase();
            let mut words: Vec<String> = Vec::new();
            fn add(words: &mut Vec<String>, w: &str) {
                let w = w.to_lowercase();
                if !w.is_empty() && !words.contains(&w) {
                    words.push(w);
                }
            }
            for w in name.split([' ', ':', ',', '-']) {
                add(&mut words, w);
            }
            let own = words.len();
            let cldr = emoji::lookup_by_glyph::lookup(e.as_str()).or_else(|| emoji::lookup_by_glyph::lookup(e.as_str().trim_end_matches('\u{fe0f}')));
            if let Some(found) = cldr {
                for a in found.annotations {
                    for k in a.keywords {
                        add(&mut words, k);
                    }
                }
            }
            for s in e.shortcodes() {
                add(&mut words, s);
                for w in s.split('_') {
                    add(&mut words, w);
                }
            }
            let common = COMMON.iter().position(|c| *c == e.as_str()).map_or(0, |n| 10 - n as i32 / 5);
            Some(Emoji { e, group, words, own, name, common })
        })
        .collect()
}

/// Whether the colour emoji font draws `cluster` as one glyph: the font
/// GPUI draws emoji with, found as it finds it. None: no such font (all
/// are offered).
pub fn font_check() -> Option<impl Fn(&str) -> bool> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let query = fontdb::Query { families: &[fontdb::Family::Name("Noto Color Emoji")], ..Default::default() };
    let id = db.query(&query)?;
    let (source, index) = db.face_source(id)?;
    let path = match source {
        fontdb::Source::File(path) | fontdb::Source::SharedFile(path, _) => path,
        fontdb::Source::Binary(_) => return None,
    };
    // Mapped, not read: the font's 5 MB stay the file's (shared, and
    // let go by the system as it likes), not the picker's own.
    // SAFETY: a system font, mapped read-only, as GPUI maps its faces.
    let map = unsafe { memmap2::Mmap::map(&std::fs::File::open(path).ok()?) }.ok()?;
    let font = olo_emoji::ColorFont::from_static(&Box::leak(Box::new(map))[..], index)?;
    Some(move |cluster: &str| font.glyph(cluster).is_some())
}

/// How well `emoji` answers the query's `words` (all must match
/// something), or None.
pub fn score(emoji: &Emoji, words: &[String]) -> Option<i32> {
    let mut total = 0;
    for q in words {
        let mut best = 0;
        for (i, w) in emoji.words.iter().enumerate() {
            // The name's own words count more than keywords.
            let own = if i < emoji.own { 10 } else { 0 };
            let s = if w == q {
                100 + own
            } else if w.starts_with(q.as_str()) {
                60 + own - (w.len() - q.len()).min(20) as i32
            } else if q.len() >= 4 && w.contains(q.as_str()) {
                30
            } else if q.len() >= 4 && near(q, w) {
                // A typo, or letters left out ("thmbs").
                12
            } else {
                0
            };
            best = best.max(s);
        }
        if best == 0 {
            return None;
        }
        total += best;
    }
    // The name as typed first; then the names with less else in them
    // ("red heart" before "heart with arrow").
    if emoji.name == words.join(" ") {
        total += 60;
    }
    Some(total - 4 * emoji.own.saturating_sub(words.len()) as i32 - emoji.name.len() as i32 / 5 + emoji.common)
}

/// `q`'s letters in order in `word`, from its first, and not much
/// shorter: what's left of a word typed with letters missing.
fn near(q: &str, word: &str) -> bool {
    if q.chars().next() != word.chars().next() || word.len() > q.len() + 2 {
        return false;
    }
    let mut it = word.chars();
    q.chars().all(|c| it.any(|h| h == c))
}

/// The query's words, lowercase.
pub fn words(query: &str) -> Vec<String> {
    query.split_whitespace().map(|w| w.trim_matches(':').to_lowercase()).filter(|w| !w.is_empty()).collect()
}

/// The emoji matching `query`, best first (ties: picked more, then
/// Unicode's order).
pub fn search(all: &[Emoji], query: &str, recents: &Recents) -> Vec<usize> {
    let words = words(query);
    let mut found: Vec<(i32, usize)> = all.iter().enumerate().filter_map(|(i, e)| score(e, &words).map(|s| (s + recents.boost(e.e.as_str()), i))).collect();
    found.sort_by_key(|&(s, i)| (std::cmp::Reverse(s), i));
    found.into_iter().map(|(_, i)| i).collect()
}

/// What was picked, how often and when; and the skin tone chosen.
#[derive(Default, Serialize, Deserialize)]
pub struct Recents {
    picked: HashMap<String, (u32, u64)>,
    #[serde(default)]
    tone: usize,
}

impl Recents {
    fn path() -> PathBuf {
        olo_xdg::config_dir().join("emoji.json")
    }

    pub fn load() -> Recents {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string(self) {
            let _ = std::fs::write(path, s);
        }
    }

    fn now() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }

    /// `base` (the emoji without its tone) picked.
    pub fn note(&mut self, base: &str) {
        let e = self.picked.entry(base.to_owned()).or_default();
        e.0 += 1;
        e.1 = Self::now();
        if self.picked.len() > 200 {
            let oldest = self.picked.iter().min_by_key(|(_, v)| v.1).map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                self.picked.remove(&k);
            }
        }
        self.save();
    }

    /// How much more `base` counts for being picked often and lately.
    pub fn boost(&self, base: &str) -> i32 {
        let Some(&(count, when)) = self.picked.get(base) else { return 0 };
        let days = Self::now().saturating_sub(when) / 86_400;
        count.min(20) as i32 * 3 + if days < 1 { 20 } else if days < 7 { 10 } else { 0 }
    }

    /// The ones picked most, often and lately, first.
    pub fn most(&self, all: &[Emoji], n: usize) -> Vec<usize> {
        let mut found: Vec<(i32, usize)> = all.iter().enumerate().filter_map(|(i, e)| self.picked.contains_key(e.e.as_str()).then(|| (self.boost(e.e.as_str()), i))).collect();
        found.sort_by_key(|&(b, i)| (std::cmp::Reverse(b), i));
        found.into_iter().take(n).map(|(_, i)| i).collect()
    }

    pub fn tone(&self) -> Tone {
        Tone::ALL.get(self.tone).copied().unwrap_or_default()
    }

    pub fn set_tone(&mut self, tone: Tone) {
        self.tone = tone.index();
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<Emoji> {
        load(|_| true)
    }

    #[test]
    fn finds_by_name_keyword_shortcode_and_typo() {
        let all = all();
        let r = Recents::default();
        let first = |q: &str| search(&all, q, &r).first().map(|&i| all[i].e.as_str());
        assert_eq!(first("thumbs up"), Some("👍"));
        assert_eq!(first("tada"), Some("🎉"));
        assert_eq!(first(":joy:"), Some("😂"));
        assert_eq!(first("thmbs up"), Some("👍"));
        assert!(search(&all, "celebration", &r).iter().any(|&i| all[i].e.as_str() == "🎉"));
        assert!(search(&all, "xqzv", &r).is_empty());
        // The plainest first, and nothing whose letters only happen to
        // come in order across its name.
        assert_eq!(first("heart"), Some("❤️"));
        assert!(!search(&all, "heart", &r).iter().any(|&i| all[i].e.as_str() == "✅"));
        assert!(!search(&all, "cat", &r).iter().any(|&i| ["🌵", "🥕", "🏰", "📈"].contains(&all[i].e.as_str())));
        assert_eq!(first("cat"), Some("🐈"));
    }

    #[test]
    fn tones_and_groups() {
        let all = all();
        let wave = all.iter().find(|e| e.e.as_str() == "👋").unwrap();
        assert!(wave.e.skin_tones().is_some());
        assert_eq!(wave.group, 1);
        assert_eq!(wave.glyph(Tone::ALL[3]), "👋🏽");
        assert_eq!(wave.glyph(Tone::ALL[0]), "👋");
        // Toned ones are the base's, not listed again.
        assert!(!all.iter().any(|e| e.e.as_str() == "👋🏽"));
        let flag = all.iter().find(|e| e.e.as_str() == "🇳🇴").unwrap();
        assert_eq!(flag.group, 8);
        assert!(flag.e.skin_tones().is_none());
    }

    #[test]
    fn picked_lately_come_first() {
        let all = all();
        let mut r = Recents::default();
        let party = all.iter().position(|e| e.e.as_str() == "🎉").unwrap();
        let popper = all.iter().position(|e| e.e.as_str() == "🪅").unwrap_or(party);
        r.picked.insert("🎉".into(), (5, Recents::now()));
        assert_eq!(r.most(&all, 3), vec![party]);
        assert_eq!(search(&all, "party", &r)[0], party);
        let _ = popper;
    }
}
