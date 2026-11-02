//! Tables: rows (`Para::row`) one after another, each row's cells its
//! text between tabs, so a table saved is tab-separated lines. The Table
//! menu's commands, each one step to undo; what to select after.

use std::ops::Range;

use crate::doc::{Change, Doc, List, Para, Row};

/// What a command did and where the caret goes.
pub type Done = Option<(Change, Range<usize>)>;

/// The rows (paragraph indices) of the table paragraph `i` is in.
pub fn table_of(doc: &Doc, i: usize) -> Option<Range<usize>> {
    let paras = doc.paras();
    paras.get(i)?.row.as_ref()?;
    let mut a = i;
    while a > 0 && paras[a - 1].row.is_some() {
        a -= 1;
    }
    let mut b = i + 1;
    while b < paras.len() && paras[b].row.is_some() {
        b += 1;
    }
    Some(a..b)
}

/// Row `i`'s cells, as bytes of the text.
pub fn cells(doc: &Doc, i: usize) -> Vec<Range<usize>> {
    let r = doc.para_range(i);
    olo_ui::editor::cells(&doc.text()[r.clone()]).into_iter().map(|(s, l)| r.start + s..r.start + s + l).collect()
}

/// Which cell of row `i` byte `at` is in.
fn column_at(doc: &Doc, i: usize, at: usize) -> usize {
    let cells = cells(doc, i);
    cells.iter().position(|c| at <= c.end).unwrap_or(cells.len() - 1)
}

/// A table of `rows` rows (the first a header) and `columns` columns after
/// paragraph `i` (in its place, if it is an empty one), with an empty
/// paragraph after it if it would end the document; the caret in its
/// first cell.
pub fn insert(doc: &mut Doc, i: usize, rows: usize, columns: usize, selection: Range<usize>) -> Done {
    let (rows, columns) = (rows.max(1), columns.max(1));
    let r = doc.para_range(i);
    let p = &doc.paras()[i];
    let in_place = r.is_empty() && p.row.is_none() && p.list == List::None;
    let body = vec!["\t".repeat(columns - 1); rows].join("\n");
    let last = i + 1 == doc.paras().len();
    let tail = if last { "\n" } else { "" };
    let (at, with, start) = if in_place { (r.start, format!("{body}{tail}"), r.start) } else { (r.end, format!("\n{body}{tail}"), r.end + 1) };
    let end = start + body.len() + tail.len();
    let change = doc.replace_paras(std::slice::from_ref(&(at..at)), &with, selection, |_| start..end, |k, p| {
        *p = if k < rows { Para { row: Some(Row { header: k == 0, ..Row::default() }), ..Para::default() } } else { Para::default() };
    })?;
    Some((change, start..start))
}

/// The paragraphs bytes `range` touch made a table's rows (their tabs
/// part the cells), or (`to_table` false) the table the caret is in
/// made text again, its rows tab-separated lines.
pub fn convert(doc: &mut Doc, range: Range<usize>, to_table: bool, selection: Range<usize>) -> Done {
    let rows = if to_table { doc.paras_in(range) } else { table_of(doc, doc.para_index(range.start))? };
    let whole = doc.para_range(rows.start).start..doc.para_range(rows.end - 1).end;
    let change = doc.replace_paras(&[], "", selection.clone(), |_| whole.clone(), |_, p| {
        p.row = to_table.then(Row::default);
        if to_table {
            p.list = List::None;
        }
    })?;
    Some((change, selection))
}

/// The header (first) row of the caret's table on or off.
pub fn toggle_header(doc: &mut Doc, i: usize, selection: Range<usize>) -> Done {
    let t = table_of(doc, i)?;
    let on = !doc.paras()[t.start].row.as_ref()?.header;
    let start = doc.para_range(t.start).start;
    let change = doc.replace_paras(&[], "", selection.clone(), |_| start..start, |_, p| {
        if let Some(r) = &mut p.row {
            r.header = on;
        }
    })?;
    Some((change, selection))
}

