//! BFilePanel: the Open, Save and folder panels every app shares.
//! [`show`] opens one in a window of its own (on a looper of its own)
//! and calls back once with the answer, there, then closes it: the
//! caller sends itself a message from that (BFilePanel's way). Folders are listed on a worker
//! thread, so a slow disk never stalls the window, and listed again the
//! moment they change on disk (`watch`), the selection kept. A panel can
//! offer filters (a Show pop-up: by glob or by MIME type), choose several
//! files at once, and carry an app's extra choices (a check box or a
//! pop-up each, above the buttons).
//!
//! As Tracker's windows, its list has Name, Kind, Size and Modified,
//! sorted by a column's header (folders first); its folder pop-up also
//! goes to the standard folders, favourites and recent folders
//! (`places`, shared with Tracker's Go menu), and adds or takes away the
//! folder shown as a favourite. Space takes a look at the selected file
//! in the Viewer (`olo-viewer --look`, over the panel by the handle it
//! exported, `--over=`: Space again, or Escape there, puts it away). The
//! look follows the panel's list, as Quick Look does: its arrows ask the
//! panel for the file before or after (`step -1`/`step 1` on its stdout),
//! and the panel, its selection moved there, tells it what to show (the
//! path on its stdin), as it does when its selection moves by itself.

use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::SystemTime,
};

use olo_interface::{
    Bounds, Child, ClickEvent, Corner, Cx, Element, FontWeight, InteractiveElement as _, KeyDownEvent, ParentElement as _, PathBuilder, Pixels, Point, SharedString, Styled as _, View, WindowHandle,
    canvas, div, point, px,
};
use olo_kit_base::{
    files::{self, Kind},
    metrics as m, places,
};

use crate::{
    WindowOptions, icons,
    menu::{self, Menu},
    open_with, style, theme,
    widgets::{Button, CheckBox, Column, ColumnListView, ListModel, MenuField, SCROLL_BAR, TextControl, TextField, TextFieldEvent, text_field},
};

/// A good window size for a panel.
pub const PANEL_SIZE: (f32, f32) = (560.0, 420.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Open,
    Save,
    /// Choose a folder: the selected one, or the one shown.
    Folder,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PanelResult {
    Chosen(PathBuf),
    /// What an Open panel made [`multiple`](FilePanel::multiple) chose:
    /// one file or more.
    Several(Vec<PathBuf>),
    Cancelled,
}

/// A filter a panel offers in its Show pop-up: the files whose names match
/// one of `globs` (`*.pdf`, ignoring case) or whose type is one of `types`
/// (`image/*`, `text/plain`; a kind of one counts). Folders always show.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Filter {
    pub name: String,
    pub globs: Vec<String>,
    pub types: Vec<String>,
}

impl Filter {
    pub fn new(name: impl Into<String>) -> Self {
        Filter { name: name.into(), ..Default::default() }
    }

    pub fn glob(mut self, glob: impl Into<String>) -> Self {
        self.globs.push(glob.into());
        self
    }

    pub fn mime(mut self, mime: impl Into<String>) -> Self {
        self.types.push(mime.into());
        self
    }

    /// Whether a file called `name`, of type `mime`, shows.
    pub fn matches(&self, name: &str, mime: &str) -> bool {
        let lower = name.to_lowercase();
        self.globs.iter().any(|g| olo_xdg::mime::glob(&g.to_lowercase(), &lower)) || self.types.iter().any(|t| olo_xdg::mime::is_a(mime, t))
    }
}

/// An app's extra choice beside the list ("Open read-only", "Encoding"): a
/// check box, `value` "true" or "false", if it has no `options`; else a
/// pop-up of them, (id, label) each, `value` the chosen one's id.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub id: String,
    pub label: String,
    pub options: Vec<(String, String)>,
    pub value: String,
}

/// A panel to show: what it's for and what it offers.
#[derive(Clone)]
pub struct FilePanel {
    mode: Mode,
    dir: PathBuf,
    name: String,
    /// Lowercase extensions to offer ("pdf"); empty means everything.
    types: Vec<String>,
    filters: Arc<Vec<Filter>>,
    filter: usize,
    multiple: bool,
    choices: Vec<Choice>,
}

