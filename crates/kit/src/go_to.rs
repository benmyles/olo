//! Go to File or Folder (as Finder's Go to Folder): a small window where
//! a path is typed or pasted (one in the clipboard is there already,
//! selected, so Return goes at once), the names in its folder that
//! what's typed could be are listed under it (each folder read once,
//! off the looper; Tab completes, ↑↓ choose), and Go hands the path
//! back. Tracker's Go menu opens it (a folder opens, a file shows
//! selected in its folder's window), and so does every Open, Save and
//! folder panel ([`crate::file_panel`]: the panel goes to the folder, or
//! to the file's folder with the file selected), by the same keys
//! ([`shortcut`]). A relative path is from the folder it was asked in.
//!
//! Whose names they are is the opener's ([`Names`]): Tracker lists its
//! own entries (its icons and marks), the panel plain ones ([`Plain`]).

use std::{
    collections::{HashMap, HashSet},
    marker::PhantomData,
    path::{Path, PathBuf},
    sync::Arc,
};

use olo_interface::{
    Bounds, Child, ClickEvent, Cx, Element, InteractiveElement as _, IntoElement as _, KeyDownEvent, Painter, ParentElement as _, SharedString, Styled as _, View, WindowCx, WindowHandle, div, point,
    px,
};
use olo_kit_base::{
    files::{self, Kind},
    keys::Shortcut,
    metrics as m,
};

use crate::{
    WindowOptions, icons, style,
    widgets::{
        Button, StringView, TextControl, TextField, TextFieldEvent,
        list::{Cell, Column, ColumnListView, ListModel},
        text_field,
    },
};

/// The window's size: the field, a few names, the buttons.
pub const WIDTH: f32 = 520.0;
pub const HEIGHT: f32 = 300.0;

/// The profile's entry for it (Preferences ▸ Keyboard): Tracker's, which
/// the panels take too.
pub const KEYS: &str = "tracker.go-to";

/// The keys that open it, as an app presses them: the profile's
/// ([`KEYS`]), ⇧⌘G unless changed.
pub fn shortcut() -> Shortcut {
    olo_kit_base::keys::shortcut_of(KEYS).unwrap_or(Shortcut::command(crate::keys::evdev("g")).shifted())
}

/// The names of a folder, as the window lists them.
pub trait Names: 'static {
    type Entry: Clone + Send + 'static;
    /// What's in `dir`, hidden names too (called off the looper).
    fn list(dir: &Path) -> Vec<Self::Entry>;
    fn name(e: &Self::Entry) -> &str;
    fn is_dir(e: &Self::Entry) -> bool;
    /// Its row: its icon and its name.
    fn cell(cell: &mut Cell, g: &mut Painter, e: &Self::Entry);
    /// What its row shows, as a number that changes when it does.
    fn key(e: &Self::Entry) -> u64;
}

/// A folder's names as they are on disk: a folder's icon or its kind's.
pub struct Plain;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainName {
    pub name: String,
    pub is_dir: bool,
    pub kind: Kind,
}

impl Names for Plain {
    type Entry = PlainName;

    fn list(dir: &Path) -> Vec<PlainName> {
        use std::os::unix::fs::PermissionsExt;
        let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut names: Vec<PlainName> = rd
            .flatten()
            .filter_map(|e| {
                let meta = std::fs::metadata(e.path()).or_else(|_| e.metadata()).ok()?;
                let is_dir = meta.is_dir();
                Some(PlainName { name: e.file_name().to_string_lossy().into_owned(), is_dir, kind: files::kind_for(&e.path(), is_dir, meta.permissions().mode()) })
            })
            .collect();
        names.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        names
    }

    fn name(e: &PlainName) -> &str {
        &e.name
    }

    fn is_dir(e: &PlainName) -> bool {
        e.is_dir
    }

