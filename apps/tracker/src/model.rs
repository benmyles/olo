//! A folder window's model: its folder's items (and in column view the
//! columns after it), listed on workers and kept up to date from what
//! changes on disk, with the selection kept by item through it all.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use olo_kit_base::list::ListState;
use olo_ui::{Proxy, gpui::{App, EntityId}};

use crate::{Msg, ViewMode, folder::FolderWin, fs, fs::Entry, watch};

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
/// `Msg::Listed` to the window that asked.
#[derive(Clone)]
pub struct Loader {
    pub win: EntityId,
    pub proxy: Proxy,
}

/// Put `v` in place of `entries`, keeping the selection (by path) and
/// selecting `extra` too.
pub fn replace_entries(entries: &mut Vec<Entry>, list: &mut ListState, mut v: Vec<Entry>, sort: (SortKey, bool), extra: &mut Vec<PathBuf>) {
    let mut keep: Vec<PathBuf> = list.selected.iter().filter_map(|&i| entries.get(i)).map(|e| e.path.clone()).collect();
    keep.append(extra);
    // Where a ⇧ range starts and the keyboard is, by item too.
    let path = |row: Option<usize>| row.and_then(|i| entries.get(i)).map(|e| e.path.clone());
    let (anchor, cursor) = (path(list.anchor), path(list.cursor));
    sort_entries(&mut v, sort.0, sort.1);
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

pub fn sort_entries(entries: &mut [Entry], key: SortKey, asc: bool) {
    entries.sort_by(|a, b| {
        // Folders first, then the chosen key; names break ties.
        let folders = b.is_dir.cmp(&a.is_dir);
        let name = || olo_kit_base::files::natural_cmp(&a.name, &b.name);
        let o = match key {
            SortKey::Name => name(),
            SortKey::Size => a.size.cmp(&b.size).then_with(name),
            SortKey::Modified => a.modified.cmp(&b.modified).then_with(name),
            SortKey::Kind => a.kind.name().cmp(b.kind.name()).then_with(name),
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
        let (hidden, counter, Loader { win, proxy }) = (self.hidden, self.listed.clone(), self.loader.clone());
        let counter = if pane.is_none() { counter } else { Arc::default() };
        std::thread::spawn(move || {
            let result = fs::list_counting(&path, hidden, &counter).map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => "This folder was moved or deleted.".to_owned(),
                std::io::ErrorKind::PermissionDenied => "You don't have permission to see inside this folder.".to_owned(),
                _ => e.to_string(),
            });
            proxy.send(Msg::Listed { win, path, generation, result });
        });
    }

    /// A listing came back; ignore it if a newer one was asked for since.
    pub fn listed(&mut self, path: &Path, generation: u64, result: Result<Vec<Entry>, String>) {
        self.stamp += 1;
        let sort = self.sort;
        let mut first = None;
        if path == self.path && generation == self.root_generation {
            self.loading = false;
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
        if let Some(k) = self.column_of(path) {
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
            let mine: Vec<&watch::Change> = changes
                .iter()
                .filter(|c| matches!(c, watch::Change::Item { dir: d, .. } | watch::Change::Rescan(d) if *d == dir))
                .collect();
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
        self.stamp += 1;
        let Some(e) = fs::entry(path) else { return };
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
    pub fn edit_name(&mut self, p: &Path, cx: &mut App) {
        self.select_path(p);
        let link = self.current().0.iter().any(|e| e.path == p && e.is_link);
        self.editing = Some(crate::items::Rename::new(p.to_owned(), link, cx));
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
        let Some(dir) = paths.first().and_then(|p| p.parent()).map(Path::to_owned) else { return };
        let paths: Vec<PathBuf> = paths.iter().filter(|p| p.parent() == Some(dir.as_path())).cloned().collect();
        let k = match self.column_of(&dir) {
            Some(k) => k,
            None if self.view == ViewMode::Columns && dir.starts_with(&self.path) => {
                // From the deepest column on the way, each selects the
                // folder the next one shows.
                let mut k = (1..=self.panes.len()).rev().find(|&k| dir.starts_with(&self.panes[k - 1].path)).unwrap_or(0);
                let mut at = self.column_at(k).0.to_owned();
                let steps: Vec<_> = dir.strip_prefix(&at).map(|r| r.components().map(|c| c.as_os_str().to_owned()).collect()).unwrap_or_default();
                for name in steps {
                    let next = at.join(name);
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
    fn select_pending(&mut self, k: usize, landed: bool) {
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
