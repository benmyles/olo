//! Tracker's other windows: Get Info, Find, and Tracker Status (the file
//! operations running).

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use olo_kit_base::metrics as m;
use olo_ui::{
    Menu,
    gpui::{
        self, Bounds, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseMoveEvent, ParentElement as _, Pixels, Point, Render, SharedString,
        Styled as _, Subscription, Task, WeakEntity, Window, canvas, div, point, px, size,
    },
    icons, menu,
    style::{self, Align},
    theme,
    widgets::{
        Button, CheckBox, MenuField, StatusBar, StringView, TextControl, TextField, TextFieldEvent,
        list::{Column, ColumnListView, ListModel},
        text_field,
    },
};

use crate::{
    FIND_FOLLOWS, Tracker, find,
    folder::name_cell,
    fs::{self, Entry},
    items, ops,
};

// ---- Get Info ---------------------------------------------------------------

const INFO_ICON: f32 = 64.0;
/// Width of the right-aligned key column.
const INFO_KEY_WIDTH: f32 = 84.0;

/// Rows in a Get Info window, for sizing it to fit.
pub fn info_rows(e: &Entry) -> usize {
    5 + usize::from(e.is_link)
}

/// Height of a Get Info window's content for `rows` rows.
pub fn info_height(rows: usize) -> f32 {
    2.0 * m::WINDOW_INSET + INFO_ICON + 2.0 * m::SPACING + 2.0 + rows as f32 * (m::LABEL_ROW + m::TIGHT) - m::TIGHT
}

pub struct InfoWin {
    pub entry: Entry,
    /// A folder's size and how many items are in it, once counted.
    pub size: Option<(u64, u64)>,
    press: Option<Point<Pixels>>,
}

impl InfoWin {
    pub fn new(entry: Entry) -> InfoWin {
        InfoWin { entry, size: None, press: None }
    }

    fn rows(&self) -> Vec<(&'static str, String)> {
        let e = &self.entry;
        let size_text = if e.is_dir {
            match self.size {
                Some((b, 1)) => format!("{} (1 item)", fs::format_size(b)),
                Some((b, n)) => format!("{} ({n} items)", fs::format_size(b)),
                None => "Calculating…".into(),
            }
        } else if e.size < 1024 || e.downloading.is_some() {
            e.size_text()
        } else {
            format!("{} ({} bytes)", fs::format_size(e.size), style::thousands(e.size as usize))
        };
        let location = e.path.parent().map(fs::display_path).unwrap_or_default();
        let created = std::fs::metadata(&e.path).ok().and_then(|md| md.created().ok());
        let mut rows = vec![
            ("Size:", size_text),
            ("Where:", location),
            ("Created:", fs::format_time(created)),
            ("Modified:", fs::format_time(e.modified)),
            ("Permissions:", fs::permissions_string(e.mode, e.is_dir)),
        ];
        if e.is_link
            && let Ok(t) = std::fs::read_link(&e.path)
        {
            rows.push(("Link to:", t.display().to_string()));
        }
        rows
    }
}

