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
    /// Where programs put their scratch files: on disk, where `/tmp` is
    /// the machine's memory. Ben, 2026-11-26: "add an item in there
    /// that checks if /tmp is in ram and if it is and the various tmp
    /// dir system envs are not set have it suggest setting them and
    /// able to apply it there" ("the temp folder system envs on the
    /// server aren't set today -- because /tmp is ram they should all
    /// be set to be /var/tmp instead").
    TempFiles,
    /// brush (github.com/reubeno/brush, the bash-compatible shell in
    /// Rust that Olo's own desktop signs in to) put on the host and
    /// made the shell new accounts are given. Ben, 2026-11-26: "as
    /// another suggestion under tweaks, add one for installing brush
    /// shell and making it the default for all users with an option to
    /// also change for all existing users" ([`Tuning::everyone`]).
    Shell,
}

impl Tune {
    pub const ALL: [Tune; 7] = [Tune::Sessions, Tune::Files, Tune::Kernel, Tune::Writeback, Tune::Memory, Tune::TempFiles, Tune::Shell];

    /// Its name in commands and in the files' names.
    pub fn id(self) -> &'static str {
        match self {
            Tune::Sessions => "sessions",
            Tune::Files => "files",
            Tune::Kernel => "kernel",
            Tune::Writeback => "writeback",
            Tune::Memory => "memory",
            Tune::TempFiles => "tmpdir",
            Tune::Shell => "shell",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Tune::Sessions => "SSH sessions",
            Tune::Files => "Open files and tasks",
            Tune::Kernel => "Kernel limits",
            Tune::Writeback => "Writing to disk",
            Tune::Memory => "Memory for builds",
            Tune::TempFiles => "Temporary files",
            Tune::Shell => "Brush shell",
        }
    }

    /// Why, in a sentence.
    pub fn why(self) -> &'static str {
        match self {
            Tune::Sessions => "Lets each pane have its own session; SSH’s default is ten per connection.",
            Tune::Files => "Lets builds and agents open many files and start many tasks at once.",
            Tune::Kernel => "Raises the kernel’s ceilings that file watchers and large programs hit.",
            Tune::Writeback => "Lets a build’s many small writes reach the disk together.",
            Tune::Memory => "Keeps a build’s files in memory and running programs out of swap.",
            Tune::TempFiles => "Keeps programs’ scratch files on disk where /tmp is in memory.",
            Tune::Shell => "Installs a fast, bash-compatible shell as new accounts’ sign-in shell.",
        }
    }

    /// What applying it does besides, where that is worth knowing.
    pub fn note(self) -> Option<&'static str> {
        match self {
            Tune::Sessions => Some("The SSH server reads its settings again; open connections stay."),
            Tune::Files => Some("The open files hold from the next sign-in (for the panes you have open, after Disconnect and Connect), and for services from their next start."),
            Tune::Kernel | Tune::Writeback | Tune::Memory => None,
            Tune::TempFiles => Some("Programs are told to use /var/tmp, from their next sign-in or start. Unlike /tmp, /var/tmp isn’t emptied when the machine restarts."),
            Tune::Shell => Some("Brush is downloaded from its project on GitHub, checked against the build Olo knows, and tried before any account is given it."),
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
            Tune::TempFiles => &[
                "/etc/profile.d/90-olo-tmpdir.sh",
                "/etc/profile.d/90-olo-tmpdir.csh",
                "/etc/environment.d/90-olo-tmpdir.conf",
                "/etc/systemd/system.conf.d/90-olo-tmpdir.conf",
                "/etc/ssh/sshd_config.d/00-olo-tmpdir.conf",
                "/etc/sudoers.d/90-olo-tmpdir",
            ],
            Tune::Shell => &[BRUSH, BRUSH_WAS],
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
            // (One thing, yes or no: whether what a sign-in is told to
            // put its scratch files in is on disk. [`TEMP_ON_DISK`].)
            Tune::TempFiles => vec![(TEMP_ON_DISK, 1, false)],
            // (And one: whether brush is there and is what a new
            // account is given. [`BRUSH_DEFAULT`].)
            Tune::Shell => vec![(BRUSH_DEFAULT, 1, false)],
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
        let head = "# Olo's Rex put this here (the Host window, Tweaks). Put it back there, or delete this file.";
        Some(match self {
            Tune::Sessions => format!(
                "set -e\nf=/etc/ssh/sshd_config.d/00-olo.conf\n\
                 grep -qiE '^[[:space:]]*Include[[:space:]].*sshd_config\\.d' /etc/ssh/sshd_config || {{ echo 'Rex: This host’s sshd_config reads no sshd_config.d folder.'; exit 1; }}\n\
                 mkdir -p /etc/ssh/sshd_config.d\n\
                 printf '%s\\n' {} 'MaxSessions {SESSIONS}' > \"$f\"\n\
                 if ! sshd -t; then rm -f \"$f\"; echo 'Rex: The SSH server refused the setting; it was taken out again.'; exit 1; fi\n\
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
                 if command -v su > /dev/null 2>&1 && ! su - root -c true > /dev/null 2>&1; then rm -f /etc/security/limits.d/90-olo.conf; echo 'Rex: A sign-in was refused with the new limits; they were taken out again.'; exit 1; fi\n",
                head = shell_word(head)
            ),
            Tune::TempFiles => {
                let each = |f: &dyn Fn(&str) -> String, between: &str| TEMP_NAMES.iter().map(|n| f(n)).collect::<Vec<_>>().join(between);
                TEMP_APPLY
                    .replace("@HEAD@", &shell_word(head))
                    .replace("@EXPORT@", &shell_word(&format!("export {}", each(&|n| format!("{n}={TEMP_DIR}"), " "))))
                    .replace("@SETENV@", &each(&|n| shell_word(&format!("setenv {n} {TEMP_DIR}")), " "))
                    .replace("@LINES@", &each(&|n| shell_word(&format!("{n}={TEMP_DIR}")), " "))
                    .replace("@DEFAULT@", &shell_word(&format!("DefaultEnvironment={}", each(&|n| format!("{n}={TEMP_DIR}"), " "))))
                    .replace("@SSH@", &shell_word(&format!("SetEnv {}", each(&|n| format!("{n}={TEMP_DIR}"), " "))))
                    .replace("@KEEP@", &shell_word(&format!("Defaults env_keep += \"{}\"", TEMP_NAMES.join(" "))))
                    .replace("@DIR@", TEMP_DIR)
            }
            Tune::Shell => {
                let sums = BRUSH_BUILDS.iter().map(|(arch, sum)| format!("{arch}) sum={sum} ;;")).collect::<Vec<_>>().join(" ");
                SHELL_APPLY
                    .replace("@HEAD@", &shell_word(head))
                    .replace("@SUMS@", &sums)
                    .replace("@VERSION@", BRUSH_VERSION)
                    .replace("@EVERYONE@", if tuning.everyone { "1" } else { "0" })
                    .replace("@ACCOUNTS@", ACCOUNTS)
                    .replace("@WAS@", BRUSH_WAS)
                    .replace("@B@", BRUSH)
            }
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
            Tune::TempFiles => format!(
                "ssh=0; [ -f /etc/ssh/sshd_config.d/00-olo-tmpdir.conf ] && ssh=1\nsys=0; [ -f /etc/systemd/system.conf.d/90-olo-tmpdir.conf ] && sys=1\nrm -f {}\n\
                 [ $ssh = 1 ] && {{ systemctl reload sshd 2> /dev/null || systemctl reload ssh 2> /dev/null || true; }}\n[ $sys = 1 ] && {{ systemctl daemon-reexec 2> /dev/null || true; }}\ntrue\n",
                self.files().join(" ")
            ),
            Tune::Shell => SHELL_UNDO.replace("@WAS@", BRUSH_WAS).replace("@B@", BRUSH),
            Tune::Kernel | Tune::Writeback | Tune::Memory => {
                let file = self.files()[0];
                // (A ratio's bytes put back to nothing gives the say
                // back to the ratio, as the kernel has it.)
                format!("[ -f {file} ] || exit 0\nsed -n 's/^# was: \\(.*\\) = \\([0-9][0-9]*\\)$/\\1=\\2/p' {file} | while read -r was; do sysctl -q -w \"$was\" || true; done\nrm -f {file}\n")
            }
        }
    }
}

