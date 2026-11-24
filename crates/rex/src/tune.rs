//! A host made ready for heavy work: the limits a Linux machine comes
//! with that long builds and many sessions run into, each a suggestion
//! that can be applied from the Server window, as root, and taken back.
//! Ben, 2026-11-23, when a host's SSH server refused an eleventh
//! session: "why is there a 10 session limit? that should be like 1024
//! or something. the host setup in rex app should handle configuring
//! that. and also in the host setup have suggestions that can be
//! applied: it should also increase / max out the open file limits in
//! linux and any other artificial limits like that, and make it persist
//! across restarts, also Raise dirty page writeback thresholds … and
//! any other settings that optimize it for intensive work like rust
//! builds etc".
//!
//! Each [`Tune`] is a handful of settings of one kind, kept across
//! restarts in files of Olo's own (never an edit to a file of the
//! host's): under `sshd_config.d`, `limits.d`, systemd's `*.conf.d`,
//! `sysctl.d`. The host is asked how each setting stands now
//! ([`LOOK`], [`Tuning`]), so the window shows what it has beside what
//! Olo would set; a setting the host already has higher is left alone,
//! never lowered. What is run is shown before it is run
//! ([`Tune::apply`]), and taken back by removing the file and putting
//! back what it noted was there ([`Tune::undo`]).

use crate::provision::shell_word;

/// The most open files a process is given: the kernel's own ceiling as
/// it comes (`fs.nr_open`).
pub const FILES: u64 = 1_048_576;
/// Sessions on one SSH connection (OpenSSH's default is ten: Rex opens
/// a session for each pane, and a further connection when one is full).
pub const SESSIONS: u64 = 1024;

const GB: u64 = 1 << 30;

/// One kind of limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tune {
    /// The SSH server's sessions on a connection.
    Sessions,
    /// Open files and tasks, for sign-ins and for services.
    Files,
    /// The kernel's own ceilings: file watches, processes, memory maps.
    Kernel,
    /// How much written data waits in memory before it must go to disk.
    Writeback,
    /// Memory kept for what builds read again and again.
    Memory,
}

impl Tune {
    pub const ALL: [Tune; 5] = [Tune::Sessions, Tune::Files, Tune::Kernel, Tune::Writeback, Tune::Memory];

