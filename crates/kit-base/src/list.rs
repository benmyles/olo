//! A list's selection and sort, for the kit's list views (olo-ui's
//! `ColumnListView`, Tracker's icon view): the selection every file
//! manager has ([`ListState::click`], [`ListState::move_to`] for the
//! keyboard, [`ListState::band`] for a selection band, select all,
//! invert), and type to select ([`TypeSelect`]).

use std::{collections::BTreeSet, time::Instant};

/// The modifiers a click or band starts with: ⌘ (Ctrl on a PC keyboard)
/// and ⇧.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub command: bool,
    pub shift: bool,
}

/// Selection and sort state the app keeps for a list (or a grid of
/// icons: rows are items in their order). Its methods are the selection
/// every file manager has: [`click`](Self::click),
/// [`move_to`](Self::move_to) for the keyboard, [`band`](Self::band) for
/// a selection band, [`select_all`](Self::select_all) and
/// [`invert`](Self::invert).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListState {
    pub selected: BTreeSet<usize>,
    /// Where a ⇧ range starts: the row last clicked alone or with ⌘.
    pub anchor: Option<usize>,
    /// Where the keyboard is: the row last clicked or moved to, the far
    /// end of a ⇧ range ([`cursor`](Self::cursor)).
    pub cursor: Option<usize>,
    /// Sort column and ascending flag.
    pub sort: Option<(usize, bool)>,
    /// Whether more than one row may be selected.
    pub multiple: bool,
}

impl ListState {
    pub fn single(&self) -> Option<usize> {
        self.selected.iter().next().copied()
    }

    /// `row` alone.
    pub fn select(&mut self, row: usize) {
        self.selected.clear();
        self.selected.insert(row);
        self.anchor = Some(row);
        self.cursor = Some(row);
    }

    /// The row the keyboard moves on from: the last clicked or moved to,
    /// else the anchor (when the selection was set some other way).
    pub fn cursor(&self) -> Option<usize> {
        self.cursor.filter(|c| self.selected.contains(c)).or(self.anchor)
    }

    /// A click on `row`: it alone; with ⌘ it added or taken away; with ⇧
    /// everything from the anchor to it (the anchor stays, so another ⇧
    /// click makes the range again from there); with ⌘⇧ that range added
    /// to what's selected.
    pub fn click(&mut self, row: usize, mods: Mods) {
        if !self.multiple || !(mods.command || mods.shift) {
            self.select(row);
        } else if mods.shift {
            let a = self.anchor.unwrap_or(row);
            let range = a.min(row)..=a.max(row);
            if mods.command {
                self.selected.extend(range);
            } else {
                self.selected = range.collect();
            }
            self.anchor = Some(a);
            self.cursor = Some(row);
        } else {
            if !self.selected.remove(&row) {
                self.selected.insert(row);
            }
            self.anchor = Some(row);
            self.cursor = Some(row);
        }
    }

    /// The keyboard moved to `row`: it alone, or with ⇧ (`extend`)
    /// everything from the anchor to it, so ⇧↓ grows a range and ⇧↑
    /// takes it back.
    pub fn move_to(&mut self, row: usize, extend: bool) {
        if extend && self.multiple {
            let a = self.anchor.or(self.cursor).unwrap_or(row);
            self.selected = (a.min(row)..=a.max(row)).collect();
            self.anchor = Some(a);
            self.cursor = Some(row);
        } else {
            self.select(row);
        }
    }

    /// The row `d` on from the cursor among `rows`, kept to them; with no
    /// cursor, the first going forward and the last going back.
    pub fn step(&self, d: isize, rows: usize) -> Option<usize> {
        let last = rows.checked_sub(1)?;
        Some(match self.cursor() {
            Some(c) => (c as isize + d).clamp(0, last as isize) as usize,
            None if d < 0 => last,
            None => 0,
        })
    }

    /// Every one of `rows`.
    pub fn select_all(&mut self, rows: usize) {
        if self.multiple {
            self.selected = (0..rows).collect();
        }
    }

