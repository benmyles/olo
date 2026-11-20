//! To-Do's lists on disk. A folder of lists (`$XDG_DATA_HOME/olo/todo`;
//! `OLO_TODO_DIR` for tests), each list a folder of to-dos, each to-do a
//! file of its own (`UID.ics`, one `VTODO`): the layout vdirsyncer and
//! todoman call a vdir, so other programs read, change and sync them as
//! they are. As BeOS kept a person's things (People, Mail): each a file,
//! found and followed as it changes, wherever it's changed from.
//!
//! A list's name and colour are files beside its to-dos (`displayname`,
//! `color`: `#RRGGBB`), as a vdir keeps them; its sections, in their
//! order, are To-Do's own (`olo-sections`, one a line). Files attached to
//! a to-do are kept with its list, out of the way of programs that read
//! only `.ics` files (`.files/UID/`).
//!
//! Everything here reads or writes files: never on a looper (the app
//! calls it on its workers).

use std::{
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use jiff::Timestamp;
use olo_theme::Hue;

use crate::item::{self, Attachment, ToDo};

/// Where the lists are.
pub fn root() -> PathBuf {
    std::env::var_os("OLO_TODO_DIR").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| olo_xdg::data_dir().join("todo"))
}

/// A list: a folder of to-dos.
#[derive(Debug, Clone, PartialEq)]
pub struct List {
    /// Its folder's name.
    pub id: String,
    pub name: String,
    pub hue: Hue,
    /// Its sections, in order (its board's columns).
    pub sections: Vec<String>,
}

/// A to-do as kept: which list's, the file it's in, when that was last
/// changed (to tell a change made elsewhere).
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub list: String,
    pub file: PathBuf,
    pub changed: Option<SystemTime>,
    pub todo: ToDo,
}

/// Every list and every to-do in them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Book {
    pub lists: Vec<List>,
    pub items: Vec<Item>,
}

/// Colours as a vdir's `color` file has them: each hue's, as Apple's and
/// most programs' list colours are, and read back as the nearest hue.
const HEX: [(Hue, [u8; 3]); 10] = [
    (Hue::Red, [0xFF, 0x3B, 0x30]),
    (Hue::Orange, [0xFF, 0x95, 0x00]),
    (Hue::Yellow, [0xFF, 0xCC, 0x00]),
    (Hue::Green, [0x34, 0xC7, 0x59]),
    (Hue::Teal, [0x30, 0xB0, 0xC7]),
    (Hue::Blue, [0x00, 0x7A, 0xFF]),
    (Hue::Purple, [0xAF, 0x52, 0xDE]),
    (Hue::Pink, [0xFF, 0x2D, 0x55]),
    (Hue::Brown, [0xA2, 0x84, 0x5E]),
    (Hue::Gray, [0x8E, 0x8E, 0x93]),
];

