//! Who goes in the session's background group, and the thread that puts
//! them there and has it kept (`olo_system::background`,
//! `olo_system::session_groups`; kernel/TUNING.md has the why).
//!
//! **Who**: compilers and linkers, by their own name
//! (`olo_protocol::performance::COMPILERS`, of the names the person has
//! down as background work), found at the supervisor's look and moved by
//! one write each, checked to be the process seen. Never what starts
//! them: a build's driver, a cache's server, a shell start programs
//! someone is waiting for, and what a process starts is born in its
//! group. What a compiler held until it was moved stays charged where it
//! was (up to a look's two seconds of its life). Only from the desktop's
//! own group: what someone put elsewhere stays there.
//!
//! **Where it runs**: nothing of this on the supervisor's own (realtime)
//! thread but a list handed over. A thread of its own starts and stops
//! the keeper (which waits for a process and a thread), reads where a
//! process is and moves it (a write the kernel may hold), and tries a
//! refused move again: after 2, 8 and 32 seconds, then says so once.
//!
//! **Off** is the allowance lifted and read back, and everything still
//! in the group moved back among the desktop's (off the idle
//! processors): said as it came out.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use olo_system::{
    background::{self, Keeper, Moved, Seen},
    session_groups::{self, Groups},
};

use crate::sample::Proc;

/// How often a refused move is tried again before it is given up.
const TRIES: u32 = 3;

/// The compilers among `procs` not handed over yet (`sent`: those that
/// were, kept to the processes still there). `background`: whether the
/// person has a name down as background work. `favoured`: the active
/// window's app and everything it started, which the supervisor never
/// lowers either: a compiler among them is left where it is while that
/// window is in front (and goes in at the first look after).
pub fn compilers(procs: &[Proc], background: &dyn Fn(&str) -> bool, favoured: &HashSet<i32>, sent: &mut HashMap<i32, u64>) -> Vec<Seen> {
    let live: HashMap<i32, u64> = procs.iter().map(|p| (p.pid, p.start)).collect();
    sent.retain(|pid, start| live.get(pid) == Some(start));
    procs
        .iter()
        .filter(|p| {
            p.state != 'Z' && olo_protocol::performance::COMPILERS.contains(&p.name.as_str()) && background(&p.name) && !favoured.contains(&p.pid) && sent.insert(p.pid, p.start) != Some(p.start)
        })
        .map(|p| (p.pid as u32, p.start))
        .collect()
}

enum Msg {
    /// On: keep the group (with this reserve), and these to put in it.
    Look {
        reserve: f32,
        new: Vec<Seen>,
    },
    Off,
}

/// The supervisor's end of it.
pub struct Group {
    /// None in a session Olo's login didn't divide.
    to: Option<mpsc::Sender<Msg>>,
    notes: mpsc::Receiver<String>,
    sent: HashMap<i32, u64>,
    /// What the thread was last told: on with this reserve, or off.
    told: Option<Option<f32>>,
    /// Whether the thread keeps the group now (another supervisor may:
    /// until it lets go, everything is offered again at each look).
    kept: Arc<AtomicBool>,
    /// Off, with something left in the group that another supervisor
    /// held then: looked at again at each look until it is cleared.
    left: Arc<AtomicBool>,
}

impl Default for Group {
    fn default() -> Self {
        Group::new()
    }
}