/// An empty row above or below row `i`, the caret in its first cell.
pub fn add_row(doc: &mut Doc, i: usize, below: bool, selection: Range<usize>) -> Done {
    let row = doc.paras()[i].row.clone()?;
    let blank = "\t".repeat(cells(doc, i).len() - 1);
    let new = Para { row: Some(Row { header: false, ..row }), ..Para::default() };
    let r = doc.para_range(i);
    if below {
        let start = r.end + 1;
        let change = doc.replace_paras(std::slice::from_ref(&(r.end..r.end)), &format!("\n{blank}"), selection, |_| start..start, |_, p| *p = new.clone())?;
        return Some((change, start..start));
    }
    // In its place, this one after it (as it was).
    let old = doc.paras()[i].clone();
    let with = format!("{blank}\n");
    let end = r.start + with.len();
    let change = doc.replace_paras(std::slice::from_ref(&(r.start..r.start)), &with, selection, |_| r.start..end, |k, p| *p = if k == 0 { new.clone() } else { old.clone() })?;
    Some((change, r.start..r.start))
}

/// Rows `rows` (paragraph indices) taken out, the caret where they were.
pub fn delete_rows(doc: &mut Doc, rows: Range<usize>, selection: Range<usize>) -> Done {
    let n = doc.paras().len();
    let (first, last) = (doc.para_range(rows.start), doc.para_range(rows.end - 1));
    if rows.start > 0 {
        // With the newline before them: the paragraph before keeps its own.
        let start = doc.para_range(rows.start - 1).end;
        let change = doc.replace_paras(std::slice::from_ref(&(start..last.end)), "", selection, |_| start..start, |_, _| {})?;
        let caret = if rows.end < n { start + 1 } else { start };
        Some((change, caret..caret))
    } else if rows.end < n {
        // With the newline after them: the paragraph after keeps its own.
        let next = doc.paras()[rows.end].clone();
        let end = doc.para_range(rows.end).start;
        let change = doc.replace_paras(std::slice::from_ref(&(first.start..end)), "", selection, |_| first.start..first.start, |_, p| *p = next.clone())?;
        Some((change, first.start..first.start))
    } else {
        let change = doc.replace_paras(std::slice::from_ref(&(first.start..last.end)), "", selection, |_| 0..0, |_, p| *p = Para::default())?;
        Some((change, 0..0))
    }
}

/// An empty column before or after the caret's (byte `at` of row `i`),
/// in every row of its table; the caret in its cell.
pub fn add_column(doc: &mut Doc, i: usize, at: usize, after: bool, selection: Range<usize>) -> Done {
    let t = table_of(doc, i)?;
    let k = column_at(doc, i, at);
    let mut ranges = Vec::new();
    let mut caret = 0;
    for j in t.clone() {
        let c = cells(doc, j);
        let end = doc.para_range(j).end;
        let pos = match c.get(k) {
            Some(cell) if after => cell.end,
            Some(cell) => cell.start,
            None => end,
        };
        if j == i {
            // (Shifted by the tabs put in the rows before it.)
            caret = pos + (j - t.start) + usize::from(after);
        }
        ranges.push(pos..pos);
    }
    let n = cells(doc, i).len();
    let whole = |d: &Doc| d.para_range(t.start).start..d.para_range(t.end - 1).end;
    let change = doc.replace_paras(&ranges, "\t", selection, whole, |_, p| {
        if let Some(r) = &mut p.row
            && r.columns.len() == n
        {
            let share = r.columns.iter().sum::<f32>() / n as f32;
            r.columns.insert((k + usize::from(after)).min(n), share);
        }
    })?;
    Some((change, caret..caret))
}

/// The caret's column taken out of every row of its table (unless it is
/// the only one); the caret where it was.
pub fn delete_column(doc: &mut Doc, i: usize, at: usize, selection: Range<usize>) -> Done {
    let t = table_of(doc, i)?;
    let k = column_at(doc, i, at);
    if t.clone().all(|j| cells(doc, j).len() <= 1) {
        return None;
    }
    let mut ranges = Vec::new();
    for j in t.clone() {
        let c = cells(doc, j);
        if c.len() <= 1 || k >= c.len() {
            continue;
        }
        // With the tab before it (after it, for the first).
        ranges.push(if k > 0 { c[k - 1].end..c[k].end } else { c[0].start..c[1].start });
    }
    let n = cells(doc, i).len();
    let whole = |d: &Doc| d.para_range(t.start).start..d.para_range(t.end - 1).end;
    let change = doc.replace_paras(&ranges, "", selection, whole, |_, p| {
        if let Some(r) = &mut p.row
            && r.columns.len() == n
        {
            r.columns.remove(k);
        }
    })?;
    // In its row, at the start of the cell now in its place.
    let c = cells(doc, i);
    let caret = c[k.min(c.len() - 1)].start;
    Some((change, caret..caret))
}