impl Render for InfoWin {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (e, rows) = (self.entry.clone(), self.rows());
        let me = cx.entity();
        let drag_me = me.clone();
        let content = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let area = Bounds::from_corners(point(b.left() + px(m::WINDOW_INSET), b.top() + px(m::WINDOW_INSET)), point(b.right() - px(m::WINDOW_INSET), b.bottom() - px(m::WINDOW_INSET)));
                // Header: the icon, with the name and kind centred beside it.
                let icon = Bounds { origin: area.origin, size: size(px(INFO_ICON), px(INFO_ICON)) };
                icons::paint(window, icon, items::icon_of(&e));
                let tx = icon.right() + px(m::SPACING);
                let cy = icon.center().y;
                let half = px((m::LABEL_ROW + m::TIGHT) / 2.0);
                let at = style::label(window, cx, tx, cy - half, Align::Left, e.label().to_owned(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), style::c(p.panel_text));
                olo_ui::agent::record(window, cx, olo_ui::agent::Node::label(e.label().to_owned(), at));
                style::label(window, cx, tx, cy + half, Align::Left, e.kind_text(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                // An etched rule, then the rows on an exact grid.
                let y = style::snap_coord(window, icon.bottom() + px(m::SPACING));
                style::hline(window, area.left(), area.right(), y, style::shade(&p, p.panel_background, 1.2));
                style::hline(window, area.left(), area.right(), y + style::hairline(window), style::c(p.shine));
                let mut y = y + px(2.0 + m::SPACING);
                for (k, v) in &rows {
                    let cy = y + px(m::LABEL_ROW / 2.0);
                    let kx = area.left() + px(INFO_KEY_WIDTH);
                    style::label(window, cx, kx, cy, Align::Right, *k, style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                    let vx = kx + px(m::LABEL_GAP);
                    let at =
                        style::label_fit(window, cx, vx, cy, Align::Left, v.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::c(p.panel_text), (area.right() - vx).max(px(0.0)));
                    olo_ui::agent::record(window, cx, olo_ui::agent::Node::label(format!("{k} {v}"), at));
                    y += px(m::LABEL_ROW + m::TIGHT);
                }
                // The icon is the item: it drags out, as from a folder
                // window.
                let mv = drag_me.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                    if phase != gpui::DispatchPhase::Bubble || ev.pressed_button != Some(MouseButton::Left) {
                        return;
                    }
                    let Some(at) = mv.read(cx).press else { return };
                    if (ev.position - at).magnitude() < 3.0 {
                        return;
                    }
                    let e = mv.update(cx, |i, _| {
                        i.press = None;
                        i.entry.clone()
                    });
                    let hotspot = point((at.x - icon.left()).clamp(px(0.0), px(INFO_ICON)), (at.y - icon.top()).clamp(px(0.0), px(INFO_ICON)));
                    let d = items::Drag { paths: vec![e.path.clone()], icon: items::icon_of(&e), size: INFO_ICON, hotspot, ask: false, name: e.name.clone() };
                    items::start(window, cx, mv.entity_id(), d);
                });
            },
        )
        .size_full();
        let grab = div()
            .absolute()
            .left(px(m::WINDOW_INSET))
            .top(px(m::WINDOW_INSET))
            .w(px(INFO_ICON))
            .h(px(INFO_ICON))
            .on_mouse_down(MouseButton::Left, move |ev, _, cx| me.update(cx, |i, _| i.press = Some(ev.position)))
            .on_mouse_up(MouseButton::Left, {
                let me = cx.entity();
                move |_, _, cx| me.update(cx, |i, _| i.press = None)
            });
        div().size_full().relative().child(content).child(grab)
    }
}

// ---- Find -------------------------------------------------------------------

