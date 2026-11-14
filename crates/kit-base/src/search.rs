//! Searching and filtering, the same in every app: a query language
//! with matching and ranking ([`Query`]), the filters an app offers
//! ([`Facet`]) and what to suggest as one is typed ([`suggest`]). The
//! kit's search field and filter bar draw them (`olo_kit::search`); what
//! is typed and
//! what is chosen edit the same text, so they always agree (and an agent
//! can just type a query).
//!
//! The language: words (each must match, in any order, ignoring case),
//! `"quoted phrases"`, `-word` to leave out, and an app's filters
//! ([`Facet`]s) as `key:value`, `key:a,b` for either, `-key:value` to
//! leave out, `key:"two words"`. A `key:` the app doesn't know is only a
//! word, so `https://example.com` looks for itself.

use std::ops::Range;

// ---- What to look for ----------------------------------------------------

/// Something to look for, lower-cased once: case-insensitive, and fast
/// for ASCII (Logs looks through a million lines at a time).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Needle {
    lower: String,
    ascii: bool,
}

impl Needle {
    pub fn new(s: &str) -> Needle {
        let lower = s.to_lowercase();
        Needle { ascii: lower.is_ascii(), lower }
    }

    /// It, lower-cased.
    pub fn as_str(&self) -> &str {
        &self.lower
    }

    pub fn is_empty(&self) -> bool {
        self.lower.is_empty()
    }

    /// Whether `hay` has it anywhere (an empty one is everywhere).
    pub fn in_text(&self, hay: &str) -> bool {
        self.lower.is_empty() || self.find(hay).is_some()
    }

    /// Where in `hay` it first is, as a range of `hay`'s bytes.
    pub fn find(&self, hay: &str) -> Option<Range<usize>> {
        self.find_from(hay, 0)
    }

    fn find_from(&self, hay: &str, from: usize) -> Option<Range<usize>> {
        let n = self.lower.as_bytes();
        let first = *n.first()?;
        if self.ascii {
            let h = hay.as_bytes();
            let last = h.len().checked_sub(n.len())?;
            if from > last {
                return None;
            }
            return memchr::memchr2_iter(first, first.to_ascii_uppercase(), &h[from..=last]).map(|i| i + from).find(|&i| h[i + 1..i + n.len()].eq_ignore_ascii_case(&n[1..])).map(|i| i..i + n.len());
        }
        // Lower-cased a character at a time, so the range is `hay`'s.
        let want: Vec<char> = self.lower.chars().collect();
        for (i, _) in hay.char_indices().filter(|(i, _)| *i >= from) {
            let mut k = 0;
            for (j, ch) in hay[i..].char_indices() {
                let mut ok = true;
                for l in ch.to_lowercase() {
                    if want.get(k) != Some(&l) {
                        ok = false;
                        break;
                    }
                    k += 1;
                }
                if !ok {
                    break;
                }
                if k == want.len() {
                    return Some(i..i + j + ch.len_utf8());
                }
            }
        }
        None
    }

    /// Every place in `hay` it is, not overlapping.
    pub fn find_all(&self, hay: &str) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let mut at = 0;
        while let Some(r) = self.find_from(hay, at) {
            at = r.end.max(r.start + 1);
            out.push(r);
            if at >= hay.len() {
                break;
            }
        }
        out
    }

    /// Whether `value` is it, ignoring case.
    pub fn is(&self, value: &str) -> bool {
        if self.ascii { value.eq_ignore_ascii_case(&self.lower) } else { value.to_lowercase() == self.lower }
    }

    /// Whether `value` starts with it, ignoring case.
    pub fn starts(&self, value: &str) -> bool {
        self.find(value).is_some_and(|r| r.start == 0)
    }
}

// ---- The text of a query -------------------------------------------------

/// A piece of a query's text: a word, a phrase or a filter.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    /// Where it is in the text (bytes), the `-` included.
    pub range: Range<usize>,
    /// Left out (`-`).
    pub not: bool,
    /// A filter's key (one the app knows), and where it is.
    pub key: Option<(String, Range<usize>)>,
    /// What it looks for (any of them), and where each is.
    pub values: Vec<(String, Range<usize>)>,
}

