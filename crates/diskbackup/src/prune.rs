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
//!
//! A pack no index names (a backup stopped before naming it, an index file
//! lost) is read back from its own list first, so what the snapshots still
//! use in it is kept, and it's named again; only one nothing uses goes.
//! Unfinished snapshots count as snapshots here: what a backup stopped
//! part way sent stays, for the next to go on from.
//!
//! 3. The index files naming a pack that goes are written again (the rest
//!    stay as they are: no index rewritten to drop nothing); then they go;
//!    then the packs. Stopped anywhere, nothing is lost: what was written
//!    and not yet named goes at the next tidy.
//!
//! It needs the backup to itself (an exclusive lock): no computer backing
//! up or restoring meanwhile.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
    },
    time::Instant,
};

use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    backup::Progress,
    crypto::Id,
    repo::{DATA, Entry, Repo, TREE, TREES, decode, pack_name},
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
    /// Packs no index named that the snapshots still use, read back from
    /// their own lists and named again.
    #[serde(default)]
    pub packs_found: u64,
    /// Index files that didn't open or read (damaged), let go of once
    /// their packs were named again.
    #[serde(default)]
    pub index_files_damaged: u64,
    pub seconds: f64,
}

/// A pack this much unused (tenths) is copied from.
const SPARSE_TENTHS: u64 = 3;
/// Packs under a quarter of their kind's size are put together once there
/// are `SMALL_COUNT`.
const SMALL_COUNT: usize = 8;
/// An index file holds at most this many blobs; smaller ones are put
/// together once there are `SMALL_COUNT`.
const INDEX_FILE: usize = 65_536;

/// Tidying's steps (`Progress::step`), each said as it starts, with how
/// many of how many when it knows (`files` of `total_files`; deleting,
/// `bytes` freed of `total_bytes` too).
pub const STEPS: u8 = 5;

/// Step `n`, `what` it is, starting (its counts from nothing).
fn step(progress: &Progress, n: u8, what: &str) {
    progress.step.store(n, Relaxed);
    for c in [&progress.files, &progress.total_files, &progress.bytes, &progress.total_bytes] {
        c.store(0, Relaxed);
    }
    say(progress, what);
}

fn say(progress: &Progress, what: impl Into<String>) {
    if let Ok(mut c) = progress.current.lock() {
        *c = what.into();
    }
}

/// Workers deleting at once: a slow store's requests overlap (Google
/// Drive's take a few hundred milliseconds each, one after another).
const DELETERS: usize = 8;

/// `names` (each with its size) deleted from the store, `DELETERS` at a
/// time, said as they go (how many of how many, bytes freed of how many);
/// each only while tidying still has the backup to itself. Stopped, or one
/// failing: the rest left for the next tidy (nothing names them by then).
/// The bytes freed.
fn delete_all(repo: &Repo, names: &[(String, u64)], cancel: &AtomicBool, progress: &Progress) -> Result<u64> {
    if names.is_empty() {
        return Ok(0);
    }
    let held = repo.exclusive()?;
    let store = repo.store.clone();
    progress.total_files.store(names.len() as u64, Relaxed);
    progress.total_bytes.store(names.iter().map(|(_, s)| s).sum(), Relaxed);
    let next = AtomicUsize::new(0);
    let failed: Mutex<Option<Error>> = Mutex::new(None);
    std::thread::scope(|s| {
        for _ in 0..DELETERS.min(names.len()) {
            s.spawn(|| {
                loop {
                    if cancel.load(Relaxed) || failed.lock().map_or(true, |f| f.is_some()) {
                        break;
                    }
                    let Some((name, size)) = names.get(next.fetch_add(1, Relaxed)) else { break };
                    match held.check().and_then(|()| store.delete(name).map_err(Error::from)) {
                        Ok(()) => {
                            progress.files.fetch_add(1, Relaxed);
                            progress.bytes.fetch_add(*size, Relaxed);
                        }
                        Err(e) => {
                            if let Ok(mut f) = failed.lock() {
                                f.get_or_insert(e);
                            }
                            break;
                        }
                    }
                }
            });
        }
    });
    if let Some(e) = failed.into_inner().ok().flatten() {
        return Err(e);
    }
    if cancel.load(Relaxed) {
        return Err(Error::Cancelled);
    }
    Ok(progress.bytes.load(Relaxed))
}

/// Tidy `repo` (which must hold its exclusive lock: [`Repo::lock`]).
pub fn prune(repo: &mut Repo, cancel: &AtomicBool, progress: &Progress) -> Result<Pruned> {
    repair(repo, &HashSet::new(), cancel, progress)
}