    /// Every one of `rows` that wasn't selected, and none that was.
    pub fn invert(&mut self, rows: usize) {
        if self.multiple {
            self.selected = (0..rows).filter(|r| !self.selected.contains(r)).collect();
        }
    }

    /// A selection band starting with `mods` held: only what it covers;
    /// with ⇧ that added to what's selected now; with ⌘ what it covers
    /// flipped, as in Finder.
    pub fn band(&self, mods: Mods) -> SelectionBand {
        let keep = self.multiple && (mods.command || mods.shift);
        SelectionBand { base: if keep { self.selected.clone() } else { BTreeSet::new() }, flip: self.multiple && mods.command }
    }
}

/// What was selected when a selection band started, and how what it
/// covers changes that ([`ListState::band`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SelectionBand {
    pub base: BTreeSet<usize>,
    pub flip: bool,
}

impl SelectionBand {
    /// The selection with the band over `rows`.
    pub fn over(&self, rows: impl IntoIterator<Item = usize>) -> BTreeSet<usize> {
        let mut out = self.base.clone();
        for r in rows {
            if !(self.flip && out.remove(&r)) {
                out.insert(r);
            }
        }
        out
    }
}

/// What's being typed to select: letters typed within a moment of each
/// other are the start of a name ([`type_match`]); a space starts
/// nothing (it's Quick Look's, a button's) but goes into a name being
/// typed.
#[derive(Debug, Clone, Default)]
pub struct TypeSelect {
    prefix: String,
    at: Option<Instant>,
}

impl TypeSelect {
    /// `typed` came: the row to go to, of `rows` named by `name`, from
    /// the row the keyboard is on.
    pub fn typed<'n>(&mut self, typed: &str, rows: usize, from: Option<usize>, name: impl Fn(usize) -> &'n str) -> Option<usize> {
        let now = Instant::now();
        if self.at.is_none_or(|at| now.duration_since(at).as_secs_f64() > TYPE_SELECT_PAUSE) {
            self.prefix.clear();
        }
        let typed = if self.prefix.is_empty() { typed.trim_start() } else { typed };
        if typed.is_empty() || rows == 0 {
            return None;
        }
        self.prefix.push_str(&typed.to_lowercase());
        self.at = Some(now);
        type_match(&self.prefix, rows, from, name)
    }

    /// A name is being typed (a space now is part of it).
    pub fn typing(&self) -> bool {
        self.at.is_some_and(|at| at.elapsed().as_secs_f64() <= TYPE_SELECT_PAUSE)
    }
}

/// The row typing `prefix` (lower case) goes to ([`TypeSelect`]):
/// the first whose name starts so, else (the same letter typed again and
/// again) the next starting with it after `from`, else the first after
/// it in name order, as Finder does.
pub fn type_match<'n>(prefix: &str, rows: usize, from: Option<usize>, name: impl Fn(usize) -> &'n str) -> Option<usize> {
    let lower = |i: usize| name(i).to_lowercase();
    let mut chars = prefix.chars();
    let first = chars.next()?;
    let repeated = prefix.chars().count() > 1 && chars.all(|c| c == first);
    if let Some(i) = (0..rows).find(|&i| lower(i).starts_with(prefix)) {
        return Some(i);
    }
    if repeated {
        let starts: Vec<usize> = (0..rows).filter(|&i| lower(i).starts_with(first)).collect();
        if !starts.is_empty() {
            let next = from.and_then(|f| starts.iter().find(|&&i| i > f)).unwrap_or(&starts[0]);
            return Some(*next);
        }
    }
    (0..rows).filter(|&i| lower(i).as_str() > prefix).min_by_key(|&i| lower(i))
}

/// How long a pause ends what's typed to select (seconds).
pub const TYPE_SELECT_PAUSE: f64 = 0.9;

#[cfg(test)]
mod tests {
    use super::{ListState, Mods, type_match};
    use std::collections::BTreeSet;