    fn cell(cell: &mut Cell, g: &mut Painter, e: &PlainName) {
        let ir = style::snap_centred(g, point(cell.bounds.left() + px(m::CELL_INSET + 8.0), cell.bounds.center().y), 16.0, 16.0);
        icons::paint(g, ir, if e.is_dir { "folder" } else { e.kind.icon() });
        cell.bounds = Bounds::from_corners(point(ir.right(), cell.bounds.top()), cell.bounds.bottom_right());
        cell.text(g, e.name.clone());
    }

    fn key(e: &PlainName) -> u64 {
        style::key((&e.name, e.is_dir, e.kind.icon()))
    }
}

/// The path `text` names, if it names one: absolute, from home (`~`,
/// `~/…`), a `file://` URI, or relative to `base`; quoted or with a
/// trailing newline as copied. None for nothing or more than a line.
pub fn parse(text: &str, base: &Path) -> Option<PathBuf> {
    if let Some((_, path)) = files::written_path(text) {
        return Some(path);
    }
    let t = text.trim();
    if t.is_empty() || t.contains('\n') {
        return None;
    }
    Some(base.join(['"', '\''].iter().find_map(|q| t.strip_prefix(*q).and_then(|t| t.strip_suffix(*q))).unwrap_or(t)))
}

/// A path in the clipboard's text, as it's to be shown in the field:
/// only one written whole (absolute, from home as written, a `file://`
/// URI's), not any word that could be a name here.
fn clipboard_path(text: &str) -> Option<String> {
    files::written_path(text).map(|(shown, _)| shown)
}

/// The folder `text`'s last name is looked for in, and that name so far
/// (a path ending in `/` is the folder itself, nothing typed in it yet).
fn split(text: &str, base: &Path) -> Option<(PathBuf, String)> {
    let path = parse(text, base)?;
    if text.trim().ends_with('/') || text.trim() == "~" {
        return Some((path, String::new()));
    }
    let name = path.file_name()?.to_string_lossy().into_owned();
    Some((path.parent()?.to_owned(), name))
}

/// The names in a folder `prefix` could be the start of: case aside, and
/// hidden ones only for a prefix that starts with a dot, as a shell's.
fn matching<N: Names>(entries: &[N::Entry], prefix: &str) -> Vec<N::Entry> {
    let p = prefix.to_lowercase();
    entries.iter().filter(|e| (p.starts_with('.') || !N::name(e).starts_with('.')) && N::name(e).to_lowercase().starts_with(&p)).cloned().collect()
}

/// A name as it goes in the field: a folder's ending in `/`.
fn as_typed<N: Names>(e: &N::Entry) -> String {
    if N::is_dir(e) { format!("{}/", N::name(e)) } else { N::name(e).to_owned() }
}

/// What Tab completes to among the names matching: the one (a folder's
/// ending in `/`), or as far as they all agree, case aside, as the first
/// spells it.
fn completion<N: Names>(matches: &[N::Entry]) -> Option<String> {
    let first = matches.first()?;
    if matches.len() == 1 {
        return Some(as_typed::<N>(first));
    }
    let fold = |s: &str| s.chars().flat_map(char::to_lowercase).collect::<Vec<char>>();
    let f = fold(N::name(first));
    let mut agree = f.len();
    for e in &matches[1..] {
        let n = fold(N::name(e));
        agree = agree.min(f.iter().zip(&n).take_while(|(a, b)| a == b).count());
    }
    Some(N::name(first).chars().take(agree).collect())
}

/// A path for showing: the home folder written `~`.
fn shown(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into());
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// The window's options: a dialog over the window it was asked in (never
/// a tab in its group).
pub fn window_options() -> WindowOptions {
    WindowOptions::new("Go to File or Folder", WIDTH, HEIGHT).min_size(360.0, 200.0).dialog().child()
}

/// Opens it for paths from `base`, with what's typed already (`text`: a
/// `/` or `~` typed where it was asked): `on_go` is told the path that's
/// there once, as the window closes.
pub fn open<N: Names>(base: PathBuf, text: impl Into<String>, on_go: impl Fn(PathBuf) + Send + 'static) -> WindowHandle<GoTo<N>> {
    let text = text.into();
    crate::open_window(window_options(), move |cx| GoTo::<N>::new(base, text, Box::new(on_go), cx))
}

