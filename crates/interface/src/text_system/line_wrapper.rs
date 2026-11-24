use crate::{FontId, FontRun, Pixels, PlatformTextSystem, SharedString, TextRun, px};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use std::{iter, sync::Arc};

/// The line wrapper, used to wrap lines of text to a given width.
pub struct LineWrapper {
    platform_text_system: Arc<dyn PlatformTextSystem>,
    pub(crate) font_id: FontId,
    pub(crate) font_size: Pixels,
    cached_ascii_char_widths: [Option<Pixels>; 128],
    cached_other_char_widths: FxHashMap<char, Pixels>,
}

impl LineWrapper {
    /// The maximum indent that can be applied to a line.
    pub const MAX_INDENT: u32 = 256;

    pub(crate) fn new(font_id: FontId, font_size: Pixels, text_system: Arc<dyn PlatformTextSystem>) -> Self {
        Self { platform_text_system: text_system, font_id, font_size, cached_ascii_char_widths: [None; 128], cached_other_char_widths: FxHashMap::default() }
    }

    /// Wrap a line of text to the given width with this wrapper's font and font size.
    pub fn wrap_line<'a>(&'a mut self, fragments: &'a [LineFragment], wrap_width: Pixels) -> impl Iterator<Item = Boundary> + 'a {
        let mut width = px(0.);
        let mut first_non_whitespace_ix = None;
        let mut indent = None;
        let mut last_candidate_ix = 0;
        let mut last_candidate_width = px(0.);
        // Olo: just after a no-break space, a place to break only as a
        // last resort (`opportunity`), unless what it joins is wider than
        // a line (`free_glue`): then as at a space. Found at the first one
        // met: text without one (nearly all) pays nothing for them.
        let mut free: Option<SmallVec<[usize; 4]>> = None;
        let mut glue_ix = 0;
        let mut glue_width = px(0.);
        let mut last_wrap_ix = 0;
        let mut prev_c = '\0';
        // Olo: the last character that wasn't a space nor a joiner
        // (`breaks_between`).
        let mut word_c = '\0';
        let mut index = 0;
        let mut candidates = fragments.iter().flat_map(move |fragment| fragment.wrap_boundary_candidates()).peekable();
        iter::from_fn(move || {
            for candidate in candidates.by_ref() {
                let ix = index;
                index += candidate.len_utf8();
                // Olo: a space after a word hangs past the edge rather
                // than pushing the word before it down a row ("…with Ann
                // at" fits when "at" does), as typesetters, CSS and egui
                // wrap. Leading spaces (an indent) still count.
                let hangs = matches!(candidate, WrapBoundaryCandidate::Char { character: ' ' }) && first_non_whitespace_ix.is_some();
                let mut new_prev_c = prev_c;
                let item_width = match candidate {
                    WrapBoundaryCandidate::Char { character: c } => {
                        if c == '\n' {
                            continue;
                        }

                        match Self::opportunity(prev_c, word_c, c, first_non_whitespace_ix.is_some()) {
                            Some(Break::Here) => {
                                last_candidate_ix = ix;
                                last_candidate_width = width;
                            }
                            Some(Break::Joined) if free.get_or_insert_with(|| self.free_glue(fragments, wrap_width)).contains(&ix) => {
                                last_candidate_ix = ix;
                                last_candidate_width = width;
                            }
                            Some(_) => {
                                glue_ix = ix;
                                glue_width = width;
                            }
                            None => {}
                        }

                        if c != ' ' && first_non_whitespace_ix.is_none() {
                            first_non_whitespace_ix = Some(ix);
                        }

                        new_prev_c = c;
                        if c != ' ' && !Self::joins(c) {
                            word_c = c;
                        }

                        self.width_for_char(c)
                    }
                    WrapBoundaryCandidate::Element { width: element_width, .. } => {
                        if prev_c == ' ' && first_non_whitespace_ix.is_some() {
                            last_candidate_ix = ix;
                            last_candidate_width = width;
                        }

                        if first_non_whitespace_ix.is_none() {
                            first_non_whitespace_ix = Some(ix);
                        }

                        element_width
                    }
                };

                width += item_width;
                if width > wrap_width && ix > last_wrap_ix && !hangs {
                    if let (None, Some(first_non_whitespace_ix)) = (indent, first_non_whitespace_ix) {
                        indent = Some(Self::MAX_INDENT.min((first_non_whitespace_ix - last_wrap_ix) as u32));
                    }

                    let before = width;
                    if last_candidate_ix > 0 {
                        last_wrap_ix = last_candidate_ix;
                        width -= last_candidate_width;
                        last_candidate_ix = 0;
                    } else if glue_ix > last_wrap_ix {
                        last_wrap_ix = glue_ix;
                        width -= glue_width;
                    } else {
                        last_wrap_ix = ix;
                        width = item_width;
                    }

                    if let Some(indent) = indent {
                        width += self.width_for_char(' ') * indent as f32;
                    }
                    // (A no-break space past the break is still one, on
                    // the line it's on now: measured from that line's start.)
                    if glue_ix > last_wrap_ix {
                        glue_width -= before - width;
                    } else {
                        glue_ix = 0;
                    }
                    // (Olo: this character read, as below: returning
                    // without it, the next took the one before for its
                    // neighbour, a space, and broke inside its word:
                    // "w / orld" where "world" was wider than a line.)
                    prev_c = new_prev_c;

                    return Some(Boundary::new(last_wrap_ix, indent.unwrap_or(0)));
                }

                prev_c = new_prev_c;
            }

            None
        })
    }