/// The columns of row `i`'s table these widths (shares of the width), a
/// rule dragged in the editor.
pub fn set_columns(doc: &mut Doc, i: usize, columns: Vec<f32>, selection: Range<usize>) -> Done {
    let t = table_of(doc, i)?;
    let whole = doc.para_range(t.start).start..doc.para_range(t.end - 1).end;
    let change = doc.replace_paras(&[], "", selection.clone(), |_| whole.clone(), |_, p| {
        if let Some(r) = &mut p.row {
            r.columns = columns.clone();
        }
    })?;
    Some((change, selection))
}

/// Every column of the caret's table as wide as the others.
pub fn even_columns(doc: &mut Doc, i: usize, selection: Range<usize>) -> Done {
    let t = table_of(doc, i)?;
    let whole = doc.para_range(t.start).start..doc.para_range(t.end - 1).end;
    let change = doc.replace_paras(&[], "", selection.clone(), |_| whole.clone(), |_, p| {
        if let Some(r) = &mut p.row {
            r.columns.clear();
        }
    })?;
    Some((change, selection))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(d: &Doc) -> Vec<(String, Option<bool>)> {
        d.text().split('\n').zip(d.paras()).map(|(t, p)| (t.to_owned(), p.row.as_ref().map(|r| r.header))).collect()
    }

    #[test]
    fn insert_add_and_delete() {
        let mut d = Doc::new("Before".into(), None);
        let (_, sel) = insert(&mut d, 0, 2, 3, 0..0).unwrap();
        // After the paragraph, a header row and a row, and one to go on in.
        assert_eq!(rows(&d), [("Before".into(), None), ("\t\t".into(), Some(true)), ("\t\t".into(), Some(false)), ("".into(), None)]);
        assert_eq!(sel, 7..7);
        let (_, sel) = add_row(&mut d, 2, true, 0..0).unwrap();
        assert_eq!(d.paras()[3].row.as_ref().map(|r| r.header), Some(false));
        assert_eq!(sel, d.para_range(3).start..d.para_range(3).start);
        // A column after the second, in every row.
        let at = cells(&d, 1)[1].start;
        add_column(&mut d, 1, at, true, 0..0).unwrap();
        assert!(d.text().split('\n').skip(1).take(3).all(|l| l == "\t\t\t"));
        // Rows and the table go; the paragraph around them stays as it was.
        delete_rows(&mut d, 2..4, 0..0).unwrap();
        assert_eq!(rows(&d), [("Before".into(), None), ("\t\t\t".into(), Some(true)), ("".into(), None)]);
        let at = d.para_range(1).start;
        delete_column(&mut d, 1, at, 0..0).unwrap();
        assert_eq!(d.text(), "Before\n\t\t\n");
        delete_rows(&mut d, 1..2, 0..0).unwrap();
        assert_eq!(rows(&d), [("Before".into(), None), ("".into(), None)]);
        // Undone, it is all back.
        while d.undo(0..0).is_some() {}
        assert_eq!(rows(&d), [("Before".into(), None)]);
    }

    #[test]
    fn convert_and_header() {
        let mut d = Doc::new("a\tb\nc\td\nafter".into(), None);
        convert(&mut d, 0..7, true, 0..0).unwrap();
        assert_eq!(table_of(&d, 1), Some(0..2));
        toggle_header(&mut d, 1, 0..0).unwrap();
        assert!(d.paras()[0].row.as_ref().unwrap().header);
        convert(&mut d, 2..2, false, 0..0).unwrap();
        assert!(d.paras().iter().all(|p| p.row.is_none()));
    }
}