pub fn hex_of(hue: Hue) -> String {
    let [r, g, b] = HEX.iter().find(|(h, _)| *h == hue).map(|(_, c)| *c).unwrap_or([0, 0x7A, 0xFF]);
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// The hue nearest a `#RRGGBB` (or `#RRGGBBAA`) colour.
pub fn hue_of(hex: &str) -> Option<Hue> {
    let hex = hex.trim().trim_start_matches('#');
    if hex.len() < 6 {
        return None;
    }
    let part = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok().map(i32::from);
    let (r, g, b) = (part(0)?, part(2)?, part(4)?);
    HEX.iter().min_by_key(|(_, c)| (r - i32::from(c[0])).pow(2) + (g - i32::from(c[1])).pow(2) + (b - i32::from(c[2])).pow(2)).map(|(h, _)| *h)
}

/// A file's name for a UID: what a path can't hold, as `_`.
pub fn file_name(uid: &str) -> String {
    let safe: String = uid.chars().map(|c| if c == '/' || c == '\\' || c == '\0' || c.is_control() { '_' } else { c }).collect();
    let safe = safe.trim_start_matches('.');
    format!("{}.ics", if safe.is_empty() { "to-do" } else { safe })
}

fn read_text(path: &Path) -> String {
    std::fs::read_to_string(path).map(|t| t.trim().to_owned()).unwrap_or_default()
}

/// A list's folder read: its name, colour and sections (its folder's
/// name, blue and none, where it doesn't say).
fn read_list(dir: &Path) -> Option<List> {
    let id = dir.file_name()?.to_str()?.to_owned();
    if id.starts_with('.') || !dir.is_dir() {
        return None;
    }
    let name = Some(read_text(&dir.join("displayname"))).filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
    let hue = hue_of(&read_text(&dir.join("color"))).unwrap_or(Hue::Blue);
    let sections = std::fs::read_to_string(dir.join("olo-sections")).unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
    Some(List { id, name, hue, sections })
}

/// A to-do's file read (None: not a to-do's).
fn read_item(list: &str, path: &Path) -> Option<Item> {
    if path.extension().is_none_or(|x| x != "ics") || path.file_name()?.to_str()?.starts_with('.') {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    let (todo, _) = item::read_file(&text)?;
    Some(Item { list: list.to_owned(), file: path.to_owned(), changed: meta.modified().ok(), todo })
}

impl Book {
    /// Every list under `root`, and their to-dos.
    pub fn load(root: &Path) -> Book {
        let mut book = Book::default();
        let Ok(dirs) = std::fs::read_dir(root) else { return book };
        let mut dirs: Vec<PathBuf> = dirs.flatten().map(|e| e.path()).collect();
        dirs.sort();
        for dir in dirs {
            let Some(list) = read_list(&dir) else { continue };
            let Ok(files) = std::fs::read_dir(&dir) else { continue };
            book.items.extend(files.flatten().filter_map(|f| read_item(&list.id, &f.path())));
            book.lists.push(list);
        }
        book
    }

    /// What changed under `root` read again (`changed`: the paths the
    /// watch told of; a list's folder, its files, a to-do's file), and
    /// nothing else. Whether anything did change.
    pub fn refresh(&mut self, root: &Path, changed: &[PathBuf]) -> bool {
        let before = self.clone();
        for path in changed {
            let Ok(rel) = path.strip_prefix(root) else { continue };
            let parts: Vec<&str> = rel.iter().filter_map(|p| p.to_str()).collect();
            match parts.as_slice() {
                // The folder of lists itself: lists come or go.
                [] => {
                    let now: Vec<String> = std::fs::read_dir(root).into_iter().flatten().flatten().filter_map(|e| read_list(&e.path())).map(|l| l.id).collect();
                    self.lists.retain(|l| now.contains(&l.id));
                    self.items.retain(|i| now.contains(&i.list));
                    for id in now {
                        if !self.lists.iter().any(|l| l.id == id) {
                            self.reread_list(root, &id);
                        }
                    }
                }
                [id] => {
                    if root.join(id).is_dir() {
                        self.reread_list(root, id);
                    } else {
                        self.lists.retain(|l| l.id != *id);
                        self.items.retain(|i| i.list != *id);
                    }
                }
                [id, "displayname" | "color" | "olo-sections"] => {
                    if let Some(list) = read_list(&root.join(id)) {
                        match self.lists.iter_mut().find(|l| l.id == list.id) {
                            Some(l) => *l = list,
                            None => self.reread_list(root, id),
                        }
                    }
                }
                [id, file] if file.ends_with(".ics") && !file.starts_with('.') => {
                    // (A list not yet known: all of it, as it comes.)
                    if !self.lists.iter().any(|l| l.id == *id) {
                        self.reread_list(root, id);
                        continue;
                    }
                    self.items.retain(|i| i.file != *path);
                    if let Some(item) = read_item(id, path) {
                        // (One to-do, one file: a UID seen in another
                        // file of the list is the same to-do, moved.)
                        self.items.retain(|i| !(i.list == item.list && i.todo.uid == item.todo.uid));
                        self.items.push(item);
                    }
                }
                _ => {}
            }
        }
        self.lists.sort_by(|a, b| a.id.cmp(&b.id));
        *self != before
    }

    fn reread_list(&mut self, root: &Path, id: &str) {
        self.lists.retain(|l| l.id != id);
        self.items.retain(|i| i.list != id);
        let dir = root.join(id);
        let Some(list) = read_list(&dir) else { return };
        let files = std::fs::read_dir(&dir).into_iter().flatten().flatten();
        self.items.extend(files.filter_map(|f| read_item(&list.id, &f.path())));
        self.lists.push(list);
    }

    pub fn list(&self, id: &str) -> Option<&List> {
        self.lists.iter().find(|l| l.id == id)
    }

    pub fn item(&self, uid: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.todo.uid == uid)
    }
}

/// `text` written to `path` whole or not at all: to a file beside it,
/// then put in its place.
fn write_whole(path: &Path, text: &str) -> io::Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temp = path.with_file_name(format!(".{name}.{}.new", std::process::id()));
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

/// A new list, called `name`, in `hue`: its folder made.
pub fn create_list(root: &Path, name: &str, hue: Hue) -> io::Result<List> {
    std::fs::create_dir_all(root)?;
    let raw = olo_calendar::event::new_uid();
    let id = format!("list-{}", &raw[..8]);
    let dir = root.join(&id);
    std::fs::create_dir(&dir)?;
    write_whole(&dir.join("displayname"), name)?;
    write_whole(&dir.join("color"), &hex_of(hue))?;
    Ok(List { id, name: name.to_owned(), hue, sections: Vec::new() })
}

/// A list's name, colour and sections written (what differs from `was`).
pub fn change_list(root: &Path, was: Option<&List>, list: &List) -> io::Result<()> {
    let dir = root.join(&list.id);
    if was.is_none_or(|w| w.name != list.name) {
        write_whole(&dir.join("displayname"), &list.name)?;
    }
    if was.is_none_or(|w| w.hue != list.hue) {
        write_whole(&dir.join("color"), &hex_of(list.hue))?;
    }
    if was.is_none_or(|w| w.sections != list.sections) {
        if list.sections.is_empty() {
            let _ = std::fs::remove_file(dir.join("olo-sections"));
        } else {
            write_whole(&dir.join("olo-sections"), &(list.sections.join("\n") + "\n"))?;
        }
    }
    Ok(())
}

/// A list's folder moved to the Trash (where Tracker puts it back from):
/// where it went.
pub fn trash_list(root: &Path, id: &str) -> io::Result<PathBuf> {
    let dir = root.join(id);
    let (target, info) = olo_xdg::trash::trash_slot(&dir)?;
    std::fs::rename(&dir, &target).inspect_err(|_| {
        let _ = std::fs::remove_file(&info);
    })?;
    Ok(target)
}

/// A list's folder brought back from where `trash_list` put it.
pub fn untrash_list(root: &Path, id: &str, from: &Path) -> io::Result<()> {
    std::fs::rename(from, root.join(id))?;
    if let Ok((_, info)) = olo_xdg::trash::trash_origin(from) {
        let _ = std::fs::remove_file(info);
    }
    Ok(())
}

/// A new to-do written into list `list`: its file.
pub fn create(root: &Path, list: &str, todo: &ToDo) -> io::Result<PathBuf> {
    let dir = root.join(list);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(file_name(&todo.uid));
    if path.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "a to-do with that UID is there already"));
    }
    write_whole(&path, &item::new_file(todo, Timestamp::now()))?;
    Ok(path)
}

