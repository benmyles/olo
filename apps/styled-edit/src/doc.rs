//! A styled document: the text and runs of styles over it (byte lengths
//! that add up to the text's), kept in step with edits made by the text
//! field by diffing (see [`Doc::sync`]), plus undo.
//!
//! On disk it is plain UTF-8 text; the styles, alignment and wrapping go in
//! extended attributes beside it, as BeOS kept them in BFS attributes, so
//! other programs see an ordinary text file.

use std::{
    ops::Range,
    path::Path,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tabula_kit::egui::{self, Color32, FontFamily, FontId, Stroke, text::LayoutJob};

/// The sizes the Font menu offers (any size can be stored).
pub const SIZES: [f32; 10] = [9.0, 10.0, 11.0, 12.0, 14.0, 18.0, 24.0, 36.0, 48.0, 72.0];
pub const DEFAULT_SIZE: f32 = 12.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    /// A font family; empty is the system font.
    pub family: String,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    /// None is the document's text colour (black, or white in dark mode).
    pub color: Option<[u8; 3]>,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            family: String::new(),
            size: DEFAULT_SIZE,
            bold: false,
            italic: false,
            underline: false,
            strikeout: false,
            color: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// Bytes of text this style covers.
    pub len: usize,
    pub style: Style,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

/// A snapshot for undo.
#[derive(Clone)]
struct State {
    text: String,
    runs: Vec<Run>,
    /// Selection to restore, in characters.
    selection: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum EditKind {
    Typing,
    Deleting,
    Other,
}

pub struct Doc {
    /// The text as of the last sync (the text field holds the live copy).
    text: String,
    runs: Vec<Run>,
    pub align: Align,
    pub wrap: bool,
    /// Style for text typed next at `typing.0` (bytes), set by a style
    /// command with nothing selected.
    pub typing: Option<(usize, Style)>,
    /// Text and styles last copied here, to paste with styles.
    clip: Option<(String, Vec<Run>)>,
    undo: Vec<State>,
    redo: Vec<State>,
    last_edit: Option<(EditKind, Instant, usize)>,
    /// Bumped on every change (for layout caching and "edited").
    pub generation: u64,
    /// The generation that was last saved or loaded.
    pub saved_generation: u64,
}

/// Merge neighbouring runs of the same style and drop empty ones.
fn normalize(runs: &mut Vec<Run>) {
    let mut out: Vec<Run> = Vec::with_capacity(runs.len());
    for r in runs.drain(..) {
        if r.len == 0 {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.style == r.style => last.len += r.len,
            _ => out.push(r),
        }
    }
    *runs = out;
}

/// Split `runs` so a run boundary falls at byte `at`; returns the index of
/// the run starting there.
fn split_at(runs: &mut Vec<Run>, at: usize) -> usize {
    let mut pos = 0;
    for i in 0..runs.len() {
        if pos == at {
            return i;
        }
        let end = pos + runs[i].len;
        if at < end {
            let tail = Run { len: end - at, style: runs[i].style.clone() };
            runs[i].len = at - pos;
            runs.insert(i + 1, tail);
            return i + 1;
        }
        pos = end;
    }
    runs.len()
}

/// Back `i` off to a char boundary of `s`.
fn floor_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Byte offset of char index `c` in `s`.
pub fn byte_of(s: &str, c: usize) -> usize {
    s.char_indices().nth(c).map_or(s.len(), |(b, _)| b)
}

/// Char index of byte offset `b` in `s`.
pub fn char_of(s: &str, b: usize) -> usize {
    s[..b.min(s.len())].chars().count()
}

impl Doc {
    pub fn new(text: String, runs: Option<Vec<Run>>) -> Doc {
        let mut runs = runs.unwrap_or_default();
        // Styles that don't fit the text (edited elsewhere) are dropped.
        let fits = runs.iter().map(|r| r.len).sum::<usize>() == text.len() && {
            let mut pos = 0;
            runs.iter().all(|r| {
                pos += r.len;
                text.is_char_boundary(pos)
            })
        };
        if !fits {
            runs = vec![Run { len: text.len(), style: Style::default() }];
        }
        normalize(&mut runs);
        Doc {
            text,
            runs,
            align: Align::Left,
            wrap: true,
            typing: None,
            clip: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            generation: 1,
            saved_generation: 1,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn runs(&self) -> &[Run] {
        &self.runs
    }

    pub fn edited(&self) -> bool {
        self.generation != self.saved_generation
    }

    /// The style of the character before byte `at` (what typing there
    /// continues), or after it at the start.
    pub fn style_at(&self, at: usize) -> Style {
        if let Some((pos, style)) = &self.typing
            && *pos == at
        {
            return style.clone();
        }
        let mut pos = 0;
        let mut last = None;
        for r in &self.runs {
            if at > pos && at <= pos + r.len {
                return r.style.clone();
            }
            if at == 0 && r.len > 0 {
                return r.style.clone();
            }
            pos += r.len;
            last = Some(&r.style);
        }
        last.cloned().unwrap_or_default()
    }

    /// The text field changed it to `new`: work out what was replaced and
    /// move the styles to match. `selection` is the selection before the
    /// change, for undo.
    pub fn sync(&mut self, new: &str, selection: Range<usize>) {
        if new == self.text {
            return;
        }
        let old = &self.text;
        let mut p = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
        p = floor_boundary(old, p.min(old.len())).min(floor_boundary(new, p.min(new.len())));
        let max_s = (old.len() - p).min(new.len() - p);
        let mut s = old.bytes().rev().zip(new.bytes().rev()).take(max_s).take_while(|(a, b)| a == b).count();
        while s > 0 && (!old.is_char_boundary(old.len() - s) || !new.is_char_boundary(new.len() - s)) {
            s -= 1;
        }
        let removed = p..old.len() - s;
        let inserted = &new[p..new.len() - s];

        // Undo: typing runs together into one step, as do deletions.
        let kind = match (removed.is_empty(), inserted.chars().count()) {
            (true, 1) if !inserted.contains(char::is_whitespace) => EditKind::Typing,
            (false, 0) if removed.len() <= 4 => EditKind::Deleting,
            _ => EditKind::Other,
        };
        let joins = matches!(self.last_edit, Some((k, t, at)) if k == kind && kind != EditKind::Other
            && t.elapsed() < Duration::from_secs(2)
            && (at == p || at == removed.end));
        if !joins {
            self.push_undo(selection);
        }
        let caret = p + inserted.len();
        self.last_edit = Some((kind, Instant::now(), caret));
        let inserted = inserted.to_owned();
        self.splice(removed, &inserted);
        self.redo.clear();
        self.generation += 1;
    }

    /// Replace bytes `removed` with `inserted` in the text and the runs.
    /// Pasting what was copied here brings its styles; other text takes
    /// the pending typing style, the style of what it replaces, or the
    /// style of the character before it.
    fn splice(&mut self, removed: Range<usize>, inserted: &str) {
        let p = removed.start;
        let added = match &self.clip {
            Some((text, runs)) if text == inserted => runs.clone(),
            _ => {
                let style = match &self.typing {
                    Some((pos, style)) if *pos == p => style.clone(),
                    _ if !removed.is_empty() => self.style_at(p + self.text[p..].chars().next().map_or(0, char::len_utf8)),
                    _ => self.style_at(p),
                };
                vec![Run { len: inserted.len(), style }]
            }
        };
        let start = split_at(&mut self.runs, removed.start);
        let end = split_at(&mut self.runs, removed.end);
        self.runs.splice(start..end, added);
        normalize(&mut self.runs);
        self.text.replace_range(removed, inserted);
        if self.runs.is_empty() && !self.text.is_empty() {
            self.runs.push(Run { len: self.text.len(), style: Style::default() });
        }
        debug_assert_eq!(self.runs.iter().map(|r| r.len).sum::<usize>(), self.text.len());
        // Typing at the remembered spot used the pending style; it moves on.
        self.typing = self.typing.take().filter(|(pos, _)| *pos == p && inserted.is_empty());
    }

    /// Replace every range (bytes, in order, not overlapping) with `with`,
    /// as one undoable step.
    pub fn replace_all(&mut self, ranges: &[Range<usize>], with: &str, selection: Range<usize>) {
        if ranges.is_empty() {
            return;
        }
        self.push_undo(selection);
        self.last_edit = None;
        self.typing = None;
        for r in ranges.iter().rev() {
            self.splice(r.clone(), with);
        }
        self.redo.clear();
        self.generation += 1;
    }

    fn push_undo(&mut self, selection: Range<usize>) {
        self.undo.push(State { text: self.text.clone(), runs: self.runs.clone(), selection });
        // Plenty, without keeping every keystroke of a long session.
        if self.undo.len() > 500 {
            self.undo.remove(0);
        }
    }

    /// Change the style over `range` (bytes) with `f`. With nothing
    /// selected, the change applies to what is typed next there.
    pub fn restyle(&mut self, range: Range<usize>, selection: Range<usize>, f: impl Fn(&mut Style)) {
        if range.is_empty() {
            let mut style = self.style_at(range.start);
            f(&mut style);
            self.typing = Some((range.start, style));
            return;
        }
        self.push_undo(selection);
        self.last_edit = None;
        let start = split_at(&mut self.runs, range.start);
        let end = split_at(&mut self.runs, range.end);
        for r in &mut self.runs[start..end] {
            f(&mut r.style);
        }
        normalize(&mut self.runs);
        self.redo.clear();
        self.generation += 1;
    }

    /// Remember a copy, so pasting it here keeps its styles.
    pub fn copied(&mut self, range: Range<usize>) {
        let text = self.text[range.clone()].to_owned();
        let mut runs = self.runs.clone();
        let start = split_at(&mut runs, range.start);
        let end = split_at(&mut runs, range.end);
        self.clip = Some((text, runs[start..end].to_vec()));
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Step back; returns the selection to show (characters).
    pub fn undo(&mut self, selection: Range<usize>) -> Option<Range<usize>> {
        let prev = self.undo.pop()?;
        self.redo.push(State { text: std::mem::replace(&mut self.text, prev.text), runs: std::mem::replace(&mut self.runs, prev.runs), selection });
        self.last_edit = None;
        self.typing = None;
        self.generation += 1;
        Some(prev.selection)
    }

    pub fn redo(&mut self, selection: Range<usize>) -> Option<Range<usize>> {
        let next = self.redo.pop()?;
        self.undo.push(State { text: std::mem::replace(&mut self.text, next.text), runs: std::mem::replace(&mut self.runs, next.runs), selection });
        self.last_edit = None;
        self.typing = None;
        self.generation += 1;
        Some(next.selection)
    }

    /// Replace `range` (bytes) with `with`, as one undoable step (Replace).
    pub fn replace(&mut self, range: Range<usize>, with: &str, selection: Range<usize>) {
        self.replace_all(&[range], with, selection);
    }

    /// The text laid out with its styles. `family` maps a style to the
    /// egui family to draw it with (fonts load lazily).
    pub fn layout(&self, text: &str, wrap: f32, base: Color32, family: &mut dyn FnMut(&Style) -> FontFamily) -> LayoutJob {
        let mut job = LayoutJob { text: text.to_owned(), ..Default::default() };
        job.wrap.max_width = if self.wrap { wrap } else { f32::INFINITY };
        job.halign = match self.align {
            Align::Left | Align::Justify => egui::Align::Min,
            Align::Center => egui::Align::Center,
            Align::Right => egui::Align::Max,
        };
        job.justify = self.align == Align::Justify;
        let mut pos = 0;
        // The runs fit the synced text; text the field has but we haven't
        // synced yet (mid-frame) uses the last style.
        for r in &self.runs {
            let end = (pos + r.len).min(text.len());
            if end > pos && text.is_char_boundary(pos) && text.is_char_boundary(end) {
                job.sections.push(section(pos..end, &r.style, base, family));
            }
            pos += r.len;
        }
        if pos < text.len() {
            let start = floor_boundary(text, pos.min(text.len()));
            let style = self.runs.last().map(|r| r.style.clone()).unwrap_or_default();
            job.sections.push(section(start..text.len(), &style, base, family));
        }
        if job.sections.is_empty() {
            // An empty document still needs a line height for the caret.
            let style = self.typing.as_ref().map(|(_, s)| s.clone()).unwrap_or_default();
            job.sections.push(section(0..0, &style, base, family));
        }
        job
    }
}

fn section(range: Range<usize>, s: &Style, base: Color32, family: &mut dyn FnMut(&Style) -> FontFamily) -> egui::text::LayoutSection {
    let color = s.color.map_or(base, |[r, g, b]| Color32::from_rgb(r, g, b));
    egui::text::LayoutSection {
        leading_space: 0.0,
        byte_range: egui::text::ByteIndex(range.start)..egui::text::ByteIndex(range.end),
        format: egui::TextFormat {
            font_id: FontId::new(s.size, family(s)),
            color,
            underline: if s.underline { Stroke::new((s.size / 14.0).max(1.0), color) } else { Stroke::NONE },
            strikethrough: if s.strikeout { Stroke::new((s.size / 14.0).max(1.0), color) } else { Stroke::NONE },
            ..Default::default()
        },
    }
}

/// Find `needle` in `hay` from byte `from`: forwards (the first match
/// starting at or after it) or backwards (the last one ending at or before
/// it), wrapping round the end if asked. Byte range of the match.
pub fn find(hay: &str, needle: &str, from: usize, backwards: bool, case: bool, wrap: bool) -> Option<Range<usize>> {
    if needle.is_empty() {
        return None;
    }
    let fold = |c: char| if case { c } else { c.to_lowercase().next().unwrap_or(c) };
    let needle: Vec<char> = needle.chars().map(fold).collect();
    // The match starting at byte `i`, if there is one.
    let at = |i: usize| -> Option<Range<usize>> {
        let mut it = hay[i..].char_indices();
        for n in &needle {
            let (_, c) = it.next()?;
            if fold(c) != *n {
                return None;
            }
        }
        Some(i..it.next().map_or(hay.len(), |(j, _)| i + j))
    };
    let starts: Vec<usize> = hay.char_indices().map(|(i, _)| i).collect();
    let from = from.min(hay.len());
    if backwards {
        let before = starts.iter().rev().filter(|&&i| i < from).find_map(|&i| at(i).filter(|r| r.end <= from));
        before.or_else(|| if wrap { starts.iter().rev().find_map(|&i| at(i)) } else { None })
    } else {
        let after = starts.iter().filter(|&&i| i >= from).find_map(|&i| at(i));
        after.or_else(|| if wrap { starts.iter().find_map(|&i| at(i)) } else { None })
    }
}

/// Every match of `needle`, in order, not overlapping.
pub fn find_all(hay: &str, needle: &str, case: bool) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(r) = find(hay, needle, from, false, case, false) {
        from = r.end;
        out.push(r);
    }
    out
}

// ---- files ----------------------------------------------------------------

const ATTR_STYLES: &str = "user.tabula.styles";
const ATTR_ALIGN: &str = "user.tabula.alignment";
const ATTR_WRAP: &str = "user.tabula.wrap";

/// What a file holds.
pub struct Loaded {
    pub text: String,
    pub runs: Option<Vec<Run>>,
    pub align: Align,
    pub wrap: bool,
    /// Not UTF-8: read as Latin-1 (saved back as UTF-8).
    pub latin1: bool,
    /// We may not write it.
    pub read_only: bool,
}

fn get_attr(path: &Path, name: &str) -> Option<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    let p = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let n = std::ffi::CString::new(name).ok()?;
    // SAFETY: valid C strings; a null buffer asks for the size.
    let size = unsafe { libc::getxattr(p.as_ptr(), n.as_ptr(), std::ptr::null_mut(), 0) };
    if size <= 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    // SAFETY: buf has room for `size` bytes.
    let got = unsafe { libc::getxattr(p.as_ptr(), n.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
    (got >= 0).then(|| {
        buf.truncate(got as usize);
        buf
    })
}

fn set_attr(path: &Path, name: &str, value: &[u8]) {
    use std::os::unix::ffi::OsStrExt;
    let (Ok(p), Ok(n)) = (std::ffi::CString::new(path.as_os_str().as_bytes()), std::ffi::CString::new(name)) else {
        return;
    };
    // SAFETY: valid C strings and buffer. Failure (a filesystem without
    // user attributes) just loses the styles.
    unsafe { libc::setxattr(p.as_ptr(), n.as_ptr(), value.as_ptr().cast(), value.len(), 0) };
}

/// Read a file (on a worker thread).
pub fn load(path: &Path) -> Result<Loaded, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let (text, latin1) = match String::from_utf8(bytes) {
        Ok(t) => (t, false),
        Err(e) => (e.into_bytes().iter().map(|&b| b as char).collect(), true),
    };
    let runs = get_attr(path, ATTR_STYLES).and_then(|v| serde_json::from_slice::<Vec<Run>>(&v).ok());
    let align = get_attr(path, ATTR_ALIGN).and_then(|v| serde_json::from_slice(&v).ok()).unwrap_or_default();
    let wrap = get_attr(path, ATTR_WRAP).is_none_or(|v| v != b"false");
    let read_only = {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::CString::new(path.as_os_str().as_bytes())
            // SAFETY: a valid C string.
            .is_ok_and(|p| unsafe { libc::access(p.as_ptr(), libc::W_OK) } != 0)
    };
    Ok(Loaded { text, runs: if latin1 { None } else { runs }, align, wrap, latin1, read_only })
}

/// Write a file whole (beside it, then renamed over it, keeping its
/// permissions), styles in its attributes. On a worker thread.
pub fn save(path: &Path, text: &str, runs: &[Run], align: Align, wrap: bool) -> Result<(), String> {
    let dir = path.parent().ok_or("no folder")?;
    let name = path.file_name().ok_or("no name")?.to_string_lossy();
    let tmp = dir.join(format!(".{name}.saving"));
    let res = (|| {
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        if let Ok(meta) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        // Plain text needs no styles attribute.
        let plain = runs.len() <= 1 && runs.first().is_none_or(|r| r.style == Style::default());
        if !plain {
            set_attr(&tmp, ATTR_STYLES, &serde_json::to_vec(runs).unwrap_or_default());
        }
        if align != Align::Left {
            set_attr(&tmp, ATTR_ALIGN, &serde_json::to_vec(&align).unwrap_or_default());
        }
        if !wrap {
            set_attr(&tmp, ATTR_WRAP, b"false");
        }
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bold() -> Style {
        Style { bold: true, ..Style::default() }
    }

    fn lens(d: &Doc) -> Vec<(usize, bool)> {
        d.runs.iter().map(|r| (r.len, r.style.bold)).collect()
    }

    #[test]
    fn typing_continues_the_style_before() {
        let mut d = Doc::new("hello world".into(), None);
        d.restyle(0..5, 0..0, |s| s.bold = true);
        assert_eq!(lens(&d), [(5, true), (6, false)]);
        d.sync("hello! world", 5..5);
        assert_eq!(lens(&d), [(6, true), (6, false)]);
        d.sync("Xhello! world", 0..0);
        assert_eq!(lens(&d), [(7, true), (6, false)]);
    }

    #[test]
    fn deleting_across_runs() {
        let mut d = Doc::new("abcdef".into(), None);
        d.restyle(2..4, 0..0, |s| s.bold = true);
        assert_eq!(lens(&d), [(2, false), (2, true), (2, false)]);
        d.sync("af", 1..5);
        assert_eq!(lens(&d), [(2, false)]);
        assert_eq!(d.text(), "af");
    }

    #[test]
    fn pending_style_for_typing() {
        let mut d = Doc::new("ab".into(), None);
        d.restyle(1..1, 1..1, |s| s.bold = true);
        d.sync("aXb", 1..1);
        assert_eq!(lens(&d), [(1, false), (1, true), (1, false)]);
        // It sticks for the next character too (typing continues it).
        d.sync("aXYb", 2..2);
        assert_eq!(lens(&d), [(1, false), (2, true), (1, false)]);
    }

    #[test]
    fn multibyte_edits() {
        let mut d = Doc::new("héllo".into(), None);
        d.restyle(0..3, 0..0, |s| s.bold = true);
        d.sync("hèllo", 1..2);
        assert_eq!(d.text(), "hèllo");
        assert_eq!(d.runs.iter().map(|r| r.len).sum::<usize>(), "hèllo".len());
        d.sync("h€llo", 1..2);
        assert_eq!(d.runs.iter().map(|r| r.len).sum::<usize>(), "h€llo".len());
    }

    #[test]
    fn undo_redo_and_coalescing() {
        let mut d = Doc::new(String::new(), None);
        for (i, t) in ["h", "he", "hel", "hell", "hello"].iter().enumerate() {
            d.sync(t, i..i);
        }
        d.sync("hello ", 5..5);
        d.sync("hello w", 6..6);
        // "hello" was one step; the space and "w" are more.
        d.undo(7..7);
        d.undo(6..6);
        assert_eq!(d.text(), "hello");
        d.undo(5..5);
        assert_eq!(d.text(), "");
        assert!(!d.can_undo());
        d.redo(0..0);
        assert_eq!(d.text(), "hello");
        d.restyle(0..5, 0..5, |s| *s = bold());
        d.undo(0..5);
        assert_eq!(lens(&d), [(5, false)]);
        d.redo(0..5);
        assert_eq!(lens(&d), [(5, true)]);
    }

    #[test]
    fn paste_keeps_copied_styles() {
        let mut d = Doc::new("ab cd".into(), None);
        d.restyle(0..2, 0..0, |s| s.bold = true);
        d.copied(0..3);
        d.sync("ab cdab ", 5..5);
        assert_eq!(lens(&d), [(2, true), (3, false), (2, true), (1, false)]);
    }

    #[test]
    fn mismatched_styles_are_dropped() {
        let runs = vec![Run { len: 99, style: bold() }];
        let d = Doc::new("short".into(), Some(runs));
        assert_eq!(lens(&d), [(5, false)]);
    }

    #[test]
    fn replacing_takes_the_replaced_style() {
        let mut d = Doc::new("ab cd".into(), None);
        d.restyle(3..5, 0..0, |s| s.bold = true);
        d.sync("ab X", 3..5);
        assert_eq!(lens(&d), [(3, false), (1, true)]);
    }

    #[test]
    fn replace_all_keeps_styles_between() {
        let mut d = Doc::new("cat dog cat".into(), None);
        d.restyle(4..7, 0..0, |s| s.bold = true);
        let ranges = find_all(d.text(), "CAT", false);
        assert_eq!(ranges, [0..3, 8..11]);
        d.replace_all(&ranges, "mouse", 0..0);
        assert_eq!(d.text(), "mouse dog mouse");
        assert_eq!(lens(&d), [(6, false), (3, true), (6, false)]);
        d.undo(0..0);
        assert_eq!(d.text(), "cat dog cat");
    }

    #[test]
    fn finding() {
        let t = "Straße café CAFÉ";
        assert_eq!(find(t, "café", 0, false, true, false), Some(8..13));
        assert_eq!(find(t, "café", 9, false, false, false), Some(14..19));
        assert_eq!(find(t, "café", 9, false, true, false), None);
        assert_eq!(find(t, "café", 9, false, true, true), Some(8..13));
        assert_eq!(find(t, "café", 19, true, false, false), Some(14..19));
        assert_eq!(find(t, "café", 14, true, false, false), Some(8..13));
        assert_eq!(find(t, "ß", 0, false, true, false), Some(4..6));
    }

    #[test]
    fn files_round_trip() {
        let dir = std::env::temp_dir().join(format!("styled-edit-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("note.txt");
        let mut d = Doc::new("Hello, world".into(), None);
        d.restyle(0..5, 0..0, |s| {
            s.bold = true;
            s.color = Some([200, 0, 0]);
        });
        save(&path, d.text(), d.runs(), Align::Center, false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Hello, world");
        let l = load(&path).unwrap();
        assert_eq!(l.text, "Hello, world");
        // Some filesystems (tmpfs on old kernels) refuse user attributes.
        if let Some(runs) = l.runs {
            assert_eq!(runs, d.runs);
            assert_eq!(l.align, Align::Center);
            assert!(!l.wrap);
        }
        std::fs::write(&path, [b'c', b'a', b'f', 0xE9]).unwrap();
        let l = load(&path).unwrap();
        assert_eq!(l.text, "café");
        assert!(l.latin1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