    /// Truncate a line of text to the given width with this wrapper's font and font size.
    pub fn truncate_line(&mut self, line: SharedString, truncate_width: Pixels, truncation_suffix: &str, runs: &mut Vec<TextRun>) -> SharedString {
        let mut width = px(0.);
        let mut suffix_width = truncation_suffix.chars().map(|c| self.width_for_char(c)).fold(px(0.0), |a, x| a + x);
        let mut char_indices = line.char_indices();
        let mut truncate_ix = 0;
        for (ix, c) in char_indices {
            if width + suffix_width < truncate_width {
                truncate_ix = ix;
            }

            let char_width = self.width_for_char(c);
            width += char_width;

            if width.floor() > truncate_width {
                let result = SharedString::from(format!("{}{}", &line[..truncate_ix], truncation_suffix));
                update_runs_after_truncation(&result, truncation_suffix, runs);

                return result;
            }
        }

        line
    }

    /// Whether `c` is part of a word, which a line doesn't break inside
    /// (unless the word is wider than the line). Olo: everything but
    /// white space, as egui wraps and as typesetters do: a curly
    /// apostrophe (computer’s), quotes, brackets, a path's slashes and an
    /// emoji stay with what they're beside. Chinese and Japanese, written
    /// without spaces, break between any two of their characters. A
    /// no-break space (and the narrow and figure ones) joins what's either
    /// side of it, as it's meant to: "3.0 MB" stays on one line (counted
    /// as white space, a line broke there and began with it).
    /// Olo: whether a line may break at the space between `before` (the
    /// last character before it) and `after` (the first after it). Not
    /// beside the arrow of a path through menus: "Preferences ▸
    /// Keyboard" breaks before "Preferences" or after "Keyboard", and a
    /// line neither ends with the arrow nor begins with it (Olo Doctor's
    /// card began one with "▸ Keyboard).", at 1× and 2× and not at
    /// 1.5×, 2026-11-20).
    /// A no-break space or the word joiner: what's either side stays on
    /// one line (as Unicode's line breaking has it, classes GL and WJ).
    #[inline(always)]
    pub(crate) fn joins(c: char) -> bool {
        // (Each at U+00A0 or past it: one compare for most characters.)
        c >= '\u{00A0}' && matches!(c, '\u{00A0}' | '\u{2007}' | '\u{202F}' | '\u{2060}')
    }

    /// Olo: whether a line may break before `c`, after `prev` (`word`:
    /// the last character before it that wasn't a space nor a joiner;
    /// `started`: a character not a space came before), and how readily
    /// ([`Break`]); `None` inside a word. One rule for the wrapper and
    /// the paragraph's layout (`LineLayout::compute_wrap_boundaries`),
    /// which must agree: an alert is sized by the one and drawn by the
    /// other. (Always inlined: called for each glyph, a call to it cost
    /// as much as the rest of the loop, its state kept across the call.)
    #[inline(always)]
    pub(crate) fn opportunity(prev: char, word: char, c: char, started: bool) -> Option<Break> {
        if !started || c == ' ' {
            return None;
        }
        let spaced = prev == ' ';
        let joined = !spaced && Self::joins(prev);
        // (A letter inside a word, as nearly every character is: decided
        // by the one before it.)
        if !spaced && !joined && Self::is_word_char(c) || Self::joins(c) {
            return None;
        }
        if (joined || spaced) && !Self::breaks_between(word, c) {
            return Some(Break::LastResort);
        }
        Some(if joined { Break::Joined } else { Break::Here })
    }

