//! A sheet as the egui view shows it: every column (scrolled sideways
//! when they don't all fit), the first row as the columns' names when it
//! looks like one; a bar with a search, in every column or one, and how
//! many rows show; sorting by a column's header. A big sheet is searched
//! and sorted on a worker, the rows shown until then staying put.

use std::sync::{
    Arc,
    atomic::{self, AtomicBool},
};

use olo_kit_base::metrics as m;
use olo_ui::{
    gpui::{self, AppContext as _, Context, Entity, Focusable as _, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window, canvas, div, px},
    review,
    style::{self, Align},
    theme,
    widgets::{Column, ColumnListView, ListModel, MenuField, TextControl, TextField, TextFieldEvent, text_field},
};

use crate::table::{Key, ON_A_WORKER, Rows, arrange, column_name, looks_like_header};

/// The bar over the table (its rule inside it; on the 2-point grid).
const BAR: f32 = m::CONTROL_HEIGHT + 2.0 * m::TIGHT + 2.0;
/// The bar's left inset: the search's text under the menu bar's titles.
const BAR_LEFT: f32 = m::WINDOW_INSET - m::TEXT_INSET;

pub struct TableView {
    rows: Option<Rows>,
    sheet: (u64, usize),
    search: Entity<TextField>,
    /// 0: every column; else the column before it.
    scope: usize,
    list: Entity<ListModel>,
    /// The rows shown (indices into the sheet), in order, and what for.
    shown: Option<(Key, Arc<Vec<usize>>)>,
    /// A worker making them, for that.
    making: Option<(Key, Task<()>, Arc<AtomicBool>)>,
    _search: Subscription,
}

impl TableView {
    pub fn new(cx: &mut Context<Self>) -> TableView {
        let search = text_field(cx, |f| f.name("Search").hint("Search"));
        let sub = cx.subscribe(&search, |_: &mut TableView, _, e: &TextFieldEvent, cx| {
            if matches!(e, TextFieldEvent::Changed) {
                cx.notify();
            }
        });
        TableView { rows: None, sheet: (0, 0), search, scope: 0, list: ListModel::new(cx, true), shown: None, making: None, _search: sub }
    }

    /// Start again (another file, or another sheet of it).
    pub fn reset(&mut self) {
        if let Some((_, _, cancel)) = self.making.take() {
            cancel.store(true, atomic::Ordering::Relaxed);
        }
        self.rows = None;
        self.shown = None;
        self.scope = 0;
    }

    /// Show `rows`, sheet `sheet` (the document's generation and which).
    pub fn show(&mut self, rows: Rows, sheet: (u64, usize), cx: &mut Context<Self>) {
        if self.sheet != sheet || self.rows.as_ref().is_none_or(|r| !Arc::ptr_eq(r, &rows)) {
            self.sheet = sheet;
            self.rows = Some(rows);
            cx.notify();
        }
    }

    /// The rows to show for what's asked now: here, or (a big sheet) from
    /// a worker, the last ones showing meanwhile.
    fn arrange(&mut self, rows: &Rows, first: usize, ncols: usize, cx: &mut Context<Self>) {
        let sort = self.list.read(cx).state.sort;
        let key = Key { sheet: self.sheet, rows: rows.len(), search: self.search.read(cx).text().trim().to_lowercase(), scope: self.scope, sort };
        if self.making.as_ref().is_some_and(|(k, ..)| *k != key) {
            let (_, _, cancel) = self.making.take().expect("checked");
            cancel.store(true, atomic::Ordering::Relaxed);
        }
        let stale = self.shown.as_ref().is_none_or(|(k, _)| *k != key);
        if !stale || self.making.is_some() {
            return;
        }
        if rows.len() * ncols <= ON_A_WORKER {
            self.shown = Some((key.clone(), Arc::new(arrange(rows, first, &key, &AtomicBool::new(false)))));
            self.list.update(cx, |l, _| l.state.selected.clear());
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let (rows, k, stop) = (rows.clone(), key.clone(), cancel.clone());
        let task = cx.spawn(async move |view, cx| {
            let order = cx.background_spawn(async move { arrange(&rows, first, &k, &stop) }).await;
            let _ = view.update(cx, |v, cx| {
                if let Some((k, _, stop)) = v.making.take() {
                    if stop.load(atomic::Ordering::Relaxed) {
                        return;
                    }
                    v.shown = Some((k, Arc::new(order)));
                    v.list.update(cx, |l, _| l.state.selected.clear());
                    cx.notify();
                }
            });
        });
        self.making = Some((key, task, cancel));
    }
}

impl Render for TableView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(rows) = self.rows.clone() else { return div().into_any_element() };
        let ncols = rows.iter().map(Vec::len).max().unwrap_or(0).clamp(1, 200);
        let header = looks_like_header(&rows);
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
        self.arrange(&rows, first, ncols, cx);
        let total = rows.len().saturating_sub(first);
        let order: Arc<Vec<usize>> = self.shown.as_ref().map(|(_, o)| o.clone()).unwrap_or_else(|| Arc::new((first..rows.len()).collect()));
        let count = if self.making.is_some() {
            "Searching…".to_owned()
        } else if order.len() != total {
            format!("{} of {total} rows", order.len())
        } else if total == 1 {
            "1 row".to_owned()
        } else {
            format!("{total} rows")
        };

