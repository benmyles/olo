//! Unicode 18 factual properties. The algorithm remains unicode-bidi's safe
//! Rust UAX #9 implementation; data are supplied from the pinned current UCD.
#[path = "unicode_data.rs"]
mod data;
pub(crate) struct Unicode18;
impl unicode_bidi::BidiDataSource for Unicode18 {
    fn bidi_class(&self, c: char) -> unicode_bidi::BidiClass {
        let cp = c as u32;
        if cp < 128 {
            return data::CLASSES[data::ASCII[cp as usize] as usize];
        }
        let index = data::BIDI.partition_point(|&(end, _)| end < cp);
        data::CLASSES[data::BIDI[index].1 as usize]
    }
    fn bidi_matched_opening_bracket(&self, c: char) -> Option<unicode_bidi::data_source::BidiMatchedOpeningBracket> {
        let index = data::BRACKETS.binary_search_by_key(&(c as u32), |v| v.0).ok()?;
        let (_, opening, is_open) = data::BRACKETS[index];
        Some(unicode_bidi::data_source::BidiMatchedOpeningBracket { opening: char::from_u32(opening)?, is_open })
    }
}
pub(crate) fn ignorable(c: char) -> bool {
    let cp = c as u32;
    let index = data::IGNORABLE.partition_point(|&(_, end)| end < cp);
    data::IGNORABLE.get(index).is_some_and(|&(start, _)| start <= cp)
}
pub(crate) fn variation_selector(c: char) -> bool {
    matches!(c as u32,0xfe00..=0xfe0f|0xe0100..=0xe01ef)
}

fn property(c: u32, data: &[(u32, u32, u8)]) -> u8 {
    let i = data.partition_point(|&(_, end, _)| end < c);
    data.get(i).filter(|&&(start, _, _)| start <= c).map_or(0, |v| v.2)
}
#[derive(Clone, Copy)]
struct GraphemeProps {
    gcb: u8,
    incb: u8,
    pictographic: bool,
}
impl GraphemeProps {
    fn new(c: char) -> Self {
        let code = c as u32;
        if !cfg!(feature = "baseline-text-ascii") && code < 128 {
            return Self {
                gcb: match code {
                    13 => 2,
                    10 => 3,
                    0..=31 | 127 => 1,
                    _ => 0,
                },
                incb: 0,
                pictographic: false,
            };
        }
        let gcb = if (0xac00..=0xd7a3).contains(&code) { if (code - 0xac00).is_multiple_of(28) { 11 } else { 12 } } else { property(code, data::GCB) };
        Self { gcb, incb: property(code, data::INCB), pictographic: property(code, data::PICTOGRAPHIC) != 0 }
    }
}
/// Original forward Unicode 18 extended-grapheme segmentation (UAX #29).
/// Constant state, no substring copies, UTF-8 byte ranges. In particular GB9c
/// follows the Unicode 18 Linker Extend* × Consonant rule, not the older
/// requirement for a preceding consonant. Caller runs large text on a worker.
pub struct Graphemes<'a> {
    chars: std::str::CharIndices<'a>,
    start: usize,
    len: usize,
    previous: Option<GraphemeProps>,
    ri_odd: bool,
    linker: bool,
    pictographic_extend: bool,
    pictographic_zwj: bool,
}
pub fn graphemes(text: &str) -> Graphemes<'_> {
    Graphemes { chars: text.char_indices(), start: 0, len: text.len(), previous: None, ri_odd: false, linker: false, pictographic_extend: false, pictographic_zwj: false }
}
impl Iterator for Graphemes<'_> {
    type Item = std::ops::Range<usize>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.start >= self.len {
            return None;
        }
        for (offset, c) in self.chars.by_ref() {
            let b = GraphemeProps::new(c);
            let boundary = self.previous.is_some_and(|a| {
                if a.gcb == 2 && b.gcb == 3 {
                    return false;
                } // GB3
                if matches!(a.gcb, 1..=3) || matches!(b.gcb, 1..=3) {
                    return true;
                } // GB4/5
                if a.gcb == 8 && matches!(b.gcb, 8 | 9 | 11 | 12) {
                    return false;
                } // GB6
                if matches!(a.gcb, 9 | 11) && matches!(b.gcb, 9 | 10) {
                    return false;
                } // GB7
                if matches!(a.gcb, 10 | 12) && b.gcb == 10 {
                    return false;
                } // GB8
                if matches!(b.gcb, 4..=6) || a.gcb == 7 {
                    return false;
                } // GB9/9a/9b
                if self.linker && b.incb == 1 {
                    return false;
                } // GB9c, Unicode 18
                if self.pictographic_zwj && b.pictographic {
                    return false;
                } // GB11
                if a.gcb == 13 && b.gcb == 13 && self.ri_odd {
                    return false;
                } // GB12/13
                true
            });
            self.ri_odd = if b.gcb == 13 { if self.previous.is_some_and(|a| a.gcb == 13) { !self.ri_odd } else { true } } else { false };
            self.linker = match b.incb {
                3 => true,
                2 => self.linker,
                _ => false,
            };
            self.pictographic_zwj = b.gcb == 5 && self.pictographic_extend;
            self.pictographic_extend = if b.pictographic { true } else { b.gcb == 4 && self.pictographic_extend };
            self.previous = Some(b);
            if boundary {
                let start = self.start;
                self.start = offset;
                return Some(start..offset);
            }
        }
        let start = self.start;
        self.start = self.len;
        Some(start..self.len)
    }
}
