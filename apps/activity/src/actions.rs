//! What Activity Monitor can do to processes and the system: send a
//! signal, change a process's priority, the cores it may use, its limits
//! and how readily the kernel kills it when memory runs out; set a kernel
//! tunable or a CPU cluster's frequency governor. Done here when this
//! account may, else by the privileged helper (helper.rs), which runs
//! these same functions as root and only for the tunables Olo lists.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Action {
    Signal { pid: i32, signal: i32 },
    /// Every thread's nice value.
    Nice { pid: i32, nice: i32 },
    Affinity { pid: i32, cores: Vec<usize> },
    /// A limit (`details::resource`'s name); None is unlimited.
    Limit { pid: i32, resource: String, soft: Option<u64>, hard: Option<u64> },
    OomAdjust { pid: i32, value: i32 },
    /// One of `sample::TUNABLES`.
    Sysctl { name: String, value: String },
    /// A cpufreq policy's governor (one it offers).
    Governor { policy: usize, governor: String },
}

/// Why an action failed, and whether full access would do it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Refusal {
    pub message: String,
    pub needs_access: bool,
}

/// The signals the Signal menu offers: (number, name, what it asks).
pub const SIGNALS: &[(i32, &str, &str)] = &[
    (libc::SIGHUP, "SIGHUP", "Hang Up (reload)"),
    (libc::SIGINT, "SIGINT", "Interrupt"),
    (libc::SIGQUIT, "SIGQUIT", "Quit and Dump Core"),
    (libc::SIGTERM, "SIGTERM", "Terminate"),
    (libc::SIGKILL, "SIGKILL", "Kill"),
    (libc::SIGSTOP, "SIGSTOP", "Pause"),
    (libc::SIGCONT, "SIGCONT", "Continue"),
    (libc::SIGUSR1, "SIGUSR1", "User Signal 1"),
    (libc::SIGUSR2, "SIGUSR2", "User Signal 2"),
];

fn os_error(what: &str) -> Refusal {
    let err = std::io::Error::last_os_error();
    let needs_access = matches!(err.raw_os_error(), Some(libc::EPERM) | Some(libc::EACCES));
    let message = match err.raw_os_error() {
        Some(libc::ESRCH) => "The process has quit.".to_owned(),
        Some(libc::EPERM) | Some(libc::EACCES) => format!("{what} needs full access."),
        _ => format!("{what} failed: {err}."),
    };
    Refusal { message, needs_access }
}

