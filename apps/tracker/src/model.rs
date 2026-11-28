//! A folder window's model: its folder's items (and in column view the
//! columns after it), listed on workers and kept up to date from what
//! changes on disk, with the selection kept by item through it all.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use olo_kit::{Cx, Messenger, widgets::TextFieldEvent};
use olo_kit_base::list::ListState;

use crate::{
    ViewMode,
    folder::{FolderMsg, FolderWin},
    fs,
    fs::Entry,
    watch,
};

/// A column to the right of the root in column view.
pub struct Pane {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
    pub list: ListState,
    /// Changed on disk while being listed: list again when it lands.
    pub dirty: bool,
    /// The listing request whose result we are waiting for (see `Loader`).
    pub generation: u64,
    pub loading: bool,
    /// Select the first item once listed (→ came in before the listing).
    pub first: bool,
}

impl Pane {
    fn new(path: PathBuf) -> Self {
        Pane { path, entries: Vec::new(), list: ListState { multiple: true, ..Default::default() }, dirty: false, generation: 0, loading: true, first: false }
    }
}

/// Folders are listed on worker threads (listing stats every item and can
/// take a while on big or slow folders); results come back as
/// `FolderMsg::Listed` to the window that asked.
#[derive(Clone)]
pub struct Loader {
    pub to: Messenger<FolderMsg>,
}

/// Put `v` in place of `entries`, keeping the selection (by path) and
/// selecting `extra` too.
pub fn replace_entries(entries: &mut Vec<Entry>, list: &mut ListState, mut v: Vec<Entry>, sort: Order, extra: &mut Vec<PathBuf>) {
    let mut keep: Vec<PathBuf> = list.selected.iter().filter_map(|&i| entries.get(i)).map(|e| e.path.clone()).collect();
    keep.append(extra);
    // Where a ⇧ range starts and the keyboard is, by item too.
    let path = |row: Option<usize>| row.and_then(|i| entries.get(i)).map(|e| e.path.clone());
    let (anchor, cursor) = (path(list.anchor), path(list.cursor));
    sort_entries(&mut v, sort);
    *entries = v;
    list.selected = entries.iter().enumerate().filter(|(_, e)| keep.contains(&e.path)).map(|(i, _)| i).collect();
    let row = |p: Option<PathBuf>| p.and_then(|p| entries.iter().position(|e| e.path == p));
    list.anchor = row(anchor);
    list.cursor = row(cursor);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SortKey {
    Name,
    Size,
    Modified,
    Kind,
}

/// How a folder's items are put in order: `None` is Olo's own, the
/// folders by name and then the files most recently changed first;
/// `Some` a key chosen by hand (`sorts`), up or down, folders still
/// first.
pub type Order = Option<(SortKey, bool)>;

/// The kind an item sorts by in its Kind column: "Broken Link" for a
/// broken link, as the column says; else its kind, a download's the kind
/// it will be (its column says "Downloading, 31%": it sorts with the
/// finished ones of its kind, not by those words).
fn shown_kind(e: &Entry) -> &'static str {
    if e.broken { olo_kit::tr!("Broken Link") } else { e.kind.name() }
}

pub fn sort_entries(entries: &mut [Entry], order: Order) {
    entries.sort_by(|a, b| {
        // Folders first, then the chosen key; names break ties.
        let folders = b.is_dir.cmp(&a.is_dir);
        let name = || olo_kit_base::files::natural_cmp(&a.name, &b.name);
        let Some((key, asc)) = order else {
            let newest = || b.modified.cmp(&a.modified).then_with(name);
            return folders.then_with(|| if a.is_dir { name() } else { newest() });
        };
        let o = match key {
            SortKey::Name => name(),
            SortKey::Size => a.size.cmp(&b.size).then_with(name),
            SortKey::Modified => a.modified.cmp(&b.modified).then_with(name),
            // (A broken link's is "Broken Link", whatever its name says it
            // was; a download's the kind it will be: `shown_kind`.)
            SortKey::Kind => shown_kind(a).cmp(shown_kind(b)).then_with(name),
        };
        folders.then(if asc { o } else { o.reverse() })
    });
}

