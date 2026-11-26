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

pub(crate) fn proc_of(pid: i32) -> Option<Proc> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (open, close) = (stat.find('(')?, stat.rfind(')')?);
    let comm = stat[open + 1..close].to_owned();
    let mut rest = stat[close + 2..].split_whitespace();
    let state = rest.next()?.chars().next()?;
    let ppid = rest.next()?.parse().ok()?;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let uid = status.lines().find_map(|l| l.strip_prefix("Uid:"))?.split_whitespace().next()?.parse().ok()?;
    // (The account's own systemd: `systemd --user`.)
    let user_manager = comm == "systemd" && std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| c.split(|&b| b == 0).any(|w| w == b"--user"));
    Some(Proc { pid, ppid, uid, comm, state, user_manager })
}

pub(crate) fn all_procs() -> Vec<Proc> {
    std::fs::read_dir("/proc").map(|d| d.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok()).filter_map(proc_of).collect()).unwrap_or_default()
}

/// Codex's home (`CODEX_HOME`, else `~/.codex`).
pub(crate) fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex"))
}