/// The threads of `pid` (a priority or affinity is per thread).
fn threads(pid: i32) -> Vec<i32> {
    let mut v: Vec<i32> = std::fs::read_dir(format!("/proc/{pid}/task"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse().ok())
        .collect();
    if v.is_empty() {
        v.push(pid);
    }
    v
}

impl Action {
    /// What it does, for an error ("Changing the priority of 1234").
    pub fn describe(&self) -> String {
        match self {
            Action::Signal { pid, signal } => {
                let name = SIGNALS.iter().find(|(n, ..)| n == signal).map_or("a signal", |(_, n, _)| *n);
                format!("Sending {name} to process {pid}")
            }
            Action::Nice { pid, .. } => format!("Changing the priority of process {pid}"),
            Action::Affinity { pid, .. } => format!("Changing the cores process {pid} may use"),
            Action::Limit { pid, .. } => format!("Changing a limit of process {pid}"),
            Action::OomAdjust { pid, .. } => format!("Changing how process {pid} fares when memory runs out"),
            Action::Sysctl { name, .. } => format!("Setting {name}"),
            Action::Governor { .. } => "Changing the CPU governor".into(),
        }
    }

    pub fn run(&self) -> Result<(), Refusal> {
        let what = self.describe();
        match self {
            Action::Signal { pid, signal } => {
                // SAFETY: kill(2) with a pid and a signal number.
                if unsafe { libc::kill(*pid, *signal) } != 0 {
                    return Err(os_error(&what));
                }
            }
            Action::Nice { pid, nice } => {
                for tid in threads(*pid) {
                    // SAFETY: setpriority on a thread id.
                    if unsafe { libc::setpriority(libc::PRIO_PROCESS, tid as libc::id_t, (*nice).clamp(-20, 19)) } != 0 {
                        return Err(os_error(&what));
                    }
                }
            }
            Action::Affinity { pid, cores } => {
                if cores.is_empty() {
                    return Err(Refusal { message: "A process needs at least one core.".into(), needs_access: false });
                }
                // SAFETY: a zeroed cpu_set_t is empty; CPU_SET writes within it.
                let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
                for &c in cores.iter().filter(|&&c| c < libc::CPU_SETSIZE as usize) {
                    unsafe { libc::CPU_SET(c, &mut set) };
                }
                for tid in threads(*pid) {
                    // SAFETY: the set is ours, its size given.
                    if unsafe { libc::sched_setaffinity(tid, std::mem::size_of::<libc::cpu_set_t>(), &set) } != 0 {
                        return Err(os_error(&what));
                    }
                }
            }
            Action::Limit { pid, resource, soft, hard } => {
                let Some(r) = crate::details::resource(resource) else {
                    return Err(Refusal { message: format!("There is no limit “{resource}”."), needs_access: false });
                };
                let lim = libc::rlimit { rlim_cur: soft.unwrap_or(libc::RLIM_INFINITY), rlim_max: hard.unwrap_or(libc::RLIM_INFINITY) };
                // SAFETY: prlimit with a limit to set and none to read back.
                if unsafe { libc::prlimit(*pid, r, &lim, std::ptr::null_mut()) } != 0 {
                    return Err(os_error(&what));
                }
            }
            Action::OomAdjust { pid, value } => {
                std::fs::write(format!("/proc/{pid}/oom_score_adj"), (*value).clamp(-1000, 1000).to_string()).map_err(|e| refusal(&what, &e))?;
            }
            Action::Sysctl { name, value } => {
                if !crate::sample::TUNABLES.iter().any(|(n, _)| n == name) {
                    return Err(Refusal { message: format!("Activity Monitor doesn't change {name}."), needs_access: false });
                }
                if value.contains(['\n', '\0']) {
                    return Err(Refusal { message: "That isn't a value.".into(), needs_access: false });
                }
                std::fs::write(crate::sample::sysctl_path(name), value.trim()).map_err(|e| refusal(&what, &e))?;
            }
            Action::Governor { policy, governor } => {
                let dir = format!("/sys/devices/system/cpu/cpufreq/policy{policy}");
                let offered = std::fs::read_to_string(format!("{dir}/scaling_available_governors")).unwrap_or_default();
                if !offered.split_whitespace().any(|g| g == governor) {
                    return Err(Refusal { message: format!("The CPU doesn't offer “{governor}”."), needs_access: false });
                }
                std::fs::write(format!("{dir}/scaling_governor"), governor).map_err(|e| refusal(&what, &e))?;
            }
        }
        Ok(())
    }
}

fn refusal(what: &str, e: &std::io::Error) -> Refusal {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => Refusal { message: format!("{what} needs full access."), needs_access: true },
        std::io::ErrorKind::NotFound => Refusal { message: "The process has quit.".into(), needs_access: false },
        _ => Refusal { message: format!("{what} failed: {e}."), needs_access: false },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acts_on_a_child() {
        let mut child = std::process::Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let pid = child.id() as i32;
        Action::Nice { pid, nice: 5 }.run().unwrap();
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let close = stat.rfind(')').unwrap();
        assert_eq!(stat[close + 1..].split_whitespace().nth(16), Some("5"));
        Action::Affinity { pid, cores: vec![0] }.run().unwrap();
        assert_eq!(crate::details::affinity(pid), Some(vec![0]));
        Action::Limit { pid, resource: "nofile".into(), soft: Some(64), hard: Some(128) }.run().unwrap();
        // A lower priority can't be raised back without full access.
        let back = Action::Nice { pid, nice: -5 }.run();
        assert!(back.is_err_and(|r| r.needs_access));
        Action::Signal { pid, signal: libc::SIGKILL }.run().unwrap();
        child.wait().unwrap();
        assert!(Action::Signal { pid, signal: libc::SIGTERM }.run().is_err_and(|r| !r.needs_access));
    }

    #[test]
    fn only_listed_tunables() {
        let r = Action::Sysctl { name: "kernel.core_pattern".into(), value: "|/tmp/x".into() }.run();
        assert!(r.is_err_and(|r| !r.needs_access));
        let r = Action::Governor { policy: 0, governor: "made-up".into() }.run();
        assert!(r.is_err());
    }
}
