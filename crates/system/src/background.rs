//! Background work kept to the memory that is to be had for nothing
//! (kernel/TUNING.md has the measurements and the reviews behind every
//! choice here).
//!
//! Linux has no link between a process's priority and what is taken from
//! memory first; it chooses by control group. Olo's login divides a
//! login session in two (`session_groups`: `desk`, and `bg` beside it),
//! the supervisor puts compilers and linkers in `bg`, and one keeper
//! (here) decides `bg`'s allowance (`memory.high`) about four times a
//! second while anything is in it, twenty near the line where memory
//! runs short: what the group holds plus what is to be had for nothing,
//! less a reserve. Past its allowance the kernel takes memory back
//! from the group (its cache, and what its ceiling lets go to swap) and
//! slows the programs of it that ask for more. The allowance itself
//! refuses nothing and ends nothing; a computer short of memory
//! altogether still can, as with no keeper.
//!
//! What the reviews asked of a keeper, and where it is here (and how
//! far each goes: the fifth review held every sentence here against the
//! code):
//! - **One owner.** A lock ([`Keeper::start`]): a second supervisor
//!   reaching the same session neither keeps nor lifts.
//! - **Nobody else's figure is left standing for long.** At every look
//!   of a group with something in it the keeper reads the allowance
//!   and the swap ceiling and sets again what differs from what it
//!   decides (a write that fails is said, and tried again at the
//!   next). What was written and overwritten between two looks is
//!   never seen; and with nothing in the group and everything lifted
//!   the keeper waits, for someone to come into it or to be told:
//!   what is written meanwhile stands until then.
//! - **An end by any road is meant to lift it.** Told to stop, the
//!   keeper lifts and reads back, and says "lifted" only of what read
//!   so ([`Keeper::stop`]: the allowance, and the swap ceiling where
//!   it is the keeper's to set); killed, a guard that outlives it
//!   lifts both ([`guard`]: a process waiting on a pipe the keeper's
//!   process holds the other end of), if the session's lock is then
//!   free to take (held by another keeper by then, they are that
//!   one's). Keeper and guard killed in the same moment, or a write
//!   that fails, leave them standing for whoever keeps the session
//!   next ([`clear_if_unkept`], which says when it couldn't either).
//! - **Nothing on anyone's realtime thread.** The keeper and its taker
//!   are threads at the background's own priority (batch scheduling and
//!   nice 10 asked for); starting and stopping are a worker thread's.
//! - **A look never asks the kernel for memory back, and never waits
//!   for a process.** (It has no bound on the clock all the same: the
//!   scheduler, a page of its own to bring back and the kernel's own
//!   counting can each hold it.) The kernel takes memory back in the
//!   time of whoever asks, for as long as it takes, and the keeper
//!   can't call it off. It used to ask in its own write of the
//!   allowance: with compilers asking
//!   for more all the while it spent four and a half seconds there at
//!   nine tenths of a processor (2026-11-21, the person's desktop),
//!   looked at nothing meanwhile, and the floor it had written was by
//!   then under what the group's programs held: 0.8 GiB of their own
//!   memory went to swap, which the rule forbids. So the allowance is
//!   written without waiting (`allow`: O_NONBLOCK, Linux 6.16; on a
//!   kernel before it nothing is kept); what the group then holds over
//!   it is a second thread's to take back (`Taker`, `take`,
//!   `memory.reclaim`: its droppable cache first, with `swappiness=0`;
//!   then, that asked for whole or not to be had, its programs' own);
//!   and a guard that ended is replaced without a look waiting for the
//!   new one ([`Guard::look`]).
//! - **What the rule forbids, the kernel refuses, while there is
//!   memory to spare** ([`SwapCeiling`], `memory.swap.max`). A look
//!   decides; what happens between two looks and inside the kernel
//!   isn't the keeper's to see. The group's own programs take memory
//!   back themselves as they ask for more under the allowance, and a
//!   part the taker asked for can enter the kernel after the desktop
//!   let go: both sent the programs' own memory to swap with memory to
//!   spare (the third review made each happen). So whether more of the
//!   group may go to swap is set at every look, before the allowance:
//!   not while the look leaves its programs what they hold with more
//!   than the band (`SUDDEN`) to spare over the line. Closed, the
//!   kernel charges the group no more swap, whoever asks; what was
//!   already on its way to the disk still goes. Every ask of the
//!   taker's is still for one look (`Taking::revision`) and read
//!   against the group again before each part (the group can still
//!   change between that reading and the kernel's taking: it is the
//!   closed ceiling, not the reading, that lets none of its own go).
//! - **When the computer is short, the background's is meant to go
//!   first.** A ceiling kept closed keeps the background's own out of
//!   the kernel's reach too, and a desktop that suddenly needed more
//!   than there was had its own memory swapped in the background's
//!   place (the fourth review). So the ceiling is open at every look
//!   that puts the allowance under what the programs hold, and at one
//!   within the band over that line; looks come sooner as what is
//!   available nears the band ([`near`]); and by how much the
//!   desktop's swap grew lately ([`Lately`]) counts as needing its
//!   room back before the background keeps what it holds
//!   ([`allowance`]). Open, the kernel is free to take from either
//!   group, and which it takes first is its own choice: an allowance
//!   under what the programs hold and the taker's asking lean it
//!   towards the background's, no more. (What the sudden-need guests
//!   show is how much of each group's was in swap before and after,
//!   not who took it: kernel/TUNING.md.) The price is within the
//!   band: a build that grows there past its allowance can have its
//!   own swapped though memory is to spare (49 MiB of 80, the fifth
//!   review's guest).
//! - **A session whose login didn't hand over `memory.reclaim` and
//!   `memory.swap.max`** (one from before it did, or one whose
//!   hand-over failed; `session_groups`) is kept by its allowance alone
//!   (`Way::AllowanceOnly`), and only while memory is short: with
//!   nothing to refuse the group's own to swap, an allowance held while
//!   there was memory to spare had a growing build swap its own (65 MiB
//!   of 80, where a group with no allowance at all lost nothing: the
//!   fifth review). Short, the kernel holds the group's programs to it
//!   as they ask for more, swapping their own where their cache doesn't
//!   cover it; a group asleep gives nothing back until the whole
//!   computer is short. Said once, with what is missing and what
//!   may bring it (the login's update and a new login).
//! - **Errors aren't emptiness.** Where who is in the group or what it
//!   holds can't be read, the keeper tries to lift what it set, and
//!   says each kind of trouble once; the desktop's waiting not to be
//!   read counts the unused cache for nothing; its swap not to be read
//!   counts nothing as gone lately.

use std::{
    fs,
    io::{Read as _, Seek as _, Write as _},
    os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use crate::session_groups::Groups;

/// How often the allowance is set while anything is in the group.
const EVERY: Duration = Duration::from_millis(250);
/// And how often near the line where the background's own starts to
/// go (twice the reserve available): as near as a desktop could bring
/// it between two looks ([`near`]). (The time is from one look's end to
/// the next's start; when the thread then runs is the scheduler's.)
const EVERY_NEAR: Duration = Duration::from_millis(50);
/// What a desktop can come to need between two such looks (ten
/// gigabytes a second, written by every processor at once): with less
/// than this to spare over twice the reserve, a need that sudden could
/// be met before the next look, and must not find the background's own
/// out of the kernel's reach. So wide is the band in which the swap
/// ceiling is open with memory still to spare.
const SUDDEN: u64 = 512 << 20;
/// And between two ordinary looks, at the same rate.
const SUDDEN_SLOW: u64 = SUDDEN * (EVERY.as_millis() / EVERY_NEAR.as_millis()) as u64;

/// Whether the next look is to be the sooner one: with less available
/// than the line (`short`: twice the reserve, and what of the
/// desktop's went to swap lately), the band over it, and what a
/// desktop could take in an ordinary look's time. By that reckoning
/// no need at the rate assumed reaches the band from further off
/// before the next look, and from here on each look comes before the
/// band is crossed, so that the ceiling is opened with twice the
/// reserve still to be had. A reckoning, not a promise: the time
/// between two looks is the wait and the look and whenever the
/// scheduler then runs the thread (25 ms over, once, in a guest), a
/// need can be faster than assumed, and the line itself moves with
/// what the desktop lost to swap. (Before, the sooner looks began
/// within one reserve of the line, which on a small computer is
/// inside the band: a need that began with the ceiling closed had it
/// opened a quarter of a second on, in the fifth review's 1.8 GB
/// guest; and on a large one further off than they need be.)
fn near(available: u64, short: u64) -> bool {
    available < short.saturating_add(SUDDEN).saturating_add(SUDDEN_SLOW)
}
/// The allowance's steps: it is written only when it moves by one.
const STEP: u64 = 16 << 20;
/// The least an allowance may be.
const LEAST: u64 = 16 << 20;
/// How long one part of what the keeper takes back should take at the
/// most: over it, the next part is half the size; under a quarter of
/// it, twice.
const BUDGET: Duration = Duration::from_millis(100);
/// A part of the group's cache, as the taker asks for it: the most
/// and the least.
const CACHE_MOST: u64 = 256 << 20;
const CACHE_LEAST: u64 = 16 << 20;
/// A part of its programs' own memory (to swap): the most, the least,
/// and the first (small: how fast swap takes it isn't known yet).
const OWN_MOST: u64 = 64 << 20;
const OWN_LEAST: u64 = 1 << 20;
const OWN_FIRST: u64 = 4 << 20;

/// The computer's memory, in bytes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Memory {
    pub total: u64,
    /// Used for nothing at all.
    pub free: u64,
    /// What could be had without swapping (the kernel's estimate).
    pub available: u64,
    /// Files' cache the kernel has down as not lately used (its inactive
    /// file pages): what a computer that has run for a day holds most of
    /// its memory as, while it keeps little free on purpose.
    pub unused_cache: u64,
    /// Files that programs have mapped: in use, or soon to be again,
    /// wherever the kernel has them listed.
    pub mapped: u64,
}

impl Memory {
    /// As /proc/meminfo has it now.
    pub fn now() -> Option<Memory> {
        Memory::parse(&fs::read_to_string("/proc/meminfo").ok()?)
    }

    fn parse(meminfo: &str) -> Option<Memory> {
        let kb = |key: &str| meminfo.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(':')?.split_ascii_whitespace().next()?.parse::<u64>().ok()).map(|kb| kb * 1024);
        Some(Memory { total: kb("MemTotal")?, free: kb("MemFree")?, available: kb("MemAvailable")?, unused_cache: kb("Inactive(file)")?, mapped: kb("Mapped")? })
    }
}

/// What the group holds, in bytes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Held {
    /// All of it (`memory.current`).
    pub all: u64,
    /// Its droppable cache: files' pages on the kernel's two file lists
    /// (`active_file` + `inactive_file`), and no more than its files
    /// less its memory files (`file` - `shmem`). Not what is locked in
    /// memory, nor its programs' own, nor memory files.
    pub cache: u64,
    /// What is locked in memory (`unevictable`; part of `all`): never
    /// given up, whatever is asked.
    pub locked: u64,
    /// File pages on the inactive list, including files a program can use again.
    pub unused_cache: u64,
}

impl Held {
    /// The group whose cgroup folder is `dir`, now.
    pub fn of(dir: &Path) -> Option<Held> {
        Held::parse(&fs::read_to_string(dir.join("memory.current")).ok()?, &fs::read_to_string(dir.join("memory.stat")).ok()?)
    }

    fn parse(current: &str, stat: &str) -> Option<Held> {
        let figure = |key: &str| stat.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(' ')?.trim().parse::<u64>().ok());
        let disk_file = figure("file")?.saturating_sub(figure("shmem")?);
        let unused_cache = figure("inactive_file")?;
        let locked = figure("unevictable").unwrap_or(0);
        // (Counted from the file lists themselves, not as files less what
        // is locked: what is locked may be a program's own memory or a
        // memory file, and taking it off would hide cache that can go. A
        // kernel that doesn't say what is on the active list: files less
        // what is locked, as before.)
        let cache = match figure("active_file") {
            Some(active) => disk_file.min(active.saturating_add(unused_cache)),
            None => disk_file.saturating_sub(locked),
        };
        Some(Held { all: current.trim().parse().ok()?, cache, unused_cache, locked })
    }
}

