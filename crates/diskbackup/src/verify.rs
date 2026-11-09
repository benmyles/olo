//! Checking a backup without restoring it (Ben, 2026-11-08: "backups also
//! need to be validatable and should be validated at the end of each
//! backup but i should be able to manually trigger a re-validate of any
//! backup at any time"): that everything it needs is in the store, whole.
//!
//! - Fast (after each backup, and Validate's unless asked for Full; Ben,
//!   2026-11-08: "add a fast validate option which compares checksums and
//!   just spot-checks some samples … fast validate should be the default
//!   and the one that is done automatically after a backup"): every
//!   folder's list it needs named by the index and read back from the
//!   store itself, not the local cache (each tree pack fetched whole and
//!   checked against its id, a keyed hash of all of it: exactly as
//!   written; one checked before, by this computer, isn't fetched again);
//!   every chunk its files need named by the index; every pack holding them
//!   in the store, as long as it was written; where the store keeps a
//!   checksum of each (Google Drive's MD5), each pack's compared with the
//!   one summed as it was sent (`sums`, in the cache: it arrived and stayed
//!   as sent, nothing fetched; one that differs is fetched and checked);
//!   and a few of its data packs read back whole (`SAMPLES`: about one in
//!   a hundred, one to four), those never read back first, so over time
//!   the samples go round them all.
//! - Full (Validate Fully): every pack it uses fetched and checked that
//!   way, whatever was checked before, and each blob of the backup's in it
//!   opened and checked against its own id, as a restore would.
//!
//! What's wrong is said plainly, with (the first few of) the files it
//! leaves unable to come back as they were; the packs missing or damaged
//! are named, for tidying to let go of (what's whole in a damaged one
//! salvaged), so the next backup sends again what it still can.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering::Relaxed},
    time::Instant,
};

use crate::{
    Error, Result,
    backup::Progress,
    crypto::Id,
    repo::{Repo, Snapshot, TREE, pack_name, unseal},
};

/// What checking a backup found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Checked {
    pub trees: u64,
    pub chunks: u64,
    /// Packs it uses, and how many were fetched and checked whole.
    pub packs: u64,
    pub packs_read: u64,
    pub bytes_read: u64,
    /// Packs the store's checksum showed as sent (fast), and data packs
    /// read back as samples.
    pub by_checksum: u64,
    pub sampled: u64,
    /// What's wrong, said plainly (none: it restores whole).
    pub problems: Vec<String>,
    /// Files (and folders) that can't come back as they were: the first
    /// few, and how many in all.
    pub files: Vec<String>,
    pub files_affected: u64,
    /// Packs missing from the store or damaged there.
    pub bad_packs: Vec<Id>,
    pub seconds: f64,
}

/// "one of its packs is", "3 of its packs are".
fn packs(n: u64) -> String {
    if n == 1 { "one of its packs is".into() } else { format!("{n} of its packs are") }
}

/// Data packs read back as samples (fast): one in this many, …
const SAMPLE_ONE_IN: usize = 100;
/// … at most this many.
const SAMPLES: usize = 4;

/// A blob where it's kept: its id, offset, length (sealed) and flags.
type Place = (Id, u32, u32, u8);

/// How many affected files are named.
const NAMED: usize = 12;

/// The packs this computer has checked whole before (in the cache, one id
/// a line): a pack never changes, so one checked needn't be fetched again
/// after the next backup.
fn checked_before(repo: &Repo) -> (Option<PathBuf>, HashSet<Id>) {
    let Some(path) = repo.cache.as_ref().map(|c| c.join("checked")) else { return (None, HashSet::new()) };
    let ids = std::fs::read_to_string(&path).unwrap_or_default().lines().filter_map(Id::from_hex).collect();
    (Some(path), ids)
}

/// The checksums of the packs this computer sent (or read back whole), by
/// pack (in the cache's `sums`, a pack and its MD5 a line; the last line
/// for a pack says it).
fn sums(repo: &Repo) -> HashMap<Id, String> {
    let Some(path) = repo.cache.as_ref().map(|c| c.join("sums")) else { return HashMap::new() };
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (id, sum) = l.split_once(' ')?;
            Some((Id::from_hex(id)?, sum.to_owned()))
        })
        .collect()
}

/// `pack`'s checksum (as `data`, its bytes, are), kept in the cache's
/// `sums`: to compare with the store's when it's checked.
pub(crate) fn note_sum(repo: &Repo, pack: &Id, data: &[u8]) {
    use md5::Digest as _;
    use std::io::Write as _;
    let Some(path) = repo.cache.as_ref().map(|c| c.join("sums")) else { return };
    let sum: String = md5::Md5::digest(data).iter().map(|b| format!("{b:02x}")).collect();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{} {sum}", pack.hex());
    }
}