/// Where Olo would have programs put their scratch files: on disk on
/// every Linux, there across restarts, writable by all as `/tmp` is.
pub const TEMP_DIR: &str = "/var/tmp";

/// The one setting of [`Tune::TempFiles`], as [`LOOK`] says it: 1 where
/// the folder a sign-in's programs are told to use for scratch files
/// (`TMPDIR`, else `/tmp`) is on disk, 0 where it is the machine's
/// memory.
pub const TEMP_ON_DISK: &str = "temporary files";

/// Every name a program may ask where its scratch files go: POSIX's
/// `TMPDIR`; `TMP`, `TEMP` and `TEMPDIR`, which Python, Node, Ruby and
/// others read too; and Claude Code's own (an agent's commands and
/// their output: most of what agents on a host leave behind).
pub const TEMP_NAMES: [&str; 5] = ["TMPDIR", "TMP", "TEMP", "TEMPDIR", "CLAUDE_CODE_TMPDIR"];

/// [`Tune::TempFiles`] applied: a file of Olo's own for each way a
/// program comes by its environment. A sign-in's shell (`profile.d`,
/// for sh's kind and for csh's); what the account's own systemd starts
/// (`environment.d`: a Rex server kept as a service, and all under it);
/// the machine's services (`system.conf.d`, read at once); a command
/// run over SSH with no sign-in shell (`SetEnv`, which the SSH server
/// is asked to check before it is kept, as its sessions are); and sudo,
/// which otherwise drops them (`env_keep`, checked by `visudo` before
/// it is put where sudo reads it: a file sudo can't read stops sudo).
/// The last two only where the host reads such a folder, and neither
/// keeps the rest from being applied.
const TEMP_APPLY: &str = r#"set -e
[ -d @DIR@ ] && [ -w @DIR@ ] || { echo 'Rex: This host has no @DIR@ to write to.'; exit 1; }
mkdir -p /etc/profile.d /etc/environment.d /etc/systemd/system.conf.d
printf '%s\n' @HEAD@ @EXPORT@ > /etc/profile.d/90-olo-tmpdir.sh
printf '%s\n' @HEAD@ @SETENV@ > /etc/profile.d/90-olo-tmpdir.csh
printf '%s\n' @HEAD@ @LINES@ > /etc/environment.d/90-olo-tmpdir.conf
printf '%s\n' @HEAD@ '[Manager]' @DEFAULT@ > /etc/systemd/system.conf.d/90-olo-tmpdir.conf
systemctl daemon-reexec 2> /dev/null || true
f=/etc/ssh/sshd_config.d/00-olo-tmpdir.conf
if grep -qiE '^[[:space:]]*Include[[:space:]].*sshd_config\.d' /etc/ssh/sshd_config 2> /dev/null; then
  mkdir -p /etc/ssh/sshd_config.d
  printf '%s\n' @HEAD@ @SSH@ > "$f"
  if sshd -t 2> /dev/null; then systemctl reload sshd 2> /dev/null || systemctl reload ssh 2> /dev/null || true; else rm -f "$f"; echo 'Rex: The SSH server took no SetEnv: a command run over SSH with no sign-in keeps /tmp.'; fi
