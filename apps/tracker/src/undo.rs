//! Edit ▸ Undo and Redo for what Tracker did to files (Ben, 2026-11-07:
//! "tracker needs to support undo for file/folder renames/moves/etc").
//!
//! Each step is what was done, as it was done: items moved (renamed, moved,
//! put back) from where to where, put in the Trash (from where, to where in
//! it), restored from it, or made (copied, duplicated, a new folder, links,
//! what was pasted). Undoing a step runs what takes it back as a job like
//! any other (on a worker, with progress, stoppable): moved items go back,
//! trashed ones are restored, restored ones go back in the Trash, made ones
//! go in the Trash. What that job did is then the step Redo takes back, and
//! the other way round, so both lists hold what was done and a step can go
//! back and forth as often as wanted, many files as one. When it can't any
//! more (an item since moved, renamed or gone, the Trash emptied, something
//! new where it would go back to), the job says so, plainly, and the step
//! goes.
//!
//! The history is the app's (`olo_ui::undo::History`, as every app's), for
//! every window (as Finder's): a rename in one window undoes from another.

use std::path::PathBuf;

use crate::ops::{Kind, Outcome};

/// What a step did.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// Moved (renamed, moved, put back): from, to.
    Moved(Vec<(PathBuf, PathBuf)>),
    /// Put in the Trash: from, where in the Trash.
    Trashed(Vec<(PathBuf, PathBuf)>),
    /// Taken out of the Trash: where in the Trash, to.
    Restored(Vec<(PathBuf, PathBuf)>),
    /// Made: copies, a new folder, links, what was pasted.
    Made(Vec<PathBuf>),
    /// Desktop icons placed (dragged, cleaned up): no job, put back as
    /// they were at once (`desktop::Placed`).
    Placed(crate::desktop::Placed),
}

impl Change {
    pub fn is_empty(&self) -> bool {
        match self {
            Change::Moved(v) | Change::Trashed(v) | Change::Restored(v) => v.is_empty(),
            Change::Made(v) => v.is_empty(),
            Change::Placed(v) => v.is_empty(),
        }
    }

    /// The job that takes it back: its kind, sources and (for `PutBack`)
    /// targets (None: no job, `Placed`).
    pub fn inverse(&self) -> Option<(Kind, Vec<PathBuf>, Vec<PathBuf>)> {
        Some(match self {
            Change::Moved(pairs) => (Kind::PutBack, pairs.iter().map(|(_, to)| to.clone()).collect(), pairs.iter().map(|(from, _)| from.clone()).collect()),
            Change::Trashed(pairs) => (Kind::Restore, pairs.iter().map(|(_, at)| at.clone()).collect(), Vec::new()),
            Change::Restored(pairs) => (Kind::Trash, pairs.iter().map(|(_, to)| to.clone()).collect(), Vec::new()),
            Change::Made(paths) => (Kind::Trash, paths.clone(), Vec::new()),
            Change::Placed(_) => return None,
        })
    }

    /// Icons' places the other way round: what putting them back did.
    pub fn swapped(moved: &crate::desktop::Placed) -> Change {
        Change::Placed(moved.iter().map(|(k, before, after)| (k.clone(), *after, *before)).collect())
    }

    /// What a finished job of `kind` did, from its outcome (some of it, if
    /// it stopped part way: what was done can still be undone).
    pub fn of(kind: Kind, outcome: &Outcome) -> Option<Change> {
        let c = match kind {
            Kind::Move | Kind::PutBack => Change::Moved(outcome.moved.clone()),
            Kind::Trash => Change::Trashed(outcome.moved.clone()),
            Kind::Restore => Change::Restored(outcome.moved.clone()),
            Kind::Copy | Kind::Duplicate => Change::Made(outcome.created.clone()),
            Kind::EmptyTrash => return None,
        };
        (!c.is_empty()).then_some(c)
    }
}

pub use olo_ui::undo::{History, Purpose};