impl FilePanel {
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        Self::new(Mode::Open, dir.into(), String::new())
    }

    /// A Save panel in `dir`, proposing `name`.
    pub fn save(dir: impl Into<PathBuf>, name: impl Into<String>) -> Self {
        Self::new(Mode::Save, dir.into(), name.into())
    }

    /// A panel that picks a folder, starting in `dir`.
    pub fn folder(dir: impl Into<PathBuf>) -> Self {
        Self::new(Mode::Folder, dir.into(), String::new())
    }

    fn new(mode: Mode, dir: PathBuf, name: String) -> Self {
        let dir = if dir.is_dir() { dir } else { home() };
        FilePanel { mode, dir, name, types: Vec::new(), filters: Arc::default(), filter: 0, multiple: false, choices: Vec::new() }
    }

    /// Only offer files with these extensions (folders always show).
    pub fn types(mut self, exts: &[&str]) -> Self {
        self.types = exts.iter().map(|e| e.to_lowercase()).collect();
        self
    }

    /// Offer `filters` in a Show pop-up (the first 64), starting with
    /// `current`.
    pub fn filters(mut self, filters: Vec<Filter>, current: usize) -> Self {
        let filters: Vec<Filter> = filters.into_iter().take(64).collect();
        self.filter = current.min(filters.len().saturating_sub(1));
        self.filters = Arc::new(filters);
        self
    }

    /// Let an Open panel choose several files (⇧ and ⌘ clicks): the answer
    /// is then [`PanelResult::Several`].
    pub fn multiple(mut self) -> Self {
        self.multiple = self.mode == Mode::Open;
        self
    }

    /// Offer the app's own `choices` above the buttons.
    pub fn choices(mut self, choices: Vec<Choice>) -> Self {
        self.choices = choices;
        self
    }
}

/// What a panel came to: the result, the filter chosen (if any were
/// offered), the choices as made, and the folder it ended in.
#[derive(Debug, Clone)]
pub struct Answer {
    pub result: PanelResult,
    pub filter: Option<usize>,
    pub choices: Vec<Choice>,
    pub dir: PathBuf,
}

type OnDone = Box<dyn FnOnce(Answer) + Send>;

/// Opens `panel` in a window titled `title`; `on_done` gets the answer
/// once (Cancelled if the window is closed), on the panel's looper, and
/// the window closes.
pub fn show(title: impl Into<SharedString>, panel: FilePanel, on_done: impl FnOnce(Answer) + Send + 'static) -> WindowHandle<PanelView> {
    show_in(window_options(title), panel, on_done)
}

/// A panel's window, titled `title`, for [`show_in`] (to open it over
/// another program's window: `.foreign_parent(handle)`).
pub fn window_options(title: impl Into<SharedString>) -> WindowOptions {
    WindowOptions::new(title, PANEL_SIZE.0, PANEL_SIZE.1).min_size(440.0, 300.0).dialog()
}

/// [`show`] in a window opened with `options`; it takes the keyboard.
pub fn show_in(options: WindowOptions, panel: FilePanel, on_done: impl FnOnce(Answer) + Send + 'static) -> WindowHandle<PanelView> {
    crate::open_window(options, move |cx| {
        cx.activate_window();
        PanelView::new(panel, Box::new(on_done), cx)
    })
}

#[derive(Debug, Clone)]
pub struct Item {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<SystemTime>,
    kind: Kind,
    /// Which filters it matches, a bit each (worked out with the listing,
    /// off the looper, so choosing another filter is instant).
    fits: u64,
}

/// What a panel is told.
#[derive(Clone)]
pub enum PanelMsg {
    /// The folder shown changed on disk.
    Changed,
    /// A folder, listed.
    Listed(PathBuf, Result<Vec<Item>, String>),
    /// The look's arrow: this many files on.
    LookStep(isize),
    /// The handle the window is exported as.
    Exported(Option<String>),
    /// A header clicked.
    Sorted,
    Selected,
    /// A row opened.
    Open(usize),
    /// A row's menu asked for, there.
    Context(Point<Pixels>),
    /// What was chosen from it.
    Menu(String),
    /// The folder pop-up, under its button.
    PathMenu(Bounds<Pixels>),
    Place(String),
    Home,
    /// An app's choice set (which, its value).
    Choice(usize, String),
    Filter(usize),
    /// Return in Save's name.
    Save,
    /// Replace the file that's there, or don't.
    Replace(bool),
    Cancel,
    Ok,
}

pub struct PanelView {
    panel: FilePanel,
    all: Vec<Item>,
    /// Those shown (they match), sorted.
    items: Rc<Vec<Item>>,
    error: Option<String>,
    loading: bool,
    list: ListModel,
    /// Save's name.
    name: Option<Child<TextField>>,
    /// Asking whether to replace this existing file.
    confirm: Option<PathBuf>,
    /// The folder shown, followed as it changes.
    watch: Option<olo_kit_base::watch::Watch>,
    /// The Viewer showing a file for Space, while it is up, and what to
    /// tell it to show next (its stdin).
    look: Option<std::process::Child>,
    look_in: Option<std::process::ChildStdin>,
    /// What the look shows now.
    looking_at: Option<PathBuf>,
    /// The handle the panel's window is exported as, for the look to
    /// open over it (`WindowCx::export_handle`).
    over: Option<String>,
    /// What the menu that's up is for: the kind and the files.
    menu_for: Option<(Option<String>, Vec<PathBuf>)>,
    on_done: Option<OnDone>,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// Names in the order a person reads them: "file2" before "file10",
/// letters without case.
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |s: &str| {
        let mut out: Vec<(u8, u64, String)> = Vec::new();
        let mut digits = String::new();
        let mut text = String::new();
        let flush = |out: &mut Vec<(u8, u64, String)>, digits: &mut String, text: &mut String| {
            if !digits.is_empty() {
                out.push((0, digits.parse().unwrap_or(u64::MAX), String::new()));
                digits.clear();
            }
            if !text.is_empty() {
                out.push((1, 0, text.to_lowercase()));
                text.clear();
            }
        };
        for c in s.chars() {
            if c.is_ascii_digit() {
                if !text.is_empty() {
                    flush(&mut out, &mut digits, &mut text);
                }
                digits.push(c);
            } else {
                if !digits.is_empty() {
                    flush(&mut out, &mut digits, &mut text);
                }
                text.push(c);
            }
        }
        flush(&mut out, &mut digits, &mut text);
        out
    };
    key(a).cmp(&key(b)).then_with(|| a.cmp(b))
}