        // Each column as wide as what it holds, within reason; one of
        // numbers lined up on the right.
        let cols: Vec<Column> = (0..ncols)
            .map(|k| {
                let sample = || rows.iter().skip(first).take(200).filter_map(|r| r.get(k)).filter(|s| !s.trim().is_empty());
                let longest = sample().map(|s| s.chars().count()).chain([titles[k].chars().count() + 2]).max().unwrap_or(4);
                let col = Column::new(titles[k].clone(), (longest as f32 * 7.0 + 14.0).clamp(48.0, 320.0));
                let (cells, numbers) = sample().fold((0, 0), |(n, k), s| (n + 1, k + usize::from(crate::table::is_number(s))));
                if cells > 0 && numbers * 10 >= cells * 9 { col.right() } else { col }
            })
            .collect();
        let p = theme::palette(cx);
        let width = f32::from(window.viewport_size().width);
        let count_w = f32::from(style::text_width(window, count.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL))).ceil();
        let inner = width - BAR_LEFT - m::SPACING;
        let scope_w = 150.0f32.min((inner - count_w - 3.0 * m::SPACING) / 2.0).max(90.0);
        let search_w = (inner - scope_w - count_w - 3.0 * m::SPACING).clamp(80.0, 280.0);
        let mut options: Vec<SharedString> = vec!["All Columns".into()];
        options.extend(titles.iter().map(|t| SharedString::from(t.clone())));
        let me = cx.entity();
        let scope = MenuField::new(format!("ql-table-scope-{}-{}", self.sheet.0, self.sheet.1), options, self.scope).width(scope_w).on_change(move |k, _, cx| {
            me.update(cx, |v, cx| {
                v.scope = k;
                cx.notify();
            })
        });
        let bar_ground = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                window.paint_quad(gpui::fill(b, style::c(p.panel_background)));
                style::hline_above(window, b.left(), b.right(), b.bottom(), style::shade(&p, p.panel_background, 1.2));
            },
        )
        .absolute()
        .size_full();
        let label = count.clone();
        let count_el = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let at = style::label(window, cx, b.right(), b.center().y, Align::Right, label.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                olo_ui::agent::record(window, cx, olo_ui::agent::Node::label(label.clone(), at));
            },
        )
        .flex_1()
        .h(px(m::CONTROL_HEIGHT));
        let field = self.search.clone();
        let mut row = div().flex().items_center().gap(px(m::SPACING)).child(TextControl::new(self.search.clone()).width(search_w));
        if ncols > 1 {
            row = row.child(scope);
        }
        let bar = div()
            .relative()
            .h(px(BAR))
            .w_full()
            .pl(px(BAR_LEFT))
            .pr(px(m::SPACING))
            .pt(px(m::TIGHT))
            .child(bar_ground)
            .child(row.child(count_el))
            // Escape clears the search, then lets it go.
            .on_key_down(move |e: &KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" && field.read(cx).focus_handle(cx).is_focused(window) {
                    let empty = field.read(cx).text().is_empty();
                    field.update(cx, |f, cx| f.set_text("", cx));
                    if empty {
                        window.blur();
                    }
                    cx.stop_propagation();
                }
            });
        let (cells_rows, cells_order) = (rows.clone(), order.clone());
        let empty = if self.search.read(cx).text().trim().is_empty() { "No rows." } else { "No rows match." };
        let me = cx.entity();
        let list = ColumnListView::new(format!("ql-table-{}-{}", self.sheet.0, self.sheet.1), &self.list, cols, order.len(), move |cell, window, cx| {
            if let Some(s) = cells_order.get(cell.row).and_then(|&i| cells_rows.get(i)).and_then(|r| r.get(cell.column)) {
                cell.text(window, cx, s.clone());
            }
        })
        .empty_text(empty)
        .on_sort(move |_, cx| me.update(cx, |_, cx| cx.notify()));
        // The cells line up with each other, not with the bar's search.
        let grid = canvas(|_, _, _| {}, |b, _, window, _| review::own_grid(window, b)).absolute().size_full();
        let _ = p;
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(bar)
            .child(div().relative().flex_1().min_h_0().child(grid).child(list))
            .into_any_element()
    }
}