    fn list(selected: &[usize]) -> ListState {
        let mut l = ListState { multiple: true, ..Default::default() };
        l.selected = selected.iter().copied().collect();
        l
    }

    fn set(rows: &[usize]) -> BTreeSet<usize> {
        rows.iter().copied().collect()
    }

    const NONE: Mods = Mods { command: false, shift: false };
    const SHIFT: Mods = Mods { command: false, shift: true };
    const CMD: Mods = Mods { command: true, shift: false };
    const BOTH: Mods = Mods { command: true, shift: true };

    #[test]
    fn selection_clicks() {
        let mut l = list(&[]);
        l.click(3, NONE);
        assert_eq!(l.selected, set(&[3]));
        // ⇧: from the anchor, again from the same anchor.
        l.click(6, SHIFT);
        assert_eq!(l.selected, set(&[3, 4, 5, 6]));
        l.click(1, SHIFT);
        assert_eq!(l.selected, set(&[1, 2, 3]));
        assert_eq!((l.anchor, l.cursor()), (Some(3), Some(1)));
        // ⌘ adds one, and is the anchor for the next ⌘⇧ range, added.
        l.click(8, CMD);
        assert_eq!(l.selected, set(&[1, 2, 3, 8]));
        l.click(10, BOTH);
        assert_eq!(l.selected, set(&[1, 2, 3, 8, 9, 10]));
        // ⌘ takes one away.
        l.click(2, CMD);
        assert_eq!(l.selected, set(&[1, 3, 8, 9, 10]));
        // One row only: modifiers don't add.
        let mut one = ListState::default();
        one.click(2, NONE);
        one.click(5, SHIFT);
        assert_eq!(one.selected, set(&[5]));
    }

    #[test]
    fn selection_keys() {
        let mut l = list(&[]);
        // Nothing selected: down to the first, up to the last.
        assert_eq!((l.step(1, 10), l.step(-1, 10), l.step(1, 0)), (Some(0), Some(9), None));
        l.select(4);
        // ⇧↓ grows from the anchor, ⇧↑ takes it back and past.
        l.move_to(l.step(1, 10).unwrap(), true);
        l.move_to(l.step(1, 10).unwrap(), true);
        assert_eq!(l.selected, set(&[4, 5, 6]));
        for _ in 0..3 {
            l.move_to(l.step(-1, 10).unwrap(), true);
        }
        assert_eq!(l.selected, set(&[3, 4]));
        // A plain arrow: one, from where the keyboard was.
        l.move_to(l.step(-1, 10).unwrap(), false);
        assert_eq!(l.selected, set(&[2]));
        // Set in code (the anchor alone): moves on from that.
        l.selected = set(&[7]);
        l.anchor = Some(7);
        assert_eq!(l.step(1, 10), Some(8));
        l.select_all(10);
        assert_eq!(l.selected.len(), 10);
        l.selected = set(&[0, 2]);
        l.invert(4);
        assert_eq!(l.selected, set(&[1, 3]));
    }

    #[test]
    fn selection_bands() {
        let l = list(&[1, 2]);
        assert_eq!(l.band(NONE).over([2, 3]), set(&[2, 3]));
        assert_eq!(l.band(SHIFT).over([2, 3]), set(&[1, 2, 3]));
        assert_eq!(l.band(CMD).over([2, 3]), set(&[1, 3]));
    }

    #[test]
    fn typing_selects() {
        let names = ["Apple", "apricot", "Banana", "berry", "Cherry", "Date"];
        let m = |p: &str, from: Option<usize>| type_match(p, names.len(), from, |i| names[i]);
        assert_eq!(m("ap", None), Some(0));
        assert_eq!(m("apr", None), Some(1));
        assert_eq!(m("be", None), Some(3));
        // None starts so: the first after it.
        assert_eq!(m("bz", None), Some(4));
        assert_eq!(m("zz", None), None);
        // One letter again and again: through the names starting with it.
        assert_eq!(m("bb", Some(2)), Some(3));
        assert_eq!(m("bbb", Some(3)), Some(2));
    }
}