/// `v` in a random order.
fn shuffled<T>(mut v: Vec<T>) -> Vec<T> {
    for i in (1..v.len()).rev() {
        let mut r = [0u8; 8];
        let _ = getrandom::fill(&mut r);
        v.swap(i, (u64::from_le_bytes(r) % (i as u64 + 1)) as usize);
    }
    v
}

/// Check `snapshot` (see the module's words): fast, or `full` (every byte
/// read back).
pub fn check(repo: &Repo, snapshot: &Snapshot, full: bool, cancel: &AtomicBool, progress: &Progress) -> Result<Checked> {
    let started = Instant::now();
    let mut out = Checked::default();
    let stop = || if cancel.load(Relaxed) { Err(Error::Cancelled) } else { Ok(()) };
    let say = |what: &str| {
        if let Ok(mut c) = progress.current.lock() {
            *c = what.to_owned();
        }
    };

    // 1. Its folders' lists (from the cache) and what they need.
    say("Reading its lists");
    let mut trees: HashSet<Id> = HashSet::new();
    let mut chunks: HashSet<Id> = snapshot.extra.iter().copied().collect();
    let mut unreadable: HashSet<Id> = HashSet::new();
    let mut todo = vec![snapshot.tree];
    while let Some(t) = todo.pop() {
        if !trees.insert(t) {
            continue;
        }
        stop()?;
        match repo.tree(&t) {
            Ok(tree) => {
                for n in tree.nodes {
                    chunks.extend(n.content);
                    todo.extend(n.subtree);
                }
            }
            // (Its own pack, below, says why.)
            Err(Error::Damaged(_)) => {
                unreadable.insert(t);
            }
            Err(e) => return Err(e),
        }
    }
    out.trees = trees.len() as u64;
    out.chunks = chunks.len() as u64;

    // 2. Where each is, by pack.
    let mut bad: HashSet<Id> = HashSet::new();
    let mut by_pack: HashMap<Id, Vec<Place>> = HashMap::new();
    let mut not_indexed = 0u64;
    for id in trees.iter().chain(chunks.iter()) {
        match repo.locate(id) {
            Some((pack, offset, len, flags)) => by_pack.entry(pack).or_default().push((*id, offset, len, flags)),
            None => {
                not_indexed += 1;
                bad.insert(*id);
            }
        }
    }
    if not_indexed > 0 {
        out.problems.push(if not_indexed == 1 { "a piece it needs isn’t in the index".into() } else { format!("{not_indexed} pieces it needs aren’t in the index") });
    }
    out.packs = by_pack.len() as u64;

    // 3. Each pack there, as long as it was written.
    say("Looking at what the store holds");
    let present: HashMap<Id, u64> = repo.store.list("data")?.into_iter().filter_map(|(n, size)| n.rsplit('/').next().and_then(Id::from_hex).map(|id| (id, size))).collect();
    let (mut missing, mut short) = (0u64, 0u64);
    for (pack, blobs) in &by_pack {
        let end = blobs.iter().map(|(_, o, l, _)| *o as u64 + *l as u64).max().unwrap_or(0);
        match present.get(pack) {
            None => missing += 1,
            Some(size) if *size < end + 4 => short += 1,
            Some(_) => continue,
        }
        out.bad_packs.push(*pack);
        bad.extend(blobs.iter().map(|b| b.0));
    }
    if missing > 0 {
        out.problems.push(format!("{} missing", packs(missing)));
    }
    if short > 0 {
        out.problems.push(format!("{} cut short", packs(short)));
    }

    // 4. Fast: the store's checksum of each pack against the one summed as
    // it was sent (one that differs, fetched and checked below).
    let (record, mut before) = checked_before(repo);
    let trees_only = |blobs: &Vec<Place>| blobs.iter().all(|b| b.3 & TREE != 0);
    let mut differs: HashSet<Id> = HashSet::new();
    let mut by_checksum: HashSet<Id> = HashSet::new();
    if !full && let Some(theirs) = repo.store.checksums("data")? {
        say("Comparing checksums");
        let ours = sums(repo);
        for pack in by_pack.keys().filter(|p| !out.bad_packs.contains(p)) {
            if let (Some(o), Some(t)) = (ours.get(pack), theirs.get(&pack_name(pack))) {
                if o.eq_ignore_ascii_case(t) {
                    by_checksum.insert(*pack);
                } else {
                    differs.insert(*pack);
                }
            }
        }
    }
    out.by_checksum = by_checksum.len() as u64;
    // A few data packs as samples: those never read back first, then those
    // the checksums didn't cover.
    let mut samples: HashSet<Id> = HashSet::new();
    if !full {
        let data: Vec<Id> = by_pack.iter().filter(|(p, b)| !trees_only(b) && !out.bad_packs.contains(p) && !differs.contains(p)).map(|(p, _)| *p).collect();
        let want = data.len().div_ceil(SAMPLE_ONE_IN).min(SAMPLES);
        let (unread, read): (Vec<Id>, Vec<Id>) = shuffled(data).into_iter().partition(|p| !before.contains(p));
        let (unread_unsummed, unread_summed): (Vec<Id>, Vec<Id>) = unread.into_iter().partition(|p| !by_checksum.contains(p));
        samples.extend(unread_unsummed.into_iter().chain(unread_summed).chain(read).take(want));
        out.sampled = samples.len() as u64;
    }

    // 5. Fetched whole and checked: if `full`, every pack; else the tree
    // packs not checked before, the samples, and any whose checksum
    // differs.
    let fetch: Vec<(&Id, &Vec<Place>)> =
        by_pack.iter().filter(|(p, _)| !out.bad_packs.contains(p)).filter(|(p, blobs)| full || (trees_only(blobs) && !before.contains(p)) || samples.contains(p) || differs.contains(p)).collect();
    progress.total_files.store(fetch.len() as u64, Relaxed);
    progress.total_bytes.store(fetch.iter().map(|(p, _)| present.get(p).copied().unwrap_or(0)).sum(), Relaxed);
    let mut damaged = 0u64;
    for (i, (pack, blobs)) in fetch.iter().enumerate() {
        stop()?;
        say(&format!("Reading back pack {} of {}", i + 1, fetch.len()));
        let data = repo.store.get(&pack_name(pack))?;
        out.bytes_read += data.len() as u64;
        progress.bytes.fetch_add(data.len() as u64, Relaxed);
        progress.files.fetch_add(1, Relaxed);
        let mut whole = repo.key.id(&data) == **pack;
        for (id, offset, len, flags) in blobs.iter() {
            let ok = data.get(*offset as usize..(*offset + *len) as usize).is_some_and(|sealed| unseal(&repo.key, id, sealed, *flags).is_ok());
            if !ok {
                whole = false;
                bad.insert(*id);
            }
        }
        if whole {
            before.insert(**pack);
            if repo.store.keeps_checksums() {
                note_sum(repo, pack, &data);
            }
        } else {
            damaged += 1;
            out.bad_packs.push(**pack);
        }
        out.packs_read += 1;
    }
    if damaged > 0 {
        out.problems.push(format!("{} damaged (not as it was written)", packs(damaged)).replace("are damaged (not as it was", "are damaged (not as they were"));
    }
    // (The packs found whole, for next time.)
    if let Some(path) = record {
        let text: String = before.iter().map(|id| format!("{}\n", id.hex())).collect();
        let tmp = path.with_extension("part");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
    let unread = unreadable.iter().filter(|t| !bad.contains(t)).count();
    if unread > 0 {
        out.problems.push(if unread == 1 { "a folder’s list can’t be read".into() } else { format!("{unread} folders’ lists can’t be read") });
        bad.extend(unreadable);
    }

    // 6. What that leaves unable to come back.
    if !bad.is_empty() {
        say("Finding the files it affects");
        affected(repo, snapshot, &bad, &mut out)?;
    }
    out.seconds = started.elapsed().as_secs_f64();
    Ok(out)
}

/// The files (and folders) under `snapshot` that need any of `bad`: the
/// first few, by path, and how many.
fn affected(repo: &Repo, snapshot: &Snapshot, bad: &HashSet<Id>, out: &mut Checked) -> Result<()> {
    let mut todo: Vec<(Id, PathBuf)> = vec![(snapshot.tree, PathBuf::from("/"))];
    let mut seen: HashSet<Id> = HashSet::new();
    while let Some((t, at)) = todo.pop() {
        if bad.contains(&t) {
            out.files_affected += 1;
            if out.files.len() < NAMED {
                out.files.push(format!("{}/ (its list)", at.display()));
            }
            continue;
        }
        // (A folder seen once is the same everywhere it's used.)
        if !seen.insert(t) {
            continue;
        }
        let Ok(tree) = repo.tree(&t) else { continue };
        for n in tree.nodes {
            let path = at.join(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(&n.name));
            if n.content.iter().any(|c| bad.contains(c)) {
                out.files_affected += 1;
                if out.files.len() < NAMED {
                    out.files.push(path.display().to_string());
                }
            }
            if let Some(sub) = n.subtree {
                todo.push((sub, path));
            }
        }
    }
    out.files.sort();
    Ok(())
}
