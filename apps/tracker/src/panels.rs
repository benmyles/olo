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

use olo_kit::{
    Bounds, Child, ClickEvent, Cx, Element, FontWeight, InteractiveElement as _, IntoElement as _, KeyDownEvent, Menu, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    Pixels, Point, SharedString, Styled as _, View, WindowOptions, canvas, div, icons, menu, point, px, size,
    style::{self, Align},
    theme,
    widgets::{
        Button, CheckBox, MenuField, StatusBar, StringView, TextControl, TextField, TextFieldEvent,
        list::{Column, ColumnListView, ListModel},
        text_field,
    },
};
use olo_kit_base::metrics as m;

use crate::{
    FIND_FOLLOWS, ToHub, WinInfo, find,
    folder::{self, name_cell},
    fs::{self, Entry},
    items, ops, to_hub, watch,
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

/// What a Get Info window is told.
pub enum InfoMsg {
    /// Its folder's size and how many items are in it, counted.
    DirSize(u64, u64),
    /// Changes seen on disk.
    Changed(Arc<Vec<watch::Change>>),
    /// A drag out of it is over.
    DragOver,
}

pub struct InfoWin {
    /// The app's number for this window.
    id: u64,
    pub entry: Entry,
    /// A folder's size and how many items are in it, once counted.
    pub size: Option<(u64, u64)>,
    press: Option<Point<Pixels>>,
    told: bool,
}

impl InfoWin {
    pub fn new(id: u64, entry: Entry, cx: &mut Cx<Self>) -> InfoWin {
        // A folder: what's in it, counted on a worker.
        if entry.is_dir {
            let (to, path) = (cx.messenger(), entry.path.clone());
            std::thread::spawn(move || {
                let mut limit = 200_000;
                let (b, f) = fs::dir_size(&path, &mut limit);
                to.send(InfoMsg::DirSize(b, f));
            });
        }
        InfoWin { id, entry, size: None, press: None, told: false }
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

impl View for InfoWin {
    type Message = InfoMsg;

    fn message(&mut self, message: InfoMsg, cx: &mut Cx<Self>) {
        match message {
            InfoMsg::DirSize(b, n) => self.size = Some((b, n)),
            InfoMsg::Changed(changes) => match crate::now_on_disk(&changes, &self.entry.path) {
                // Gone: its Info goes too, as on BeOS.
                Some(None) => return cx.close_window(),
                Some(Some(e)) => self.entry = e,
                None => return,
            },
            InfoMsg::DragOver => self.press = None,
        }
        cx.invalidate();
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        to_hub(ToHub::Closed(self.id));
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        // An item's Info follows it: its folder is watched.
        if !self.told {
            self.told = true;
            crate::publish(
                self.id,
                WinInfo {
                    path: self.entry.path.clone(),
                    surface: cx.surface_id(),
                    watched: self.entry.path.parent().map(Path::to_owned).into_iter().collect(),
                    selected: None,
                    folder: None,
                    find: false,
                },
            );
        }
        let (e, rows) = (self.entry.clone(), self.rows());
        let content = canvas(move |g, b| {
            let p = theme::palette();
            let area = Bounds::from_corners(point(b.left() + px(m::WINDOW_INSET), b.top() + px(m::WINDOW_INSET)), point(b.right() - px(m::WINDOW_INSET), b.bottom() - px(m::WINDOW_INSET)));
            // Header: the icon, with the name and kind centred beside it.
            let icon = Bounds { origin: area.origin, size: size(px(INFO_ICON), px(INFO_ICON)) };
            icons::paint(g, icon, items::icon_of(&e));
            let tx = icon.right() + px(m::SPACING);
            let cy = icon.center().y;
            let half = px((m::LABEL_ROW + m::TIGHT) / 2.0);
            let at = style::label(g, tx, cy - half, Align::Left, e.label().to_owned(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), style::c(p.panel_text));
            olo_kit::agent::record(g, olo_kit::agent::Node::label(e.label().to_owned(), at));
            style::label(g, tx, cy + half, Align::Left, e.kind_text(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
            // An etched rule, then the rows on an exact grid.
            let y = style::snap_coord(g, icon.bottom() + px(m::SPACING));
            style::hline(g, area.left(), area.right(), y, style::shade(&p, p.panel_background, 1.2));
            let lit = y + style::hairline(g);
            style::hline(g, area.left(), area.right(), lit, style::c(p.shine));
            let mut y = y + px(2.0 + m::SPACING);
            for (k, v) in &rows {
                let cy = y + px(m::LABEL_ROW / 2.0);
                let kx = area.left() + px(INFO_KEY_WIDTH);
                style::label(g, kx, cy, Align::Right, *k, style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p));
                let vx = kx + px(m::LABEL_GAP);
                let at = style::label_fit(g, vx, cy, Align::Left, v.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::c(p.panel_text), (area.right() - vx).max(px(0.0)));
                olo_kit::agent::record(g, olo_kit::agent::Node::label(format!("{k} {v}"), at));
                y += px(m::LABEL_ROW + m::TIGHT);
            }
        })
        .size_full();
        // The icon is the item: it drags out, as from a folder window.
        let grab = div()
            .absolute()
            .left(px(m::WINDOW_INSET))
            .top(px(m::WINDOW_INSET))
            .w(px(INFO_ICON))
            .h(px(INFO_ICON))
            .on_mouse_down(MouseButton::Left, cx.listener(|i: &mut InfoWin, ev: &MouseDownEvent, _| i.press = Some(ev.position)))
            .on_window_mouse_up(cx.listener(|i: &mut InfoWin, _: &MouseUpEvent, _| i.press = None))
            .on_window_mouse_move(cx.listener(|i: &mut InfoWin, ev: &MouseMoveEvent, cx| {
                let Some(at) = i.press.filter(|_| ev.pressed_button == Some(MouseButton::Left)) else { return };
                if (ev.position - at).magnitude() < 3.0 {
                    return;
                }
                i.press = None;
                let icon = cx.listener_bounds();
                let e = i.entry.clone();
                let hotspot = point((at.x - icon.left()).clamp(px(0.0), px(INFO_ICON)), (at.y - icon.top()).clamp(px(0.0), px(INFO_ICON)));
                let d = items::Drag { paths: vec![e.path.clone()], icon: items::icon_of(&e), size: INFO_ICON, hotspot, ask: false, name: e.name.clone() };
                items::start(cx, i.id, d, || InfoMsg::DragOver);
            }));
        div().size_full().relative().child(content).child(grab).into_any_element()
    }
}

// ---- Find -------------------------------------------------------------------

/// What a Find window is told.
pub enum FindMsg {
    /// Results came (of search `.0`): the last, with `.2`.
    Found(u64, Vec<find::Hit>, bool),
    /// Changes seen on disk.
    Changed(Arc<Vec<watch::Change>>),
    /// Quick Look's arrows: the selection a step on.
    Step(isize),
    Close,
}

pub struct FindWin {
    /// The app's number for this window.
    id: u64,
    query: Child<TextField>,
    root: usize,
    /// Index into `find::Mode::LABELS`.
    mode: usize,
    everything: bool,
    pub results: Vec<find::Hit>,
    /// The mode the current results were found with.
    shown_mode: find::Mode,
    pub list: ListModel,
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
    polling: bool,
    told: Option<WinInfo>,
}

impl FindWin {
    pub fn new(id: u64, cx: &mut Cx<Self>) -> FindWin {
        let query = text_field(cx, |f| f.hint("Name or text").name("Find"));
        query.focus(cx);
        cx.subscribe(&query, |_: &mut FindWin, _: &TextFieldEvent, cx| cx.invalidate());
        FindWin {
            id,
            query,
            root: 0,
            mode: 0,
            everything: false,
            results: Vec::new(),
            shown_mode: find::Mode::Names,
            list: ListModel::new(false),
            running: false,
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::default(),
            stopped: false,
            started: Instant::now(),
            took: Duration::ZERO,
            follow: (0, Vec::new()),
            polling: false,
            told: None,
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
    pub fn selected(&self) -> Option<Entry> {
        self.list.state().single().and_then(|i| self.results.get(i)).map(|h| h.entry.clone())
    }

    /// Move the selection by `d` results (Quick Look's arrow keys).
    pub fn step_selection(&mut self, d: isize, cx: &mut Cx<Self>) -> Option<Entry> {
        let n = self.results.len();
        if n == 0 {
            return None;
        }
        let k = {
            let mut st = self.list.state_mut();
            let k = st.single().map_or(0, |k| (k as isize + d).clamp(0, n as isize - 1) as usize);
            st.select(k);
            k
        };
        self.list.reveal(cx, k);
        self.results.get(k).map(|h| h.entry.clone())
    }

    /// Find (or Stop, while it runs).
    fn go(&mut self, cx: &mut Cx<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        if self.running {
            self.running = false;
            self.stopped = true;
            self.generation += 1;
            cx.invalidate();
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
        *self.list.state_mut() = Default::default();
        self.cancel = Arc::new(AtomicBool::new(false));
        self.progress = Arc::default();
        self.shown_mode = find::Mode::from_index(self.mode);
        let query = find::Query { root: if self.root == 0 { fs::home() } else { PathBuf::from("/") }, text, mode: self.shown_mode, everything: self.everything };
        let (to, generation) = (cx.messenger(), self.generation);
        find::spawn(query, self.cancel.clone(), self.progress.clone(), move |hits, done| {
            to.send(FindMsg::Found(generation, hits, done));
        });
        cx.invalidate();
    }

    /// Progress lives in the workers' counters: shown again a few times
    /// a second while they run.
    fn poll_while_running(&mut self, cx: &mut Cx<Self>) {
        if !self.running || self.polling {
            return;
        }
        self.polling = true;
        cx.after_do(Duration::from_millis(100), |f, cx| {
            f.polling = false;
            cx.invalidate();
        });
    }

    /// Results came: `done` once the search is over.
    pub fn found(&mut self, generation: u64, hits: Vec<find::Hit>, done: bool, cx: &mut Cx<Self>) {
        if generation != self.generation {
            return;
        }
        self.results.extend(hits);
        if done {
            self.running = false;
            self.took = self.started.elapsed();
        }
        cx.invalidate();
    }

    /// Items changed on disk (`now` says how each is: None, gone).
    pub fn changed(&mut self, now: impl Fn(&Path) -> Option<Option<Entry>>, cx: &mut Cx<Self>) {
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
            self.list.state_mut().selected.clear();
        }
        if changed || self.results.len() != before {
            cx.invalidate();
        }
    }

    /// What the result menu (or its keys) chose, for the selection.
    fn run(&mut self, cmd: &str, _: &mut Cx<Self>) {
        let Some(e) = self.selected() else { return };
        match cmd {
            "open" if e.is_dir => to_hub(ToHub::Open { path: e.path, from: None }),
            "open" => fs::open(&e),
            "parent" => to_hub(ToHub::Reveal(e.path, true)),
            "quick-look" => to_hub(ToHub::QuickLook(self.id)),
            _ => {}
        }
    }

    fn key_down(&mut self, e: &KeyDownEvent, cx: &mut Cx<Self>) {
        let k = &e.keystroke;
        // Escape stops, as the Stop button does.
        if k.key == "escape" && self.running {
            self.go(cx);
            cx.stop_propagation();
            return;
        }
        // Space is Quick Look on the selected result, as in a folder
        // window, unless it goes into the Find field.
        if k.key == "space" && !self.list.typing() && self.selected().is_some() && !self.query.is_focused(cx) {
            self.run("quick-look", cx);
            cx.stop_propagation();
            return;
        }
        // The menu's keys, as there is no menu bar to take them (the Find
        // field keeps its own).
        for c in ["parent", "open", "quick-look"] {
            if olo_kit_base::keys::shortcut(c).is_some_and(|s| olo_kit::keys::pressed(&s, k)) {
                self.run(c, cx);
                cx.stop_propagation();
                return;
            }
        }
    }

    /// What the app knows of this window, told when it changes: its
    /// results' folders (to watch), its one selected result (Quick Look
    /// follows it).
    fn tell_app(&mut self, surface: Option<u32>) {
        let info = WinInfo { path: PathBuf::new(), surface, watched: self.followed().to_vec(), selected: self.selected(), folder: None, find: true };
        if self.told.as_ref().is_none_or(|t| !t.same(&info)) {
            crate::publish(self.id, info.clone());
            self.told = Some(info);
        }
    }
}

impl View for FindWin {
    type Message = FindMsg;

    fn message(&mut self, message: FindMsg, cx: &mut Cx<Self>) {
        match message {
            FindMsg::Found(generation, hits, done) => self.found(generation, hits, done, cx),
            FindMsg::Changed(changes) => self.changed(|path| crate::now_on_disk(&changes, path), cx),
            FindMsg::Step(d) => {
                if let Some(e) = self.step_selection(d, cx) {
                    to_hub(ToHub::Look(self.id, e));
                }
                cx.invalidate();
            }
            FindMsg::Close => cx.close_window(),
        }
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        self.run(command, cx);
        true
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        to_hub(ToHub::Closed(self.id));
    }

    fn save_state(&mut self, _: &mut Cx<Self>) -> Option<String> {
        serde_json::to_string(&crate::SavedWindow::Find).ok()
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        self.poll_while_running(cx);
        self.tell_app(cx.surface_id());
        let contents = self.shown_mode != find::Mode::Names;
        let mut cols = vec![Column::new("Name", 200.0), Column::new("Location", 170.0)];
        if contents {
            cols.push(Column::new("Line", 44.0).right());
            cols.push(Column::new("Match", 300.0));
        } else {
            cols.push(Column::new("Size", 70.0).right());
            cols.push(Column::new("Modified", 170.0));
        }
        let list = ColumnListView::new(cx, "find-results", &self.list, cols, self.results.len(), move |f: &FindWin, cell, g| {
            let Some(h) = f.results.get(cell.row) else { return };
            let e = &h.entry;
            match (cell.column, contents) {
                (0, _) => name_cell(cell, g, e, true),
                (1, _) => cell.text(g, e.path.parent().map(fs::display_path).unwrap_or_default()),
                (2, true) => cell.text(g, h.line.as_ref().map(|(n, _)| n.to_string()).unwrap_or_default()),
                (_, true) => cell.text(g, h.line.as_ref().map(|(_, t)| t.clone()).unwrap_or_default()),
                (2, false) => cell.text(g, e.shown().size.clone()),
                _ => cell.text(g, e.shown().modified.clone()),
            }
        })
        // (Results come in while it's drawn: a row already found isn't
        // drawn again for them.)
        .row_key(move |f: &FindWin, row| f.results.get(row).map_or(0, |h| style::key((folder::row_key(&h.entry), &h.line, contents))))
        .type_select(|f: &FindWin, i| f.results.get(i).map(|h| SharedString::from(h.entry.name.clone())).unwrap_or_default())
        .on_select(cx.listener(|_: &mut FindWin, _: &(), cx| cx.invalidate()))
        .on_open(cx.listener(|f: &mut FindWin, _: &usize, cx| f.run("open", cx)))
        .on_context(cx.listener(|_: &mut FindWin, (_, at): &(usize, Point<Pixels>), cx| {
            let me = cx.view_ref();
            let menu = Menu::new("").item("Open", "open").key("o").item("Open Parent", "parent").key("up").item("Quick Look", "quick-look").key("y");
            menu::context_menu(cx, menu, *at, move |cmd, cx| {
                let cmd = cmd.to_owned();
                me.update(cx, move |f, cx| f.run(&cmd, cx));
            });
        }));
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
        let modes: Vec<SharedString> = find::Mode::LABELS.iter().map(|l| SharedString::from(*l)).collect();
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::TIGHT))
            .on_key_down(cx.listener(|f: &mut FindWin, e: &KeyDownEvent, cx| f.key_down(e, cx)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING))
                    .items_center()
                    .child(div().flex_1().child(TextControl::new(cx, &self.query).label("Find:")))
                    .child(Button::new(if self.running { "Stop" } else { "Find" }).default(true).min_width(80.0).on_click(cx.listener(|f: &mut FindWin, _: &ClickEvent, cx| f.go(cx)))),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING))
                    .items_center()
                    .child(MenuField::new("find-mode", modes, self.mode).label("Match:").on_change(cx.listener(|f: &mut FindWin, i: &usize, cx| {
                        f.mode = *i;
                        cx.invalidate();
                    })))
                    .child(MenuField::new("find-root", vec!["Home".into(), "Entire disk".into()], self.root).label("In:").on_change(cx.listener(|f: &mut FindWin, i: &usize, cx| {
                        f.root = *i;
                        cx.invalidate();
                    })))
                    .child(CheckBox::new("Hidden and ignored files", self.everything).on_toggle(cx.listener(|f: &mut FindWin, on: &bool, cx| {
                        f.everything = *on;
                        cx.invalidate();
                    }))),
            )
            .child(div().flex_1().min_h_0().flex().flex_col().child(list))
            .child(div().h(px(m::CONTROL_HEIGHT)).flex().items_center().child(StringView::new(status).dim(true)))
            .into_any_element()
    }
}

// ---- Tracker Status ---------------------------------------------------------

/// Tracker Status's width: fixed (as BeOS's), so what changes as a job
/// runs (the item's name, the counts) never moves Stop or the window.
/// Room beside Stop for the longest counts ("109,369 of 109,369 items ·
/// about 59 min left") and a name's first twenty letters or so.
pub const STATUS_WIDTH: f32 = 540.0;

/// Tracker Status's options: as tall as its rows (`fits_content`), at
/// its width.
pub fn status_options() -> WindowOptions {
    WindowOptions { fixed: true, ..WindowOptions::new("Tracker Status", STATUS_WIDTH, 60.0).fits_content().floating() }
}

/// "Tracker Status": the file operations running (the app's, read as
/// it's shown), as tall as its rows; it goes when the last is done.
pub struct StatusWin;

impl StatusWin {
    pub fn new(cx: &mut Cx<Self>) -> StatusWin {
        Self::poll(cx);
        StatusWin
    }

    /// Progress lives in the workers' counters: shown again a few times
    /// a second while they run.
    fn poll(cx: &mut Cx<Self>) {
        cx.after_do(Duration::from_millis(100), |_, cx| {
            if crate::jobs().is_empty() {
                return cx.close_window();
            }
            cx.invalidate();
            Self::poll(cx);
        });
    }
}

impl View for StatusWin {
    type Message = ();

    fn body(&mut self, _cx: &mut Cx<Self>) -> Element {
        let jobs: Vec<(u64, String, String, (String, String), f32)> = crate::jobs().iter().map(|j| (j.id, j.title.clone(), ops::amount(j), ops::detail(j), j.progress.fraction())).collect();
        // (As tall as its rows: `status_options`.)
        let mut col = div().w_full().p(px(m::WINDOW_INSET)).flex().flex_col();
        for (k, (id, title, amount, (item, counts), fraction)) in jobs.into_iter().enumerate() {
            if k > 0 {
                col = col.child(div().h(px(m::SPACING))).child(olo_kit::widgets::separator()).child(div().h(px(m::SPACING)));
            }
            // The item's name, cut short to the room the counts leave;
            // the counts whole, at the right, under the amount.
            let mut under = div().flex().flex_row().justify_end().gap(px(m::SPACING));
            if !item.is_empty() {
                under = under.child(StringView::new(item).dim(true).cut(true).height(m::LABEL_ROW));
            }
            if !counts.is_empty() {
                under = under.child(StringView::new(counts).dim(true).height(m::LABEL_ROW));
            }
            col = col.child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(m::TIGHT)).child(StatusBar::new(fraction).label(title).trailing(amount)).child(under))
                    .child(
                        div()
                            .flex_shrink_0()
                            .pt(px(m::LABEL_ROW + m::TIGHT - (m::CONTROL_HEIGHT - 14.0) / 2.0))
                            .child(Button::new("Stop").min_width(64.0).on_click(move |_, _| to_hub(ToHub::StopJob(id)))),
                    ),
            );
        }
        col.into_any_element()
    }
}
