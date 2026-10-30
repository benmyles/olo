//! Searching and filtering, the same in every app: a query language with
//! matching, ranking and highlighting ([`Query`]), a search field that
//! colours filters and suggests them as you type ([`SearchField`]), and a
//! bar of filter menus ([`FilterBar`]) that edit the same text, so what is
//! typed and what is chosen always agree (and an agent can just type a
//! query).
//!
//! The language: words (each must match, in any order, ignoring case),
//! `"quoted phrases"`, `-word` to leave out, and an app's filters
//! ([`Facet`]s) as `key:value`, `key:a,b` for either, `-key:value` to
//! leave out, `key:"two words"`. A `key:` the app doesn't know is only a
//! word, so `https://example.com` looks for itself.

use std::ops::Range;

use egui::{
    Align, Color32, CursorIcon, FontId, Id, Key, Modifiers, Rect, Response, Role, Sense, Shape, Stroke, Ui, Widget,
    WidgetInfo, pos2, text::LayoutJob, vec2,
};
use olo_theme::Palette;

use crate::{
    menu::{self, Menu, MenuAnchor, MenuRequest},
    metrics::{self as m, CONTROL_HEIGHT},
    style::{self, c, plain_font},
};

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
            return memchr::memchr2_iter(first, first.to_ascii_uppercase(), &h[from..=last])
                .map(|i| i + from)
                .find(|&i| h[i + 1..i + n.len()].eq_ignore_ascii_case(&n[1..]))
                .map(|i| i..i + n.len());
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
        let key = text[i..word_end].find(':').map(|k| (&text[i..i + k], i..i + k)).filter(|(k, _)| {
            !k.is_empty() && k.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_') && keys.iter().any(|known| known.eq_ignore_ascii_case(k))
        });
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
    if v.is_empty() || v.contains(|c: char| c.is_whitespace() || c == ',' || c == '"') {
        format!("\"{}\"", v.replace('"', ""))
    } else {
        v.to_owned()
    }
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

/// The tint behind what a search found.
pub fn mark_color(p: &Palette) -> Color32 {
    c(olo_theme::mix(p.hues.yellow, p.document_background, 0.55))
}

/// `text` in `font` and `color`, what `query`'s words find in it marked
/// with `mark` behind: for a list's cell or a label.
pub fn highlight(text: &str, query: &Query, font: FontId, color: Color32, mark: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let plain = egui::TextFormat { font_id: font.clone(), color, ..Default::default() };
    let marked = egui::TextFormat { background: mark, ..plain.clone() };
    let mut at = 0;
    for r in found(text, query) {
        job.append(&text[at..r.start], 0.0, plain.clone());
        job.append(&text[r.clone()], 0.0, marked.clone());
        at = r.end;
    }
    job.append(&text[at..], 0.0, plain);
    job
}

