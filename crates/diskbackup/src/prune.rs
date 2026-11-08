//! Tidying (`prune`): what no snapshot uses any more let go of, once
//! snapshots past keeping (`keep`) are forgotten.
//!
//! 1. What every snapshot uses: its trees and their files' chunks (a tree
//!    seen once is enough: an unchanged folder is the same tree in every
//!    snapshot, and trees are read from the cache, not the store).
//! 2. Each pack, from the index: nothing in it used, it goes; much of it
//!    unused (30%), or one of many small ones (a backup's last, partly
//!    filled), what's used in it is copied into new packs (still sealed:
//!    nothing opened but to check it's whole) and it goes. A blob kept
//!    twice is kept once, in the pack most used.
//! 3. The index files naming a pack that goes are written again (the rest
//!    stay as they are: no index rewritten to drop nothing); then they go;
//!    then the packs. Stopped anywhere, nothing is lost: what was written
//!    and not yet named goes at the next tidy.
//!
//! It needs the backup to itself (an exclusive lock): no computer backing
//! up or restoring meanwhile.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::atomic::{AtomicBool, Ordering::Relaxed},
    time::Instant,
};

use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    backup::Progress,
    crypto::Id,
    repo::{DATA, Entry, PACK_TARGET, Repo, TREE, TREES, decode, pack_name},
};

/// What tidying did.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Pruned {
    /// Packs gone whole (nothing in them used), and copied from (partly
    /// used, or small), and written.
    pub packs_deleted: u64,
    pub packs_repacked: u64,
    pub packs_written: u64,
    /// What went from the store, what was fetched to copy, and what went up.
    pub bytes_deleted: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
    /// The store's size after (all but locks).
    pub size: u64,
    /// Blobs a snapshot uses that aren't anywhere (damage: the next backup
    /// puts back what it still has).
    pub missing: u64,
    pub seconds: f64,
}

/// A pack this much unused (tenths) is copied from.
const SPARSE_TENTHS: u64 = 3;
/// Packs under this size are put together once there are `SMALL_COUNT`.
const SMALL: u64 = (PACK_TARGET / 4) as u64;
const SMALL_COUNT: usize = 8;
/// An index file holds at most this many blobs; smaller ones are put
/// together once there are `SMALL_COUNT`.
const INDEX_FILE: usize = 65_536;

fn say(progress: &Progress, what: impl Into<String>) {
    if let Ok(mut c) = progress.current.lock() {
        *c = what.into();
    }
}

/// Tidy `repo` (which must hold its exclusive lock: [`Repo::lock`]).
pub fn prune(repo: &mut Repo, cancel: &AtomicBool, progress: &Progress) -> Result<Pruned> {
    let r = run(repo, cancel, progress);
    // The index read again as it is now, whatever happened.
    repo.checkpoints = true;
    repo.written.clear();
    let reread = repo.reload();
    let pruned = r?;
    reread?;
    sweep_cache(repo);
    Ok(pruned)
}

/// An index file's packs, and what's in each.
type Packs = Vec<(Id, Vec<Entry>)>;

struct Pack {
    id: Id,
    size: u64,
    entries: Vec<Entry>,
    /// What's kept of it (used, and not kept elsewhere).
    keep: Vec<Entry>,
    trees: bool,
}

