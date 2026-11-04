//! BFilePanel: the Open, Save and folder panels every app shares.
//! [`show`] opens one in a window of its own and calls back
//! once with the answer, then closes it. Folders are listed on a worker
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

use futures::{StreamExt, channel::mpsc};
use gpui::{
    App, AppContext as _, Context, Corner, Entity, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, PathBuilder, Render, SharedString, Styled as _, Subscription,
    Task, Window, WindowHandle, canvas, div, point, px,
};
use olo_kit_base::{
    files::{self, Kind},
    metrics as m, places,
};

use crate::{
    Frame, WindowOptions, icons,
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

type OnDone = Box<dyn FnOnce(Answer, &mut App)>;

/// Opens `panel` in a window titled `title`; `on_done` gets the answer
/// once (Cancelled if the window is closed), then the window closes.
pub fn show(cx: &mut App, title: impl Into<SharedString>, panel: FilePanel, on_done: impl FnOnce(Answer, &mut App) + 'static) -> Option<WindowHandle<Frame>> {
    show_in(cx, window_options(title), panel, on_done)
}

/// A panel's window, titled `title`, for [`show_in`] (to open it over
/// another program's window: `.foreign_parent(handle)`).
pub fn window_options(title: impl Into<SharedString>) -> WindowOptions {
    WindowOptions::new(title, PANEL_SIZE.0, PANEL_SIZE.1).min_size(440.0, 300.0).dialog()
}

/// [`show`] in a window opened with `options`; it takes the keyboard.
pub fn show_in(cx: &mut App, options: WindowOptions, panel: FilePanel, on_done: impl FnOnce(Answer, &mut App) + 'static) -> Option<WindowHandle<Frame>> {
    crate::open_window(cx, options, move |window, cx| {
        window.activate_window();
        cx.new(|cx| PanelView::new(panel, Box::new(on_done), window, cx))
    })
}

#[derive(Debug, Clone)]
struct Item {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<SystemTime>,
    kind: Kind,
    /// Which filters it matches, a bit each (worked out with the listing,
    /// off the UI thread, so choosing another filter is instant).
    fits: u64,
}

struct PanelView {
    panel: FilePanel,
    all: Vec<Item>,
    /// Those shown (they match), sorted.
    items: Rc<Vec<Item>>,
    error: Option<String>,
    loading: bool,
    list: Entity<ListModel>,
    /// The column and direction the items were last sorted in.
    sorted: Option<(usize, bool)>,
    /// Save's name.
    name: Option<Entity<TextField>>,
    /// Asking whether to replace this existing file.
    confirm: Option<PathBuf>,
    /// The folder shown, followed as it changes.
    watch: Option<olo_kit_base::watch::Watch>,
    changed: mpsc::UnboundedSender<()>,
    /// The Viewer showing a file for Space, while it is up, and what to
    /// tell it to show next (its stdin).
    look: Option<std::process::Child>,
    look_in: Option<std::process::ChildStdin>,
    /// What the look shows now.
    looking_at: Option<PathBuf>,
    /// The handle the panel's window is exported as, for the look to
    /// open over it (`Window::export_handle`).
    over: Rc<std::cell::RefCell<Option<String>>>,
    on_done: Option<OnDone>,
    /// Its window, closed once it has answered.
    window: gpui::AnyWindowHandle,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
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
    fn new(panel: FilePanel, on_done: OnDone, window: &mut Window, cx: &mut Context<Self>) -> PanelView {
        let list = ListModel::new(cx, panel.multiple);
        let mut subscriptions = Vec::new();
        let name = (panel.mode == Mode::Save).then(|| {
            let field = text_field(cx, |f| f.name("Save as"));
            let proposed = panel.name.clone();
            field.update(cx, |f, cx| {
                f.set_text(proposed, cx);
                f.caret_to_end(cx);
            });
            // Return saves.
            subscriptions.push(cx.subscribe(&field, |v: &mut PanelView, _, e: &TextFieldEvent, cx| {
                if matches!(e, TextFieldEvent::Entered) {
                    v.save(cx);
                }
            }));
            field
        });
        // The name (Save) or the list has the keyboard.
        match &name {
            Some(f) => window.focus(&gpui::Focusable::focus_handle(f.read(cx), cx)),
            None => list.read(cx).focus(window),
        }
        // Answered Cancelled if the window goes before an answer.
        subscriptions.push(cx.on_release(|v: &mut PanelView, cx| {
            v.put_look_away();
            if let Some(f) = v.on_done.take() {
                f(Answer { result: PanelResult::Cancelled, filter: v.filter(), choices: v.panel.choices.clone(), dir: v.panel.dir.clone() }, cx);
            }
        }));
        // The folder changed on disk: listed again.
        let (changed, mut rx) = mpsc::unbounded::<()>();
        let follow = cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                if this.update(cx, |v, cx| v.list_again(cx)).is_err() {
                    return;
                }
            }
        });
        let dir = panel.dir.clone();
        let over = Rc::<std::cell::RefCell<Option<String>>>::default();
        let exported = over.clone();
        window.export_handle(move |handle| *exported.borrow_mut() = handle);
        let mut v = PanelView {
            panel,
            all: Vec::new(),
            items: Rc::default(),
            error: None,
            loading: false,
            list,
            sorted: None,
            name,
            confirm: None,
            watch: None,
            changed,
            look: None,
            look_in: None,
            looking_at: None,
            over,
            on_done: Some(on_done),
            window: window.window_handle(),
            _tasks: vec![follow],
            _subscriptions: subscriptions,
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
    fn refilter(&mut self, cx: &mut Context<Self>) {
        let state = self.list.read(cx).state.clone();
        let chosen: Vec<String> = state.selected.iter().filter_map(|&i| self.items.get(i)).map(|it| it.name.clone()).collect();
        let mut items: Vec<Item> = self.all.iter().filter(|i| self.matches(i)).cloned().collect();
        // Folders first, then by the column chosen (by name until one
        // is): Name, Kind, Size, Modified.
        let (col, asc) = state.sort.unwrap_or((0, true));
        self.sorted = state.sort;
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
        self.list.update(cx, |l, _| {
            let (multiple, sort) = (l.state.multiple, l.state.sort);
            l.state = olo_kit_base::list::ListState { multiple, sort, ..Default::default() };
            for (i, it) in items.iter().enumerate() {
                if chosen.contains(&it.name) {
                    if l.state.selected.is_empty() {
                        l.state.select(i);
                    } else {
                        l.state.selected.insert(i);
                    }
                }
            }
        });
        self.items = Rc::new(items);
        cx.notify();
    }

    fn go(&mut self, dir: PathBuf, remember: bool, cx: &mut Context<Self>) {
        // (Remembered as a recent folder off the UI thread.)
        if remember {
            let d = dir.clone();
            std::thread::spawn(move || places::visited(&d));
        }
        self.panel.dir = dir.clone();
        self.list.update(cx, |l, _| l.state = olo_kit_base::list::ListState { multiple: l.state.multiple, sort: l.state.sort, ..Default::default() });
        self.loading = true;
        match &self.watch {
            Some(w) => w.set([dir.clone()]),
            None => {
                let changed = self.changed.clone();
                self.watch = olo_kit_base::watch::watch([dir.clone()], move |_| {
                    let _ = changed.unbounded_send(());
                });
            }
        }
        self.list_again(cx);
    }

    /// Lists the folder shown (again), off the UI thread.
    fn list_again(&mut self, cx: &mut Context<Self>) {
        let dir = self.panel.dir.clone();
        let filters = self.panel.filters.clone();
        let task = cx.spawn(async move |this, cx| {
            let read_dir = dir.clone();
            let result = cx.background_executor().spawn(async move { read(&read_dir, &filters) }).await;
            let _ = this.update(cx, |v, cx| {
                if v.panel.dir != dir {
                    return;
                }
                v.loading = false;
                match result {
                    Ok(items) => {
                        v.all = items;
                        v.error = None;
                        v.refilter(cx);
                    }
                    Err(e) => {
                        v.all.clear();
                        v.items = Rc::default();
                        v.error = Some(e);
                    }
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
        // (Only the follower and the latest listing are worth keeping.)
        if self._tasks.len() > 4 {
            self._tasks.drain(1..self._tasks.len() - 1);
        }
    }

    /// Space: the selected file shown in the Viewer, or (shown already)
    /// put away.
    fn toggle_look(&mut self, cx: &mut Context<Self>) {
        if self.put_look_away() {
            return;
        }
        let Some(path) = self.selected_file(cx) else { return };
        let Some(viewer) = olo_xdg::find_program("olo-viewer") else {
            tracing::warn!("file panel: no olo-viewer to look with");
            return;
        };
        // Over the panel (else it opens wherever a new window would).
        let over = self.over.borrow().as_ref().map(|h| format!("--over={h}"));
        use std::process::Stdio;
        let Ok(mut child) = std::process::Command::new(viewer).arg("--look").arg("--follow").args(over).arg(&path).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn() else { return };
        self.look_in = child.stdin.take();
        self.looking_at = Some(path);
        // Its arrows' asks, on a thread (the panel waits on a channel, never
        // polls), each a step through the panel's list.
        if let Some(out) = child.stdout.take() {
            let (tx, mut rx) = mpsc::unbounded::<isize>();
            let read = std::thread::Builder::new().name("file panel look".into()).spawn(move || {
                use std::io::BufRead as _;
                for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                    if let Some(by) = line.strip_prefix("step ").and_then(|n| n.trim().parse().ok())
                        && tx.unbounded_send(by).is_err()
                    {
                        return;
                    }
                }
            });
            if read.is_ok() {
                let task = cx.spawn(async move |me, cx| {
                    while let Some(by) = rx.next().await {
                        if me.update(cx, |v, cx| v.look_step(by, cx)).is_err() {
                            return;
                        }
                    }
                });
                self._tasks.push(task);
            }
        }
        self.look = Some(child);
    }

    /// The file selected (one, not a folder), where it is.
    fn selected_file(&self, cx: &App) -> Option<PathBuf> {
        self.list.read(cx).state.single().and_then(|i| self.items.get(i)).filter(|it| !it.is_dir).map(|it| self.panel.dir.join(&it.name))
    }

    /// The look's arrow: the selection to the file `by` files on (folders
    /// passed over), and the look shown it.
    fn look_step(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(at) = self.list.read(cx).state.single() else { return };
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
        self.list.update(cx, |l, _| {
            l.state.select(i);
            l.reveal(i);
        });
        self.look_follows(cx);
        cx.notify();
    }

    /// The look, if it's up, shown what's selected now.
    fn look_follows(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected_file(cx) else { return };
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
    fn context_menu(&mut self, at: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let chosen: Vec<Item> = self.list.read(cx).state.selected.iter().filter_map(|&i| self.items.get(i)).cloned().collect();
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
        let paths: Vec<PathBuf> = chosen.iter().map(|it| self.panel.dir.join(&it.name)).collect();
        let me = cx.entity();
        menu::context_menu(window, cx, m, at, move |cmd, _, cx| match cmd {
            "quick-look" => me.update(cx, |v, cx| v.toggle_look(cx)),
            olo_kit_base::tracker::COMMAND => {
                if let Some(p) = paths.first()
                    && let Err(e) = olo_kit_base::tracker::open(p)
                {
                    tracing::warn!("file panel: show in Tracker: {e}");
                }
            }
            c if open_with::is_command(c) => {
                if let Some(kind) = kind.clone() {
                    open_with::run(c, kind, paths.clone(), cx);
                }
            }
            _ => {}
        });
    }

    /// The files selected, as paths (folders left out).
    fn selected_files(&self, cx: &App) -> Vec<PathBuf> {
        self.list.read(cx).state.selected.iter().filter_map(|&i| self.items.get(i)).filter(|it| !it.is_dir).map(|it| self.panel.dir.join(&it.name)).collect()
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
    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(field) = &self.name else { return };
        let name = field.read(cx).text().trim().to_owned();
        if name.is_empty() || name.contains('/') {
            return;
        }
        let path = self.panel.dir.join(&name);
        if path.exists() {
            self.confirm = Some(path);
            cx.notify();
        } else {
            self.finish(PanelResult::Chosen(path), cx);
        }
    }

    /// Answers, then closes the window it's in.
    fn finish(&mut self, result: PanelResult, cx: &mut Context<Self>) {
        let Some(f) = self.on_done.take() else { return };
        self.put_look_away();
        let answer = Answer { result, filter: self.filter(), choices: self.panel.choices.clone(), dir: self.panel.dir.clone() };
        let win = self.window;
        // (After this update, its window gone: the app may open another.)
        cx.defer(move |cx| {
            let _ = win.update(cx, |_, window, _| window.remove_window());
            f(answer, cx);
        });
    }

    fn open_row(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(it) = self.items.get(row).cloned() else { return };
        if it.is_dir {
            let d = self.panel.dir.join(&it.name);
            self.go(d, true, cx);
        } else if self.panel.mode == Mode::Open {
            // The row opened, with the rest selected if it's one of them.
            let mut paths = self.selected_files(cx);
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

    fn ok(&mut self, cx: &mut Context<Self>) {
        match self.panel.mode {
            Mode::Save => self.save(cx),
            Mode::Folder => {
                let picked = self.list.read(cx).state.single().and_then(|i| self.items.get(i)).map(|it| self.panel.dir.join(&it.name));
                let dir = self.panel.dir.clone();
                self.finish(PanelResult::Chosen(picked.unwrap_or(dir)), cx);
            }
            Mode::Open => {
                let files = self.selected_files(cx);
                if !files.is_empty() {
                    if let Some(r) = self.opened(files) {
                        self.finish(r, cx);
                    }
                } else if let Some(it) = self.list.read(cx).state.single().and_then(|i| self.items.get(i)).cloned()
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

    fn chose_place(&mut self, choice: &str, cx: &mut Context<Self>) {
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

impl Render for PanelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A header clicked: sorted that way.
        if self.list.read(cx).state.sort != self.sorted {
            self.refilter(cx);
        }
        let me = cx.entity();
        let width = f32::from(window.viewport_size().width) - 2.0 * m::WINDOW_INSET;

        // Top: where we are (a pop-up of the enclosing folders) and Home.
        let home_w = 64.0;
        let path_w = width - home_w - m::SPACING;
        let dir = self.panel.dir.clone();
        let title: SharedString = dir.file_name().map_or_else(|| "/".to_string(), |n| n.to_string_lossy().into_owned()).into();
        let icon = if dir == home() { "home" } else { "folder" };
        let pop = me.clone();
        let path_button = Button::new("").id("panel-path").min_width(path_w).on_click_at(move |b, window, cx| {
            let menu = pop.read(cx).places_menu();
            let to = pop.clone();
            menu::popup(window, cx, menu, b, Corner::BottomLeft, move |choice, _, cx| to.update(cx, |v, cx| v.chose_place(choice, cx)));
        });
        let path_face = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let p = theme::palette(cx);
                let ir = style::snap_centred(window, point(b.left() + px(16.0), b.center().y), 16.0, 16.0);
                icons::paint(window, ir, icon);
                style::label(
                    window,
                    cx,
                    ir.right() + px(m::TIGHT * 2.0),
                    b.center().y,
                    style::Align::Left,
                    title.clone(),
                    style::PLAIN_SIZE,
                    style::ui_font(FontWeight::NORMAL),
                    style::c(p.control_text),
                );
                // A small down arrow at the right of the pop-up.
                let (ax, ay) = (b.right() - px(14.0), b.center().y);
                let mut path = PathBuilder::fill();
                path.move_to(point(ax - px(4.0), ay - px(2.0)));
                path.line_to(point(ax + px(4.0), ay - px(2.0)));
                path.line_to(point(ax, ay + px(2.0)));
                path.close();
                if let Ok(path) = path.build() {
                    window.paint_path(path, style::c(p.control_text));
                }
            },
        )
        .absolute()
        .inset_0();
        let home_to = me.clone();
        let top = div()
            .flex()
            .gap(px(m::SPACING))
            .child(div().relative().child(path_button).child(path_face))
            .child(Button::new("Home").min_width(home_w).on_click(move |_, cx| home_to.update(cx, |v, cx| v.go(home(), true, cx))));

        // The list: Name takes what the others leave (no sideways
        // scrolling).
        let (kind_w, size_w, modified_w) = (96.0, 72.0, 140.0);
        let name_w = (width - kind_w - size_w - modified_w - SCROLL_BAR - 6.0).floor().max(120.0);
        let columns = [Column::new("Name", name_w), Column::new("Kind", kind_w), Column::new("Size", size_w).right(), Column::new("Modified", modified_w)];
        let (items, names) = (self.items.clone(), self.items.clone());
        let message = match (&self.error, self.loading) {
            (Some(e), _) => e.clone(),
            (None, true) => String::new(),
            (None, false) => "No items".into(),
        };
        let (opener, chooser, context) = (me.clone(), me.clone(), me.clone());
        let list = ColumnListView::new("file-panel", &self.list, columns, items.len(), move |cell, window, cx| {
            let Some(it) = items.get(cell.row) else { return };
            match cell.column {
                0 => {
                    let ir = style::snap_centred(window, point(cell.bounds.left() + px(m::CELL_INSET + 8.0), cell.bounds.center().y), 16.0, 16.0);
                    icons::paint(window, ir, if it.is_dir { "folder" } else { it.kind.icon() });
                    // (Read by agents too, whole, as the row's name.)
                    cell.bounds = gpui::Bounds::from_corners(point(ir.right(), cell.bounds.top()), cell.bounds.bottom_right());
                    cell.text(window, cx, it.name.clone());
                }
                1 => cell.text(window, cx, if it.is_dir { "Folder" } else { it.kind.name() }),
                2 if !it.is_dir => cell.text(window, cx, files::format_size(it.size)),
                2 => cell.text(window, cx, "—"),
                _ => cell.text(window, cx, files::format_time(it.modified)),
            }
        })
        .type_select(move |r| names.get(r).map(|it| SharedString::from(it.name.clone())).unwrap_or_default())
        .empty_text(message)
        .on_open(move |row, _, cx| opener.update(cx, |v, cx| v.open_row(row, cx)))
        .on_context(move |_, at, window, cx| context.update(cx, |v, cx| v.context_menu(at, window, cx)))
        .on_select(move |_, cx| {
            chooser.update(cx, |v, cx| {
                // A look up follows the selection.
                v.look_follows(cx);
                // Save: a file chosen gives its name.
                if v.panel.mode == Mode::Save
                    && let Some(it) = v.list.read(cx).state.single().and_then(|i| v.items.get(i)).filter(|it| !it.is_dir).cloned()
                    && let Some(field) = v.name.clone()
                {
                    field.update(cx, |f, cx| f.set_text(it.name, cx));
                }
                cx.notify();
            })
        });

        let mut panel = div().size_full().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).child(top).child(div().flex_1().min_h_0().child(list));

        // Bottom: the name (Save), the app's choices and the buttons (with
        // the Show pop-up).
        if let Some(field) = &self.name {
            panel = panel.child(div().w(px(width * 0.7)).child(TextControl::new(field.clone()).label("Save as:").width(width * 0.7)));
        }
        let show = self.panel.filters.len() > 1 && self.panel.mode != Mode::Folder;
        let font = style::ui_font(FontWeight::NORMAL);
        let labels: Vec<String> = self.panel.choices.iter().filter(|ch| !ch.options.is_empty()).map(|ch| ch.label.clone()).chain(show.then(|| "Show:".to_string())).collect();
        let divider = labels.iter().map(|l| f32::from(style::text_width(window, l.clone(), style::PLAIN_SIZE, font.clone())).ceil()).fold(0.0, f32::max) + m::LABEL_GAP;
        for (i, ch) in self.panel.choices.iter().enumerate() {
            let set = me.clone();
            if ch.options.is_empty() {
                panel = panel.child(CheckBox::new(ch.label.clone(), ch.value == "true").id(format!("panel-choice-{i}")).on_toggle(move |on, _, cx| {
                    set.update(cx, |v, cx| {
                        v.panel.choices[i].value = on.to_string();
                        cx.notify();
                    })
                }));
            } else {
                let picked = ch.options.iter().position(|(o, _)| *o == ch.value).unwrap_or(0);
                let options: Vec<SharedString> = ch.options.iter().map(|(_, l)| l.clone().into()).collect();
                panel =
                    panel.child(div().w(px(width * 0.7)).child(MenuField::new(format!("panel-choice-{i}"), options, picked).label(ch.label.clone()).divider(divider).on_change(move |k, _, cx| {
                        set.update(cx, |v, cx| {
                            if let Some((o, _)) = v.panel.choices[i].options.get(k) {
                                v.panel.choices[i].value = o.clone();
                            }
                            cx.notify();
                        })
                    })));
            }
        }
        let mut row = div().flex().items_center().gap(px(m::SPACING + 2.0)).h(px(m::CONTROL_HEIGHT));
        if let Some(existing) = &self.confirm {
            let msg = format!("“{}” already exists. Replace it?", existing.file_name().unwrap_or_default().to_string_lossy());
            let ask = canvas(
                |_, _, _| {},
                move |b, _, window, cx| {
                    let p = theme::palette(cx);
                    style::label_fit(window, cx, b.left(), b.center().y, style::Align::Left, msg.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD), style::c(p.panel_text), b.size.width);
                },
            )
            .flex_1()
            .h_full();
            let (no, yes, path) = (me.clone(), me.clone(), existing.clone());
            row = row
                .child(ask)
                .child(Button::new("Cancel").on_click(move |_, cx| {
                    no.update(cx, |v, cx| {
                        v.confirm = None;
                        cx.notify();
                    })
                }))
                .child(Button::new("Replace").default(true).on_click(move |_, cx| {
                    let path = path.clone();
                    yes.update(cx, |v, cx| v.finish(PanelResult::Chosen(path), cx))
                }));
        } else {
            let mut left = div().flex_1();
            if show {
                let names: Vec<SharedString> = self.panel.filters.iter().map(|f| f.name.clone().into()).collect();
                let set = me.clone();
                let field_w = (width - 2.0 * m::BUTTON_MIN_WIDTH - 4.0 * m::SPACING).min(260.0);
                left = left.child(MenuField::new("panel-filter", names, self.panel.filter).label("Show:").divider(divider).width(field_w).on_change(move |k, _, cx| {
                    set.update(cx, |v, cx| {
                        v.panel.filter = k;
                        v.refilter(cx);
                    })
                }));
            }
            let label = match self.panel.mode {
                Mode::Save => "Save",
                Mode::Open => "Open",
                Mode::Folder => "Select",
            };
            let single = self.list.read(cx).state.single().is_some();
            let enabled = match self.panel.mode {
                Mode::Save => self.name.as_ref().is_some_and(|f| !f.read(cx).text().trim().is_empty()),
                Mode::Open => single,
                Mode::Folder => true,
            };
            let (cancel, ok) = (me.clone(), me.clone());
            row = row
                .child(left)
                .child(Button::new("Cancel").on_click(move |_, cx| cancel.update(cx, |v, cx| v.finish(PanelResult::Cancelled, cx))))
                .child(Button::new(label).default(true).enabled(enabled).on_click(move |_, cx| ok.update(cx, |v, cx| v.ok(cx))));
        }
        panel = panel.child(row);

        // Keys: Escape cancels (or the question), Space looks.
        let keys = me.clone();
        let typing_field = self.name.clone();
        div().size_full().child(panel).on_key_down(move |e: &KeyDownEvent, window, cx| match e.keystroke.key.as_str() {
            "escape" if e.keystroke.modifiers.number_of_modifiers() == 0 => {
                keys.update(cx, |v, cx| {
                    if v.confirm.take().is_some() {
                        cx.notify();
                    } else {
                        v.finish(PanelResult::Cancelled, cx);
                    }
                });
                cx.stop_propagation();
            }
            "space" if e.keystroke.modifiers.number_of_modifiers() == 0 => {
                // (Not while typing a name, in the list or in Save's field.)
                let in_field = typing_field.as_ref().is_some_and(|f| f.read(cx).is_focused(window));
                let typing = keys.read(cx).list.read(cx).typing();
                if !in_field && !typing {
                    keys.update(cx, |v, cx| v.toggle_look(cx));
                    cx.stop_propagation();
                }
            }
            _ => {}
        })
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