impl FolderWin {
    /// Relist the folder (and any columns) in the background.
    pub fn reload(&mut self, hidden: bool) {
        self.hidden = hidden;
        self.request(None);
        for k in 0..self.panes.len() {
            self.request(Some(k));
        }
    }

    /// List the root (`None`) or column `k` on a worker thread.
    pub fn request(&mut self, pane: Option<usize>) {
        self.generation += 1;
        let generation = self.generation;
        let path = match pane {
            None => {
                self.root_generation = generation;
                self.loading = true;
                self.listed = Arc::default();
                self.dirty = false;
                self.path.clone()
            }
            Some(k) => {
                let p = &mut self.panes[k];
                p.generation = generation;
                p.loading = true;
                p.dirty = false;
                p.path.clone()
            }
        };
        let (hidden, counter, Loader { to }) = (self.hidden, self.listed.clone(), self.loader.clone());
        let counter = if pane.is_none() { counter } else { Arc::default() };
        std::thread::spawn(move || {
            let (result, remote_free, host_state, resolved) = if let Some(rex) = fs::rex_path(&path) {
                match crate::rex::list(&rex, hidden) {
                    Ok(res) => {
                        counter.fetch_add(res.entries.len() as u64, std::sync::atomic::Ordering::Relaxed);
                        let canonical = fs::RexPath::new(&res.host, &res.resolved).to_path_buf();
                        (Ok(res.entries), res.free_bytes, Some(crate::rex::HostState::Connected), Some(canonical))
                    }
                    Err(e) => {
                        let st = crate::rex::host_state(&rex.host);
                        (Err(e), None, Some(st), None)
                    }
                }
            } else {
                let r = fs::list_counting(&path, hidden, &counter).map_err(|e| match e.kind() {
                    std::io::ErrorKind::NotFound => olo_kit::words::tr("This folder was moved or deleted.").to_owned(),
                    std::io::ErrorKind::PermissionDenied => olo_kit::words::tr("You don't have permission to see inside this folder.").to_owned(),
                    _ => e.to_string(),
                });
                (r, None, None, None)
            };
            // What Open With offers for the kinds here, worked out now
            // (after the listing's gone), so a menu finds it at once.
            let kinds: std::collections::BTreeSet<String> = result.as_ref().map(|v| v.iter().map(|e| olo_xdg::open::kind_of(&e.name, e.is_dir)).collect()).unwrap_or_default();
            to.send(FolderMsg::Listed { path, generation, result, remote_free, host_state, resolved });
            olo_xdg::open::warm(kinds);
        });
    }