fn run(repo: &mut Repo, cancel: &AtomicBool, progress: &Progress) -> Result<Pruned> {
    let started = Instant::now();
    repo.check_exclusive()?;
    repo.flush()?;
    let stop = || if cancel.load(Relaxed) { Err(Error::Cancelled) } else { Ok(()) };
    let mut out = Pruned::default();

    // 1. What the snapshots use.
    say(progress, "Looking at what the snapshots use");
    let mut used: HashSet<Id> = HashSet::new();
    let mut todo: Vec<Id> = repo.snapshots()?.iter().map(|(_, s)| s.tree).collect();
    while let Some(t) = todo.pop() {
        if !used.insert(t) {
            continue;
        }
        stop()?;
        let tree = repo.tree(&t)?;
        progress.files.fetch_add(1, Relaxed);
        for n in tree.nodes {
            used.extend(n.content);
            todo.extend(n.subtree);
        }
    }

    // 2. The index as the store has it, and the packs there.
    say(progress, "Looking at what's kept");
    let index_files = repo.store.list("index")?;
    let mut files: Vec<(String, Packs)> = Vec::new();
    let mut by_pack: HashMap<Id, Vec<Entry>> = HashMap::new();
    for (name, _) in &index_files {
        let packs: Vec<(Id, Vec<Entry>)> = decode(&repo.cached(name)?)?;
        for (p, es) in &packs {
            by_pack.entry(*p).or_default().extend_from_slice(es);
        }
        files.push((name.clone(), packs));
    }
    let present: HashMap<Id, u64> = repo.store.list("data")?.into_iter().filter_map(|(n, size)| n.rsplit('/').next().and_then(Id::from_hex).map(|id| (id, size))).collect();
    let mut gone: HashSet<Id> = HashSet::new();
    let mut packs: Vec<Pack> = Vec::new();
    for (id, mut entries) in by_pack {
        let Some(size) = present.get(&id) else {
            // Named, but not there (damage): what's in it isn't anywhere.
            gone.insert(id);
            continue;
        };
        entries.sort_by_key(|e| e.offset);
        entries.dedup_by_key(|e| e.offset);
        let trees = entries.iter().all(|e| e.flags & TREE != 0);
        packs.push(Pack { id, size: *size, entries, keep: Vec::new(), trees });
    }
    // Each used blob kept once: in the pack most used, first.
    let used_of = |p: &Pack| p.entries.iter().filter(|e| used.contains(&e.id)).map(|e| e.len as u64).sum::<u64>();
    packs.sort_by_cached_key(|p| std::cmp::Reverse((used_of(p) as u128 * 1_000_000 / p.size.max(1) as u128, p.id.0)));
    let mut claimed: HashSet<Id> = HashSet::new();
    for p in &mut packs {
        p.keep = p.entries.iter().filter(|e| used.contains(&e.id) && claimed.insert(e.id)).copied().collect();
    }
    out.missing = used.iter().filter(|id| !claimed.contains(id)).count() as u64;

    // What to do with each pack.
    let (mut delete, mut repack, mut small) = (Vec::new(), Vec::new(), Vec::new());
    for (i, p) in packs.iter().enumerate() {
        let all: u64 = p.entries.iter().map(|e| e.len as u64).sum();
        let kept: u64 = p.keep.iter().map(|e| e.len as u64).sum();
        if p.keep.is_empty() {
            delete.push(i);
        } else if (all - kept) * 10 > all * SPARSE_TENTHS {
            repack.push(i);
        } else if p.size < SMALL {
            small.push(i);
        }
    }
    if small.len() >= SMALL_COUNT {
        repack.extend(small);
    }
    // In no index (a backup stopped before naming them, a tidy before):
    // nobody's, with the backup held.
    let indexed: HashSet<Id> = packs.iter().map(|p| p.id).chain(gone.iter().copied()).collect();
    let unindexed: Vec<(Id, u64)> = present.iter().filter(|(id, _)| !indexed.contains(id)).map(|(id, s)| (*id, *s)).collect();
    // The index files to write again: those naming a pack that goes (or is
    // gone), and the small ones once there are many.
    let going: HashSet<Id> = delete.iter().chain(repack.iter()).map(|i| packs[*i].id).chain(gone.iter().copied()).collect();
    let small_files: Vec<usize> = files.iter().enumerate().filter(|(_, (_, ps))| ps.iter().map(|(_, es)| es.len()).sum::<usize>() < INDEX_FILE / 4).map(|(i, _)| i).collect();
    let rewrite: HashSet<usize> =
        files.iter().enumerate().filter(|(i, (_, ps))| ps.iter().any(|(p, _)| going.contains(p)) || (small_files.len() >= SMALL_COUNT && small_files.contains(i))).map(|(i, _)| i).collect();
    if going.is_empty() && unindexed.is_empty() && rewrite.is_empty() {
        out.size = stored_size(repo)?;
        out.seconds = started.elapsed().as_secs_f64();
        return Ok(out);
    }

    // 3. What's used of each pack copied from, into new ones.
    repo.checkpoints = false;
    let stored = repo.stored;
    for (k, i) in repack.iter().enumerate() {
        stop()?;
        say(progress, format!("Putting packs together ({} of {})", k + 1, repack.len()));
        let p = &packs[*i];
        let whole = repo.pack_bytes(&p.id, p.trees)?;
        out.bytes_read += whole.len() as u64;
        progress.bytes.fetch_add(whole.len() as u64, Relaxed);
        for e in &p.keep {
            match whole.get(e.offset as usize..(e.offset + e.len) as usize) {
                // (Whole, as sealed: copied without opening it further.)
                Some(sealed) if repo.key.open(sealed).is_some() => repo.put_sealed(*e, sealed)?,
                _ => out.missing += 1,
            }
        }
    }
    repo.finish_pack(DATA)?;
    repo.finish_pack(TREES)?;
    let new_packs = std::mem::take(&mut repo.written);
    out.packs_written = new_packs.len() as u64;
    out.packs_repacked = repack.len() as u64;

    // 4. The index files naming what goes, written again: what's kept of
    // each pack that stays, and the new packs.
    stop()?;
    say(progress, "Writing the index");
    let kept: HashSet<(Id, u32)> = packs.iter().filter(|p| !going.contains(&p.id)).flat_map(|p| p.keep.iter().map(move |e| (p.id, e.offset))).collect();
    let mut merged: BTreeMap<Id, Vec<Entry>> = BTreeMap::new();
    for i in &rewrite {
        for (p, es) in &files[*i].1 {
            if going.contains(p) || !present.contains_key(p) {
                continue;
            }
            merged.entry(*p).or_default().extend(es.iter().filter(|e| kept.contains(&(*p, e.offset))));
        }
    }
    let mut all: Vec<(Id, Vec<Entry>)> = merged
        .into_iter()
        .map(|(p, mut es)| {
            es.sort_by_key(|e| e.offset);
            es.dedup_by_key(|e| e.offset);
            (p, es)
        })
        .filter(|(_, es)| !es.is_empty())
        .collect();
    all.extend(new_packs);
    let mut written: HashSet<String> = HashSet::new();
    let mut batch: Vec<(Id, Vec<Entry>)> = Vec::new();
    let mut count = 0;
    for (p, es) in all {
        if count > 0 && count + es.len() > INDEX_FILE {
            repo.check_exclusive()?;
            written.insert(repo.put_index(&batch)?);
            batch.clear();
            count = 0;
        }
        count += es.len();
        batch.push((p, es));
    }
    if !batch.is_empty() {
        repo.check_exclusive()?;
        written.insert(repo.put_index(&batch)?);
    }
    out.bytes_written = repo.stored - stored;

    // 5. The old index files, then the packs (nothing names them now).
    say(progress, "Letting go of what isn't used");
    for i in &rewrite {
        let name = &files[*i].0;
        if !written.contains(name) {
            repo.check_exclusive()?;
            repo.store.delete(name)?;
        }
    }
    out.packs_deleted = (delete.len() + unindexed.len()) as u64;
    let doomed = delete.iter().chain(repack.iter()).map(|i| (packs[*i].id, packs[*i].size)).chain(unindexed);
    for (id, size) in doomed {
        repo.check_exclusive()?;
        repo.store.delete(&pack_name(&id))?;
        out.bytes_deleted += size;
    }
    out.size = stored_size(repo)?;
    out.seconds = started.elapsed().as_secs_f64();
    Ok(out)
}

/// What the store holds (but locks).
fn stored_size(repo: &Repo) -> Result<u64> {
    Ok(repo.store.list("")?.iter().filter(|(n, _)| !n.starts_with("locks/")).map(|(_, s)| s).sum())
}

/// The cache's copies of what isn't in the store any more, gone.
fn sweep_cache(repo: &Repo) {
    let Some(cache) = &repo.cache else { return };
    let Ok(index) = repo.store.list("index") else { return };
    let mut keep: HashSet<std::path::PathBuf> = index.into_iter().map(|(n, _)| cache.join(n)).collect();
    keep.extend(repo.pack_ids().filter(|(_, trees)| *trees).map(|(id, _)| cache.join(pack_name(id))));
    fn walk(at: &std::path::Path, keep: &HashSet<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(at) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                walk(&p, keep);
            } else if !keep.contains(&p) {
                let _ = std::fs::remove_file(&p);
            }
        }
        // (Emptied: gone too.)
        let _ = std::fs::remove_dir(at);
    }
    walk(&cache.join("index"), &keep);
    walk(&cache.join("data"), &keep);
}