/// The pieces of `text`; `keys` are the filters the app knows.
pub fn tokens(text: &str, keys: &[&str]) -> Vec<Token> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        // (A `-` alone is one being typed: nothing yet.)
        let not = b[i] == b'-';
        if not {
            i += 1;
        }
        let word_end = text[i..].find(|c: char| c.is_whitespace()).map_or(text.len(), |e| i + e);
        let key = text[i..word_end]
            .find(':')
            .map(|k| (&text[i..i + k], i..i + k))
            .filter(|(k, _)| !k.is_empty() && k.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_') && keys.iter().any(|known| known.eq_ignore_ascii_case(k)));
        let mut values = Vec::new();
        if let Some((k, kr)) = &key {
            i = kr.end + 1;
            // Values, a comma between, each bare or quoted.
            loop {
                let (v, r, next) = value(text, i, true);
                values.push((v, r));
                i = next;
                if b.get(i) == Some(&b',') {
                    i += 1;
                    continue;
                }
                break;
            }
            out.push(Token { range: start..i, not, key: Some((k.to_lowercase(), kr.clone())), values });
        } else {
            let (v, r, next) = value(text, i, false);
            i = next;
            values.push((v, r));
            out.push(Token { range: start..i, not, key: None, values });
        }
    }
    out
}

/// A value at `i`: quoted (to the closing quote), or bare (to a space, or
/// a comma in a filter). What it says, where, and where the text goes on.
fn value(text: &str, i: usize, in_filter: bool) -> (String, Range<usize>, usize) {
    let b = text.as_bytes();
    if b.get(i) == Some(&b'"') {
        let end = text[i + 1..].find('"').map_or(text.len(), |e| i + 1 + e);
        let next = if end < text.len() { end + 1 } else { end };
        return (text[i + 1..end].to_owned(), i + 1..end, next);
    }
    let end = text[i..].find(|c: char| c.is_whitespace() || in_filter && c == ',').map_or(text.len(), |e| i + e);
    (text[i..end].to_owned(), i..end, end)
}

/// `v` as it is typed in a query: quoted if it has a space, a comma or a
/// quote.
pub fn quote(v: &str) -> String {
    if v.is_empty() || v.contains(|c: char| c.is_whitespace() || c == ',' || c == '"') { format!("\"{}\"", v.replace('"', "")) } else { v.to_owned() }
}

/// `text` with filter `key` set to `values` (none: taken out): the
/// tokens choosing values for it go, and one with `values` goes at the
/// end. Those leaving values out stay.
pub fn set_filter(text: &mut String, keys: &[&str], key: &str, values: &[&str]) {
    let mut out = String::new();
    let mut at = 0;
    for t in tokens(text, keys) {
        if !t.not && t.key.as_ref().is_some_and(|(k, _)| k.eq_ignore_ascii_case(key)) {
            out.push_str(&text[at..t.range.start]);
            at = t.range.end;
        }
    }
    out.push_str(&text[at..]);
    let mut out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if !values.is_empty() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&format!("{key}:{}", values.iter().map(|v| quote(v)).collect::<Vec<_>>().join(",")));
    }
    *text = out;
}

// ---- The query -----------------------------------------------------------

/// One part of a query: what to find (any of these), in an item's text
/// or under `key`; or, with `not`, what mustn't be there.
#[derive(Clone, Debug, PartialEq)]
pub struct Term {
    pub key: Option<String>,
    pub any: Vec<Needle>,
    pub not: bool,
}

/// A query, made from its text with [`Query::parse`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub terms: Vec<Term>,
}