/// The background group's allowance: what it holds plus what is to be
/// had for nothing beyond a reserve (`reserve`: a share of the
/// computer's memory, in percent, kept for everyone else). To be had for
/// nothing: what is free, and `credit` (none of it to all of it:
/// [`Credit`]) of the files' cache nobody is using, which is what the
/// kernel has down as not lately used, less the group's own (already in
/// what it holds) and less what programs have mapped (a mapped file is
/// in use whatever list it is on). Never under what its programs
/// themselves hold (what isn't droppable cache: under that, they would
/// be swapped to make room nobody needs), unless little is to be had at
/// all: then by as much as is missing, and the background's own memory
/// may be taken back, to swap. (That is the floor, not always the
/// allowance: what is free beyond the reserve still counts, and what is
/// locked in memory can't go, so the allowance can stand over what the
/// programs hold while less than the line is available: with 1152 MiB
/// free of 8 GiB and 1280 available, 1024 MiB of the group's own kept
/// an allowance of 1184, the fifth review's figures.) Little is to be had
/// when less than twice
/// the reserve is available, counting what of the desktop's own went
/// to swap lately (`desk_swapped`: [`Lately`]) as needing its room back
/// first: the kernel meets a sudden need by swapping whoever it may,
/// and what is available afterwards looks ample again at the desktop's
/// cost (a burst of 110 MB over what was free left 120 MB of the
/// desktop's own in swap and 4 of the background's, the fourth
/// review). In steps
/// (`STEP`: the allowance down to one, the least it may be up to one, so
/// that a step never takes what the rule leaves), and at least `LEAST`.
pub fn allowance(memory: &Memory, held: &Held, reserve: f32, credit: f32, desk_swapped: u64) -> u64 {
    let reserve = (memory.total as f64 * f64::from(reserve.clamp(0.0, 50.0)) / 100.0) as u64;
    let unused = memory.unused_cache.saturating_sub(held.unused_cache + memory.mapped);
    let to_be_had = memory.free + (unused as f64 * f64::from(credit.clamp(0.0, 1.0))) as u64;
    let wanted = (held.all + to_be_had).saturating_sub(reserve);
    let own = held.all.saturating_sub(held.cache);
    let missing = (2 * reserve).saturating_add(desk_swapped).saturating_sub(memory.available);
    let floor = own.saturating_sub(missing).max(held.locked.min(held.all));
    (wanted / STEP * STEP).max(floor.div_ceil(STEP) * STEP).max(LEAST)
}

/// How much of a group's memory is in swap (`memory.swap.current`).
pub fn in_swap(dir: &Path) -> Option<u64> {
    fs::read_to_string(dir.join("memory.swap.current")).ok()?.trim().parse().ok()
}

/// What of the desktop's own memory went to swap lately: by how much
/// its group's swap grew, from one look to the next, counting for half
/// as much every `HALF` (it has had its chance to come back, and room
/// kept for it for good would be the background's memory swapped to
/// make room nobody needs), and never for more than is in swap now. A
/// fall pays it back: counted by the rises alone, swap that went up
/// and down by 64 MiB four times a second around a gigabyte of old
/// stood at 909 MiB "lately" half a minute on, and took 800 MiB of the
/// background's allowance (the fifth review). What was there before
/// the keeper looked isn't counted (a desktop's programs of last week
/// are where they belong), nor a rise across a time nobody looked
/// (the group empty, the figure not to be read: longer than `HALF`,
/// and there is no saying when it rose). It is the group's balance,
/// which is all the kernel gives: not how much was written to swap
/// and came back between two looks, nor whose pages they were.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Lately {
    went: f64,
    read: Option<u64>,
    at: Option<Instant>,
}

/// How long until what went to swap counts for half.
const HALF: Duration = Duration::from_secs(5);

impl Lately {
    /// What counts now (`now`), the desktop's group having `in_swap`
    /// bytes in swap ([`in_swap`]; None: not to be read, and nothing
    /// counts until it can).
    pub fn after(&mut self, in_swap: Option<u64>, now: Instant) -> u64 {
        let before = std::mem::replace(&mut self.read, in_swap);
        if let Some(at) = self.at.replace(now) {
            let elapsed = now.saturating_duration_since(at);
            self.went *= 0.5f64.powf(elapsed.as_secs_f64() / HALF.as_secs_f64());
            // (Across a time nobody looked, longer than it counts for:
            // a rise there has no date. From here it is counted again.)
            if elapsed <= HALF
                && let (Some(current), Some(previous)) = (in_swap, before)
            {
                self.went = (self.went + current as f64 - previous as f64).max(0.0);
            }
        }
        let Some(current) = in_swap else { return 0 };
        self.went = self.went.min(current as f64);
        self.went as u64
    }
}

/// How much of the unused cache counts as to be had for nothing.
///
/// "Unused" is the kernel's guess, and wrong both ways (a file a program
/// read sixty-four times running was all unused to the
/// multi-generational LRU). So it is believed only while the desktop
/// isn't seen to wait on memory: its own group says how long its
/// programs have waited (`desk/memory.pressure`, the kernel's total for
/// "some"). Between two readings in which they waited more than `HURTS`
/// of the time, the credit halves for every quarter of a second
/// between them (to nothing, under a sixteenth); without, it comes
/// back at a tenth a second of all of it: quick to give way, slow to
/// take. Both by the time, not by the reading: looks come five times as
/// often near the line, and the credit went five times as fast there
/// while it halved at each (the fifth review). Each reading is measured against the time
/// since the one before it, however long that was (a look missed, the
/// group empty for a while). Where the desktop's waiting can't be read
/// there is nothing to believe the guess by, and it counts for nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Credit {
    share: f32,
    /// The last reading, and when.
    read: Option<(u64, Instant)>,
}

/// The share of the time the desktop's programs may wait on memory
/// before the unused cache stops counting.
const HURTS: f64 = 0.01;
/// How long without such a wait until all of the cache counts again.
const BACK: Duration = Duration::from_secs(10);

impl Default for Credit {
    fn default() -> Self {
        Credit { share: 1.0, read: None }
    }
}

impl Credit {
    /// The credit now (`now`), the desktop's programs having waited
    /// `waited` microseconds on memory in all ([`waited`]; None: not to
    /// be read, and the cache counts for nothing until it can).
    pub fn after(&mut self, waited: Option<u64>, now: Instant) -> f32 {
        let Some(waited) = waited else { return 0.0 };
        if let Some((before, at)) = self.read {
            let over = now.saturating_duration_since(at);
            let hurt = waited.saturating_sub(before) as f64 > over.as_micros() as f64 * HURTS;
            self.share = if hurt {
                let less = self.share * 0.5f32.powf(over.as_secs_f32() / EVERY.as_secs_f32());
                if less < 1.0 / 16.0 { 0.0 } else { less }
            } else {
                (self.share + over.as_secs_f32() / BACK.as_secs_f32()).min(1.0)
            };
        }
        self.read = Some((waited, now));
        self.share
    }
}

/// How long a group's programs have waited on memory in all, in
/// microseconds (`memory.pressure`: "some … total=N").
pub fn waited(dir: &Path) -> Option<u64> {
    parse_waited(&fs::read_to_string(dir.join("memory.pressure")).ok()?)
}

fn parse_waited(pressure: &str) -> Option<u64> {
    pressure.lines().find(|l| l.starts_with("some "))?.split_ascii_whitespace().find_map(|f| f.strip_prefix("total="))?.parse().ok()
}

/// The allowance a group has: none (`max`), or bytes. None where it
/// can't be read.
pub fn allowed(dir: &Path) -> Option<Option<u64>> {
    let text = fs::read_to_string(dir.join("memory.high")).ok()?;
    match text.trim() {
        "max" => Some(None),
        bytes => bytes.parse().ok().map(Some),
    }
}

/// The allowance written, and without waiting (O_NONBLOCK: the kernel
/// sets it and returns, where a plain write under what the group holds
/// takes pages back in the writer until the group is under it. Linux
/// 6.16: on a kernel before it nothing is kept, [`Keeper::start`]).
/// Opened as a shell's `>` opens it: the kernel's files take a write as
/// the whole value either way, and a test's plain file is left holding
/// only what was written.
fn allow(dir: &Path, value: Option<u64>) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt as _;
    fs::OpenOptions::new().write(true).truncate(true).custom_flags(libc::O_NONBLOCK).open(dir.join("memory.high"))?.write_all(value.map_or("max".to_owned(), |v| v.to_string()).as_bytes())
}

/// Whether this kernel sets an allowance without waiting (Linux 6.16).
fn sets_without_waiting() -> bool {
    fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .and_then(|release| {
            let mut fields = release.trim().split(['.', '-']);
            Some((fields.next()?.parse::<u32>().ok()?, fields.next()?.parse::<u32>().ok()?))
        })
        .is_some_and(|version| version >= (6, 16))
}

/// Whether `path` is this process's to write.
fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt as _;
    // SAFETY: a plain system call on our string.
    std::ffi::CString::new(path.as_os_str().as_bytes()).is_ok_and(|p| unsafe { libc::access(p.as_ptr(), libc::W_OK) } == 0)
}

/// Whether more of the group's memory may go to swap
/// (`memory.swap.max`): how the rule's "never its programs' own, unless
/// little is to be had" is kept by the kernel itself, not by the keeper
/// being quick. Closed (the kernel charges the group no more swap,
/// whoever takes memory back: the group's own programs as they ask for
/// more under their allowance, the taker, a part it asked for before
/// the desktop let go) at every look that leaves the programs what
/// they hold with more than the band to spare: twice the reserve and
/// `SUDDEN` available, counting what of the desktop's went to swap
/// lately. Open at every other.
///
/// Before it the rule held only as far as the keeper was quick, and it
/// can't be: a build that grew 80 MiB under its allowance had 65 MiB of
/// its own swapped by the kernel, in the build's own programs, with
/// memory to spare and no look in between; a part the taker had asked
/// for entered the kernel after the allowance had risen (the third
/// review, 2026-11-21, on Linux 7.3 and 6.14 alike).
///
/// Open means open, not a figure: when the computer is short the kernel
/// takes from whoever it may, and a ceiling that let the background
/// give only what a look had reckoned left the rest to the desktop (a
/// desktop that took 110 MB more than was free had 95 to 121 MB of its
/// own swapped under a closed or a reckoned ceiling, 61 with nobody
/// keeping: the fourth review's guest). Open, the kernel may take from
/// either group, as with nobody keeping; an allowance under what the
/// programs hold leans it towards the background's, as the taker's
/// asking does.
///
/// Closed, a program that outgrows its allowance is slowed by the
/// kernel until a look gives it more (80 MiB asked for in 1.3 s, not
/// 0.17), where it used to be swapped. The ceiling is on how much of
/// the group is in swap, not on what is written: what is there stays
/// (and comes back as it is used); what was handed to the disk before
/// it closed is still written, and with zswap what sits compressed in
/// memory can still be written out later.
///
/// The ceiling of Olo's `bg` group is the keeper's own, as its
/// allowance is: none ("max") whenever nothing is kept. (One meant to
/// bind a whole session belongs on the scope above, which nothing here
/// writes.)
struct SwapCeiling(PathBuf);

impl SwapCeiling {
    /// The ceiling of the group in `dir`, where the keeper has all it
    /// takes to keep whole: the ceiling and `memory.reclaim` its to
    /// write (the login's hand-over, `session_groups`).
    fn of(dir: &Path) -> Option<SwapCeiling> {
        (writable(&dir.join("memory.swap.max")) && writable(&dir.join("memory.reclaim"))).then(|| SwapCeiling(dir.to_owned()))
    }

    /// Open (more may go to swap), or closed (no more than is there).
    fn set(&self, open: bool) -> std::io::Result<()> {
        write_swap(&self.0, if open { "max" } else { "0" })
    }
}

/// A group's swap ceiling written (`figure`: bytes, or "max" for none),
/// where it stands at another.
fn write_swap(dir: &Path, figure: &str) -> std::io::Result<()> {
    let path = dir.join("memory.swap.max");
    if fs::read_to_string(&path)?.trim() == figure {
        return Ok(());
    }
    fs::OpenOptions::new().write(true).truncate(true).open(path)?.write_all(figure.as_bytes())
}