/// Mark `found` (byte ranges of the job's text, as [`found`] gives) with
/// `color` behind, in a job already laid out in sections of its own
/// (coloured text): the sections are split where a mark starts or ends.
pub fn mark(job: &mut LayoutJob, found: &[Range<usize>], color: Color32) {
    if found.is_empty() {
        return;
    }
    let mut out = Vec::with_capacity(job.sections.len() + 2 * found.len());
    for sec in job.sections.drain(..) {
        let (start, end) = (sec.byte_range.start.0, sec.byte_range.end.0);
        let mut cuts = vec![start, end];
        for r in found {
            for b in [r.start, r.end] {
                if start < b && b < end {
                    cuts.push(b);
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for w in cuts.windows(2) {
            let mut part = sec.clone();
            part.byte_range = egui::text::ByteIndex(w[0])..egui::text::ByteIndex(w[1]);
            if w[0] != start {
                part.leading_space = 0.0;
            }
            if found.iter().any(|r| r.start <= w[0] && w[1] <= r.end) {
                part.format.background = color;
            }
            out.push(part);
        }
    }
    job.sections = out;
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

    fn label_of(&self, v: &str) -> String {
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
        let mut vals: Vec<&FacetValue> = f
            .values
            .iter()
            .filter(|v| !others.iter().any(|o| o.eq_ignore_ascii_case(&v.value)))
            .filter(|v| n.in_text(&v.value) || n.in_text(&v.label))
            .collect();
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
        let mut vals: Vec<(&Facet, &FacetValue)> =
            facets.iter().flat_map(|f| f.values.iter().map(move |v| (f, v))).filter(|(_, v)| n.in_text(&v.value) || n.in_text(&v.label)).collect();
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

// ---- The search field -----------------------------------------------------

fn field_id(id: impl std::hash::Hash + std::fmt::Debug) -> Id {
    Id::unique(("olo-search", id))
}

/// A search field: a magnifier, the query (filters coloured, ones left
/// out in red), a clear button, and while typing a list under it of the
/// filters and values that fit ([`suggest`]): ↑ and ↓ choose, Tab or
/// Return takes one (Tab the first), Escape closes the list, then clears,
/// then lets go. Its response is changed when the text is.
pub struct SearchField<'a> {
    id: Id,
    text: &'a mut String,
    hint: &'a str,
    facets: &'a [Facet],
    width: Option<f32>,
}

impl<'a> SearchField<'a> {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, text: &'a mut String) -> Self {
        SearchField { id: field_id(id), text, hint: "Search", facets: &[], width: None }
    }
    pub fn hint(mut self, hint: &'a str) -> Self {
        self.hint = hint;
        self
    }
    /// The filters it colours and suggests.
    pub fn facets(mut self, facets: &'a [Facet]) -> Self {
        self.facets = facets;
        self
    }
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// Give field `id` the keyboard (Find, ⌘F), its text selected.
    pub fn focus(ctx: &egui::Context, id: impl std::hash::Hash + std::fmt::Debug) {
        let id = field_id(id);
        ctx.memory_mut(|m| m.request_focus(id));
        ctx.data_mut(|d| d.insert_temp(id.with("select-all"), true));
    }

    /// Whether field `id` has the keyboard.
    pub fn has_focus(ctx: &egui::Context, id: impl std::hash::Hash + std::fmt::Debug) -> bool {
        ctx.memory(|m| m.has_focus(field_id(id)))
    }
}

/// The suggestions' rows, under the field at `well`.
fn suggestion_rows(well: Rect, n: usize) -> (Rect, impl Fn(usize) -> Rect) {
    let w = well.width().max(280.0);
    let frame = Rect::from_min_size(pos2(well.left(), well.bottom() + 2.0), vec2(w, n as f32 * m::LIST_ROW + 2.0 * m::TIGHT));
    let row = move |k: usize| Rect::from_min_size(pos2(frame.left() + 1.0, frame.top() + m::TIGHT + k as f32 * m::LIST_ROW), vec2(w - 2.0, m::LIST_ROW));
    (frame, row)
}

/// `text` (a query) laid out in `font`: filters' keys in `key`, what is
/// left out in `not`.
fn coloured(text: &str, keys: &[&str], font: &FontId, ink: Color32, key: Color32, not: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let fmt = |color: Color32| egui::TextFormat { font_id: font.clone(), color, ..Default::default() };
    let mut at = 0;
    for t in tokens(text, keys) {
        job.append(&text[at..t.range.start], 0.0, fmt(ink));
        let mut j = t.range.start;
        if t.not {
            job.append(&text[j..j + 1], 0.0, fmt(not));
            j += 1;
        }
        if let Some((_, kr)) = &t.key {
            let end = (kr.end + 1).min(t.range.end);
            job.append(&text[j..end], 0.0, fmt(key));
            j = end;
        }
        job.append(&text[j..t.range.end], 0.0, fmt(if t.not { not } else { ink }));
        at = t.range.end;
    }
    job.append(&text[at..], 0.0, fmt(ink));
    job
}

impl Widget for SearchField<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let p = style::palette(ui.ctx());
        let ctx = ui.ctx().clone();
        let id = self.id;
        let width = self.width.unwrap_or_else(|| ui.available_width());
        let (well, _) = ui.allocate_exact_size(vec2(width, CONTROL_HEIGHT), Sense::hover());
        let keys = keys(self.facets);
        let focused = ui.memory(|mm| mm.has_focus(id));
        let mut changed = false;
        if focused && ui.data(|d| d.get_temp::<bool>(id.with("select-all"))).unwrap_or(false) {
            let mut st = egui::text_edit::TextEditState::load(&ctx, id).unwrap_or_default();
            let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(self.text.chars().count()));
            st.cursor.set_char_range(Some(all));
            st.store(&ctx, id);
            ui.data_mut(|d| d.remove::<bool>(id.with("select-all")));
        }

        // What to offer for the word at the caret (unless Escape closed
        // the list for this text).
        let caret = egui::text_edit::TextEditState::load(&ctx, id)
            .and_then(|s| s.cursor.char_range())
            .filter(|r| r.primary.index == r.secondary.index)
            .map(|r| self.text.char_indices().nth(r.primary.index.0).map_or(self.text.len(), |(b, _)| b));
        let closed_for = ui.data(|d| d.get_temp::<String>(id.with("closed")));
        let list = match caret {
            Some(at) if focused && closed_for.as_deref() != Some(self.text.as_str()) => suggest(self.text, at, self.facets),
            _ => Vec::new(),
        };
        let chosen_id = id.with("chosen");
        let mut chosen = ui.data(|d| d.get_temp::<Option<usize>>(chosen_id)).flatten().filter(|&k| k < list.len());
        let (frame, row) = suggestion_rows(well, list.len());
        let mut take: Option<Suggestion> = None;
        let mut keep_focus = false;
        if focused {
            // Keys and presses for the list, before the editor sees them.
            let pressed = ui.input(|i| if i.pointer.any_pressed() { i.pointer.press_origin() } else { None });
            ui.input_mut(|i| {
                if !list.is_empty() {
                    if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                        chosen = Some(chosen.map_or(0, |k| (k + 1).min(list.len() - 1)));
                    }
                    if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                        chosen = chosen.and_then(|k| k.checked_sub(1));
                    }
                    let tab = i.consume_key(Modifiers::NONE, Key::Tab);
                    let enter = chosen.is_some() && i.consume_key(Modifiers::NONE, Key::Enter);
                    if tab || enter {
                        take = list.get(chosen.unwrap_or(0)).cloned();
                    }
                    if i.consume_key(Modifiers::NONE, Key::Escape) {
                        ctx.data_mut(|d| d.insert_temp(id.with("closed"), self.text.clone()));
                    }
                } else if i.consume_key(Modifiers::NONE, Key::Escape) {
                    if self.text.is_empty() {
                        ctx.memory_mut(|mm| mm.surrender_focus(id));
                    } else {
                        self.text.clear();
                        changed = true;
                    }
                }
            });
            // A press on a suggestion takes it (on the press: the editor
            // lets the keyboard go when pressed outside it).
            if let Some(at) = pressed
                && let Some(k) = (0..list.len()).find(|&k| row(k).contains(at))
            {
                take = Some(list[k].clone());
                keep_focus = true;
            }
        }
        if let Some(s) = take.take() {
            self.text.replace_range(s.replace.clone(), &s.insert);
            let at = self.text[..(s.replace.start + s.insert.len()).min(self.text.len())].chars().count();
            let mut st = egui::text_edit::TextEditState::load(&ctx, id).unwrap_or_default();
            st.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(at))));
            st.store(&ctx, id);
            chosen = None;
            changed = true;
        }

        // The well, the magnifier, the clear button.
        let painter = ui.painter().clone();
        painter.rect_filled(style::inset(&ctx, well, 2.0), 0.0, c(p.document_background));
        style::sunken(&painter, well, &p, focused);
        let dim = style::disabled_text(&p);
        let cy = style::snap_coord(&ctx, well.center().y.round());
        let glass = pos2(style::snap_coord(&ctx, well.left() + m::TEXT_INSET + 5.0), cy - 1.0);
        painter.circle_stroke(glass, 4.0, Stroke::new(1.5, dim));
        painter.line_segment([glass + vec2(3.0, 3.0), glass + vec2(6.5, 6.5)], Stroke::new(1.75, dim));
        let clear_r = Rect::from_center_size(pos2(well.right() - m::TEXT_INSET - 6.0, cy), vec2(14.0, 14.0));
        let has_text = !self.text.is_empty();
        let text_right = if has_text { clear_r.left() - m::TIGHT } else { well.right() - m::TEXT_INSET };
        let font = plain_font();
        let row_h = ui.fonts_mut(|f| f.row_height(&font));
        let probe = painter.layout_no_wrap("X".into(), font.clone(), Color32::BLACK);
        let top = style::text_origin(&ctx, &probe, 0.0, well.center().y, font.size).y;
        let line = Rect::from_min_max(pos2(glass.x + 10.0 + m::TIGHT, top), pos2(text_right, top + row_h));
        if !has_text {
            let mut job = LayoutJob::simple_singleline(self.hint.to_owned(), font.clone(), dim);
            job.wrap = egui::text::TextWrapping::truncate_at_width(line.width());
            let g = painter.layout_job(job);
            let pos = style::text_origin(&ctx, &g, line.left(), well.center().y, font.size);
            painter.with_clip_rect(well.shrink(2.0).intersect(ui.clip_rect())).galley(pos, g, dim);
        }
        let (ink, key_ink, not_ink) = (c(p.document_text), c(p.hues.blue), c(p.failure));
        let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
            ui.fonts_mut(|f| f.layout_job(coloured(buf.as_str(), &keys, &font, ink, key_ink, not_ink)))
        };
        let well_resp = ui.interact(well, id.with("well"), Sense::CLICK);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(line));
        child.set_clip_rect(well.shrink(2.0).intersect(ui.clip_rect()));
        let edit = egui::TextEdit::singleline(&mut *self.text)
            .id(id)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .desired_width(line.width())
            // (Tab takes a suggestion, not the focus to the next field.)
            .lock_focus(!list.is_empty())
            .layouter(&mut layouter);
        let out = edit.show(&mut child);
        let mut r = out.response.response;
        if r.changed() {
            chosen = None;
            changed = true;
        }
        if keep_focus || well_resp.clicked() {
            ui.memory_mut(|mm| mm.request_focus(id));
        }
        if well_resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Text);
        }
        let hint = self.hint.to_owned();
        ui.ctx().accesskit_node_builder(r.id, |n| {
            n.set_label(hint.clone());
            n.set_placeholder(hint);
        });
        if has_text {
            let resp = ui.interact(clear_r, id.with("clear"), Sense::click());
            resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Clear Search"));
            let fill = if resp.hovered() { style::shade(&p, p.document_background, 1.45) } else { style::shade(&p, p.document_background, 1.3) };
            let k = style::snap_centred(&ctx, clear_r.center(), vec2(12.0, 12.0));
            painter.circle_filled(k.center(), 6.0, fill);
            let x = k.shrink(3.5);
            let st = Stroke::new(1.25, c(p.document_background));
            painter.line_segment([x.left_top(), x.right_bottom()], st);
            painter.line_segment([x.right_top(), x.left_bottom()], st);
            if resp.clicked() {
                self.text.clear();
                changed = true;
                ui.memory_mut(|mm| mm.request_focus(id));
            }
        }

        // The suggestions, over what is below.
        if focused && !list.is_empty() {
            let hover = ui.input(|i| i.pointer.hover_pos());
            egui::Area::new(id.with("suggestions")).order(egui::Order::Foreground).fixed_pos(frame.min).interactable(false).show(&ctx, |ui| {
                let painter = ui.painter();
                style::soft_shadow(painter, frame, &p);
                painter.rect_filled(frame, 3.0, c(p.menu_background));
                painter.rect_stroke(frame, 3.0, Stroke::new(style::hairline(&ctx), style::shade(&p, p.menu_background, 1.4)), egui::StrokeKind::Inside);
                for (k, s) in list.iter().enumerate() {
                    let rr = row(k);
                    let lit = chosen == Some(k) || hover.is_some_and(|h| rr.contains(h));
                    if lit {
                        painter.rect_filled(rr, 0.0, c(p.menu_selected_background));
                    }
                    let ink = if lit { c(p.menu_selected_item_text) } else { c(p.menu_item_text) };
                    let detail = style::label_fit(painter, rr.right() - m::TEXT_INSET, rr.center().y, Align::Max, &s.detail, plain_font(), if lit { ink } else { dim }, rr.width() / 2.0);
                    let room = (detail.left() - m::SPACING) - (rr.left() + m::TEXT_INSET);
                    style::label_fit(painter, rr.left() + m::TEXT_INSET, rr.center().y, Align::Min, &s.label, plain_font(), ink, room);
                    let label = format!("{} {}", s.label, s.detail);
                    let resp = ui.interact(rr, id.with(("suggestion", k)), Sense::hover());
                    resp.widget_info(|| WidgetInfo { selected: Some(lit), ..WidgetInfo::labeled(Role::MenuItem, true, label.trim()) });
                }
            });
        }
        ui.data_mut(|d| d.insert_temp(chosen_id, chosen));
        if changed {
            r.mark_changed();
        }
        r
    }
}