/// `item`'s to-do changed to `todo`: its file read again (another
/// program may have changed it), the change written into it, and what it
/// held before given back (for undo).
pub fn change(item: &Item, todo: &ToDo) -> io::Result<String> {
    let text = std::fs::read_to_string(&item.file)?;
    // (Changed from what's in the file now, not what was read before:
    // a change made elsewhere meanwhile is kept where this doesn't touch.)
    let (now, _) = item::read_file(&text).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no to-do in the file"))?;
    let base = if now.uid == item.todo.uid { &now } else { &item.todo };
    let mut changed = todo.clone();
    // (What this change didn't touch, as the file has it now.)
    merge_untouched(&item.todo, base, &mut changed);
    let new = item::rewrite(&text, base, &changed, Timestamp::now()).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no to-do in the file"))?;
    write_whole(&item.file, &new)?;
    Ok(text)
}

/// `changed` was made from `was`; the file now has `now`: what `changed`
/// left as it was takes the file's (a change made elsewhere since it was
/// read survives this one).
fn merge_untouched(was: &ToDo, now: &ToDo, changed: &mut ToDo) {
    macro_rules! keep {
        ($($f:ident),*) => {$(if changed.$f == was.$f { changed.$f = now.$f.clone(); })*};
    }
    keep!(title, notes, status, done_at, due, remind, section, order, attachments);
}

/// A file's text put back as it was (undo).
pub fn put_back(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_whole(path, text)
}

