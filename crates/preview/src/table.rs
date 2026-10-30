//! Spreadsheets and CSV: every column, scrolled sideways when they don't
//! all fit (`ColumnListView`); the first row as the columns' names when
//! it looks like a header; a search, in every column or one, that keeps
//! only the rows with it; and sorting by a column (its header, again to
//! reverse), numbers as numbers and names as people sort them ("file2"
//! before "file10"). A big sheet is filtered and sorted on a worker,
//! the rows shown until then staying put.

use std::{
    cmp::Ordering,
    sync::{
        Arc,
        atomic::{self, AtomicBool},
        mpsc,
    },
};

use olo_kit::{
    egui::{self, Align, Rect, Ui, pos2, vec2},
    metrics as m,
    style::{self, c},
    widgets::{Column, ColumnListView, ListState, MenuField, TextControl},
};

/// A sheet's rows, shared with the worker that filters and sorts them.
pub type Rows = Arc<Vec<Vec<String>>>;

/// Sheets this big (cells) are filtered and sorted on a worker.
const ON_A_WORKER: usize = 40_000;

/// The bar over the table: search, where, and how many rows show (its
/// rule inside it; on the 2-point grid).
const BAR: f32 = m::CONTROL_HEIGHT + 2.0 * m::TIGHT + 2.0;

#[derive(Default)]
pub struct TableView {
    search: String,
    /// 0: every column; else the column before it.
    scope: usize,
    /// The rows shown (indices into the sheet), in order, and what for.
    shown: Option<(Key, Vec<usize>)>,
    /// A worker making them, for that.
    making: Option<(Key, mpsc::Receiver<Vec<usize>>, Arc<AtomicBool>)>,
}

/// What the rows shown depend on.
#[derive(Clone, PartialEq)]
struct Key {
    sheet: (u64, usize),
    rows: usize,
    search: String,
    scope: usize,
    sort: Option<(usize, bool)>,
}

impl TableView {
    /// Start again (another file, or another sheet of it).
    pub fn reset(&mut self) {
        if let Some((_, _, cancel)) = self.making.take() {
            cancel.store(true, atomic::Ordering::Relaxed);
        }
        *self = TableView::default();
    }

    /// The sheet `rows` (`id`: the document and sheet) in `r`.
    pub fn show(&mut self, ui: &mut Ui, r: Rect, rows: &Rows, list: &mut ListState, id: (u64, usize)) {
        let p = style::palette(ui.ctx());
        let ncols = rows.iter().map(Vec::len).max().unwrap_or(0).clamp(1, 200);
        let header = looks_like_header(rows);
        let first = usize::from(header);
        let titles: Vec<String> = (0..ncols)
            .map(|k| match rows.first().and_then(|row| row.get(k)).filter(|_| header).map(|t| t.trim()) {
                Some(t) if !t.is_empty() => t.to_owned(),
                _ => column_name(k),
            })
            .collect();
        if self.scope > ncols {
            self.scope = 0;
        }

        // The bar: search, in which column, and how many rows show.
        let bar = Rect::from_min_max(r.min, pos2(r.right(), r.top() + BAR));
        ui.painter().rect_filled(bar, 0.0, c(p.panel_background));
        style::hline_above(ui.painter(), bar.left(), bar.right(), bar.bottom(), style::shade(&p, p.panel_background, 1.2));
        let row_r = Rect::from_min_max(pos2(bar.left() + m::SPACING, bar.top() + m::TIGHT), pos2(bar.right() - m::SPACING, bar.bottom() - m::TIGHT - 2.0));
        let total = rows.len().saturating_sub(first);
        let count = match &self.shown {
            Some((_, order)) if order.len() != total => format!("{} of {total} rows", order.len()),
            _ if total == 1 => "1 row".to_owned(),
            _ => format!("{total} rows"),
        };
        let count_w = style::label_width(ui.ctx(), &count, style::small_font()).ceil();
        let mut options: Vec<&str> = vec!["All Columns"];
        options.extend(titles.iter().map(String::as_str));
        let scope_w = 150.0f32.min((row_r.width() - count_w - 3.0 * m::SPACING) / 2.0).max(90.0);
        let search_w = (row_r.width() - scope_w - count_w - 3.0 * m::SPACING).clamp(80.0, 280.0);
        let search_r = Rect::from_min_size(row_r.min, vec2(search_w, m::CONTROL_HEIGHT));
        let search = ui.put(search_r, TextControl::new(&mut self.search).hint("Search"));
        ui.ctx().accesskit_node_builder(search.id, |n| n.set_label("Search"));
        if search.has_focus() && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            if self.search.is_empty() {
                search.surrender_focus();
            }
            self.search.clear();
        }
        if ncols > 1 {
            let scope_r = Rect::from_min_size(pos2(search_r.right() + m::SPACING, row_r.top()), vec2(scope_w, m::CONTROL_HEIGHT));
            ui.put(scope_r, MenuField::new(("ql-table-scope", id), &mut self.scope, &options).width(scope_w));
        }
        let searching = self.making.is_some();
        let label = if searching { "Searching…".to_owned() } else { count };
        style::label(ui.painter(), row_r.right(), row_r.center().y, Align::Max, label, style::small_font(), style::disabled_text(&p));