impl PanelView {
    fn new(panel: FilePanel, on_done: OnDone, cx: &mut Cx<Self>) -> PanelView {
        let list = ListModel::new(panel.multiple);
        let name = (panel.mode == Mode::Save).then(|| {
            let field = text_field(cx, |f| f.name("Save as").with_text(panel.name.clone()));
            field.update(cx, |f, cx| f.caret_to_end(cx));
            // Return saves.
            cx.subscribe(&field, |_: &mut PanelView, e: &TextFieldEvent, cx| {
                if matches!(e, TextFieldEvent::Entered) {
                    cx.send(PanelMsg::Save);
                }
            });
            field
        });
        // The name (Save) or the list has the keyboard.
        match &name {
            Some(f) => f.focus(cx),
            None => list.focus(cx),
        }
        let dir = panel.dir.clone();
        let exported = cx.messenger();
        cx.export_handle(move |handle| {
            exported.send(PanelMsg::Exported(handle));
        });
        let mut v = PanelView {
            panel,
            all: Vec::new(),
            items: Rc::default(),
            error: None,
            loading: false,
            list,
            name,
            confirm: None,
            watch: None,
            look: None,
            look_in: None,
            looking_at: None,
            over: None,
            menu_for: None,
            on_done: Some(on_done),
        };
        v.go(dir, false, cx);
        v
    }

    fn filter(&self) -> Option<usize> {
        (!self.panel.filters.is_empty()).then_some(self.panel.filter)
    }

    fn matches(&self, it: &Item) -> bool {
        let p = &self.panel;
        let typed = p.types.is_empty() || Path::new(&it.name).extension().is_some_and(|e| p.types.contains(&e.to_string_lossy().to_lowercase()));
        let filtered = p.filters.is_empty() || it.fits & (1 << p.filter) != 0;
        it.is_dir || (p.mode != Mode::Folder && typed && filtered)
    }

    /// Shows the items that match, sorted, the selection kept by name.
    fn refilter(&mut self, cx: &mut Cx<Self>) {
        let (chosen, sort): (Vec<String>, _) = {
            let state = self.list.state();
            (state.selected.iter().filter_map(|&i| self.items.get(i)).map(|it| it.name.clone()).collect(), state.sort)
        };
        let mut items: Vec<Item> = self.all.iter().filter(|i| self.matches(i)).cloned().collect();
        // Folders first, then by the column chosen (by name until one
        // is): Name, Kind, Size, Modified.
        let (col, asc) = sort.unwrap_or((0, true));
        items.sort_by(|a, b| {
            let by = match col {
                1 => a.kind.name().cmp(b.kind.name()),
                2 => a.size.cmp(&b.size),
                3 => a.modified.cmp(&b.modified),
                _ => std::cmp::Ordering::Equal,
            }
            .then_with(|| natural(&a.name, &b.name));
            b.is_dir.cmp(&a.is_dir).then(if asc { by } else { by.reverse() })
        });
        {
            let mut state = self.list.state_mut();
            let (multiple, sort) = (state.multiple, state.sort);
            *state = olo_kit_base::list::ListState { multiple, sort, ..Default::default() };
            for (i, it) in items.iter().enumerate() {
                if chosen.contains(&it.name) {
                    if state.selected.is_empty() {
                        state.select(i);
                    } else {
                        state.selected.insert(i);
                    }
                }
            }
        }
        self.items = Rc::new(items);
        cx.invalidate();
    }

    fn go(&mut self, dir: PathBuf, remember: bool, cx: &mut Cx<Self>) {
        // (Remembered as a recent folder off the looper.)
        if remember {
            let d = dir.clone();
            std::thread::spawn(move || places::visited(&d));
        }
        self.panel.dir = dir.clone();
        {
            let mut state = self.list.state_mut();
            let (multiple, sort) = (state.multiple, state.sort);
            *state = olo_kit_base::list::ListState { multiple, sort, ..Default::default() };
        }
        self.loading = true;
        match &self.watch {
            Some(w) => w.set([dir.clone()]),
            None => {
                let changed = cx.messenger();
                self.watch = olo_kit_base::watch::watch([dir.clone()], move |_| {
                    changed.send(PanelMsg::Changed);
                });
            }
        }
        self.list_again(cx);
        cx.invalidate();
    }