/// What the window is told by its workers.
pub enum GoMsg<E> {
    /// A path in the clipboard, as the window opened.
    Copied(String),
    /// A folder read.
    Read(PathBuf, Vec<E>),
}

pub struct GoTo<N: Names> {
    field: Child<TextField>,
    /// Where a relative path starts (the folder of the window it was
    /// asked in).
    base: PathBuf,
    list: ListModel,
    /// What the typed path's last name could be, in its folder.
    pub matches: Vec<N::Entry>,
    /// Folders read, each once.
    read: HashMap<PathBuf, Arc<Vec<N::Entry>>>,
    reading: HashSet<PathBuf>,
    /// A row chosen with ↑↓, which Return takes.
    chosen: bool,
    error: Option<String>,
    on_go: Box<dyn Fn(PathBuf) + Send>,
    names: PhantomData<N>,
}

impl<N: Names> GoTo<N> {
    pub fn new(base: PathBuf, text: String, on_go: Box<dyn Fn(PathBuf) + Send>, cx: &mut Cx<Self>) -> GoTo<N> {
        let typed = !text.is_empty();
        let field = text_field(cx, |f| f.hint("A path: /…, ~/… or a name here").name("Go to").with_text(text));
        if typed {
            field.update(cx, |f, cx| f.caret_to_end(cx));
        } else {
            // A path copied, ready to go to (read off the looper; put in
            // unless something's been typed by then).
            let to = cx.messenger();
            crate::clipboard::paste_item(move |item| {
                if let Some(p) = item.and_then(|c| c.text()).as_deref().and_then(clipboard_path) {
                    to.send(GoMsg::Copied(p));
                }
            });
        }
        field.focus(cx);
        cx.subscribe(&field, |g: &mut GoTo<N>, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Changed => {
                g.error = None;
                g.chosen = false;
                g.update_matches(cx);
            }
            TextFieldEvent::Entered => g.go(cx),
            TextFieldEvent::Typed(_) => {}
        });
        let mut g = GoTo { field, base, list: ListModel::new(false), matches: Vec::new(), read: HashMap::new(), reading: HashSet::new(), chosen: false, error: None, on_go, names: PhantomData };
        g.update_matches(cx);
        g
    }

    fn text(&self, cx: &WindowCx) -> String {
        self.field.read(cx).text().to_owned()
    }

    /// The names the typed path could go on to, from its folder as read
    /// (read now, on a worker, if it hasn't been).
    fn update_matches(&mut self, cx: &mut Cx<Self>) {
        let Some((dir, prefix)) = split(&self.text(cx), &self.base) else {
            self.matches.clear();
            cx.invalidate();
            return;
        };
        match self.read.get(&dir) {
            Some(entries) => self.matches = matching::<N>(entries, &prefix),
            None => {
                self.matches.clear();
                if self.reading.insert(dir.clone()) {
                    let to = cx.messenger();
                    std::thread::spawn(move || {
                        let entries = N::list(&dir);
                        to.send(GoMsg::Read(dir, entries));
                    });
                }
            }
        }
        {
            let mut st = self.list.state_mut();
            st.selected.clear();
            st.cursor = None;
        }
        cx.invalidate();
    }

    /// The typed path's last name as `name` (a folder's with `/`).
    fn put(&mut self, name: &str, cx: &mut Cx<Self>) {
        let text = self.text(cx);
        let keep = match text.rfind('/') {
            Some(i) => &text[..=i],
            None => "",
        };
        let new = format!("{keep}{name}");
        self.field.update(cx, |f, cx| {
            f.set_text(new, cx);
            f.caret_to_end(cx);
        });
        // (Set, not typed: no `Changed` from the field.)
        self.error = None;
        self.chosen = false;
        self.update_matches(cx);
    }

    /// Tab: the chosen name, else as far as the names agree. False with
    /// none to complete to (Tab goes on to the next control then).
    fn complete(&mut self, cx: &mut Cx<Self>) -> bool {
        let chosen = self.list.state().single().and_then(|i| self.matches.get(i)).cloned();
        let name = match chosen {
            Some(e) => Some(as_typed::<N>(&e)),
            None => completion::<N>(&self.matches),
        };
        match name {
            Some(name) => {
                self.put(&name, cx);
                true
            }
            None => false,
        }
    }

    /// ↑↓: the next name up or down.
    fn step(&mut self, by: isize, cx: &mut Cx<Self>) {
        if self.matches.is_empty() {
            return;
        }
        let last = self.matches.len() as isize - 1;
        let at = self.list.state().single().map_or(if by > 0 { 0 } else { last }, |i| (i as isize + by).clamp(0, last)) as usize;
        self.list.state_mut().select(at);
        self.list.reveal(cx, at);
        self.chosen = true;
        cx.invalidate();
    }

    /// Return or Go: the name chosen with ↑↓ (put in first), else what's
    /// typed. What's there is handed back; nothing there, it says so.
    fn go(&mut self, cx: &mut Cx<Self>) {
        let chosen = self.list.state().single().and_then(|i| self.matches.get(i)).cloned();
        if self.chosen
            && let Some(e) = chosen
        {
            self.chosen = false;
            return self.put(&as_typed::<N>(&e), cx);
        }
        let text = self.text(cx);
        let Some(path) = parse(&text, &self.base) else { return };
        if std::fs::symlink_metadata(&path).is_err() {
            self.error = Some(format!("There's nothing at {}.", shown(&path)));
            cx.invalidate();
            return;
        }
        // Gone where it was asked: the window closes.
        (self.on_go)(path);
        cx.close_window();
    }
}