/// A to-do's file removed: what it held (for undo). Its attached files
/// stay until it's gone for good (`sweep`).
pub fn remove(item: &Item) -> io::Result<String> {
    let text = std::fs::read_to_string(&item.file)?;
    std::fs::remove_file(&item.file)?;
    Ok(text)
}

/// A to-do moved into another list: its file there (its attached files
/// with it).
pub fn move_to(root: &Path, item: &Item, list: &str) -> io::Result<PathBuf> {
    let dir = root.join(list);
    std::fs::create_dir_all(&dir)?;
    let to = dir.join(item.file.file_name().unwrap_or_default());
    if to.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "that list has a to-do with that UID"));
    }
    std::fs::rename(&item.file, &to)?;
    let files = files_dir(root, &item.list, &item.todo.uid);
    if files.is_dir() {
        let there = files_dir(root, list, &item.todo.uid);
        if let Some(parent) = there.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let moved = std::fs::rename(&files, &there);
        // (Its attachments' addresses follow them.)
        if moved.is_ok() {
            let text = std::fs::read_to_string(&to)?;
            if let Some((todo, _)) = item::read_file(&text) {
                let mut changed = todo.clone();
                let (old, new) = (item::file_uri(&files), item::file_uri(&there));
                for a in &mut changed.attachments {
                    if let Some(rest) = a.uri.strip_prefix(&old) {
                        a.uri = format!("{new}{rest}");
                    }
                }
                if changed != todo
                    && let Some(text) = item::rewrite(&text, &todo, &changed, Timestamp::now())
                {
                    write_whole(&to, &text)?;
                }
            }
        }
    }
    Ok(to)
}

/// Where a to-do's attached files are kept.
pub fn files_dir(root: &Path, list: &str, uid: &str) -> PathBuf {
    root.join(list).join(".files").join(file_name(uid).trim_end_matches(".ics"))
}

/// `file` attached to a to-do: copied into its list's keeping (a picture
/// pasted, a file dropped: the to-do keeps it, wherever the original
/// goes), under a name no other of its files has.
pub fn attach(root: &Path, list: &str, uid: &str, file: &Path, mime: Option<&str>) -> io::Result<Attachment> {
    let dir = files_dir(root, list, uid);
    std::fs::create_dir_all(&dir)?;
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let to = olo_xdg::trash::unique(&dir, &name);
    std::fs::copy(file, &to)?;
    let name = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(name);
    Ok(Attachment { uri: item::file_uri(&to), name, mime: mime.map(str::to_owned) })
}

/// Bytes attached as a file called `name` (a picture pasted).
pub fn attach_bytes(root: &Path, list: &str, uid: &str, name: &str, bytes: &[u8], mime: Option<&str>) -> io::Result<Attachment> {
    let dir = files_dir(root, list, uid);
    std::fs::create_dir_all(&dir)?;
    let to = olo_xdg::trash::unique(&dir, name);
    std::fs::write(&to, bytes)?;
    let name = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| name.to_owned());
    Ok(Attachment { uri: item::file_uri(&to), name, mime: mime.map(str::to_owned) })
}