    /// Lists the folder shown (again), on a worker.
    fn list_again(&mut self, cx: &mut Cx<Self>) {
        let dir = self.panel.dir.clone();
        let filters = self.panel.filters.clone();
        cx.work(
            move || {
                let listed = read(&dir, &filters);
                (dir, listed)
            },
            |(dir, listed)| PanelMsg::Listed(dir, listed),
        );
    }

    /// Space: the selected file shown in the Viewer, or (shown already)
    /// put away.
    fn toggle_look(&mut self, cx: &mut Cx<Self>) {
        if self.put_look_away() {
            return;
        }
        let Some(path) = self.selected_file() else { return };
        let Some(viewer) = olo_xdg::find_program("olo-viewer") else {
            tracing::warn!("file panel: no olo-viewer to look with");
            return;
        };
        // Over the panel (else it opens wherever a new window would).
        let over = self.over.as_ref().map(|h| format!("--over={h}"));
        use std::process::Stdio;
        let Ok(mut child) = std::process::Command::new(viewer).arg("--look").arg("--follow").args(over).arg(&path).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn() else { return };
        self.look_in = child.stdin.take();
        self.looking_at = Some(path);
        // Its arrows' asks, read on a thread (the panel is told: it never
        // polls), each a step through the panel's list.
        if let Some(out) = child.stdout.take() {
            let steps = cx.messenger();
            let _ = std::thread::Builder::new().name("file panel look".into()).spawn(move || {
                use std::io::BufRead as _;
                for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                    if let Some(by) = line.strip_prefix("step ").and_then(|n| n.trim().parse().ok())
                        && !steps.send(PanelMsg::LookStep(by))
                    {
                        return;
                    }
                }
            });
        }
        self.look = Some(child);
    }

    /// The file selected (one, not a folder), where it is.
    fn selected_file(&self) -> Option<PathBuf> {
        self.list.state().single().and_then(|i| self.items.get(i)).filter(|it| !it.is_dir).map(|it| self.panel.dir.join(&it.name))
    }

    /// The look's arrow: the selection to the file `by` files on (folders
    /// passed over), and the look shown it.
    fn look_step(&mut self, by: isize, cx: &mut Cx<Self>) {
        let Some(at) = self.list.state().single() else { return };
        let mut i = at as isize;
        loop {
            i += by.signum();
            match usize::try_from(i).ok().and_then(|i| self.items.get(i)) {
                None => return,
                Some(it) if it.is_dir => continue,
                Some(_) => break,
            }
        }
        let i = i as usize;
        self.list.state_mut().select(i);
        self.list.reveal(cx, i);
        self.look_follows();
        cx.invalidate();
    }

    /// The look, if it's up, shown what's selected now.
    fn look_follows(&mut self) {
        let Some(path) = self.selected_file() else { return };
        if self.looking_at.as_ref() == Some(&path) {
            return;
        }
        let Some(stdin) = self.look_in.as_mut() else { return };
        use std::io::Write as _;
        if writeln!(stdin, "{}", path.display()).and_then(|_| stdin.flush()).is_ok() {
            self.looking_at = Some(path);
        }
    }

    /// The Viewer put away, if it was up (and still is).
    fn put_look_away(&mut self) -> bool {
        self.look_in = None;
        self.looking_at = None;
        let Some(mut child) = self.look.take() else { return false };
        let up = matches!(child.try_wait(), Ok(None));
        if up {
            let _ = child.kill();
        }
        let _ = child.wait();
        up
    }

    /// An item's menu (the selection's, the item right-clicked in it):
    /// Quick Look, Open With ▸ (`open_with`), Show in Tracker.
    fn context_menu(&mut self, at: Point<Pixels>, cx: &mut Cx<Self>) {
        let chosen: Vec<Item> = self.list.state().selected.iter().filter_map(|&i| self.items.get(i)).cloned().collect();
        let Some(first) = chosen.first() else { return };
        let mut m = Menu::new("");
        if chosen.len() == 1 && !first.is_dir {
            m = m.item("Quick Look", "quick-look");
        }
        let kind = open_with::kind_of(chosen.iter().map(|it| (it.name.as_str(), it.is_dir)));
        if let Some(sub) = kind.as_deref().and_then(open_with::menu) {
            m = m.submenu(sub);
        }
        if chosen.len() == 1 {
            m = m.separator().item("Show in Tracker", olo_kit_base::tracker::COMMAND);
        }
        self.menu_for = Some((kind, chosen.iter().map(|it| self.panel.dir.join(&it.name)).collect()));
        let chose = cx.msg_with(|command: &str| PanelMsg::Menu(command.to_owned()));
        menu::context_menu(cx, m, at, chose);
    }

    fn menu_chose(&mut self, command: &str, cx: &mut Cx<Self>) {
        let Some((kind, paths)) = self.menu_for.clone() else { return };
        match command {
            "quick-look" => self.toggle_look(cx),
            olo_kit_base::tracker::COMMAND => {
                if let Some(p) = paths.first()
                    && let Err(e) = olo_kit_base::tracker::open(p)
                {
                    tracing::warn!("file panel: show in Tracker: {e}");
                }
            }
            c if open_with::is_command(c) => {
                if let Some(kind) = kind {
                    open_with::run(c, kind, paths);
                }
            }
            _ => {}
        }
    }

    /// The files selected, as paths (folders left out).
    fn selected_files(&self) -> Vec<PathBuf> {
        self.list.state().selected.iter().filter_map(|&i| self.items.get(i)).filter(|it| !it.is_dir).map(|it| self.panel.dir.join(&it.name)).collect()
    }

    /// The answer for an Open panel choosing `paths`.
    fn opened(&self, paths: Vec<PathBuf>) -> Option<PanelResult> {
        match (self.panel.multiple, paths.len()) {
            (_, 0) => None,
            (true, _) => Some(PanelResult::Several(paths)),
            (false, _) => paths.into_iter().next().map(PanelResult::Chosen),
        }
    }

    /// Save: the path, or ask first if it exists.
    fn save(&mut self, cx: &mut Cx<Self>) {
        let Some(field) = &self.name else { return };
        let name = field.read(cx).text().trim().to_owned();
        if name.is_empty() || name.contains('/') {
            return;
        }
        let path = self.panel.dir.join(&name);
        if path.exists() {
            self.confirm = Some(path);
            cx.invalidate();
        } else {
            self.finish(PanelResult::Chosen(path), cx);
        }
    }

    /// Answers, then closes the window it's in.
    fn finish(&mut self, result: PanelResult, cx: &mut Cx<Self>) {
        let Some(f) = self.on_done.take() else { return };
        self.put_look_away();
        f(Answer { result, filter: self.filter(), choices: self.panel.choices.clone(), dir: self.panel.dir.clone() });
        cx.close_window();
    }

    fn open_row(&mut self, row: usize, cx: &mut Cx<Self>) {
        let Some(it) = self.items.get(row).cloned() else { return };
        if it.is_dir {
            let d = self.panel.dir.join(&it.name);
            self.go(d, true, cx);
        } else if self.panel.mode == Mode::Open {
            // The row opened, with the rest selected if it's one of them.
            let mut paths = self.selected_files();
            let path = self.panel.dir.join(&it.name);
            if !paths.contains(&path) {
                paths = vec![path];
            }
            if let Some(r) = self.opened(paths) {
                self.finish(r, cx);
            }
        } else if self.panel.mode == Mode::Save {
            if let Some(field) = &self.name {
                field.update(cx, |f, cx| f.set_text(it.name.clone(), cx));
            }
            self.save(cx);
        }
    }

    fn ok(&mut self, cx: &mut Cx<Self>) {
        match self.panel.mode {
            Mode::Save => self.save(cx),
            Mode::Folder => {
                let picked = self.list.state().single().and_then(|i| self.items.get(i)).map(|it| self.panel.dir.join(&it.name));
                let dir = self.panel.dir.clone();
                self.finish(PanelResult::Chosen(picked.unwrap_or(dir)), cx);
            }
            Mode::Open => {
                let files = self.selected_files();
                let single = self.list.state().single().and_then(|i| self.items.get(i)).cloned();
                if !files.is_empty() {
                    if let Some(r) = self.opened(files) {
                        self.finish(r, cx);
                    }
                } else if let Some(it) = single
                    && it.is_dir
                {
                    let d = self.panel.dir.join(&it.name);
                    self.go(d, true, cx);
                }
            }
        }
    }

    /// The folder pop-up: the folders it is in, the standard ones,
    /// favourites (this one added or taken away) and the ones gone to
    /// lately.
    fn places_menu(&self) -> Menu {
        let dir = &self.panel.dir;
        let saved = places::load();
        let mut menu = Menu::new("");
        for (k, a) in dir.ancestors().enumerate().skip(1) {
            menu = menu.item(places::label(a), format!("a{k}")).icon("folder");
        }
        menu = menu.separator();
        for (label, d) in places::standard() {
            let icon = if d == home() { "home" } else { "folder" };
            menu = menu.item(label, format!("g{}", d.display())).icon(icon);
        }
        let mut favorites = Menu::new("Favorites");
        for f in saved.favorites.iter().filter(|f| f.is_dir()) {
            favorites = favorites.item(places::label(f), format!("g{}", f.display())).icon("folder");
        }
        let favorite = saved.favorites.contains(dir);
        if !saved.favorites.is_empty() {
            favorites = favorites.separator();
        }
        let here = places::label(dir);
        favorites = if favorite { favorites.item(format!("Remove “{here}” from Favorites"), "f-") } else { favorites.item(format!("Add “{here}” to Favorites"), "f+") };
        menu = menu.separator().submenu(favorites);
        let recent: Vec<&PathBuf> = saved.recent.iter().filter(|r| *r != dir && r.is_dir()).collect();
        if !recent.is_empty() {
            let mut sub = Menu::new("Recent Folders");
            for r in recent {
                sub = sub.item(places::label(r), format!("g{}", r.display())).icon("folder");
            }
            menu = menu.submenu(sub);
        }
        menu
    }

    fn chose_place(&mut self, choice: &str, cx: &mut Cx<Self>) {
        if choice.is_empty() {
            return;
        }
        let dir = self.panel.dir.clone();
        let to = match choice.split_at(1) {
            ("a", k) => k.parse::<usize>().ok().and_then(|k| dir.ancestors().nth(k)).map(Path::to_path_buf),
            ("g", d) => Some(PathBuf::from(d)).filter(|d| d.is_dir()),
            ("f", on) => {
                let on = on == "+";
                std::thread::spawn(move || places::set_favorite(&dir, on));
                None
            }
            _ => None,
        };
        if let Some(to) = to {
            self.go(to, true, cx);
        }
    }
}