    /// Olo: the places just after a no-break space where a line may break
    /// as after a space: inside words they join that are wider than
    /// `wrap_width` all together, which can't be kept on one line, so are
    /// set as words are rather than moved down whole first (leaving
    /// "choose" alone above "Pin ▸ Always Open on / Workspace").
    fn free_glue(&mut self, fragments: &[LineFragment], wrap_width: Pixels) -> SmallVec<[usize; 4]> {
        let (mut free, mut unit) = (SmallVec::new(), SmallVec::<[usize; 4]>::new());
        let (mut x, mut start_x, mut end_x) = (px(0.), px(0.), px(0.));
        let (mut prev_c, mut word_c, mut started, mut index) = ('\0', '\0', false, 0);
        let mut close = |unit: &mut SmallVec<[usize; 4]>, start_x: Pixels, end_x: Pixels| {
            if end_x - start_x > wrap_width {
                free.extend(unit.iter().copied());
            }
            unit.clear();
        };
        for candidate in fragments.iter().flat_map(|f| f.wrap_boundary_candidates()) {
            let ix = index;
            index += candidate.len_utf8();
            match candidate {
                WrapBoundaryCandidate::Char { character: c } => {
                    if c == '\n' {
                        continue;
                    }
                    match Self::opportunity(prev_c, word_c, c, started) {
                        Some(Break::Joined) => unit.push(ix),
                        Some(Break::Here) => {
                            close(&mut unit, start_x, end_x);
                            start_x = x;
                        }
                        Some(Break::LastResort) | None => {}
                    }
                    started |= c != ' ';
                    x += self.width_for_char(c);
                    if c != ' ' {
                        end_x = x;
                    }
                    prev_c = c;
                    if c != ' ' && !Self::joins(c) {
                        word_c = c;
                    }
                }
                WrapBoundaryCandidate::Element { width, .. } => {
                    // (Where `wrap_line` may break before one.)
                    if prev_c == ' ' && started {
                        close(&mut unit, start_x, end_x);
                        start_x = x;
                    }
                    started = true;
                    x += width;
                    end_x = x;
                }
            }
        }
        close(&mut unit, start_x, end_x);
        free
    }

    #[inline(always)]
    pub(crate) fn breaks_between(before: char, after: char) -> bool {
        before != '▸' && after != '▸'
    }

    #[inline(always)]
    pub(crate) fn is_word_char(c: char) -> bool {
        (!c.is_whitespace() || matches!(c, '\u{00A0}' | '\u{2007}' | '\u{202F}'))
            && !matches!(c,
                // CJK radicals, punctuation, kana, Bopomofo, ideographs
                '\u{2E80}'..='\u{9FFF}'
                // Compatibility ideographs, halfwidth and fullwidth forms
                | '\u{F900}'..='\u{FAFF}' | '\u{FF00}'..='\u{FFEF}'
                // Ideographs beyond the basic plane
                | '\u{20000}'..='\u{3FFFF}')
    }

    #[inline(always)]
    fn width_for_char(&mut self, c: char) -> Pixels {
        if (c as u32) < 128 {
            if let Some(cached_width) = self.cached_ascii_char_widths[c as usize] {
                cached_width
            } else {
                let width = self.compute_width_for_char(c);
                self.cached_ascii_char_widths[c as usize] = Some(width);
                width
            }
        } else if let Some(cached_width) = self.cached_other_char_widths.get(&c) {
            *cached_width
        } else {
            let width = self.compute_width_for_char(c);
            self.cached_other_char_widths.insert(c, width);
            width
        }
    }

    fn compute_width_for_char(&self, c: char) -> Pixels {
        let mut buffer = [0; 4];
        let buffer = c.encode_utf8(&mut buffer);
        self.platform_text_system.layout_line(buffer, self.font_size, &[FontRun { len: buffer.len(), font_id: self.font_id, size: None }]).width
    }
}