/// Attached files no to-do holds any more (one removed, an attachment
/// taken off) removed: run now and then, not as they go, so undo can put
/// them back.
pub fn sweep(root: &Path, book: &Book) {
    for list in &book.lists {
        let dir = root.join(&list.id).join(".files");
        let Ok(uids) = std::fs::read_dir(&dir) else { continue };
        for entry in uids.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let held: Vec<&Attachment> = book.items.iter().filter(|i| i.list == list.id && file_name(&i.todo.uid).trim_end_matches(".ics") == name).flat_map(|i| &i.todo.attachments).collect();
            let Ok(files) = std::fs::read_dir(entry.path()) else { continue };
            for f in files.flatten() {
                let uri = item::file_uri(&f.path());
                if !held.iter().any(|a| a.uri == uri) {
                    let _ = std::fs::remove_file(f.path());
                }
            }
            let _ = std::fs::remove_dir(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of the test's own, beside its program (on disk, not /tmp).
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::current_exe().unwrap().parent().unwrap().join(format!("olo-todo-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn colours_as_a_vdir_has_them() {
        for (hue, _) in HEX {
            assert_eq!(hue_of(&hex_of(hue)), Some(hue));
        }
        // Another program's own red, and a colour with its alpha.
        assert_eq!(hue_of("#E0303A"), Some(Hue::Red));
        assert_eq!(hue_of("#1BADF8FF"), Some(Hue::Teal));
        assert_eq!(hue_of("nonsense"), None);
    }

    #[test]
    fn lists_and_to_dos_kept_and_followed() {
        let root = scratch("kept");
        let home = create_list(&root, "Home", Hue::Green).unwrap();
        let mut todo = ToDo::new("one@olo", "Fix the gate");
        todo.order = 10;
        let file = create(&root, &home.id, &todo).unwrap();
        assert_eq!(file, root.join(&home.id).join("one@olo.ics"));
        let mut book = Book::load(&root);
        assert_eq!(book.lists, vec![home.clone()]);
        assert_eq!(book.items.len(), 1);
        assert_eq!(book.items[0].todo.title, "Fix the gate");

        // Changed elsewhere (another program), then here: both kept.
        let item = book.items[0].clone();
        let text = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, text.replace("SUMMARY:Fix the gate", "SUMMARY:Fix the garden gate")).unwrap();
        let mut done = item.todo.clone();
        done.status = crate::item::Status::Done;
        let before = change(&item, &done).unwrap();
        assert!(before.contains("Fix the garden gate"));
        assert!(book.refresh(&root, std::slice::from_ref(&file)));
        assert_eq!(book.items[0].todo.title, "Fix the garden gate");
        assert_eq!(book.items[0].todo.status, crate::item::Status::Done);

        // Sections, a new list, a move between them.
        let mut list = home.clone();
        list.sections = vec!["To Do".into(), "Doing".into()];
        change_list(&root, Some(&home), &list).unwrap();
        assert!(book.refresh(&root, &[root.join(&home.id).join("olo-sections")]));
        assert_eq!(book.list(&home.id).unwrap().sections, ["To Do", "Doing"]);
        let work = create_list(&root, "Work", Hue::Blue).unwrap();
        let moved = move_to(&root, &book.items[0], &work.id).unwrap();
        assert!(book.refresh(&root, &[root.clone(), file.clone(), moved.clone()]));
        assert_eq!(book.items.len(), 1);
        assert_eq!(book.items[0].list, work.id);
        assert_eq!(book, Book::load(&root));

        // Removed, and put back.
        let item = book.items[0].clone();
        let text = remove(&item).unwrap();
        assert!(book.refresh(&root, std::slice::from_ref(&item.file)));
        assert!(book.items.is_empty());
        put_back(&item.file, &text).unwrap();
        assert!(book.refresh(&root, std::slice::from_ref(&item.file)));
        assert_eq!(book.items.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn files_attached_go_with_their_to_do() {
        let root = scratch("attached");
        let home = create_list(&root, "Home", Hue::Green).unwrap();
        let mut todo = ToDo::new("two@olo", "Frame the print");
        create(&root, &home.id, &todo).unwrap();
        let picture = root.join("print.png");
        std::fs::write(&picture, b"not really a png").unwrap();
        let a = attach(&root, &home.id, &todo.uid, &picture, Some("image/png")).unwrap();
        // A second of the same name doesn't take the first's place.
        let b = attach(&root, &home.id, &todo.uid, &picture, Some("image/png")).unwrap();
        assert_ne!(a.uri, b.uri);
        let book = Book::load(&root);
        let item = book.items[0].clone();
        todo.attachments = vec![a.clone(), b.clone()];
        change(&item, &todo).unwrap();
        let book = Book::load(&root);
        let work = create_list(&root, "Work", Hue::Blue).unwrap();
        let moved = move_to(&root, &book.items[0], &work.id).unwrap();
        let book = Book::load(&root);
        let there = &book.items[0].todo.attachments;
        assert!(there.iter().all(|a| a.path().unwrap().exists()), "{there:?}");
        assert!(moved.exists());
        // One taken off: swept, the other kept.
        let mut fewer = book.items[0].todo.clone();
        fewer.attachments.truncate(1);
        change(&book.items[0], &fewer).unwrap();
        let book = Book::load(&root);
        sweep(&root, &book);
        let kept = &book.items[0].todo.attachments;
        assert_eq!(kept.len(), 1);
        assert!(kept[0].path().unwrap().exists());
        assert_eq!(std::fs::read_dir(files_dir(&root, &work.id, "two@olo")).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