    /// Its name in commands and in the files' names.
    pub fn id(self) -> &'static str {
        match self {
            Tune::Sessions => "sessions",
            Tune::Files => "files",
            Tune::Kernel => "kernel",
            Tune::Writeback => "writeback",
            Tune::Memory => "memory",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Tune::Sessions => "SSH sessions",
            Tune::Files => "Open files and tasks",
            Tune::Kernel => "Kernel limits",
            Tune::Writeback => "Writing to disk",
            Tune::Memory => "Memory for builds",
        }
    }

    /// Why, in a sentence.
    pub fn why(self) -> &'static str {
        match self {
            Tune::Sessions => "SSH allows ten sessions on a connection by default; each pane uses one.",
            Tune::Files => "Builds and agents open many files and start many tasks at once.",
            Tune::Kernel => "File watchers, linkers and large programs run into the kernel’s own ceilings.",
            Tune::Writeback => "Lets a build’s many small writes reach the disk together.",
            Tune::Memory => "Keeps a build’s files in memory and running programs out of swap.",
        }
    }

    /// What applying it does besides, where that is worth knowing.
    pub fn note(self) -> Option<&'static str> {
        match self {
            Tune::Sessions => Some("The SSH server reads its settings again; open connections stay."),
            Tune::Files => Some("The open files hold from the next sign-in (for the panes you have open, after Disconnect and Connect), and for services from their next start."),
            Tune::Kernel | Tune::Writeback | Tune::Memory => None,
        }
    }

    /// What it can cost, where it can.
    pub fn cost(self) -> Option<&'static str> {
        match self {
            Tune::Writeback => Some("If the machine loses power, up to a minute of what was just written can be lost."),
            _ => None,
        }
    }

    /// The files of Olo's that keep it across restarts.
    pub fn files(self) -> &'static [&'static str] {
        match self {
            Tune::Sessions => &["/etc/ssh/sshd_config.d/00-olo.conf"],
            Tune::Files => &["/etc/security/limits.d/90-olo.conf", "/etc/systemd/system.conf.d/90-olo.conf", "/etc/systemd/user.conf.d/90-olo.conf", "/etc/systemd/logind.conf.d/90-olo.conf"],
            Tune::Kernel => &["/etc/sysctl.d/90-olo-kernel.conf"],
            Tune::Writeback => &["/etc/sysctl.d/90-olo-writeback.conf"],
            Tune::Memory => &["/etc/sysctl.d/90-olo-memory.conf"],
        }
    }

    /// The settings it is, each with what Olo would set on a machine of
    /// `ram` bytes: its name as the host has it, the value, and whether
    /// that is a most (lower is as good) and not a least.
    pub fn wanted(self, ram: u64) -> Vec<(&'static str, u64, bool)> {
        match self {
            Tune::Sessions => vec![("MaxSessions", SESSIONS, false)],
            Tune::Files => vec![("open files", FILES, false), ("DefaultLimitNOFILE", FILES, false), ("DefaultTasksMax", u64::MAX, false)],
            Tune::Kernel => vec![
                ("fs.file-max", 2 * FILES, false),
                ("fs.inotify.max_user_watches", FILES, false),
                ("fs.inotify.max_user_instances", 8192, false),
                ("fs.inotify.max_queued_events", 65_536, false),
                ("fs.aio-max-nr", FILES, false),
                ("kernel.pid_max", 4_194_304, false),
                ("vm.max_map_count", FILES, false),
            ],
            // (As Ben gave them, 8 GB and 32 GB and a minute, on a
            // machine with the memory for it: never more than half its
            // memory waiting, and a quarter of that before writing
            // begins in the background.)
            Tune::Writeback => {
                let dirty = (32 * GB).min(ram / 2) >> 20 << 20;
                vec![("vm.dirty_background_bytes", (8 * GB).min(dirty / 4) >> 20 << 20, false), ("vm.dirty_bytes", dirty, false), ("vm.dirty_expire_centisecs", 6000, false)]
            }
            Tune::Memory => vec![("vm.swappiness", 10, true), ("vm.vfs_cache_pressure", 50, true)],
        }
    }

    /// The script that applies it, as root, to a host that stands as
    /// `tuning` says: the files written and what is told the running
    /// system. A setting the host already has as high isn't written.
    /// None where nothing is to do.
    pub fn apply(self, tuning: &Tuning) -> Option<String> {
        let short: Vec<&Value> = tuning.of(self).values.iter().filter(|v| !v.met()).collect();
        if short.is_empty() {
            return None;
        }
        let head = "# Olo's Rex put this here (the host's window, Limits). Remove it there, or delete this file.";
        Some(match self {
            Tune::Sessions => format!(
                "set -e\nf=/etc/ssh/sshd_config.d/00-olo.conf\n\
                 grep -qiE '^[[:space:]]*Include[[:space:]].*sshd_config\\.d' /etc/ssh/sshd_config || {{ echo 'This host’s sshd_config reads no sshd_config.d folder.'; exit 1; }}\n\
                 mkdir -p /etc/ssh/sshd_config.d\n\
                 printf '%s\\n' {} 'MaxSessions {SESSIONS}' > \"$f\"\n\
                 if ! sshd -t; then rm -f \"$f\"; echo 'The SSH server refused the setting; it was taken out again.'; exit 1; fi\n\
                 systemctl reload sshd 2> /dev/null || systemctl reload ssh\n",
                shell_word(head)
            ),
            // (No more than can be set: a limit above the kernel's
            // `fs.nr_open` can't be, nor one above its own where root
            // here may not raise limits, as in a container; and a
            // sign-in that can't set its limits is refused, "su:
            // cannot open session: Permission denied": met in a
            // container, 2026-11-23. So the most that this shell can
            // give itself is what is written, and a sign-in is tried
            // after: if it fails, the file is taken out again.)
            Tune::Files => format!(
                "set -e\nn={FILES}; most=$(sysctl -n fs.nr_open 2> /dev/null || echo $n); [ \"$most\" -lt \"$n\" ] && n=$most\n\
                 (ulimit -Hn \"$n\") 2> /dev/null || n=$(ulimit -Hn)\n\
                 mkdir -p /etc/security/limits.d /etc/systemd/system.conf.d /etc/systemd/user.conf.d /etc/systemd/logind.conf.d\n\
                 printf '%s\\n' {head} \"* soft nofile $n\" \"* hard nofile $n\" \"root soft nofile $n\" \"root hard nofile $n\" > /etc/security/limits.d/90-olo.conf\n\
                 printf '%s\\n' {head} '[Manager]' \"DefaultLimitNOFILE=$n:$n\" 'DefaultTasksMax=infinity' > /etc/systemd/system.conf.d/90-olo.conf\n\
                 printf '%s\\n' {head} '[Manager]' \"DefaultLimitNOFILE=$n:$n\" > /etc/systemd/user.conf.d/90-olo.conf\n\
                 printf '%s\\n' {head} '[Login]' 'UserTasksMax=infinity' > /etc/systemd/logind.conf.d/90-olo.conf\n\
                 systemctl daemon-reexec\n\
                 if command -v su > /dev/null 2>&1 && ! su - root -c true > /dev/null 2>&1; then rm -f /etc/security/limits.d/90-olo.conf; echo 'A sign-in was refused with the new limits; they were taken out again.'; exit 1; fi\n",
                head = shell_word(head)
            ),
            Tune::Kernel | Tune::Writeback | Tune::Memory => {
                let file = self.files()[0];
                // (What each was, noted in the file: what taking it
                // back puts there again.)
                // (How much may wait is said in bytes or as a share of
                // memory, one or the other: where it was a share, the
                // bytes read nothing, which can't be written back
                // (met on a real host: the bytes stayed), and the
                // share is what was.)
                let was = |v: &Value| match (v.name, v.now) {
                    ("vm.dirty_background_bytes", Some(0)) => format!("# was: vm.dirty_background_ratio = {}", tuning.ratios.0.unwrap_or(10)),
                    ("vm.dirty_bytes", Some(0)) => format!("# was: vm.dirty_ratio = {}", tuning.ratios.1.unwrap_or(20)),
                    (name, now) => format!("# was: {name} = {}", now.map_or("?".to_owned(), |n| n.to_string())),
                };
                let lines: Vec<String> = short.iter().flat_map(|v| [was(v), format!("{} = {}", v.name, v.wanted)]).map(|l| shell_word(&l)).collect();
                format!("set -e\nmkdir -p /etc/sysctl.d\nprintf '%s\\n' {} {} > {file}\nsysctl -q -p {file}\n", shell_word(head), lines.join(" "))
            }
        })
    }

    /// The script that takes it back: Olo's files gone, the running
    /// system told what it had (a kernel setting, from what the file
    /// noted) or to read its settings again.
    pub fn undo(self) -> String {
        match self {
            Tune::Sessions => "rm -f /etc/ssh/sshd_config.d/00-olo.conf\nsystemctl reload sshd 2> /dev/null || systemctl reload ssh\n".to_owned(),
            Tune::Files => format!("rm -f {}\nsystemctl daemon-reexec\n", self.files().join(" ")),
            Tune::Kernel | Tune::Writeback | Tune::Memory => {
                let file = self.files()[0];
                // (A ratio's bytes put back to nothing gives the say
                // back to the ratio, as the kernel has it.)
                format!("[ -f {file} ] || exit 0\nsed -n 's/^# was: \\(.*\\) = \\([0-9][0-9]*\\)$/\\1=\\2/p' {file} | while read -r was; do sysctl -q -w \"$was\" || true; done\nrm -f {file}\n")
            }
        }
    }
}