/// Tidy `repo`, `damaged` packs (a check found them not as written)
/// copied from whatever its blobs still open whole, then let go of; a
/// blob kept elsewhere too is kept from there. What's lost, the next
/// backup sends again, of the files still as they were.
pub fn repair(repo: &mut Repo, damaged: &HashSet<Id>, cancel: &AtomicBool, progress: &Progress) -> Result<Pruned> {
    let r = run(repo, damaged, cancel, progress);
    repo.let_go();
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
    /// Named in no index: read back from its own list.
    found: bool,
}

fn run(repo: &mut Repo, damaged_packs: &HashSet<Id>, cancel: &AtomicBool, progress: &Progress) -> Result<Pruned> {
    let started = Instant::now();
    repo.check_exclusive()?;
    repo.flush()?;
    let stop = || if cancel.load(Relaxed) { Err(Error::Cancelled) } else { Ok(()) };
    let mut out = Pruned::default();

    // 1. The index as the store has it, and the packs there.
    step(progress, 1, "Looking at what's kept");
    let index_files = repo.store.list("index")?;
    let mut files: Vec<(String, Packs)> = Vec::new();
    let mut by_pack: HashMap<Id, Vec<Entry>> = HashMap::new();
    // (One that doesn't open or read names nothing: its packs are found
    // below, read back from their own lists, and it goes at the end.)
    let mut damaged: Vec<&String> = Vec::new();
    for (name, _) in &index_files {
        let packs: Vec<(Id, Vec<Entry>)> = match repo.cached(name).and_then(|plain| decode(&plain)) {
            Ok(packs) => packs,
            Err(Error::Damaged(_)) => {
                damaged.push(name);
                continue;
            }
            Err(e) => return Err(e),
        };
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
        packs.push(Pack { id, size: *size, entries, keep: Vec::new(), trees, found: false });
    }
    // Packs no index names (a backup stopped before naming them; an index
    // file lost): each read back from its own list, and taken into the
    // index here, so what the snapshots still use in them is found below
    // (and kept), never let go of with the rest. One whose list is
    // damaged is nobody's; one that couldn't be fetched (the network) stops
    // the tidying, to be tried again, rather than be let go of.
    let named: HashSet<Id> = packs.iter().map(|p| p.id).collect();
    let mut unreadable: Vec<(Id, u64)> = Vec::new();
    let unnamed: Vec<(&Id, &u64)> = present.iter().filter(|(id, _)| !named.contains(id)).collect();
    if !unnamed.is_empty() {
        say(progress, "Reading the lists of packs no index names");
        progress.total_files.store(unnamed.len() as u64, Relaxed);
    }
    for (id, size) in unnamed {
        stop()?;
        progress.files.fetch_add(1, Relaxed);
        match repo.pack_header(id, *size) {
            Ok(mut entries) => {
                entries.sort_by_key(|e| e.offset);
                let trees = entries.iter().all(|e| e.flags & TREE != 0);
                repo.take_pack(*id, &entries);
                packs.push(Pack { id: *id, size: *size, entries, keep: Vec::new(), trees, found: true });
            }
            Err(Error::Damaged(_)) => unreadable.push((*id, *size)),
            Err(e) => return Err(e),
        }
    }

    // 2. What the snapshots use (unfinished ones too: the next backup goes
    // on from them).
    step(progress, 2, "Looking at what the backups use");
    let used = used(repo, cancel, progress)?;

    // Each used blob kept once: in the pack most used, first.
    let used_of = |p: &Pack| p.entries.iter().filter(|e| used.contains(&e.id)).map(|e| e.len as u64).sum::<u64>();
    // (A damaged pack's last: a blob it holds is kept from another, if one
    // has it.)
    packs.sort_by_cached_key(|p| (damaged_packs.contains(&p.id), std::cmp::Reverse((used_of(p) as u128 * 1_000_000 / p.size.max(1) as u128, p.id.0))));
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
        } else if damaged_packs.contains(&p.id) || (all - kept) * 10 > all * SPARSE_TENTHS {
            repack.push(i);
        } else if p.size < (repo.pack_size(p.trees) / 4) as u64 {
            small.push(i);
        }
    }
    if small.len() >= SMALL_COUNT {
        repack.extend(small);
    }
    // Found again and kept: named in the index written below.
    let found: Vec<usize> = (0..packs.len()).filter(|i| packs[*i].found && !delete.contains(i) && !repack.contains(i)).collect();
    out.packs_found = found.len() as u64;
    out.index_files_damaged = damaged.len() as u64;
    let unindexed = unreadable;
    // The index files to write again: those naming a pack that goes (or is
    // gone), and the small ones once there are many.
    let going: HashSet<Id> = delete.iter().chain(repack.iter()).map(|i| packs[*i].id).chain(gone.iter().copied()).collect();
    let small_files: Vec<usize> = files.iter().enumerate().filter(|(_, (_, ps))| ps.iter().map(|(_, es)| es.len()).sum::<usize>() < INDEX_FILE / 4).map(|(i, _)| i).collect();
    let rewrite: HashSet<usize> =
        files.iter().enumerate().filter(|(i, (_, ps))| ps.iter().any(|(p, _)| going.contains(p)) || (small_files.len() >= SMALL_COUNT && small_files.contains(i))).map(|(i, _)| i).collect();
    if going.is_empty() && unindexed.is_empty() && rewrite.is_empty() && found.is_empty() && damaged.is_empty() {
        out.size = stored_size(repo)?;
        out.seconds = started.elapsed().as_secs_f64();
        return Ok(out);
    }

    // 3. What's used of each pack copied from, into new ones.
    repo.checkpoints = false;
    let stored = repo.stored;
    step(progress, 3, "Copying what's still used out of packs mostly unused");
    progress.total_files.store(repack.len() as u64, Relaxed);
    for i in repack.iter() {
        stop()?;
        let p = &packs[*i];
        let whole = repo.pack_bytes(&p.id, p.trees)?;
        out.bytes_read += whole.len() as u64;
        progress.files.fetch_add(1, Relaxed);
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
    step(progress, 4, "Writing the index");
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
    all.extend(found.iter().map(|i| (packs[*i].id, packs[*i].keep.clone())).filter(|(_, es)| !es.is_empty()));
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

    // 5. The old index files, then the packs (nothing names them now),
    // several at once.
    step(progress, 5, "Deleting what isn't used");
    let old: Vec<(String, u64)> = rewrite.iter().map(|i| files[*i].0.clone()).chain(damaged.iter().map(|n| (*n).clone())).filter(|n| !written.contains(n)).map(|n| (n, 0)).collect();
    delete_all(repo, &old, cancel, progress)?;
    out.packs_deleted = (delete.len() + unindexed.len()) as u64;
    let doomed: Vec<(String, u64)> = delete.iter().chain(repack.iter()).map(|i| (pack_name(&packs[*i].id), packs[*i].size)).chain(unindexed.iter().map(|(id, size)| (pack_name(id), *size))).collect();
    step(progress, 5, "Deleting what isn't used");
    out.bytes_deleted = delete_all(repo, &doomed, cancel, progress)?;
    out.size = stored_size(repo)?;
    out.seconds = started.elapsed().as_secs_f64();
    Ok(out)
}