fn update_runs_after_truncation(result: &str, ellipsis: &str, runs: &mut Vec<TextRun>) {
    let mut truncate_at = result.len() - ellipsis.len();
    for (run_index, run) in runs.iter_mut().enumerate() {
        if run.len <= truncate_at {
            truncate_at -= run.len;
        } else {
            run.len = truncate_at + ellipsis.len();
            runs.truncate(run_index + 1);
            break;
        }
    }
}

/// A fragment of a line that can be wrapped.
pub enum LineFragment<'a> {
    /// A text fragment consisting of characters.
    Text {
        /// The text content of the fragment.
        text: &'a str,
    },
    /// A non-text element with a fixed width.
    Element {
        /// The width of the element in pixels.
        width: Pixels,
        /// The UTF-8 encoded length of the element.
        len_utf8: usize,
    },
}

impl<'a> LineFragment<'a> {
    /// Creates a new text fragment from the given text.
    pub fn text(text: &'a str) -> Self {
        LineFragment::Text { text }
    }

    /// Creates a new non-text element with the given width and UTF-8 encoded length.
    pub fn element(width: Pixels, len_utf8: usize) -> Self {
        LineFragment::Element { width, len_utf8 }
    }

    fn wrap_boundary_candidates(&self) -> impl Iterator<Item = WrapBoundaryCandidate> {
        let text = match self {
            LineFragment::Text { text } => text,
            LineFragment::Element { .. } => "\0",
        };
        text.chars().map(move |character| {
            if let LineFragment::Element { width, len_utf8 } = self { WrapBoundaryCandidate::Element { width: *width, len_utf8: *len_utf8 } } else { WrapBoundaryCandidate::Char { character } }
        })
    }
}

enum WrapBoundaryCandidate {
    Char { character: char },
    Element { width: Pixels, len_utf8: usize },
}

impl WrapBoundaryCandidate {
    pub fn len_utf8(&self) -> usize {
        match self {
            WrapBoundaryCandidate::Char { character } => character.len_utf8(),
            WrapBoundaryCandidate::Element { len_utf8: len, .. } => *len,
        }
    }
}

/// Olo: how readily a line breaks before a character
/// (`LineWrapper::opportunity`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Break {
    /// Where words are: after a space, before an ideograph.
    Here,
    /// Just after a no-break space: where what it joins is wider than a
    /// line (`free_glue`), as `Here`; else only where nothing else on the
    /// line is a place to break, rather than inside a word.
    Joined,
    /// Beside a menu path's arrow ("Preferences ▸ Keyboard",
    /// `breaks_between`): only where nothing else on the line is a place
    /// to break, rather than inside a word („Anheften ▸ / Immer“ in a
    /// column narrower than „Anheften ▸ Immer“).
    LastResort,
}

/// A boundary between two lines of text.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Boundary {
    /// The index of the last character in a line
    pub ix: usize,
    /// The indent of the next line.
    pub next_indent: u32,
}

impl Boundary {
    fn new(ix: usize, next_indent: u32) -> Self {
        Self { ix, next_indent }
    }
}

#[cfg(test)]
mod tests {
    use super::LineWrapper;

    #[test]
    fn a_no_break_space_joins_what_is_either_side() {
        for c in ['\u{00A0}', '\u{2007}', '\u{202F}', 'a', '’', '/'] {
            assert!(LineWrapper::is_word_char(c), "{c:?} breaks a word");
        }
        for c in [' ', '\t', '\u{2003}', '世'] {
            assert!(!LineWrapper::is_word_char(c), "{c:?} doesn't break");
        }
    }

