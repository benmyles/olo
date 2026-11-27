//! What the helper reads of this host's processes and the account's
//! places: for the agents service (whose child a process is, which
//! agent runs where) and for moving an account (what to stop).

use std::path::PathBuf;

pub(crate) fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg_attr(not(feature = "move"), allow(dead_code))]
pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    (s.len().is_multiple_of(2)).then_some(())?;
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// A process of this host: its parent, its name, its state.
#[derive(Clone, Debug)]
pub(crate) struct Proc {
    pub pid: i32,
    pub ppid: i32,
    #[cfg_attr(not(feature = "move"), allow(dead_code))]
    pub(crate) uid: u32,
    pub comm: String,
    #[cfg_attr(not(feature = "move"), allow(dead_code))]
    pub(crate) state: char,
    #[cfg_attr(not(feature = "move"), allow(dead_code))]
    pub(crate) user_manager: bool,
}

pub(crate) fn proc_stat_of(pid: i32) -> Option<Proc> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (open, close) = (stat.find('(')?, stat.rfind(')')?);
    let comm = stat[open + 1..close].to_owned();
    let mut rest = stat[close + 2..].split_whitespace();
    let state = rest.next()?.chars().next()?;
    let ppid = rest.next()?.parse().ok()?;
    Some(Proc { pid, ppid, uid: 0, comm, state, user_manager: false })
}

pub(crate) fn proc_of(pid: i32) -> Option<Proc> {
    let mut p = proc_stat_of(pid)?;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    p.uid = status.lines().find_map(|l| l.strip_prefix("Uid:"))?.split_whitespace().next()?.parse().ok()?;
    // (The account's own systemd: `systemd --user`.)
    p.user_manager = p.comm == "systemd" && std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| c.split(|&b| b == 0).any(|w| w == b"--user"));
    Some(p)
}

pub(crate) fn all_procs() -> Vec<Proc> {
    std::fs::read_dir("/proc").map(|d| d.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok()).filter_map(proc_of).collect()).unwrap_or_default()
}

pub(crate) fn all_procs_stat() -> Vec<Proc> {
    std::fs::read_dir("/proc").map(|d| d.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok()).filter_map(proc_stat_of).collect()).unwrap_or_default()
}

/// Direct children of `pid` from `/proc/{pid}/task/*/children`.
/// Returns `None` if the kernel does not expose `task/*/children` (caller falls back to `all_procs_stat`).
fn direct_children(pid: i32) -> Option<Vec<i32>> {
    use std::collections::HashSet;
    use std::io::ErrorKind;
    let task_dir = format!("/proc/{pid}/task");
    let mut kids = Vec::new();
    let mut seen = HashSet::new();
    // Read twice, always, and what either reading has is kept. The
    // kernel's list is exact only while none of the thread's children
    // ends (proc_pid_task(5), `children`: a child that exits while the
    // file is read can leave one that lives out of it): an agent's
    // short-lived probe ending just then left its long-running shell
    // out, and the agent was "done, no shell" for a moment again (a
    // code review, 2026-11-27). A child left out of one reading by a
    // sibling's end is in the other, unless another sibling ends at
    // that very instant too.
    for _pass in 0..2 {
        let rd = match std::fs::read_dir(&task_dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == ErrorKind::NotFound => return Some(kids),
            Err(_) => return None,
        };
        for entry in rd.flatten() {
            let tid_path = entry.path();
            let children_path = tid_path.join("children");
            match std::fs::read_to_string(&children_path) {
                Ok(text) => {
                    for tok in text.split_whitespace() {
                        if let Ok(cpid) = tok.parse::<i32>()
                            && seen.insert(cpid)
                        {
                            kids.push(cpid);
                        }
                    }
                }
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    // If the thread directory still exists, the kernel lacks CONFIG_PROC_CHILDREN.
                    if tid_path.exists() {
                        return None;
                    }
                    // Thread exited mid-scan; its children were reparented to another thread of `pid`
                    // (found there, in this reading or the next).
                }
                Err(_) => return None,
            }
        }
    }
    Some(kids)
}

/// Reads `root_pid` and all of its descendants via `/proc/{pid}/task/*/children`
/// and `/proc/{pid}/stat`, without scanning unrelated processes on the host.
/// Returns `None` only if `task/*/children` is unavailable on this kernel.
pub(crate) fn subtree_procs(root_pid: i32) -> Option<Vec<Proc>> {
    use std::collections::HashSet;
    if root_pid <= 0 {
        return Some(Vec::new());
    }
    let Some(root) = proc_stat_of(root_pid) else {
        return Some(Vec::new());
    };
    let mut out = vec![root];
    let mut stack = vec![root_pid];
    let mut seen = HashSet::new();
    seen.insert(root_pid);
    while let Some(p) = stack.pop() {
        let kids = direct_children(p)?;
        for cpid in kids {
            if seen.insert(cpid)
                && let Some(proc) = proc_stat_of(cpid)
                && proc.ppid == p
            {
                out.push(proc);
                stack.push(cpid);
            }
        }
    }
    Some(out)
}

/// Codex's home (`CODEX_HOME`, else `~/.codex`).
pub(crate) fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex"))
}