impl Group {
    /// For the session this process is in: its groups found, if Olo's
    /// login made them.
    pub fn new() -> Group {
        let own = session_groups::own().unwrap_or_default();
        let groups = session_groups::find(std::path::Path::new(session_groups::MOUNT), &own);
        // One lock for a login session, where every supervisor of it
        // looks: beside its other run-time files.
        let scope = own.trim_end_matches("/olo/desk").rsplit('/').next().unwrap_or("session").to_owned();
        let lock = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_else(|| "/var/tmp".into())).join(format!("olo-background-{scope}.lock"));
        Group::with(groups, own, lock, Some(PathBuf::from("/proc/self/exe")))
    }

    /// With the groups, the group a process to be moved must be in
    /// (`desk`, as the kernel says it), the lock, and the program that
    /// guards the allowance, all given (a test's are made up).
    pub fn with(groups: Option<Groups>, desk: String, lock: PathBuf, guard: Option<PathBuf>) -> Group {
        let (said, notes) = mpsc::channel();
        let (kept, left) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let to = groups.and_then(|groups| {
            let (to, from) = mpsc::channel();
            let (kept, left) = (kept.clone(), left.clone());
            std::thread::Builder::new().name("background group".into()).spawn(move || work(&groups, &desk, &lock, guard.as_deref(), &from, &said, &kept, &left)).ok()?;
            Some(to)
        });
        Group { to, notes, sent: HashMap::new(), told: None, kept, left }
    }

    /// Whether this session has a background group to keep.
    #[cfg(test)]
    pub fn available(&self) -> bool {
        self.to.is_some()
    }

    fn said(&self) -> Vec<String> {
        self.notes.try_iter().collect()
    }

    /// At the supervisor's look, the choice on: the compilers that have
    /// started since handed over. Never waits. What the thread has said
    /// since the last look comes back, to be noted.
    pub fn look(&mut self, procs: &[Proc], background: &dyn Fn(&str) -> bool, favoured: &HashSet<i32>, reserve: f32) -> Vec<String> {
        let Some(to) = &self.to else { return Vec::new() };
        // Not kept (yet, or another supervisor keeps it): nothing was put
        // in, so nothing counts as handed over, and the thread is asked
        // again at every look until it keeps.
        let kept = self.kept.load(Ordering::Relaxed);
        if !kept {
            self.sent.clear();
        }
        let new = compilers(procs, background, favoured, &mut self.sent);
        if !new.is_empty() || !kept || self.told != Some(Some(reserve)) {
            let _ = to.send(Msg::Look { reserve, new });
            self.told = Some(Some(reserve));
        }
        self.said()
    }

    /// The choice off (or the supervisor itself): told once.
    pub fn off(&mut self) -> Vec<String> {
        let Some(to) = &self.to else { return Vec::new() };
        if self.told != Some(None) || self.left.load(Ordering::Relaxed) {
            let _ = to.send(Msg::Off);
            self.told = Some(None);
            self.sent.clear();
        }
        self.said()
    }
}