    /// Words joined by no-break spaces (a menu's name in a sentence,
    /// `words::menu_path`) go down to the next line together; where they
    /// are wider than a line, they break at a no-break space, never inside
    /// a word. In Chinese, which breaks between any two characters, never
    /// beside one.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn no_break_spaces_hold_unless_what_they_join_is_wider_than_a_line() {
        use super::LineFragment;
        use crate::px;
        let mut wrapper = noto();
        let unit = "„Fenster\u{a0}schweben\u{a0}lassen\u{a0}▸\u{a0}Immer“";
        let text = format!("Keine. Um eine App hinzuzufügen, wählen Sie {unit} im Fenstermenü eines ihrer Fenster.");
        let (at, end) = (text.find(unit).unwrap(), text.find(unit).unwrap() + unit.len());
        let (mut whole, mut broken) = (0, 0);
        // (From a width each word fits.)
        for width in (100..420).step_by(3) {
            let w = px(width as f32);
            let inside: Vec<usize> = wrapper.wrap_line(&[LineFragment::text(&text)], w).map(|b| b.ix).filter(|&s| s > at && s < end).collect();
            if wrapper.wrap_line(&[LineFragment::text(unit)], w).next().is_none() {
                assert!(inside.is_empty(), "at {width}: broken at {inside:?} though it fits a line");
                whole += 1;
            } else {
                assert!(!inside.is_empty() && inside.iter().all(|&s| text[..s].ends_with('\u{a0}')), "at {width}: {inside:?}");
                broken += 1;
            }
        }
        assert!(whole > 0 && broken > 0, "{whole} widths whole, {broken} broken");
        // Wider than a line, it's set as words are: "choose" isn't left
        // alone over it where "choose Pin ▸ Always" fits (it is where the
        // name fits a line whole: kept so, as it's meant to be).
        let unit = "Pin\u{a0}▸\u{a0}Always\u{a0}Open\u{a0}on\u{a0}Workspace";
        let text = format!("None. To give an app one, choose {unit} in the window menu of one of its windows.");
        let mut wider = 0;
        for width in (90..280).step_by(2) {
            let w = px(width as f32);
            let s: Vec<usize> = wrapper.wrap_line(&[LineFragment::text(&text)], w).map(|b| b.ix).collect();
            let rows: Vec<&str> = std::iter::once(0).chain(s.iter().copied()).zip(s.iter().copied().chain([text.len()])).map(|(a, b)| text[a..b].trim_end()).collect();
            if wrapper.wrap_line(&[LineFragment::text(unit)], w).next().is_some() {
                wider += 1;
                if rows.contains(&"choose") {
                    assert!(wrapper.wrap_line(&[LineFragment::text("choose Pin\u{a0}▸\u{a0}Always")], w).next().is_some(), "at {width}: {rows:?}");
                }
            }
            assert!(rows.iter().all(|r| !r.starts_with('▸') && !r.ends_with('▸')), "at {width}: {rows:?}");
        }
        assert!(wider > 0);
        let text = "要添加应用，请在其窗口菜单中选择“浮动窗口\u{a0}▸\u{a0}始终”。";
        for width in (40..300).step_by(2) {
            for s in wrapper.wrap_line(&[LineFragment::text(text)], px(width as f32)).map(|b| b.ix) {
                assert!(!text[..s].ends_with(['\u{a0}', '▸']) && !text[s..].starts_with(['\u{a0}', '▸']), "at {width}: {:?} | {:?}", &text[..s], &text[s..]);
            }
        }
    }

    /// A word wider than a line, just after a break, is cut where it
    /// overflows, not after its first letter (the wrapper took the space
    /// before the break for the next letter's neighbour).
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn a_long_word_after_a_break_isnt_cut_after_its_first_letter() {
        use super::LineFragment;
        use crate::px;
        let mut wrapper = noto();
        let text = "hello worldwidewebthatgoesonandonandon";
        for width in (40..240).step_by(2) {
            let mut starts = vec![0];
            starts.extend(wrapper.wrap_line(&[LineFragment::text(text)], px(width as f32)).map(|b| b.ix));
            for pair in starts.windows(2) {
                assert!(text[pair[0]..pair[1]].trim_end().len() > 1, "at {width}: {:?}", &text[pair[0]..pair[1]]);
            }
        }
    }

    /// Olo's Noto Sans at 13 points.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn noto() -> LineWrapper {
        use crate::{PlatformTextSystem, px};
        use std::{borrow::Cow, sync::Arc};
        let system = crate::OloTextSystem::new();
        let data = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/fonts/NotoSans[wght].ttf")).expect("Olo's Noto Sans");
        system.add_fonts(vec![Cow::Owned(data)]).unwrap();
        let id = system.font_id(&crate::font("Noto Sans")).unwrap();
        LineWrapper::new(id, px(13.0), Arc::new(system))
    }

    #[test]
    fn a_menu_paths_arrow_stays_with_both_its_neighbours() {
        // "Preferences ▸ Keyboard": neither space is a place to break.
        assert!(!LineWrapper::breaks_between('s', '▸'));
        assert!(!LineWrapper::breaks_between('▸', 'K'));
        assert!(LineWrapper::breaks_between('s', 'K'));
        assert!(LineWrapper::breaks_between(')', 'a'));
    }
}
