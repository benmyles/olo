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
    },
};

/// A sheet's rows, shared with the worker that filters and sorts them.
pub type Rows = Arc<Vec<Vec<String>>>;

/// Sheets this big (cells) are filtered and sorted on a worker.
pub const ON_A_WORKER: usize = 40_000;

/// What the rows shown depend on: the sheet (a document's, and which),
/// how many rows it has, the search, where it looks (0: every column;
/// else the column before it) and the sort (a column, ascending).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Key {
    pub sheet: (u64, usize),
    pub rows: usize,
    pub search: String,
    pub scope: usize,
    pub sort: Option<(usize, bool)>,
}

/// The rows (indices) with `key.search` in them, in `key.sort`'s order;
/// what there is so far when `stop` is set.
pub fn arrange(rows: &[Vec<String>], first: usize, key: &Key, stop: &AtomicBool) -> Vec<usize> {
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
        (None, None) => olo_kit_base::files::natural_cmp(a, b),
    }
}

/// Whether a cell holds a number (as sorting takes them).
pub fn is_number(s: &str) -> bool {
    number(s).is_some()
}

fn number(s: &str) -> Option<f64> {
    let t: String = s.trim().trim_start_matches(['$', '€', '£', '¥']).trim_end_matches('%').chars().filter(|&ch| ch != ',').collect();
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok()
}

/// The first row names the columns: every cell of it has text, none is
/// a number, and some column below it has a number or the row is unlike
/// the rows below in being all text.
pub fn looks_like_header(rows: &[Vec<String>]) -> bool {
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