/// One setting, as the host has it and as Olo would set it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Value {
    pub name: &'static str,
    /// What the host says it is now (`u64::MAX`: no limit); None where
    /// it didn't say.
    pub now: Option<u64>,
    pub wanted: u64,
    /// Lower is as good (a most, not a least).
    pub at_most: bool,
}

impl Value {
    /// The host has it as Olo would set it, or better.
    pub fn met(&self) -> bool {
        self.now.is_some_and(|now| if self.at_most { now <= self.wanted } else { now >= self.wanted })
    }
}

/// A number as a person reads it: bytes in GB and MB where the
/// setting is one of bytes, "no limit", else the number with its
/// thousands apart.
pub fn said(name: &str, value: u64) -> String {
    if value == u64::MAX {
        return "no limit".to_owned();
    }
    if name.ends_with("_bytes") {
        // (Nothing: the kernel goes by a share of memory instead.)
        if value == 0 {
            return "not set".to_owned();
        }
        return if value >= GB && value % (GB / 4) == 0 { format!("{} GB", value as f64 / GB as f64) } else { format!("{} MB", value >> 20) };
    }
    let digits = value.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// How one [`Tune`] stands on a host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stands {
    pub tune: Tune,
    pub values: Vec<Value>,
    /// Olo's file for it is there.
    pub kept: bool,
}