/// Whether a group has no swap ceiling: its file says so, or there is
/// no such file to have one in. One that can't be read isn't known to
/// have none.
fn swap_free(dir: &Path) -> bool {
    match fs::read_to_string(dir.join("memory.swap.max")) {
        Ok(text) => text.trim() == "max",
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

/// What of the group's droppable cache the keeper asks back now: what
/// the group holds over its allowance, of the cache it can drop, `most`
/// at the most; nothing under a step (the allowance moves by steps:
/// less than one over is no news).
pub fn cache_part(held: &Held, allowance: u64, most: u64) -> u64 {
    let over = held.all.saturating_sub(allowance).min(held.cache);
    if over < STEP { 0 } else { over.min(most) }
}

/// What of its programs' own memory the keeper asks back now (to swap):
/// what they hold above the allowance, `most` at the most; nothing under
/// a step. The rule puts the allowance under what they hold only where
/// little is to be had at all, so mostly nothing.
pub fn own_part(held: &Held, allowance: u64, most: u64) -> u64 {
    let over = held.all.saturating_sub(held.cache).saturating_sub(allowance);
    if over < STEP { 0 } else { over.min(most) }
}

/// The size of the next part, after one of `size` took the kernel
/// `took`: half where it took longer than [`BUDGET`], twice where under
/// a quarter of it, within `least` and `most`.
fn paced(size: u64, took: Duration, least: u64, most: u64) -> u64 {
    if took > BUDGET {
        (size / 2).max(least)
    } else if took < BUDGET / 4 {
        (size * 2).min(most)
    } else {
        size
    }
}

/// When a part the kernel couldn't take whole is asked for again: half
/// a second later, twice as long each time up to two seconds, while
/// the allowance is the same and what the group holds counts the same
/// number of whole steps (all of it, and its cache). One of the three
/// changed, it is asked again at once. (An ask cut short by a signal
/// isn't one the kernel couldn't take.)
#[derive(Default, Debug, Clone, Copy, PartialEq)]
struct Retry {
    /// Whether the kernel took less than was asked (EAGAIN: some of
    /// it, or none at all), not an error:
    /// for the cache, that is as good as "what could be had is had", and
    /// the programs' own may be asked for while the cache waits its
    /// turn again (`partial_in_backoff`). Without it, a cache that took
    /// longer to give out than a look is apart never let the own be
    /// asked for: every look began again at the cache, which waited
    /// (80 ms a try at looks 50 ms apart: three tries at the cache and
    /// none at the own in 2.5 s, the fifth review).
    partial: bool,
    key: Option<(u64, u64, u64)>,
    delay: Duration,
    after: Option<Instant>,
}

impl Retry {
    fn key(held: &Held, high: u64) -> (u64, u64, u64) {
        (high, held.all / STEP, held.cache / STEP)
    }

    fn ready(&self, key: (u64, u64, u64)) -> bool {
        self.key != Some(key) || self.after.is_none_or(|after| Instant::now() >= after)
    }

    fn partial_in_backoff(&self, key: (u64, u64, u64)) -> bool {
        self.partial && !self.ready(key)
    }

    fn result(&mut self, key: (u64, u64, u64), result: &std::io::Result<()>) {
        if matches!(result, Err(error) if error.raw_os_error() != Some(libc::EINTR)) {
            self.partial = matches!(result, Err(e) if e.raw_os_error() == Some(libc::EAGAIN));
            self.delay = if self.key == Some(key) { (self.delay * 2).min(Duration::from_secs(2)) } else { EVERY * 2 };
            self.key = Some(key);
            self.after = Some(Instant::now() + self.delay);
        } else {
            *self = Self::default();
        }
    }
}

/// How large the parts are that a keeper takes back ([`paced`]).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pace {
    cache: u64,
    own: u64,
    cache_retry: Retry,
    own_retry: Retry,
}

impl Default for Pace {
    fn default() -> Self {
        Pace { cache: CACHE_MOST, own: OWN_FIRST, cache_retry: Retry::default(), own_retry: Retry::default() }
    }
}

/// What the taker is asked: what a group with this allowance holds over
/// it, taken back ([`take`]), for the look that decided it.
#[derive(Clone, Copy)]
struct Ask {
    high: u64,
    own_budget: u64,
    reserve: f32,
    credit: f32,
    desk_swapped: u64,
}

/// How the taker stands.
#[derive(Default)]
struct Taking {
    /// An ask is being done (or is on its way to be).
    busy: bool,
    /// What the keeper decided at its last look (None: no allowance),
    /// and that look's number: an ask is for one look and no other (two
    /// looks can decide the same allowance and differ in what they let
    /// be taken).
    desired: Option<u64>,
    revision: u64,
    pace: Pace,
    /// `memory.reclaim` turned out not to be the keeper's to write.
    refused: bool,
}

/// The thread that takes back for a keeper: one ask at a time, in its
/// own time (batch scheduling and nice 10 asked for), however long the
/// kernel is about it. A keeper that stops leaves it nothing more to do
/// (every ask is for a look that is no longer the last) and waits for
/// it to end before it gives the group up.
struct Taker {
    asks: mpsc::Sender<(u64, Ask)>,
    worker: std::thread::JoinHandle<()>,
    taking: Arc<Mutex<Taking>>,
}

impl Taker {
    fn start(dir: &Path, notes: mpsc::Sender<String>) -> std::io::Result<Taker> {
        let (asks, asked) = mpsc::channel::<(u64, Ask)>();
        let taking = Arc::new(Mutex::new(Taking::default()));
        let (dir, shared) = (dir.to_owned(), taking.clone());
        let worker = std::thread::Builder::new().name("background taker".into()).spawn(move || {
            lower_this_thread();
            let mut reported = false;
            for (revision, ask) in asked {
                let current = || {
                    let t = shared.lock().unwrap_or_else(|e| e.into_inner());
                    t.revision == revision && t.desired == Some(ask.high)
                };
                let mut pace = shared.lock().unwrap_or_else(|e| e.into_inner()).pace;
                let refused = match take(&dir, &ask, &current, &mut pace) {
                    Ok(()) => false,
                    Err(error) => {
                        if !reported {
                            let _ = notes.send(format!("The background group's memory couldn't be taken back: {error}"));
                            reported = true;
                        }
                        matches!(error.raw_os_error(), Some(libc::EACCES | libc::EPERM | libc::ENOENT | libc::EOPNOTSUPP | libc::EINVAL))
                    }
                };
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                state.busy = false;
                state.pace = pace;
                state.refused |= refused;
            }
        })?;
        Ok(Taker { asks, taking, worker })
    }

    /// A look's decision: numbered (whatever the taker was asked for a
    /// look before it is for nothing from here on), and written where
    /// the kernel has another figure, without waiting.
    fn decide(&self, dir: &Path, desired: Option<u64>) -> std::io::Result<()> {
        {
            let mut state = self.taking.lock().unwrap_or_else(|e| e.into_inner());
            state.desired = desired;
            state.revision = state.revision.wrapping_add(1);
        }
        if allowed(dir) != Some(desired) {
            allow(dir, desired)?;
        }
        Ok(())
    }

    fn finish(self) {
        drop(self.asks);
        let _ = self.worker.join();
    }

    /// `ask` handed over, unless one is still being done: whether it was.
    fn ask(&self, ask: Ask) -> bool {
        let mut t = self.taking.lock().unwrap_or_else(|e| e.into_inner());
        if t.busy {
            return false;
        }
        t.busy = self.asks.send((t.revision, ask)).is_ok();
        t.busy
    }

    fn pace(&self) -> Pace {
        self.taking.lock().unwrap_or_else(|e| e.into_inner()).pace
    }

    fn refused(&self) -> bool {
        self.taking.lock().unwrap_or_else(|e| e.into_inner()).refused
    }
}

/// What a keeper has to keep with.
enum Way {
    /// All of it (the login handed over `memory.reclaim` and
    /// `memory.swap.max`): the allowance, the swap ceiling, and what
    /// the group holds over its allowance taken back by the taker.
    Whole(SwapCeiling),
    /// The allowance alone (a session of a login from before that
    /// hand-over): the kernel holds the group's programs to it as they
    /// ask for more, and takes their own memory too where their cache
    /// doesn't cover it; a group asleep gives nothing back until the
    /// whole computer is short. Said once, with what brings the rest.
    AllowanceOnly,
}

/// `bytes` of the group's memory taken back now, in this thread
/// (`memory.reclaim`): `cache_only`, files' cache and nothing moved to
/// swap (`swappiness=0`, Linux 6.11); else what the kernel finds (the
/// cache gone, its programs' own, to swap). Less than asked is EAGAIN.
fn take_back(dir: &Path, bytes: u64, cache_only: bool) -> std::io::Result<()> {
    let ask = if cache_only { format!("{bytes} swappiness=0") } else { bytes.to_string() };
    let written = fs::OpenOptions::new().write(true).open(dir.join("memory.reclaim"))?.write(ask.as_bytes())?;
    if written == ask.len() { Ok(()) } else { Err(std::io::Error::other("memory.reclaim took only part of what was written to it")) }
}

/// One ask of the taker's, in at most two parts. Before each: is this
/// still the look that asked (its revision), is anything in the group,
/// is the allowance the one asked for; and how much, against the
/// higher of that allowance and the rule reckoned again from what the
/// computer and the group hold now (with the look's reserve and
/// credit). (What is read and what is then written are moments apart:
/// the kernel may hold another state by the write.) The cache first,
/// and nothing to swap. The programs' own may be asked for once the
/// part asked of the cache covers what the group holds over in cache,
/// or there is less than a step of it to ask for, or the kernel had no
/// more of it to give (EAGAIN, now or at a try that still waits its
/// turn again): no more than the look that decided allowed, and no
/// more than is over the allowance as reckoned now. A part that came
/// back whole sets the size of
/// the next ([`paced`]); an error that isn't "no more" or a signal is
/// the taker's to report.
fn take(dir: &Path, ask: &Ask, current: &impl Fn() -> bool, pace: &mut Pace) -> std::io::Result<()> {
    let Ask { high, own_budget, reserve, credit, desk_swapped } = *ask;
    let snapshot = || {
        if !current() || populated(&mut fs::File::open(dir.join("cgroup.events")).ok()?) != Some(true) || allowed(dir) != Some(Some(high)) {
            return None;
        }
        let held = Held::of(dir)?;
        let safe = high.max(allowance(&Memory::now()?, &held, reserve, credit, desk_swapped));
        current().then_some((held, safe))
    };
    let Some((held, allowance)) = snapshot() else { return Ok(()) };
    let over = held.all.saturating_sub(allowance).min(held.cache);
    let cache = cache_part(&held, allowance, pace.cache);
    let key = Retry::key(&held, high);
    let mut cache_done = cache == 0 || pace.cache_retry.partial_in_backoff(key);
    if cache > 0 && pace.cache_retry.ready(key) {
        let began = Instant::now();
        let asked = take_back(dir, cache, true);
        pace.cache_retry.result(key, &asked);
        if asked.is_ok() {
            pace.cache = paced(cache, began.elapsed(), CACHE_LEAST, CACHE_MOST);
        }
        match asked {
            Ok(()) => cache_done = cache >= over,
            Err(e) if e.raw_os_error() == Some(libc::EAGAIN) => cache_done = true,
            Err(e) if e.raw_os_error() == Some(libc::EINTR) => {}
            Err(e) => return Err(e),
        }
    }
    if !cache_done || own_budget == 0 {
        return Ok(());
    }
    let Some((held, allowance)) = snapshot() else { return Ok(()) };
    let own = own_part(&held, allowance, pace.own).min(own_budget);
    let key = Retry::key(&held, high);
    if own > 0 && pace.own_retry.ready(key) {
        let began = Instant::now();
        let asked = take_back(dir, own, false);
        pace.own_retry.result(key, &asked);
        if asked.is_ok() {
            pace.own = paced(own, began.elapsed(), OWN_LEAST, OWN_MOST);
        }
        if let Err(e) = asked
            && !matches!(e.raw_os_error(), Some(libc::EAGAIN | libc::EINTR))
        {
            return Err(e);
        }
    }
    Ok(())
}

/// Whether anything is in the group (`cgroup.events`, read again from
/// its start). None where it can't be read: not the same as empty.
fn populated(events: &mut fs::File) -> Option<bool> {
    let mut text = String::new();
    events.rewind().ok()?;
    events.read_to_string(&mut text).ok()?;
    text.lines().find_map(|l| l.strip_prefix("populated ")).map(|v| v.trim() == "1")
}

/// A process as it was seen: its number and when it started (clock
/// ticks after boot), which together are one process and no other.
pub type Seen = (u32, u64);

/// When process `pid` started, if it is there.
pub fn started(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // (After the name, which may hold anything: the 20th field from its
    // closing bracket.)
    stat.rsplit_once(')')?.1.split_ascii_whitespace().nth(19)?.parse().ok()
}

/// How a move came out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Moved {
    Done,
    /// It ended first.
    Gone,
    /// The kernel said no (why).
    Refused(String),
}

/// One process moved into group `to` (its whole self: every thread),
/// checked to be the process seen before the write and after it. The
/// kernel accepts a number; a changed identity leaves the result unknown.
pub fn move_to(to: &Path, (pid, start): Seen) -> Moved {
    if started(pid) != Some(start) {
        return Moved::Gone;
    }
    let write = |dir: &Path| fs::OpenOptions::new().write(true).truncate(true).open(dir.join("cgroup.procs")).and_then(|mut f| f.write_all(pid.to_string().as_bytes()));
    match write(to) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => return Moved::Gone,
        Err(e) => return Moved::Refused(e.to_string()),
    }
    match started(pid) {
        Some(now) if now == start => Moved::Done,
        Some(_) => Moved::Refused("process identity changed across the write".into()),
        None => Moved::Gone,
    }
}

/// Every process of `from` moved to `to` (turned off: what is still in
/// the background's group goes back among the desktop's, off the idle
/// processors). How many were left behind; None where the group's list
/// can't be read (which isn't "none").
pub fn empty(from: &Path, to: &Path) -> Option<usize> {
    let list = || fs::read_to_string(from.join("cgroup.procs")).ok();
    for pid in list()?.split_ascii_whitespace() {
        let _ = fs::OpenOptions::new().write(true).truncate(true).open(to.join("cgroup.procs")).and_then(|mut f| f.write_all(pid.as_bytes()));
    }
    Some(list()?.split_ascii_whitespace().count())
}