impl<N: Names> View for GoTo<N> {
    type Message = GoMsg<N::Entry>;

    fn message(&mut self, message: GoMsg<N::Entry>, cx: &mut Cx<Self>) {
        match message {
            GoMsg::Copied(p) => {
                if self.text(cx).is_empty() {
                    self.field.update(cx, |f, cx| {
                        f.set_text(p, cx);
                        f.select_all_text(cx);
                    });
                    self.update_matches(cx);
                }
            }
            GoMsg::Read(dir, entries) => {
                self.reading.remove(&dir);
                self.read.insert(dir, Arc::new(entries));
                self.update_matches(cx);
            }
        }
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let list = ColumnListView::new(cx, "go-to-names", &self.list, vec![Column::new("Name", 120.0)], self.matches.len(), |g: &GoTo<N>, cell, painter| {
            let Some(e) = g.matches.get(cell.row) else { return };
            N::cell(cell, painter, e);
        })
        // (Each key typed says what the window shows: a name still
        // matching isn't drawn again.)
        .row_key(|g: &GoTo<N>, row| g.matches.get(row).map_or(0, N::key))
        .no_header()
        .empty_text(SharedString::from(""))
        .on_select(cx.listener(|_: &mut GoTo<N>, _: &(), cx| cx.invalidate()))
        .on_open(cx.listener(|g: &mut GoTo<N>, i: &usize, cx| {
            if let Some(e) = g.matches.get(*i).cloned() {
                g.put(&as_typed::<N>(&e), cx);
            }
        }));
        let note: SharedString = match &self.error {
            Some(e) => e.clone().into(),
            None => "Tab completes a name; ↑↓ choose one.".into(),
        };
        div()
            .size_full()
            .p(px(m::WINDOW_INSET))
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            // Tab completes, while there's a name to complete to, rather
            // than taking the keyboard to the next control; ↑↓ choose.
            .capture_key_down(cx.listener(|g: &mut GoTo<N>, e: &KeyDownEvent, cx| {
                let k = &e.keystroke;
                match k.key.as_str() {
                    "tab" if !k.modifiers.shift && !k.modifiers.modified() => {
                        if !g.complete(cx) {
                            return;
                        }
                    }
                    "down" => g.step(1, cx),
                    "up" => g.step(-1, cx),
                    "escape" => cx.close_window(),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(TextControl::new(cx, &self.field).label("Go to:"))
            .child(div().flex_1().min_h_0().flex().flex_col().child(list))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(m::SPACING))
                    .items_center()
                    .child(div().flex_1().child(StringView::new(note).dim(self.error.is_none())))
                    .child(Button::new("Cancel").min_width(80.0).on_click(|_: &ClickEvent, cx| cx.close_window()))
                    .child(Button::new("Go").default(true).min_width(80.0).on_click(cx.listener(|g: &mut GoTo<N>, _: &ClickEvent, cx| g.go(cx)))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_as_typed_and_copied() {
        let base = Path::new("/srv/here");
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into());
        assert_eq!(parse("/etc/hosts", base), Some(PathBuf::from("/etc/hosts")));
        assert_eq!(parse("  /etc/hosts\n", base), Some(PathBuf::from("/etc/hosts")));
        assert_eq!(parse("\"/a b/c\"", base), Some(PathBuf::from("/a b/c")));
        assert_eq!(parse("'/a b/c'", base), Some(PathBuf::from("/a b/c")));
        assert_eq!(parse("~", base), Some(home.clone()));
        assert_eq!(parse("~/Documents", base), Some(home.join("Documents")));
        assert_eq!(parse("file:///tmp/a%20b", base), Some(PathBuf::from("/tmp/a b")));
        assert_eq!(parse("src/main.rs", base), Some(PathBuf::from("/srv/here/src/main.rs")));
        assert_eq!(parse("", base), None);
        assert_eq!(parse("/a\n/b", base), None);
        // Only something that is a path is taken from the clipboard.
        assert_eq!(clipboard_path("/var/log\n"), Some("/var/log".to_owned()));
        assert_eq!(clipboard_path("'~/Documents/a b'"), Some("~/Documents/a b".to_owned()));
        assert_eq!(clipboard_path("file:///tmp/a%20b"), Some("/tmp/a b".to_owned()));
        assert_eq!(clipboard_path("hello"), None);
        assert_eq!(split("/usr/lo", base), Some((PathBuf::from("/usr"), "lo".to_owned())));
        assert_eq!(split("/usr/", base), Some((PathBuf::from("/usr/"), String::new())));
        assert_eq!(shown(&home.join("Pictures")), "~/Pictures");
        assert_eq!(shown(Path::new("/etc")), "/etc");
    }

    #[test]
    fn names_and_completions() {
        let e = |name: &str, is_dir: bool| PlainName { name: name.into(), is_dir, kind: if is_dir { Kind::Folder } else { Kind::Document } };
        let all = vec![e("Documents", true), e("Downloads", true), e("dotfile", false), e(".config", true), e("Music", true)];
        let names = |v: Vec<PlainName>| v.into_iter().map(|e| e.name).collect::<Vec<_>>();
        assert_eq!(names(matching::<Plain>(&all, "do")), ["Documents", "Downloads", "dotfile"]);
        assert_eq!(names(matching::<Plain>(&all, ".")), [".config"]);
        assert_eq!(names(matching::<Plain>(&all, "")), ["Documents", "Downloads", "dotfile", "Music"]);
        assert_eq!(completion::<Plain>(&matching::<Plain>(&all, "Doc")), Some("Documents/".into()));
        assert_eq!(completion::<Plain>(&matching::<Plain>(&all, "Do")), Some("Do".into()));
        assert_eq!(completion::<Plain>(&matching::<Plain>(&all, "dot")), Some("dotfile".into()));
    }

    /// A folder as it is on disk: folders first, hidden names too.
    #[test]
    fn a_folder_listed() {
        // (On disk: /tmp is memory here.)
        let dir = Path::new("/var/tmp").join(format!("olo-go-to-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"a").unwrap();
        std::fs::write(dir.join(".hidden"), b"").unwrap();
        let listed: Vec<(String, bool)> = Plain::list(&dir).into_iter().map(|e| (e.name, e.is_dir)).collect();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(listed, [("Sub".to_owned(), true), (".hidden".to_owned(), false), ("a.txt".to_owned(), false)]);
        assert!(Plain::list(Path::new("/nonexistent-olo")).is_empty());
    }
}