impl Query {
    /// `text` as a query; `keys` are the filters the app knows. Parts
    /// still being typed (`key:` with nothing after it) ask for nothing.
    pub fn parse(text: &str, keys: &[&str]) -> Query {
        let terms = tokens(text, keys)
            .into_iter()
            .filter_map(|t| {
                let any: Vec<Needle> = t.values.iter().filter(|(v, _)| !v.is_empty()).map(|(v, _)| Needle::new(v)).collect();
                (!any.is_empty()).then(|| Term { key: t.key.map(|(k, _)| k), any, not: t.not })
            })
            .collect();
        Query { terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// The words to find in an item's text (not those left out): what to
    /// mark in it.
    pub fn words(&self) -> impl Iterator<Item = &Needle> {
        self.terms.iter().filter(|t| t.key.is_none() && !t.not).flat_map(|t| &t.any)
    }

    /// The values chosen for filter `key`.
    pub fn values(&self, key: &str) -> Vec<&Needle> {
        self.terms.iter().filter(|t| !t.not && t.key.as_deref() == Some(key)).flat_map(|t| &t.any).collect()
    }

    /// The values left out with `-key:`.
    pub fn left_out(&self, key: &str) -> Vec<&Needle> {
        self.terms.iter().filter(|t| t.not && t.key.as_deref() == Some(key)).flat_map(|t| &t.any).collect()
    }

    /// Whether an item matches: `has(key, needle)` says whether it has
    /// `needle` under filter `key` (`None`: anywhere in its text). Every
    /// term must hold; a term holds when any of its values is there (or,
    /// left out, none is).
    pub fn matches(&self, mut has: impl FnMut(Option<&str>, &Needle) -> bool) -> bool {
        self.terms.iter().all(|t| t.any.iter().any(|n| has(t.key.as_deref(), n)) != t.not)
    }

    /// Whether an item whose text is `fields` has every word (filters
    /// aside, for the app to judge).
    pub fn words_in(&self, fields: &[&str]) -> bool {
        self.terms.iter().filter(|t| t.key.is_none()).all(|t| t.any.iter().any(|n| fields.iter().any(|f| n.in_text(f))) != t.not)
    }

    /// How well an item's `fields` (each with a weight) match the words,
    /// to put the best first: for each word, its best field: the whole of
    /// it 8, its start 4, a word's start in it 2, anywhere 1, times the
    /// field's weight.
    pub fn score(&self, fields: &[(&str, u32)]) -> u32 {
        self.words()
            .map(|n| {
                fields
                    .iter()
                    .map(|(f, w)| {
                        let s = if n.is(f) {
                            8
                        } else if n.starts(f) {
                            4
                        } else if n.find_all(f).iter().any(|r| r.start > 0 && !f[..r.start].chars().next_back().is_some_and(char::is_alphanumeric)) {
                            2
                        } else if n.in_text(f) {
                            1
                        } else {
                            0
                        };
                        s * w
                    })
                    .max()
                    .unwrap_or(0)
            })
            .sum()
    }
}

// ---- Marking what was found ----------------------------------------------

/// Where `query`'s words are in `text`: byte ranges, in order, joined
/// where they touch.
pub fn found(text: &str, query: &Query) -> Vec<Range<usize>> {
    let mut all: Vec<Range<usize>> = query.words().flat_map(|n| n.find_all(text)).collect();
    all.sort_by_key(|r| r.start);
    let mut out: Vec<Range<usize>> = Vec::new();
    for r in all {
        match out.last_mut() {
            Some(l) if r.start <= l.end => l.end = l.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// `text` with only its filters (what's typed to find goes): to keep the
/// filters of a search from one run to the next.
pub fn filters_only(text: &str, keys: &[&str]) -> String {
    tokens(text, keys).iter().filter(|t| t.key.is_some()).map(|t| &text[t.range.clone()]).collect::<Vec<_>>().join(" ")
}

// ---- Filters -------------------------------------------------------------

/// One of an app's filters: `key:value` in a query, a menu in the
/// [`FilterBar`], suggested as it is typed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facet {
    pub key: String,
    /// What the bar calls it ("Program").
    pub label: String,
    /// What it says with nothing chosen ("Any", "Olo Programs").
    pub any: String,
    /// The values it knows (more can be typed).
    pub values: Vec<FacetValue>,
    /// Several values at once (any of them), or one.
    pub multiple: bool,
    /// In the bar (else only typed and suggested).
    pub bar: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FacetValue {
    /// As it is typed (`program:olo-pool`).
    pub value: String,
    /// As menus show it.
    pub label: String,
    /// How many items have it, when known.
    pub count: Option<usize>,
}

impl Facet {
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> Facet {
        Facet { key: key.into(), label: label.into(), any: "Any".into(), values: Vec::new(), multiple: false, bar: true }
    }
    /// What it says with nothing chosen.
    pub fn any(mut self, any: impl Into<String>) -> Facet {
        self.any = any.into();
        self
    }
    /// Several values at once (any of them).
    pub fn multiple(mut self) -> Facet {
        self.multiple = true;
        self
    }
    /// Only typed and suggested, not in the bar.
    pub fn typed_only(mut self) -> Facet {
        self.bar = false;
        self
    }
    /// A value, its label in menus, and how many items have it.
    pub fn value(mut self, value: impl Into<String>, label: impl Into<String>, count: Option<usize>) -> Facet {
        self.values.push(FacetValue { value: value.into(), label: label.into(), count });
        self
    }

    /// What menus call value `v` (itself, if it isn't one of ours).
    pub fn label_of(&self, v: &str) -> String {
        self.values.iter().find(|x| x.value.eq_ignore_ascii_case(v)).map_or_else(|| v.to_owned(), |x| x.label.clone())
    }
}

/// The keys of `facets`, for [`Query::parse`].
pub fn keys(facets: &[Facet]) -> Vec<&str> {
    facets.iter().map(|f| f.key.as_str()).collect()
}

/// What a [`SearchField`] offers for the text it has.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    /// The text it replaces (bytes) and what goes there.
    pub replace: Range<usize>,
    pub insert: String,
    /// What the list shows, and beside it (a count, the filter's name).
    pub label: String,
    pub detail: String,
}

/// At most this many suggestions show.
const SUGGESTIONS: usize = 8;

/// What to offer for the token the caret (`caret`, bytes) is at the end
/// of: a filter's values while one is typed (`program:olo-p`), else the
/// filters whose name, and the values whose name, start or have it.
pub fn suggest(text: &str, caret: usize, facets: &[Facet]) -> Vec<Suggestion> {
    let keys = keys(facets);
    let Some(t) = tokens(text, &keys).into_iter().find(|t| t.range.start < caret && caret <= t.range.end) else { return Vec::new() };
    if caret != t.range.end {
        return Vec::new();
    }
    let count = |n: Option<usize>| n.map_or_else(String::new, |n| n.to_string());
    let end_space = if t.range.end >= text.len() { " " } else { "" };
    let mut out = Vec::new();
    if let Some((key, _)) = &t.key {
        let Some(f) = facets.iter().find(|f| f.key.eq_ignore_ascii_case(key)) else { return out };
        let (typed, vr) = t.values.last().cloned().unwrap_or_default();
        let n = Needle::new(&typed);
        let others: Vec<&str> = t.values[..t.values.len().saturating_sub(1)].iter().map(|(v, _)| v.as_str()).collect();
        let mut vals: Vec<&FacetValue> = f.values.iter().filter(|v| !others.iter().any(|o| o.eq_ignore_ascii_case(&v.value))).filter(|v| n.in_text(&v.value) || n.in_text(&v.label)).collect();
        vals.sort_by_key(|v| !(n.starts(&v.value) || n.starts(&v.label)));
        // (A quoted value's quotes go too.)
        let replace = if vr.start > 0 && text.as_bytes()[vr.start - 1] == b'"' { vr.start - 1..t.range.end } else { vr.start..t.range.end };
        for v in vals.into_iter().take(SUGGESTIONS) {
            if v.value.eq_ignore_ascii_case(&typed) && end_space.is_empty() {
                continue;
            }
            out.push(Suggestion { replace: replace.clone(), insert: format!("{}{end_space}", quote(&v.value)), label: v.label.clone(), detail: count(v.count) });
        }
        return out;
    }
    let (typed, vr) = t.values[0].clone();
    if typed.is_empty() || text.as_bytes().get(vr.start.wrapping_sub(1)) == Some(&b'"') {
        return out;
    }
    let n = Needle::new(&typed);
    let minus = if t.not { "-" } else { "" };
    for f in facets.iter().filter(|f| n.starts(&f.key) || n.starts(&f.label)) {
        out.push(Suggestion { replace: t.range.clone(), insert: format!("{minus}{}:", f.key), label: format!("{}:", f.key), detail: f.label.clone() });
    }
    if typed.chars().count() >= 2 {
        let mut vals: Vec<(&Facet, &FacetValue)> = facets.iter().flat_map(|f| f.values.iter().map(move |v| (f, v))).filter(|(_, v)| n.in_text(&v.value) || n.in_text(&v.label)).collect();
        vals.sort_by_key(|(_, v)| !(n.starts(&v.value) || n.starts(&v.label)));
        for (f, v) in vals {
            out.push(Suggestion {
                replace: t.range.clone(),
                insert: format!("{minus}{}:{}{end_space}", f.key, quote(&v.value)),
                label: v.label.clone(),
                detail: if v.count.is_some() { format!("{} · {}", f.label, count(v.count)) } else { f.label.clone() },
            });
        }
    }
    out.truncate(SUGGESTIONS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYS: &[&str] = &["program", "level", "vault"];

    fn has(item: &[(&str, &str)], text: &str) -> impl Fn(Option<&str>, &Needle) -> bool {
        let item: Vec<(String, String)> = item.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let text = text.to_owned();
        move |k, n| match k {
            None => n.in_text(&text),
            Some(k) => item.iter().any(|(ik, iv)| ik == k && n.is(iv)),
        }
    }

    #[test]
    fn parses_words_phrases_and_filters() {
        let q = Query::parse(r#"quota -429 "reading the" program:olo-pool,olo-chrome -level:debug https://x.com"#, KEYS);
        let t: Vec<(Option<&str>, Vec<&str>, bool)> = q.terms.iter().map(|t| (t.key.as_deref(), t.any.iter().map(Needle::as_str).collect(), t.not)).collect();
        assert_eq!(
            t,
            vec![
                (None, vec!["quota"], false),
                (None, vec!["429"], true),
                (None, vec!["reading the"], false),
                (Some("program"), vec!["olo-pool", "olo-chrome"], false),
                (Some("level"), vec!["debug"], true),
                (None, vec!["https://x.com"], false),
            ]
        );
        // Still being typed: nothing yet.
        assert!(Query::parse("program: -", KEYS).is_empty());
        assert_eq!(Query::parse(r#"vault:"two words""#, KEYS).values("vault")[0].as_str(), "two words");
    }

    #[test]
    fn matches_every_term() {
        let q = Query::parse("quota -429 program:olo-pool,olo-chrome", KEYS);
        assert!(q.matches(has(&[("program", "olo-pool")], "Reading the quota")));
        assert!(q.matches(has(&[("program", "olo-chrome")], "QUOTA left")));
        assert!(!q.matches(has(&[("program", "olo-pool")], "quota: HTTP 429")));
        assert!(!q.matches(has(&[("program", "olo-tracker")], "quota")));
        assert!(Query::parse("", KEYS).matches(has(&[], "")));
    }

    #[test]
    fn needles_find_and_mark() {
        let n = Needle::new("Straße");
        assert_eq!(n.find("die STRASSE, die straße"), Some(17..24));
        let a = Needle::new("ab");
        assert_eq!(a.find_all("xAbab-AB"), vec![1..3, 3..5, 6..8]);
        let q = Query::parse("ab b", KEYS);
        assert_eq!(found("xAbab cb", &q), vec![1..5, 7..8]);
        assert!(Needle::new("olo-").starts("OLO-pool"));
    }

    #[test]
    fn keeps_only_filters() {
        assert_eq!(filters_only("quota program:a,b -level:debug \"x y\"", KEYS), "program:a,b -level:debug");
    }

    #[test]
    fn scores_put_the_best_first() {
        let q = Query::parse("git", KEYS);
        let title = |t: &str| q.score(&[(t, 4), ("", 1)]);
        assert!(title("git") > title("GitHub"));
        assert!(title("GitHub") > title("My Git"));
        assert!(title("My Git") > title("Legit"));
        assert_eq!(title("Mail"), 0);
    }

    #[test]
    fn filters_are_set_in_the_text() {
        let mut t = String::from("quota program:a -program:b level:info");
        set_filter(&mut t, KEYS, "program", &["olo-pool", "two words"]);
        assert_eq!(t, r#"quota -program:b level:info program:olo-pool,"two words""#);
        set_filter(&mut t, KEYS, "program", &[]);
        assert_eq!(t, "quota -program:b level:info");
        set_filter(&mut t, KEYS, "level", &["error"]);
        assert_eq!(t, "quota -program:b level:error");
    }

    #[test]
    fn suggests_filters_and_values() {
        let facets = [
            Facet::new("program", "Program").value("olo-pool", "olo-pool", Some(12)).value("olo-chrome", "olo-chrome", Some(3)).multiple(),
            Facet::new("level", "Level").value("error", "Errors Only", None),
        ];
        let s = suggest("quota pro", 9, &facets);
        assert_eq!(s[0].insert, "program:");
        assert_eq!(s[0].replace, 6..9);
        let s = suggest("program:olo-c", 13, &facets);
        assert_eq!((s.len(), s[0].insert.as_str(), s[0].replace.clone()), (1, "olo-chrome ", 8..13));
        // A value typed as a word: its filter.
        let s = suggest("-chro", 5, &facets);
        assert_eq!(s[0].insert, "-program:olo-chrome ");
        // The second of either: the first isn't offered again.
        let s = suggest("program:olo-pool,olo", 20, &facets);
        assert_eq!(s.iter().map(|s| s.insert.as_str()).collect::<Vec<_>>(), vec!["olo-chrome "]);
        assert!(suggest("quota ", 6, &facets).is_empty());
        assert!(suggest("err", 3, &facets).iter().any(|s| s.insert == "level:error "));
    }
}