// ---- The filter bar ------------------------------------------------------

/// A row of filter menus, one for each of `facets` in the bar ("Program:
/// Any ▾"), each choosing its filter's values in `text` (a [`Query`]'s);
/// those with values chosen are marked. Clear, at the end, empties the
/// query. Its response is changed when the text is.
pub struct FilterBar<'a> {
    id: Id,
    text: &'a mut String,
    facets: &'a [Facet],
    clear: bool,
}

impl<'a> FilterBar<'a> {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, text: &'a mut String, facets: &'a [Facet]) -> Self {
        FilterBar { id: Id::unique(("olo-filters", id)), text, facets, clear: true }
    }
    /// No Clear button (the app has its own).
    pub fn no_clear(mut self) -> Self {
        self.clear = false;
        self
    }

    /// How wide the bar is for `text`.
    pub fn width(ui: &Ui, text: &str, facets: &[Facet]) -> f32 {
        let q = Query::parse(text, &keys(facets));
        let mut w = 0.0;
        for f in facets.iter().filter(|f| f.bar) {
            w += pill_width(ui, &pill_text(f, &q), !q.values(&f.key).is_empty()) + m::SPACING;
        }
        w - m::SPACING
    }
}

fn chosen_values(f: &Facet, q: &Query) -> Vec<String> {
    q.values(&f.key).iter().map(|n| f.values.iter().find(|v| n.is(&v.value)).map_or_else(|| n.as_str().to_owned(), |v| v.value.clone())).collect()
}