    /// A listing came back; ignore it if a newer one was asked for since.
    pub fn listed(&mut self, path: &Path, generation: u64, result: Result<Vec<Entry>, String>, remote_free: Option<u64>, host_state: Option<crate::rex::HostState>, resolved: Option<PathBuf>) {
        self.stamp += 1;
        let sort = self.sort;
        let mut first = None;
        if path == self.path && generation == self.root_generation {
            self.loading = false;
            if let Some(st) = host_state {
                self.remote_state = Some(st);
            }
            if fs::is_remote(&self.path) {
                self.remote_free = remote_free;
            }
            if let Some(canon) = resolved
                && canon != self.path
            {
                if self.history.get(self.at) == Some(&self.path) {
                    self.history[self.at] = canon.clone();
                }
                self.path = canon;
                self.path_said = false;
            }
            if self.dirty {
                // Something changed after the listing began: what it found
                // may already be stale, so look again (keeping what is
                // shown meanwhile).
                self.request(None);
            }
            match result {
                Ok(v) => {
                    replace_entries(&mut self.entries, &mut self.list, v, sort, &mut Vec::new());
                    self.error = None;
                }
                Err(e) => {
                    self.entries.clear();
                    self.list.selected.clear();
                    self.error = Some(e);
                }
            }
        } else if let Some(k) = self.panes.iter().position(|p| p.path == path && p.generation == generation) {
            if let Some(st) = host_state {
                self.remote_state = Some(st);
            }
            if fs::is_remote(path) && remote_free.is_some() {
                self.remote_free = remote_free;
            }
            match result {
                Ok(v) => {
                    let p = &mut self.panes[k];
                    p.loading = false;
                    let again = p.dirty;
                    replace_entries(&mut p.entries, &mut p.list, v, sort, &mut Vec::new());
                    first = std::mem::take(&mut p.first).then_some(k + 1);
                    if again {
                        self.request(Some(k));
                    }
                }
                // The folder is gone: so are it and the columns after it.
                Err(_) => {
                    self.panes.truncate(k);
                    self.active = self.active.min(self.panes.len());
                }
            }
        } else {
            return;
        }
        // What waits for a folder no longer shown won't be.
        let shown: Vec<bool> = self.pending_select.iter().map(|(d, _)| self.column_of(d).is_some()).collect();
        let mut shown = shown.into_iter();
        self.pending_select.retain(|_| shown.next().unwrap_or(false));
        let listed_dir = if path == self.path || (generation == self.root_generation) { self.path.as_path() } else { path };
        if let Some(k) = self.column_of(listed_dir) {
            self.select_pending(k, true);
        }
        // → went into this column before it was listed: in now, as it
        // would have then.
        if let Some(k) = first
            && self.active == k
            && let Some(p) = self.panes.get(k - 1)
            && p.list.selected.is_empty()
            && !p.entries.is_empty()
        {
            self.choose(k, 0);
        }
    }

    /// Re-sort what is shown (no relisting needed).
    pub fn resort(&mut self) {
        self.stamp += 1;
        let sort = self.sort;
        let v = std::mem::take(&mut self.entries);
        replace_entries(&mut self.entries, &mut self.list, v, sort, &mut Vec::new());
        for p in &mut self.panes {
            let v = std::mem::take(&mut p.entries);
            replace_entries(&mut p.entries, &mut p.list, v, sort, &mut Vec::new());
        }
    }

    /// Move the selection by `d` items (Quick Look's arrow keys); the
    /// newly selected item.
    pub fn step_selection(&mut self, d: isize) -> Option<Entry> {
        let (entries, list) = self.current_mut();
        let n = entries.len();
        if n == 0 {
            return None;
        }
        let k = list.step(d, n)?;
        list.select(k);
        entries.get(k).cloned()
    }