fi
f=/etc/sudoers.d/90-olo-tmpdir
if [ -d /etc/sudoers.d ] && grep -qE '^[#@]includedir[[:space:]]+/etc/sudoers\.d' /etc/sudoers 2> /dev/null && command -v visudo > /dev/null 2>&1; then
  printf '%s\n' @HEAD@ @KEEP@ > "$f.new"
  chmod 0440 "$f.new"
  if visudo -cf "$f.new" > /dev/null 2>&1; then mv -f "$f.new" "$f"; else rm -f "$f.new"; echo 'Rex: sudo took no env_keep: what is run with sudo keeps /tmp.'; fi
fi
"#;

/// Where brush is put on a host: where Olo's own desktop has it.
pub const BRUSH: &str = "/usr/local/bin/brush";

/// What [`Tune::Shell`] found before it changed anything, for the way
/// back: the shell new accounts were given, `adduser`'s own line where
/// the host has one, and each account changed with the shell it had.
const BRUSH_WAS: &str = "/etc/olo-brush.was";

/// The release of brush a host is given, and what each of its builds
/// is (SHA-256 of the project's own archive, as GitHub gave it when
/// this was written): its static builds, which run on any Linux. A
/// download that isn't the build known here is not installed.
pub const BRUSH_VERSION: &str = "0.5.0";
const BRUSH_BUILDS: [(&str, &str); 2] =
    [("x86_64", "541a5d12c7e6a62e341bd7705ac018993b6ba36261e126d77682d6bb052b2103"), ("aarch64", "8bb25aab542eb6fcd6c6885c289fd4ae6f4833c4cc627e32768e71ee4220490b")];

/// The one setting of [`Tune::Shell`], as [`LOOK`] says it: 1 where
/// brush is on the host and is the shell a new account is given.
pub const BRUSH_DEFAULT: &str = "brush";

/// The machine's own accounts that people sign in to with a shell of
/// the usual kind, and not brush yet, a name a line (awk, over
/// `/etc/passwd`, given `min`, `max` and `b`): who [`Tuning::everyone`]
/// changes. Never an account of a directory's (the machine can't
/// change those), a service's, nor one kept to a program that isn't a
/// shell (`git-shell`, `rbash`, `nologin`: to give such an account a
/// shell is to let it in).
const ACCOUNTS: &str = r#"$3 >= min && $3 <= max && $7 != b && $7 ~ /\/(sh|bash|dash|ash|zsh|ksh|mksh|fish|csh|tcsh)$/"#;

/// [`Tune::Shell`] applied (it says what it is at, a line each: the
/// window shows the last line a change wrote while it is under way,
/// and this one takes as long as the download). Brush's own static build for the machine,
/// downloaded to a folder of its own on disk, checked, run once by
/// itself and once as a sign-in's shell, and only then put in place,
/// listed in `/etc/shells` and made what new accounts are given
/// (`useradd -D`, and `adduser`'s own `DSHELL` where the host has
/// that). `@EVERYONE@`: the accounts there are too ([`ACCOUNTS`]),
/// each noted with the shell it had.
const SHELL_APPLY: &str = r#"set -e
b=@B@; was=@WAS@
case "$(uname -m)" in x86_64 | amd64) arch=x86_64 ;; aarch64 | arm64) arch=aarch64 ;; *) echo "Rex: Brush has no build for this machine ($(uname -m))."; exit 1 ;; esac
case $arch in @SUMS@ esac
n=brush-$arch-unknown-linux-musl.tar.gz
d=$(mktemp -d /var/tmp/olo-brush.XXXXXX); trap 'rm -rf "$d"' EXIT
url=https://github.com/reubeno/brush/releases/download/brush-shell-v@VERSION@/$n
echo "Rex> Downloading brush @VERSION@ from GitHub"
if command -v curl > /dev/null 2>&1; then curl -fsSL --retry 2 -o "$d/$n" "$url" || { echo 'Rex: Brush couldn’t be downloaded from GitHub; nothing was installed.'; exit 1; }
elif command -v wget > /dev/null 2>&1; then wget -q -O "$d/$n" "$url" || { echo 'Rex: Brush couldn’t be downloaded from GitHub; nothing was installed.'; exit 1; }
else echo 'Rex: This host has neither curl nor wget to download brush with.'; exit 1; fi
[ "$(sha256sum "$d/$n" | cut -d ' ' -f 1)" = "$sum" ] || { echo 'Rex: What was downloaded isn’t the brush @VERSION@ that Olo knows; nothing was installed.'; exit 1; }
echo "Rex> Checked: it is the build Olo knows. Trying it"
tar -xzf "$d/$n" -C "$d" brush
"$d/brush" -c 'exit 0' || { echo 'Rex: Brush doesn’t run on this host; nothing was installed.'; exit 1; }
install -m 0755 "$d/brush" "$b.new"
mv -f "$b.new" "$b"
if command -v su > /dev/null 2>&1 && ! su -s "$b" - root -c 'exit 0' > /dev/null 2>&1; then rm -f "$b"; echo 'Rex: A sign-in with brush as its shell didn’t work here; it was taken out again.'; exit 1; fi
grep -qxF "$b" /etc/shells 2> /dev/null || echo "$b" >> /etc/shells
if [ ! -f "$was" ]; then
  printf '%s\n' @HEAD@ "default $(useradd -D | sed -n 's/^SHELL=//p')" > "$was"
  [ -f /etc/adduser.conf ] && echo "adduser $(grep -m 1 -E '^#?[[:space:]]*DSHELL=' /etc/adduser.conf)" >> "$was"
