//! Which scheduler the computer runs under, as anyone may read it: the
//! kernel's own, or one loaded over it (sched_ext, Linux 6.12 and
//! later). Olo's is `scx_lavd` (sched-ext/scx, GPL-2.0: the
//! Latency-criticality Aware Virtual Deadline scheduler), run by a
//! service of the system's (`olo-scheduler.service`); chosen means that
//! service starts with the computer. The kernel says what is loaded
//! now, whoever loaded it (`/sys/kernel/sched_ext`). Nothing here
//! changes anything: choosing is root's (Olo Doctor's helper:
//! `olo-doctor --scheduler`).
//!
//! `OLO_SCHEDULER_ROOT=DIR` puts all of it under DIR (tests: a made-up
//! kernel in a scratch folder, which Doctor's steps then write to as
//! the person, with nothing of systemd's asked).

use std::path::{Path, PathBuf};

/// What schedules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Linux's own (EEVDF).
    Kernel,
    /// `scx_lavd`, loaded over it.
    Lavd,
}

impl Choice {
    pub const ALL: [Choice; 2] = [Choice::Kernel, Choice::Lavd];

    /// As the command line says it.
    pub fn word(self) -> &'static str {
        match self {
            Choice::Kernel => "kernel",
            Choice::Lavd => "lavd",
        }
    }

    pub fn parse(word: &str) -> Option<Choice> {
        Choice::ALL.into_iter().find(|c| c.word() == word.trim())
    }

    pub fn label(self) -> &'static str {
        match self {
            Choice::Kernel => "The kernel's own",
            Choice::Lavd => "LAVD (recommended)",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Choice::Kernel => "Linux's scheduler as it comes (EEVDF): fair to every program, whatever it's doing.",
            Choice::Lavd => {
                "scx_lavd, made for a computer someone is sitting at: what is being waited for (the window typed in, a frame about to be drawn, sound) runs first, with builds and other long work around it. It is loaded over the kernel's own, which takes over again by itself if LAVD ever stops."
            }
        }
    }
}

/// Where the scheduler's things are: the system's own places, or a
/// test's tree (`OLO_SCHEDULER_ROOT`).
#[derive(Debug, Clone, PartialEq)]
pub struct Places {
    root: PathBuf,
    stand_in: bool,
}

impl Places {
    /// The system's, or `OLO_SCHEDULER_ROOT`'s.
    pub fn here() -> Places {
        match std::env::var_os("OLO_SCHEDULER_ROOT").filter(|r| !r.is_empty()) {
            Some(root) => Places::under(Path::new(&root)),
            None => Places { root: "/".into(), stand_in: false },
        }
    }

    /// A test's tree.
    pub fn under(root: &Path) -> Places {
        Places { root: root.to_owned(), stand_in: true }
    }

    /// A made-up tree: nothing of the real system's is touched or asked.
    pub fn stand_in(&self) -> bool {
        self.stand_in
    }

    fn at(&self, path: &str) -> PathBuf {
        self.root.join(path.trim_start_matches('/'))
    }

    /// The kernel's say on loaded schedulers: there at all where it can
    /// load one.
    pub fn sched_ext(&self) -> PathBuf {
        self.at("/sys/kernel/sched_ext")
    }

    /// LAVD's program, root's copy.
    pub fn program(&self) -> PathBuf {
        self.at("/usr/local/libexec/olo/scx_lavd")
    }

    pub fn unit(&self) -> PathBuf {
        self.at("/etc/systemd/system/olo-scheduler.service")
    }

    /// The unit as enabled: systemd's link to it from what starts with
    /// the computer.
    pub fn wanted(&self) -> PathBuf {
        self.at("/etc/systemd/system/multi-user.target.wants/olo-scheduler.service")
    }
}

/// The scheduler as it is now.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    /// What starts with the computer.
    pub chosen: Choice,
    /// LAVD's program and unit are there (root's).
    pub installed: bool,
    /// The scheduler loaded over the kernel's now, as the kernel names
    /// it ("lavd_1.1.4_x86_64_unknown_linux_gnu"); None: the kernel's
    /// own is scheduling.
    pub loaded: Option<String>,
}