struct Told {
    reserve: f32,
    stop: bool,
    /// The longest a look took so far (reading, deciding, writing and
    /// taking back): said to whoever measures ([`Keeper::longest_look`]).
    longest_look: Duration,
    active_since: Option<Instant>,
}

/// How a keeper ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ended {
    /// The group has no allowance, read back.
    pub lifted: bool,
    /// How many processes were still in the group after everything in it
    /// was moved back ([`Keeper::stop`]); None where that can't be said
    /// (its list not to be read).
    pub left: Option<usize>,
}

/// The one that keeps a session's background group to its allowance.
pub struct Keeper {
    told: Arc<Mutex<Told>>,
    wake: Arc<OwnedFd>,
    /// (It comes back with whether the allowance is lifted, and its
    /// guard.)
    thread: Option<std::thread::JoinHandle<(bool, Option<Guard>)>>,
    groups: Groups,
    /// Held while this keeps (the kernel lets go of it when the process
    /// ends, however it ends).
    _lock: Locked,
}

/// The lock, held: let go when this goes. *Let go*, not only closed. A
/// lock of this kind belongs to the open file, and every child of this
/// process is born with a copy of every open file, this one too, until
/// it becomes its program (which closes it): a lock that was only
/// closed stays with any child being born at that moment, and whoever
/// asks next is told another keeps. Seen 2026-11-21, in the keeper's
/// own test under load (other tests starting programs beside it), and
/// then at once in a test of just that.
struct Locked(fs::File);

impl Drop for Locked {
    fn drop(&mut self) {
        // SAFETY: a plain system call on our descriptor.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn lock(path: &Path) -> Result<Locked, String> {
    let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    // SAFETY: a plain system call on our descriptor.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("another supervisor keeps this session's background group".into());
    }
    Ok(Locked(file))
}

/// The lock, for one who comes after a holder that didn't let go in
/// good order (a guard, after its keeper's process was ended): asked
/// for `tries` times, `pause` apart. That process may have left a child
/// still being born with its copy of the lock, for a moment; a keeper
/// that took over holds it for good.
fn lock_once_free(path: &Path, tries: u32, pause: Duration) -> Option<Locked> {
    for n in 0..tries {
        if let Ok(held) = lock(path) {
            return Some(held);
        }
        if n + 1 < tries {
            std::thread::sleep(pause);
        }
    }
    None
}

impl Keeper {
    /// Starts keeping `groups`' background group, with `reserve` percent
    /// of memory left for everyone else. `lock_file`: the file whose lock
    /// says who keeps (one for a login session; held by another, an Err,
    /// and nothing is touched). `guard`: the program, and its arguments
    /// before the two [`guard_args`] gives, that lifts the allowance if
    /// this process ends without doing it (None: no guard, a test's).
    /// What goes wrong while it keeps is sent to `notes`, each kind once.
    /// Not for a realtime thread: it starts a process. An Err too on a
    /// kernel that sets an allowance only by waiting (before Linux
    /// 6.16): a keeper never waits, so there it doesn't keep.
    pub fn start(groups: Groups, reserve: f32, lock_file: &Path, guard: Option<(&Path, &[&str])>, notes: mpsc::Sender<String>) -> Result<Keeper, String> {
        if !sets_without_waiting() {
            return Err("this kernel sets an allowance only by waiting for the memory to come back, which a keeper never does (Linux 6.16 sets one at once)".into());
        }
        let held = lock(lock_file)?;
        // SAFETY: a new descriptor of our own.
        let wake = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if wake < 0 {
            return Err(format!("no way to wake the keeper: {}", std::io::Error::last_os_error()));
        }
        // SAFETY: just made, owned by nothing else.
        let wake = Arc::new(unsafe { OwnedFd::from_raw_fd(wake) });
        // (No taker, no keeping: nothing the kernel may be long about is
        // the keeper's own to ask.)
        let taker = Taker::start(&groups.bg, notes.clone()).map_err(|error| format!("its taker couldn't start ({error})"))?;
        // (No guard, no keeping: an allowance that a kill would leave
        // standing isn't set.)
        let guard = match guard {
            Some((program, before)) => {
                let (program, before, after) = (program.to_owned(), before.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>(), guard_args(&groups.bg, lock_file));
                let start: Arc<dyn Fn() -> std::io::Result<std::process::Child> + Send + Sync> =
                    Arc::new(move || std::process::Command::new(&program).args(&before).args(&after).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn());
                Some(Guard { child: start().and_then(ready).map_err(|e| format!("its guard couldn't start ({e})"))?, start, starting: None, coming: None, ended: Vec::new(), again: Vec::new() })
            }
            None => None,
        };
        let way = match SwapCeiling::of(&groups.bg) {
            Some(swap) => Way::Whole(swap),
            None => {
                let _ = notes.send(
                    "The background group is kept by its allowance alone, and only while memory is short: to keep it in full its keeper must be able to write both memory.reclaim and memory.swap.max, and in this session it can't (the login hands them over; a login from before it did may be why: Olo Doctor brings the login screen up to date, then log in again)".into(),
                );
                Way::AllowanceOnly
            }
        };
        let told = Arc::new(Mutex::new(Told { reserve, stop: false, longest_look: Duration::ZERO, active_since: None }));
        let thread = std::thread::Builder::new()
            .name("background keeper".into())
            .spawn({
                let (told, wake) = (told.clone(), wake.clone());
                let groups = groups.clone();
                move || keep(&groups, &told, &wake, &notes, guard, taker, way)
            })
            .map_err(|e| e.to_string())?;
        Ok(Keeper { told, wake, thread: Some(thread), groups, _lock: held })
    }

    /// The longest one look has taken so far (it reads a dozen small
    /// files and writes two: nothing in it waits for memory to come back
    /// or for a process).
    pub fn longest_look(&self) -> Duration {
        let t = self.told.lock().unwrap_or_else(|e| e.into_inner());
        t.longest_look.max(t.active_since.map_or(Duration::ZERO, |at| at.elapsed()))
    }

    fn tell(&self, change: impl FnOnce(&mut Told)) {
        change(&mut self.told.lock().unwrap_or_else(|e| e.into_inner()));
        // SAFETY: eight bytes of ours to our own descriptor.
        unsafe { libc::write(self.wake.as_raw_fd(), 1u64.to_ne_bytes().as_ptr().cast(), 8) };
    }

    /// The share of memory left for everyone else, from now.
    pub fn set_reserve(&self, reserve: f32) {
        self.tell(|t| t.reserve = reserve);
    }

    /// Stops keeping: the allowance lifted and read back, then everything
    /// still in the group moved back among the desktop's, all of it while
    /// this still holds the lock (nobody else keeps or clears in
    /// between). Waits for the keeper (a write of its may be under way),
    /// so not for a realtime thread either.
    pub fn stop(mut self) -> Ended {
        self.end(true)
    }

    fn end(&mut self, clear: bool) -> Ended {
        self.tell(|t| t.stop = true);
        let (lifted, guard) = self.thread.take().and_then(|t| t.join().ok()).unwrap_or((false, None));
        let left = clear.then(|| empty(&self.groups.bg, &self.groups.desk)).flatten();
        // The guards after the keeper, which has lifted what it could
        // under the lock: the one there and those that ended before
        // it are ended and waited for here; one still being started is
        // its starter's to end and wait for (dropped unreceived, a
        // child was left a zombie under the supervisor: the fifth
        // review).
        if let Some(guard) = guard {
            drop(guard.starting);
            for mut child in guard.ended.into_iter().chain(std::iter::once(guard.child)) {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        Ended { lifted, left }
    }
}

impl Drop for Keeper {
    /// (The supervisor ending with the choice on: as stopped. Nobody
    /// keeping, nothing stays in the group: what is there would run on
    /// idle processors with no one to say when it may have more. The
    /// supervisor that starts next puts compilers in again.)
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.end(true);
        }
    }
}

/// The process that clears up if the keeper's is killed ([`guard`]), and
/// how to start another: a look that finds its guard gone tries to
/// lift everything before it asks for another. (A guard that was there
/// when a look checked can end before that look's writes: found at the
/// next look of a group with something in it.)
struct Guard {
    child: std::process::Child,
    /// Starts another. Never called on the keeper's thread once it
    /// keeps: starting a process waits on the kernel for as long as that
    /// takes (a look waited two seconds in it, the fourth review).
    start: Arc<dyn Fn() -> std::io::Result<std::process::Child> + Send + Sync>,
    /// Another being started, on a thread of its own: the process, once
    /// there is one.
    starting: Option<mpsc::Receiver<std::io::Result<std::process::Child>>>,
    /// Since when `child` is one started in another's place that hasn't
    /// said it is there yet.
    coming: Option<Instant>,
    /// Ones that ended, until the kernel has been told so.
    ended: Vec<std::process::Child>,
    /// When one was started again, lately.
    again: Vec<Instant>,
}

/// How often in a minute a guard that ended is started again before it
/// is taken to be one that can't stay.
const GUARD_AGAIN: usize = 3;
/// How long a guard has to say it is there.
const READY_WITHIN: Duration = Duration::from_secs(2);

/// What a look finds of the guard.
enum Watch {
    There,
    /// Another was started and hasn't said it is there yet.
    Coming,
    /// None: how the last one ended.
    Ended(String),
}

impl Guard {
    /// Whether a guard is there now. Nothing here waits: a look of the
    /// keeper's once waited two seconds on a guard started in another's
    /// place (2026-11-21, the third review), the allowance standing all
    /// the while.
    fn look(&mut self) -> Watch {
        self.ended.retain_mut(|child| matches!(child.try_wait(), Ok(None)));
        if let Some(starting) = &self.starting {
            match starting.try_recv() {
                Ok(Ok(new)) => {
                    self.ended.push(std::mem::replace(&mut self.child, new));
                    (self.starting, self.coming) = (None, Some(Instant::now()));
                }
                Ok(Err(e)) => {
                    self.starting = None;
                    return Watch::Ended(format!("another couldn't be started ({e})"));
                }
                Err(mpsc::TryRecvError::Empty) => return Watch::Coming,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.starting = None;
                    return Watch::Ended("the thread starting another ended without one".into());
                }
            }
        }
        match self.child.try_wait() {
            Ok(None) => match self.coming {
                None => Watch::There,
                Some(since) => match said_ready(&self.child) {
                    Some(true) => {
                        self.coming = None;
                        Watch::There
                    }
                    None if since.elapsed() < READY_WITHIN => Watch::Coming,
                    _ => {
                        let _ = self.child.kill();
                        Watch::Ended("it didn't say it was there".into())
                    }
                },
            },
            ended => Watch::Ended(format!("{ended:?}")),
        }
    }

    /// Another asked for in the place of one that ended (`how`), on a
    /// thread of its own, unless that has been done too often lately: an
    /// Err says why none is on its way.
    fn replace(&mut self, how: &str) -> Result<(), String> {
        self.again.retain(|at| at.elapsed() < Duration::from_secs(60));
        if self.again.len() >= GUARD_AGAIN {
            return Err(format!("it keeps ending ({how}): started again {GUARD_AGAIN} times in a minute"));
        }
        // (The child is the starter's until a look takes it: handed
        // over hand to hand, so one nobody takes is ended and waited
        // for there, not dropped.)
        let (started, starting) = mpsc::sync_channel(0);
        let start = self.start.clone();
        std::thread::Builder::new()
            .name("background guard".into())
            .spawn(move || {
                if let Err(mpsc::SendError(Ok(mut child))) = started.send(start()) {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            })
            .map_err(|e| format!("it ended ({how}) and another couldn't be started ({e})"))?;
        self.starting = Some(starting);
        self.again.push(Instant::now());
        Ok(())
    }
}

/// Ask for batch scheduling and nice 10 for the calling thread. It
/// stays in the supervisor's cgroup.
fn lower_this_thread() {
    // SAFETY: plain system calls on this thread.
    unsafe {
        let param = libc::sched_param { sched_priority: 0 };
        libc::sched_setscheduler(0, libc::SCHED_BATCH, &param);
        libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, 10);
    }
}