impl Stands {
    pub fn met(&self) -> bool {
        self.values.iter().all(Value::met)
    }
}

/// How a host's limits stand ([`LOOK`]'s answer, read).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tuning {
    /// The machine's memory, in bytes.
    pub ram: u64,
    pub stands: Vec<Stands>,
    /// The shares of memory the kernel goes by where the bytes aren't
    /// set (`vm.dirty_background_ratio`, `vm.dirty_ratio`): what
    /// putting the writeback back restores.
    pub ratios: (Option<u64>, Option<u64>),
}

impl Tuning {
    pub fn of(&self, tune: Tune) -> &Stands {
        self.stands.iter().find(|s| s.tune == tune).expect("every kind is in it")
    }

    /// The kinds that could be applied: something of each is short.
    pub fn short(&self) -> Vec<Tune> {
        self.stands.iter().filter(|s| !s.met()).map(|s| s.tune).collect()
    }

    /// [`LOOK`]'s output, read: a line for each setting (its name, a
    /// space, its value), `ram` in kB, and `kept ID` for each kind
    /// whose file of Olo's is there. A setting the host didn't say is
    /// not known, and so not met.
    pub fn read(output: &str) -> Tuning {
        let number = |text: &str| -> Option<u64> {
            let text = text.trim();
            match text {
                "infinity" | "unlimited" | "none" => Some(u64::MAX),
                _ => text.parse().ok(),
            }
        };
        let said = |name: &str| output.lines().find_map(|line| line.strip_prefix(name).and_then(|rest| rest.strip_prefix(' ')).and_then(number));
        let ram = said("ram").unwrap_or(0).saturating_mul(1024);
        let stands = Tune::ALL
            .into_iter()
            .map(|tune| Stands {
                tune,
                values: tune.wanted(ram).into_iter().map(|(name, wanted, at_most)| Value { name, now: said(name), wanted, at_most }).collect(),
                kept: output.lines().any(|line| line.trim() == format!("kept {}", tune.id())),
            })
            .collect();
        Tuning { ram, stands, ratios: (said("vm.dirty_background_ratio"), said("vm.dirty_ratio")) }
    }
}

/// Asks a host, as root, how its limits stand: what [`Tuning::read`]
/// reads. (The open files are those of this very command's shell: what
/// a sign-in over SSH is given.)
pub const LOOK: &str = r#"echo "ram $(awk '/^MemTotal:/ {print $2}' /proc/meminfo)"
echo "MaxSessions $(sshd -T 2> /dev/null | awk 'tolower($1) == "maxsessions" {print $2}')"
echo "open files $(ulimit -Hn)"
echo "DefaultLimitNOFILE $(systemctl show --property DefaultLimitNOFILE --value 2> /dev/null)"
echo "DefaultTasksMax $(systemctl show --property DefaultTasksMax --value 2> /dev/null)"
for k in vm.dirty_background_ratio vm.dirty_ratio fs.file-max fs.inotify.max_user_watches fs.inotify.max_user_instances fs.inotify.max_queued_events fs.aio-max-nr kernel.pid_max vm.max_map_count vm.dirty_background_bytes vm.dirty_bytes vm.dirty_expire_centisecs vm.swappiness vm.vfs_cache_pressure; do
  echo "$k $(sysctl -n $k 2> /dev/null)"
