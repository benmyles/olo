//! Background work kept to the memory that is to be had for nothing
//! (kernel/TUNING.md has the measurements and the two reviews behind
//! every choice here).
//!
//! Linux has no link between a process's priority and what is taken from
//! memory first; it chooses by control group. Olo's login divides a
//! login session in two (`session_groups`: `desk`, and `bg` beside it),
//! the supervisor puts compilers and linkers in `bg`, and one keeper
//! (here) sets `bg`'s allowance (`memory.high`) four times a second
//! while anything is in it: what the group holds plus what is to be had
//! for nothing, less a reserve. Past its allowance the kernel takes the
//! group's own cache and slows its own programs; nothing is ended and
//! nothing refused.
//!
//! What the second review asked of a keeper, and where it is here:
//! - **One owner.** A lock ([`Keeper::start`]): a second supervisor
//!   reaching the same session neither keeps nor lifts.
//! - **Nobody else's figure stays.** Every look reads the allowance back
//!   before deciding, so a value written by anyone else is seen.
//! - **An end by any road lifts it.** Told to stop, the keeper lifts and
//!   reads back ([`Keeper::stop`]); killed, a guard that outlives it
//!   does ([`guard`]: a process waiting on a pipe the keeper's process
//!   holds the other end of).
//! - **Nothing on anyone's realtime thread.** The keeper is a thread at
//!   the background's own priority (a write under what the group holds
//!   has the kernel take pages back in the writer's time); starting and
//!   stopping it are for a worker thread too (`stop` waits for it).
//! - **Errors aren't emptiness.** What can't be read lifts the allowance
//!   and is said, once.

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
/// The allowance's steps: it is written only when it moves by one.
const STEP: u64 = 16 << 20;
/// The least an allowance may be.
const LEAST: u64 = 16 << 20;

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
    /// Of that, files' cache the kernel can drop and read again: the
    /// group's `file` less its `shmem`. (`file` counts files that live in
    /// memory too, tmpfs and shared memory, which can only go to swap: a
    /// job working in /tmp had 3 GiB of it taken for cache and no floor
    /// under its allowance.)
    pub cache: u64,
    /// Of that cache, what nobody is using (`inactive_file`).
    pub unused_cache: u64,
}

impl Held {
    /// The group whose cgroup folder is `dir`, now.
    pub fn of(dir: &Path) -> Option<Held> {
        Held::parse(&fs::read_to_string(dir.join("memory.current")).ok()?, &fs::read_to_string(dir.join("memory.stat")).ok()?)
    }

    fn parse(current: &str, stat: &str) -> Option<Held> {
        let figure = |key: &str| stat.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(' ')?.trim().parse::<u64>().ok());
        Some(Held { all: current.trim().parse().ok()?, cache: figure("file")?.saturating_sub(figure("shmem")?), unused_cache: figure("inactive_file")? })
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
/// all (less than twice the reserve): then by as much as is missing, and
/// the background's own memory is what goes to swap first. In steps
/// (`STEP`: the allowance down to one, the least it may be up to one, so
/// that a step never takes what the rule leaves), and at least `LEAST`.
pub fn allowance(memory: &Memory, held: &Held, reserve: f32, credit: f32) -> u64 {
    let reserve = (memory.total as f64 * f64::from(reserve.clamp(0.0, 50.0)) / 100.0) as u64;
    let unused = memory.unused_cache.saturating_sub(held.unused_cache + memory.mapped);
    let to_be_had = memory.free + (unused as f64 * f64::from(credit.clamp(0.0, 1.0))) as u64;
    let wanted = (held.all + to_be_had).saturating_sub(reserve);
    let own = held.all.saturating_sub(held.cache);
    let floor = own.saturating_sub((2 * reserve).saturating_sub(memory.available));
    (wanted / STEP * STEP).max(floor.div_ceil(STEP) * STEP).max(LEAST)
}

/// How much of the unused cache counts as to be had for nothing.
///
/// "Unused" is the kernel's guess, and wrong both ways (a file a program
/// read sixty-four times running was all unused to the
/// multi-generational LRU). So it is believed only while the desktop
/// isn't seen to wait on memory: its own group says how long its
/// programs have waited (`desk/memory.pressure`, the kernel's total for
/// "some"). Between two readings in which they waited more than `HURTS`
/// of the time, the credit halves (to nothing, under a sixteenth);
/// without, it comes back at a tenth a second of all of it: quick to
/// give way, slow to take. Each reading is measured against the time
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
            self.share = if hurt { if self.share < 1.0 / 8.0 { 0.0 } else { self.share / 2.0 } } else { (self.share + over.as_secs_f32() / BACK.as_secs_f32()).min(1.0) };
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

// (Opened as a shell's `>` opens it: the kernel's files take a write as
// the whole value either way, and a test's plain file is left holding
// only what was written.)
fn allow(dir: &Path, value: Option<u64>) -> std::io::Result<()> {
    fs::OpenOptions::new().write(true).truncate(true).open(dir.join("memory.high"))?.write_all(value.map_or("max".to_owned(), |v| v.to_string()).as_bytes())
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
    _lock: fs::File,
}

fn lock(path: &Path) -> Result<fs::File, String> {
    let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    // SAFETY: a plain system call on our descriptor.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("another supervisor keeps this session's background group".into());
    }
    Ok(file)
}