/// Waits for `wake` to be written, `events` (a group's `cgroup.events`)
/// to change, or `most` to pass.
fn wait(wake: &OwnedFd, events: Option<&fs::File>, guard: Option<&Guard>, most: Option<Duration>) {
    // (The guard's end of its pipe hangs up when the guard ends; one
    // on its way says a word on it.)
    let word = if guard.is_some_and(|g| g.coming.is_some()) { libc::POLLIN } else { 0 };
    // (While another is being started, the one held is the one that ended:
    // its pipe has hung up, and says nothing new.)
    let guard = guard.filter(|g| g.starting.is_none()).and_then(|g| g.child.stdout.as_ref()).map_or(-1, |out| out.as_raw_fd());
    let mut fds = [
        libc::pollfd { fd: wake.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        libc::pollfd { fd: events.map_or(-1, |e| e.as_raw_fd()), events: libc::POLLPRI, revents: 0 },
        libc::pollfd { fd: guard, events: word, revents: 0 },
    ];
    // SAFETY: our descriptors, our array.
    unsafe {
        libc::poll(fds.as_mut_ptr(), 3, most.map_or(-1, |d| d.as_millis() as libc::c_int));
        let mut count = [0u8; 8];
        libc::read(wake.as_raw_fd(), count.as_mut_ptr().cast(), 8);
    }
}

fn keep(groups: &Groups, told: &Mutex<Told>, wake: &OwnedFd, notes: &mpsc::Sender<String>, mut guard: Option<Guard>, taker: Taker, way: Way) -> (bool, Option<Guard>) {
    lower_this_thread();
    let bg = &groups.bg;
    let mut said = std::collections::HashSet::new();
    let mut say = |what: &'static str, why: String| {
        if said.insert(what) {
            let _ = notes.send(format!("{what}: {why}"));
        }
    };
    // (Nothing of the keeper's left standing while nothing is kept: no
    // allowance, then no swap ceiling.)
    let lift = |say: &mut dyn FnMut(&'static str, String)| {
        let allowance = taker.decide(bg, None).map_err(|e| say("The background group's allowance couldn't be lifted", e.to_string())).is_ok();
        let ceiling = !matches!(way, Way::Whole(_)) || write_swap(bg, "max").map_err(|e| say("The background group's swap ceiling couldn't be lifted", e.to_string())).is_ok();
        allowance && ceiling
    };
    let mut events = fs::File::open(bg.join("cgroup.events")).ok();
    let mut credit = Credit::default();
    let mut lately = Lately::default();
    loop {
        let (reserve, stop) = {
            let t = told.lock().unwrap_or_else(|e| e.into_inner());
            (t.reserve, t.stop)
        };
        if stop {
            break;
        }
        let looked = Instant::now();
        told.lock().unwrap_or_else(|e| e.into_inner()).active_since = Some(looked);
        let finish_look = || {
            let mut t = told.lock().unwrap_or_else(|e| e.into_inner());
            t.longest_look = t.longest_look.max(looked.elapsed());
            t.active_since = None;
        };
        match events.as_mut().and_then(populated) {
            Some(true) => {}
            Some(false) => {
                lately.read = None;
                // (The credit stays as it was: what the desktop was seen
                // to wait holds for the next compiler, a moment later.)
                let lifted = lift(&mut say);
                finish_look();
                wait(wake, events.as_ref(), None, (!lifted).then_some(EVERY));
                continue;
            }
            None => {
                // Not to be read is not empty: nothing is kept on a guess.
                lift(&mut say);
                say("The background group can't be read", bg.join("cgroup.events").display().to_string());
                events = fs::File::open(bg.join("cgroup.events")).ok();
                finish_look();
                wait(wake, None, None, Some(Duration::from_secs(2)));
                continue;
            }
        }
        // (Where there is to be a guard, it is looked for before anything
        // is set. One that ended: what was set is lifted first, as far
        // as that goes, then another started, which no look waits for;
        // until that one says it is there, nothing is set.)
        if let Some(g) = guard.as_mut() {
            let watch = g.look();
            if !matches!(watch, Watch::There) {
                lift(&mut say);
                let coming = match watch {
                    Watch::Ended(how) => g.replace(&how).map_err(|why| say("The background group has no guard", why)).is_ok(),
                    _ => true,
                };
                finish_look();
                // (One on its way: until its word, or a look's time. None
                // on its way: looked at again as the old ones were.)
                if coming {
                    wait(wake, None, guard.as_ref(), Some(EVERY));
                } else {
                    wait(wake, None, None, Some(Duration::from_secs(2)));
                }
                continue;
            }
        }
        let mut every = EVERY;
        match (Memory::now(), Held::of(bg)) {
            (Some(memory), Some(held)) => {
                let desk = waited(&groups.desk);
                if desk.is_none() {
                    say("The desktop's waiting on memory can't be read", "only free memory counts for the background group until it can".into());
                }
                let share = credit.after(desk, Instant::now());
                let desk_swapped = lately.after(in_swap(&groups.desk), Instant::now());
                let want = allowance(&memory, &held, reserve, share, desk_swapped);
                let line = (memory.total as f64 * f64::from(reserve.clamp(0.0, 50.0)) / 100.0) as u64;
                let shortage = (2 * line).saturating_add(desk_swapped);
                // (Near the line where the background's own starts to
                // go: looked at again sooner.)
                if near(memory.available, shortage) {
                    every = EVERY_NEAR;
                }
                // Whether its programs' own may go to swap: at a look
                // that puts the allowance under what they hold, and at
                // one so close to that line that a sudden need could
                // cross it before the next (closed there, the kernel
                // met a desktop's 110 MB over what was free out of the
                // desktop's own: 78 MB of it, to 72 of the
                // background's). The ceiling before the allowance:
                // closed before an allowance rises out of the band
                // (whatever is still being taken back takes none of
                // their own from here on), open before one falls under
                // them; within the band it stays open through a rise.
                // A session kept by its allowance alone has none while
                // more than the line is available (nothing there would
                // keep a growing build's own out of swap).
                let open = held.all.saturating_sub(held.cache) > want || memory.available < shortage.saturating_add(SUDDEN);
                let high = match &way {
                    Way::AllowanceOnly if memory.available >= shortage => None,
                    _ => Some(want),
                };
                let set = match &way {
                    Way::Whole(swap) => swap.set(open),
                    Way::AllowanceOnly => Ok(()),
                }
                // (Read back at every look: whoever else wrote it, it is
                // what this look decides again.)
                .and_then(|()| taker.decide(bg, high));
                match (set, Held::of(bg)) {
                    (Err(e), _) => {
                        lift(&mut say);
                        say("The background group's allowance couldn't be set", e.to_string());
                    }
                    (Ok(()), None) => {
                        lift(&mut say);
                        say("The background group's memory can't be read", "its allowance is lifted until it can".into());
                    }
                    // What the group holds over its allowance, as it
                    // stands after the write: the taker's to take back,
                    // where the keeper keeps whole.
                    (Ok(()), Some(now)) => {
                        if matches!(way, Way::Whole(_)) {
                            if taker.refused() {
                                say("The background group's memory can't be taken back by its keeper", "its allowance and its swap ceiling are kept all the same".into());
                            } else {
                                let pace = taker.pace();
                                let own_budget = own_part(&held, want, pace.own).min(own_part(&now, want, pace.own));
                                let key = Retry::key(&now, want);
                                let cache = cache_part(&now, want, pace.cache);
                                let try_cache = cache > 0 && pace.cache_retry.ready(key);
                                let may_try_own = cache == 0 || pace.cache_retry.partial_in_backoff(key);
                                if try_cache || (may_try_own && own_budget > 0 && pace.own_retry.ready(key)) {
                                    taker.ask(Ask { high: want, own_budget, reserve, credit: share, desk_swapped });
                                }
                            }
                        }
                    }
                }
            }
            _ => {
                lift(&mut say);
                say("The background group's memory can't be read", "its allowance is lifted until it can".into());
            }
        }
        finish_look();
        wait(wake, None, guard.as_ref(), Some(every));
    }
    // The end. No allowance is what is decided now, so nothing the taker
    // was asked stands; the swap ceiling is asked closed while the
    // taker finishes (not checked: both are lifted and read back just
    // below; a part being taken can't be called off: closed, it is
    // charged no more of the programs' own, though what it had already
    // handed to the disk is still written, and behind a slow disk it is
    // the time that takes), the allowance lifted meanwhile. Then, the session's
    // lock still held, both lifted and read back.
    if let Way::Whole(swap) = &way {
        let _ = swap.set(false);
    }
    let _ = taker.decide(bg, None);
    taker.finish();
    let _ = allow(bg, None);
    if matches!(way, Way::Whole(_)) {
        let _ = write_swap(bg, "max");
    }
    (allowed(bg) == Some(None) && (!matches!(way, Way::Whole(_)) || swap_free(bg)), guard)
}

/// What clearing a group nobody keeps came to ([`clear_if_unkept`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cleared {
    /// No allowance, nothing in the group.
    Nothing,
    /// Something is left there, but another keeps the group now (or is
    /// clearing it): to be looked at again.
    Busy,
    /// Lifted as far as could be written, but not read back as lifted
    /// (the allowance or the swap ceiling: a figure still there, or not
    /// to be read): to be looked at again, and not the same as cleared
    /// (it used to come back as "nothing was left there").
    Unconfirmed { left: Option<usize> },
    /// Cleared: whether an allowance or a swap ceiling left there was
    /// lifted (read back), and how many processes were left behind
    /// ([`empty`]; None: the list can't be read).
    Done { lifted: bool, left: Option<usize> },
}

/// What a supervisor that isn't to keep finds left from one that was (a
/// supervisor killed, and its guard too; or the choice turned off while
/// none ran): an allowance, a swap ceiling, processes still in the
/// group. Cleared, under the lock, if nobody keeps: both lifted,
/// everything moved back to `desk`.
pub fn clear_if_unkept(groups: &Groups, lock_file: &Path) -> Cleared {
    let Ok(_held) = lock(lock_file) else { return Cleared::Busy };
    let finite = allowed(&groups.bg).is_none_or(|a| a.is_some());
    let ceiling = !swap_free(&groups.bg);
    let members = fs::read_to_string(groups.bg.join("cgroup.procs"));
    if !finite && !ceiling && members.as_ref().is_ok_and(|pids| pids.trim().is_empty()) {
        return Cleared::Nothing;
    }
    if finite {
        let _ = allow(&groups.bg, None);
    }
    if ceiling {
        let _ = write_swap(&groups.bg, "max");
    }
    let limits_free = allowed(&groups.bg) == Some(None) && swap_free(&groups.bg);
    let left = empty(&groups.bg, &groups.desk);
    if limits_free { Cleared::Done { lifted: finite || ceiling, left } } else { Cleared::Unconfirmed { left } }
}

/// The arguments a guard is started with, after the program's own.
pub fn guard_args(bg: &Path, lock_file: &Path) -> [PathBuf; 2] {
    [bg.to_owned(), lock_file.to_owned()]
}