impl View for PanelView {
    type Message = PanelMsg;

    fn message(&mut self, message: PanelMsg, cx: &mut Cx<Self>) {
        match message {
            PanelMsg::Changed => self.list_again(cx),
            PanelMsg::Listed(dir, listed) => {
                if self.panel.dir != dir {
                    return;
                }
                self.loading = false;
                match listed {
                    Ok(items) => {
                        self.all = items;
                        self.error = None;
                        self.refilter(cx);
                    }
                    Err(e) => {
                        self.all.clear();
                        self.items = Rc::default();
                        self.error = Some(e);
                    }
                }
                cx.invalidate();
            }
            PanelMsg::LookStep(by) => self.look_step(by, cx),
            PanelMsg::Exported(handle) => self.over = handle,
            PanelMsg::Sorted => self.refilter(cx),
            PanelMsg::Selected => {
                // A look up follows the selection.
                self.look_follows();
                // Save: a file chosen gives its name.
                let picked = self.list.state().single().and_then(|i| self.items.get(i)).filter(|it| !it.is_dir).cloned();
                if self.panel.mode == Mode::Save
                    && let Some(it) = picked
                    && let Some(field) = &self.name
                {
                    field.update(cx, |f, cx| f.set_text(it.name, cx));
                }
                cx.invalidate();
            }
            PanelMsg::Open(row) => self.open_row(row, cx),
            PanelMsg::Context(at) => self.context_menu(at, cx),
            PanelMsg::Menu(command) => self.menu_chose(&command, cx),
            PanelMsg::PathMenu(at) => {
                let chose = cx.msg_with(|choice: &str| PanelMsg::Place(choice.to_owned()));
                menu::popup(cx, self.places_menu(), at, Corner::BottomLeft, chose);
            }
            PanelMsg::Place(choice) => self.chose_place(&choice, cx),
            PanelMsg::Home => self.go(home(), true, cx),
            PanelMsg::Choice(i, value) => {
                if let Some(ch) = self.panel.choices.get_mut(i) {
                    ch.value = value;
                }
                cx.invalidate();
            }
            PanelMsg::Filter(k) => {
                self.panel.filter = k;
                self.refilter(cx);
            }
            PanelMsg::Save => self.save(cx),
            PanelMsg::Replace(yes) => match (yes, self.confirm.take()) {
                (true, Some(path)) => self.finish(PanelResult::Chosen(path), cx),
                _ => cx.invalidate(),
            },
            PanelMsg::Cancel => self.finish(PanelResult::Cancelled, cx),
            PanelMsg::Ok => self.ok(cx),
        }
    }