pub struct FindWin {
    pub app: WeakEntity<Tracker>,
    query: Entity<TextField>,
    root: usize,
    /// Index into `find::Mode::LABELS`.
    mode: usize,
    everything: bool,
    pub results: Vec<find::Hit>,
    /// The mode the current results were found with.
    shown_mode: find::Mode,
    pub list: Entity<ListModel>,
    pub running: bool,
    pub generation: u64,
    pub cancel: Arc<AtomicBool>,
    progress: Arc<find::Progress>,
    /// The last search was stopped before it finished.
    stopped: bool,
    started: Instant,
    pub took: Duration,
    /// The results' folders to follow, as of so many results.
    follow: (usize, Vec<PathBuf>),
    poll: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl FindWin {
    pub fn new(app: WeakEntity<Tracker>, window: &mut Window, cx: &mut Context<Self>) -> FindWin {
        let query = text_field(cx, |f| f.hint("Name or text").name("Find"));
        query.read(cx).focus(window);
        let list = ListModel::new(cx, false);
        let subs = vec![cx.subscribe(&query, |_, _, _: &TextFieldEvent, cx| cx.notify()), cx.observe(&list, |_, _, cx| cx.notify())];
        FindWin {
            app,
            query,
            root: 0,
            mode: 0,
            everything: false,
            results: Vec::new(),
            shown_mode: find::Mode::Names,
            list,
            running: false,
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::default(),
            stopped: false,
            started: Instant::now(),
            took: Duration::ZERO,
            follow: (0, Vec::new()),
            poll: None,
            _subs: subs,
        }
    }

    pub fn followed(&mut self) -> &[PathBuf] {
        if self.follow.0 != self.results.len() {
            let dirs: BTreeSet<PathBuf> = self.results.iter().filter_map(|h| h.entry.path.parent().map(Path::to_owned)).collect();
            self.follow = (self.results.len(), dirs.into_iter().take(FIND_FOLLOWS).collect());
        }
        &self.follow.1
    }

    /// The one result selected.
    pub fn selected(&self, cx: &gpui::App) -> Option<Entry> {
        self.list.read(cx).state.single().and_then(|i| self.results.get(i)).map(|h| h.entry.clone())
    }

    /// Move the selection by `d` results (Quick Look's arrow keys).
    pub fn step_selection(&mut self, d: isize, cx: &mut Context<Self>) -> Option<Entry> {
        let n = self.results.len();
        if n == 0 {
            return None;
        }
        let k = self.list.update(cx, |l, _| {
            let k = l.state.single().map_or(0, |k| (k as isize + d).clamp(0, n as isize - 1) as usize);
            l.state.select(k);
            l.reveal(k);
            k
        });
        self.results.get(k).map(|h| h.entry.clone())
    }

    /// Find (or Stop, while it runs).
    fn go(&mut self, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        if self.running {
            self.running = false;
            self.stopped = true;
            self.generation += 1;
            cx.notify();
            return;
        }
        let text = self.query.read(cx).text().to_owned();
        if text.trim().is_empty() {
            return;
        }
        self.stopped = false;
        self.started = Instant::now();
        self.generation += 1;
        self.running = true;
        self.results.clear();
        self.list.update(cx, |l, _| l.state = Default::default());
        self.cancel = Arc::new(AtomicBool::new(false));
        self.progress = Arc::default();
        self.shown_mode = find::Mode::from_index(self.mode);
        let query = find::Query { root: if self.root == 0 { fs::home() } else { PathBuf::from("/") }, text, mode: self.shown_mode, everything: self.everything };
        let (proxy, generation, win) = (olo_ui::proxy(cx), self.generation, cx.entity_id());
        find::spawn(query, self.cancel.clone(), self.progress.clone(), move |hits, done| {
            proxy.send(crate::Msg::Found(win, generation, hits, done));
        });
        // Progress lives in the workers' counters: drawn again a few times
        // a second while they run.
        self.poll = Some(cx.spawn(async move |me, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(100)).await;
                let on = me.update(cx, |f, cx| {
                    cx.notify();
                    f.running
                });
                if !matches!(on, Ok(true)) {
                    return;
                }
            }
        }));
        cx.notify();
    }

    /// Results came: `done` once the search is over.
    pub fn found(&mut self, generation: u64, hits: Vec<find::Hit>, done: bool, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        self.results.extend(hits);
        if done {
            self.running = false;
            self.took = self.started.elapsed();
            self.poll = None;
        }
        cx.notify();
    }

    /// Items changed on disk (`now` says how each is: None, gone).
    pub fn changed(&mut self, now: impl Fn(&Path) -> Option<Option<Entry>>, cx: &mut Context<Self>) {
        let before = self.results.len();
        let mut changed = false;
        self.results.retain_mut(|h| match now(&h.entry.path) {
            Some(None) => false,
            Some(Some(e)) => {
                changed = true;
                h.entry = e;
                true
            }
            None => true,
        });
        if self.results.len() != before {
            self.list.update(cx, |l, _| l.state.selected.clear());
        }
        if changed || self.results.len() != before {
            cx.notify();
        }
    }

    fn command(&self, cmd: &str, cx: &mut Context<Self>) {
        let (app, key, cmd) = (self.app.clone(), cx.entity_id(), cmd.to_owned());
        cx.defer(move |cx| {
            if let Some(app) = app.upgrade() {
                app.update(cx, |t, cx| t.find_command(key, &cmd, cx));
            }
        });
    }

    fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        // Escape stops, as the Stop button does.
        if k.key == "escape" && self.running {
            self.go(cx);
            cx.stop_propagation();
            return;
        }
        // Space is Quick Look on the selected result, as in a folder
        // window, unless it goes into the Find field.
        if k.key == "space" && !self.list.read(cx).typing() && self.selected(cx).is_some() && !self.query.read(cx).is_focused(window) {
            self.command("quick-look", cx);
            cx.stop_propagation();
            return;
        }
        // The menu's keys, as there is no menu bar to take them (the Find
        // field keeps its own).
        for c in ["parent", "open", "quick-look"] {
            if olo_kit_base::keys::shortcut(c).is_some_and(|s| olo_ui::keys::pressed(&s, k)) {
                self.command(c, cx);
                cx.stop_propagation();
                return;
            }
        }
    }
}