impl Keeper {
    /// Starts keeping `groups`' background group, with `reserve` percent
    /// of memory left for everyone else. `lock_file`: the file whose lock
    /// says who keeps (one for a login session; held by another, an Err,
    /// and nothing is touched). `guard`: the program, and its arguments
    /// before the two [`guard_args`] gives, that lifts the allowance if
    /// this process ends without doing it (None: no guard, a test's).
    /// What goes wrong while it keeps is sent to `notes`, each kind once.
    /// Not for a realtime thread: it starts a process.
    pub fn start(groups: Groups, reserve: f32, lock_file: &Path, guard: Option<(&Path, &[&str])>, notes: mpsc::Sender<String>) -> Result<Keeper, String> {
        let held = lock(lock_file)?;
        // SAFETY: a new descriptor of our own.
        let wake = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if wake < 0 {
            return Err(format!("no way to wake the keeper: {}", std::io::Error::last_os_error()));
        }
        // SAFETY: just made, owned by nothing else.
        let wake = Arc::new(unsafe { OwnedFd::from_raw_fd(wake) });
        // (No guard, no keeping: an allowance that a kill would leave
        // standing isn't set.)
        let guard = match guard {
            Some((program, before)) => {
                let (program, before, after) = (program.to_owned(), before.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>(), guard_args(&groups.bg, lock_file));
                let start: Box<dyn Fn() -> std::io::Result<std::process::Child> + Send> =
                    Box::new(move || std::process::Command::new(&program).args(&before).args(&after).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().and_then(ready));
                Some(Guard { child: start().map_err(|e| format!("its guard couldn't start ({e})"))?, start, again: Vec::new() })
            }
            None => None,
        };
        let told = Arc::new(Mutex::new(Told { reserve, stop: false }));
        let thread = std::thread::Builder::new()
            .name("background keeper".into())
            .spawn({
                let (told, wake) = (told.clone(), wake.clone());
                let groups = groups.clone();
                move || keep(&groups, &told, &wake, &notes, guard)
            })
            .map_err(|e| e.to_string())?;
        Ok(Keeper { told, wake, thread: Some(thread), groups, _lock: held })
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
        // The guard after the keeper (it finds the pipe closed, the lock
        // still ours, and goes without touching anything).
        if let Some(mut guard) = guard {
            drop(guard.child.stdin.take());
            let _ = guard.child.wait();
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
/// how to start another: an allowance is only ever set while one is
/// there.
struct Guard {
    child: std::process::Child,
    start: Box<dyn Fn() -> std::io::Result<std::process::Child> + Send>,
    /// When it was started again, lately.
    again: Vec<Instant>,
}

/// How often in a minute a guard that ended is started again before it
/// is taken to be one that can't stay.
const GUARD_AGAIN: usize = 3;

impl Guard {
    /// Whether a guard is there now: the one started, or, that one
    /// having ended (it should outlive the keeper: killed, or a fault of
    /// its own), another started in its place. An Err says why none is.
    fn there(&mut self) -> Result<(), String> {
        match self.child.try_wait() {
            Ok(None) => Ok(()),
            ended => {
                self.again.retain(|at| at.elapsed() < Duration::from_secs(60));
                if self.again.len() >= GUARD_AGAIN {
                    return Err(format!("it keeps ending ({ended:?}): started again {GUARD_AGAIN} times in a minute"));
                }
                self.child = (self.start)().map_err(|e| format!("it ended ({ended:?}) and another couldn't be started ({e})"))?;
                self.again.push(Instant::now());
                Ok(())
            }
        }
    }
}

/// The background's own priority for the calling thread: a write of the
/// allowance under what the group holds reclaims in the writer's time.
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
    // (The guard's end of its pipe hangs up when the guard ends.)
    let guard = guard.and_then(|g| g.child.stdout.as_ref()).map_or(-1, |out| out.as_raw_fd());
    let mut fds = [
        libc::pollfd { fd: wake.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        libc::pollfd { fd: events.map_or(-1, |e| e.as_raw_fd()), events: libc::POLLPRI, revents: 0 },
        libc::pollfd { fd: guard, events: 0, revents: 0 },
    ];
    // SAFETY: our descriptors, our array.
    unsafe {
        libc::poll(fds.as_mut_ptr(), 3, most.map_or(-1, |d| d.as_millis() as libc::c_int));
        let mut count = [0u8; 8];
        libc::read(wake.as_raw_fd(), count.as_mut_ptr().cast(), 8);
    }
}

fn keep(groups: &Groups, told: &Mutex<Told>, wake: &OwnedFd, notes: &mpsc::Sender<String>, mut guard: Option<Guard>) -> (bool, Option<Guard>) {
    lower_this_thread();
    let bg = &groups.bg;
    let mut said = std::collections::HashSet::new();
    let mut say = |what: &'static str, why: String| {
        if said.insert(what) {
            let _ = notes.send(format!("{what}: {why}"));
        }
    };
    // (Nothing of anyone's left standing while nothing is kept.)
    let lift = |say: &mut dyn FnMut(&'static str, String)| {
        if allowed(bg) != Some(None)
            && let Err(e) = allow(bg, None)
        {
            say("The background group's allowance couldn't be lifted", e.to_string());
        }
    };
    let mut events = fs::File::open(bg.join("cgroup.events")).ok();
    let mut credit = Credit::default();
    loop {
        let (reserve, stop) = {
            let t = told.lock().unwrap_or_else(|e| e.into_inner());
            (t.reserve, t.stop)
        };
        if stop {
            break;
        }
        match events.as_mut().and_then(populated) {
            Some(true) => {}
            Some(false) => {
                // (The credit stays as it was: what the desktop was seen
                // to wait holds for the next compiler, a moment later.)
                lift(&mut say);
                wait(wake, events.as_ref(), None, None);
                continue;
            }
            None => {
                // Not to be read is not empty: nothing is kept on a guess.
                lift(&mut say);
                say("The background group can't be read", bg.join("cgroup.events").display().to_string());
                events = fs::File::open(bg.join("cgroup.events")).ok();
                wait(wake, None, None, Some(Duration::from_secs(2)));
                continue;
            }
        }
        // (No allowance without a guard there to clear it: one that ended
        // is started again, and until one is, the group has none.)
        if let Some(Err(why)) = guard.as_mut().map(Guard::there) {
            lift(&mut say);
            say("The background group has no guard", why);
            wait(wake, None, None, Some(Duration::from_secs(2)));
            continue;
        }
        match (Memory::now(), Held::of(bg)) {
            (Some(memory), Some(held)) => {
                let desk = waited(&groups.desk);
                if desk.is_none() {
                    say("The desktop's waiting on memory can't be read", "only free memory counts for the background group until it can".into());
                }
                let want = allowance(&memory, &held, reserve, credit.after(desk, Instant::now()));
                // (Read back at every look: whoever else wrote it, it is
                // what this look decides again.)
                if allowed(bg) != Some(Some(want))
                    && let Err(e) = allow(bg, Some(want))
                {
                    say("The background group's allowance couldn't be set", e.to_string());
                }
            }
            _ => {
                lift(&mut say);
                say("The background group's memory can't be read", "its allowance is lifted until it can".into());
            }
        }
        wait(wake, None, guard.as_ref(), Some(EVERY));
    }
    let _ = allow(bg, None);
    (allowed(bg) == Some(None), guard)
}

/// What clearing a group nobody keeps came to ([`clear_if_unkept`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cleared {
    /// No allowance, nothing in the group.
    Nothing,
    /// Something is left there, but another keeps the group now (or is
    /// clearing it): to be looked at again.
    Busy,
    /// Cleared: whether an allowance was lifted (read back), and how many
    /// processes were left behind ([`empty`]; None: the list can't be
    /// read).
    Done { lifted: bool, left: Option<usize> },
}

/// What a supervisor that isn't to keep finds left from one that was (a
/// supervisor killed, and its guard too; or the choice turned off while
/// none ran): an allowance, processes still in the group. Cleared,
/// under the lock, if nobody keeps: the allowance lifted, everything
/// moved back to `desk`.
pub fn clear_if_unkept(groups: &Groups, lock_file: &Path) -> Cleared {
    let Ok(_held) = lock(lock_file) else { return Cleared::Busy };
    let finite = allowed(&groups.bg).is_none_or(|a| a.is_some());
    let members = fs::read_to_string(groups.bg.join("cgroup.procs"));
    if !finite && members.as_ref().is_ok_and(|pids| pids.trim().is_empty()) {
        return Cleared::Nothing;
    }
    let lifted = finite && allow(&groups.bg, None).is_ok() && allowed(&groups.bg) == Some(None);
    Cleared::Done { lifted, left: empty(&groups.bg, &groups.desk) }
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
/// nor another that took over), lifts the allowance and moves what is
/// in the group back to the desktop's (`desk`, beside `bg`).
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
    if let Ok(_held) = lock(lock_file) {
        let desk = bg.with_file_name("desk");
        for _ in 0..20 {
            let _ = allow(bg, None);
            if allowed(bg) == Some(None) && empty(bg, &desk) == Some(0) {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

/// What a guard says once it is there.
const READY: &[u8] = b"r";

/// Waits for a guard just started to say it is there (two seconds at the
/// most); one that doesn't is ended and is no guard.
fn ready(mut child: std::process::Child) -> std::io::Result<std::process::Child> {
    let said = child.stdout.as_ref().is_some_and(|out| {
        let mut fds = [libc::pollfd { fd: out.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        let mut byte = [0u8; 1];
        // SAFETY: the guard's own pipe, our buffer of one.
        unsafe { libc::poll(fds.as_mut_ptr(), 1, 2000) == 1 && libc::read(out.as_raw_fd(), byte.as_mut_ptr().cast(), 1) == 1 && byte == *READY }
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
        let held = Held { all: 4 * GB, cache: GB, unused_cache: GB / 2 };
        let a = allowance(&memory(20 * GB, 40 * GB, 0, 0), &held, 12.0, 1.0);
        assert_eq!(a, (4 * GB + 20 * GB - reserve) / STEP * STEP);
        // Little free, much cache nobody uses (a computer after a day):
        // that counts, less the group's own and what programs have mapped.
        let a = allowance(&memory(GB, 30 * GB, 15 * GB, 2 * GB), &held, 12.0, 1.0);
        assert_eq!(a, (4 * GB + GB + (15 * GB - GB / 2 - 2 * GB) - reserve) / STEP * STEP);
        // The same with the desktop seen waiting: the cache counts for
        // less, and for nothing; then the group has what its programs
        // hold (the floor), its own cache given up.
        let half = allowance(&memory(GB, 30 * GB, 15 * GB, 2 * GB), &held, 12.0, 0.5);
        assert_eq!(half, (4 * GB + GB + (15 * GB - GB / 2 - 2 * GB) / 2 - reserve) / STEP * STEP);
        assert_eq!(allowance(&memory(GB, 30 * GB, 15 * GB, 2 * GB), &held, 12.0, 0.0), 3 * GB);
        // Little to be had at all (available under twice the reserve):
        // under what its programs hold by as much as is missing.
        let short = allowance(&memory(GB / 2, 2 * reserve - GB, 0, 0), &held, 12.0, 1.0);
        assert_eq!(short, 2 * GB);
        // Never under the least.
        assert_eq!(allowance(&memory(0, 0, 0, 0), &Held { all: MB, cache: 0, unused_cache: 0 }, 12.0, 1.0), LEAST);
        // Memory files aren't cache to give up: a job working in /tmp
        // keeps its floor (it was 16 MiB).
        let tmp = Held::parse(&(4 * GB).to_string(), &format!("file {}\nshmem {}\ninactive_file 0\n", 3 * GB + GB / 2, 3 * GB)).unwrap();
        assert_eq!(allowance(&memory(GB / 2, 24 * GB, 0, 0), &tmp, 12.0, 1.0), 4 * GB - GB / 2);
    }

    #[test]
    fn a_step_takes_nothing_from_its_programs() {
        // What its programs hold isn't a whole number of steps: the floor
        // is the step above, not the one below (16 MB of theirs went to
        // swap at each look).
        let held = Held { all: 3 * GB + 5 * MB, cache: 0, unused_cache: 0 };
        let a = allowance(&memory(0, 40 * GB, 0, 0), &held, 12.0, 1.0);
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
        // Not to be read: nothing to believe the kernel's guess by. And
        // readable again, the credit is what it was.
        assert_eq!(after(&mut credit, None, look), 0.0);
        assert_eq!(after(&mut credit, Some(1_095_000), look), 1.0);
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
        // Empty: a figure left from before is lifted.
        assert!(until(&|| allowed(&bg) == Some(None)));
        // Only one keeps.
        assert!(Keeper::start(groups.clone(), 12.0, &lock_file, None, notes.clone()).is_err());
        // Something in it: an allowance (a wake by hand, as said).
        fs::write(bg.join("cgroup.events"), "populated 1\nfrozen 0\n").unwrap();
        keeper.set_reserve(12.0);
        assert!(until(&|| matches!(allowed(&bg), Some(Some(_)))));
        let set = allowed(&bg).unwrap().unwrap();
        assert!(set >= 512 * MB, "{set}");
        // Someone else's figure doesn't stay (the keeper's own moves
        // with the computer's free memory: at least what the group holds).
        fs::write(bg.join("memory.high"), "16777216\n").unwrap();
        assert!(until(&|| matches!(allowed(&bg), Some(Some(v)) if v >= 512 * MB)));
        // Stopped: lifted and read back, the group's list read (nothing
        // of a test's is in it); then another may keep.
        fs::write(bg.join("cgroup.procs"), "").unwrap();
        fs::write(desk.join("cgroup.procs"), "").unwrap();
        assert_eq!(keeper.stop(), Ended { lifted: true, left: Some(0) });
        assert_eq!(allowed(&bg), Some(None));
        fs::write(bg.join("memory.high"), "16777216\n").unwrap();
        let again = Keeper::start(groups, 12.0, &lock_file, None, notes).unwrap();
        assert!(until(&|| matches!(allowed(&bg), Some(Some(v)) if v >= 512 * MB)));
        drop(again);
        assert_eq!(allowed(&bg), Some(None));
        assert!(said.try_recv().is_err());
    }

    /// What a keeper's look costs while something is in the group: the
    /// computer's figures, the group's, the desktop's waiting, the
    /// allowance read back, the rule (`cargo test --release -p olo-system
    /// what_a_keepers_look_costs -- --ignored --nocapture`; the group's
    /// files are plain ones here, the computer's the kernel's).
    #[test]
    #[ignore = "a timing, to read"]
    fn what_a_keepers_look_costs() {
        let dir = tempfile::Builder::new().prefix("olo-background-").tempdir_in("/var/tmp").unwrap();
        fs::write(dir.path().join("memory.high"), "8589934592\n").unwrap();
        fs::write(dir.path().join("memory.current"), (6 * GB).to_string()).unwrap();
        fs::write(dir.path().join("memory.stat"), "anon 4294967296\nfile 2147483648\nkernel 1\nshmem 0\nfile_mapped 0\ninactive_anon 0\nactive_anon 0\ninactive_file 1073741824\nactive_file 0\n")
            .unwrap();
        fs::write(dir.path().join("memory.pressure"), "some avg10=0.00 avg60=0.00 avg300=0.00 total=0\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n").unwrap();
        let mut credit = Credit::default();
        let n = 5000;
        let t = Instant::now();
        for _ in 0..n {
            let (memory, held) = (Memory::now().unwrap(), Held::of(dir.path()).unwrap());
            let want = allowance(&memory, &held, 12.0, credit.after(waited(dir.path()), Instant::now()));
            std::hint::black_box(allowed(dir.path()) != Some(Some(want)));
        }
        println!("a keeper's look: {:.1} µs", t.elapsed().as_secs_f64() * 1e6 / f64::from(n));
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
        // Someone still in it (a plain file keeps naming it: said as left).
        fs::write(bg.join("cgroup.procs"), "4242\n").unwrap();
        assert_eq!(clear_if_unkept(&groups, &lock_file), Cleared::Done { lifted: false, left: Some(1) });
        assert_eq!(fs::read_to_string(desk.join("cgroup.procs")).unwrap(), "4242");
        // A list that can't be read isn't an empty group: cleared as far
        // as it can be, and how many are left not claimed.
        fs::remove_file(bg.join("cgroup.procs")).unwrap();
        assert_eq!(empty(&bg, &desk), None);
        assert_eq!(clear_if_unkept(&groups, &lock_file), Cleared::Done { lifted: false, left: None });
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
        assert!(said.recv_timeout(Duration::from_secs(1)).is_ok_and(|note| note.starts_with("The background group has no guard: it keeps ending")));
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