/// What there is; None where the kernel can't load a scheduler (no
/// sched_ext): nothing to choose.
pub fn look(places: &Places) -> Option<State> {
    let ext = places.sched_ext();
    if !ext.is_dir() {
        return None;
    }
    let read = |name: &str| std::fs::read_to_string(ext.join(name)).unwrap_or_default().trim().to_owned();
    let loaded = (read("state") == "enabled").then(|| read("root/ops")).filter(|ops| !ops.is_empty());
    let installed = places.program().is_file() && places.unit().is_file();
    // (A link whose unit is gone starts nothing.)
    let chosen = if installed && std::fs::symlink_metadata(places.wanted()).is_ok() { Choice::Lavd } else { Choice::Kernel };
    Some(State { chosen, installed, loaded })
}

/// "LAVD 1.1.4" of the kernel's name for it
/// ("lavd_1.1.4_x86_64_unknown_linux_gnu"); None: not LAVD.
pub fn lavd_version(ops: &str) -> Option<String> {
    let rest = ops.strip_prefix("lavd")?;
    let version = rest.trim_start_matches('_').split('_').next().filter(|v| v.starts_with(|c: char| c.is_ascii_digit()));
    Some(match version {
        Some(v) => format!("LAVD {v}"),
        None => "LAVD".to_owned(),
    })
}

impl State {
    /// What is scheduling now, in a line.
    pub fn now(&self) -> String {
        match (&self.loaded, self.chosen) {
            (Some(ops), _) if lavd_version(ops).is_some() => format!("Now: {} is scheduling.", lavd_version(ops).unwrap_or_default()),
            (Some(ops), Choice::Kernel) => format!("Now: another scheduler is loaded ({ops}), not by Olo."),
            (Some(ops), Choice::Lavd) => format!("Now: another scheduler is loaded ({ops}) where LAVD would be."),
            (None, Choice::Kernel) => "Now: the kernel's own is scheduling.".to_owned(),
            (None, Choice::Lavd) => "LAVD is chosen, but the kernel's own is scheduling now: LAVD stopped, or the kernel put it out (Logs says why, under olo-scheduler).".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(path: PathBuf, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn what_is_chosen_and_what_is_loaded() {
        let root = PathBuf::from(format!("/var/tmp/olo-system-scheduler-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let p = Places::under(&root);
        // No sched_ext: nothing to choose.
        assert_eq!(look(&p), None);
        put(p.sched_ext().join("state"), "disabled\n");
        let s = look(&p).unwrap();
        assert_eq!((s.chosen, s.installed, s.loaded.clone()), (Choice::Kernel, false, None));
        assert_eq!(s.now(), "Now: the kernel's own is scheduling.");
        // Someone else's, loaded by hand.
        put(p.sched_ext().join("state"), "enabled\n");
        put(p.sched_ext().join("root/ops"), "rusty_1.0.9\n");
        assert_eq!(look(&p).unwrap().now(), "Now: another scheduler is loaded (rusty_1.0.9), not by Olo.");
        // Olo's, installed and enabled, and running.
        put(p.program(), "");
        put(p.unit(), "");
        put(p.wanted(), "");
        put(p.sched_ext().join("root/ops"), "lavd_1.1.4_x86_64_unknown_linux_gnu\n");
        let s = look(&p).unwrap();
        assert_eq!((s.chosen, s.installed), (Choice::Lavd, true));
        assert_eq!(s.now(), "Now: LAVD 1.1.4 is scheduling.");
        // Put out by the kernel.
        put(p.sched_ext().join("state"), "disabled\n");
        assert!(look(&p).unwrap().now().starts_with("LAVD is chosen, but the kernel's own is scheduling now"));
        // Its link without its program chooses nothing.
        std::fs::remove_file(p.program()).unwrap();
        assert_eq!(look(&p).unwrap().chosen, Choice::Kernel);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn names() {
        assert_eq!(lavd_version("lavd_1.1.4_x86_64_unknown_linux_gnu").as_deref(), Some("LAVD 1.1.4"));
        assert_eq!(lavd_version("lavd").as_deref(), Some("LAVD"));
        assert_eq!(lavd_version("rusty_1.0.9"), None);
        assert_eq!(Choice::parse("lavd\n"), Some(Choice::Lavd));
        assert_eq!(Choice::parse("bore"), None);
    }
}