impl Render for FindWin {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let contents = self.shown_mode != find::Mode::Names;
        let mut cols = vec![Column::new("Name", 200.0), Column::new("Location", 170.0)];
        if contents {
            cols.push(Column::new("Line", 44.0).right());
            cols.push(Column::new("Match", 300.0));
        } else {
            cols.push(Column::new("Size", 70.0).right());
            cols.push(Column::new("Modified", 170.0));
        }
        let names: Vec<SharedString> = self.results.iter().map(|h| SharedString::from(h.entry.name.clone())).collect();
        let (cell_me, open_me, context_me, go_me, mode_me, root_me, hidden_me) = (me.clone(), me.clone(), me.clone(), me.clone(), me.clone(), me.clone(), me.clone());
        let list = ColumnListView::new("find-results", &self.list, cols, self.results.len(), move |cell, window, cx| {
            let Some(h) = cell_me.read(cx).results.get(cell.row).cloned() else { return };
            let e = &h.entry;
            match (cell.column, contents) {
                (0, _) => name_cell(cell, window, cx, e, true),
                (1, _) => cell.text(window, cx, e.path.parent().map(fs::display_path).unwrap_or_default()),
                (2, true) => cell.text(window, cx, h.line.as_ref().map(|(n, _)| n.to_string()).unwrap_or_default()),
                (_, true) => cell.text(window, cx, h.line.as_ref().map(|(_, t)| t.clone()).unwrap_or_default()),
                (2, false) => cell.text(window, cx, e.size_text()),
                _ => cell.text(window, cx, fs::format_time(e.modified)),
            }
        })
        .type_select(move |i| names.get(i).cloned().unwrap_or_default())
        .on_open(move |_, _, cx| open_me.update(cx, |f, cx| f.command("open", cx)))
        .on_context(move |_, at, window, cx| {
            let me = context_me.clone();
            let menu = Menu::new("").item("Open", "open").key("o").item("Open Parent", "parent").key("up").item("Quick Look", "quick-look").key("y");
            menu::context_menu(window, cx, menu, at, move |cmd, _, cx| me.update(cx, |f, cx| f.command(cmd, cx)));
        });
        let status = if self.running {
            let scanned = self.progress.scanned.load(Ordering::Relaxed);
            let folder = self.progress.folder.lock().map(|f| fs::display_path(&f)).unwrap_or_default();
            format!("{} found · {scanned} items searched · {folder}", self.results.len())
        } else if self.generation > 0 {
            let found = match self.results.len() {
                1 => "1 item found".to_owned(),
                n => format!("{n} items found"),
            };
            if self.stopped { format!("Stopped · {found}") } else { format!("{found} in {:.1} s", self.took.as_secs_f32()) }
        } else {
            String::new()
        };
        let _ = window;
        let modes: Vec<SharedString> = find::Mode::LABELS.iter().map(|l| SharedString::from(*l)).collect();
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .on_key_down(cx.listener(Self::key_down))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING))
                    .items_center()
                    .child(div().flex_1().child(TextControl::new(self.query.clone()).label("Find:")))
                    .child(Button::new(if self.running { "Stop" } else { "Find" }).default(true).min_width(80.0).on_click(move |_, cx| go_me.update(cx, |f, cx| f.go(cx)))),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING))
                    .items_center()
                    .child(MenuField::new("find-mode", modes, self.mode).label("Match:").on_change(move |i, _, cx| {
                        mode_me.update(cx, |f, cx| {
                            f.mode = i;
                            cx.notify();
                        })
                    }))
                    .child(MenuField::new("find-root", vec!["Home".into(), "Entire disk".into()], self.root).label("In:").on_change(move |i, _, cx| {
                        root_me.update(cx, |f, cx| {
                            f.root = i;
                            cx.notify();
                        })
                    }))
                    .child(CheckBox::new("Hidden and ignored files", self.everything).on_toggle(move |on, _, cx| {
                        hidden_me.update(cx, |f, cx| {
                            f.everything = on;
                            cx.notify();
                        })
                    })),
            )
            .child(div().flex_1().min_h_0().flex().flex_col().child(list))
            .child(div().h(px(m::CONTROL_HEIGHT)).flex().items_center().child(StringView::new(status).dim(true)))
    }
}