fn pill_text(f: &Facet, q: &Query) -> String {
    let chosen = chosen_values(f, q);
    let summary = match chosen.as_slice() {
        [] => f.any.clone(),
        [one] => f.label_of(one),
        [first, rest @ ..] => format!("{} +{}", f.label_of(first), rest.len()),
    };
    if f.label.is_empty() { summary } else { format!("{}: {summary}", f.label) }
}

/// How wide a pill is for `text` (in bold when it filters).
fn pill_width(ui: &Ui, text: &str, bold: bool) -> f32 {
    let font = if bold { style::bold_font() } else { plain_font() };
    let g = ui.painter().layout_no_wrap(text.to_owned(), font, Color32::BLACK);
    (g.size().x + m::TEXT_INSET + 15.0 + m::TIGHT).ceil()
}

/// `n` with thousands separated, for counts in menus.
fn count_text(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

impl Widget for FilterBar<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let p = style::palette(ui.ctx());
        let ctx = ui.ctx().clone();
        let keys = keys(self.facets);
        let (all, mut response) = ui.allocate_exact_size(vec2(ui.available_width(), CONTROL_HEIGHT), Sense::hover());
        let mut x = all.left();
        let mut changed = false;
        for f in self.facets.iter().filter(|f| f.bar) {
            let pid = self.id.with(&f.key);
            // A choice from its menu.
            if let Some(cmd) = menu::take_chosen(&ctx, pid) {
                let q = Query::parse(self.text, &keys);
                let mut chosen = chosen_values(f, &q);
                if cmd == "any" {
                    chosen.clear();
                } else if let Some(v) = cmd.strip_prefix("v:") {
                    match chosen.iter().position(|c| c.eq_ignore_ascii_case(v)) {
                        Some(i) if f.multiple => {
                            chosen.remove(i);
                        }
                        Some(_) => {}
                        None if f.multiple => chosen.push(v.to_owned()),
                        None => chosen = vec![v.to_owned()],
                    }
                }
                let refs: Vec<&str> = chosen.iter().map(String::as_str).collect();
                set_filter(self.text, &keys, &f.key, &refs);
                changed = true;
            }
            let q = Query::parse(self.text, &keys);
            let chosen = chosen_values(f, &q);
            let label = pill_text(f, &q);
            let w = pill_width(ui, &label, !chosen.is_empty()).min((all.right() - x).max(0.0));
            if w < 40.0 {
                break;
            }
            let pill = Rect::from_min_size(pos2(x, all.top()), vec2(w, CONTROL_HEIGHT));
            x += w + m::SPACING;
            let resp = ui.interact(pill, pid, Sense::click());
            if resp.clicked() {
                let mut menu = Menu::new("").item(f.any.clone(), "any", None).checked(chosen.is_empty()).separator();
                for v in &f.values {
                    menu = menu.item(v.label.clone(), format!("v:{}", v.value), None).checked(chosen.iter().any(|c| c.eq_ignore_ascii_case(&v.value)));
                    // How many have it, where a shortcut would be.
                    if let Some(n) = v.count {
                        menu = menu.keys(count_text(n));
                    }
                }
                menu::request_popup(&ctx, MenuRequest { menu, source: pid, anchor: MenuAnchor::Below(pill), selected: None });
            }
            // Raised, as a menu field; tinted when it filters.
            let painter = ui.painter();
            let base = if chosen.is_empty() { p.menu_background } else { olo_theme::mix(p.keyboard_navigation, p.menu_background, 0.82) };
            let inner = style::inset(&ctx, pill, 1.0);
            style::vgradient(painter, inner, c(olo_theme::tint(base, 0.82)), style::shade(&p, base, 1.05));
            style::frame_rect(painter, pill, style::shade(&p, p.panel_background, 1.55));
            style::bevel(painter, inner, c(p.shine), style::shade(&p, base, 1.2));
            let ink = c(p.control_text);
            let font = if chosen.is_empty() { plain_font() } else { style::bold_font() };
            // (Its middle on the device grid, as a row beside it can put its own.)
            let cy = style::snap_coord(&ctx, pill.center().y);
            style::label_fit(painter, pill.left() + m::TEXT_INSET, cy, Align::Min, &label, font, ink, pill.width() - m::TEXT_INSET - 15.0 - m::TIGHT);
            let ax = pill.right() - 11.0;
            let ay = pill.center().y.round();
            painter.add(Shape::convex_polygon(vec![pos2(ax - 4.0, ay - 2.0), pos2(ax + 4.0, ay - 2.0), pos2(ax, ay + 2.5)], ink, Stroke::NONE));
            let (name, value) = (f.label.clone(), label.clone());
            resp.widget_info(|| WidgetInfo { label: Some(name.clone()), current_text_value: Some(value.clone()), ..WidgetInfo::new(Role::ComboBox) });
        }
        if self.clear && !self.text.trim().is_empty() {
            let g = ui.painter().layout_no_wrap("Clear".into(), plain_font(), Color32::BLACK);
            let r = Rect::from_min_size(pos2(x, all.top()), vec2(g.size().x.ceil() + 2.0 * m::TEXT_INSET, CONTROL_HEIGHT));
            if r.right() <= all.right() {
                let resp = ui.interact(r, self.id.with("clear"), Sense::click());
                resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Clear"));
                let ink = if resp.hovered() { c(p.keyboard_navigation) } else { c(p.hues.blue) };
                style::label(ui.painter(), r.left() + m::TEXT_INSET, r.center().y, Align::Min, "Clear", plain_font(), ink);
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                }
                if resp.clicked() {
                    self.text.clear();
                    changed = true;
                }
            }
        }
        if changed {
            response.mark_changed();
        }
        response
    }
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
    fn marks_split_coloured_text() {
        let mut job = LayoutJob::default();
        let fmt = |c: Color32| egui::TextFormat { color: c, ..Default::default() };
        job.append("pool: ", 0.0, fmt(Color32::RED));
        job.append("quota 429", 0.0, fmt(Color32::BLUE));
        let q = Query::parse("ol quot", KEYS);
        let at = found(&job.text, &q);
        mark(&mut job, &at, Color32::YELLOW);
        let parts: Vec<(&str, bool)> =
            job.sections.iter().map(|s| (&job.text[s.byte_range.start.0..s.byte_range.end.0], s.format.background == Color32::YELLOW)).collect();
        assert_eq!(parts, vec![("po", false), ("ol", true), (": ", false), ("quot", true), ("a 429", false)]);
        assert_eq!(parts.iter().map(|(t, _)| *t).collect::<String>(), "pool: quota 429");
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