/// Every blob a snapshot uses, finished or not: its trees and their files'
/// chunks (a tree seen once is enough), and an unfinished one's chunks of
/// the file it was part way through.
fn used(repo: &Repo, cancel: &AtomicBool, progress: &Progress) -> Result<HashSet<Id>> {
    let mut used: HashSet<Id> = HashSet::new();
    let unfinished = repo.unfinished()?;
    let mut todo: Vec<Id> = repo.snapshots()?.iter().chain(&unfinished).map(|(_, s)| s.tree).collect();
    for (_, s) in &unfinished {
        used.extend(s.extra.iter().copied());
    }
    while let Some(t) = todo.pop() {
        if !used.insert(t) {
            continue;
        }
        if cancel.load(Relaxed) {
            return Err(Error::Cancelled);
        }
        let tree = repo.tree(&t)?;
        progress.files.fetch_add(1, Relaxed);
        for n in tree.nodes {
            used.extend(n.content);
            todo.extend(n.subtree);
        }
    }
    Ok(used)
}

/// What the index names that no snapshot (finished or not) uses, bytes as
/// kept: about what tidying frees (a backup that stopped before it kept
/// anything of its own leaves that much). Read from the cache's trees,
/// not the store.
pub fn unused(repo: &Repo, cancel: &AtomicBool) -> Result<u64> {
    let used = used(repo, cancel, &Progress::default())?;
    Ok(repo.blob_sizes().filter(|(id, _)| !used.contains(*id)).map(|(_, n)| n).sum())
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