/// What a job of `kind` is called as a step.
pub fn name_of(kind: Kind) -> &'static str {
    match kind {
        Kind::Copy => "Copy",
        Kind::Move => "Move",
        Kind::Duplicate => "Duplicate",
        Kind::Trash => "Move to Trash",
        Kind::Restore => "Restore",
        Kind::EmptyTrash => "Empty Trash",
        Kind::PutBack => "Move",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops;
    use std::{fs as sfs, path::Path, sync::mpsc};

    fn run(kind: Kind, sources: Vec<PathBuf>, dest: Option<PathBuf>, targets: Vec<PathBuf>) -> Outcome {
        let (tx, rx) = mpsc::channel();
        ops::start(1, kind, sources, dest, targets, move |o| tx.send(o).unwrap());
        rx.recv().unwrap()
    }

    /// Undo or redo the top step as the app does: its inverse run, what
    /// that did recorded for the other list.
    fn turn(h: &mut History<Change>, redo: bool) -> Outcome {
        let step = if redo { h.take_redo() } else { h.take_undo() }.expect("a step");
        let (kind, sources, targets) = step.change.inverse().expect("a job");
        let o = run(kind, sources, None, targets);
        if let Some(change) = Change::of(kind, &o) {
            h.record(if redo { Purpose::Redo } else { Purpose::Undo }, step.name, change);
        }
        o
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("olo-undo-{name}-{}", std::process::id()));
        let _ = sfs::remove_dir_all(&d);
        sfs::create_dir_all(&d).unwrap();
        d
    }

    /// A move of many files undone puts each back where it was, and redone
    /// moves them again, as often as wanted; the menu names it.
    #[test]
    fn a_move_goes_back_and_forth() {
        let d = dir("move");
        sfs::create_dir_all(d.join("a")).unwrap();
        sfs::create_dir_all(d.join("b")).unwrap();
        for n in ["x.txt", "y.txt"] {
            sfs::write(d.join("a").join(n), n).unwrap();
        }
        let mut h = History::default();
        assert_eq!((h.undo_label(), h.can_undo()), ("Undo".into(), false));
        let o = run(Kind::Move, vec![d.join("a/x.txt"), d.join("a/y.txt")], Some(d.join("b")), Vec::new());
        h.done(name_of(Kind::Move), Change::of(Kind::Move, &o).unwrap());
        assert_eq!(h.undo_label(), "Undo Move");
        for _ in 0..2 {
            let o = turn(&mut h, false);
            assert!(o.error.is_none(), "{o:?}");
            assert!(d.join("a/x.txt").exists() && d.join("a/y.txt").exists() && !d.join("b/x.txt").exists());
            assert_eq!(h.redo_label(), "Redo Move");
            assert!(!h.can_undo());
            let o = turn(&mut h, true);
            assert!(o.error.is_none(), "{o:?}");
            assert!(d.join("b/x.txt").exists() && d.join("b/y.txt").exists() && !d.join("a/x.txt").exists());
        }
        let _ = sfs::remove_dir_all(&d);
    }

    /// A rename undone is the old name back; not over something that has
    /// taken it meanwhile, and not of something gone: said, and nothing
    /// touched.
    #[test]
    fn a_rename_undone_or_said_why_not() {
        let d = dir("rename");
        sfs::write(d.join("new.txt"), "it").unwrap();
        let mut h = History::default();
        h.done("Rename", Change::Moved(vec![(d.join("old.txt"), d.join("new.txt"))]));
        let o = turn(&mut h, false);
        assert!(o.error.is_none(), "{o:?}");
        assert_eq!(sfs::read_to_string(d.join("old.txt")).unwrap(), "it");
        // Redone, then something else takes the old name: Undo says so.
        turn(&mut h, true);
        sfs::write(d.join("old.txt"), "another").unwrap();
        let o = turn(&mut h, false);
        assert!(o.error.as_deref().is_some_and(|e| e.contains("something else is called “old.txt”")), "{o:?}");
        assert_eq!(sfs::read_to_string(d.join("old.txt")).unwrap(), "another");
        assert_eq!(sfs::read_to_string(d.join("new.txt")).unwrap(), "it");
        // Gone: said too.
        h.done("Rename", Change::Moved(vec![(d.join("a"), d.join("gone"))]));
        let o = turn(&mut h, false);
        assert!(o.error.as_deref().is_some_and(|e| e.contains("isn't there any more")), "{o:?}");
        let _ = sfs::remove_dir_all(&d);
    }

    /// Icons put back where they were: what that did is the same places
    /// the other way round, so Redo puts them where they were dragged.
    #[test]
    fn icons_back_and_forth() {
        let moved = vec![("a.txt".to_owned(), Some([10.0, 20.0]), Some([300.0, 40.0])), ("b".to_owned(), None, Some([5.0, 5.0]))];
        let Change::Placed(back) = Change::swapped(&moved) else { panic!() };
        assert_eq!(back, vec![("a.txt".to_owned(), Some([300.0, 40.0]), Some([10.0, 20.0])), ("b".to_owned(), Some([5.0, 5.0]), None)]);
        assert!(Change::Placed(moved).inverse().is_none());
    }

    /// The inverse of each kind of step: moves back, the Trash's restores,
    /// what was made to the Trash.
    #[test]
    fn inverses() {
        let (a, b) = (Path::new("/a").to_path_buf(), Path::new("/t/a").to_path_buf());
        assert_eq!(Change::Moved(vec![(a.clone(), b.clone())]).inverse().unwrap(), (Kind::PutBack, vec![b.clone()], vec![a.clone()]));
        assert_eq!(Change::Trashed(vec![(a.clone(), b.clone())]).inverse().unwrap(), (Kind::Restore, vec![b.clone()], vec![]));
        assert_eq!(Change::Restored(vec![(b.clone(), a.clone())]).inverse().unwrap(), (Kind::Trash, vec![a.clone()], vec![]));
        assert_eq!(Change::Made(vec![a.clone()]).inverse().unwrap(), (Kind::Trash, vec![a], vec![]));
    }
}