    /// Answered Cancelled if the window goes before an answer.
    fn window_closing(&mut self, _: &mut Cx<Self>) {
        self.put_look_away();
        if let Some(f) = self.on_done.take() {
            f(Answer { result: PanelResult::Cancelled, filter: self.filter(), choices: self.panel.choices.clone(), dir: self.panel.dir.clone() });
        }
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let width = f32::from(cx.viewport_size().width) - 2.0 * m::WINDOW_INSET;

        // Top: where we are (a pop-up of the enclosing folders) and Home.
        let home_w = 64.0;
        let path_w = width - home_w - m::SPACING;
        let dir = self.panel.dir.clone();
        let title: SharedString = dir.file_name().map_or_else(|| "/".to_string(), |n| n.to_string_lossy().into_owned()).into();
        let icon = if dir == home() { "home" } else { "folder" };
        let path_button = Button::new("").id("panel-path").min_width(path_w).on_click(cx.listener(|_: &mut Self, _: &ClickEvent, cx| {
            let at = cx.listener_bounds();
            cx.send(PanelMsg::PathMenu(at));
        }));
        let look = style::key((title.clone(), icon));
        let path_face = canvas(move |g, b| {
            let p = theme::palette();
            let ir = style::snap_centred(g, point(b.left() + px(16.0), b.center().y), 16.0, 16.0);
            icons::paint(g, ir, icon);
            style::label(g, ir.right() + px(m::TIGHT * 2.0), b.center().y, style::Align::Left, title.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::c(p.control_text));
            // A small down arrow at the right of the pop-up.
            let (ax, ay) = (b.right() - px(14.0), b.center().y);
            let mut path = PathBuilder::fill();
            path.move_to(point(ax - px(4.0), ay - px(2.0)));
            path.line_to(point(ax + px(4.0), ay - px(2.0)));
            path.line_to(point(ax, ay + px(2.0)));
            path.close();
            if let Ok(path) = path.build() {
                g.paint_path(path, style::c(p.control_text));
            }
        })
        .paint_key(look)
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .size_full();
        let top =
            div().flex().gap(px(m::SPACING)).child(div().relative().child(path_button).child(path_face)).child(Button::new("Home").min_width(home_w).on_click(cx.msg::<ClickEvent>(PanelMsg::Home)));

        // The list: Name takes what the others leave (no sideways
        // scrolling).
        let (kind_w, size_w, modified_w) = (96.0, 72.0, 140.0);
        let name_w = (width - kind_w - size_w - modified_w - SCROLL_BAR - 6.0).floor().max(120.0);
        let columns = [Column::new("Name", name_w), Column::new("Kind", kind_w), Column::new("Size", size_w).right(), Column::new("Modified", modified_w)];
        let message = match (&self.error, self.loading) {
            (Some(e), _) => e.clone(),
            (None, true) => String::new(),
            (None, false) => "No items".into(),
        };
        let list = ColumnListView::new(cx, "file-panel", &self.list, columns, self.items.len(), |v: &PanelView, cell, g| {
            let Some(it) = v.items.get(cell.row) else { return };
            match cell.column {
                0 => {
                    let ir = style::snap_centred(g, point(cell.bounds.left() + px(m::CELL_INSET + 8.0), cell.bounds.center().y), 16.0, 16.0);
                    icons::paint(g, ir, if it.is_dir { "folder" } else { it.kind.icon() });
                    // (Read by agents too, whole, as the row's name.)
                    cell.bounds = Bounds::from_corners(point(ir.right(), cell.bounds.top()), cell.bounds.bottom_right());
                    cell.text(g, it.name.clone());
                }
                1 => cell.text(g, if it.is_dir { "Folder" } else { it.kind.name() }),
                2 if !it.is_dir => cell.text(g, files::format_size(it.size)),
                2 => cell.text(g, "—"),
                _ => cell.text(g, files::format_time(it.modified)),
            }
        })
        .row_key(|v: &PanelView, row| v.items.get(row).map_or(0, |it| style::key((&it.name, it.is_dir, it.size, it.modified))))
        .type_select(|v: &PanelView, row| v.items.get(row).map(|it| SharedString::from(it.name.clone())).unwrap_or_default())
        .empty_text(message)
        .on_open(cx.msg_with(|row: &usize| PanelMsg::Open(*row)))
        .on_context(cx.msg_with(|(_, at): &(usize, Point<Pixels>)| PanelMsg::Context(*at)))
        .on_select(cx.msg(PanelMsg::Selected))
        .on_sort(cx.msg(PanelMsg::Sorted));

        let mut panel = div().size_full().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).child(top).child(div().flex_1().min_h_0().child(list));