fi
echo "Rex> Making it the shell new accounts are given"
useradd -D -s "$b"
if [ -f /etc/adduser.conf ]; then
  if grep -qE '^#?[[:space:]]*DSHELL=' /etc/adduser.conf; then sed -i -E "0,/^#?[[:space:]]*DSHELL=.*/s||DSHELL=$b|" /etc/adduser.conf; else echo "DSHELL=$b" >> /etc/adduser.conf; fi
fi
if [ @EVERYONE@ = 1 ]; then
  min=$(awk '$1 == "UID_MIN" {print $2}' /etc/login.defs 2> /dev/null); max=$(awk '$1 == "UID_MAX" {print $2}' /etc/login.defs 2> /dev/null)
  awk -F: -v min="${min:-1000}" -v max="${max:-60000}" -v b="$b" '@ACCOUNTS@ {print $1, $7}' /etc/passwd | while read -r who had; do
    echo "user $who $had" >> "$was"
    echo "Rex> Changing $who’s shell"
    usermod -s "$b" "$who"
  done
fi
"#;

/// [`Tune::Shell`] taken back, from what it noted: each account it
/// changed given the shell it had (one that has chosen another since
/// keeps its choice), new accounts given what they were, and brush
/// itself taken off, unless an account still signs in to it (one that
/// chose it by itself: to take its shell away is to lock it out).
const SHELL_UNDO: &str = r#"b=@B@; was=@WAS@
[ -f "$was" ] || exit 0
sed -n 's/^user \([^ ]*\) \(.*\)$/\1 \2/p' "$was" | while read -r who had; do
  [ "$(awk -F: -v who="$who" '$1 == who {print $7}' /etc/passwd)" = "$b" ] && usermod -s "$had" "$who"
done
had=$(sed -n 's/^default //p' "$was" | head -n 1)
useradd -D -s "${had:-/bin/sh}"
if [ -f /etc/adduser.conf ] && grep -q '^adduser' "$was"; then
  line=$(sed -n 's/^adduser \{0,1\}//p' "$was" | head -n 1)
  if [ -n "$line" ]; then sed -i "s|^DSHELL=$b\$|$line|" /etc/adduser.conf; else sed -i "\|^DSHELL=$b\$|d" /etc/adduser.conf; fi
fi
rm -f "$was"
if awk -F: -v b="$b" '$7 == b {found = 1} END {exit !found}' /etc/passwd; then
  echo "Rex: Brush stays installed: $(awk -F: -v b="$b" '$7 == b {print $1}' /etc/passwd | tr '\n' ' ')still signs in to it."
else
  sed -i "\|^$b\$|d" /etc/shells
  rm -f "$b"
fi
"#;

/// What begins a line a script here says of its own accord: why it
/// stops ([`SAYS`]: a sentence of Olo's, for the alert's own words),
/// and what it is at ([`DOING`]: shown while it runs). Every other
/// line is the host's tools' (`curl: (6) Could not resolve host`), and
/// is shown as it was said, in a well.
pub const SAYS: &str = "Rex: ";
pub const DOING: &str = "Rex> ";

/// What a script that stopped left as its last lines, parted: Olo's own
/// sentences (in a row, as words), and the host's tools' lines (as they
/// were said). What it was at is left out: it stopped.
pub fn parted(said: &str) -> (String, String) {
    let lines = || said.lines().map(str::trim).filter(|line| !line.is_empty());
    let own: Vec<&str> = lines().filter_map(|line| line.strip_prefix(SAYS)).collect();
    let tools: Vec<&str> = lines().filter(|line| !line.starts_with(SAYS) && !line.starts_with(DOING)).collect();
    (own.join(" "), tools.join("\n"))
}