        // The rows shown: as they were asked for, here or (a big sheet)
        // from a worker, the last ones showing meanwhile.
        let key = Key { sheet: id, rows: rows.len(), search: self.search.trim().to_lowercase(), scope: self.scope, sort: list.sort };
        if self.making.as_ref().is_some_and(|(k, ..)| *k != key) {
            let (_, _, cancel) = self.making.take().unwrap();
            cancel.store(true, atomic::Ordering::Relaxed);
        }
        if let Some((k, rx, _)) = &self.making
            && let Ok(order) = rx.try_recv()
        {
            self.shown = Some((k.clone(), order));
            self.making = None;
            list.selected.clear();
        }
        let stale = self.shown.as_ref().is_none_or(|(k, _)| *k != key);
        if stale && self.making.is_none() {
            if rows.len() * ncols <= ON_A_WORKER {
                self.shown = Some((key.clone(), arrange(rows, first, &key, &AtomicBool::new(false))));
                list.selected.clear();
            } else {
                let (tx, rx) = mpsc::channel();
                let cancel = Arc::new(AtomicBool::new(false));
                let (rows, k, stop, ctx) = (rows.clone(), key.clone(), cancel.clone(), ui.ctx().clone());
                std::thread::Builder::new()
                    .name("ql-table".into())
                    .spawn(move || {
                        let order = arrange(&rows, first, &k, &stop);
                        if !stop.load(atomic::Ordering::Relaxed) {
                            let _ = tx.send(order);
                            ctx.request_repaint();
                        }
                    })
                    .expect("thread");
                self.making = Some((key, rx, cancel));
            }
        }
        if self.making.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
        }
        let all: Vec<usize>;
        let order: &[usize] = match &self.shown {
            Some((_, o)) => o,
            None => {
                all = (first..rows.len()).collect();
                &all
            }
        };

        // Each column as wide as what it holds, within reason; one of
        // numbers lined up on the right.
        let cols: Vec<Column> = (0..ncols)
            .map(|k| {
                let sample = || rows.iter().skip(first).take(200).filter_map(|r| r.get(k)).filter(|s| !s.trim().is_empty());
                let longest = sample().map(|s| s.chars().count()).chain([titles[k].chars().count() + 2]).max().unwrap_or(4);
                let col = Column::new(&titles[k], (longest as f32 * 7.0 + 14.0).clamp(48.0, 320.0));
                let (cells, numbers) = sample().fold((0, 0), |(n, k), s| (n + 1, k + usize::from(number(s).is_some())));
                if cells > 0 && numbers * 10 >= cells * 9 { col.right() } else { col }
            })
            .collect();
        let area = Rect::from_min_max(pos2(r.left(), bar.bottom()), r.max);
        // The cells line up with each other, not with the bar's search
        // (a column of numbers on the right meets it only by chance).
        olo_kit::layout_review::own_grid(ui, area);
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(area));
        let empty = if self.search.trim().is_empty() { "No rows." } else { "No rows match." };
        ColumnListView::new(("ql-table", id), &cols, order.len(), list).empty_text(empty).show(&mut ui, |cell| {
            if let Some(s) = order.get(cell.row).and_then(|&i| rows.get(i)).and_then(|r| r.get(cell.column)) {
                cell.text(s);
            }
        });
    }
}

/// The rows (indices) with `key.search` in them, in `key.sort`'s order;
/// what there is so far when `stop` is set.
fn arrange(rows: &[Vec<String>], first: usize, key: &Key, stop: &AtomicBool) -> Vec<usize> {
    let words: Vec<&str> = key.search.split_whitespace().collect();
    let has = |cell: &str, w: &str| cell.to_lowercase().contains(w);
    let mut order: Vec<usize> = (first..rows.len())
        .filter(|&i| {
            words.is_empty()
                || words.iter().all(|w| match key.scope {
                    0 => rows[i].iter().any(|cell| has(cell, w)),
                    k => rows[i].get(k - 1).is_some_and(|cell| has(cell, w)),
                })
        })
        .collect();
    if stop.load(atomic::Ordering::Relaxed) {
        return order;
    }
    if let Some((col, ascending)) = key.sort {
        let cell = |i: usize| rows[i].get(col).map_or("", |s| s.trim());
        // (Stable: rows that tie keep the file's order.)
        order.sort_by(|&a, &b| {
            let o = compare(cell(a), cell(b));
            if ascending { o } else { o.reverse() }
        });
        // Empty cells last, whichever way.
        let empty: Vec<usize> = order.iter().copied().filter(|&i| cell(i).is_empty()).collect();
        order.retain(|&i| !cell(i).is_empty());
        order.extend(empty);
    }
    order
}