/// The thread: until the supervisor is gone (its sender dropped), when
/// the keeper's own end lifts the allowance.
#[allow(clippy::too_many_arguments)]
fn work(groups: &Groups, desk: &str, lock: &std::path::Path, guard: Option<&std::path::Path>, from: &mpsc::Receiver<Msg>, notes: &mpsc::Sender<String>, kept: &AtomicBool, busy: &AtomicBool) {
    let mut keeper: Option<(Keeper, f32)> = None;
    // (Each said once while it stays so.)
    let (mut not_ours, mut gave_up) = (false, false);
    let left = |left: Option<usize>| match left {
        Some(0) => "nothing is in it".to_owned(),
        Some(n) => format!("{n} still in it couldn't be moved back"),
        None => "what is still in it can't be read".to_owned(),
    };
    // A move asked for: how often it was refused, and when it is next tried.
    let mut waiting: HashMap<Seen, (u32, Instant)> = HashMap::new();
    let mut last_put: Option<Instant> = None;
    loop {
        let msg = match waiting.values().map(|(_, at)| *at).min() {
            Some(at) => match from.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(msg) => Some(msg),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            },
            None => match from.recv() {
                Ok(msg) => Some(msg),
                Err(_) => return,
            },
        };
        match msg {
            Some(Msg::Off) => {
                waiting.clear();
                not_ours = false;
                if let Some((keeper, _)) = keeper.take() {
                    // (Lifted, read back and cleared by the keeper itself,
                    // under its lock.)
                    let ended = keeper.stop();
                    kept.store(false, Ordering::Relaxed);
                    let _ = notes.send(if ended.lifted {
                        format!("Turned off: the background group has no allowance of memory any more; {}", left(ended.left))
                    } else {
                        format!("Turned off, but the background group's allowance of memory couldn't be lifted; {}", left(ended.left))
                    });
                } else {
                    // (What a supervisor that was killed left, its guard
                    // too: cleared if nobody keeps it now, looked at
                    // again while another does.)
                    let cleared = background::clear_if_unkept(groups, lock);
                    busy.store(cleared == background::Cleared::Busy, Ordering::Relaxed);
                    if let background::Cleared::Done { lifted, left: still } = cleared {
                        let _ = notes.send(format!(
                            "Off: the background group {}; {}",
                            if lifted { "had an allowance of memory left from before, lifted" } else { "was left with programs in it" },
                            left(still)
                        ));
                    }
                }
            }
            Some(Msg::Look { reserve, new }) => {
                match &keeper {
                    Some((k, kept)) if *kept != reserve => {
                        k.set_reserve(reserve);
                        keeper = keeper.map(|(k, _)| (k, reserve));
                    }
                    Some(_) => {}
                    None => match Keeper::start(groups.clone(), reserve, lock, guard.map(|g| (g, &["--background-guard"][..])), notes.clone()) {
                        Ok(k) => {
                            keeper = Some((k, reserve));
                            kept.store(true, Ordering::Relaxed);
                            not_ours = false;
                        }
                        Err(why) => {
                            if !not_ours {
                                let _ = notes.send(format!("The background group isn't this supervisor's to keep: {why}"));
                            }
                            not_ours = true;
                        }
                    },
                }
                // (Kept by nobody of ours, nothing is put in it.)
                if keeper.is_some() {
                    waiting.extend(new.into_iter().map(|seen| (seen, (0, Instant::now()))));
                }
            }
            None => {}
        }
        let now = Instant::now();
        let due: Vec<Seen> = waiting.iter().filter(|(_, (_, at))| *at <= now).map(|(seen, _)| *seen).collect();
        let mut put = 0;
        for seen in due {
            let tries = waiting.get(&seen).map_or(0, |(tries, _)| *tries);
            // Only out of the desktop's own group: already in the
            // background's, put somewhere by someone (a unit, a
            // container), or gone, it is left where it is.
            let moved = if session_groups::of(seen.0).as_deref() == Some(desk) { background::move_to(&groups.bg, seen) } else { Moved::Gone };
            match moved {
                Moved::Done => put += 1,
                Moved::Gone => {}
                Moved::Refused(why) if tries >= TRIES => {
                    if !gave_up {
                        let _ = notes.send(format!("A compiler couldn't be put in the background group: {why}"));
                    }
                    gave_up = true;
                }
                Moved::Refused(_) => {
                    waiting.insert(seen, (tries + 1, now + Duration::from_secs(2 << (2 * tries))));
                    continue;
                }
            }
            waiting.remove(&seen);
        }
        // (Said when a build begins, not for each of its compilers.)
        if put > 0 {
            if last_put.is_none_or(|t| t.elapsed() > Duration::from_secs(600)) {
                let _ = notes.send("Compilers are in the background group: kept to the memory that is to be had for nothing".into());
            }
            last_put = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::Sample;

    fn proc(pid: i32, name: &str, start: u64) -> Proc {
        Proc { pid, ppid: 1, name: name.into(), state: 'S', start, sample: Sample::default() }
    }

    #[test]
    fn compilers_by_their_own_name_and_once() {
        let background = |name: &str| name != "as";
        let mut sent = HashMap::new();
        let procs = vec![proc(10, "cargo", 1), proc(11, "rustc", 2), proc(12, "kache", 3), proc(13, "ld.lld", 4), proc(14, "as", 5), proc(15, "bash", 6), proc(16, "cc1plus", 7)];
        // A driver, a cache and a shell stay; a name the person took off
        // their list stays; the compilers and the linker go.
        let none = HashSet::new();
        // …and one the active window's app started waits for that window
        // to leave the front.
        let front = HashSet::from([16]);
        assert_eq!(compilers(&procs, &background, &front, &mut sent), vec![(11, 2), (13, 4)]);
        assert_eq!(compilers(&procs, &background, &none, &mut sent), vec![(16, 7)]);
        // Once.
        assert_eq!(compilers(&procs, &background, &none, &mut sent), vec![]);
        // A number used again is another process; one that ended is forgotten.
        let procs = vec![proc(11, "rustc", 9), proc(13, "ld.lld", 4)];
        assert_eq!(compilers(&procs, &background, &none, &mut sent), vec![(11, 9)]);
        assert_eq!(sent.len(), 2);
        let mut zombie = proc(20, "rustc", 1);
        zombie.state = 'Z';
        assert_eq!(compilers(&[zombie], &background, &none, &mut sent), vec![]);
    }

    /// The thread against groups of plain files: a compiler of the
    /// test's own (a `sleep` under a compiler's name) is put in the
    /// background's group and the group is given an allowance; turned
    /// off, the allowance is lifted and what is in the group goes back.
    #[test]
    fn puts_a_compiler_in_and_takes_everything_out_when_turned_off() {
        let dir = tempfile::Builder::new().prefix("olo-supervisor-group-").tempdir_in("/var/tmp").unwrap();
        let (desk, bg) = (dir.path().join("desk"), dir.path().join("bg"));
        for d in [&desk, &bg] {
            std::fs::create_dir(d).unwrap();
            std::fs::write(d.join("cgroup.procs"), "").unwrap();
        }
        std::fs::write(bg.join("cgroup.events"), "populated 1\n").unwrap();
        std::fs::write(bg.join("memory.high"), "max\n").unwrap();
        std::fs::write(bg.join("memory.current"), "1073741824\n").unwrap();
        std::fs::write(bg.join("memory.stat"), "file 0\nshmem 0\ninactive_file 0\n").unwrap();
        std::fs::write(desk.join("memory.pressure"), "some avg10=0.00 avg60=0.00 avg300=0.00 total=0\n").unwrap();
        let cc1 = dir.path().join("cc1");
        std::fs::copy("/usr/bin/sleep", &cc1).unwrap();
        let mut child = std::process::Command::new(&cc1).arg("60").spawn().unwrap();
        let pid = child.id();
        let start = background::started(pid).unwrap();
        let own = session_groups::own().unwrap();
        let mut group = Group::with(Some(Groups { desk: desk.clone(), bg: bg.clone() }), own, dir.path().join("lock"), None);
        assert!(group.available());
        let until = |what: &dyn Fn() -> bool| {
            let t = Instant::now();
            while !what() && t.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(10));
            }
            what()
        };
        let procs = vec![proc(pid as i32, "cc1", start), proc(1, "cargo", 1)];
        let mut notes = group.look(&procs, &|_| true, &HashSet::new(), 12.0);
        assert!(until(&|| std::fs::read_to_string(bg.join("cgroup.procs")).unwrap() == pid.to_string()));
        assert!(until(&|| matches!(background::allowed(&bg), Some(Some(v)) if v >= 1 << 30)));
        // Turned off: lifted; "moved back" (plain files keep what was
        // written to them, so the list still names it and it is said).
        notes.extend(group.off());
        assert!(until(&|| background::allowed(&bg) == Some(None)));
        assert!(until(&|| std::fs::read_to_string(desk.join("cgroup.procs")).unwrap() == pid.to_string()));
        std::thread::sleep(Duration::from_millis(50));
        notes.extend(group.off());
        assert!(notes.iter().any(|n| n.starts_with("Compilers are in the background group")), "{notes:?}");
        assert!(notes.iter().any(|n| n.starts_with("Turned off: the background group has no allowance")), "{notes:?}");
        let _ = child.kill();
        let _ = child.wait();
    }

    /// What the supervisor's own thread pays at each look: the filter of
    /// its process table (`cargo test --release -p olo-supervisor
    /// what_a_look_costs -- --ignored --nocapture`).
    #[test]
    #[ignore = "a timing, to read"]
    fn what_a_look_costs() {
        let names = ["bash", "olo-app-server", "chrome", "rustc", "cargo", "kache", "cc1plus", "node", "ld.lld", "sleep"];
        let procs: Vec<Proc> = (0..750).map(|i| proc(1000 + i, names[i as usize % names.len()], 7 * i as u64)).collect();
        let background = |name: &str| name != "node";
        let mut sent = HashMap::new();
        let none = HashSet::new();
        assert_eq!(compilers(&procs, &background, &none, &mut sent).len(), 225);
        let n = 2000;
        let t = Instant::now();
        for _ in 0..n {
            std::hint::black_box(compilers(&procs, &background, &none, &mut sent));
        }
        println!("a look among 750 processes, 225 of them compilers already handed over: {:.1} µs", t.elapsed().as_secs_f64() * 1e6 / f64::from(n));
    }

    #[test]
    fn nothing_to_do_where_the_session_isnt_divided() {
        let mut group = Group::with(None, "/user.slice/user-1000.slice/session-3.scope".into(), "/var/tmp/none".into(), None);
        assert!(!group.available());
        assert!(group.look(&[proc(11, "rustc", 2)], &|_| true, &HashSet::new(), 12.0).is_empty());
        assert!(group.off().is_empty());
    }
}