done
[ -f /etc/ssh/sshd_config.d/00-olo.conf ] && echo "kept sessions"
[ -f /etc/security/limits.d/90-olo.conf ] && echo "kept files"
for g in kernel writeback memory; do [ -f /etc/sysctl.d/90-olo-$g.conf ] && echo "kept $g"; done
true
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// A machine as it comes (Fedora's own numbers, 64 GB of memory).
    const AS_IT_COMES: &str = "vm.dirty_background_ratio 10\nvm.dirty_ratio 20\nram 65536000\nMaxSessions 10\nopen files 524288\nDefaultLimitNOFILE 524288\nDefaultTasksMax 76000\nfs.file-max 9223372036854775807\nfs.inotify.max_user_watches 524288\n\
        fs.inotify.max_user_instances 128\nfs.inotify.max_queued_events 16384\nfs.aio-max-nr 65536\nkernel.pid_max 4194304\nvm.max_map_count 1048576\nvm.dirty_background_bytes 0\nvm.dirty_bytes 0\n\
        vm.dirty_expire_centisecs 3000\nvm.swappiness 60\nvm.vfs_cache_pressure 100\n";

    #[test]
    fn a_host_as_it_comes_is_short_of_each_kind_and_says_what_it_has() {
        let t = Tuning::read(AS_IT_COMES);
        assert_eq!(t.ram, 65_536_000 * 1024);
        assert_eq!(t.short(), Tune::ALL.to_vec());
        assert!(t.stands.iter().all(|s| !s.kept));
        let sessions = &t.of(Tune::Sessions).values[0];
        assert_eq!((sessions.name, sessions.now, sessions.wanted, sessions.met()), ("MaxSessions", Some(10), 1024, false));
        // What the host already has as high is met, and not written.
        let kernel = t.of(Tune::Kernel);
        let met: Vec<&str> = kernel.values.iter().filter(|v| v.met()).map(|v| v.name).collect();
        assert_eq!(met, ["fs.file-max", "kernel.pid_max", "vm.max_map_count"]);
        let script = Tune::Kernel.apply(&t).unwrap();
        assert!(script.contains("'fs.inotify.max_user_watches = 1048576'") && script.contains("'# was: fs.inotify.max_user_instances = 128'"), "{script}");
        assert!(!script.contains("fs.file-max") && !script.contains("kernel.pid_max"), "never lowered, never rewritten: {script}");
        // Ben's numbers where the machine has the memory for them.
        let w: Vec<(&str, u64)> = t.of(Tune::Writeback).values.iter().map(|v| (v.name, v.wanted)).collect();
        let dirty = 31 * GB + (256 << 20);
        assert_eq!(w, [("vm.dirty_background_bytes", dirty / 4), ("vm.dirty_bytes", dirty), ("vm.dirty_expire_centisecs", 6000)], "62.5 GB of memory: half of it, and a quarter of that");
        // A most: sixty is short of ten, five would do.
        assert!(!t.of(Tune::Memory).values[0].met());
        assert!(Value { name: "vm.swappiness", now: Some(5), wanted: 10, at_most: true }.met());
    }

    /// Where the machine went by shares of memory, the shares are what
    /// was, and what is put back (bytes of nothing can't be written).
    #[test]
    fn writeback_put_back_gives_the_say_back_to_the_shares() {
        let script = Tune::Writeback.apply(&Tuning::read(AS_IT_COMES)).unwrap();
        assert!(
            script.contains("'# was: vm.dirty_background_ratio = 10'") && script.contains("'# was: vm.dirty_ratio = 20'") && script.contains("'# was: vm.dirty_expire_centisecs = 3000'"),
            "{script}"
        );
        assert!(!script.contains("was: vm.dirty_bytes") && !script.contains("was: vm.dirty_background_bytes"), "{script}");
        // A machine that had bytes set has those noted.
        let had = Tuning::read(&AS_IT_COMES.replace("vm.dirty_bytes 0", "vm.dirty_bytes 1073741824"));
        assert!(Tune::Writeback.apply(&had).unwrap().contains("'# was: vm.dirty_bytes = 1073741824'"));
    }

    #[test]
    fn writeback_is_scaled_to_the_machines_memory() {
        let of = |gb: u64| Tune::Writeback.wanted(gb * GB).into_iter().map(|(_, v, _)| v).collect::<Vec<_>>();
        assert_eq!(of(128), [8 * GB, 32 * GB, 6000], "as Ben gave them");
        assert_eq!(of(64), [8 * GB, 32 * GB, 6000]);
        assert_eq!(of(16), [2 * GB, 8 * GB, 6000], "half the memory at most, a quarter of that in the background");
        assert_eq!(of(2), [GB / 4, GB, 6000]);
    }

    #[test]
    fn a_host_made_ready_has_nothing_to_apply_and_says_what_is_kept() {
        let ready = "ram 65536000\nMaxSessions 1024\nopen files 1048576\nDefaultLimitNOFILE 1048576\nDefaultTasksMax infinity\nfs.file-max 9223372036854775807\nfs.inotify.max_user_watches 1048576\n\
            fs.inotify.max_user_instances 8192\nfs.inotify.max_queued_events 65536\nfs.aio-max-nr 1048576\nkernel.pid_max 4194304\nvm.max_map_count 1048576\nvm.dirty_background_bytes 8589934592\n\
            vm.dirty_bytes 33554432000\nvm.dirty_expire_centisecs 6000\nvm.swappiness 10\nvm.vfs_cache_pressure 50\nkept sessions\nkept files\nkept kernel\nkept writeback\nkept memory\n";
        let t = Tuning::read(ready);
        assert_eq!(t.short(), Vec::<Tune>::new(), "{:?}", t.stands.iter().flat_map(|s| s.values.iter().filter(|v| !v.met())).collect::<Vec<_>>());
        assert!(t.stands.iter().all(|s| s.kept) && Tune::ALL.iter().all(|tune| tune.apply(&t).is_none()));
        // A host that says nothing of a setting isn't taken to have it.
        let silent = Tuning::read("ram 1000\nMaxSessions \n");
        assert_eq!((silent.of(Tune::Sessions).values[0].now, silent.of(Tune::Sessions).met()), (None, false));
    }

    /// What is asked and what is applied are scripts a shell takes
    /// (each parsed by `sh -n`), and a kernel setting taken back is put
    /// to what the file noted it was: tried on a file of the test's own.
    #[test]
    fn the_scripts_are_ones_a_shell_takes_and_undo_reads_what_was() {
        let t = Tuning::read(AS_IT_COMES);
        let parses = |script: &str| std::process::Command::new("sh").args(["-n", "-c", script]).status().unwrap().success();
        assert!(parses(LOOK));
        for tune in Tune::ALL {
            assert!(parses(&tune.apply(&t).unwrap()), "{}", tune.apply(&t).unwrap());
            assert!(parses(&tune.undo()), "{}", tune.undo());
        }
        // The file as `apply` writes it, by its own printf, read back as `undo` reads it.
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-rex-tune-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("90-olo-memory.conf");
        let script = Tune::Memory.apply(&t).unwrap();
        let printf = script.lines().find(|l| l.starts_with("printf")).unwrap().replace("/etc/sysctl.d/90-olo-memory.conf", file.to_str().unwrap());
        assert!(std::process::Command::new("sh").args(["-c", &printf]).status().unwrap().success());
        let written = std::fs::read_to_string(&file).unwrap();
        assert_eq!(written.lines().skip(1).collect::<Vec<_>>(), ["# was: vm.swappiness = 60", "vm.swappiness = 10", "# was: vm.vfs_cache_pressure = 100", "vm.vfs_cache_pressure = 50"]);
        let was = std::process::Command::new("sed").args(["-n", r"s/^# was: \(.*\) = \([0-9][0-9]*\)$/\1=\2/p"]).arg(&file).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&was.stdout), "vm.swappiness=60\nvm.vfs_cache_pressure=100\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn numbers_are_said_as_a_person_reads_them() {
        assert_eq!(said("vm.dirty_bytes", 32 * GB), "32 GB");
        assert_eq!(said("vm.dirty_background_bytes", 8 * GB), "8 GB");
        assert_eq!(said("vm.dirty_bytes", 31 * GB + (256 << 20)), "31.25 GB");
        assert_eq!(said("vm.dirty_bytes", 300 << 20), "300 MB");
        assert_eq!(said("vm.dirty_bytes", 0), "not set");
        assert_eq!(said("MaxSessions", 1024), "1,024");
        assert_eq!(said("open files", 1_048_576), "1,048,576");
        assert_eq!(said("DefaultTasksMax", u64::MAX), "no limit");
        assert_eq!(said("vm.swappiness", 60), "60");
    }

    /// The scripts on a real machine as root: each kind applied makes
    /// the host say it has what Olo would set, a new sign-in still
    /// works and has the open files, and put back the host says what it
    /// said before. `OLO_TUNE_EXEC`: the words that run a command as
    /// root there (a container's `podman … exec -i NAME`, or `ssh
    /// root@host`); `OLO_TUNE_KINDS`: the kinds to try (a container
    /// can't write the kernel's settings: "sessions,files" there).
    #[test]
    #[ignore = "needs OLO_TUNE_EXEC, a machine to change as root"]
    fn the_scripts_do_what_they_say_on_a_real_machine() {
        let Ok(exec) = std::env::var("OLO_TUNE_EXEC") else { return };
        let kinds = std::env::var("OLO_TUNE_KINDS").unwrap_or_else(|_| "sessions,files,kernel,writeback,memory".into());
        // (Over SSH a shell there reads the words again: the script as one quoted word.)
        let over_ssh = exec.split_whitespace().any(|w| w == "ssh");
        let run = |script: &str| {
            let mut words = exec.split_whitespace();
            let out = std::process::Command::new(words.next().unwrap()).args(words).args(["sh", "-c", &if over_ssh { shell_word(script) } else { script.to_owned() }]).output().unwrap();
            (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr))
        };
        let look = || Tuning::read(&run(LOOK).1);
        let before = look();
        println!("as it stands: {:?}", before.stands.iter().map(|s| (s.tune.id(), s.met(), s.kept)).collect::<Vec<_>>());
        for tune in Tune::ALL.into_iter().filter(|t| kinds.split(',').any(|k| k == t.id())) {
            let Some(script) = tune.apply(&before) else {
                println!("{}: the host has it already", tune.id());
                continue;
            };
            let (ok, said) = run(&script);
            assert!(ok, "{}: {said}", tune.id());
            let after = look();
            let stands = after.of(tune);
            println!("{} applied: {:?}", tune.id(), stands.values.iter().map(|v| (v.name, v.now.map(|n| super::said(v.name, n)))).collect::<Vec<_>>());
            // (What a sign-in is given shows in a look over SSH, each
            // command of which is one; a container's exec is none,
            // and `su` below stands for it.)
            // (A container's root may not raise its limits: there the
            // files are written with what it has, and nothing is met.)
            assert!(stands.kept && (stands.met() || (!over_ssh && tune == Tune::Files)), "{}: {:?}", tune.id(), stands);
            // (A sign-in still works, with its limits set: PAM's.)
            let (ok, said) = run("su - root -c 'ulimit -Hn'");
            assert!(ok, "a sign-in after {}: {said}", tune.id());
            if tune == Tune::Files && over_ssh {
                assert_eq!(said.trim().parse::<u64>().ok(), Some(FILES), "a new sign-in's open files");
            }
            let (ok, said) = run(&tune.undo());
            assert!(ok, "{} put back: {said}", tune.id());
            let back = look();
            assert!(!back.of(tune).kept, "{}: its file is gone", tune.id());
            // (A kernel setting is put back at once; the others hold
            // from the next sign-in or start, which a new look is.)
            assert_eq!(back.of(tune).values, before.of(tune).values, "{} put back", tune.id());
        }
    }
}