        // Bottom: the name (Save), the app's choices and the buttons (with
        // the Show pop-up).
        if let Some(field) = &self.name {
            panel = panel.child(div().w(px(width * 0.7)).child(TextControl::new(cx, field).label("Save as:").width(width * 0.7)));
        }
        let show = self.panel.filters.len() > 1 && self.panel.mode != Mode::Folder;
        let font = style::ui_font(FontWeight::NORMAL);
        let labels: Vec<String> = self.panel.choices.iter().filter(|ch| !ch.options.is_empty()).map(|ch| ch.label.clone()).chain(show.then(|| "Show:".to_string())).collect();
        let divider = labels.iter().map(|l| f32::from(style::text_width(l.clone(), style::PLAIN_SIZE, font.clone())).ceil()).fold(0.0, f32::max) + m::LABEL_GAP;
        for (i, ch) in self.panel.choices.iter().enumerate() {
            if ch.options.is_empty() {
                panel = panel.child(CheckBox::new(ch.label.clone(), ch.value == "true").id(format!("panel-choice-{i}")).on_toggle(cx.msg_with(move |on: &bool| PanelMsg::Choice(i, on.to_string()))));
            } else {
                let picked = ch.options.iter().position(|(o, _)| *o == ch.value).unwrap_or(0);
                let options: Vec<SharedString> = ch.options.iter().map(|(_, l)| l.clone().into()).collect();
                let ids: Vec<String> = ch.options.iter().map(|(o, _)| o.clone()).collect();
                panel = panel.child(
                    div().w(px(width * 0.7)).child(
                        MenuField::new(format!("panel-choice-{i}"), options, picked)
                            .label(ch.label.clone())
                            .divider(divider)
                            .on_change(cx.msg_with(move |k: &usize| PanelMsg::Choice(i, ids.get(*k).cloned().unwrap_or_default()))),
                    ),
                );
            }
        }
        let mut row = div().flex().items_center().gap(px(m::SPACING + 2.0)).h(px(m::CONTROL_HEIGHT));
        if let Some(existing) = &self.confirm {
            let msg = format!("“{}” already exists. Replace it?", existing.file_name().unwrap_or_default().to_string_lossy());
            let look = style::key(&msg);
            let ask = canvas(move |g, b| {
                let p = theme::palette();
                style::label_fit(g, b.left(), b.center().y, style::Align::Left, msg.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), style::c(p.panel_text), b.size.width);
            })
            .paint_key(look)
            .flex_1()
            .h_full();
            row = row
                .child(ask)
                .child(Button::new("Cancel").on_click(cx.msg::<ClickEvent>(PanelMsg::Replace(false))))
                .child(Button::new("Replace").default(true).on_click(cx.msg::<ClickEvent>(PanelMsg::Replace(true))));
        } else {
            let mut left = div().flex_1();
            if show {
                let names: Vec<SharedString> = self.panel.filters.iter().map(|f| f.name.clone().into()).collect();
                let field_w = (width - 2.0 * m::BUTTON_MIN_WIDTH - 4.0 * m::SPACING).min(260.0);
                left = left.child(MenuField::new("panel-filter", names, self.panel.filter).label("Show:").divider(divider).width(field_w).on_change(cx.msg_with(|k: &usize| PanelMsg::Filter(*k))));
            }
            let label = match self.panel.mode {
                Mode::Save => "Save",
                Mode::Open => "Open",
                Mode::Folder => "Select",
            };
            let single = self.list.state().single().is_some();
            let enabled = match self.panel.mode {
                Mode::Save => self.name.as_ref().is_some_and(|f| !f.read(cx).text().trim().is_empty()),
                Mode::Open => single,
                Mode::Folder => true,
            };
            row = row
                .child(left)
                .child(Button::new("Cancel").on_click(cx.msg::<ClickEvent>(PanelMsg::Cancel)))
                .child(Button::new(label).default(true).enabled(enabled).on_click(cx.msg::<ClickEvent>(PanelMsg::Ok)));
        }
        panel = panel.child(row);

        // Keys: Escape cancels (or the question), Space looks.
        div().size_full().child(panel).on_key_down(cx.listener(|v: &mut Self, e: &KeyDownEvent, cx| match e.keystroke.key.as_str() {
            "escape" if !e.keystroke.modifiers.modified() => {
                if v.confirm.take().is_some() {
                    cx.invalidate();
                } else {
                    v.finish(PanelResult::Cancelled, cx);
                }
                cx.stop_propagation();
            }
            "space" if !e.keystroke.modifiers.modified() => {
                // (Not while typing a name, in the list or in Save's field.)
                let in_field = v.name.as_ref().is_some_and(|f| f.is_focused(cx));
                if !in_field && !v.list.typing() {
                    v.toggle_look(cx);
                    cx.stop_propagation();
                }
            }
            _ => {}
        }))
    }
}