/// A line a script wrote, as the window shows it while the script runs:
/// less the mark that says whose it is.
pub fn shown_line(line: &str) -> &str {
    let line = line.trim();
    line.strip_prefix(DOING).or_else(|| line.strip_prefix(SAYS)).unwrap_or(line)
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
    if name == TEMP_ON_DISK {
        return if value == 0 { "in memory" } else { "on disk" }.to_owned();
    }
    if name == BRUSH_DEFAULT {
        return if value == 0 { "not the default" } else { "the default" }.to_owned();
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
    /// The shell a new account is given now (`useradd -D`), where the
    /// host said.
    pub default_shell: Option<String>,
    /// Whether brush is on the host (whatever accounts are given).
    pub brush_there: bool,
    /// The machine's own accounts that sign in to another shell than
    /// brush ([`ACCOUNTS`]), each with that shell: who `everyone` would
    /// change.
    pub accounts: Vec<(String, String)>,
    /// The person's choice, in the question [`Tune::Shell`] asks:
    /// those accounts given brush too, not only the new ones.
    pub everyone: bool,
    /// Each of [`TEMP_NAMES`] as a sign-in's programs have it now
    /// (None: not set), in their order.
    pub temp: Vec<(String, Option<String>)>,
    /// What applying [`Tune::Shell`] noted before it changed anything
    /// (`/etc/olo-brush.was`): the shell new accounts were given, and
    /// each account it changed with the shell it had. What Put Back…
    /// gives back.
    pub was_default: Option<String>,
    pub was_accounts: Vec<(String, String)>,
    /// The accounts that sign in to brush now (any account's, a
    /// service's too): while there is one that Put Back… doesn't give
    /// its shell back to, brush stays installed.
    pub brush_users: Vec<String>,
}

impl Tuning {
    /// Whether putting [`Tune::Shell`] back takes brush off the host:
    /// nobody signs in to it but the accounts that get their own shell
    /// back.
    pub fn brush_goes(&self) -> bool {
        self.brush_users.iter().all(|user| self.was_accounts.iter().any(|(name, _)| name == user))
    }
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
        let word = |name: &str| output.lines().find_map(|line| line.strip_prefix(name).and_then(|rest| rest.strip_prefix(' '))).map(str::trim).filter(|w| !w.is_empty()).map(str::to_owned);
        Tuning {
            ram,
            stands,
            ratios: (said("vm.dirty_background_ratio"), said("vm.dirty_ratio")),
            default_shell: word("default shell"),
            brush_there: output.lines().any(|line| line.trim() == "brush there"),
            accounts: output
                .lines()
                .filter_map(|line| line.strip_prefix("account "))
                .filter_map(|rest| rest.trim().split_once(' '))
                .map(|(name, shell)| (name.to_owned(), shell.trim().to_owned()))
                .collect(),
            everyone: false,
            temp: TEMP_NAMES
                .iter()
                // (By its whole name: "TMP" begins "TMPDIR" too.)
                .map(|name| {
                    let said = output.lines().filter_map(|line| line.strip_prefix("temp ")).map(|rest| rest.split_once(' ').unwrap_or((rest, ""))).find(|(said, _)| said.trim() == *name);
                    ((*name).to_owned(), said.map(|(_, value)| value.trim()).filter(|value| !value.is_empty()).map(str::to_owned))
                })
                .collect(),
            was_default: word("was default"),
            was_accounts: output
                .lines()
                .filter_map(|line| line.strip_prefix("was user "))
                .filter_map(|rest| rest.trim().split_once(' '))
                .map(|(name, shell)| (name.to_owned(), shell.trim().to_owned()))
                .collect(),
            brush_users: output.lines().filter_map(|line| line.strip_prefix("brush user ")).map(|name| name.trim().to_owned()).filter(|name| !name.is_empty()).collect(),
        }
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
env -i HOME=/root PATH=/usr/sbin:/usr/bin:/sbin:/bin sh -lc 'for n in TMPDIR TMP TEMP TEMPDIR CLAUDE_CODE_TMPDIR; do eval "v=\${$n:-}"; echo "temp $n $v"; done' 2> /dev/null
t=$(env -i HOME=/root PATH=/usr/sbin:/usr/bin:/sbin:/bin sh -lc 'printf %s "${TMPDIR:-/tmp}"' 2> /dev/null); [ -d "$t" ] || t=/tmp
case "$(stat -f -c %T "$t" 2> /dev/null)" in tmpfs | ramfs) echo "temporary files 0" ;; ?*) echo "temporary files 1" ;; esac
[ -f /etc/profile.d/90-olo-tmpdir.sh ] && echo "kept tmpdir"
b=/usr/local/bin/brush; d=$(useradd -D 2> /dev/null | sed -n 's/^SHELL=//p'); echo "default shell $d"
[ -x "$b" ] && echo "brush there"
if [ -x "$b" ] && [ "$d" = "$b" ]; then echo "brush 1"; else echo "brush 0"; fi
min=$(awk '$1 == "UID_MIN" {print $2}' /etc/login.defs 2> /dev/null); max=$(awk '$1 == "UID_MAX" {print $2}' /etc/login.defs 2> /dev/null)
awk -F: -v min="${min:-1000}" -v max="${max:-60000}" -v b="$b" '$3 >= min && $3 <= max && $7 != b && $7 ~ /\/(sh|bash|dash|ash|zsh|ksh|mksh|fish|csh|tcsh)$/ {print "account " $1 " " $7}' /etc/passwd 2> /dev/null
awk -F: -v b="$b" '$7 == b {print "brush user " $1}' /etc/passwd 2> /dev/null
[ -f /etc/olo-brush.was ] && { echo "kept shell"; sed -n 's/^default /was default /p; s/^user /was user /p' /etc/olo-brush.was; }
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
            vm.dirty_bytes 33554432000\nvm.dirty_expire_centisecs 6000\nvm.swappiness 10\nvm.vfs_cache_pressure 50\ntemporary files 1\ndefault shell /usr/local/bin/brush\nbrush there\nbrush 1\nkept sessions\nkept files\nkept kernel\nkept writeback\nkept memory\nkept tmpdir\nkept shell\n";
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
        assert_eq!((said(TEMP_ON_DISK, 0), said(TEMP_ON_DISK, 1)), ("in memory".to_owned(), "on disk".to_owned()));
    }

    /// No `@NAME@` of a script's own is left in it.
    fn none_left(script: &str) -> bool {
        !script.split('@').skip(1).step_by(2).any(|name| !name.is_empty() && name.chars().all(|c| c.is_ascii_uppercase()))
    }

    /// Temporary files: a host whose `/tmp` is its memory and that tells
    /// its sign-ins nothing is short of it; one whose `/tmp` is on disk,
    /// or that already says where, is not; and what is written says
    /// every name, in each place a program comes by them.
    #[test]
    fn temporary_files_are_moved_to_disk_where_tmp_is_memory() {
        let in_memory = Tuning::read(&format!("{AS_IT_COMES}temporary files 0\n"));
        let stands = in_memory.of(Tune::TempFiles);
        assert_eq!((stands.values[0].now, stands.met(), stands.kept), (Some(0), false, false));
        assert!(in_memory.short().contains(&Tune::TempFiles));
        let script = Tune::TempFiles.apply(&in_memory).unwrap();
        let all = "TMPDIR=/var/tmp TMP=/var/tmp TEMP=/var/tmp TEMPDIR=/var/tmp CLAUDE_CODE_TMPDIR=/var/tmp";
        for must in [
            format!("'export {all}' > /etc/profile.d/90-olo-tmpdir.sh"),
            "'setenv TMPDIR /var/tmp' 'setenv TMP /var/tmp'".to_owned(),
            "'TMPDIR=/var/tmp' 'TMP=/var/tmp' 'TEMP=/var/tmp' 'TEMPDIR=/var/tmp' 'CLAUDE_CODE_TMPDIR=/var/tmp' > /etc/environment.d/90-olo-tmpdir.conf".to_owned(),
            format!("'[Manager]' 'DefaultEnvironment={all}' > /etc/systemd/system.conf.d/90-olo-tmpdir.conf"),
            format!("'SetEnv {all}' > \"$f\""),
            "sshd -t".to_owned(),
            "'Defaults env_keep += \"TMPDIR TMP TEMP TEMPDIR CLAUDE_CODE_TMPDIR\"' > \"$f.new\"".to_owned(),
            "visudo -cf \"$f.new\"".to_owned(),
        ] {
            assert!(script.contains(&must), "{must}\n{script}");
        }
        assert!(none_left(&script), "every name put in: {script}");
        // On disk already (its `/tmp` is a disk's, or it says where): nothing to do.
        let on_disk = Tuning::read(&format!("{AS_IT_COMES}temporary files 1\n"));
        assert!(on_disk.of(Tune::TempFiles).met() && Tune::TempFiles.apply(&on_disk).is_none());
        // A host that didn't say isn't taken to be right.
        assert!(!Tuning::read(AS_IT_COMES).of(Tune::TempFiles).met());
        // Applied for real, to a root of the test's own: the files as a shell and systemd read them.
        let root = std::path::PathBuf::from(format!("/var/tmp/olo-rex-tune-tmp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("etc/ssh")).unwrap();
        std::fs::write(root.join("etc/ssh/sshd_config"), "Port 22\n").unwrap();
        let there = |script: &str| script.replace("/etc/", &format!("{}/etc/", root.display())).replace("systemctl ", "true ");
        let out = std::process::Command::new("sh").args(["-c", &there(&script)]).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let read = |file: &str| std::fs::read_to_string(root.join(file)).unwrap();
        let profile = read("etc/profile.d/90-olo-tmpdir.sh");
        assert!(profile.starts_with("# Olo") && profile.ends_with(&format!("export {all}\n")), "{profile}");
        let sourced = std::process::Command::new("sh")
            .args(["-c", &format!(". {}/etc/profile.d/90-olo-tmpdir.sh; printf '%s %s %s %s %s' \"$TMPDIR\" \"$TMP\" \"$TEMP\" \"$TEMPDIR\" \"$CLAUDE_CODE_TMPDIR\"", root.display())])
            .env_remove("TMPDIR")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&sourced.stdout), "/var/tmp /var/tmp /var/tmp /var/tmp /var/tmp");
        assert_eq!(read("etc/profile.d/90-olo-tmpdir.csh").lines().skip(1).collect::<Vec<_>>(), TEMP_NAMES.map(|n| format!("setenv {n} /var/tmp")));
        assert_eq!(read("etc/environment.d/90-olo-tmpdir.conf").lines().skip(1).collect::<Vec<_>>(), TEMP_NAMES.map(|n| format!("{n}=/var/tmp")));
        assert_eq!(read("etc/systemd/system.conf.d/90-olo-tmpdir.conf").lines().skip(1).collect::<Vec<_>>(), ["[Manager]".to_owned(), format!("DefaultEnvironment={all}")]);
        // (This root's sshd_config reads no sshd_config.d folder and it has no sudoers: nothing is written for either, and nothing fails.)
        assert!(!root.join("etc/ssh/sshd_config.d/00-olo-tmpdir.conf").exists() && !root.join("etc/sudoers.d").exists());
        // Put back: the files gone.
        assert!(std::process::Command::new("sh").args(["-c", &there(&Tune::TempFiles.undo())]).status().unwrap().success());
        for file in Tune::TempFiles.files() {
            assert!(!root.join(file.trim_start_matches('/')).exists(), "{file}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Brush: short where it isn't there or isn't what a new account is
    /// given; the accounts that would change are the machine's own
    /// people with a shell of the usual kind, and only when the person
    /// chose them; the build is pinned, and checked before anything is
    /// put in place.
    #[test]
    fn brush_is_for_new_accounts_and_for_those_there_only_when_chosen() {
        let host = format!("{AS_IT_COMES}default shell /bin/bash\nbrush 0\naccount ana /bin/bash\naccount ben /usr/bin/zsh\n");
        let t = Tuning::read(&host);
        assert_eq!(
            (t.default_shell.as_deref(), t.brush_there, t.accounts.clone(), t.everyone),
            (Some("/bin/bash"), false, vec![("ana".to_owned(), "/bin/bash".to_owned()), ("ben".to_owned(), "/usr/bin/zsh".to_owned())], false)
        );
        assert!(!t.of(Tune::Shell).met() && t.short().contains(&Tune::Shell));
        let script = Tune::Shell.apply(&t).unwrap();
        assert!(none_left(&script), "every name put in: {script}");
        for must in [
            "x86_64) sum=541a5d12c7e6a62e341bd7705ac018993b6ba36261e126d77682d6bb052b2103 ;;",
            "aarch64) sum=8bb25aab542eb6fcd6c6885c289fd4ae6f4833c4cc627e32768e71ee4220490b ;;",
            "https://github.com/reubeno/brush/releases/download/brush-shell-v0.5.0/$n",
            "useradd -D -s \"$b\"",
            "if [ 0 = 1 ]; then",
        ] {
            assert!(script.contains(must), "{must}\n{script}");
        }
        // (Checked, run and tried as a sign-in's shell before it is listed or given to anybody.)
        let at = |what: &str| script.find(what).unwrap_or_else(|| panic!("{what}"));
        assert!(at("sha256sum") < at("tar -xzf") && at("tar -xzf") < at("-c 'exit 0'") && at("su -s") < at(">> /etc/shells") && at(">> /etc/shells") < at("useradd -D -s"));
        // The accounts there are too, when chosen.
        let everyone = Tuning { everyone: true, ..t.clone() };
        assert!(Tune::Shell.apply(&everyone).unwrap().contains("if [ 1 = 1 ]; then"));
        // What the look says besides: each variable as a sign-in has it, what was noted, who signs in to brush.
        let noted = Tuning::read(&format!(
            "{AS_IT_COMES}temp TMPDIR /var/tmp\ntemp TMP \ntemp TEMP\ntemp TEMPDIR /scratch\ntemp CLAUDE_CODE_TMPDIR \nbrush there\nbrush 1\nkept shell\nwas default /bin/bash\nwas user ana /bin/bash\nwas user ben /usr/bin/zsh\nbrush user ana\nbrush user ben\n"
        ));
        assert_eq!(
            noted.temp,
            [("TMPDIR", Some("/var/tmp")), ("TMP", None), ("TEMP", None), ("TEMPDIR", Some("/scratch")), ("CLAUDE_CODE_TMPDIR", None)].map(|(n, v)| (n.to_owned(), v.map(str::to_owned)))
        );
        assert_eq!((noted.was_default.as_deref(), noted.was_accounts.len(), noted.brush_users.clone()), (Some("/bin/bash"), 2, vec!["ana".to_owned(), "ben".to_owned()]));
        assert!(noted.brush_goes(), "only the accounts that get their shell back sign in to it");
        let chosen = Tuning { brush_users: vec!["ana".into(), "carol".into()], ..noted.clone() };
        assert!(!chosen.brush_goes(), "carol chose it herself: it stays");
        // There and the default: met, kept, nothing to apply.
        let done = Tuning::read(&format!("{AS_IT_COMES}default shell /usr/local/bin/brush\nbrush there\nbrush 1\nkept shell\n"));
        assert!(done.of(Tune::Shell).met() && done.of(Tune::Shell).kept && done.brush_there && Tune::Shell.apply(&done).is_none());
        assert_eq!((said(BRUSH_DEFAULT, 0), said(BRUSH_DEFAULT, 1)), ("not the default".to_owned(), "the default".to_owned()));
        // What a script says of its own accord is told from the host's tools' lines.
        let stopped = "Rex> Downloading brush 0.5.0 from GitHub\ncurl: (6) Could not resolve host: github.com\nRex: Brush couldn’t be downloaded from GitHub; nothing was installed.\n";
        assert_eq!(parted(stopped), ("Brush couldn’t be downloaded from GitHub; nothing was installed.".to_owned(), "curl: (6) Could not resolve host: github.com".to_owned()));
        assert_eq!(
            parted("sh: line 21: sysctl: command not found\nsh: line 25: sysctl: command not found"),
            (String::new(), "sh: line 21: sysctl: command not found\nsh: line 25: sysctl: command not found".to_owned())
        );
        assert_eq!(
            (shown_line("Rex> Downloading brush 0.5.0 from GitHub"), shown_line("  Rex: It stopped."), shown_line("curl: (6)")),
            ("Downloading brush 0.5.0 from GitHub", "It stopped.", "curl: (6)")
        );
        assert!(script.contains("echo 'Rex: Brush couldn’t be downloaded from GitHub; nothing was installed.'") && script.contains("echo \"Rex> Downloading brush 0.5.0 from GitHub\""));
        // Who is changed: people's accounts with a usual shell, never a service's, a kept-in one's, nor one that has brush.
        let passwd = "root:x:0:0::/root:/bin/bash\ndaemon:x:2:2::/sbin:/usr/sbin/nologin\nana:x:1000:1000::/home/ana:/bin/bash\nben:x:1001:1001::/home/ben:/usr/bin/zsh\n\
            git:x:1002:1002::/home/git:/usr/bin/git-shell\nkept:x:1003:1003::/home/kept:/bin/rbash\nhas:x:1004:1004::/home/has:/usr/local/bin/brush\nnobody:x:65534:65534::/:/usr/sbin/nologin\n";
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-rex-tune-shell-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("passwd"), passwd).unwrap();
        let awk = std::process::Command::new("awk")
            .args(["-F:", "-v", "min=1000", "-v", "max=60000", "-v", "b=/usr/local/bin/brush", &format!("{ACCOUNTS} {{print $1, $7}}")])
            .arg(dir.join("passwd"))
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&awk.stdout), "ana /bin/bash\nben /usr/bin/zsh\n", "{}", String::from_utf8_lossy(&awk.stderr));
        // (What LOOK asks is the same test.)
        assert!(LOOK.contains(ACCOUNTS), "the look and the change name the same accounts");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The scripts on a real machine as root: each kind applied makes
    /// the host say it has what Olo would set, a new sign-in still
    /// works and has the open files, and put back the host says what it
    /// said before. `OLO_TUNE_EXEC`: the words that run a command as
    /// root there (a container's `podman … exec -i NAME`, or `ssh
    /// root@host`); `OLO_TUNE_KINDS`: the kinds to try (a container
    /// can't write the kernel's settings: "sessions,files" there; its
    /// `/tmp` is memory only if it was started so, `--tmpfs /tmp`; and
    /// "shell" downloads brush, and with `OLO_TUNE_EVERYONE=1` gives it
    /// to the accounts there are, each of which must have it after and
    /// its own shell again once put back).
    #[test]
    #[ignore = "needs OLO_TUNE_EXEC, a machine to change as root"]
    fn the_scripts_do_what_they_say_on_a_real_machine() {
        let Ok(exec) = std::env::var("OLO_TUNE_EXEC") else { return };
        let kinds = std::env::var("OLO_TUNE_KINDS").unwrap_or_else(|_| "sessions,files,kernel,writeback,memory,tmpdir".into());
        // (Over SSH a shell there reads the words again: the script as one quoted word.)
        let over_ssh = exec.split_whitespace().any(|w| w == "ssh");
        let run = |script: &str| {
            let mut words = exec.split_whitespace();
            let out = std::process::Command::new(words.next().unwrap()).args(words).args(["sh", "-c", &if over_ssh { shell_word(script) } else { script.to_owned() }]).output().unwrap();
            (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr))
        };
        let everyone = std::env::var("OLO_TUNE_EVERYONE").ok().as_deref() == Some("1");
        let look = || Tuning { everyone, ..Tuning::read(&run(LOOK).1) };
        let shells =
            || run("cat /etc/passwd").1.lines().filter_map(|l| l.split(':').next().zip(l.rsplit(':').next()).map(|(a, b)| (a.to_owned(), b.to_owned()))).collect::<std::collections::BTreeMap<_, _>>();
        let before = look();
        let shells_before = shells();
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
            if tune == Tune::TempFiles {
                // (What a sign-in's programs are told, and what a file made by one of them is on.)
                let (ok, said) = run("su - root -c 'echo $TMPDIR $TMP $TEMP $TEMPDIR $CLAUDE_CODE_TMPDIR; stat -f -c %T \"$(mktemp)\"'");
                println!("a sign-in after: {}", said.trim().replace('\n', " | "));
                assert!(ok && said.starts_with("/var/tmp /var/tmp /var/tmp /var/tmp /var/tmp\n") && !said.contains("tmpfs"), "{said}");
            }
            if tune == Tune::Shell {
                let now = shells();
                println!("shell applied: new accounts {:?}; {:?}", after.default_shell, before.accounts.iter().map(|(a, _)| (a, now.get(a))).collect::<Vec<_>>());
                assert_eq!(after.default_shell.as_deref(), Some(BRUSH));
                for (account, had) in &before.accounts {
                    assert_eq!(&shells_before[account], had, "{account}: the look said the shell it has");
                    assert_eq!(now.get(account).map(String::as_str), Some(if everyone { BRUSH } else { had.as_str() }), "{account}");
                }
                // (Nobody else's: a service's account, one kept to a program, root.)
                for (account, shell) in &shells_before {
                    assert!(before.accounts.iter().any(|(a, _)| a == account) || now.get(account) == Some(shell), "{account} was {shell}, is {:?}", now.get(account));
                }
                // (And it is a shell a sign-in works with.)
                let (ok, said) = run(&format!("su -s {BRUSH} - root -c 'echo signed in with $0'"));
                assert!(ok && said.contains("signed in"), "{said}");
            }
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
            if tune == Tune::Shell {
                assert_eq!((shells(), back.default_shell.clone(), back.brush_there), (shells_before.clone(), before.default_shell.clone(), false), "every account's shell as it was, and brush gone");
                let (_, listed) = run("grep -c brush /etc/shells; ls /etc/olo-brush.was 2>&1");
                println!("shell put back: {}", listed.trim().replace('\n', " | "));
            }
        }
    }
}