/// Two cells as people sort them: numbers (a thousands comma, a
/// currency sign or a percent allowed) as numbers, before text; text
/// without regard to case, with runs of digits as numbers.
fn compare(a: &str, b: &str) -> Ordering {
    match (number(a), number(b)) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => natural(a, b),
    }
}

fn number(s: &str) -> Option<f64> {
    let t: String = s.trim().trim_start_matches(['$', '€', '£', '¥']).trim_end_matches('%').chars().filter(|&ch| ch != ',').collect();
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok()
}

/// "file2" before "file10", "a" with "A".
fn natural(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) if p.is_ascii_digit() && q.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut n = String::new();
                    while let Some(d) = it.peek().copied().filter(char::is_ascii_digit) {
                        n.push(d);
                        it.next();
                    }
                    n
                };
                let (m, n) = (take(&mut x), take(&mut y));
                let (m, n) = (m.trim_start_matches('0'), n.trim_start_matches('0'));
                match m.len().cmp(&n.len()).then_with(|| m.cmp(n)) {
                    Ordering::Equal => {}
                    o => return o,
                }
            }
            (Some(p), Some(q)) => {
                match p.to_lowercase().cmp(q.to_lowercase()) {
                    Ordering::Equal => {}
                    o => return o,
                }
                x.next();
                y.next();
            }
        }
    }
}

/// The first row names the columns: every cell of it has text, none is
/// a number, and some column below it has a number or the row is unlike
/// the rows below in being all text.
pub(crate) fn looks_like_header(rows: &[Vec<String>]) -> bool {
    let Some(top) = rows.first() else { return false };
    if rows.len() < 2 || top.iter().any(|c| c.trim().is_empty() || number(c).is_some()) {
        return false;
    }
    let below = &rows[1..rows.len().min(50)];
    let numbers_below = below.iter().any(|r| r.iter().any(|c| number(c).is_some()));
    let blanks_below = below.iter().any(|r| r.iter().any(|c| c.trim().is_empty()));
    numbers_below || blanks_below || top.iter().all(|c| c.chars().next().is_some_and(char::is_uppercase))
}

/// Spreadsheet column letters: A … Z, AA, AB, …
pub fn column_name(mut k: usize) -> String {
    let mut s = String::new();
    loop {
        s.insert(0, (b'A' + (k % 26) as u8) as char);
        if k < 26 {
            return s;
        }
        k = k / 26 - 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter().map(|r| r.iter().map(|s| s.to_string()).collect()).collect()
    }

    #[test]
    fn columns() {
        assert_eq!(column_name(0), "A");
        assert_eq!(column_name(25), "Z");
        assert_eq!(column_name(26), "AA");
        assert_eq!(column_name(27), "AB");
        assert_eq!(column_name(701), "ZZ");
        assert_eq!(column_name(702), "AAA");
    }

    #[test]
    fn headers() {
        assert!(looks_like_header(&sheet(&[&["Name", "Size"], &["a.txt", "12"]])));
        assert!(!looks_like_header(&sheet(&[&["1", "2"], &["3", "4"]])));
        assert!(!looks_like_header(&sheet(&[&["Name", ""], &["a", "1"]])));
        assert!(!looks_like_header(&sheet(&[&["apple", "pear"], &["plum", "fig"]])));
    }

    #[test]
    fn sorts_and_filters() {
        let rows = sheet(&[&["Name", "Size"], &["file10", "1,200"], &["File2", "$90"], &["notes", ""], &["file1", "7%"]]);
        let key = |search: &str, scope, sort| Key { sheet: (0, 0), rows: rows.len(), search: search.into(), scope, sort };
        let stop = AtomicBool::new(false);
        // By name: natural, without case.
        assert_eq!(arrange(&rows, 1, &key("", 0, Some((0, true))), &stop), [4, 2, 1, 3]);
        // By size: as numbers, the empty one last either way.
        assert_eq!(arrange(&rows, 1, &key("", 0, Some((1, true))), &stop), [4, 2, 1, 3]);
        assert_eq!(arrange(&rows, 1, &key("", 0, Some((1, false))), &stop), [1, 2, 4, 3]);
        // Search: every word, in any column or one.
        assert_eq!(arrange(&rows, 1, &key("file", 0, None), &stop), [1, 2, 4]);
        assert_eq!(arrange(&rows, 1, &key("file 1", 0, None), &stop), [1, 4]);
        assert_eq!(arrange(&rows, 1, &key("90", 1, None), &stop), Vec::<usize>::new());
        assert_eq!(arrange(&rows, 1, &key("90", 2, None), &stop), [2]);
    }
}