fn read(dir: &Path, filters: &[Filter]) -> Result<Vec<Item>, String> {
    use std::os::unix::fs::PermissionsExt;
    let rd = std::fs::read_dir(dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => "You don't have permission to see this folder.".to_string(),
        _ => e.to_string(),
    })?;
    let mut items: Vec<Item> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let meta = std::fs::metadata(e.path()).ok()?;
            let is_dir = meta.is_dir();
            let mut fits = 0;
            if !is_dir && !filters.is_empty() {
                // (Its type only when a filter asks by type.)
                let mime = if filters.iter().any(|f| !f.types.is_empty()) { olo_xdg::mime::from_name(&name).unwrap_or_else(|| "application/octet-stream".into()) } else { String::new() };
                for (i, f) in filters.iter().enumerate() {
                    if f.matches(&name, &mime) {
                        fits |= 1 << i;
                    }
                }
            }
            Some(Item { fits, kind: files::kind_for(&e.path(), is_dir, meta.permissions().mode()), name, is_dir, size: meta.len(), modified: meta.modified().ok() })
        })
        .collect();
    items.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    // What Open With ▸ offers for each kind here, ready for an item's menu.
    let mut kinds: Vec<String> = items.iter().map(|it| olo_xdg::open::kind_of(&it.name, it.is_dir)).collect();
    kinds.sort_unstable();
    kinds.dedup();
    olo_xdg::open::warm(kinds);
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_in_reading_order() {
        let mut names = vec!["file10", "File2", "file1", "a", "B", "file2b"];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(names, ["a", "B", "file1", "File2", "file2b", "file10"]);
    }
}
