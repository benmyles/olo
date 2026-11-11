//! Go to File or Folder (Go menu, ⇧⌘G, as Finder's Go to Folder): a path
//! typed or pasted (one in the clipboard is there already, selected, so
//! Return goes at once), the names in its folder that what's typed could
//! be listed under it (each folder read once, off the UI thread; Tab
//! completes, ↑↓ choose), and Go: a folder opens, a file shows selected
//! in its folder's window. A relative path is from the window it was
//! asked in.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use olo_kit::{
    Child, ClickEvent, Cx, Element, InteractiveElement as _, IntoElement as _, KeyDownEvent, ParentElement as _, SharedString, Styled as _, View, WindowCx, div, px,
    widgets::{
        Button, StringView, TextControl, TextField, TextFieldEvent,
        list::{Column, ColumnListView, ListModel},
        text_field,
    },
};
use olo_kit_base::metrics as m;

use crate::{
    ToHub,
    folder::name_cell,
    fs::{self, Entry},
    to_hub,
};

/// The window's size: the field, a few names, the buttons.
pub const WIDTH: f32 = 520.0;
pub const HEIGHT: f32 = 300.0;

/// The path `text` names, if it names one: absolute, from home (`~`,
/// `~/…`), a `file://` URI, or relative to `base`; quoted or with a
/// trailing newline as copied. None for nothing or more than a line.
pub fn parse(text: &str, base: &Path) -> Option<PathBuf> {
    if let Some((_, path)) = olo_kit_base::files::written_path(text) {
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
    olo_kit_base::files::written_path(text).map(|(shown, _)| shown)
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
fn matching(entries: &[Entry], prefix: &str) -> Vec<Entry> {
    let p = prefix.to_lowercase();
    entries.iter().filter(|e| (p.starts_with('.') || !e.name.starts_with('.')) && e.name.to_lowercase().starts_with(&p)).cloned().collect()
}

/// What Tab completes to among the names matching: the one (a folder's
/// ending in `/`), or as far as they all agree, case aside, as the first
/// spells it.
fn completion(matches: &[Entry]) -> Option<String> {
    let first = matches.first()?;
    if matches.len() == 1 {
        return Some(if first.is_dir { format!("{}/", first.name) } else { first.name.clone() });
    }
    let fold = |s: &str| s.chars().flat_map(char::to_lowercase).collect::<Vec<char>>();
    let mut agree = fold(&first.name).len();
    let f = fold(&first.name);
    for e in &matches[1..] {
        let n = fold(&e.name);
        agree = agree.min(f.iter().zip(&n).take_while(|(a, b)| a == b).count());
    }
    Some(first.name.chars().take(agree).collect())
}

/// What the window is told by its workers.
pub enum GoMsg {
    /// A path in the clipboard, as the window opened.
    Copied(String),
    /// A folder read.
    Read(PathBuf, Vec<Entry>),
}

pub struct GoToWin {
    field: Child<TextField>,
    /// Where a relative path starts (the folder of the window it was
    /// asked in).
    base: PathBuf,
    list: ListModel,
    /// What the typed path's last name could be, in its folder.
    pub matches: Vec<Entry>,
    /// Folders read, each once.
    read: HashMap<PathBuf, Arc<Vec<Entry>>>,
    reading: HashSet<PathBuf>,
    /// A row chosen with ↑↓, which Return takes.
    chosen: bool,
    error: Option<String>,
}

impl GoToWin {
    pub fn new(base: PathBuf, cx: &mut Cx<Self>) -> GoToWin {
        let field = text_field(cx, |f| f.hint("A path: /…, ~/… or a name here").name("Go to"));
        // A path copied, ready to go to (read off the looper; put in
        // unless something's been typed by then).
        let to = cx.messenger();
        olo_kit::clipboard::paste_item(move |item| {
            if let Some(p) = item.and_then(|c| c.text()).as_deref().and_then(clipboard_path) {
                to.send(GoMsg::Copied(p));
            }
        });
        field.focus(cx);
        cx.subscribe(&field, |g: &mut GoToWin, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Changed => {
                g.error = None;
                g.chosen = false;
                g.update_matches(cx);
            }
            TextFieldEvent::Entered => g.go(cx),
            TextFieldEvent::Typed(_) => {}
        });
        let mut g = GoToWin { field, base, list: ListModel::new(false), matches: Vec::new(), read: HashMap::new(), reading: HashSet::new(), chosen: false, error: None };
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
            Some(entries) => self.matches = matching(entries, &prefix),
            None => {
                self.matches.clear();
                if self.reading.insert(dir.clone()) {
                    let to = cx.messenger();
                    std::thread::spawn(move || {
                        let entries = fs::list(&dir, true).unwrap_or_default();
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
            Some(e) => Some(if e.is_dir { format!("{}/", e.name) } else { e.name }),
            None => completion(&self.matches),
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
    /// typed. What's there is shown; nothing there, it says so.
    fn go(&mut self, cx: &mut Cx<Self>) {
        let chosen = self.list.state().single().and_then(|i| self.matches.get(i)).cloned();
        if self.chosen
            && let Some(e) = chosen
        {
            self.chosen = false;
            return self.put(&if e.is_dir { format!("{}/", e.name) } else { e.name.clone() }, cx);
        }
        let text = self.text(cx);
        let Some(path) = parse(&text, &self.base) else { return };
        if std::fs::symlink_metadata(&path).is_err() {
            self.error = Some(format!("There's nothing at {}.", fs::display_path(&path)));
            cx.invalidate();
            return;
        }
        // Gone where it was asked: the window closes.
        to_hub(ToHub::Reveal(path, false));
        cx.close_window();
    }
}

impl View for GoToWin {
    type Message = GoMsg;

    fn message(&mut self, message: GoMsg, cx: &mut Cx<Self>) {
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
        let list = ColumnListView::new(cx, "go-to-names", &self.list, vec![Column::new("Name", 120.0)], self.matches.len(), |g: &GoToWin, cell, painter| {
            let Some(e) = g.matches.get(cell.row) else { return };
            name_cell(cell, painter, e, true);
        })
        .no_header()
        .empty_text(SharedString::from(""))
        .on_select(cx.listener(|_: &mut GoToWin, _: &(), cx| cx.invalidate()))
        .on_open(cx.listener(|g: &mut GoToWin, i: &usize, cx| {
            if let Some(e) = g.matches.get(*i).cloned() {
                g.put(&if e.is_dir { format!("{}/", e.name) } else { e.name }, cx);
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
            .capture_key_down(cx.listener(|g: &mut GoToWin, e: &KeyDownEvent, cx| {
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
                    .child(Button::new("Go").default(true).min_width(80.0).on_click(cx.listener(|g: &mut GoToWin, _: &ClickEvent, cx| g.go(cx)))),
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
        let home = fs::home();
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
    }

    #[test]
    fn names_and_completions() {
        let e = |name: &str, is_dir: bool| Entry {
            path: PathBuf::from("/x").join(name),
            name: name.into(),
            is_dir,
            is_link: false,
            size: 0,
            modified: None,
            mode: 0,
            kind: if is_dir { fs::Kind::Folder } else { fs::Kind::Document },
            app_icon: None,
            downloading: None,
        };
        let all = vec![e("Documents", true), e("Downloads", true), e("dotfile", false), e(".config", true), e("Music", true)];
        let names = |v: Vec<Entry>| v.into_iter().map(|e| e.name).collect::<Vec<_>>();
        assert_eq!(names(matching(&all, "do")), ["Documents", "Downloads", "dotfile"]);
        assert_eq!(names(matching(&all, ".")), [".config"]);
        assert_eq!(names(matching(&all, "")), ["Documents", "Downloads", "dotfile", "Music"]);
        assert_eq!(completion(&matching(&all, "Doc")), Some("Documents/".into()));
        assert_eq!(completion(&matching(&all, "Do")), Some("Do".into()));
        assert_eq!(completion(&matching(&all, "dot")), Some("dotfile".into()));
    }
}