// ---- Tracker Status ---------------------------------------------------------

/// Tracker Status's width.
pub const STATUS_WIDTH: f32 = 420.0;
/// A job's row: its bar (label above, bar, the detail below) and Stop.
const STATUS_ROW: f32 = m::LABEL_ROW + 10.0 + m::TIGHT + m::LABEL_ROW;

/// The window's height for `jobs` rows.
pub fn status_height(jobs: usize) -> f32 {
    2.0 * m::WINDOW_INSET + jobs.max(1) as f32 * STATUS_ROW + jobs.saturating_sub(1) as f32 * (2.0 * m::SPACING + 2.0)
}

pub struct StatusWin {
    pub app: WeakEntity<Tracker>,
    poll: Option<Task<()>>,
}

impl StatusWin {
    pub fn new(app: WeakEntity<Tracker>, cx: &mut Context<Self>) -> StatusWin {
        // Progress lives in the workers' counters: drawn again a few times
        // a second while they run.
        let poll = cx.spawn(async move |me, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(100)).await;
                if me.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
        });
        StatusWin { app, poll: Some(poll) }
    }
}

impl Render for StatusWin {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = &self.poll;
        let Some(app) = self.app.upgrade() else { return div() };
        let jobs: Vec<(u64, String, String, String, f32)> = app.read(cx).jobs.iter().map(|(j, _)| (j.id, j.title.clone(), ops::amount(j), ops::detail(j), j.progress.fraction())).collect();
        let mut col = div().size_full().p(px(m::WINDOW_INSET)).flex().flex_col();
        for (k, (id, title, amount, detail, fraction)) in jobs.into_iter().enumerate() {
            if k > 0 {
                col = col.child(div().h(px(m::SPACING))).child(olo_ui::widgets::separator()).child(div().h(px(m::SPACING)));
            }
            let stop = app.clone();
            col = col.child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING))
                    .child(div().flex_1().flex().flex_col().gap(px(m::TIGHT)).child(StatusBar::new(fraction).label(title).trailing(amount)).child(StringView::new(detail).dim(true)))
                    .child(
                        div()
                            .pt(px(m::LABEL_ROW + m::TIGHT - (m::CONTROL_HEIGHT - 14.0) / 2.0))
                            .child(Button::new("Stop").min_width(64.0).on_click(move |_, cx| stop.update(cx, |t, _| t.stop_job(id)))),
                    ),
            );
        }
        col
    }
}
