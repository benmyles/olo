//! Tests holding the pool's slots, and ending one (Ben's Kache request:
//! "a test that's holding slots shown and ended"). A test binary run
//! through kache's test runner holds a marker a slot under
//! `scheduler/tests/`, each locked (`flock`) for as long as it runs; the
//! kernel's list of locks (`/proc/locks`) names the process holding each
//! (its fifth field), so each test is found without taking anything: its
//! process, its program, how many slots, since when. Ended with SIGTERM,
//! only a process of this user's that still holds a marker (checked again
//! just before).

use std::{collections::BTreeMap, path::Path};

/// A test holding slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub pid: i32,
    /// Its program's name ("olo_kit-1be4ef95d8cf4dc4" → "olo_kit").
    pub name: String,
    /// Its project's folder's name (where it runs).
    pub project: String,
    pub slots: u32,
    /// When it started (ms since 1970; 0: not known).
    pub since_ms: u64,
}

/// The tests holding slots in the store at `dir`, most slots first.
pub fn held(dir: &Path) -> Vec<Held> {
    held_in(dir, Path::new("/proc"))
}

pub fn held_in(dir: &Path, proc: &Path) -> Vec<Held> {
    use std::os::unix::fs::MetadataExt as _;
    let locks = std::fs::read_to_string(proc.join("locks")).unwrap_or_default();
    // inode → pid, each lock held (not one waited for: "->").
    let holders: BTreeMap<u64, i32> = locks
        .lines()
        .filter(|l| !l.contains("->"))
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            Some((f.get(5)?.rsplit(':').next()?.parse().ok()?, f.get(4)?.parse().ok()?))
        })
        .collect();
    let mut slots: BTreeMap<i32, u32> = BTreeMap::new();
    for e in std::fs::read_dir(dir.join("scheduler/tests")).into_iter().flatten().flatten() {
        if let Some(pid) = e.metadata().ok().and_then(|m| holders.get(&m.ino()).copied()) {
            *slots.entry(pid).or_default() += 1;
        }
    }
    let boot_ms = boot_ms(proc);
    let mut out: Vec<Held> = slots
        .into_iter()
        .filter_map(|(pid, slots)| {
            let p = proc.join(pid.to_string());
            let cmd = std::fs::read(p.join("cmdline")).ok()?;
            let program = cmd.split(|b| *b == 0).next().map(|a| String::from_utf8_lossy(a).into_owned()).unwrap_or_default();
            let base = program.rsplit('/').next().unwrap_or_default();
            // (A test binary's name ends in its unit's hash.)
            let name = match base.rsplit_once('-') {
                Some((n, h)) if h.len() == 16 && h.chars().all(|c| c.is_ascii_hexdigit()) => n.to_owned(),
                _ => base.to_owned(),
            };
            let project = std::fs::read_link(p.join("cwd")).ok().and_then(|c| c.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
            Some(Held { pid, name, project, slots, since_ms: started_ms(&p, boot_ms) })
        })
        .collect();
    out.sort_by(|a, b| b.slots.cmp(&a.slots).then(a.pid.cmp(&b.pid)));
    out
}

/// When the computer started (ms since 1970).
fn boot_ms(proc: &Path) -> u64 {
    let up: f64 = std::fs::read_to_string(proc.join("uptime")).ok().and_then(|t| t.split_whitespace().next()?.parse().ok()).unwrap_or(0.0);
    crate::data::now_ms().saturating_sub((up * 1000.0) as u64)
}

/// When process `p` started (ms since 1970), from its stat's start time
/// (clock ticks since boot).
fn started_ms(p: &Path, boot_ms: u64) -> u64 {
    let stat = std::fs::read_to_string(p.join("stat")).unwrap_or_default();
    // (After the command's closing parenthesis: field 22 is the 20th.)
    let after = stat.rsplit_once(')').map_or("", |(_, r)| r);
    let ticks: u64 = after.split_whitespace().nth(19).and_then(|t| t.parse().ok()).unwrap_or(0);
    // SAFETY: sysconf has no preconditions.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    if ticks == 0 { 0 } else { boot_ms + ticks * 1000 / hz }
}

/// End test `pid` (SIGTERM), if it's still one of `dir`'s tests holding
/// slots and this user's.
pub fn end(dir: &Path, pid: i32) -> Result<(), String> {
    if !held(dir).iter().any(|h| h.pid == pid) {
        return Err("that test no longer holds slots".into());
    }
    // SAFETY: kill with a pid and a signal has no memory effects; the
    // kernel refuses another user's process (EPERM).
    if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 { Ok(()) } else { Err(format!("it couldn't be ended: {}", std::io::Error::last_os_error())) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store whose test markers two processes hold (as in /proc/locks),
    /// and a fake /proc for them.
    #[test]
    fn tests_holding_slots() {
        use std::os::unix::fs::MetadataExt as _;
        let root = std::path::PathBuf::from(format!("/var/tmp/olo-kache-held-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tests = root.join("store/scheduler/tests");
        std::fs::create_dir_all(&tests).unwrap();
        let ino = |n: &str| {
            std::fs::write(tests.join(n), "").unwrap();
            std::fs::metadata(tests.join(n)).unwrap().ino()
        };
        let (a, b, c) = (ino("3"), ino("4"), ino("5"));
        ino("6");
        let proc = root.join("proc");
        std::fs::create_dir_all(proc.join("4100")).unwrap();
        std::fs::create_dir_all(proc.join("4200")).unwrap();
        std::fs::write(
            proc.join("locks"),
            format!("1: FLOCK  ADVISORY  WRITE 4100 00:24:{a} 0 EOF\n2: FLOCK  ADVISORY  WRITE 4100 00:24:{b} 0 EOF\n3: FLOCK  ADVISORY  WRITE 4200 00:24:{c} 0 EOF\n3: -> FLOCK  ADVISORY  WRITE 4300 00:24:{c} 0 EOF\n"),
        )
        .unwrap();
        std::fs::write(proc.join("4100/cmdline"), b"/home/ben/src/olo/target/debug/build/olo-kit/1be4ef95d8cf4dc4/out/olo_kit-1be4ef95d8cf4dc4\0--test-threads=4\0").unwrap();
        std::fs::write(proc.join("4200/cmdline"), b"/tmp/x/serde_test\0").unwrap();
        std::os::unix::fs::symlink("/home/ben/src/olo", proc.join("4100/cwd")).unwrap();
        std::fs::write(proc.join("uptime"), "1000.00 2.00\n").unwrap();
        let h = held_in(&root.join("store"), &proc);
        assert_eq!(h.len(), 2, "{h:?}");
        assert_eq!((h[0].pid, h[0].name.as_str(), h[0].project.as_str(), h[0].slots), (4100, "olo_kit", "olo", 2));
        assert_eq!((h[1].pid, h[1].name.as_str(), h[1].slots), (4200, "serde_test", 1));
        let _ = std::fs::remove_dir_all(&root);
    }
}