/// A guard's whole life (a process of its own, started by
/// [`Keeper::start`] with the keeper's process holding the other end of
/// its input): waits until that end is gone, which it is when the
/// keeper's process has ended, however it ended. Then, if nobody keeps
/// the group (the lock is free: not a keeper stopping in good order,
/// nor another that took over), lifts the allowance and the swap
/// ceiling and moves what is in the group back to the desktop's
/// (`desk`, beside `bg`).
pub fn guard(bg: &Path, lock_file: &Path) {
    // There, and said so (the keeper sets no allowance before it hears
    // this: [`ready`]).
    let _ = std::io::stdout().write_all(READY);
    let _ = std::io::stdout().flush();
    let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
    // (Held across it all: a keeper that starts meanwhile starts after.
    // Nobody keeping, nothing stays in the group either: back among the
    // desktop's, off the idle processors. Until both read back so, for a
    // few seconds: a process born in the group meanwhile, a write the
    // kernel refused for a moment.)
    // (Asked for over a second: what the keeper's process left being
    // born with a copy of the lock is not a keeper.)
    if let Some(_held) = lock_once_free(lock_file, 20, Duration::from_millis(50)) {
        let desk = bg.with_file_name("desk");
        for _ in 0..20 {
            let _ = allow(bg, None);
            let _ = write_swap(bg, "max");
            // (Moved back whether or not the two read back yet.)
            let left = empty(bg, &desk);
            if allowed(bg) == Some(None) && swap_free(bg) && left == Some(0) {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

/// What a guard says once it is there.
const READY: &[u8] = b"r";

/// Whether a guard has said it is there, without waiting for it: None
/// while it has said nothing yet.
fn said_ready(child: &std::process::Child) -> Option<bool> {
    let out = child.stdout.as_ref()?;
    let mut fds = [libc::pollfd { fd: out.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
    let mut byte = [0u8; 1];
    // SAFETY: the guard's own pipe, our buffer of one; read only once
    // the kernel says there is something to read (or the pipe's end).
    unsafe {
        if libc::poll(fds.as_mut_ptr(), 1, 0) != 1 {
            return None;
        }
        Some(libc::read(out.as_raw_fd(), byte.as_mut_ptr().cast(), 1) == 1 && byte == *READY)
    }
}

/// Waits for a guard just started to say it is there (`READY_WITHIN` at
/// the most); one that doesn't is ended and is no guard. For the
/// keeper's start, which a worker thread makes: no look of the keeper's
/// waits so ([`Guard::look`]).
fn ready(mut child: std::process::Child) -> std::io::Result<std::process::Child> {
    let said = child.stdout.as_ref().is_some_and(|out| {
        let mut fds = [libc::pollfd { fd: out.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        let mut byte = [0u8; 1];
        // SAFETY: the guard's own pipe, our buffer of one.
        unsafe { libc::poll(fds.as_mut_ptr(), 1, READY_WITHIN.as_millis() as libc::c_int) == 1 && libc::read(out.as_raw_fd(), byte.as_mut_ptr().cast(), 1) == 1 && byte == *READY }
    });
    if said {
        return Ok(child);
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(std::io::Error::other("it didn't say it was there"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;
    const MB: u64 = 1 << 20;

    #[test]
    fn the_next_look_with_the_same_high_supersedes_queued_work() {
        let dir = tempfile::Builder::new().prefix("olo-background-revision-").tempdir_in("/var/tmp").unwrap();
        fs::write(dir.path().join("memory.high"), (64 * MB).to_string()).unwrap();
        let (notes, _) = mpsc::channel();
        let taker = Taker::start(dir.path(), notes).unwrap();
        taker.decide(dir.path(), Some(64 * MB)).unwrap();
        let first = taker.taking.lock().unwrap().revision;
        taker.decide(dir.path(), Some(64 * MB)).unwrap();
        assert_ne!(first, taker.taking.lock().unwrap().revision);
        taker.finish();
    }

    #[test]
    fn reads_the_kernels_figures() {
        let m = Memory::parse("MemTotal:       65213200 kB\nMemFree:         1000000 kB\nMemAvailable:   30000000 kB\nInactive(file): 15000000 kB\nMapped:          2600000 kB\n").unwrap();
        assert_eq!((m.total, m.free, m.available, m.unused_cache, m.mapped), (65213200 * 1024, 1000000 * 1024, 30000000 * 1024, 15000000 * 1024, 2600000 * 1024));
        assert_eq!(Memory::parse("MemTotal: 1 kB\n"), None);
        // A group's cache is its files' less the files that live in memory.
        let h = Held::parse("4294967296\n", "anon 1073741824\nfile 3221225472\nshmem 2147483648\ninactive_file 536870912\n").unwrap();
        assert_eq!((h.all, h.cache, h.unused_cache), (4 * GB, GB, GB / 2));
        assert_eq!(Held::parse("1\n", "file 1\n"), None);
        assert_eq!(parse_waited("some avg10=0.00 avg60=0.00 avg300=0.00 total=123456\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=99\n"), Some(123456));
        assert_eq!(parse_waited("full avg10=0.00 total=99\n"), None);
    }

    fn memory(free: u64, available: u64, unused: u64, mapped: u64) -> Memory {
        Memory { total: 64 * GB, free, available, unused_cache: unused, mapped }
    }

    #[test]
    fn the_background_keeps_to_what_is_to_be_had_for_nothing() {
        // 64 GB, a reserve of 12% (7.68 GB).
        let reserve = (64.0 * 0.12 * GB as f64) as u64;
        // Plenty free: what it holds and all that is free beyond the reserve.
        let held = Held { all: 4 * GB, cache: GB, unused_cache: GB / 2, locked: 0 };
        let a = allowance(&memory(20 * GB, 40 * GB, 0, 0), &held, 12.0, 1.0, 0);
        assert_eq!(a, (4 * GB + 20 * GB - reserve) / STEP * STEP);
        // Little free, much cache nobody uses (a computer after a day):
        // that counts, less the group's own and what programs have mapped.
        let a = allowance(&memory(GB, 30 * GB, 15 * GB, 2 * GB), &held, 12.0, 1.0, 0);
        assert_eq!(a, (4 * GB + GB + (15 * GB - GB / 2 - 2 * GB) - reserve) / STEP * STEP);
        // The same with the desktop seen waiting: the cache counts for
        // less, and for nothing; then the group has what its programs
        // hold (the floor), its own cache given up.
        let half = allowance(&memory(GB, 30 * GB, 15 * GB, 2 * GB), &held, 12.0, 0.5, 0);
        assert_eq!(half, (4 * GB + GB + (15 * GB - GB / 2 - 2 * GB) / 2 - reserve) / STEP * STEP);
        assert_eq!(allowance(&memory(GB, 30 * GB, 15 * GB, 2 * GB), &held, 12.0, 0.0, 0), 3 * GB);
        // Little to be had at all (available under twice the reserve):
        // under what its programs hold by as much as is missing.
        let short = allowance(&memory(GB / 2, 2 * reserve - GB, 0, 0), &held, 12.0, 1.0, 0);
        assert_eq!(short, 2 * GB);
        // Never under the least.
        assert_eq!(allowance(&memory(0, 0, 0, 0), &Held { all: MB, cache: 0, unused_cache: 0, locked: 0 }, 12.0, 1.0, 0), LEAST);
        // Memory files aren't cache to give up: a job working in /tmp
        // keeps its floor (it was 16 MiB).
        let tmp = Held::parse(&(4 * GB).to_string(), &format!("file {}\nshmem {}\ninactive_file 0\n", 3 * GB + GB / 2, 3 * GB)).unwrap();
        assert_eq!(allowance(&memory(GB / 2, 24 * GB, 0, 0), &tmp, 12.0, 1.0, 0), 4 * GB - GB / 2);
    }

    /// What the keeper takes back itself: the group's cache over its
    /// allowance, a part at a time; its programs' own only where the
    /// allowance is under what they hold.
    #[test]
    fn what_is_over_the_allowance_is_taken_back_in_parts() {
        let held = |all, cache| Held { all, cache, unused_cache: 0, locked: 0 };
        // Under its allowance, or over by less than a step: nothing.
        assert_eq!((cache_part(&held(3 * GB, GB), 4 * GB, CACHE_MOST), own_part(&held(3 * GB, GB), 4 * GB, OWN_MOST)), (0, 0));
        assert_eq!(cache_part(&held(3 * GB, GB), 3 * GB - 8 * MB, CACHE_MOST), 0);
        // Over, with the cache to cover it: of its cache, nothing else.
        assert_eq!((cache_part(&held(3 * GB, GB), 3 * GB - 100 * MB, CACHE_MOST), own_part(&held(3 * GB, GB), 3 * GB - 100 * MB, OWN_MOST)), (100 * MB, 0));
        // A part at a time, as large as the pace says.
        assert_eq!(cache_part(&held(8 * GB, 2 * GB), 7 * GB, CACHE_MOST), CACHE_MOST);
        assert_eq!(cache_part(&held(8 * GB, 2 * GB), 7 * GB, 32 * MB), 32 * MB);
        // The allowance under what its programs hold (the rule's choice,
        // little being to be had): its cache, and of their own what is
        // above it, a part.
        assert_eq!((cache_part(&held(3 * GB, 40 * MB), 2 * GB, CACHE_MOST), own_part(&held(3 * GB, 40 * MB), 2 * GB, OWN_MOST)), (40 * MB, OWN_MOST));
        assert_eq!(own_part(&held(3 * GB, 0), 3 * GB - 20 * MB, OWN_MOST), 20 * MB);
        assert_eq!(own_part(&held(3 * GB, 0), 3 * GB - 20 * MB, 4 * MB), 4 * MB);
        assert_eq!(own_part(&held(3 * GB, 0), 3 * GB - 8 * MB, OWN_MOST), 0);
    }

    /// A part's size follows how long the kernel took over the last.
    #[test]
    fn a_slow_part_makes_the_next_one_smaller() {
        let ms = Duration::from_millis;
        assert_eq!(paced(64 * MB, ms(400), OWN_LEAST, OWN_MOST), 32 * MB);
        assert_eq!(paced(MB, ms(400), OWN_LEAST, OWN_MOST), OWN_LEAST, "never under the least");
        assert_eq!(paced(4 * MB, ms(5), OWN_LEAST, OWN_MOST), 8 * MB);
        assert_eq!(paced(64 * MB, ms(5), OWN_LEAST, OWN_MOST), OWN_MOST, "never over the most");
        assert_eq!(paced(16 * MB, ms(60), OWN_LEAST, OWN_MOST), 16 * MB, "within the aim: as it was");
        // A swap that takes 4 MB a second: from the first part to the
        // least in two looks, a quarter of a second each from then on.
        let mut size = Pace::default().own;
        for _ in 0..3 {
            size = paced(size, Duration::from_secs_f64(size as f64 / (4.0 * MB as f64)), OWN_LEAST, OWN_MOST);
        }
        assert_eq!(size, OWN_LEAST);
    }

    /// The swap ceiling: closed or open; written only where the kernel
    /// has it the other way; none when lifted; and one that can't be
    /// read isn't taken for none.
    #[test]
    fn the_swap_ceiling_is_closed_unless_a_look_opens_it() {
        let dir = tempfile::Builder::new().prefix("olo-background-swap-").tempdir_in("/var/tmp").unwrap();
        // (Neither file there: nothing to keep whole with.)
        assert!(SwapCeiling::of(dir.path()).is_none());
        assert!(swap_free(dir.path()), "no file, no ceiling");
        for (file, text) in [("memory.swap.max", "max\n"), ("memory.swap.current", "0\n"), ("memory.reclaim", "")] {
            fs::write(dir.path().join(file), text).unwrap();
        }
        let swap = SwapCeiling::of(dir.path()).unwrap();
        let ceiling = || fs::read_to_string(dir.path().join("memory.swap.max")).unwrap();
        assert!(swap_free(dir.path()));
        swap.set(false).unwrap();
        assert_eq!(ceiling(), "0");
        assert!(!swap_free(dir.path()));
        swap.set(true).unwrap();
        assert_eq!(ceiling(), "max");
        swap.set(false).unwrap();
        // The same again: not written again (a file that can't be
        // written any more shows it).
        let mut only_read = fs::metadata(dir.path().join("memory.swap.max")).unwrap().permissions();
        only_read.set_readonly(true);
        fs::set_permissions(dir.path().join("memory.swap.max"), only_read.clone()).unwrap();
        swap.set(false).unwrap();
        assert!(swap.set(true).is_err(), "another figure is a write");
        #[allow(clippy::permissions_set_readonly_false)]
        only_read.set_readonly(false);
        fs::set_permissions(dir.path().join("memory.swap.max"), only_read).unwrap();
        swap.set(true).unwrap();
        assert!(swap_free(dir.path()));
        // A ceiling that can't be read isn't known to be none (a folder
        // where the file should be: reading it fails, and not for want
        // of it).
        fs::remove_file(dir.path().join("memory.swap.max")).unwrap();
        assert!(swap_free(dir.path()), "no file, no ceiling");
        fs::create_dir(dir.path().join("memory.swap.max")).unwrap();
        assert!(!swap_free(dir.path()));
    }

    /// The sooner looks begin far enough off that, by the arithmetic
    /// (the looks on time, the need at the rate reckoned with), the band
    /// isn't crossed between two: from where the ordinary look still
    /// holds, a quarter of a second of the need ends at the band's edge
    /// or above; and inside, a twentieth ends at the line or above.
    #[test]
    fn looks_come_sooner_before_the_band_can_be_crossed() {
        const RATE: u64 = 10 << 30;
        for total_gb in [2u64, 4, 8, 16, 64, 256] {
            let line = total_gb * GB * 12 / 100;
            let short = 2 * line;
            // The least available at which a look is still an ordinary one.
            let slow = short + SUDDEN + SUDDEN_SLOW;
            assert!(!near(slow, short) && near(slow - 1, short));
            // A quarter of a second of the need later: the band not crossed (the ceiling still as it was decided).
            assert!(slow - RATE * EVERY.as_millis() as u64 / 1000 >= short + SUDDEN, "{total_gb} GB");
            // And from the band's edge, at the sooner pace, the line isn't.
            assert!(short + SUDDEN - RATE * EVERY_NEAR.as_millis() as u64 / 1000 >= short, "{total_gb} GB");
        }
        // What of the desktop's went to swap lately moves all of it up.
        assert!(near(4 * GB, 2 * GB) && !near(6 * GB, 2 * GB) && near(6 * GB - 1, 3 * GB));
    }

    /// What went to swap lately: what was seen to go, for half as much
    /// every five seconds, never more than is there; what was there
    /// before isn't counted, nor anything while it can't be read.
    #[test]
    fn what_the_desktop_lost_to_swap_lately() {
        let mut now = Instant::now();
        let mut lately = Lately::default();
        let mut after = |lately: &mut Lately, in_swap: Option<u64>, later: Duration| {
            now += later;
            lately.after(in_swap, now)
        };
        let look = Duration::from_millis(250);
        // 2 GB of last week's: not this keeper's to make room for.
        assert_eq!(after(&mut lately, Some(2 * GB), look), 0);
        assert_eq!(after(&mut lately, Some(2 * GB), look), 0);
        // 100 MB more between two looks, and 50 more: counted.
        assert_eq!(after(&mut lately, Some(2 * GB + 100 * MB), look), 100 * MB);
        let both = after(&mut lately, Some(2 * GB + 150 * MB), look);
        assert!(both > 145 * MB && both < 150 * MB, "{both}");
        // Five seconds on, for half; and for next to nothing after a minute.
        let half = after(&mut lately, Some(2 * GB + 150 * MB), HALF);
        assert!(half > 70 * MB && half < 76 * MB, "{half}");
        assert!(after(&mut lately, Some(2 * GB + 150 * MB), Duration::from_secs(60)) < MB);
        // Never more than is in swap: what came back needs no room.
        let mut lately = Lately::default();
        after(&mut lately, Some(0), look);
        assert_eq!(after(&mut lately, Some(100 * MB), look), 100 * MB);
        let remaining = after(&mut lately, Some(10 * MB), look);
        assert!((6 * MB..7 * MB).contains(&remaining));
        // Not to be read: nothing counted, and what it was when it can again.
        assert_eq!(after(&mut lately, None, look), 0);
        let resumed = after(&mut lately, Some(10 * MB), look);
        assert!((6 * MB..remaining).contains(&resumed));

        // A fall pays back: swap up and down around a gigabyte of old
        // counts for what is up now, however often it went (by the rises
        // alone, 909 MiB after half a minute of it).
        let mut oscillating = Lately::default();
        after(&mut oscillating, Some(GB), look);
        for _ in 0..20 {
            assert_eq!(after(&mut oscillating, Some(GB + 64 * MB), look), 64 * MB);
            assert_eq!(after(&mut oscillating, Some(GB), look), 0);
        }
        assert_eq!(after(&mut oscillating, Some(GB + 512 * MB), Duration::from_secs(60)), 0);
        after(&mut oscillating, None, look);
        assert_eq!(after(&mut oscillating, Some(2 * GB), look), 0);
    }

    /// What of the desktop's own went to swap needs its room back before
    /// the background keeps what it holds.
    #[test]
    fn the_desktops_memory_in_swap_comes_before_the_backgrounds_in_memory() {
        // 64 GB, a reserve of 12% (7.68 GB): 16 GB available is ample.
        let held = Held { all: 4 * GB, cache: 0, unused_cache: 0, locked: 0 };
        let ample = memory(GB, 16 * GB, 0, 0);
        assert_eq!(allowance(&ample, &held, 12.0, 0.0, 0), 4 * GB);
        // Half a gigabyte of the desktop's in swap, and room for it to
        // come back with twice the reserve still to be had: nothing changes.
        assert_eq!(allowance(&ample, &held, 12.0, 0.0, GB / 2), 4 * GB);
        // 2 GB of the desktop's in swap: 16 GB available is then 1.36 GB
        // short of twice the reserve and that. The background's own
        // goes under by as much.
        let reserve = (64.0 * 0.12 * GB as f64) as u64;
        let missing = 2 * reserve + 2 * GB - 16 * GB;
        let a = allowance(&ample, &held, 12.0, 0.0, 2 * GB);
        assert!(a < 4 * GB && a >= 4 * GB - missing && a - (4 * GB - missing) < STEP, "{a}");
        // The kernel's own figure short too: both count.
        let short = memory(GB / 2, 2 * reserve - GB, 0, 0);
        assert_eq!(allowance(&short, &held, 12.0, 1.0, 0), 3 * GB);
        assert_eq!(allowance(&short, &held, 12.0, 1.0, GB), 2 * GB);
    }

    /// A guard that ended is replaced without a look waiting for the new
    /// one: on its way until it says it is there, none if it never does.
    #[test]
    fn a_guard_on_its_way_isnt_waited_for() {
        let run = |script: &'static str| std::process::Command::new("/bin/sh").args(["-c", script]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn();
        let mut ended = run("exit 0").unwrap();
        ended.wait().unwrap();
        let mut guard = Guard { child: ended, start: Arc::new(move || run("sleep 0.3; printf r; exec cat")), starting: None, coming: None, ended: Vec::new(), again: Vec::new() };
        let began = Instant::now();
        let Watch::Ended(how) = guard.look() else { panic!("an ended guard is no guard") };
        guard.replace(&how).unwrap();
        assert!(matches!(guard.look(), Watch::Coming));
        assert!(began.elapsed() < Duration::from_millis(200), "nothing waited: {:?}", began.elapsed());
        let t = Instant::now();
        while !matches!(guard.look(), Watch::There) && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(matches!(guard.look(), Watch::There));
        assert!(guard.coming.is_none());
        // One that never says it is there is ended, and is none.
        let _ = guard.child.kill();
        let _ = guard.child.wait();
        guard.start = Arc::new(move || run("exec sleep 30"));
        let Watch::Ended(how) = guard.look() else { panic!("killed, and still there") };
        guard.replace(&how).unwrap();
        let t = Instant::now();
        let ended = loop {
            match guard.look() {
                Watch::Ended(how) => break how,
                _ if t.elapsed() > READY_WITHIN + Duration::from_secs(3) => panic!("still waited for"),
                _ => std::thread::sleep(Duration::from_millis(50)),
            }
        };
        assert_eq!(ended, "it didn't say it was there");
        assert!(t.elapsed() >= READY_WITHIN);
        // Started again too often: given up for now, and said why.
        guard.replace(&ended).unwrap();
        let why = guard.replace("killed").unwrap_err();
        assert!(why.starts_with("it keeps ending (killed)"), "{why}");
        // (The one still being started is let come, then ended: nothing
        // of a test's stays behind.)
        let t = Instant::now();
        while guard.starting.is_some() && t.elapsed() < Duration::from_secs(5) {
            let _ = guard.look();
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = guard.child.kill();
        let _ = guard.child.wait();
        // And one whose program isn't there: said, not waited for.
        guard.again.clear();
        guard.start = Arc::new(|| std::process::Command::new("/var/tmp/no-such-guard").spawn());
        guard.replace("ended").unwrap();
        let t = Instant::now();
        let how = loop {
            match guard.look() {
                Watch::Ended(how) if guard.starting.is_none() => break how,
                _ if t.elapsed() > Duration::from_secs(5) => panic!("never said"),
                _ => std::thread::sleep(Duration::from_millis(10)),
            }
        };
        assert!(how.starts_with("another couldn't be started"), "{how}");
    }

    /// Pages locked in memory aren't cache to give up.
    #[test]
    fn locked_pages_arent_droppable_cache() {
        for shmem in [0, 160 * MB] {
            let stat = format!("file {}\nshmem {shmem}\nactive_file 0\ninactive_file {}\nunevictable {}\n", 128 * MB + shmem, 128 * MB, 160 * MB);
            assert_eq!(Held::parse(&GB.to_string(), &stat).unwrap().cache, 128 * MB);
        }
        let stat = |unevictable: u64| format!("file {}\nshmem 0\ninactive_file 0\nunevictable {unevictable}\n", 400 * MB);
        assert_eq!(Held::parse(&GB.to_string(), &stat(0)).unwrap().cache, 400 * MB);
        assert_eq!(Held::parse(&GB.to_string(), &stat(96 * MB)).unwrap().cache, 304 * MB);
        assert_eq!(Held::parse(&GB.to_string(), &stat(GB)).unwrap().cache, 0, "never under nothing");
        // (A kernel that doesn't say: as before.)
        assert_eq!(Held::parse(&GB.to_string(), &format!("file {}\nshmem 0\ninactive_file 0\n", 400 * MB)).unwrap().cache, 400 * MB);
        // The floor is then over what it could come down to without swap.
        let locked = Held::parse(&(GB + 96 * MB).to_string(), &format!("file {}\nshmem 0\ninactive_file 0\nunevictable {}\n", 96 * MB, 96 * MB)).unwrap();
        assert!(allowance(&memory(0, 40 * GB, 0, 0), &locked, 12.0, 1.0, 0) >= locked.all);
    }

    #[test]
    fn a_step_takes_nothing_from_its_programs() {
        // What its programs hold isn't a whole number of steps: the floor
        // is the step above, not the one below (16 MB of theirs went to
        // swap at each look).
        let held = Held { all: 3 * GB + 5 * MB, cache: 0, unused_cache: 0, locked: 0 };
        let a = allowance(&memory(0, 40 * GB, 0, 0), &held, 12.0, 1.0, 0);
        assert!(a >= held.all && a - held.all < STEP, "{a}");
    }

    #[test]
    fn the_cache_counts_while_the_desktop_doesnt_wait() {
        let look = Duration::from_millis(250);
        let mut now = Instant::now();
        let mut credit = Credit::default();
        let mut after = |credit: &mut Credit, waited: Option<u64>, later: Duration| {
            now += later;
            credit.after(waited, now)
        };
        // The first reading is only where counting starts.
        assert_eq!(after(&mut credit, Some(1_000_000), look), 1.0);
        // Waited 1 ms of 250: under what hurts.
        assert_eq!(after(&mut credit, Some(1_001_000), look), 1.0);
        // Waited 10 ms of 250: halved, and again, to nothing.
        assert_eq!(after(&mut credit, Some(1_011_000), look), 0.5);
        assert_eq!(after(&mut credit, Some(1_031_000), look), 0.25);
        assert_eq!(after(&mut credit, Some(1_051_000), look), 0.125);
        assert_eq!(after(&mut credit, Some(1_071_000), look), 0.0625);
        assert_eq!(after(&mut credit, Some(1_091_000), look), 0.0);
        // Not waiting: back at a tenth of it a second.
        assert!((after(&mut credit, Some(1_091_000), Duration::from_secs(4)) - 0.4).abs() < 1e-4);
        let mut share = 0.0;
        for _ in 0..40 {
            share = after(&mut credit, Some(1_091_000), look);
        }
        assert_eq!(share, 1.0);
        // A reading measured against the time since the one before it: 4 ms
        // waited over a second isn't a wait of 4 ms in a look (it halved).
        assert_eq!(after(&mut credit, Some(1_095_000), Duration::from_secs(1)), 1.0);
        // By the time, not by the look: five looks a twentieth of a second
        // apart, the desktop waiting through each, take what one look a
        // quarter of a second on does (they took it all, halving at each).
        let soon = Duration::from_millis(50);
        let mut waited = 1_095_000;
        let mut share = 1.0;
        for _ in 0..5 {
            waited += 2_000;
            share = after(&mut credit, Some(waited), soon);
        }
        assert!((share - 0.5).abs() < 1e-4, "{share}");
        // And a wait through two seconds unlooked at takes it all.
        assert_eq!(after(&mut credit, Some(waited + 100_000), Duration::from_secs(2)), 0.0);
        for _ in 0..41 {
            share = after(&mut credit, Some(waited + 100_000), look);
        }
        assert_eq!(share, 1.0);
        let waited = waited + 100_000;
        // Not to be read: nothing to believe the kernel's guess by. And
        // readable again, the credit is what it was.
        assert_eq!(after(&mut credit, None, look), 0.0);
        assert_eq!(after(&mut credit, Some(waited), look), 1.0);
    }

    /// A group of plain files: the keeper sets an allowance while
    /// something is in it, sets it again over anyone else's figure, lifts
    /// it when the group empties and when it is stopped; a second keeper
    /// is refused while the first keeps. (Plain files don't wake a wait
    /// as the kernel's do: the keeper is woken by hand here.)
    #[test]
    fn keeps_while_something_is_in_the_group_and_only_one_does() {
        let dir = tempfile::Builder::new().prefix("olo-background-").tempdir_in("/var/tmp").unwrap();
        let (desk, bg) = (dir.path().join("desk"), dir.path().join("bg"));
        for d in [&desk, &bg] {
            fs::create_dir(d).unwrap();
        }
        fs::write(bg.join("cgroup.events"), "populated 0\nfrozen 0\n").unwrap();
        fs::write(bg.join("memory.high"), "123\n").unwrap();
        fs::write(bg.join("memory.current"), (512 * MB).to_string()).unwrap();
        fs::write(bg.join("memory.stat"), "file 0\nshmem 0\ninactive_file 0\n").unwrap();
        // (What the login hands over, so that the keeper keeps whole; a
        // ceiling left from before, as the allowance is.)
        fs::write(bg.join("memory.swap.max"), "4096\n").unwrap();
        fs::write(bg.join("memory.swap.current"), "0\n").unwrap();
        fs::write(bg.join("memory.reclaim"), "").unwrap();
        let ceiling = || fs::read_to_string(bg.join("memory.swap.max")).unwrap();
        fs::write(desk.join("memory.pressure"), "some avg10=0.00 avg60=0.00 avg300=0.00 total=0\n").unwrap();
        let lock_file = dir.path().join("lock");
        let (notes, said) = mpsc::channel();
        let groups = Groups { desk: desk.clone(), bg: bg.clone() };
        let keeper = Keeper::start(groups.clone(), 12.0, &lock_file, None, notes.clone()).unwrap();
        let until = |what: &dyn Fn() -> bool| {
            let t = Instant::now();
            while !what() && t.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(10));
            }
            what()
        };
        // Empty: the figures left from before are lifted.
        assert!(until(&|| allowed(&bg) == Some(None) && swap_free(&bg)));
        // Only one keeps.
        assert!(Keeper::start(groups.clone(), 12.0, &lock_file, None, notes.clone()).is_err());
        // Something in it: an allowance (a wake by hand, as said).
        fs::write(bg.join("cgroup.events"), "populated 1\nfrozen 0\n").unwrap();
        keeper.set_reserve(12.0);
        assert!(until(&|| matches!(allowed(&bg), Some(Some(_)))));
        let set = allowed(&bg).unwrap().unwrap();
        assert!(set >= 512 * MB, "{set}");
        // Its programs keep what they hold: nothing more of theirs may
        // go to swap (the ceiling written before the allowance).
        assert_eq!(ceiling(), "0");
        // Someone else's figure doesn't stay (the keeper's own moves
        // with the computer's free memory: at least what the group holds).
        fs::write(bg.join("memory.high"), "16777216\n").unwrap();
        fs::write(bg.join("memory.swap.max"), "max\n").unwrap();
        assert!(until(&|| matches!(allowed(&bg), Some(Some(v)) if v >= 512 * MB) && ceiling() == "0"));
        // Stopped: both lifted and read back, the group's list read
        // (nothing of a test's is in it); then another may keep.
        fs::write(bg.join("cgroup.procs"), "").unwrap();
        fs::write(desk.join("cgroup.procs"), "").unwrap();
        assert_eq!(keeper.stop(), Ended { lifted: true, left: Some(0) });
        assert_eq!(allowed(&bg), Some(None));
        assert!(swap_free(&bg));
        fs::write(bg.join("memory.high"), "16777216\n").unwrap();
        let again = Keeper::start(groups, 12.0, &lock_file, None, notes).unwrap();
        assert!(until(&|| matches!(allowed(&bg), Some(Some(v)) if v >= 512 * MB)));
        drop(again);
        assert_eq!(allowed(&bg), Some(None));
        assert!(swap_free(&bg));
        assert!(said.try_recv().is_err());
        // A session whose login handed over the allowance and no more:
        // kept by that alone, and said so once, with what brings the rest.
        fs::remove_file(bg.join("memory.swap.max")).unwrap();
        let (notes, said) = mpsc::channel();
        fs::write(bg.join("memory.high"), "16777216").unwrap();
        let alone = Keeper::start(Groups { desk, bg: bg.clone() }, 0.0, &lock_file, None, notes).unwrap();
        assert!(until(&|| allowed(&bg) == Some(None)));
        assert!(alone.stop().lifted);
        assert!(said.try_recv().is_ok_and(|note| note.starts_with("The background group is kept by its allowance alone")));
        assert!(said.try_recv().is_err());
    }

    /// What the reading and the rule of a keeper's look cost: the
    /// computer's figures, the group's, the desktop's waiting and swap,
    /// the allowance read back, the rule (`cargo test --release -p olo-system
    /// what_a_keepers_look_costs -- --ignored --nocapture`; the group's
    /// files are plain ones here, the computer's the kernel's). Not a
    /// whole look: who is in the group, the ceiling, the group read a
    /// second time and the kernel's own work on a real group's files
    /// aren't in it. In a guest the keeper's thread took about a fifth
    /// of a millisecond of processor a look (kernel/TUNING.md).
    #[test]
    #[ignore = "a timing, to read"]
    fn what_a_keepers_look_costs() {
        let dir = tempfile::Builder::new().prefix("olo-background-").tempdir_in("/var/tmp").unwrap();
        fs::write(dir.path().join("memory.high"), "8589934592\n").unwrap();
        fs::write(dir.path().join("memory.current"), (6 * GB).to_string()).unwrap();
        fs::write(dir.path().join("memory.stat"), "anon 4294967296\nfile 2147483648\nkernel 1\nshmem 0\nfile_mapped 0\ninactive_anon 0\nactive_anon 0\ninactive_file 1073741824\nactive_file 0\n")
            .unwrap();
        fs::write(dir.path().join("memory.pressure"), "some avg10=0.00 avg60=0.00 avg300=0.00 total=0\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n").unwrap();
        fs::write(dir.path().join("memory.swap.current"), "0").unwrap();
        let (mut credit, mut lately) = (Credit::default(), Lately::default());
        let n = 5000;
        let t = Instant::now();
        for _ in 0..n {
            let (memory, held) = (Memory::now().unwrap(), Held::of(dir.path()).unwrap());
            let want = allowance(&memory, &held, 12.0, credit.after(waited(dir.path()), Instant::now()), lately.after(in_swap(dir.path()), Instant::now()));
            std::hint::black_box(allowed(dir.path()) != Some(Some(want)));
        }
        println!("a look's reading and rule: {:.1} µs", t.elapsed().as_secs_f64() * 1e6 / f64::from(n));
    }

    /// A lock let go is free at once, though a child of this process is
    /// still on its way to being another program with a copy of it. Every
    /// child is born with a copy of every descriptor, the lock's among
    /// them, until it becomes its program (which closes it); a lock only
    /// closed, not let go, is still held by such a child for that
    /// moment. The keeper's own test met it, 2026-11-21, with other
    /// tests starting programs beside it under load: started again
    /// straight after a start that failed, it was told another kept.
    #[test]
    fn a_lock_let_go_is_free_though_a_child_is_still_being_born_with_its_copy() {
        let dir = tempfile::Builder::new().prefix("olo-background-lock-").tempdir_in("/var/tmp").unwrap();
        let lock_file = dir.path().join("lock");
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // Programs started all the while, from other threads.
        let starters: Vec<_> = (0..4)
            .map(|_| {
                let done = done.clone();
                std::thread::spawn(move || {
                    while !done.load(std::sync::atomic::Ordering::Relaxed) {
                        if let Ok(mut child) = std::process::Command::new("/usr/bin/true").spawn() {
                            let _ = child.wait();
                        }
                    }
                })
            })
            .collect();
        let mut still_held = 0;
        for _ in 0..20_000 {
            let held = lock(&lock_file).unwrap();
            drop(held);
            still_held += lock(&lock_file).is_err() as u32;
        }
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        for t in starters {
            t.join().unwrap();
        }
        assert_eq!(still_held, 0, "of 20,000");
        // One who comes after a holder that ended badly asks more than
        // once: free a moment later, it is theirs; held for good, not.
        let held = lock(&lock_file).unwrap();
        let later = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            drop(held);
        });
        assert!(lock_once_free(&lock_file, 20, Duration::from_millis(20)).is_some());
        later.join().unwrap();
        let _held = lock(&lock_file).unwrap();
        let began = Instant::now();
        assert!(lock_once_free(&lock_file, 3, Duration::from_millis(10)).is_none());
        assert!(began.elapsed() >= Duration::from_millis(20) && began.elapsed() < Duration::from_millis(500), "asked three times, 10 ms apart");
    }

    /// The lock says who keeps; and what a keeper that was killed left
    /// (an allowance, processes in the group) is cleared only where
    /// nobody keeps, and said as it came out.
    #[test]
    fn what_is_left_is_cleared_only_where_nobody_keeps() {
        let dir = tempfile::Builder::new().prefix("olo-background-").tempdir_in("/var/tmp").unwrap();
        let (desk, bg) = (dir.path().join("desk"), dir.path().join("bg"));
        for d in [&desk, &bg] {
            fs::create_dir(d).unwrap();
            fs::write(d.join("cgroup.procs"), "").unwrap();
        }
        let groups = Groups { desk: desk.clone(), bg: bg.clone() };
        let lock_file = dir.path().join("lock");
        fs::write(bg.join("memory.high"), "16777216\n").unwrap();
        // Another keeps: nothing touched, and said to be looked at again.
        let held = lock(&lock_file).unwrap();
        assert!(lock(&lock_file).is_err());
        assert_eq!(clear_if_unkept(&groups, &lock_file), Cleared::Busy);
        assert_eq!(allowed(&bg), Some(Some(16777216)));
        drop(held);
        // Nobody does: lifted (nothing in the group to move).
        assert_eq!(clear_if_unkept(&groups, &lock_file), Cleared::Done { lifted: true, left: Some(0) });
        assert_eq!(allowed(&bg), Some(None));
        // Nothing left: nothing to say.
        assert_eq!(clear_if_unkept(&groups, &lock_file), Cleared::Nothing);
        // A swap ceiling left closed, the allowance already lifted: lifted
        // too (nobody keeping, the group's programs could never be
        // swapped, whatever the computer was short of).
        for (file, text) in [("memory.swap.max", "0\n"), ("memory.swap.current", "0\n"), ("memory.reclaim", "")] {
            fs::write(bg.join(file), text).unwrap();
        }
        assert_eq!(clear_if_unkept(&groups, &lock_file), Cleared::Done { lifted: true, left: Some(0) });
        assert!(swap_free(&bg));
        assert_eq!(clear_if_unkept(&groups, &lock_file), Cleared::Nothing);
        // Someone still in it (a plain file keeps naming it: said as left).
        fs::write(bg.join("cgroup.procs"), "4242\n").unwrap();
        assert_eq!(clear_if_unkept(&groups, &lock_file), Cleared::Done { lifted: false, left: Some(1) });
        assert_eq!(fs::read_to_string(desk.join("cgroup.procs")).unwrap(), "4242");
        // A list that can't be read isn't an empty group: cleared as far
        // as it can be, and how many are left not claimed.
        fs::remove_file(bg.join("cgroup.procs")).unwrap();
        assert_eq!(empty(&bg, &desk), None);
        assert_eq!(clear_if_unkept(&groups, &lock_file), Cleared::Done { lifted: false, left: None });
        // A ceiling that doesn't read back as lifted (not to be read at
        // all, here: a folder where the file is) isn't "cleared", though
        // the group is empty: said as not read back, to be looked at
        // again (it came out as cleared, nothing lifted, nothing left).
        fs::write(bg.join("cgroup.procs"), "").unwrap();
        fs::remove_file(bg.join("memory.swap.max")).unwrap();
        fs::create_dir(bg.join("memory.swap.max")).unwrap();
        assert_eq!(clear_if_unkept(&groups, &lock_file), Cleared::Unconfirmed { left: Some(0) });
        fs::remove_dir(bg.join("memory.swap.max")).unwrap();
        fs::write(bg.join("memory.swap.max"), "max\n").unwrap();
        assert_eq!(clear_if_unkept(&groups, &lock_file), Cleared::Nothing);
        // A guard that can't be started: no keeping.
        fs::write(bg.join("cgroup.procs"), "").unwrap();
        let (notes, _said) = mpsc::channel();
        let (none, silent) = (Some((Path::new("/var/tmp/no-such-guard"), &[][..])), Some((Path::new("/usr/bin/true"), &[][..])));
        for (guard, says) in [(none, "guard couldn't start"), (silent, "didn't say it was there")] {
            let Err(why) = Keeper::start(groups.clone(), 12.0, &lock_file, guard, notes.clone()) else { panic!("kept without its guard") };
            assert!(why.contains(says), "{why}");
        }
        // A guard that ends while the keeper keeps is started again (a
        // harmless program stands in: it ends at once, every time), and
        // an allowance is set only with one there.
        let (notes, said) = mpsc::channel();
        fs::write(bg.join("cgroup.events"), "populated 1\n").unwrap();
        fs::write(bg.join("memory.current"), (512 * MB).to_string()).unwrap();
        fs::write(bg.join("memory.stat"), "file 0\nshmem 0\ninactive_file 0\n").unwrap();
        fs::write(desk.join("memory.pressure"), "some avg10=0.00 avg60=0.00 avg300=0.00 total=0\n").unwrap();
        let keeper = Keeper::start(Groups { desk: desk.clone(), bg: bg.clone() }, 12.0, &lock_file, Some((Path::new("/usr/bin/echo"), &["r"])), notes).unwrap();
        // (It is started again as it ends, the keeper woken by its
        // ending, and three times on…)
        // …and one that keeps ending is given up: no allowance, and said.
        let t = Instant::now();
        while allowed(&bg) != Some(None) && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(allowed(&bg), Some(None));
        assert!(
            said.recv_timeout(Duration::from_secs(5)).is_ok_and(|note| note.starts_with("The background group has no guard: it keeps ending")),
            "started again three times, a look apart, then given up"
        );
        assert!(swap_free(&bg), "nothing of the keeper's stands without a guard");
        assert!(keeper.stop().lifted);
    }

    #[test]
    fn a_process_moved_is_the_one_seen() {
        let me = std::process::id();
        let start = started(me).unwrap();
        let dir = tempfile::Builder::new().prefix("olo-background-").tempdir_in("/var/tmp").unwrap();
        let (to, back) = (dir.path().join("to"), dir.path().join("back"));
        for d in [&to, &back] {
            fs::create_dir(d).unwrap();
            fs::write(d.join("cgroup.procs"), "").unwrap();
        }
        assert_eq!(move_to(&to, (me, start)), Moved::Done);
        assert_eq!(fs::read_to_string(to.join("cgroup.procs")).unwrap(), me.to_string());
        // Not the process seen (its number used again, or it ended):
        // nothing is written.
        fs::write(to.join("cgroup.procs"), "").unwrap();
        assert_eq!(move_to(&to, (me, start + 1)), Moved::Gone);
        assert_eq!(fs::read_to_string(to.join("cgroup.procs")).unwrap(), "");
        // Refused by the kernel (here: nowhere to write).
        assert!(matches!(move_to(&dir.path().join("none"), (me, start)), Moved::Refused(_)));
    }
}