    /// The folders shown, for the watcher.
    pub fn watched(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.path.as_path()).chain(self.panes.iter().map(|p| p.path.as_path()))
    }

    /// Splice changes seen on disk into the root and columns: items that
    /// appeared, changed or went away, keeping the selection and sort.
    pub fn apply(&mut self, changes: &[watch::Change]) {
        self.stamp += 1;
        let (sort, hidden) = (self.sort, self.hidden);
        let mut rescan = Vec::new();
        for pane in std::iter::once(None).chain((0..self.panes.len()).map(Some)) {
            let dir = match pane {
                None => self.path.clone(),
                Some(k) => self.panes[k].path.clone(),
            };
            let mine: Vec<&watch::Change> = changes.iter().filter(|c| matches!(c, watch::Change::Item { dir: d, .. } | watch::Change::Rescan(d) if *d == dir)).collect();
            if mine.is_empty() {
                continue;
            }
            let (entries, list, loading, dirty) = match pane {
                None => (&mut self.entries, &mut self.list, self.loading, &mut self.dirty),
                Some(k) => {
                    let p = &mut self.panes[k];
                    (&mut p.entries, &mut p.list, p.loading, &mut p.dirty)
                }
            };
            if loading {
                *dirty = true;
            }
            if mine.iter().any(|c| matches!(c, watch::Change::Rescan(_))) {
                rescan.push(pane);
                continue;
            }
            let mut v = entries.clone();
            for c in mine {
                let watch::Change::Item { path, entry, .. } = c else { continue };
                v.retain(|e| e.path != *path);
                if let Some(e) = entry
                    && (hidden || !e.name.starts_with('.'))
                {
                    v.push(e.clone());
                }
            }
            replace_entries(entries, list, v, sort, &mut Vec::new());
        }
        // Deepest first, so dropping a gone column doesn't shift the rest.
        for pane in rescan.into_iter().rev() {
            match pane {
                Some(k) if !self.panes[k].path.is_dir() => {
                    self.panes.truncate(k);
                    self.active = self.active.min(self.panes.len());
                }
                _ => self.request(pane),
            }
        }
        let waiting: Vec<PathBuf> = self.pending_select.iter().map(|(d, _)| d.clone()).collect();
        for dir in waiting {
            if let Some(k) = self.column_of(&dir) {
                self.select_pending(k, false);
            }
        }
    }

    /// Show a new item right away (before the next listing lands), so it
    /// can be selected or renamed at once.
    pub fn add_entry(&mut self, path: &Path) {
        let Some(e) = fs::entry(path) else { return };
        self.insert_entry(e);
    }

    /// Insert an already-constructed entry into the current view.
    pub fn insert_entry(&mut self, e: Entry) {
        self.stamp += 1;
        let sort = self.sort;
        let (entries, list) = self.current_mut();
        if entries.iter().any(|x| x.path == e.path) {
            return;
        }
        let mut v = entries.clone();
        v.push(e);
        replace_entries(entries, list, v, sort, &mut Vec::new());
    }

    /// The folder commands act on: the window's, or in column view the
    /// active column's.
    pub fn current_dir(&self) -> PathBuf {
        match self.active_pane() {
            Some(p) => p.path.clone(),
            None => self.path.clone(),
        }
    }

    pub fn active_pane(&self) -> Option<&Pane> {
        if self.view == ViewMode::Columns && self.active > 0 { self.panes.get(self.active - 1) } else { None }
    }

    /// The entries and selection commands act on (see `current_dir`).
    pub fn current(&self) -> (&[Entry], &ListState) {
        match self.active_pane() {
            Some(p) => (&p.entries, &p.list),
            None => (&self.entries, &self.list),
        }
    }

    pub fn current_mut(&mut self) -> (&mut Vec<Entry>, &mut ListState) {
        if self.view == ViewMode::Columns && self.active > 0 && self.active <= self.panes.len() {
            let p = &mut self.panes[self.active - 1];
            (&mut p.entries, &mut p.list)
        } else {
            (&mut self.entries, &mut self.list)
        }
    }

    pub fn selected_entries(&self) -> Vec<Entry> {
        let (entries, list) = self.current();
        list.selected.iter().filter_map(|&i| entries.get(i)).cloned().collect()
    }

    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected_entries().into_iter().map(|e| e.path).collect()
    }

    /// Column view: show `dir` as the column after column `k`.
    pub fn open_column(&mut self, k: usize, dir: PathBuf, hidden: bool) {
        self.hidden = hidden;
        self.panes.truncate(k);
        self.panes.push(Pane::new(dir));
        self.request(Some(k));
    }

    /// Start editing the name of `p` in place.
    pub fn edit_name(&mut self, p: &Path, cx: &mut Cx<Self>) {
        self.select_path(p);
        let link = self.current().0.iter().any(|e| e.path == p && e.is_link);
        let rename = crate::items::Rename::new(p.to_owned(), link, cx);
        // Return takes the name; typing grows the field.
        cx.subscribe(&rename.field, |f: &mut FolderWin, e: &TextFieldEvent, cx| {
            if matches!(e, TextFieldEvent::Entered) {
                f.end_rename(true, cx);
            } else {
                cx.invalidate();
            }
        });
        self.editing = Some(rename);
        self.pending_edit = None;
    }

    fn select_path(&mut self, p: &Path) {
        let (entries, list) = self.current_mut();
        if let Some(i) = entries.iter().position(|e| e.path == p) {
            list.select(i);
        }
    }

    /// Select exactly `paths` (items of one folder) where that folder is
    /// shown; in column view, opening the columns down to it if it is
    /// inside this one, as it was before a reload. Any not listed yet are
    /// selected when that folder's listing brings them in.
    pub fn select_paths(&mut self, paths: &[PathBuf]) {
        let Some(dir) = paths.first().and_then(|p| fs::parent_of(p)) else { return };
        let paths: Vec<PathBuf> = paths.iter().filter(|p| fs::parent_of(p).as_deref() == Some(dir.as_path())).cloned().collect();
        let k = match self.column_of(&dir) {
            Some(k) => k,
            None if self.view == ViewMode::Columns && fs::starts_with_dir(&dir, &self.path) => {
                // From the deepest column on the way, each selects the
                // folder the next one shows.
                let mut k = (1..=self.panes.len()).rev().find(|&k| fs::starts_with_dir(&dir, &self.panes[k - 1].path)).unwrap_or(0);
                let mut at = self.column_at(k).0.to_owned();
                let steps = fs::relative_segments(&dir, &at).unwrap_or_default();
                for name in steps {
                    let next = fs::join_name(&at, &name);
                    self.select_paths(std::slice::from_ref(&next));
                    if self.column_of(&next).is_none() {
                        let hidden = self.hidden;
                        self.open_column(k, next.clone(), hidden);
                    }
                    (at, k) = (next, k + 1);
                }
                k
            }
            None => return,
        };
        if self.view == ViewMode::Columns {
            self.active = k;
        }
        self.pending_select.retain(|(d, _)| *d != dir);
        if !self.select_in(k, &paths) {
            self.pending_select.push((dir, paths));
        }
    }

    /// Select what waits for column `k`'s folder, now that it lists more;
    /// once `landed` (its listing is in), what still isn't there won't be.
    pub(crate) fn select_pending(&mut self, k: usize, landed: bool) {
        let dir = self.column_at(k).0.to_owned();
        let Some((_, paths)) = self.pending_select.iter().find(|(d, _)| *d == dir).cloned() else { return };
        let all = self.select_in(k, &paths);
        let loading = if k == 0 { self.loading } else { self.panes[k - 1].loading };
        if all || (landed && !loading) {
            self.pending_select.retain(|(d, _)| *d != dir);
        }
    }

    /// Select exactly those of `paths` that column `k` lists (if it lists
    /// any), the columns after it following; whether it lists them all.
    fn select_in(&mut self, k: usize, paths: &[PathBuf]) -> bool {
        let (_, entries, list) = self.column_at(k);
        let rows: Vec<usize> = (0..entries.len()).filter(|&i| paths.contains(&entries[i].path)).collect();
        let Some(&first) = rows.first() else { return paths.is_empty() };
        list.select(first);
        list.selected.extend(rows.iter().copied());
        self.show.retain(|&(c, _)| c != k);
        self.show.push((k, first));
        self.follow(k);
        rows.len() == paths.len()
    }

    /// Show the folder another way, what it has selected still in view.
    pub fn set_view(&mut self, view: ViewMode) {
        self.view = view;
        if view == ViewMode::Columns {
            self.active = 0;
        }
        self.show = self.list.cursor().filter(|c| self.list.selected.contains(c)).or_else(|| self.list.selected.first().copied()).map(|row| (0, row)).into_iter().collect();
    }

    /// The root's row to scroll into view (`show`), in the views of one
    /// column.
    pub fn take_show(&mut self) -> Option<usize> {
        std::mem::take(&mut self.show).into_iter().find_map(|(k, row)| (k == 0).then_some(row))
    }

    /// Column view: the columns after column `k` follow what it has
    /// selected, as choosing does (a folder's column, else none).
    fn follow(&mut self, k: usize) {
        if self.view != ViewMode::Columns {
            return;
        }
        let (_, entries, list) = self.column_at(k);
        let folder = match list.selected.iter().collect::<Vec<_>>()[..] {
            [&i] => entries.get(i).filter(|e| e.is_dir).map(|e| e.path.clone()),
            _ => None,
        };
        match folder {
            Some(dir) if self.panes.get(k).is_some_and(|p| p.path == dir) => {}
            Some(dir) => {
                let hidden = self.hidden;
                self.open_column(k, dir, hidden);
            }
            None => self.panes.truncate(k),
        }
        self.active = self.active.min(self.panes.len());
    }

    /// Column `k`'s folder, items and selection (0: the root).
    pub fn column_at(&mut self, k: usize) -> (&Path, &mut Vec<Entry>, &mut ListState) {
        if k == 0 {
            (&self.path, &mut self.entries, &mut self.list)
        } else {
            let p = &mut self.panes[k - 1];
            (&p.path, &mut p.entries, &mut p.list)
        }
    }

    /// The column showing `dir`: 0, the root; the others in column view.
    pub fn column_of(&self, dir: &Path) -> Option<usize> {
        if dir == self.path {
            return Some(0);
        }
        if self.view != ViewMode::Columns {
            return None;
        }
        self.panes.iter().position(|p| p.path == dir).map(|k| k + 1)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    fn item(name: &str, dir: bool, at: u64) -> Entry {
        Entry {
            path: PathBuf::from("/f").join(name),
            name: name.into(),
            is_dir: dir,
            is_link: false,
            broken: false,
            size: at,
            modified: Some(UNIX_EPOCH + Duration::from_secs(at)),
            mode: 0o644,
            kind: if dir { fs::Kind::Folder } else { fs::Kind::Text },
            app_icon: None,
            downloading: None,
            shown: Default::default(),
        }
    }

    fn names(v: &[Entry]) -> Vec<&str> {
        v.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn folders_by_name_then_files_newest_first() {
        let mut v = vec![item("old.txt", false, 1), item("b", true, 9), item("new.txt", false, 3), item("A10", true, 1), item("a2", true, 5), item("mid.txt", false, 2)];
        sort_entries(&mut v, None);
        assert_eq!(names(&v), ["a2", "A10", "b", "new.txt", "mid.txt", "old.txt"]);
        // Chosen by hand: that key for all, folders still first.
        sort_entries(&mut v, Some((SortKey::Name, true)));
        assert_eq!(names(&v), ["a2", "A10", "b", "mid.txt", "new.txt", "old.txt"]);
        sort_entries(&mut v, Some((SortKey::Modified, false)));
        assert_eq!(names(&v), ["b", "a2", "A10", "new.txt", "mid.txt", "old.txt"]);
    }

    /// By Kind, an item sorts by the kind its column shows: a broken
    /// link's "Broken Link", not the kind its name gives it.
    #[test]
    fn kinds_sort_as_shown() {
        let pdf = Entry { kind: fs::Kind::Pdf, ..item("b.pdf", false, 2) };
        let broken = Entry { is_link: true, broken: true, ..item("z.txt", false, 3) };
        // A PDF being downloaded sorts as a PDF, not as its "Downloading".
        let download = Entry { kind: fs::Kind::Pdf, downloading: Some(Some(100)), ..item("d.pdf.part", false, 5) };
        let mut v = vec![item("c.txt", false, 1), pdf, broken, item("a.txt", false, 4), download];
        sort_entries(&mut v, Some((SortKey::Kind, true)));
        assert_eq!(v.iter().map(Entry::kind_text).collect::<Vec<_>>(), ["Broken Link", "PDF Document", "Downloading, 5%", "Text", "Text"]);
        assert_eq!(names(&v), ["z.txt", "b.pdf", "d.pdf.part", "a.txt", "c.txt"]);
        sort_entries(&mut v, Some((SortKey::Kind, false)));
        assert_eq!(names(&v), ["c.txt", "a.txt", "d.pdf.part", "b.pdf", "z.txt"]);
    }
}
