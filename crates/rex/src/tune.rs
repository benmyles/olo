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
/// Tabs (windows) a Rex server allows in one session (Rex's own default is 64).
pub const REX_WINDOWS: u64 = 512;
/// Panes a Rex server allows in one tab (Rex's own default is 64).
pub const REX_PANES: u64 = 128;
/// Sessions a Rex server allows at once (Rex's own default is 64).
pub const REX_SESSIONS: u64 = 128;
/// Blocks a Rex server allows across all sessions (Rex's own default is 1024).
pub const REX_BLOCKS: u64 = 4096;

/// Rex's own defaults when the `--max-*` flags aren't passed to `rex server run`.
pub const REX_DEFAULT_WINDOWS: u64 = 64;
pub const REX_DEFAULT_PANES: u64 = 64;
pub const REX_DEFAULT_SESSIONS: u64 = 64;
pub const REX_DEFAULT_BLOCKS: u64 = 1024;

/// Olo's drop-in for the shared Rex server's system unit. Personal
/// servers use the connected account's own user config directory
/// (`${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/rex.service.d/90-olo-limits.conf`),
/// never `/etc/systemd/user`.
pub const REX_SYSTEM_DROPIN: &str = "/etc/systemd/system/rex.service.d/90-olo-limits.conf";
pub const REX_USER_DROPIN_FALLBACK: &str = "~/.config/systemd/user/rex.service.d/90-olo-limits.conf";

/// First line of every file Olo writes on a host: identifies Olo's
/// ownership before overwriting or removing a Rex service drop-in.
pub const OLO_HEAD: &str = "# Olo's Rex put this here (the Host window, Tweaks). Put it back there, or delete this file.";

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
    /// The Rex server's own ceilings on tabs, panes, sessions and
    /// blocks (`rex server run --max-*`). Ben, 2026-11-27, after a
    /// lead's session hit Rex's default 64-tab cap: "increase the tab
    /// limit ... the default should increase too (at least 2x what they
    /// are now)".
    RexLimits,
    /// brush (github.com/reubeno/brush, the bash-compatible shell in
    /// Rust that Olo's own desktop signs in to) put on the host and
    /// made the shell new accounts are given. Ben, 2026-11-26: "as
    /// another suggestion under tweaks, add one for installing brush
    /// shell and making it the default for all users with an option to
    /// also change for all existing users" ([`Tuning::everyone`]).
    Shell,
}

impl Tune {
    pub const ALL: [Tune; 8] = [Tune::Sessions, Tune::Files, Tune::Kernel, Tune::Writeback, Tune::Memory, Tune::TempFiles, Tune::RexLimits, Tune::Shell];

    /// Its name in commands and in the files' names.
    pub fn id(self) -> &'static str {
        match self {
            Tune::Sessions => "sessions",
            Tune::Files => "files",
            Tune::Kernel => "kernel",
            Tune::Writeback => "writeback",
            Tune::Memory => "memory",
            Tune::TempFiles => "tmpdir",
            Tune::RexLimits => "rex",
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
            Tune::RexLimits => "Rex server limits",
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
            Tune::RexLimits => "Lets Rex hold more tabs, panes and sessions; Rex’s default is 64 tabs.",
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
            Tune::RexLimits => Some(
                "This changes the Rex server everyone on this host shares. The new limits take effect when it next starts. Restarting it ends the programs in everyone’s tabs, so Rex leaves that to you: restart it from the Server window when you’re ready. You can put the old limits back with Put Back… in Tweaks.",
            ),
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
            Tune::RexLimits => &[REX_SYSTEM_DROPIN],
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
            // (Rex's own flags on `rex server run`: tabs in a session,
            // panes in a tab, sessions, blocks across all sessions.)
            Tune::RexLimits => vec![("--max-windows", REX_WINDOWS, false), ("--max-panes", REX_PANES, false), ("--max-sessions", REX_SESSIONS, false), ("--max-blocks", REX_BLOCKS, false)],
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
        if self == Tune::RexLimits && tuning.rex_limits.blocked.is_some() {
            return None;
        }
        let short: Vec<&Value> = tuning.of(self).values.iter().filter(|v| !v.met()).collect();
        let stale = self == Tune::RexLimits && tuning.rex_limits.stale;
        if short.is_empty() && !stale {
            return None;
        }
        let head = OLO_HEAD;
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
            Tune::RexLimits => REX_APPLY
                .replace("@HEAD@", &shell_word(head))
                .replace(
                    "@MODE@",
                    match tuning.rex_limits.scope {
                        RexScope::Shared => "shared",
                        RexScope::User => "user",
                    },
                )
                .replace("@USER@", &shell_word(tuning.rex_limits.user.as_deref().unwrap_or("")))
                .replace("@WINDOWS@", &REX_WINDOWS.to_string())
                .replace("@PANES@", &REX_PANES.to_string())
                .replace("@SESSIONS@", &REX_SESSIONS.to_string())
                .replace("@BLOCKS@", &REX_BLOCKS.to_string())
                .replace("@SYS_DROPIN@", REX_SYSTEM_DROPIN),
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
            Tune::RexLimits => REX_UNDO.replace("@HEAD@", &shell_word(OLO_HEAD)).replace("@SYS_DROPIN@", REX_SYSTEM_DROPIN),
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

/// Shared inspection script embedded verbatim into both [`LOOK`] and
/// [`REX_APPLY`] via `concat!`, guaranteeing that reading and applying
/// always inspect `rex.service` by the exact same rules and format.
///
/// Arguments: `$1` = `system` or `user`, `$2` = `OLO_HEAD`, `$3` = system
/// drop-in path. For `user` scope this runs **as the connected account**
/// (never as root) and resolves Olo's file under
/// `${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/rex.service.d/90-olo-limits.conf`.
macro_rules! rex_inspect_script {
    () => {
        r#"scope=$1; olo_head=$2; sys_dropin=$3
rex_tm() {
  if command -v timeout >/dev/null 2>&1; then timeout "$@"; else shift; "$@"; fi
}
emit() {
  printf '%s=%s\n' "$1" "$(printf '%s' "$2" | tr '\r\n' '  ' | cut -c1-12000)"
}
refuse() {
  emit status blocked
  emit why "$(printf '%s' "$1" | tr '\r\n' '  ' | cut -c1-300)"
  exit 0
}
check_regular_file() {
  _f=$1
  [ -f "$_f" ] && [ -r "$_f" ] || return 1
  _sz=$(head -c 262145 "$_f" 2>/dev/null | wc -c)
  [ "$_sz" -le 262144 ] 2>/dev/null
}
is_olo() {
  [ -f "$1" ] && [ ! -L "$1" ] || return 1
  IFS= read -r _h < "$1" 2>/dev/null || return 1
  [ "$_h" = "$olo_head" ]
}
if [ "$scope" = "user" ]; then
  _cfg="${XDG_CONFIG_HOME:-${HOME:-}/.config}"
  case "$_cfg" in
    /*) ;;
    *) emit kept 0; emit running unknown; refuse "Could not determine your systemd user config directory." ;;
  esac
  _d1="$_cfg/systemd"
  _d2="$_d1/user"
  _dir="$_d2/rex.service.d"
  olo_file="$_dir/90-olo-limits.conf"
else
  _d1=""; _d2=""
  _dir=${sys_dropin%/*}
  olo_file="$sys_dropin"
fi
emit file "$olo_file"
_bad_link=""; _bad_type=""
for _p in "$_d1" "$_d2" "$_dir"; do
  if [ -n "$_p" ]; then
    if [ -L "$_p" ]; then _bad_link="$_p"; break; fi
    if [ -e "$_p" ] && [ ! -d "$_p" ]; then _bad_type="$_p"; break; fi
  fi
done
has_olo=0
if [ -z "$_bad_link" ] && [ -z "$_bad_type" ] && { [ -e "$olo_file" ] || [ -L "$olo_file" ]; }; then
  if is_olo "$olo_file"; then has_olo=1; fi
fi
emit kept "$has_olo"
if ! command -v awk >/dev/null 2>&1; then
  emit running unknown
  refuse "awk isn’t installed on this host."
fi
if [ "$scope" = "user" ]; then
  show_out=$(rex_tm 10 systemctl --user show -p LoadState -p NeedDaemonReload -p User -p FragmentPath -p DropInPaths -p ExecStart -p ExecStartEx -p MainPID rex.service 2>/dev/null) || { emit running unknown; refuse "systemd couldn’t report how rex.service is configured."; }
else
  show_out=$(rex_tm 10 systemctl show -p LoadState -p NeedDaemonReload -p User -p FragmentPath -p DropInPaths -p ExecStart -p ExecStartEx -p MainPID rex.service 2>/dev/null) || { emit running unknown; refuse "systemd couldn’t report how rex.service is configured."; }
fi
get_prop() {
  printf '%s\n' "$show_out" | awk -v k="$1=" 'index($0, k) == 1 { print substr($0, length(k) + 1); exit }'
}
load_state=$(get_prop LoadState)
need_reload=$(get_prop NeedDaemonReload)
unit_user=$(get_prop User)
frag=$(get_prop FragmentPath)
dropins=$(get_prop DropInPaths)
main_pid=$(get_prop MainPID)
running_val="none"
case "$main_pid" in
  ''|*[!0-9]*|0) running_val="none" ;;
  *)
    if [ -r "/proc/$main_pid/cmdline" ]; then
      _raw_sz=$(head -c 8193 "/proc/$main_pid/cmdline" 2>/dev/null | wc -c)
      if [ "$_raw_sz" -gt 0 ] && [ "$_raw_sz" -le 8192 ]; then
        _b64=$(head -c 8192 "/proc/$main_pid/cmdline" 2>/dev/null | base64 | tr -d '\n')
        [ -n "$_b64" ] && running_val="b64:$_b64" || running_val="unknown"
      else
        running_val="unknown"
      fi
    else
      running_val="unknown"
    fi
    ;;
esac
emit running "$running_val"
case "$load_state" in
  loaded) ;;
  masked) refuse "rex.service is masked on this host." ;;
  not-found) refuse "rex.service isn’t installed on this host." ;;
  *) refuse "rex.service isn’t loaded cleanly (LoadState=${load_state:-unknown})." ;;
esac
if [ "$need_reload" != "no" ]; then
  if [ "$scope" = "user" ]; then
    refuse "systemd has changes to rex.service it hasn’t loaded yet: run systemctl --user daemon-reload and look again."
  else
    refuse "systemd has changes to rex.service it hasn’t loaded yet: run systemctl daemon-reload and look again."
  fi
fi
[ -z "$_bad_link" ] || refuse "$_bad_link is a symbolic link; refusing to write there."
[ -z "$_bad_type" ] || refuse "$_bad_type isn’t a directory."
if [ -e "$olo_file" ] || [ -L "$olo_file" ]; then
  [ "$has_olo" = "1" ] || refuse "$olo_file already exists and wasn’t written by Olo."
fi
[ -n "$frag" ] || refuse "rex.service has no unit file."
if [ "$scope" = "user" ]; then
  case "$frag" in
    /usr/lib/systemd/user/*|/lib/systemd/user/*|/usr/local/lib/systemd/user/*|/etc/systemd/user/*) ;;
    /run/systemd/*) refuse "$frag is generated at runtime: raise its limits where it’s defined." ;;
    *) refuse "$frag is a personal unit file: raise its limits there." ;;
  esac
else
  case "$frag" in
    /usr/lib/systemd/system/*|/lib/systemd/system/*|/usr/local/lib/systemd/system/*|/etc/systemd/system/*) ;;
    /run/systemd/*) refuse "$frag is generated at runtime: raise its limits where it’s defined." ;;
    *) refuse "$frag isn’t a standard system unit file: raise its limits there." ;;
  esac
fi
check_regular_file "$frag" || refuse "$frag couldn’t be read safely."
olo_in_dropins=0
set -f
for d in $dropins; do
  if [ "$d" = "$olo_file" ] && [ "$has_olo" = "1" ]; then
    olo_in_dropins=1
    continue
  fi
  check_regular_file "$d" || { set +f; refuse "A drop-in for rex.service ($d) couldn’t be read safely."; }
  if awk '
    BEGIN { sec = ""; in_cont = 0; found = 0 }
    {
      sub(/\r$/, "")
      t = $0; sub(/^[ \t]+/, "", t)
      if (t ~ /^ExecStart[ \t]*=/ && (sec == "[Service]" || sec == "")) { found = 1; exit 0 }
      if (in_cont) {
        if (t == "" || t ~ /^[#;]/) next
        if ($0 ~ /\\$/) in_cont = 1; else in_cont = 0
        next
      }
      if (t == "" || t ~ /^[#;]/) next
      if (t ~ /^\[/) { sec = t; sub(/[ \t]+$/, "", sec); next }
      if (sec == "[Service]" && t ~ /^ExecStart[ \t]*=/) { found = 1; exit 0 }
      if ($0 ~ /\\$/) in_cont = 1
    }
    END { exit (found ? 0 : 1) }
  ' "$d"; then
    set +f
    refuse "$d sets this server’s command: raise its limits there."
  fi
done
set +f
unit_res=$(awk '
  BEGIN {
    sec = ""; in_cont = 0; count = 0; bad = ""
    vf["--listen"]=1; vf["--extra-listen"]=1; vf["--socket-mode"]=1
    vf["--tailscaled-socket"]=1; vf["--websocket-origin-pattern"]=1
    vf["--block-dir"]=1; vf["--state-dir"]=1; vf["--config"]=1
    vf["--tailscale-hostname"]=1; vf["--tailscale-control-url"]=1
    vf["--tailscale-auth-key-file"]=1; vf["--tailscale-advertise-tag"]=1
    vf["--log-format"]=1; vf["--log-level"]=1
    vf["-S"]=1; vf["--server"]=1; vf["-C"]=1; vf["--client"]=1
    vf["--timeout"]=1; vf["--color"]=1
  }
  function fail(m) { if (bad == "") bad = m }
  {
    sub(/\r$/, "")
    t = $0; sub(/^[ \t]+/, "", t)
    if (in_cont) {
      if (t == "" || t ~ /^[#;]/) next
      if (sec == "[Service]" && t ~ /^ExecStart[ \t]*=/) fail("splits ExecStart across continuation lines")
      if (t ~ /^\[/) fail("splits a section header across continuation lines")
      if ($0 ~ /\\$/) in_cont = 1; else in_cont = 0
      next
    }
    if (t == "" || t ~ /^[#;]/) next
    if (t ~ /^\[/) {
      sec = t; sub(/[ \t]+$/, "", sec)
      if ($0 ~ /\\$/) in_cont = 1
      next
    }
    if (sec == "[Service]" && t ~ /^ExecStart[ \t]*=/) {
      count++
      if ($0 ~ /\\$/) { fail("splits ExecStart across continuation lines"); in_cont = 1; next }
      v = t; sub(/^ExecStart[ \t]*=[ \t]*/, "", v); sub(/[ \t]+$/, "", v)
      raw = v
      next
    }
    if ($0 ~ /\\$/) in_cont = 1
  }
  END {
    if (bad != "") { print "ERR|" bad; exit 0 }
    if (count == 0) { print "ERR|has no ExecStart command"; exit 0 }
    if (count>1) { print "ERR|has more than one ExecStart command"; exit 0 }
    if (raw == "") { print "ERR|has an empty ExecStart command"; exit 0 }
    if (index(raw, "\"")>0 || index(raw, "\047")>0) { print "ERR|uses quotes in ExecStart"; exit 0 }
    if (index(raw, "\\")>0) { print "ERR|uses backslashes in ExecStart"; exit 0 }
    if (index(raw, "%")>0) { print "ERR|uses systemd specifiers (%) in ExecStart"; exit 0 }
    if (index(raw, "$")>0) { print "ERR|uses environment variables ($) in ExecStart"; exit 0 }
    if (index(raw, ";")>0) { print "ERR|uses a command separator (;) in ExecStart"; exit 0 }
    if (substr(raw, 1, 1) != "/") { print "ERR|uses an ExecStart prefix or relative path"; exit 0 }
    n = split(raw, w, /[ \t]+/)
    if (n < 3 || w[1] !~ /^\/([A-Za-z0-9_.,@:\/=-]*\/)?rex$/ || index(w[1], "/../")>0 || w[1] ~ /\/\.\.$/ || w[2] != "server" || w[3] != "run") {
      print "ERR|doesn’t run rex server run directly"; exit 0
    }
    out = w[1] " " w[2] " " w[3]; skip = 0
    for (i = 4; i <= n; i++) {
      if (w[i] == "--" || w[i] !~ /^[A-Za-z0-9_.,@:\/=-]+$/) { print "ERR|has unsupported syntax in ExecStart"; exit 0 }
      out = out " " w[i]
      if (skip) { skip = 0; continue }
      if (w[i] in vf) { skip = 1; continue }
      if (w[i] == "--max-windows" || w[i] == "--max-panes" || w[i] == "--max-sessions" || w[i] == "--max-blocks") {
        if (i == n || w[i + 1] !~ /^[0-9]+$/) { print "ERR|has a non-numeric Rex limit flag"; exit 0 }
        skip = 1
      } else if (w[i] ~ /^--max-(windows|panes|sessions|blocks)=/) {
        val = w[i]; sub(/^--max-(windows|panes|sessions|blocks)=/, "", val)
        if (val !~ /^[0-9]+$/) { print "ERR|has a non-numeric Rex limit flag"; exit 0 }
      }
    }
    print "OK|" out "|" w[1]
  }
' "$frag")
case "$unit_res" in
  OK\|*)
    _rest=${unit_res#OK|}
    unit_cmd=${_rest%|*}
    bin=${_rest#*|}
    ;;
  *) refuse "$frag ${unit_res#ERR|}." ;;
esac
loaded_res=$(printf '%s\n' "$show_out" | awk '
  BEGIN { es_n = 0; ex_n = 0; bad = "" }
  /^ExecStart=/ {
    es_n++
    line = substr($0, 11)
    if (line !~ /^\{ path=[^ ;]+ ; argv\[\]=.* ; ignore_errors=no ; start_time=.* \}$/) { bad = "unexpected ExecStart format or prefix"; next }
    p = line; sub(/^\{ path=/, "", p); sub(/ ; argv\[\]=.*/, "", p)
    a = line; sub(/^\{ path=[^ ;]+ ; argv\[\]=/, "", a); sub(/ ; ignore_errors=no ; start_time=.* \}$/, "", a)
    es_p = p; es_a = a
  }
  /^ExecStartEx=/ {
    ex_n++
    line = substr($0, 13)
    if (line !~ /^\{ path=[^ ;]+ ; argv\[\]=.* ; flags= ; start_time=.* \}$/) { bad = "ExecStart has prefix flags"; next }
    p = line; sub(/^\{ path=/, "", p); sub(/ ; argv\[\]=.*/, "", p)
    a = line; sub(/^\{ path=[^ ;]+ ; argv\[\]=/, "", a); sub(/ ; flags= ; start_time=.* \}$/, "", a)
    ex_p = p; ex_a = a
  }
  END {
    if (bad != "") { print "ERR|" bad; exit 0 }
    if (es_n == 0) { print "ERR|systemd loaded no ExecStart command"; exit 0 }
    if (es_n>1) { print "ERR|systemd loaded more than one ExecStart command"; exit 0 }
    n = split(es_a, w, / /)
    if (n < 3 || w[1] != es_p) { print "ERR|loaded argv[0] differs from executable path"; exit 0 }
    if (ex_n>0 && (ex_n != 1 || ex_p != es_p || ex_a != es_a)) { print "ERR|loaded ExecStartEx disagrees with ExecStart"; exit 0 }
    print "OK|" es_a
  }
')
case "$loaded_res" in
  OK\|*) loaded_cmd=${loaded_res#OK|} ;;
  *) refuse "${loaded_res#ERR|}." ;;
esac
stale=0; olo_pkg=""
if [ "$has_olo" = "1" ]; then
  [ "$olo_in_dropins" = "1" ] || refuse "$olo_file isn’t loaded by systemd."
  check_regular_file "$olo_file" || refuse "$olo_file couldn’t be read safely."
  olo_res=$(awk -v head="$olo_head" '
    BEGIN {
      sec = ""; pkg_n = 0; reset_n = 0; exec_n = 0; bad = ""
      vf["--listen"]=1; vf["--extra-listen"]=1; vf["--socket-mode"]=1
      vf["--tailscaled-socket"]=1; vf["--websocket-origin-pattern"]=1
      vf["--block-dir"]=1; vf["--state-dir"]=1; vf["--config"]=1
      vf["--tailscale-hostname"]=1; vf["--tailscale-control-url"]=1
      vf["--tailscale-auth-key-file"]=1; vf["--tailscale-advertise-tag"]=1
      vf["--log-format"]=1; vf["--log-level"]=1
      vf["-S"]=1; vf["--server"]=1; vf["-C"]=1; vf["--client"]=1
      vf["--timeout"]=1; vf["--color"]=1
    }
    function fail(m) { if (bad == "") bad = m }
    function valid_cmd(raw,   n, w, i, skip, val, out) {
      if (raw == "" || index(raw, "\"")>0 || index(raw, "\047")>0 || index(raw, "\\")>0 || index(raw, "%")>0 || index(raw, "$")>0 || index(raw, ";")>0) return ""
      if (substr(raw, 1, 1) != "/") return ""
      n = split(raw, w, /[ \t]+/)
      if (n < 3 || w[1] !~ /^\/([A-Za-z0-9_.,@:\/=-]*\/)?rex$/ || index(w[1], "/../")>0 || w[1] ~ /\/\.\.$/ || w[2] != "server" || w[3] != "run") return ""
      out = w[1] " " w[2] " " w[3]; skip = 0
      for (i = 4; i <= n; i++) {
        if (w[i] == "--" || w[i] !~ /^[A-Za-z0-9_.,@:\/=-]+$/) return ""
        out = out " " w[i]
        if (skip) { skip = 0; continue }
        if (w[i] in vf) { skip = 1; continue }
        if (w[i] == "--max-windows" || w[i] == "--max-panes" || w[i] == "--max-sessions" || w[i] == "--max-blocks") {
          if (i == n || w[i + 1] !~ /^[0-9]+$/) return ""
          skip = 1
        } else if (w[i] ~ /^--max-(windows|panes|sessions|blocks)=/) {
          val = w[i]; sub(/^--max-(windows|panes|sessions|blocks)=/, "", val)
          if (val !~ /^[0-9]+$/) return ""
        }
      }
      return out
    }
    function strip_limits(cmd,   n, w, i, skip, out) {
      n = split(cmd, w, / /)
      out = ""; skip = 0
      for (i = 1; i <= n; i++) {
        if (skip) { out = (out == "" ? w[i] : out " " w[i]); skip = 0; continue }
        if (i>3 && (w[i] in vf)) { out = (out == "" ? w[i] : out " " w[i]); skip = 1; continue }
        if (i>3 && (w[i] == "--max-windows" || w[i] == "--max-panes" || w[i] == "--max-sessions" || w[i] == "--max-blocks")) { i++; continue }
        if (i>3 && w[i] ~ /^--max-(windows|panes|sessions|blocks)=/) continue
        out = (out == "" ? w[i] : out " " w[i])
      }
      return out
    }
    {
      sub(/\r$/, "")
      if (NR == 1) { if ($0 != head) fail("header mismatch"); next }
      if ($0 ~ /\\$/) { fail("uses continuation lines"); next }
      t = $0; sub(/^[ \t]+/, "", t); sub(/[ \t]+$/, "", t)
      if (t == "") next
      if (t ~ /^# packaged: /) { pkg_n++; pkg_raw = substr(t, 13); next }
      if (t ~ /^[#;]/) next
      if (t ~ /^\[/) { sec = t; next }
      if (sec != "[Service]") { fail("has directives outside [Service]"); next }
      if (t ~ /^ExecStart[ \t]*=[ \t]*$/) { reset_n++; next }
      if (t ~ /^ExecStart[ \t]*=/) { exec_n++; v = t; sub(/^ExecStart[ \t]*=[ \t]*/, "", v); exec_raw = v; next }
      fail("has extra directives")
    }
    END {
      if (bad != "") { print "ERR|" bad; exit 0 }
      if (pkg_n != 1 || reset_n != 1 || exec_n != 1) { print "ERR|has an unexpected structure"; exit 0 }
      pkg_c = valid_cmd(pkg_raw); exec_c = valid_cmd(exec_raw)
      if (pkg_c == "" || exec_c == "") { print "ERR|has an unsupported command"; exit 0 }
      if (strip_limits(pkg_c) != strip_limits(exec_c)) { print "ERR|was modified outside Olo: adjust its limits there"; exit 0 }
      print "OK|" pkg_c "|" exec_c
    }
  ' "$olo_file")
  case "$olo_res" in
    OK\|*)
      _orest=${olo_res#OK|}
      olo_pkg=${_orest%|*}
      olo_cmd=${_orest#*|}
      ;;
    *) refuse "$olo_file ${olo_res#ERR|}." ;;
  esac
  [ "$loaded_cmd" = "$olo_cmd" ] || refuse "systemd’s loaded command doesn’t match $olo_file."
  [ "$olo_pkg" = "$unit_cmd" ] || stale=1
else
  [ "$loaded_cmd" = "$unit_cmd" ] || refuse "systemd loaded a different command than $frag."
fi
if [ "$scope" = "user" ]; then
  [ "$(id -u)" != "0" ] || refuse "Can’t check Rex’s flags without running $bin as root."
  [ -x "$bin" ] || refuse "$bin isn’t executable."
  help_out=$(rex_tm 5 "$bin" server run --help 2>&1 || true)
elif [ "$(id -u)" = "0" ]; then
  probe_user=""
  if [ -n "$unit_user" ] && [ "$unit_user" != "root" ] && [ "$unit_user" != "0" ]; then
    case "$unit_user" in
      *[!A-Za-z0-9_.-]*) ;;
      *)
        _pu="$unit_user"
        case "$_pu" in
          *[!0-9]*) ;;
          *) _pu=$(getent passwd "$_pu" 2>/dev/null | awk -F: 'NR==1 { print $1 }' 2>/dev/null || true) ;;
        esac
        if [ -n "$_pu" ]; then
          _puid=$(id -u "$_pu" 2>/dev/null || true)
          case "$_puid" in ''|*[!0-9]*|0) ;; *) probe_user="$_pu" ;; esac
        fi
        ;;
    esac
  else
    _nuid=$(id -u nobody 2>/dev/null || true)
    case "$_nuid" in ''|*[!0-9]*|0) ;; *) probe_user="nobody" ;; esac
  fi
  if [ -z "$probe_user" ] || ! command -v runuser >/dev/null 2>&1; then
    refuse "Can’t check Rex’s flags without running $bin as root."
  fi
  if ! rex_tm 5 runuser -u "$probe_user" -- test -x "$bin" 2>/dev/null; then
    if [ -z "$unit_user" ] || [ "$unit_user" = "root" ] || [ "$unit_user" = "0" ]; then
      refuse "Can’t check Rex’s flags without running $bin as root."
    else
      refuse "$bin isn’t executable as $probe_user."
    fi
  fi
  help_out=$(rex_tm 5 runuser -u "$probe_user" -- "$bin" server run --help 2>&1 || true)
else
  [ -x "$bin" ] || refuse "$bin isn’t executable."
  help_out=$(rex_tm 5 "$bin" server run --help 2>&1 || true)
fi
for fl in --max-windows --max-panes --max-sessions --max-blocks; do
  case "$help_out" in
    *"$fl"*) ;;
    *) refuse "$bin doesn’t support $fl." ;;
  esac
done
emit status ok
emit unit "$unit_cmd"
emit loaded "$loaded_cmd"
[ -n "$olo_pkg" ] && emit pkg "$olo_pkg"
emit stale "$stale"
"#
    };
}

/// The single shared inspection script text (for tests verifying both
/// [`LOOK`] and [`REX_APPLY`] embed it verbatim).
#[cfg(test)]
const REX_INSPECT: &str = rex_inspect_script!();

/// [`Tune::RexLimits`] applied: strictly **one scope per change** — writes
/// `/etc/systemd/system/rex.service.d/90-olo-limits.conf` for the shared
/// server, OR `${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/rex.service.d/90-olo-limits.conf`
/// **as the connected account** for a personal server (never `/etc/systemd/user`
/// and never touching a home directory as root; works without root when run
/// directly as the account). Uses the exact shared [`rex_inspect_script!`],
/// stages exclusively with `mktemp`, reloads systemd (failing on reload
/// error), reads back `LoadState=loaded` and exact loaded `ExecStart`, rolls
/// back on any failure, and prints a `Rex: …` line on every non-zero exit.
/// **No Rex server is restarted.**
const REX_APPLY: &str = concat!(
    r##"set -e
umask 022
OLO_HEAD=@HEAD@
SYS_DROPIN=@SYS_DROPIN@
_baked_mode=@MODE@
_baked_user=@USER@
if [ -n "$_baked_mode" ]; then OLO_REX_MODE="$_baked_mode"; else OLO_REX_MODE="${OLO_REX_MODE:-}"; fi
if [ -n "$_baked_user" ]; then OLO_REX_USER="$_baked_user"; else OLO_REX_USER="${OLO_REX_USER:-}"; fi
rex_done=0
rex_said=0
rex_fail() {
  rex_said=1
  printf 'Rex: %s\n' "$1"
  exit 1
}
rex_outer_exit() {
  if [ "$rex_done" != "1" ] && [ "$rex_said" != "1" ]; then
    printf '%s\n' 'Rex: Couldn’t apply Rex’s limits.'
  fi
}
trap rex_outer_exit EXIT
rex_tm() {
  if command -v timeout >/dev/null 2>&1; then timeout "$@"; else shift; "$@"; fi
}
rex_me_uid=$(id -u 2>/dev/null || echo 0)
rex_user="${OLO_REX_USER:-${SUDO_USER:-}}"
[ "$rex_user" = "root" ] && rex_user=""
if [ "$rex_me_uid" != "0" ] && [ -z "$rex_user" ]; then
  rex_user=$(id -un 2>/dev/null || true)
fi
rex_uid=""; rex_home=""
if [ -n "$rex_user" ]; then
  case "$rex_user" in
    *[!A-Za-z0-9_.-]*) rex_user="" ;;
    *)
      if [ "$rex_me_uid" != "0" ] && [ "$(id -un 2>/dev/null || true)" = "$rex_user" ]; then
        rex_uid="$rex_me_uid"
        rex_home="${HOME:-}"
      else
        rex_uid=$(id -u "$rex_user" 2>/dev/null || true)
        if command -v getent >/dev/null 2>&1; then
          rex_home=$(getent passwd "$rex_user" 2>/dev/null | awk -F: 'NR==1 { print $6 }' 2>/dev/null || true)
        fi
        if [ -z "$rex_home" ] && [ -r /etc/passwd ]; then
          rex_home=$(awk -F: -v u="$rex_user" '$1 == u { print $6; exit }' /etc/passwd 2>/dev/null || true)
        fi
      fi
      ;;
  esac
  case "$rex_uid" in ''|*[!0-9]*|0) rex_uid="" ;; esac
  case "$rex_home" in /*) ;; *) rex_home="" ;; esac
fi
rex_run_user() {
  if [ "$rex_me_uid" != "0" ]; then
    _d="${XDG_RUNTIME_DIR:-/run/user/$rex_uid}"
    _b="${DBUS_SESSION_BUS_ADDRESS:-unix:path=$_d/bus}"
    rex_tm 30 env "XDG_RUNTIME_DIR=$_d" "DBUS_SESSION_BUS_ADDRESS=$_b" "$@"
  else
    _d="/run/user/$rex_uid"
    rex_tm 30 runuser -u "$rex_user" -- env -u XDG_CONFIG_HOME "HOME=$rex_home" "USER=$rex_user" "LOGNAME=$rex_user" "XDG_RUNTIME_DIR=$_d" "DBUS_SESSION_BUS_ADDRESS=unix:path=$_d/bus" "$@"
  fi
}
if ! command -v systemctl >/dev/null 2>&1 || [ ! -d /run/systemd/system ]; then
  rex_fail "This host isn’t using systemd for Rex’s service."
fi
active_mode=""
case "${OLO_REX_MODE:-}" in shared|user) active_mode="$OLO_REX_MODE" ;; esac
if [ -z "$active_mode" ]; then
  if [ "$rex_me_uid" != "0" ]; then
    active_mode=user
  elif systemctl is-active --quiet rex.service 2>/dev/null || systemctl is-active --quiet rex.socket 2>/dev/null; then
    active_mode=shared
  elif systemctl is-active --quiet rex-login-server.service 2>/dev/null || systemctl is-active --quiet rex-login-server.socket 2>/dev/null; then
    active_mode=user
  elif [ -n "$rex_uid" ] && [ -n "$rex_home" ] && [ -S "/run/user/$rex_uid/bus" ] && command -v runuser >/dev/null 2>&1 \
       && { rex_run_user systemctl --user is-active --quiet rex.service 2>/dev/null || rex_run_user systemctl --user is-active --quiet rex.socket 2>/dev/null; }; then
    active_mode=user
  elif systemctl is-enabled --quiet rex.service 2>/dev/null || systemctl is-enabled --quiet rex.socket 2>/dev/null; then
    active_mode=shared
  else
    active_mode=user
  fi
fi
rex_inspect_sh=$(cat <<'__OLO_INSPECT__'
"##,
    rex_inspect_script!(),
    r##"__OLO_INSPECT__
)
rex_apply_one=$(cat <<'__OLO_APPLY_ONE__'
set -e
umask 022
scope=$1; OLO_HEAD=$2; SYS_DROPIN=$3; WW=$4; WP=$5; WS=$6; WB=$7; BUS_WHO=${8:-}
rex_done=0; rex_said=0; tmp_file=""; bkdir=""
rex_fail() {
  rex_said=1
  printf 'Rex: %s\n' "$1"
  exit 1
}
cleanup_one() {
  [ -n "$tmp_file" ] && rm -f -- "$tmp_file" 2>/dev/null || true
  [ -n "$bkdir" ] && rm -rf -- "$bkdir" 2>/dev/null || true
  if [ "$rex_done" != "1" ] && [ "$rex_said" != "1" ]; then
    printf '%s\n' 'Rex: Couldn’t apply Rex’s limits.'
  fi
}
trap cleanup_one EXIT
rex_tm() {
  if command -v timeout >/dev/null 2>&1; then timeout "$@"; else shift; "$@"; fi
}
rex_is_olo() {
  [ -f "$1" ] && [ ! -L "$1" ] || return 1
  IFS= read -r _h < "$1" 2>/dev/null || return 1
  [ "$_h" = "$OLO_HEAD" ]
}
if [ "$scope" = "user" ]; then
  if [ ! -S "${XDG_RUNTIME_DIR:-}/bus" ]; then
    if [ -n "$BUS_WHO" ]; then
      rex_fail "$BUS_WHO’s systemd user manager isn’t running."
    else
      rex_fail "Your systemd user manager isn’t running."
    fi
  fi
fi
command -v flock >/dev/null 2>&1 || rex_fail "Can’t serialize Rex limits changes on this host (flock isn’t installed)."
if [ "$scope" = "system" ]; then
  _ld="/run"
else
  _ld="${XDG_RUNTIME_DIR:-}"
fi
[ -n "$_ld" ] && [ -d "$_ld" ] && [ -w "$_ld" ] && [ ! -L "$_ld" ] || rex_fail "Can’t lock Rex’s limits directory."
_lf="$_ld/olo-rex-limits.lock"
[ ! -L "$_lf" ] || rex_fail "Refusing to open symlink lock file $_lf."
exec 9>>"$_lf" || rex_fail "Couldn’t open lock file $_lf."
flock -w 30 9 || rex_fail "Another Rex limits change is already running."
command -v awk >/dev/null 2>&1 || rex_fail "awk isn’t installed on this host."
read_kv() {
  printf '%s\n' "$1" | awk -v k="$2=" 'index($0, k) == 1 { print substr($0, length(k) + 1); exit }'
}
inspect_out=$(sh -c "$REX_INSPECT_SH" sh "$scope" "$OLO_HEAD" "$SYS_DROPIN") || rex_fail "Couldn’t inspect rex.service on this host."
st=$(read_kv "$inspect_out" status)
why=$(read_kv "$inspect_out" why)
target_file=$(read_kv "$inspect_out" file)
unit_cmd=$(read_kv "$inspect_out" unit)
loaded_cmd=$(read_kv "$inspect_out" loaded)
if [ "$st" != "ok" ] || [ -z "$target_file" ] || [ -z "$unit_cmd" ] || [ -z "$loaded_cmd" ]; then
  rex_fail "${why:-Olo can’t change rex.service on this host.}"
fi
new_cmd=$(printf '%s\n%s\n' "$unit_cmd" "$loaded_cmd" | awk -v ww="$WW" -v wp="$WP" -v ws="$WS" -v wb="$WB" '
  function dec_norm(s) { sub(/^0+/, "", s); return (s == "" ? "0" : s) }
  function dec_hi(a, b) {
    a = dec_norm(a); b = dec_norm(b)
    if (a == "0" || b == "0") return "0"
    if (length(a) != length(b)) return (length(a)>length(b) ? a : b)
    return (a >= b ? a : b)
  }
  function scan_limits(s, res,   w, n, i, skip, k, v) {
    res["w"] = "64"; res["p"] = "64"; res["s"] = "64"; res["b"] = "1024"
    n = split(s, w, / /)
    skip = 0
    for (i = 4; i <= n; i++) {
      if (skip) { skip = 0; continue }
      if (w[i] in vf) { skip = 1; continue }
      k = ""; v = ""
      if (w[i] == "--max-windows" && i < n) { k = "w"; v = w[++i] }
      else if (w[i] == "--max-panes" && i < n) { k = "p"; v = w[++i] }
      else if (w[i] == "--max-sessions" && i < n) { k = "s"; v = w[++i] }
      else if (w[i] == "--max-blocks" && i < n) { k = "b"; v = w[++i] }
      else if (w[i] ~ /^--max-windows=/) { k = "w"; v = substr(w[i], 15) }
      else if (w[i] ~ /^--max-panes=/) { k = "p"; v = substr(w[i], 13) }
      else if (w[i] ~ /^--max-sessions=/) { k = "s"; v = substr(w[i], 16) }
      else if (w[i] ~ /^--max-blocks=/) { k = "b"; v = substr(w[i], 14) }
      if (k != "" && v ~ /^[0-9]+$/) res[k] = dec_norm(v)
    }
  }
  BEGIN {
    mw = dec_norm(ww); mp = dec_norm(wp); ms = dec_norm(ws); mb = dec_norm(wb)
    vf["--listen"]=1; vf["--extra-listen"]=1; vf["--socket-mode"]=1
    vf["--tailscaled-socket"]=1; vf["--websocket-origin-pattern"]=1
    vf["--block-dir"]=1; vf["--state-dir"]=1; vf["--config"]=1
    vf["--tailscale-hostname"]=1; vf["--tailscale-control-url"]=1
    vf["--tailscale-auth-key-file"]=1; vf["--tailscale-advertise-tag"]=1
    vf["--log-format"]=1; vf["--log-level"]=1
    vf["-S"]=1; vf["--server"]=1; vf["-C"]=1; vf["--client"]=1
    vf["--timeout"]=1; vf["--color"]=1
  }
  NR == 1 { base = $0 }
  $0 != "" {
    scan_limits($0, cur)
    mw = dec_hi(mw, cur["w"])
    mp = dec_hi(mp, cur["p"])
    ms = dec_hi(ms, cur["s"])
    mb = dec_hi(mb, cur["b"])
  }
  END {
    n = split(base, w, / /)
    out = ""; skip = 0
    for (i = 1; i <= n; i++) {
      if (skip) { out = (out == "" ? w[i] : out " " w[i]); skip = 0; continue }
      if (i>3 && (w[i] in vf)) { out = (out == "" ? w[i] : out " " w[i]); skip = 1; continue }
      if (i>3 && (w[i] == "--max-windows" || w[i] == "--max-panes" || w[i] == "--max-sessions" || w[i] == "--max-blocks") && i < n) { i++; continue }
      if (i>3 && w[i] ~ /^--max-(windows|panes|sessions|blocks)=/) continue
      out = (out == "" ? w[i] : out " " w[i])
    }
    printf "%s --max-windows %s --max-panes %s --max-sessions %s --max-blocks %s\n", out, mw, mp, ms, mb
  }
') || rex_fail "Couldn’t build the new Rex command."
[ -n "$new_cmd" ] || rex_fail "Couldn’t build the new Rex command."
target_dir=${target_file%/*}
if [ "$scope" = "user" ]; then
  _d2=${target_dir%/*}
  _d1=${_d2%/*}
  for _p in "$_d1" "$_d2" "$target_dir"; do
    [ ! -L "$_p" ] || rex_fail "$_p is a symbolic link; refusing to write there."
  done
else
  [ ! -L "$target_dir" ] || rex_fail "$target_dir is a symbolic link; refusing to write there."
fi
bkdir=$(mktemp -d /var/tmp/olo-rex-limits.XXXXXX) || rex_fail "Couldn’t create a temporary backup directory."
had_prev=0
if rex_is_olo "$target_file"; then
  cp -p -- "$target_file" "$bkdir/prev" || rex_fail "Couldn’t back up $target_file."
  had_prev=1
fi
mkdir -p -- "$target_dir" || rex_fail "Couldn’t create $target_dir."
[ -d "$target_dir" ] && [ ! -L "$target_dir" ] || rex_fail "$target_dir isn’t a regular directory."
tmp_file=$(mktemp "$target_dir/.90-olo-limits.conf.XXXXXX") || rex_fail "Couldn’t stage $target_file."
printf '%s\n' "$OLO_HEAD" "# packaged: $unit_cmd" '[Service]' 'ExecStart=' "ExecStart=$new_cmd" > "$tmp_file" || rex_fail "Couldn’t write $target_file."
chmod 0644 "$tmp_file" || rex_fail "Couldn’t set permissions on $target_file."
verify_loaded() {
  _post=$(sh -c "$REX_INSPECT_SH" sh "$scope" "$OLO_HEAD" "$SYS_DROPIN" 2>/dev/null) || return 1
  _pst=$(read_kv "$_post" status)
  _pld=$(read_kv "$_post" loaded)
  [ "$_pst" = "ok" ] && [ "$_pld" = "$new_cmd" ]
}
moved=0
rollback() {
  _msg=$1
  if [ "$moved" = "1" ]; then
    if [ "$had_prev" = "1" ]; then
      _rt=$(mktemp "$target_dir/.90-olo-limits.conf.XXXXXX" 2>/dev/null) && cp -p -- "$bkdir/prev" "$_rt" && chmod 0644 "$_rt" && mv -f -- "$_rt" "$target_file" || true
    else
      rm -f -- "$target_file" || true
    fi
    if [ "$scope" = "user" ]; then
      rex_tm 10 systemctl --user daemon-reload >/dev/null 2>&1 || true
    else
      rex_tm 10 systemctl daemon-reload >/dev/null 2>&1 || true
    fi
  fi
  rex_fail "$_msg"
}
set +e
mv -f -- "$tmp_file" "$target_file" || rollback "Couldn’t install $target_file; previous settings were kept."
tmp_file=""; moved=1
if [ "$scope" = "user" ]; then
  rex_tm 10 systemctl --user daemon-reload || rollback "Couldn’t reload your systemd user manager; previous settings were restored."
  verify_loaded || rollback "Your systemd user manager didn’t load the expected command for rex.service; previous settings were restored."
else
  rex_tm 10 systemctl daemon-reload || rollback "systemd couldn’t reload its units; previous settings were restored."
  verify_loaded || rollback "systemd didn’t load the expected command for rex.service; previous settings were restored."
fi
rex_done=1
__OLO_APPLY_ONE__
)
rex_run_checked() {
  _rc=0
  _out=$("$@" 2>&1) || _rc=$?
  [ -n "$_out" ] && printf '%s\n' "$_out"
  if [ "$_rc" != "0" ]; then
    case "$_out" in
      *'Rex: '*) rex_said=1 ;;
      *) rex_fail "Couldn’t apply Rex’s limits." ;;
    esac
    exit "$_rc"
  fi
}
if [ "$active_mode" = "shared" ]; then
  rex_run_checked env "REX_INSPECT_SH=$rex_inspect_sh" sh -c "$rex_apply_one" sh system "$OLO_HEAD" "$SYS_DROPIN" @WINDOWS@ @PANES@ @SESSIONS@ @BLOCKS@ ""
else
  _bus_who=""
  if [ "$rex_me_uid" = "0" ]; then
    [ -n "$rex_user" ] || rex_fail "Connect as your account (or use sudo) to change your Rex user service."
    command -v runuser >/dev/null 2>&1 || rex_fail "Olo can’t run as $rex_user on this host (runuser isn’t installed)."
    [ -n "$rex_uid" ] && [ -n "$rex_home" ] || rex_fail "Couldn’t find $rex_user’s home directory on this host."
    _bus_who="$rex_user"
  fi
  rex_run_checked rex_run_user env "REX_INSPECT_SH=$rex_inspect_sh" sh -c "$rex_apply_one" sh user "$OLO_HEAD" "$SYS_DROPIN" @WINDOWS@ @PANES@ @SESSIONS@ @BLOCKS@ "$_bus_who"
fi
rex_done=1
"##
);

/// [`Tune::RexLimits`] taken back: removes ONLY Olo's header-verified
/// file for the active scope (`/etc/systemd/system/rex.service.d/90-olo-limits.conf`
/// for shared, or `${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/rex.service.d/90-olo-limits.conf`
/// run **as the connected account** for personal — works with no root when
/// run directly as the account). Never removes foreign files, symlinks, or
/// directories; reloads systemd (rolling back on reload error); never
/// restarts any Rex server; emits `Rex: …` on every non-zero exit.
const REX_UNDO: &str = r#"set -e
umask 022
OLO_HEAD=@HEAD@
SYS_DROPIN=@SYS_DROPIN@
rex_done=0
rex_said=0
rex_fail() {
  rex_said=1
  printf 'Rex: %s\n' "$1"
  exit 1
}
rex_outer_exit() {
  if [ "$rex_done" != "1" ] && [ "$rex_said" != "1" ]; then
    printf '%s\n' 'Rex: Couldn’t put Rex’s limits back.'
  fi
}
trap rex_outer_exit EXIT
rex_tm() {
  if command -v timeout >/dev/null 2>&1; then timeout "$@"; else shift; "$@"; fi
}
rex_me_uid=$(id -u 2>/dev/null || echo 0)
rex_user="${OLO_REX_USER:-${SUDO_USER:-}}"
[ "$rex_user" = "root" ] && rex_user=""
if [ "$rex_me_uid" != "0" ] && [ -z "$rex_user" ]; then
  rex_user=$(id -un 2>/dev/null || true)
fi
rex_uid=""; rex_home=""
if [ -n "$rex_user" ]; then
  case "$rex_user" in
    *[!A-Za-z0-9_.-]*) rex_user="" ;;
    *)
      if [ "$rex_me_uid" != "0" ] && [ "$(id -un 2>/dev/null || true)" = "$rex_user" ]; then
        rex_uid="$rex_me_uid"
        rex_home="${HOME:-}"
      else
        rex_uid=$(id -u "$rex_user" 2>/dev/null || true)
        if command -v getent >/dev/null 2>&1; then
          rex_home=$(getent passwd "$rex_user" 2>/dev/null | awk -F: 'NR==1 { print $6 }' 2>/dev/null || true)
        fi
        if [ -z "$rex_home" ] && [ -r /etc/passwd ]; then
          rex_home=$(awk -F: -v u="$rex_user" '$1 == u { print $6; exit }' /etc/passwd 2>/dev/null || true)
        fi
      fi
      ;;
  esac
  case "$rex_uid" in ''|*[!0-9]*|0) rex_uid="" ;; esac
  case "$rex_home" in /*) ;; *) rex_home="" ;; esac
fi
rex_run_user() {
  if [ "$rex_me_uid" != "0" ]; then
    _d="${XDG_RUNTIME_DIR:-/run/user/$rex_uid}"
    _b="${DBUS_SESSION_BUS_ADDRESS:-unix:path=$_d/bus}"
    rex_tm 30 env "XDG_RUNTIME_DIR=$_d" "DBUS_SESSION_BUS_ADDRESS=$_b" "$@"
  else
    _d="/run/user/$rex_uid"
    rex_tm 30 runuser -u "$rex_user" -- env -u XDG_CONFIG_HOME "HOME=$rex_home" "USER=$rex_user" "LOGNAME=$rex_user" "XDG_RUNTIME_DIR=$_d" "DBUS_SESSION_BUS_ADDRESS=unix:path=$_d/bus" "$@"
  fi
}
active_mode=""
case "${OLO_REX_MODE:-}" in shared|user) active_mode="$OLO_REX_MODE" ;; esac
if [ -z "$active_mode" ]; then
  if [ "$rex_me_uid" != "0" ]; then
    active_mode=user
  elif command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
    if systemctl is-active --quiet rex.service 2>/dev/null || systemctl is-active --quiet rex.socket 2>/dev/null; then
      active_mode=shared
    elif systemctl is-active --quiet rex-login-server.service 2>/dev/null || systemctl is-active --quiet rex-login-server.socket 2>/dev/null; then
      active_mode=user
    elif [ -n "$rex_uid" ] && [ -n "$rex_home" ] && [ -S "/run/user/$rex_uid/bus" ] && command -v runuser >/dev/null 2>&1 \
         && { rex_run_user systemctl --user is-active --quiet rex.service 2>/dev/null || rex_run_user systemctl --user is-active --quiet rex.socket 2>/dev/null; }; then
      active_mode=user
    elif systemctl is-enabled --quiet rex.service 2>/dev/null || systemctl is-enabled --quiet rex.socket 2>/dev/null; then
      active_mode=shared
    else
      active_mode=user
    fi
  else
    active_mode=user
  fi
fi
rex_undo_one=$(cat <<'__OLO_UNDO_ONE__'
set -e
umask 022
scope=$1; OLO_HEAD=$2; SYS_DROPIN=$3; BUS_WHO=${4:-}
rex_done=0; rex_said=0; bkdir=""
rex_fail() {
  rex_said=1
  printf 'Rex: %s\n' "$1"
  exit 1
}
cleanup_undo() {
  [ -n "$bkdir" ] && rm -rf -- "$bkdir" 2>/dev/null || true
  if [ "$rex_done" != "1" ] && [ "$rex_said" != "1" ]; then
    printf '%s\n' 'Rex: Couldn’t put Rex’s limits back.'
  fi
}
trap cleanup_undo EXIT
rex_tm() {
  if command -v timeout >/dev/null 2>&1; then timeout "$@"; else shift; "$@"; fi
}
rex_is_olo() {
  [ -f "$1" ] && [ ! -L "$1" ] || return 1
  IFS= read -r _h < "$1" 2>/dev/null || return 1
  [ "$_h" = "$OLO_HEAD" ]
}
if [ "$scope" = "user" ]; then
  if [ ! -S "${XDG_RUNTIME_DIR:-}/bus" ]; then
    if [ -n "$BUS_WHO" ]; then
      rex_fail "$BUS_WHO’s systemd user manager isn’t running."
    else
      rex_fail "Your systemd user manager isn’t running."
    fi
  fi
fi
command -v flock >/dev/null 2>&1 || rex_fail "Can’t serialize Rex limits changes on this host (flock isn’t installed)."
command -v awk >/dev/null 2>&1 || rex_fail "awk isn’t installed on this host."
if [ "$scope" = "system" ]; then
  _ld="/run"
  target_file="$SYS_DROPIN"
  target_dir=${target_file%/*}
  [ ! -L "$target_dir" ] || rex_fail "$target_dir is a symbolic link; nothing was removed."
else
  _ld="${XDG_RUNTIME_DIR:-}"
  _cfg="${XDG_CONFIG_HOME:-${HOME:-}/.config}"
  case "$_cfg" in /*) ;; *) rex_fail "Could not determine your systemd user config directory." ;; esac
  _d1="$_cfg/systemd"; _d2="$_d1/user"; target_dir="$_d2/rex.service.d"
  for _p in "$_d1" "$_d2" "$target_dir"; do
    [ ! -L "$_p" ] || rex_fail "$_p is a symbolic link; nothing was removed."
  done
  target_file="$target_dir/90-olo-limits.conf"
fi
[ -n "$_ld" ] && [ -d "$_ld" ] && [ -w "$_ld" ] && [ ! -L "$_ld" ] || rex_fail "Can’t lock Rex’s limits directory."
_lf="$_ld/olo-rex-limits.lock"
[ ! -L "$_lf" ] || rex_fail "Refusing to open symlink lock file $_lf."
exec 9>>"$_lf" || rex_fail "Couldn’t open lock file $_lf."
flock -w 30 9 || rex_fail "Another Rex limits change is already running."
undo_show() {
  if [ "$scope" = "user" ]; then
    rex_tm 10 systemctl --user show -p LoadState -p DropInPaths -p ExecStart rex.service 2>/dev/null
  else
    rex_tm 10 systemctl show -p LoadState -p DropInPaths -p ExecStart rex.service 2>/dev/null
  fi
}
undo_reload() {
  if [ "$scope" = "user" ]; then
    rex_tm 10 systemctl --user daemon-reload
  else
    rex_tm 10 systemctl daemon-reload
  fi
}
check_after_undo() {
  _restored_suffix=$1
  _so=$(undo_show) || { undo_err="systemd couldn’t report how rex.service is configured after reload$_restored_suffix"; return 1; }
  _ls=$(printf '%s\n' "$_so" | awk 'index($0, "LoadState=") == 1 { print substr($0, 11); exit }')
  _dp=$(printf '%s\n' "$_so" | awk 'index($0, "DropInPaths=") == 1 { print substr($0, 13); exit }')
  _has_exec=$(printf '%s\n' "$_so" | awk '/^ExecStart=\{ path=[^ ;]+ ; argv\[\]=.+ ; / { ok = 1 } END { print (ok ? 1 : 0) }')
  set -f
  for _d in $_dp; do
    if [ "$_d" = "$target_file" ]; then
      set +f
      undo_err="systemd still has $target_file loaded after reload$_restored_suffix"
      return 1
    fi
  done
  set +f
  if [ "$_has_exec" != "1" ]; then
    undo_err="without Olo’s file rex.service has no command$_restored_suffix"
    return 1
  fi
  if [ "$_ls" != "loaded" ]; then
    undo_err="without Olo’s file rex.service isn’t loaded cleanly (LoadState=${_ls:-unknown})$_restored_suffix"
    return 1
  fi
  return 0
}
if [ ! -e "$target_file" ] && [ ! -L "$target_file" ]; then
  _pre=$(undo_show) || rex_fail "systemd couldn’t report how rex.service is configured."
  _pdp=$(printf '%s\n' "$_pre" | awk 'index($0, "DropInPaths=") == 1 { print substr($0, 13); exit }')
  _still=0
  set -f
  for _d in $_pdp; do
    [ "$_d" = "$target_file" ] && _still=1
  done
  set +f
  if [ "$_still" = "1" ]; then
    set +e
    undo_reload || rex_fail "systemd couldn’t reload its units."
    check_after_undo "." || rex_fail "$undo_err"
  fi
  rex_done=1
  exit 0
fi
if ! rex_is_olo "$target_file"; then
  rex_fail "$target_file wasn’t written by Olo; nothing was removed."
fi
bkdir=$(mktemp -d /var/tmp/olo-rex-limits.XXXXXX) || rex_fail "Couldn’t create a temporary backup directory."
cp -p -- "$target_file" "$bkdir/prev" || rex_fail "Couldn’t back up $target_file."
rollback_undo() {
  _msg=$1
  _rt=$(mktemp "$target_dir/.90-olo-limits.conf.XXXXXX" 2>/dev/null) && cp -p -- "$bkdir/prev" "$_rt" && chmod 0644 "$_rt" && mv -f -- "$_rt" "$target_file" || true
  undo_reload >/dev/null 2>&1 || true
  rex_fail "$_msg"
}
set +e
rm -- "$target_file" || rex_fail "Couldn’t remove $target_file."
if [ "$scope" = "user" ]; then
  undo_reload || rollback_undo "Couldn’t reload your systemd user manager; previous settings were restored."
else
  undo_reload || rollback_undo "systemd couldn’t reload its units; previous settings were restored."
fi
check_after_undo ": left as it was." || rollback_undo "$undo_err"
rex_done=1
__OLO_UNDO_ONE__
)
rex_run_checked() {
  _rc=0
  _out=$("$@" 2>&1) || _rc=$?
  [ -n "$_out" ] && printf '%s\n' "$_out"
  if [ "$_rc" != "0" ]; then
    case "$_out" in
      *'Rex: '*) rex_said=1 ;;
      *) rex_fail "Couldn’t put Rex’s limits back." ;;
    esac
    exit "$_rc"
  fi
}
if [ "$active_mode" = "shared" ]; then
  rex_run_checked sh -c "$rex_undo_one" sh system "$OLO_HEAD" "$SYS_DROPIN" ""
else
  _bus_who=""
  if [ "$rex_me_uid" = "0" ]; then
    [ -n "$rex_user" ] || rex_fail "Connect as your account (or use sudo) to change your Rex user service."
    command -v runuser >/dev/null 2>&1 || rex_fail "Olo can’t run as $rex_user on this host (runuser isn’t installed)."
    [ -n "$rex_uid" ] && [ -n "$rex_home" ] || rex_fail "Couldn’t find $rex_user’s home directory on this host."
    _bus_who="$rex_user"
  fi
  rex_run_checked rex_run_user sh -c "$rex_undo_one" sh user "$OLO_HEAD" "$SYS_DROPIN" "$_bus_who"
fi
rex_done=1
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

/// Known flags of `rex server run` that consume the following argv element
/// as their value (so a value looking like `--max-windows=…` is never read
/// as a limit flag).
const REX_VALUE_FLAGS: &[&str] = &[
    "--listen",
    "--extra-listen",
    "--socket-mode",
    "--tailscaled-socket",
    "--websocket-origin-pattern",
    "--block-dir",
    "--state-dir",
    "--config",
    "--tailscale-hostname",
    "--tailscale-control-url",
    "--tailscale-auth-key-file",
    "--tailscale-advertise-tag",
    "--log-format",
    "--log-level",
    "-S",
    "--server",
    "-C",
    "--client",
    "--timeout",
    "--color",
];

/// Which systemd scope holds the active Rex server for this connection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RexScope {
    /// System `rex.service` (the shared server, restartable from the Server window).
    Shared,
    /// The connected account's user `rex.service` (Login or Personal mode).
    #[default]
    User,
}

/// The four numeric ceilings of a `rex server run` command line
/// (`u64::MAX` means `0`, no limit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RexLimitValues {
    pub windows: u64,
    pub panes: u64,
    pub sessions: u64,
    pub blocks: u64,
}

fn is_plain_word(w: &str) -> bool {
    !w.is_empty() && w != "--" && w.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b'=' | b':' | b',' | b'@' | b'-'))
}

fn is_rex_path(w: &str) -> bool {
    w.starts_with('/') && (w == "/rex" || w.ends_with("/rex")) && !w.contains("/../") && !w.ends_with("/..") && is_plain_word(w)
}

fn parse_limit_decimal(raw: &str) -> Option<u64> {
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let trimmed = raw.trim_start_matches('0');
    if trimmed.is_empty() {
        return Some(u64::MAX);
    }
    let v = trimmed.parse::<u64>().ok()?;
    (v < u64::MAX).then_some(v)
}

impl RexLimitValues {
    /// Rex's own defaults when no `--max-*` flag is given.
    pub const fn defaults() -> Self {
        Self { windows: REX_DEFAULT_WINDOWS, panes: REX_DEFAULT_PANES, sessions: REX_DEFAULT_SESSIONS, blocks: REX_DEFAULT_BLOCKS }
    }

    /// What Olo sets.
    pub const fn wanted() -> Self {
        Self { windows: REX_WINDOWS, panes: REX_PANES, sessions: REX_SESSIONS, blocks: REX_BLOCKS }
    }

    /// Parses the `--max-*` flags from `args` (the elements following
    /// `<rex> server run`). Returns `None` if any limit flag has a
    /// missing, quoted, non-numeric, or overflowing value.
    pub fn parse_flag_argv(args: &[&str]) -> Option<Self> {
        let mut out = Self::defaults();
        let mut i = 0;
        while i < args.len() {
            let tok = args[i];
            if tok == "--" {
                break;
            }
            if REX_VALUE_FLAGS.contains(&tok) {
                i += 2;
                continue;
            }
            let (flag, val) = if matches!(tok, "--max-windows" | "--max-panes" | "--max-sessions" | "--max-blocks") {
                i += 1;
                (Some(tok), args.get(i).copied())
            } else if let Some(v) = tok.strip_prefix("--max-windows=") {
                (Some("--max-windows"), Some(v))
            } else if let Some(v) = tok.strip_prefix("--max-panes=") {
                (Some("--max-panes"), Some(v))
            } else if let Some(v) = tok.strip_prefix("--max-sessions=") {
                (Some("--max-sessions"), Some(v))
            } else if let Some(v) = tok.strip_prefix("--max-blocks=") {
                (Some("--max-blocks"), Some(v))
            } else {
                (None, None)
            };
            if let Some(flag) = flag {
                let parsed = parse_limit_decimal(val?)?;
                match flag {
                    "--max-windows" => out.windows = parsed,
                    "--max-panes" => out.panes = parsed,
                    "--max-sessions" => out.sessions = parsed,
                    "--max-blocks" => out.blocks = parsed,
                    _ => {}
                }
            }
            i += 1;
        }
        Some(out)
    }

    /// Strictly parses a plain-word `ExecStart` command line matching the
    /// Point 2 shape (`/ABS/PATH/rex server run` followed by plain words
    /// only, with no quotes, backslashes, `%`, `$`, `;`, or prefixes).
    /// Returns `None` for any non-plain syntax or malformed limit value.
    pub fn parse_plain_execstart(cmdline: &str) -> Option<Self> {
        if cmdline.is_empty() || cmdline.len() > 8192 || cmdline.bytes().any(|b| b < 0x20 || b == 0x7f || matches!(b, b'"' | b'\'' | b'\\' | b'%' | b'$' | b';')) {
            return None;
        }
        let words: Vec<&str> = cmdline.split_ascii_whitespace().collect();
        if words.len() < 3 || !is_rex_path(words[0]) || words[1] != "server" || words[2] != "run" {
            return None;
        }
        if !words[3..].iter().all(|w| is_plain_word(w)) {
            return None;
        }
        Self::parse_flag_argv(&words[3..])
    }

    /// Parses raw NUL-delimited `/proc/PID/cmdline` bytes directly,
    /// preserving exact argv boundaries so spaces inside an argument never
    /// masquerade as flags. Returns `None` if malformed or not `rex server run`.
    pub fn parse_proc_cmdline(mut bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() || bytes.len() > 8192 {
            return None;
        }
        while bytes.last() == Some(&0) {
            bytes = &bytes[..bytes.len() - 1];
        }
        if bytes.is_empty() {
            return None;
        }
        let mut argv = Vec::new();
        for piece in bytes.split(|b| *b == 0) {
            argv.push(std::str::from_utf8(piece).ok()?);
        }
        if argv.len() < 3 || argv[1] != "server" || argv[2] != "run" {
            return None;
        }
        Self::parse_flag_argv(&argv[3..])
    }

    pub fn get(self, name: &str) -> Option<u64> {
        match name {
            "--max-windows" => Some(self.windows),
            "--max-panes" => Some(self.panes),
            "--max-sessions" => Some(self.sessions),
            "--max-blocks" => Some(self.blocks),
            _ => None,
        }
    }
}

/// Plain-word meaning of each `rex server run --max-*` flag, shown beside
/// the flag in confirmation tables.
pub fn rex_limit_note(name: &str) -> Option<&'static str> {
    match name {
        "--max-windows" => Some("tabs in a session"),
        "--max-panes" => Some("panes in a tab"),
        "--max-sessions" => Some("sessions"),
        "--max-blocks" => Some("blocks in all"),
        _ => None,
    }
}

/// How [`Tune::RexLimits`] stands on the host beyond its four configured
/// values: which scope is active, which account's user unit was inspected,
/// whether Olo's strict-shape check refused (and why), what the running
/// server's `MainPID` was started with, what limits hold without Olo's
/// file, which Olo files exist, and whether Olo's file is stale after a
/// unit change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RexLimits {
    /// Active server scope (`Shared` = system `rex.service`, `User` = user `rex.service`).
    pub scope: RexScope,
    /// Account whose user manager was inspected (when known).
    pub user: Option<String>,
    /// Plain-words refusal reason when the active unit doesn't satisfy the
    /// strict shape (or systemd / user manager wasn't reachable). When
    /// `Some`, Apply is disabled and excluded from Apply Suggested; Put
    /// Back stays available whenever an Olo-owned file is present.
    pub blocked: Option<String>,
    /// Limits the active manager's `MainPID` was started with, parsed
    /// directly from NUL-delimited `/proc/PID/cmdline`.
    pub running: Option<RexLimitValues>,
    /// Limits the active unit file has without Olo's drop-in, so Put Back…
    /// can show what it goes back to.
    pub without_olo: Option<RexLimitValues>,
    /// Whether Olo's verified header-owned file exists in each scope.
    pub kept_sys: bool,
    pub kept_usr: bool,
    /// Resolved path of the connected account's own user drop-in
    /// (`${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/rex.service.d/90-olo-limits.conf`),
    /// as reported by the account's own inspection.
    pub file_usr: Option<String>,
    /// Olo's drop-in recorded an older unit `ExecStart=` than the unit
    /// file currently ships.
    pub stale: bool,
    /// When `stale`, the old and new unit `ExecStart=` lines.
    pub stale_was: Option<String>,
    pub stale_now: Option<String>,
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
    /// Extra state for [`Tune::RexLimits`]: running server limits,
    /// stale drop-in after a Rex upgrade, or a shadowing drop-in.
    pub rex_limits: RexLimits,
}

impl Tuning {
    /// Whether putting [`Tune::Shell`] back takes brush off the host:
    /// nobody signs in to it but the accounts that get their own shell
    /// back.
    pub fn brush_goes(&self) -> bool {
        self.brush_users.iter().all(|user| self.was_accounts.iter().any(|(name, _)| name == user))
    }

    /// When Rex's configured unit limits differ from what the running Rex
    /// server was started with (either higher after Apply or lower after
    /// Put Back): returns `(running, configured)` so the tab can say what
    /// holds once the server restarts.
    pub fn rex_limits_pending_restart(&self) -> Option<(RexLimitValues, RexLimitValues)> {
        if self.rex_limits.blocked.is_some() {
            return None;
        }
        let running = self.rex_limits.running?;
        let stands = self.of(Tune::RexLimits);
        let get = |name: &str| stands.values.iter().find(|v| v.name == name).and_then(|v| v.now);
        let configured = RexLimitValues { windows: get("--max-windows")?, panes: get("--max-panes")?, sessions: get("--max-sessions")?, blocks: get("--max-blocks")? };
        let differs = running.windows != configured.windows || running.panes != configured.panes || running.sessions != configured.sessions || running.blocks != configured.blocks;
        differs.then_some((running, configured))
    }
}

impl Tuning {
    /// Marks [`Tune::RexLimits`] unreadable/blocked with `why` and clears
    /// all ownership and pending state so neither Apply nor Put Back is
    /// offered and the row is excluded from Apply Suggested.
    pub fn block_rex(&mut self, why: impl Into<String>) {
        self.rex_limits.blocked = Some(why.into());
        self.rex_limits.kept_sys = false;
        self.rex_limits.kept_usr = false;
        self.rex_limits.stale = false;
        self.rex_limits.stale_was = None;
        self.rex_limits.stale_now = None;
        self.rex_limits.running = None;
        self.rex_limits.without_olo = None;
        if let Some(stands) = self.stands.iter_mut().find(|s| s.tune == Tune::RexLimits) {
            stands.kept = false;
            for v in &mut stands.values {
                v.now = None;
            }
        }
    }

    pub fn of(&self, tune: Tune) -> &Stands {
        self.stands.iter().find(|s| s.tune == tune).expect("every kind is in it")
    }

    /// The kinds that could be applied: something of each is short (or,
    /// for Rex server limits, Olo's drop-in was built from an older unit
    /// and needs refreshing). Blocked Rex limits are never included.
    pub fn short(&self) -> Vec<Tune> {
        self.stands
            .iter()
            .filter(|s| {
                if s.tune == Tune::RexLimits && self.rex_limits.blocked.is_some() {
                    return false;
                }
                !s.met() || (s.tune == Tune::RexLimits && self.rex_limits.stale)
            })
            .map(|s| s.tune)
            .collect()
    }

    /// [`LOOK`]'s standard output, read: a line for each setting (its name,
    /// a space, its value), `ram` in kB, `kept ID` for non-Rex kinds, and
    /// typed `rex.v1.*` records for [`Tune::RexLimits`]. Malformed records
    /// are treated as unknown.
    pub fn read(output: &str) -> Tuning {
        use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
        let number = |text: &str| -> Option<u64> {
            let text = text.trim();
            match text {
                "infinity" | "unlimited" | "none" => Some(u64::MAX),
                _ => text.parse().ok(),
            }
        };
        let said = |name: &str| output.lines().find_map(|line| line.strip_prefix(name).and_then(|rest| rest.strip_prefix(' ')).and_then(number));
        let word = |name: &str| output.lines().find_map(|line| line.strip_prefix(name).and_then(|rest| rest.strip_prefix(' '))).map(str::trim).filter(|w| !w.is_empty()).map(str::to_owned);

        // Parse typed, bounded `rex.v1.*` records from stdout only.
        let mut rex_fields: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        let mut saw_rex = false;
        let mut malformed_rex = false;
        for line in output.lines() {
            if let Some(rest) = line.strip_prefix("rex.v1.") {
                saw_rex = true;
                if line.len() > 16_384 || line.bytes().any(|b| b < 0x20 || b == 0x7f) {
                    malformed_rex = true;
                    continue;
                }
                if let Some((k, v)) = rest.split_once('=') {
                    if let Some(prev) = rex_fields.insert(k, v)
                        && prev != v
                    {
                        malformed_rex = true;
                    }
                } else {
                    malformed_rex = true;
                }
            }
        }
        let rex_get = |k: &str| rex_fields.get(k).copied();
        let parse_bool_flag = |k: &str| match rex_get(k) {
            Some("1") => Some(true),
            Some("0") => Some(false),
            _ => None,
        };

        let mut kept_sys = parse_bool_flag("kept.system").unwrap_or(false);
        let mut kept_usr = parse_bool_flag("kept.user").unwrap_or(false);
        if rex_get("kept.system").is_some_and(|v| v != "0" && v != "1") || rex_get("kept.user").is_some_and(|v| v != "0" && v != "1") {
            malformed_rex = true;
        }
        let file_usr = rex_get("file.user").filter(|p| p.starts_with('/') && p.len() <= 4096).map(str::to_owned);
        let rex_user = rex_get("user").filter(|u| !u.is_empty() && u.len() <= 64 && u.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))).map(str::to_owned);

        let (scope, scope_key, mode_ok) = match rex_get("mode") {
            Some("shared") => (RexScope::Shared, "system", true),
            Some("user") => (RexScope::User, "user", true),
            _ => (RexScope::User, "user", false),
        };

        let mut rex_configured: Option<RexLimitValues> = None;
        let mut rex_without_olo: Option<RexLimitValues> = None;
        let mut rex_running: Option<RexLimitValues> = None;
        let mut blocked: Option<String> = None;
        let mut stale = false;
        let mut stale_was: Option<String> = None;
        let mut stale_now: Option<String> = None;

        let kept_key = format!("kept.{scope_key}");
        let active_kept_opt = parse_bool_flag(&kept_key);
        let missing_or_bad = !saw_rex || malformed_rex || !mode_ok || active_kept_opt.is_none();

        if missing_or_bad {
            // An answer with no complete, consistent `rex.v1` record is never a yes
            // and never exposes Apply or Put Back (Finding 19).
            blocked = Some("Olo couldn’t read this host’s Rex service.".to_owned());
            kept_sys = false;
            kept_usr = false;
        } else {
            let status_key = format!("status.{scope_key}");
            let why_key = format!("why.{scope_key}");
            let unit_key = format!("unit.{scope_key}");
            let loaded_key = format!("loaded.{scope_key}");
            let pkg_key = format!("pkg.{scope_key}");
            let stale_key = format!("stale.{scope_key}");
            let running_key = format!("running.{scope_key}");

            // Running server limits from the active manager's MainPID only.
            if let Some(raw_run) = rex_get(&running_key)
                && let Some(b64) = raw_run.strip_prefix("b64:")
                && b64.len() <= 12_288
                && let Ok(bytes) = B64.decode(b64)
            {
                rex_running = RexLimitValues::parse_proc_cmdline(&bytes);
            }

            match rex_get(&status_key) {
                Some("blocked") => {
                    let why = rex_get(&why_key).map(str::trim).filter(|w| !w.is_empty()).unwrap_or("Olo can’t change Rex’s limits on this host.");
                    blocked = Some(why.to_owned());
                }
                Some("ok") => {
                    let unit_str = rex_get(&unit_key);
                    let loaded_str = rex_get(&loaded_key);
                    let parsed_unit = unit_str.and_then(RexLimitValues::parse_plain_execstart);
                    let parsed_loaded = loaded_str.and_then(RexLimitValues::parse_plain_execstart);
                    let parsed_stale = match rex_get(&stale_key) {
                        Some("0") | None => Some(false),
                        Some("1") => Some(true),
                        _ => None,
                    };
                    if let (Some(u_lim), Some(l_lim), Some(st)) = (parsed_unit, parsed_loaded, parsed_stale) {
                        rex_configured = Some(l_lim);
                        rex_without_olo = Some(u_lim);
                        stale = st;
                        if stale {
                            stale_was = rex_get(&pkg_key).filter(|s| !s.is_empty()).map(str::to_owned);
                            stale_now = unit_str.map(str::to_owned);
                        }
                    } else {
                        blocked = Some("Olo couldn’t read this host’s Rex service.".to_owned());
                        kept_sys = false;
                        kept_usr = false;
                    }
                }
                _ => {
                    blocked = Some("Olo couldn’t read this host’s Rex service.".to_owned());
                    kept_sys = false;
                    kept_usr = false;
                }
            }
        }

        let active_kept = match scope {
            RexScope::Shared => kept_sys,
            RexScope::User => kept_usr,
        };
        let ram = said("ram").unwrap_or(0).saturating_mul(1024);
        let stands = Tune::ALL
            .into_iter()
            .map(|tune| Stands {
                tune,
                values: tune
                    .wanted(ram)
                    .into_iter()
                    .map(|(name, wanted, at_most)| {
                        let now = if tune == Tune::RexLimits { rex_configured.and_then(|v| v.get(name)) } else { said(name) };
                        Value { name, now, wanted, at_most }
                    })
                    .collect(),
                kept: if tune == Tune::RexLimits { active_kept } else { output.lines().any(|line| line.trim() == format!("kept {}", tune.id())) },
            })
            .collect();
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
            rex_limits: RexLimits { scope, user: rex_user, blocked, running: rex_running, without_olo: rex_without_olo, kept_sys, kept_usr, file_usr, stale, stale_was, stale_now },
        }
    }
}

/// Asks a host, as root, how its limits stand: what [`Tuning::read`]
/// reads. (The open files are those of this very command's shell: what
/// a sign-in over SSH is given.) Embeds the exact same [`rex_inspect_script!`]
/// as [`REX_APPLY`].
pub const LOOK: &str = concat!(
    r##"echo "ram $(awk '/^MemTotal:/ {print $2}' /proc/meminfo)"
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
OLO_HEAD="# Olo's Rex put this here (the Host window, Tweaks). Put it back there, or delete this file."
SYS_DROPIN="/etc/systemd/system/rex.service.d/90-olo-limits.conf"
rex_tm() {
  if command -v timeout >/dev/null 2>&1; then timeout "$@"; else shift; "$@"; fi
}
rex_me_uid=$(id -u 2>/dev/null || echo 0)
rex_user="${OLO_REX_USER:-${SUDO_USER:-}}"
[ "$rex_user" = "root" ] && rex_user=""
if [ "$rex_me_uid" != "0" ] && [ -z "$rex_user" ]; then
  rex_user=$(id -un 2>/dev/null || true)
fi
rex_uid=""; rex_home=""
if [ -n "$rex_user" ]; then
  case "$rex_user" in
    *[!A-Za-z0-9_.-]*) rex_user="" ;;
    *)
      if [ "$rex_me_uid" != "0" ] && [ "$(id -un 2>/dev/null || true)" = "$rex_user" ]; then
        rex_uid="$rex_me_uid"
        rex_home="${HOME:-}"
      else
        rex_uid=$(id -u "$rex_user" 2>/dev/null || true)
        if command -v getent >/dev/null 2>&1; then
          rex_home=$(getent passwd "$rex_user" 2>/dev/null | awk -F: 'NR==1 { print $6 }' 2>/dev/null || true)
        fi
        if [ -z "$rex_home" ] && [ -r /etc/passwd ]; then
          rex_home=$(awk -F: -v u="$rex_user" '$1 == u { print $6; exit }' /etc/passwd 2>/dev/null || true)
        fi
      fi
      ;;
  esac
  case "$rex_uid" in ''|*[!0-9]*|0) rex_uid="" ;; esac
  case "$rex_home" in /*) ;; *) rex_home="" ;; esac
fi
rex_run_user() {
  if [ "$rex_me_uid" != "0" ]; then
    _d="${XDG_RUNTIME_DIR:-/run/user/$rex_uid}"
    _b="${DBUS_SESSION_BUS_ADDRESS:-unix:path=$_d/bus}"
    rex_tm 30 env "XDG_RUNTIME_DIR=$_d" "DBUS_SESSION_BUS_ADDRESS=$_b" "$@"
  else
    _d="/run/user/$rex_uid"
    rex_tm 30 runuser -u "$rex_user" -- env -u XDG_CONFIG_HOME "HOME=$rex_home" "USER=$rex_user" "LOGNAME=$rex_user" "XDG_RUNTIME_DIR=$_d" "DBUS_SESSION_BUS_ADDRESS=unix:path=$_d/bus" "$@"
  fi
}
rex_mode=""
case "${OLO_REX_MODE:-}" in shared|user|skip) rex_mode="$OLO_REX_MODE" ;; esac
if [ "$rex_mode" != "skip" ]; then
if [ -z "$rex_mode" ]; then
  if [ "$rex_me_uid" != "0" ]; then
    rex_mode=user
  elif command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
    if systemctl is-active --quiet rex.service 2>/dev/null || systemctl is-active --quiet rex.socket 2>/dev/null; then
      rex_mode=shared
    elif systemctl is-active --quiet rex-login-server.service 2>/dev/null || systemctl is-active --quiet rex-login-server.socket 2>/dev/null; then
      rex_mode=user
    elif [ -n "$rex_uid" ] && [ -n "$rex_home" ] && [ -S "/run/user/$rex_uid/bus" ] && command -v runuser >/dev/null 2>&1 \
         && { rex_run_user systemctl --user is-active --quiet rex.service 2>/dev/null || rex_run_user systemctl --user is-active --quiet rex.socket 2>/dev/null; }; then
      rex_mode=user
    elif systemctl is-enabled --quiet rex.service 2>/dev/null || systemctl is-enabled --quiet rex.socket 2>/dev/null; then
      rex_mode=shared
    else
      rex_mode=user
    fi
  else
    rex_mode=user
  fi
fi
printf 'rex.v1.mode=%s\n' "$rex_mode"
[ -n "$rex_user" ] && printf 'rex.v1.user=%s\n' "$rex_user"
rex_inspect_sh=$(cat <<'__OLO_INSPECT__'
"##,
    rex_inspect_script!(),
    r##"__OLO_INSPECT__
)
rex_emit_scope() {
  _sc=$1
  awk -v sc="$_sc" '{ i = index($0, "="); if (i>1) printf "rex.v1.%s.%s=%s\n", substr($0, 1, i - 1), sc, substr($0, i + 1) }'
}
rex_is_olo_hdr() {
  [ -f "$1" ] && [ ! -L "$1" ] || return 1
  IFS= read -r _h < "$1" 2>/dev/null || return 1
  [ "$_h" = "$OLO_HEAD" ]
}
rex_kept_user_sh=$(cat <<'__OLO_KEPT_USR__'
olo_head=$1
_cfg="${XDG_CONFIG_HOME:-${HOME:-}/.config}"
case "$_cfg" in /*) ;; *) printf 'kept=0\n'; exit 0 ;; esac
_d1="$_cfg/systemd"; _d2="$_d1/user"; _dir="$_d2/rex.service.d"
_f="$_dir/90-olo-limits.conf"
printf 'file=%s\n' "$_f"
for _p in "$_d1" "$_d2" "$_dir"; do
  if [ -L "$_p" ]; then printf 'kept=0\n'; exit 0; fi
done
if [ -f "$_f" ] && [ ! -L "$_f" ]; then
  IFS= read -r _h < "$_f" 2>/dev/null || _h=""
  if [ "$_h" = "$olo_head" ]; then printf 'kept=1\n'; exit 0; fi
fi
printf 'kept=0\n'
__OLO_KEPT_USR__
)
if ! command -v systemctl >/dev/null 2>&1 || [ ! -d /run/systemd/system ]; then
  if [ "$rex_mode" = "shared" ]; then
    printf '%s\n' 'rex.v1.kept.system=0' 'rex.v1.status.system=blocked' 'rex.v1.why.system=This host isn’t using systemd for Rex’s service.'
  else
    printf '%s\n' 'rex.v1.kept.user=0' 'rex.v1.status.user=blocked' 'rex.v1.why.user=This host isn’t using systemd for Rex’s service.'
  fi
elif [ "$rex_mode" = "shared" ]; then
  sh -c "$rex_inspect_sh" sh system "$OLO_HEAD" "$SYS_DROPIN" 2>/dev/null | rex_emit_scope system
  if [ "$rex_me_uid" != "0" ]; then
    _ku=$(rex_run_user sh -c "$rex_kept_user_sh" sh "$OLO_HEAD" 2>/dev/null) \
      && printf '%s\n' "$_ku" | rex_emit_scope user || printf 'rex.v1.kept.user=0\n'
  elif [ -n "$rex_user" ] && [ -n "$rex_uid" ] && [ -n "$rex_home" ] && command -v runuser >/dev/null 2>&1; then
    _ku=$(rex_run_user sh -c "$rex_kept_user_sh" sh "$OLO_HEAD" 2>/dev/null) \
      && printf '%s\n' "$_ku" | rex_emit_scope user || printf 'rex.v1.kept.user=0\n'
  else
    printf 'rex.v1.kept.user=0\n'
  fi
else
  _sys_dir=${SYS_DROPIN%/*}
  if [ ! -L "$_sys_dir" ] && rex_is_olo_hdr "$SYS_DROPIN"; then
    printf 'rex.v1.kept.system=1\n'
  else
    printf 'rex.v1.kept.system=0\n'
  fi
  if [ "$rex_me_uid" != "0" ]; then
    _uo=$(rex_run_user sh -c 'if [ ! -S "${XDG_RUNTIME_DIR:-}/bus" ]; then sh -c "$1" sh "$2"; printf "status=blocked\nwhy=Your systemd user manager isn’t running.\n"; else sh -c "$3" sh user "$2" "$4"; fi' sh "$rex_kept_user_sh" "$OLO_HEAD" "$rex_inspect_sh" "$SYS_DROPIN" 2>/dev/null) \
      && printf '%s\n' "$_uo" | rex_emit_scope user \
      || printf '%s\n' 'rex.v1.kept.user=0' 'rex.v1.status.user=blocked' 'rex.v1.why.user=Couldn’t ask your systemd user manager.'
  elif [ -z "$rex_user" ]; then
    printf '%s\n' 'rex.v1.kept.user=0' 'rex.v1.status.user=blocked' 'rex.v1.why.user=Connect as your account (or use sudo) to check your Rex user service.'
  elif ! command -v runuser >/dev/null 2>&1; then
    printf 'rex.v1.kept.user=0\nrex.v1.status.user=blocked\nrex.v1.why.user=Olo can’t ask systemd as %s on this host (runuser isn’t installed).\n' "$rex_user"
  elif [ -z "$rex_uid" ] || [ -z "$rex_home" ]; then
    printf 'rex.v1.kept.user=0\nrex.v1.status.user=blocked\nrex.v1.why.user=Couldn’t find %s’s home directory on this host.\n' "$rex_user"
  else
    _uo=$(rex_run_user sh -c 'if [ ! -S "${XDG_RUNTIME_DIR:-}/bus" ]; then sh -c "$1" sh "$2"; printf "status=blocked\nwhy=%s’s systemd user manager isn’t running.\n" "$5"; else sh -c "$3" sh user "$2" "$4"; fi' sh "$rex_kept_user_sh" "$OLO_HEAD" "$rex_inspect_sh" "$SYS_DROPIN" "$rex_user" 2>/dev/null) \
      && printf '%s\n' "$_uo" | rex_emit_scope user \
      || printf 'rex.v1.kept.user=0\nrex.v1.status.user=blocked\nrex.v1.why.user=Couldn’t ask %s’s systemd user manager.\n' "$rex_user"
  fi
fi
fi
true
"##
);

#[cfg(test)]
mod tests {
    use super::*;

    /// A machine as it comes (Fedora's own numbers, 64 GB of memory).
    const AS_IT_COMES: &str = "vm.dirty_background_ratio 10\nvm.dirty_ratio 20\nram 65536000\nMaxSessions 10\nopen files 524288\nDefaultLimitNOFILE 524288\nDefaultTasksMax 76000\nfs.file-max 9223372036854775807\nfs.inotify.max_user_watches 524288\n\
        fs.inotify.max_user_instances 128\nfs.inotify.max_queued_events 16384\nfs.aio-max-nr 65536\nkernel.pid_max 4194304\nvm.max_map_count 1048576\nvm.dirty_background_bytes 0\nvm.dirty_bytes 0\n\
        vm.dirty_expire_centisecs 3000\nvm.swappiness 60\nvm.vfs_cache_pressure 100\n\
        rex.v1.mode=user\nrex.v1.user=alice\nrex.v1.kept.system=0\nrex.v1.kept.user=0\nrex.v1.status.user=ok\nrex.v1.unit.user=/usr/bin/rex server run --log-format text\nrex.v1.loaded.user=/usr/bin/rex server run --log-format text\nrex.v1.stale.user=0\nrex.v1.running.user=none\n";

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
            vm.dirty_bytes 33554432000\nvm.dirty_expire_centisecs 6000\nvm.swappiness 10\nvm.vfs_cache_pressure 50\ntemporary files 1\n\
            rex.v1.mode=user\nrex.v1.user=alice\nrex.v1.kept.system=0\nrex.v1.kept.user=1\nrex.v1.status.user=ok\nrex.v1.unit.user=/usr/bin/rex server run --log-format text\nrex.v1.loaded.user=/usr/bin/rex server run --log-format text --max-windows 512 --max-panes 128 --max-sessions 128 --max-blocks 4096\nrex.v1.stale.user=0\nrex.v1.running.user=none\n\
            default shell /usr/local/bin/brush\nbrush there\nbrush 1\nkept sessions\nkept files\nkept kernel\nkept writeback\nkept memory\nkept tmpdir\nkept shell\n";
        let t = Tuning::read(ready);
        assert_eq!(t.short(), Vec::<Tune>::new(), "{:?}", t.stands.iter().flat_map(|s| s.values.iter().filter(|v| !v.met())).collect::<Vec<_>>());
        assert!(t.stands.iter().all(|s| s.kept) && Tune::ALL.iter().all(|tune| tune.apply(&t).is_none()));
        // A host that says nothing of a setting isn't taken to have it.
        let silent = Tuning::read("ram 1000\nMaxSessions \n");
        assert_eq!((silent.of(Tune::Sessions).values[0].now, silent.of(Tune::Sessions).met()), (None, false));
        assert_eq!((silent.of(Tune::RexLimits).values[0].now, silent.of(Tune::RexLimits).met()), (None, false));
    }

    /// What is asked and what is applied are scripts a shell takes
    /// (each parsed by `sh -n`), and a kernel setting taken back is put
    /// to what the file noted it was: tried on a file of the test's own.
    #[test]
    fn the_scripts_are_ones_a_shell_takes_and_undo_reads_what_was() {
        let t = Tuning::read(AS_IT_COMES);
        let parses = |script: &str| std::process::Command::new("sh").args(["-n", "-c", script]).status().unwrap().success();
        assert!(LOOK.contains(OLO_HEAD), "LOOK and OLO_HEAD must match");
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

    /// Strict Rust parsing for [`RexLimitValues`] and [`Tuning::read`]:
    /// covers every review case (quoted flag, argument with a space that
    /// looks like a flag, huge 64-bit integer and overflow, malformed
    /// records, bidirectional pending restart, and blocked rows).
    #[test]
    fn rex_server_limits_strict_rust_parsing_and_records() {
        use base64::{Engine as _, engine::general_purpose::STANDARD as B64};

        // Plain ExecStart parser: defaults, space and = forms, last flag wins, 0 = no limit.
        assert_eq!(RexLimitValues::parse_plain_execstart("/usr/bin/rex server run --log-format text"), Some(RexLimitValues::defaults()));
        assert_eq!(RexLimitValues::parse_plain_execstart("/usr/bin/rex server run --max-windows 512 --max-blocks=4096"), Some(RexLimitValues { windows: 512, panes: 64, sessions: 64, blocks: 4096 }));
        assert_eq!(
            RexLimitValues::parse_plain_execstart("/usr/bin/rex server run --max-windows 128 --max-windows=0 --max-panes 256"),
            Some(RexLimitValues { windows: u64::MAX, panes: 256, sessions: 64, blocks: 1024 })
        );

        // Review case 1: quoted flags MUST be refused (never misread as 64 and lowered).
        assert_eq!(RexLimitValues::parse_plain_execstart("/usr/bin/rex server run \"--max-windows=0\""), None);
        assert_eq!(RexLimitValues::parse_plain_execstart("/usr/bin/rex server run --max-panes \"256\""), None);
        assert_eq!(RexLimitValues::parse_plain_execstart("/usr/bin/rex server run '--max-windows=0'"), None);
        assert_eq!(RexLimitValues::parse_proc_cmdline(b"/usr/bin/rex\0server\0run\0--max-windows\0\"0\"\0"), None);
        // Specifiers, env vars, semicolons, backslashes, prefixes, shell wrappers, bare `--` all refused.
        for bad in [
            "/usr/bin/rex server run --state-dir %t/rex",
            "/usr/bin/rex server run $EXTRA_ARGS",
            "/usr/bin/rex server run ; /bin/true",
            "/usr/bin/rex server run \\ --max-windows 512",
            "-/usr/bin/rex server run",
            "@/usr/bin/rex rex0 server run",
            "/bin/sh -c /usr/bin/rex",
            "/usr/bin/rex server run -- --max-windows 512",
            "/usr/bin/rex server run --max-windows abc",
            "/usr/bin/rex server run --max-windows",
            "/usr/bin/../bin/rex server run",
        ] {
            assert_eq!(RexLimitValues::parse_plain_execstart(bad), None, "must reject: {bad}");
        }

        // Review case 2: argument containing spaces that looks like a flag in NUL-delimited /proc/PID/cmdline.
        let space_arg = b"/usr/bin/rex\0server\0run\0--state-dir\0/srv/archive --max-windows 4096\0";
        assert_eq!(RexLimitValues::parse_proc_cmdline(space_arg), Some(RexLimitValues::defaults()));
        // Value of a known value-taking flag that itself equals a limit flag is not parsed as a limit.
        let tricky_arg = b"/usr/bin/rex\0server\0run\0--state-dir\0--max-windows=4096\0";
        assert_eq!(RexLimitValues::parse_proc_cmdline(tricky_arg), Some(RexLimitValues::defaults()));
        // Even with a custom argv[0] (from `@` prefix), NUL-split cmdline still parses real flags.
        let custom_argv0 = b"rex-custom\0server\0run\0--max-windows\0512\0--max-panes=128\0--max-sessions\0128\0--max-blocks=4096\0";
        assert_eq!(RexLimitValues::parse_proc_cmdline(custom_argv0), Some(RexLimitValues::wanted()));

        // Review case 3: huge integer (> 2^53) preserved exactly without double rounding; overflow rejected.
        assert_eq!(
            RexLimitValues::parse_plain_execstart("/usr/bin/rex server run --max-windows 9007199254740993"),
            Some(RexLimitValues { windows: 9_007_199_254_740_993, ..RexLimitValues::defaults() })
        );
        assert_eq!(RexLimitValues::parse_plain_execstart("/usr/bin/rex server run --max-windows 99999999999999999999999999"), None);
        assert_eq!(RexLimitValues::parse_plain_execstart("/usr/bin/rex server run --max-windows 18446744073709551615"), None);

        // Review case 4: malformed records treated as unknown/blocked; loose `kept rex` ignored.
        let loose_kept = Tuning::read("ram 65536000\nkept rex\n");
        assert!(!loose_kept.of(Tune::RexLimits).kept, "loose text must never mark RexLimits as kept");
        let malformed_status = Tuning::read("ram 65536000\nrex.v1.mode=shared\nrex.v1.status.system=bogus\n");
        assert!(malformed_status.rex_limits.blocked.is_some());
        assert_eq!(malformed_status.of(Tune::RexLimits).values[0].now, None);
        assert!(Tune::RexLimits.apply(&malformed_status).is_none());
        let malformed_cmd = Tuning::read(
            "ram 65536000\nrex.v1.mode=shared\nrex.v1.kept.system=0\nrex.v1.kept.user=0\nrex.v1.status.system=ok\n\
             rex.v1.unit.system=/usr/bin/rex server run \"--max-windows=0\"\nrex.v1.loaded.system=/usr/bin/rex server run \"--max-windows=0\"\nrex.v1.stale.system=0\n",
        );
        assert!(malformed_cmd.rex_limits.blocked.is_some());
        assert_eq!(malformed_cmd.of(Tune::RexLimits).values[0].now, None);
        let malformed_run = Tuning::read(
            "ram 65536000\nrex.v1.mode=shared\nrex.v1.kept.system=0\nrex.v1.kept.user=0\nrex.v1.status.system=ok\n\
             rex.v1.unit.system=/usr/bin/rex server run\nrex.v1.loaded.system=/usr/bin/rex server run\nrex.v1.stale.system=0\nrex.v1.running.system=b64:!!!not-base64!!!\n",
        );
        assert_eq!(malformed_run.rex_limits.running, None);

        // Pending restart works in BOTH directions: after Apply (64 -> 512) AND after Put Back (512 -> 64).
        let run_def = B64.encode(b"/usr/bin/rex\0server\0run\0--log-format\0text\0");
        let run_hi = B64.encode(b"/usr/bin/rex\0server\0run\0--log-format\0text\0--max-windows\0512\0--max-panes\0128\0--max-sessions\0128\0--max-blocks\04096\0");
        let applied_pending = Tuning::read(&format!(
            "ram 65536000\nrex.v1.mode=shared\nrex.v1.kept.system=1\nrex.v1.kept.user=0\nrex.v1.status.system=ok\n\
             rex.v1.unit.system=/usr/bin/rex server run --log-format text\n\
             rex.v1.loaded.system=/usr/bin/rex server run --log-format text --max-windows 512 --max-panes 128 --max-sessions 128 --max-blocks 4096\n\
             rex.v1.stale.system=0\nrex.v1.running.system=b64:{run_def}\n"
        ));
        assert!(applied_pending.of(Tune::RexLimits).met() && applied_pending.of(Tune::RexLimits).kept && !applied_pending.rex_limits.stale);
        assert_eq!(applied_pending.rex_limits.scope, RexScope::Shared);
        assert_eq!(applied_pending.rex_limits_pending_restart(), Some((RexLimitValues::defaults(), RexLimitValues::wanted())));
        assert_eq!(applied_pending.rex_limits.without_olo, Some(RexLimitValues::defaults()));

        let put_back_pending = Tuning::read(&format!(
            "ram 65536000\nrex.v1.mode=shared\nrex.v1.kept.system=0\nrex.v1.kept.user=0\nrex.v1.status.system=ok\n\
             rex.v1.unit.system=/usr/bin/rex server run --log-format text\n\
             rex.v1.loaded.system=/usr/bin/rex server run --log-format text\n\
             rex.v1.stale.system=0\nrex.v1.running.system=b64:{run_hi}\n"
        ));
        assert!(!put_back_pending.of(Tune::RexLimits).met() && !put_back_pending.of(Tune::RexLimits).kept);
        assert_eq!(put_back_pending.rex_limits_pending_restart(), Some((RexLimitValues::wanted(), RexLimitValues::defaults())));

        // Blocked row: excluded from short(), apply() returns None, keeps kept=true when Olo's file is present.
        let blocked_kept = Tuning::read(
            "ram 65536000\nrex.v1.mode=shared\nrex.v1.kept.system=1\nrex.v1.kept.user=0\nrex.v1.status.system=blocked\n\
             rex.v1.why.system=/etc/systemd/system/rex.service.d/20-site.conf sets this server’s command: raise its limits there.\n",
        );
        assert_eq!(blocked_kept.rex_limits.blocked.as_deref(), Some("/etc/systemd/system/rex.service.d/20-site.conf sets this server’s command: raise its limits there."));
        assert!(blocked_kept.of(Tune::RexLimits).kept);
        assert!(!blocked_kept.short().contains(&Tune::RexLimits));
        assert!(Tune::RexLimits.apply(&blocked_kept).is_none());

        // Plain-word flag descriptions match every Rex limit flag.
        assert_eq!(rex_limit_note("--max-windows"), Some("tabs in a session"));
        assert_eq!(rex_limit_note("--max-panes"), Some("panes in a tab"));
        assert_eq!(rex_limit_note("--max-sessions"), Some("sessions"));
        assert_eq!(rex_limit_note("--max-blocks"), Some("blocks in all"));
        assert_eq!(rex_limit_note("MaxSessions"), None);

        // Round 4 Point 2: LOOK and REX_APPLY embed the exact same inspection script text whole.
        assert!(!REX_INSPECT.is_empty());
        assert!(LOOK.contains(REX_INSPECT), "LOOK must contain the exact shared REX_INSPECT text");
        assert!(REX_APPLY.contains(REX_INSPECT), "REX_APPLY must contain the exact shared REX_INSPECT text");
        assert!(!REX_APPLY.contains("/etc/systemd/user/rex.service.d"), "REX_APPLY must never touch /etc/systemd/user");
        assert!(!REX_UNDO.contains("/etc/systemd/user/rex.service.d"), "REX_UNDO must never touch /etc/systemd/user");
    }

    /// Finding 19: an answer without a complete, consistent `rex.v1` record
    /// for the active scope (none at all, bad mode, missing kept bit, or two
    /// different values for one field) is NEVER a yes: marks the row blocked
    /// ("Olo couldn’t read this host’s Rex service."), clears kept (no Put
    /// Back), excludes it from short() / Apply Suggested, and returns None
    /// from apply().
    #[test]
    fn missing_or_contradictory_rex_records_block_apply_and_put_back() {
        let base = "ram 65536000\nMaxSessions 1024\n";
        let cases: &[(&str, String)] = &[
            ("no-rex-records", base.to_owned()),
            (
                "bad-mode-with-kept-user",
                format!("{base}rex.v1.mode=banana\nrex.v1.kept.user=1\nrex.v1.status.user=ok\nrex.v1.unit.user=/usr/bin/rex server run\nrex.v1.loaded.user=/usr/bin/rex server run\n"),
            ),
            ("missing-kept-for-scope", format!("{base}rex.v1.mode=shared\nrex.v1.status.system=ok\nrex.v1.unit.system=/usr/bin/rex server run\nrex.v1.loaded.system=/usr/bin/rex server run\n")),
            (
                "malformed-kept-value",
                format!("{base}rex.v1.mode=shared\nrex.v1.kept.system=maybe\nrex.v1.status.system=ok\nrex.v1.unit.system=/usr/bin/rex server run\nrex.v1.loaded.system=/usr/bin/rex server run\n"),
            ),
            (
                "contradictory-duplicate-kept",
                format!(
                    "{base}rex.v1.mode=shared\nrex.v1.kept.system=1\nrex.v1.kept.system=0\nrex.v1.kept.user=0\nrex.v1.status.system=ok\n\
                     rex.v1.unit.system=/usr/bin/rex server run\nrex.v1.loaded.system=/usr/bin/rex server run\n"
                ),
            ),
            (
                "contradictory-duplicate-mode",
                format!(
                    "{base}rex.v1.mode=shared\nrex.v1.mode=user\nrex.v1.kept.system=1\nrex.v1.kept.user=1\nrex.v1.status.user=ok\n\
                     rex.v1.unit.user=/usr/bin/rex server run\nrex.v1.loaded.user=/usr/bin/rex server run\n"
                ),
            ),
            (
                "contradictory-duplicate-unit",
                format!(
                    "{base}rex.v1.mode=shared\nrex.v1.kept.system=1\nrex.v1.kept.user=0\nrex.v1.status.system=ok\n\
                     rex.v1.unit.system=/usr/bin/rex server run\nrex.v1.unit.system=/usr/bin/rex server run --max-windows 512\n\
                     rex.v1.loaded.system=/usr/bin/rex server run\n"
                ),
            ),
            ("missing-status-for-active-scope", format!("{base}rex.v1.mode=shared\nrex.v1.kept.system=1\nrex.v1.status.user=ok\n")),
        ];
        for (name, raw) in cases {
            let t = Tuning::read(raw);
            assert_eq!(t.rex_limits.blocked.as_deref(), Some("Olo couldn’t read this host’s Rex service."), "{name}: {:?}", t.rex_limits);
            assert!(!t.of(Tune::RexLimits).kept, "{name}: must not offer Put Back");
            assert!(!t.rex_limits.kept_sys && !t.rex_limits.kept_usr, "{name}: ownership flags must be cleared");
            assert!(!t.short().contains(&Tune::RexLimits), "{name}: must not enter Apply Suggested");
            assert!(Tune::RexLimits.apply(&t).is_none(), "{name}: apply() must be None");
        }
    }

    struct RexFixture {
        root: std::path::PathBuf,
        _bus: std::os::unix::net::UnixListener,
    }

    impl RexFixture {
        fn new(base: &std::path::Path, tag: &str) -> Self {
            use std::os::unix::fs::PermissionsExt as _;
            use std::os::unix::net::UnixListener;
            let root = base.join(tag);
            let _ = std::fs::remove_dir_all(&root);
            for d in [
                "bin",
                "state",
                "usr/bin",
                "usr/lib/systemd/system",
                "usr/lib/systemd/user",
                "etc/systemd/system/rex.service.d",
                "etc/systemd/user/rex.service.d",
                "run/systemd/system",
                "run/user/1000",
                "home/alice/.config/systemd/user/rex.service.d",
            ] {
                std::fs::create_dir_all(root.join(d)).unwrap();
            }
            let bus = UnixListener::bind(root.join("run/user/1000/bus")).unwrap();
            let rex_bin = root.join("usr/bin/rex");
            std::fs::write(&rex_bin, "#!/bin/sh\necho 'Options:'\necho '  --max-windows N'\necho '  --max-panes N'\necho '  --max-sessions N'\necho '  --max-blocks N'\n").unwrap();
            std::fs::set_permissions(&rex_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

            // Stub id: defaults to uid 0 (`root`) unless OLO_TEST_NONROOT=1 (uid 1000 `alice`).
            let id_bin = root.join("bin/id");
            std::fs::write(
                &id_bin,
                "#!/bin/sh\nif [ \"$1\" = \"-un\" ]; then echo alice; exit 0; fi\n\
                 if [ \"$1\" = \"-u\" ] && [ -z \"$2\" ]; then if [ \"${OLO_TEST_NONROOT:-0}\" = \"1\" ]; then echo 1000; else echo 0; fi; exit 0; fi\n\
                 if [ \"$1\" = \"-u\" ] && [ \"$2\" = \"alice\" ]; then echo 1000; exit 0; fi\n\
                 if [ \"$1\" = \"-u\" ] && [ \"$2\" = \"build\" ]; then echo 1001; exit 0; fi\n\
                 if [ \"$1\" = \"-u\" ] && [ \"$2\" = \"rex\" ]; then echo 999; exit 0; fi\n\
                 if [ \"$1\" = \"-u\" ] && [ \"$2\" = \"nobody\" ]; then echo 65534; exit 0; fi\n\
                 exec /usr/bin/id \"$@\"\n",
            )
            .unwrap();
            std::fs::set_permissions(&id_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

            // Stub getent: returns fixture passwd entries for alice and build.
            let getent_bin = root.join("bin/getent");
            std::fs::write(
                &getent_bin,
                format!(
                    "#!/bin/sh\nif [ \"$1\" = \"passwd\" ] && [ \"$2\" = \"alice\" ]; then echo 'alice:x:1000:1000::{r}/home/alice:/bin/bash'; exit 0; fi\n\
                     if [ \"$1\" = \"passwd\" ] && {{ [ \"$2\" = \"build\" ] || [ \"$2\" = \"1001\" ]; }}; then echo 'build:x:1001:1001::{r}/home/build:/bin/bash'; exit 0; fi\n\
                     exit 2\n",
                    r = root.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&getent_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

            // Stub runuser: strips `-u <user> --`, records identity in OLO_TEST_RUNUSER_AS,
            // and honors `state/deny_x_<user>` when checking `test -x`.
            let ru_bin = root.join("bin/runuser");
            std::fs::write(
                &ru_bin,
                format!(
                    "#!/bin/sh\nSTATE=\"{r}/state\"\n_u=\"\"\nif [ \"$1\" = \"-u\" ]; then _u=\"$2\"; shift 2; fi\nif [ \"$1\" = \"--\" ]; then shift; fi\n\
                     if [ -n \"$_u\" ] && [ \"$1\" = \"test\" ] && [ \"$2\" = \"-x\" ] && [ -f \"$STATE/deny_x_$_u\" ]; then exit 1; fi\n\
                     export OLO_TEST_RUNUSER_AS=\"$_u\"\nif [ -n \"$_u\" ] && [ \"$_u\" != \"root\" ]; then export OLO_TEST_NONROOT=1; fi\nexec \"$@\"\n",
                    r = root.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&ru_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

            // Stub systemctl: simulates LoadState, NeedDaemonReload, User, FragmentPath, DropInPaths,
            // ExecStart, ExecStartEx, MainPID, daemon-reload, and injectable failures.
            let sc_bin = root.join("bin/systemctl");
            let sc_script = format!(
                r#"#!/bin/sh
STATE="{root}/state"
scope=sys
if [ "$1" = "--user" ]; then scope=usr; shift; fi
cmd=$1; shift
if [ "$cmd" = "is-active" ] || [ "$cmd" = "is-enabled" ]; then
  if [ "$1" = "--quiet" ]; then shift; fi
  mode=$(cat "$STATE/mode" 2>/dev/null || echo shared)
  if [ "$scope" = "sys" ] && [ "$mode" = "shared" ] && [ "$1" = "rex.service" ]; then exit 0; fi
  if [ "$scope" = "usr" ] && [ "$mode" = "user" ] && [ "$1" = "rex.service" ]; then exit 0; fi
  exit 1
fi
if [ "$cmd" = "daemon-reload" ]; then
  [ -f "$STATE/${{scope}}_reload_fail" ] && exit 1
  printf '%s\n' "$scope" >> "$STATE/reloads"
  if [ "$scope" = "sys" ]; then
    drop="{root}/etc/systemd/system/rex.service.d/90-olo-limits.conf"
  else
    _cfg="${{XDG_CONFIG_HOME:-${{HOME:-{root}/home/alice}}/.config}}"
    drop="$_cfg/systemd/user/rex.service.d/90-olo-limits.conf"
  fi
  if [ -f "$drop" ]; then
    ex=$(awk '/^ExecStart=/ {{ v=substr($0,11); if (v!="") res=v }} END {{ print res }}' "$drop")
    if [ -f "$STATE/${{scope}}_after_reload_exec" ]; then
      cat "$STATE/${{scope}}_after_reload_exec" > "$STATE/${{scope}}_loaded_exec"
    else
      printf '%s\n' "$ex" > "$STATE/${{scope}}_loaded_exec"
    fi
    printf '%s\n' "$drop" > "$STATE/${{scope}}_loaded_olo"
  else
    rm -f "$STATE/${{scope}}_loaded_exec" "$STATE/${{scope}}_loaded_olo"
  fi
  exit 0
fi
if [ "$cmd" = "show" ]; then
  if [ "$scope" = "sys" ]; then
    frag_def="{root}/usr/lib/systemd/system/rex.service"
  else
    frag_def="{root}/usr/lib/systemd/user/rex.service"
  fi
  frag=$(cat "$STATE/${{scope}}_frag" 2>/dev/null || echo "$frag_def")
  drops=$(cat "$STATE/${{scope}}_dropins" 2>/dev/null || true)
  olo=$(cat "$STATE/${{scope}}_loaded_olo" 2>/dev/null || true)
  if [ -n "$olo" ]; then drops="${{drops:+$drops }}$olo"; fi
  need_reload=$(cat "$STATE/${{scope}}_need_reload" 2>/dev/null || echo no)
  unit_user=$(cat "$STATE/${{scope}}_user" 2>/dev/null || echo rex)
  if [ -f "$STATE/${{scope}}_raw_exec" ]; then
    ls=$(cat "$STATE/${{scope}}_load_state" 2>/dev/null || echo loaded)
    printf 'LoadState=%s\nNeedDaemonReload=%s\nUser=%s\nFragmentPath=%s\nDropInPaths=%s\nMainPID=0\n' "$ls" "$need_reload" "$unit_user" "$frag" "$drops"
    cat "$STATE/${{scope}}_raw_exec"
  else
    if [ -f "$STATE/${{scope}}_loaded_exec" ]; then
      ex=$(cat "$STATE/${{scope}}_loaded_exec")
    elif [ -f "$STATE/${{scope}}_exec" ]; then
      ex=$(cat "$STATE/${{scope}}_exec")
    else
      ex=$(awk '/^ExecStart=/ {{ v=substr($0,11); if (v!="") res=v }} END {{ print res }}' "$frag" 2>/dev/null)
    fi
    ls=$(cat "$STATE/${{scope}}_load_state" 2>/dev/null || echo loaded)
    printf 'LoadState=%s\nNeedDaemonReload=%s\nUser=%s\nFragmentPath=%s\nDropInPaths=%s\nMainPID=0\n' "$ls" "$need_reload" "$unit_user" "$frag" "$drops"
    if [ -n "$ex" ]; then
      p=${{ex%% *}}
      printf 'ExecStart={{ path=%s ; argv[]=%s ; ignore_errors=no ; start_time=[n/a] ; stop_time=[n/a] ; pid=0 ; code=(null) ; status=0/0 }}\n' "$p" "$ex"
      printf 'ExecStartEx={{ path=%s ; argv[]=%s ; flags= ; start_time=[n/a] ; stop_time=[n/a] ; pid=0 ; code=(null) ; status=0/0 }}\n' "$p" "$ex"
    fi
  fi
  exit 0
fi
exit 1
"#,
                root = root.display()
            );
            std::fs::write(&sc_bin, sc_script).unwrap();
            std::fs::set_permissions(&sc_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

            let f = Self { root, _bus: bus };
            let sys_cmd = format!("{}/usr/bin/rex server run --listen /run/rex/server.sock --state-dir /var/lib/rex --socket-mode 0660 --log-format text", f.root.display());
            let usr_cmd = format!("{}/usr/bin/rex server run --log-format text", f.root.display());
            f.write_state("mode", "shared");
            std::fs::write(f.root.join("usr/lib/systemd/system/rex.service"), format!("[Unit]\nDescription=Rex\n[Service]\nExecStart={sys_cmd}\n")).unwrap();
            std::fs::write(f.root.join("usr/lib/systemd/user/rex.service"), format!("[Unit]\nDescription=Rex User\n[Service]\nExecStart={usr_cmd}\n")).unwrap();
            f
        }

        fn write_state(&self, name: &str, content: &str) {
            std::fs::write(self.root.join("state").join(name), content).unwrap();
        }

        fn rewrite(&self, script: &str) -> String {
            let r = self.root.display().to_string();
            script
                .replace(" /lib/systemd/", &format!(" {r}/lib/systemd/"))
                .replace("|/lib/systemd/", &format!("|{r}/lib/systemd/"))
                .replace("/usr/lib/systemd/", &format!("{r}/usr/lib/systemd/"))
                .replace("/usr/local/lib/systemd/", &format!("{r}/usr/local/lib/systemd/"))
                .replace("/etc/systemd/", &format!("{r}/etc/systemd/"))
                .replace("/run/systemd/", &format!("{r}/run/systemd/"))
                .replace("/run/user/", &format!("{r}/run/user/"))
                .replace("_ld=\"/run\"", &format!("_ld=\"{r}/state\""))
        }

        fn run_sh(&self, script: &str, envs: &[(&str, &str)]) -> (bool, String, String) {
            let path = format!("{}/bin:{}", self.root.display(), std::env::var("PATH").unwrap_or_default());
            let mut cmd = std::process::Command::new("sh");
            cmd.args(["-c", &self.rewrite(script)]).env("PATH", path).env("HOME", self.root.join("home/alice")).env("XDG_RUNTIME_DIR", self.root.join("run/user/1000")).env_remove("XDG_CONFIG_HOME");
            for (k, v) in envs {
                cmd.env(k, v);
            }
            let out = cmd.output().unwrap();
            (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
        }

        fn rex_apply_script(&self, mode: &str, user: &str) -> String {
            REX_APPLY
                .replace("@HEAD@", &shell_word(OLO_HEAD))
                .replace("@MODE@", mode)
                .replace("@USER@", &shell_word(user))
                .replace("@WINDOWS@", &REX_WINDOWS.to_string())
                .replace("@PANES@", &REX_PANES.to_string())
                .replace("@SESSIONS@", &REX_SESSIONS.to_string())
                .replace("@BLOCKS@", &REX_BLOCKS.to_string())
                .replace("@SYS_DROPIN@", REX_SYSTEM_DROPIN)
        }

        fn look_rex(&self, envs: &[(&str, &str)]) -> Tuning {
            let (_, stdout, _) = self.run_sh(LOOK, envs);
            Tuning::read(&stdout)
        }

        fn sys_dropin(&self) -> std::path::PathBuf {
            self.root.join("etc/systemd/system/rex.service.d/90-olo-limits.conf")
        }

        fn usr_dropin(&self) -> std::path::PathBuf {
            self.root.join("home/alice/.config/systemd/user/rex.service.d/90-olo-limits.conf")
        }

        fn etc_usr_dropin(&self) -> std::path::PathBuf {
            self.root.join("etc/systemd/user/rex.service.d/90-olo-limits.conf")
        }
    }

    /// Fixture-tree tests for `LOOK`, `REX_APPLY`, and `REX_UNDO`:
    /// covers every refusal of Point 2 (asserting no file written and none
    /// removed), decimal-string merge of huge integers & `0`, stale refresh
    /// keeping previously configured limits, one-scope-a-change (shared vs
    /// per-account `${XDG_CONFIG_HOME:-$HOME/.config}` both via root+runuser
    /// and directly as the account without root), symlinked account folder,
    /// foreign-file ownership checks in both scopes, no directory deletion
    /// on Undo, atomic rollback on reload or read-back failure in both
    /// scopes, and guaranteed `Rex: ` output on missing `awk`.
    #[test]
    fn rex_server_limits_scripts_refusals_and_rollback_against_fixtures() {
        type Fixture = RexFixture;
        let base_root = std::path::PathBuf::from(format!("/var/tmp/olo-rex-tune-fix-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base_root);

        // 1. One-scope-a-change (shared writes ONLY system; user writes ONLY alice's own file, works without root),
        //    huge decimal integer + 0 never lowered, stale refresh keeps configured higher limits, undo leaves dirs intact.
        {
            let f = Fixture::new(&base_root, "plain-and-huge");
            let sys_cmd = format!("{}/usr/bin/rex server run --listen /run/rex/server.sock --max-windows 9007199254740993 --max-blocks 0", f.root.display());
            std::fs::write(f.root.join("usr/lib/systemd/system/rex.service"), format!("[Service]\nExecStart={sys_cmd}\n")).unwrap();
            let t = f.look_rex(&[("OLO_REX_USER", "alice")]);
            assert_eq!(t.rex_limits.blocked, None, "{:?}", t.rex_limits);
            assert_eq!(t.of(Tune::RexLimits).values[0].now, Some(9_007_199_254_740_993));
            assert_eq!(t.of(Tune::RexLimits).values[3].now, Some(u64::MAX));
            assert!(!t.of(Tune::RexLimits).met());
            let (ok, out, err) = f.run_sh(&Tune::RexLimits.apply(&t).unwrap(), &[("OLO_REX_USER", "alice")]);
            assert!(ok, "shared apply failed: {out}\n{err}");
            let sys_txt = std::fs::read_to_string(f.sys_dropin()).unwrap();
            assert!(sys_txt.starts_with(OLO_HEAD), "{sys_txt}");
            assert!(sys_txt.contains("--max-windows 9007199254740993 --max-panes 128 --max-sessions 128 --max-blocks 0"), "huge decimal integer and 0 must be preserved verbatim: {sys_txt}");
            assert!(!f.usr_dropin().exists(), "shared apply must write ONLY the system file");
            assert!(!f.etc_usr_dropin().exists(), "must never write /etc/systemd/user");

            // Stale refresh: package drops --max-windows and --max-blocks and adds --log-level info.
            // Refreshing MUST keep 9007199254740993 and 0!
            let new_sys = format!("{}/usr/bin/rex server run --listen /run/rex/server.sock --log-level info", f.root.display());
            std::fs::write(f.root.join("usr/lib/systemd/system/rex.service"), format!("[Service]\nExecStart={new_sys}\n")).unwrap();
            let stale = f.look_rex(&[("OLO_REX_USER", "alice")]);
            assert!(stale.rex_limits.stale && stale.of(Tune::RexLimits).met() && stale.short().contains(&Tune::RexLimits), "{:?}", stale.rex_limits);
            let (ok, out, err) = f.run_sh(&Tune::RexLimits.apply(&stale).unwrap(), &[("OLO_REX_USER", "alice")]);
            assert!(ok, "stale refresh failed: {out}\n{err}");
            let refreshed_txt = std::fs::read_to_string(f.sys_dropin()).unwrap();
            assert!(refreshed_txt.contains(&format!("# packaged: {new_sys}")), "{refreshed_txt}");
            assert!(
                refreshed_txt.contains(&format!("ExecStart={new_sys} --max-windows 9007199254740993 --max-panes 128 --max-sessions 128 --max-blocks 0")),
                "stale refresh must keep configured limits: {refreshed_txt}"
            );

            // Personal (user) scope: writes ONLY alice's own ~/.config file, never /etc/systemd/user,
            // and leaves the system file untouched!
            f.write_state("mode", "user");
            let u_before = f.look_rex(&[("OLO_REX_USER", "alice"), ("OLO_REX_MODE", "user")]);
            assert_eq!(u_before.rex_limits.scope, RexScope::User);
            assert_eq!(u_before.rex_limits.file_usr.as_deref(), Some(f.usr_dropin().to_str().unwrap()));
            assert!(!u_before.of(Tune::RexLimits).kept && u_before.rex_limits.kept_sys && !u_before.rex_limits.kept_usr);
            let (ok, out, err) = f.run_sh(&Tune::RexLimits.apply(&u_before).unwrap(), &[("OLO_REX_USER", "alice")]);
            assert!(ok, "user apply failed: {out}\n{err}");
            let usr_txt = std::fs::read_to_string(f.usr_dropin()).unwrap();
            assert!(usr_txt.starts_with(OLO_HEAD) && usr_txt.contains("--max-windows 512 --max-panes 128 --max-sessions 128 --max-blocks 4096"), "{usr_txt}");
            assert!(!f.etc_usr_dropin().exists(), "user apply must NEVER write /etc/systemd/user");

            // User Put Back removes ONLY alice's file, leaves rex.service.d AND system file intact!
            let (ok, out, err) = f.run_sh(&Tune::RexLimits.undo(), &[("OLO_REX_MODE", "user"), ("OLO_REX_USER", "alice")]);
            assert!(ok, "user undo failed: {out}\n{err}");
            assert!(!f.usr_dropin().exists() && f.usr_dropin().parent().unwrap().is_dir(), "user Put Back removes only Olo file, no folder");
            assert!(f.sys_dropin().exists(), "user Put Back must NOT touch system file");

            // Non-root direct user Apply and Put Back (OLO_TEST_NONROOT=1, no runuser needed!):
            std::fs::remove_file(f.root.join("bin/runuser")).unwrap();
            let (ok, out, err) = f.run_sh(&f.rex_apply_script("user", ""), &[("OLO_TEST_NONROOT", "1")]);
            assert!(ok, "non-root user apply failed: {out}\n{err}");
            assert!(f.usr_dropin().exists() && !f.etc_usr_dropin().exists());
            let (ok, out, err) = f.run_sh(&Tune::RexLimits.undo(), &[("OLO_TEST_NONROOT", "1"), ("OLO_REX_MODE", "user")]);
            assert!(ok, "non-root user undo failed: {out}\n{err}");
            assert!(!f.usr_dropin().exists() && f.usr_dropin().parent().unwrap().is_dir());

            // Shared Put Back removes system file and leaves rex.service.d intact.
            f.write_state("mode", "shared");
            let (ok, out, err) = f.run_sh(&Tune::RexLimits.undo(), &[("OLO_REX_MODE", "shared")]);
            assert!(ok, "shared undo failed: {out}\n{err}");
            assert!(!f.sys_dropin().exists() && f.sys_dropin().parent().unwrap().is_dir(), "shared Put Back removes no folder");
        }

        // 2. Every refusal of Point 2 + Round 4 account folder symlink: LOOK blocks it, Apply refuses with plain words, NO file written and NONE removed!
        let refusal_cases: Vec<(&str, Box<dyn Fn(&Fixture)>, &str)> = vec![
            ("masked", Box::new(|f| f.write_state("sys_load_state", "masked")), "masked"),
            ("not-found", Box::new(|f| f.write_state("sys_load_state", "not-found")), "isn’t installed"),
            (
                "admin-dropin-state-dir",
                Box::new(|f| {
                    let dp = f.root.join("etc/systemd/system/rex.service.d/20-site.conf");
                    std::fs::write(&dp, format!("[Service]\nExecStart=\nExecStart={}/usr/bin/rex server run --state-dir /srv/rex\n", f.root.display())).unwrap();
                    f.write_state("sys_dropins", dp.to_str().unwrap());
                }),
                "20-site.conf sets this server’s command: raise its limits there.",
            ),
            (
                "no-execstart",
                Box::new(|f| {
                    std::fs::write(f.root.join("usr/lib/systemd/system/rex.service"), "[Unit]\nDescription=Rex\n[Service]\nType=notify\n").unwrap();
                }),
                "has no ExecStart command",
            ),
            (
                "two-execstarts",
                Box::new(|f| {
                    let bin = format!("{}/usr/bin/rex", f.root.display());
                    std::fs::write(f.root.join("usr/lib/systemd/system/rex.service"), format!("[Service]\nType=oneshot\nExecStart=/bin/true\nExecStart={bin} server run\n")).unwrap();
                }),
                "more than one ExecStart",
            ),
            (
                "continuation-with-comment",
                Box::new(|f| {
                    let bin = format!("{}/usr/bin/rex", f.root.display());
                    std::fs::write(f.root.join("usr/lib/systemd/system/rex.service"), format!("[Service]\nExecStart={bin} server run \\\n  # indented comment\n  --log-format text\n")).unwrap();
                }),
                "continuation lines",
            ),
            (
                "quoted-flag",
                Box::new(|f| {
                    let bin = format!("{}/usr/bin/rex", f.root.display());
                    std::fs::write(f.root.join("usr/lib/systemd/system/rex.service"), format!("[Service]\nExecStart={bin} server run \"--max-windows=0\"\n")).unwrap();
                }),
                "quotes",
            ),
            (
                "backslash",
                Box::new(|f| {
                    let bin = format!("{}/usr/bin/rex", f.root.display());
                    std::fs::write(f.root.join("usr/lib/systemd/system/rex.service"), format!("[Service]\nExecStart={bin} server run --state-dir /var/lib/rex\\foo\n")).unwrap();
                }),
                "backslashes",
            ),
            (
                "specifier",
                Box::new(|f| {
                    let bin = format!("{}/usr/bin/rex", f.root.display());
                    std::fs::write(f.root.join("usr/lib/systemd/system/rex.service"), format!("[Service]\nExecStart={bin} server run --listen %t/rex.sock\n")).unwrap();
                }),
                "specifiers (%)",
            ),
            (
                "env-var",
                Box::new(|f| {
                    let bin = format!("{}/usr/bin/rex", f.root.display());
                    std::fs::write(f.root.join("usr/lib/systemd/system/rex.service"), format!("[Service]\nExecStart={bin} server run $REX_ARGS\n")).unwrap();
                }),
                "environment variables ($)",
            ),
            (
                "semicolon",
                Box::new(|f| {
                    let bin = format!("{}/usr/bin/rex", f.root.display());
                    std::fs::write(f.root.join("usr/lib/systemd/system/rex.service"), format!("[Service]\nExecStart={bin} server run ; /bin/true\n")).unwrap();
                }),
                "command separator (;)",
            ),
            (
                "prefix-char",
                Box::new(|f| {
                    let bin = format!("{}/usr/bin/rex", f.root.display());
                    std::fs::write(f.root.join("usr/lib/systemd/system/rex.service"), format!("[Service]\nExecStart=-{bin} server run\n")).unwrap();
                }),
                "prefix",
            ),
            (
                "shell-wrapper",
                Box::new(|f| {
                    std::fs::write(f.root.join("usr/lib/systemd/system/rex.service"), "[Service]\nExecStart=/bin/sh -c /usr/bin/rex\n").unwrap();
                }),
                "doesn’t run rex server run directly",
            ),
            (
                "non-plain-word",
                Box::new(|f| {
                    let bin = format!("{}/usr/bin/rex", f.root.display());
                    std::fs::write(f.root.join("usr/lib/systemd/system/rex.service"), format!("[Service]\nExecStart={bin} server run --state-dir ~/rex\n")).unwrap();
                }),
                "unsupported syntax",
            ),
            (
                "loaded-differs-from-unit",
                Box::new(|f| {
                    let bin = format!("{}/usr/bin/rex", f.root.display());
                    f.write_state("sys_exec", &format!("{bin} server run --listen /run/rex/server.sock --state-dir /srv/other"));
                }),
                "loaded a different command",
            ),
            (
                "loaded-prefix-flag",
                Box::new(|f| {
                    let bin = format!("{}/usr/bin/rex", f.root.display());
                    let cmd = format!("{bin} server run --listen /run/rex/server.sock --state-dir /var/lib/rex --socket-mode 0660 --log-format text");
                    f.write_state(
                        "sys_raw_exec",
                        &format!(
                            "ExecStart={{ path={bin} ; argv[]={cmd} ; ignore_errors=no ; start_time=[n/a] ; stop_time=[n/a] ; pid=0 ; code=(null) ; status=0/0 }}\n\
                             ExecStartEx={{ path={bin} ; argv[]={cmd} ; flags=privileged ; start_time=[n/a] ; stop_time=[n/a] ; pid=0 ; code=(null) ; status=0/0 }}\n"
                        ),
                    );
                }),
                "prefix flags",
            ),
            (
                "rex-help-missing-flag",
                Box::new(|f| {
                    let rex_bin = f.root.join("usr/bin/rex");
                    std::fs::write(&rex_bin, "#!/bin/sh\necho '  --max-windows N  --max-panes N  --max-sessions N'\n").unwrap();
                }),
                "doesn’t support --max-blocks",
            ),
            (
                "personal-home-unit",
                Box::new(|f| {
                    f.write_state("mode", "user");
                    let home_unit = f.root.join("home/alice/.config/systemd/user/rex.service");
                    std::fs::write(&home_unit, format!("[Service]\nExecStart={}/usr/bin/rex server run --log-format text\n", f.root.display())).unwrap();
                    f.write_state("usr_frag", home_unit.to_str().unwrap());
                }),
                "personal unit file: raise its limits there",
            ),
            (
                "account-folder-symlink",
                Box::new(|f| {
                    f.write_state("mode", "user");
                    let drop_dir = f.root.join("home/alice/.config/systemd/user/rex.service.d");
                    let target = f.root.join("state/symlink-target");
                    std::fs::create_dir_all(&target).unwrap();
                    std::fs::remove_dir_all(&drop_dir).unwrap();
                    std::os::unix::fs::symlink(&target, &drop_dir).unwrap();
                }),
                "symbolic link; refusing to write there",
            ),
        ];

        for (name, setup, expect) in refusal_cases {
            let f = Fixture::new(&base_root, name);
            setup(&f);
            let mode = std::fs::read_to_string(f.root.join("state/mode")).unwrap_or_else(|_| "shared".into());
            let mode = mode.trim();
            // Place a sentinel file beside the drop-in directory to verify nothing is deleted.
            let keep_file = f.root.join("etc/systemd/system/rex.service.d/10-keep.conf");
            std::fs::write(&keep_file, "[Service]\nEnvironment=KEEP=1\n").unwrap();

            let t = f.look_rex(&[("OLO_REX_USER", "alice")]);
            assert!(t.rex_limits.blocked.as_deref().is_some_and(|w| w.contains(expect)), "{name}: expected '{expect}' in LOOK, got {:?}", t.rex_limits);
            assert!(!t.short().contains(&Tune::RexLimits) && Tune::RexLimits.apply(&t).is_none(), "{name}: blocked row must not be applicable");

            let (ok, out, err) = f.run_sh(&f.rex_apply_script(mode, "alice"), &[("OLO_REX_USER", "alice")]);
            assert!(!ok, "{name}: apply unexpectedly succeeded: {out}\n{err}");
            assert!(out.contains("Rex: ") && out.contains(expect), "{name}: expected 'Rex: ...{expect}' in apply output, got: {out}\n{err}");
            assert!(!f.sys_dropin().exists(), "{name}: must NOT write system drop-in");
            assert!(!f.usr_dropin().exists(), "{name}: must NOT write user drop-in");
            assert!(!f.etc_usr_dropin().exists(), "{name}: must NOT write /etc/systemd/user drop-in");
            assert!(keep_file.exists(), "{name}: must NOT remove existing files");
        }

        // 3. Foreign file at 90-olo-limits.conf (system AND personal account): neither Apply nor Undo touches it, LOOK marks kept=false.
        {
            let f = Fixture::new(&base_root, "foreign-file");
            let foreign = "[Service]\nEnvironment=ADMIN=1\n";
            std::fs::write(f.sys_dropin(), foreign).unwrap();
            let t = f.look_rex(&[("OLO_REX_USER", "alice")]);
            assert!(!t.of(Tune::RexLimits).kept, "foreign system file must not count as kept");
            assert!(t.rex_limits.blocked.as_deref().is_some_and(|w| w.contains("wasn’t written by Olo")), "{:?}", t.rex_limits);
            let (ok, out, _) = f.run_sh(&f.rex_apply_script("shared", "alice"), &[("OLO_REX_USER", "alice")]);
            assert!(!ok && out.contains("wasn’t written by Olo"), "{out}");
            assert_eq!(std::fs::read_to_string(f.sys_dropin()).unwrap(), foreign, "Apply must never overwrite foreign system file");
            let (ok, out, _) = f.run_sh(&Tune::RexLimits.undo(), &[("OLO_REX_MODE", "shared")]);
            assert!(!ok && out.contains("wasn’t written by Olo"), "{out}");
            assert_eq!(std::fs::read_to_string(f.sys_dropin()).unwrap(), foreign, "Put Back must never remove foreign system file");

            // Account's own folder has a foreign file too!
            f.write_state("mode", "user");
            std::fs::write(f.usr_dropin(), foreign).unwrap();
            let tu = f.look_rex(&[("OLO_REX_USER", "alice"), ("OLO_REX_MODE", "user")]);
            assert!(!tu.of(Tune::RexLimits).kept, "foreign user file must not count as kept");
            assert!(tu.rex_limits.blocked.as_deref().is_some_and(|w| w.contains("wasn’t written by Olo")), "{:?}", tu.rex_limits);
            let (ok, out, _) = f.run_sh(&f.rex_apply_script("user", "alice"), &[("OLO_REX_USER", "alice")]);
            assert!(!ok && out.contains("wasn’t written by Olo"), "{out}");
            assert_eq!(std::fs::read_to_string(f.usr_dropin()).unwrap(), foreign, "Apply must never overwrite foreign user file");
            let (ok, out, _) = f.run_sh(&Tune::RexLimits.undo(), &[("OLO_REX_MODE", "user"), ("OLO_REX_USER", "alice")]);
            assert!(!ok && out.contains("wasn’t written by Olo"), "{out}");
            assert_eq!(std::fs::read_to_string(f.usr_dropin()).unwrap(), foreign, "Put Back must never remove foreign user file");
        }

        // 4. Rollback on failed daemon-reload and read-back mismatch in BOTH system AND user scopes:
        //    files restored byte-for-byte to their prior state (or removed if none existed before).
        {
            let f = Fixture::new(&base_root, "rollback");
            // 4a: System first apply fails on reload -> no file left behind.
            f.write_state("sys_reload_fail", "1");
            let (ok, out, _) = f.run_sh(&f.rex_apply_script("shared", "alice"), &[("OLO_REX_USER", "alice")]);
            assert!(!ok && out.contains("Rex: ") && out.contains("couldn’t reload"), "{out}");
            assert!(!f.sys_dropin().exists() && !f.usr_dropin().exists(), "rollback must remove newly written files");
            std::fs::remove_file(f.root.join("state/sys_reload_fail")).unwrap();

            // 4b: System install, then update fails on reload and read-back -> prior file restored verbatim.
            let (ok, out, err) = f.run_sh(&f.rex_apply_script("shared", "alice"), &[("OLO_REX_USER", "alice")]);
            assert!(ok, "initial system apply: {out}\n{err}");
            let prior_sys = std::fs::read_to_string(f.sys_dropin()).unwrap();

            let updated_sys = format!("{}/usr/bin/rex server run --listen /run/rex/server.sock --log-level debug", f.root.display());
            std::fs::write(f.root.join("usr/lib/systemd/system/rex.service"), format!("[Service]\nExecStart={updated_sys}\n")).unwrap();
            f.write_state("sys_reload_fail", "1");
            let (ok, out, _) = f.run_sh(&f.rex_apply_script("shared", "alice"), &[("OLO_REX_USER", "alice")]);
            assert!(!ok && out.contains("previous settings were restored"), "{out}");
            assert_eq!(std::fs::read_to_string(f.sys_dropin()).unwrap(), prior_sys, "prior system Olo file must be restored verbatim");
            std::fs::remove_file(f.root.join("state/sys_reload_fail")).unwrap();

            f.write_state("sys_after_reload_exec", &format!("{}/usr/bin/rex server run --wrong", f.root.display()));
            let (ok, out, _) = f.run_sh(&f.rex_apply_script("shared", "alice"), &[("OLO_REX_USER", "alice")]);
            assert!(!ok && out.contains("didn’t load the expected command"), "{out}");
            assert_eq!(std::fs::read_to_string(f.sys_dropin()).unwrap(), prior_sys);
            std::fs::remove_file(f.root.join("state/sys_after_reload_exec")).unwrap();

            f.write_state("sys_reload_fail", "1");
            let (ok, out, _) = f.run_sh(&Tune::RexLimits.undo(), &[("OLO_REX_MODE", "shared")]);
            assert!(!ok && out.contains("couldn’t reload"), "{out}");
            assert_eq!(std::fs::read_to_string(f.sys_dropin()).unwrap(), prior_sys);
            std::fs::remove_file(f.root.join("state/sys_reload_fail")).unwrap();

            // 4c: User-scope rollback (first apply reload fail, update reload fail, read-back mismatch, undo reload fail).
            f.write_state("mode", "user");
            f.write_state("usr_reload_fail", "1");
            let (ok, out, _) = f.run_sh(&f.rex_apply_script("user", "alice"), &[("OLO_REX_USER", "alice")]);
            assert!(!ok && out.contains("Rex: ") && out.contains("Couldn’t reload"), "{out}");
            assert!(!f.usr_dropin().exists(), "user rollback must remove newly written file");
            std::fs::remove_file(f.root.join("state/usr_reload_fail")).unwrap();

            let (ok, out, err) = f.run_sh(&f.rex_apply_script("user", "alice"), &[("OLO_REX_USER", "alice")]);
            assert!(ok, "initial user apply: {out}\n{err}");
            let prior_usr = std::fs::read_to_string(f.usr_dropin()).unwrap();

            let updated_usr = format!("{}/usr/bin/rex server run --log-format text --log-level info", f.root.display());
            std::fs::write(f.root.join("usr/lib/systemd/user/rex.service"), format!("[Service]\nExecStart={updated_usr}\n")).unwrap();
            f.write_state("usr_reload_fail", "1");
            let (ok, out, _) = f.run_sh(&f.rex_apply_script("user", "alice"), &[("OLO_REX_USER", "alice")]);
            assert!(!ok && out.contains("previous settings were restored"), "{out}");
            assert_eq!(std::fs::read_to_string(f.usr_dropin()).unwrap(), prior_usr, "prior user Olo file must be restored verbatim");
            std::fs::remove_file(f.root.join("state/usr_reload_fail")).unwrap();

            f.write_state("usr_after_reload_exec", &format!("{}/usr/bin/rex server run --wrong", f.root.display()));
            let (ok, out, _) = f.run_sh(&f.rex_apply_script("user", "alice"), &[("OLO_REX_USER", "alice")]);
            assert!(!ok && out.contains("didn’t load the expected command"), "{out}");
            assert_eq!(std::fs::read_to_string(f.usr_dropin()).unwrap(), prior_usr);
            std::fs::remove_file(f.root.join("state/usr_after_reload_exec")).unwrap();

            f.write_state("usr_reload_fail", "1");
            let (ok, out, _) = f.run_sh(&Tune::RexLimits.undo(), &[("OLO_REX_MODE", "user"), ("OLO_REX_USER", "alice")]);
            assert!(!ok && out.contains("Couldn’t reload"), "{out}");
            assert_eq!(std::fs::read_to_string(f.usr_dropin()).unwrap(), prior_usr);
        }

        // 5. Round 4 Point 3: an outright failure (missing `awk`) still exits non-zero AND emits a `Rex: ` line.
        {
            let f = Fixture::new(&base_root, "no-awk");
            // Build a stripped bin dir with basic POSIX utilities (`sh`, `env`, `id`, `systemctl`) and NO `awk`.
            let slim = f.root.join("slimbin");
            std::fs::create_dir_all(&slim).unwrap();
            for tool in ["sh", "env", "cat", "rm", "mkdir", "chmod", "mv", "cp", "tr", "cut", "head", "wc", "mktemp", "timeout", "flock"] {
                for dir in ["/usr/bin", "/bin"] {
                    let src = std::path::Path::new(dir).join(tool);
                    if src.exists() {
                        let _ = std::os::unix::fs::symlink(&src, slim.join(tool));
                        break;
                    }
                }
            }
            let path = format!("{}/bin:{}", f.root.display(), slim.display());
            let out = std::process::Command::new("/bin/sh").args(["-c", &f.rewrite(&f.rex_apply_script("shared", "alice"))]).env("PATH", path).output().unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(!out.status.success(), "missing awk must fail");
            assert!(stdout.contains("Rex: "), "every failure path must emit a 'Rex: ' line, got stdout={stdout:?} stderr={:?}", String::from_utf8_lossy(&out.stderr));
        }

        let _ = std::fs::remove_dir_all(&base_root);
    }

    /// Finding 1: the help probe runs as the unit's own unprivileged `User=`
    /// (or `nobody` if empty/root and `nobody` can execute it), NEVER as root,
    /// and a personal-scope LOOK never probes or executes the system unit.
    #[test]
    fn rex_help_probe_never_runs_as_root_and_looks_only_at_active_scope() {
        use std::os::unix::fs::PermissionsExt as _;
        let base_root = std::path::PathBuf::from(format!("/var/tmp/olo-rex-r5-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base_root);

        // 1a: System unit with User=build probes as `build` (never root or hard-coded rex).
        let f = RexFixture::new(&base_root, "user-build");
        let marker = f.root.join("state/probed-as.txt");
        let custom_bin = f.root.join("home/build/bin/rex");
        std::fs::create_dir_all(custom_bin.parent().unwrap()).unwrap();
        std::fs::write(
            &custom_bin,
            format!("#!/bin/sh\necho \"${{OLO_TEST_RUNUSER_AS:-DIRECT_ROOT}}\" >> {}\necho '  --max-windows N --max-panes N --max-sessions N --max-blocks N'\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&custom_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        f.write_state("sys_user", "build");
        std::fs::write(f.root.join("usr/lib/systemd/system/rex.service"), format!("[Service]\nUser=build\nExecStart={} server run --log-format text\n", custom_bin.display())).unwrap();
        let t = f.look_rex(&[("OLO_REX_MODE", "shared"), ("OLO_REX_USER", "alice")]);
        assert_eq!(t.rex_limits.blocked, None, "{:?}", t.rex_limits);
        assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), "build", "help probe must run as the unit's User=build");

        // 1b: System unit with User=root (or empty) falls back to `nobody` when executable;
        // refuses WITHOUT executing the binary when `nobody` cannot execute it!
        let _ = std::fs::remove_file(&marker);
        f.write_state("sys_user", "root");
        let t_nobody = f.look_rex(&[("OLO_REX_MODE", "shared"), ("OLO_REX_USER", "alice")]);
        assert_eq!(t_nobody.rex_limits.blocked, None, "{:?}", t_nobody.rex_limits);
        assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), "nobody");

        let _ = std::fs::remove_file(&marker);
        f.write_state("deny_x_nobody", "1");
        let t_refused = f.look_rex(&[("OLO_REX_MODE", "shared"), ("OLO_REX_USER", "alice")]);
        assert!(t_refused.rex_limits.blocked.as_deref().is_some_and(|w| w.contains("as root")), "{:?}", t_refused.rex_limits);
        assert!(!marker.exists(), "must NEVER execute service binary as root when nobody cannot execute it");

        // 1c: Personal connection (`OLO_REX_MODE=user`) NEVER inspects or runs the system unit's binary!
        let _ = std::fs::remove_file(&marker);
        let _ = std::fs::remove_file(f.root.join("state/deny_x_nobody"));
        f.write_state("sys_user", "build");
        let tu = f.look_rex(&[("OLO_REX_MODE", "user"), ("OLO_REX_USER", "alice")]);
        assert_eq!(tu.rex_limits.scope, RexScope::User);
        assert_eq!(tu.rex_limits.blocked, None, "{:?}", tu.rex_limits);
        assert!(!marker.exists(), "personal-scope LOOK must never run the system unit's executable");

        let _ = std::fs::remove_dir_all(&base_root);
    }

    /// Finding 2 & Finding 3: baked `@MODE@`/`@USER@` override inherited
    /// environment variables, and a unit with `NeedDaemonReload=yes` is
    /// refused before writing in both `LOOK` and `Apply`.
    #[test]
    fn baked_scope_overrides_env_and_pending_unit_change_is_refused() {
        let base_root = std::path::PathBuf::from(format!("/var/tmp/olo-rex-r5-scope-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base_root);

        // Inherited OLO_REX_MODE=shared / OLO_REX_USER=mallory MUST NOT override baked user/alice.
        let f = RexFixture::new(&base_root, "baked-wins");
        let (ok, out, err) = f.run_sh(&f.rex_apply_script("user", "alice"), &[("OLO_REX_MODE", "shared"), ("OLO_REX_USER", "mallory")]);
        assert!(ok, "baked user/alice apply failed: {out}\n{err}");
        assert!(f.usr_dropin().exists(), "must write alice's personal file");
        assert!(!f.sys_dropin().exists(), "inherited OLO_REX_MODE=shared must NOT write system file");

        // NeedDaemonReload=yes refused in LOOK and Apply for both shared and user scopes.
        for (tag, mode, state_key, expect_cmd) in
            [("pending-sys", "shared", "sys_need_reload", "systemctl daemon-reload"), ("pending-usr", "user", "usr_need_reload", "systemctl --user daemon-reload")]
        {
            let f = RexFixture::new(&base_root, tag);
            f.write_state(state_key, "yes");
            let t = f.look_rex(&[("OLO_REX_MODE", mode), ("OLO_REX_USER", "alice")]);
            assert!(t.rex_limits.blocked.as_deref().is_some_and(|w| w.contains("hasn’t loaded yet") && w.contains(expect_cmd)), "{tag}: {:?}", t.rex_limits);
            assert!(Tune::RexLimits.apply(&t).is_none());
            let (ok, out, _) = f.run_sh(&f.rex_apply_script(mode, "alice"), &[("OLO_REX_USER", "alice")]);
            assert!(!ok && out.contains("hasn’t loaded yet") && out.contains(expect_cmd), "{tag}: {out}");
            assert!(!f.sys_dropin().exists() && !f.usr_dropin().exists(), "{tag}: nothing written");
        }

        let _ = std::fs::remove_dir_all(&base_root);
    }

    /// Finding 7: lock file is opened with `exec 9>>` (never truncated), a
    /// symlink at the lock path is refused without touching its target, and
    /// missing `flock` refuses instead of running unserialized.
    #[test]
    fn lock_file_is_never_truncated_and_missing_flock_refuses() {
        let base_root = std::path::PathBuf::from(format!("/var/tmp/olo-rex-r5-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base_root);

        // 7a: Existing bytes in the lock file survive both Apply and Put Back intact (`exec 9>>`).
        let f = RexFixture::new(&base_root, "no-truncate");
        let lock_file = f.root.join("state/olo-rex-limits.lock");
        std::fs::write(&lock_file, "SENTINEL_BYTES_DO_NOT_TRUNCATE\n").unwrap();
        let (ok, out, err) = f.run_sh(&f.rex_apply_script("shared", "alice"), &[("OLO_REX_USER", "alice")]);
        assert!(ok, "apply: {out}\n{err}");
        assert_eq!(std::fs::read_to_string(&lock_file).unwrap(), "SENTINEL_BYTES_DO_NOT_TRUNCATE\n", "Apply must never truncate lock file");
        let (ok, out, err) = f.run_sh(&Tune::RexLimits.undo(), &[("OLO_REX_MODE", "shared")]);
        assert!(ok, "undo: {out}\n{err}");
        assert_eq!(std::fs::read_to_string(&lock_file).unwrap(), "SENTINEL_BYTES_DO_NOT_TRUNCATE\n", "Undo must never truncate lock file");

        // 7b: Symlink at the lock path is refused before opening; target stays untouched.
        let f = RexFixture::new(&base_root, "symlink-lock");
        let target = f.root.join("state/precious");
        std::fs::write(&target, "KEEP_INTACT\n").unwrap();
        std::os::unix::fs::symlink(&target, f.root.join("state/olo-rex-limits.lock")).unwrap();
        let (ok, out, _) = f.run_sh(&f.rex_apply_script("shared", "alice"), &[("OLO_REX_USER", "alice")]);
        assert!(!ok && out.contains("symlink lock file"), "apply must refuse symlink lock: {out}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "KEEP_INTACT\n");
        assert!(!f.sys_dropin().exists());
        let (ok, out, _) = f.run_sh(&Tune::RexLimits.undo(), &[("OLO_REX_MODE", "shared")]);
        assert!(!ok && out.contains("symlink lock file"), "undo must refuse symlink lock: {out}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "KEEP_INTACT\n");

        // 7c: Missing `flock` refuses instead of running unserialized.
        let f = RexFixture::new(&base_root, "no-flock");
        let slim = f.root.join("slimbin");
        std::fs::create_dir_all(&slim).unwrap();
        for tool in ["sh", "env", "awk", "cat", "rm", "mkdir", "chmod", "mv", "cp", "tr", "cut", "head", "wc", "mktemp", "timeout"] {
            for dir in ["/usr/bin", "/bin"] {
                let src = std::path::Path::new(dir).join(tool);
                if src.exists() {
                    let _ = std::os::unix::fs::symlink(&src, slim.join(tool));
                    break;
                }
            }
        }
        let path = format!("{}/bin:{}", f.root.display(), slim.display());
        let out = std::process::Command::new("/bin/sh").args(["-c", &f.rewrite(&f.rex_apply_script("shared", "alice"))]).env("PATH", &path).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(!out.status.success() && stdout.contains("flock isn’t installed"), "missing flock must refuse Apply: {stdout}");
        assert!(!f.sys_dropin().exists());
        let out = std::process::Command::new("/bin/sh").args(["-c", &f.rewrite(&Tune::RexLimits.undo())]).env("PATH", &path).env("OLO_REX_MODE", "shared").output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(!out.status.success() && stdout.contains("flock isn’t installed"), "missing flock must refuse Undo: {stdout}");

        let _ = std::fs::remove_dir_all(&base_root);
    }

    /// Finding 8: Put Back verifies after reload that `LoadState=loaded`,
    /// Olo's file is gone from `DropInPaths`, and `rex.service` still has an
    /// `ExecStart`; if not, Olo's file is restored and reloaded with the
    /// reason. Also reconciles an already-deleted file still in `DropInPaths`.
    #[test]
    fn put_back_checks_remaining_execstart_and_reconciles_absent_file() {
        let base_root = std::path::PathBuf::from(format!("/var/tmp/olo-rex-r5-undo-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base_root);

        // 8a: Fragment loses ExecStart while Olo's file is loaded -> Put Back restores Olo's file and refuses!
        let f = RexFixture::new(&base_root, "no-exec-after-undo");
        let (ok, out, err) = f.run_sh(&f.rex_apply_script("shared", "alice"), &[("OLO_REX_USER", "alice")]);
        assert!(ok, "initial apply: {out}\n{err}");
        let prior = std::fs::read_to_string(f.sys_dropin()).unwrap();
        std::fs::write(f.root.join("usr/lib/systemd/system/rex.service"), "[Unit]\nDescription=Rex\n[Service]\nType=simple\n").unwrap();
        let (ok, out, _) = f.run_sh(&Tune::RexLimits.undo(), &[("OLO_REX_MODE", "shared")]);
        assert!(!ok && out.contains("without Olo’s file rex.service has no command: left as it was."), "expected refusal & rollback: {out}");
        assert_eq!(std::fs::read_to_string(f.sys_dropin()).unwrap(), prior, "Olo's file must be restored verbatim");
        assert!(f.root.join("state/sys_loaded_olo").exists(), "rollback must reload Olo's restored file");

        // 8b: Already-absent file on disk that systemd still has in DropInPaths triggers daemon-reload + verification!
        let f = RexFixture::new(&base_root, "already-absent-still-loaded");
        let (ok, out, err) = f.run_sh(&f.rex_apply_script("shared", "alice"), &[("OLO_REX_USER", "alice")]);
        assert!(ok, "initial apply: {out}\n{err}");
        std::fs::remove_file(f.sys_dropin()).unwrap();
        let _ = std::fs::remove_file(f.root.join("state/reloads"));
        // Case 1: underlying fragment has NO ExecStart -> already-absent Put Back reloads and FAILS (never silent ok)!
        std::fs::write(f.root.join("usr/lib/systemd/system/rex.service"), "[Unit]\nDescription=Rex\n[Service]\nType=simple\n").unwrap();
        let (ok, out, _) = f.run_sh(&Tune::RexLimits.undo(), &[("OLO_REX_MODE", "shared")]);
        assert!(!ok && out.contains("without Olo’s file rex.service has no command"), "already-absent path must reload & catch missing ExecStart: {out}");
        assert!(f.root.join("state/reloads").exists(), "already-absent path must run daemon-reload");
        // Case 2: underlying fragment has valid ExecStart -> already-absent Put Back reloads, clears DropInPaths, succeeds!
        let sys_cmd = format!("{}/usr/bin/rex server run --log-format text", f.root.display());
        std::fs::write(f.root.join("usr/lib/systemd/system/rex.service"), format!("[Service]\nExecStart={sys_cmd}\n")).unwrap();
        f.write_state("sys_loaded_olo", f.sys_dropin().to_str().unwrap());
        let _ = std::fs::remove_file(f.root.join("state/reloads"));
        let (ok, out, err) = f.run_sh(&Tune::RexLimits.undo(), &[("OLO_REX_MODE", "shared")]);
        assert!(ok, "reconciling already-absent file with valid unit should succeed: {out}\n{err}");
        assert!(f.root.join("state/reloads").exists(), "must run daemon-reload");
        assert!(!f.root.join("state/sys_loaded_olo").exists(), "must clear DropInPaths");

        let _ = std::fs::remove_dir_all(&base_root);
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
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let both = stdout.clone() + &String::from_utf8_lossy(&out.stderr);
            (out.status.success(), stdout, both)
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
                println!("{}: the host has it already (or blocked)", tune.id());
                continue;
            };
            let (ok, _, said) = run(&script);
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
                let (ok, _, said) = run("su - root -c 'echo $TMPDIR $TMP $TEMP $TEMPDIR $CLAUDE_CODE_TMPDIR; stat -f -c %T \"$(mktemp)\"'");
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
                let (ok, _, said) = run(&format!("su -s {BRUSH} - root -c 'echo signed in with $0'"));
                assert!(ok && said.contains("signed in"), "{said}");
            }
            if tune == Tune::RexLimits {
                // Compare parsed values against expected merged result (never demanding literal 512 when higher existed).
                let expected: Vec<u64> = before.of(Tune::RexLimits).values.iter().map(|v| v.now.map_or(v.wanted, |n| n.max(v.wanted))).collect();
                let got: Vec<Option<u64>> = stands.values.iter().map(|v| v.now).collect();
                assert_eq!(got, expected.into_iter().map(Some).collect::<Vec<_>>(), "merged limits mismatch: {:?}", stands);
            }
            // (A sign-in still works, with its limits set: PAM's.)
            let (ok, _, said) = run("su - root -c 'ulimit -Hn'");
            assert!(ok, "a sign-in after {}: {said}", tune.id());
            if tune == Tune::Files && over_ssh {
                assert_eq!(said.trim().parse::<u64>().ok(), Some(FILES), "a new sign-in's open files");
            }
            let (ok, _, said) = run(&tune.undo());
            assert!(ok, "{} put back: {said}", tune.id());
            let back = look();
            assert!(!back.of(tune).kept, "{}: its file is gone", tune.id());
            // (A kernel setting is put back at once; the others hold
            // from the next sign-in or start, which a new look is.)
            assert_eq!(back.of(tune).values, before.of(tune).values, "{} put back", tune.id());
            if tune == Tune::Shell {
                assert_eq!((shells(), back.default_shell.clone(), back.brush_there), (shells_before.clone(), before.default_shell.clone(), false), "every account's shell as it was, and brush gone");
                let (_, _, listed) = run("grep -c brush /etc/shells; ls /etc/olo-brush.was 2>&1");
                println!("shell put back: {}", listed.trim().replace('\n', " | "));
            }
        }
    }

    /// End-to-end verification of [`Tune::RexLimits`] inside a fresh,
    /// disposable systemd container created by this test itself and removed
    /// on every exit (pass or panic). Gated by `OLO_TUNE_CONTAINER_IMAGE`
    /// (e.g. `localhost/agy-limits-f44` or `localhost/agy-limits-deb`),
    /// never `OLO_TUNE_EXEC`.
    ///
    /// Tests the cases that matter most, for real:
    /// 1. An administrator's drop-in with `--state-dir` is refused, nothing
    ///    is written, and the server restarts with its custom state dir.
    /// 2. A unit with a continuation line is refused and nothing is written.
    /// 3. The plain case: applied, reloaded without restarting Rex, server
    ///    restarted by the test comes back with 512 tabs AND its saved
    ///    session, 512th tab succeeds, 513th tab gets Rex's specific
    ///    window-limit refusal while the server still answers `rex ls`,
    ///    stale refresh preserves higher limits, and Put Back removes only
    ///    Olo's file (no folder removed) and leaves a pending decrease
    ///    ("512 tabs now, 64 after Rex restarts").
    #[test]
    #[ignore = "needs OLO_TUNE_CONTAINER_IMAGE, a disposable systemd image with rex"]
    fn rex_limits_enforced_by_systemd_in_a_container() {
        let Ok(image) = std::env::var("OLO_TUNE_CONTAINER_IMAGE") else { return };
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let name = format!("agy-rex-limits-{}-{stamp}", std::process::id());

        struct ContainerGuard(String);
        impl Drop for ContainerGuard {
            fn drop(&mut self) {
                let _ = std::process::Command::new("podman").args(["rm", "-f", "-t", "1", &self.0]).output();
            }
        }

        let started = std::process::Command::new("podman")
            .args(["run", "-d", "--name", &name, "--systemd=always", "--pids-limit=16384", "--tmpfs", "/tmp:size=64m", "--tmpfs", "/var/lib/rex:size=32m", &image])
            .output()
            .expect("launch disposable podman container");
        assert!(started.status.success(), "podman run {image}: {}{}", String::from_utf8_lossy(&started.stdout), String::from_utf8_lossy(&started.stderr));
        let _guard = ContainerGuard(name.clone());

        let run = |script: &str| -> (bool, String, String) {
            let out = std::process::Command::new("podman").args(["exec", "-i", &name, "sh", "-c", script]).output().unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            (out.status.success(), stdout, stderr)
        };

        // Wait for systemd inside the fresh container.
        let mut booted = false;
        for _ in 0..40 {
            let (_, out, _) = run("systemctl is-system-running 2>&1 || true");
            let st = out.trim();
            if st == "running" || st == "degraded" {
                booted = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        assert!(booted, "systemd did not boot in {name}");

        let look = |prefix: &str| {
            let script = if prefix.is_empty() { LOOK.to_owned() } else { format!("{prefix}\n{LOOK}") };
            let (ok, stdout, stderr) = run(&script);
            assert!(ok, "LOOK failed: {stdout}\n{stderr}");
            Tuning::read(&stdout)
        };
        let direct_apply = |mode: &str| {
            REX_APPLY
                .replace("@HEAD@", &shell_word(OLO_HEAD))
                .replace("@MODE@", mode)
                .replace("@USER@", "''")
                .replace("@WINDOWS@", &REX_WINDOWS.to_string())
                .replace("@PANES@", &REX_PANES.to_string())
                .replace("@SESSIONS@", &REX_SESSIONS.to_string())
                .replace("@BLOCKS@", &REX_BLOCKS.to_string())
                .replace("@SYS_DROPIN@", REX_SYSTEM_DROPIN)
        };

        // Start clean: packaged shared rex.service active.
        let (ok, out, err) = run("systemctl enable --now rex.service && systemctl is-active rex.service");
        assert!(ok, "start rex.service: {out}\n{err}");

        // --- Case R5-1 (Finding 1): Unit with User=build and binary in /home/build/bin/rex ---
        // Help probe MUST run as `build` (never uid 0), and a User=root private binary MUST be refused without running it!
        let r5_setup_build = r#"set -e
cp /usr/lib/systemd/system/rex.service /var/tmp/rex.service.r5orig
useradd -m -s /bin/bash build
chmod 0700 /home/build
mkdir -p /home/build/bin /var/tmp/rex-probe-dir
chmod 0777 /var/tmp/rex-probe-dir
cat > /home/build/bin/rex <<'EOF'
#!/bin/sh
printf '%s\n' "$(id -u)" >> /var/tmp/rex-probe-dir/uid.log
exec /usr/bin/rex "$@"
EOF
chown -R build:build /home/build/bin
chmod 0755 /home/build/bin/rex
printf '%s\n' '[Unit]' 'Description=Rex Build' '[Service]' 'User=build' 'Group=build' 'ExecStart=/home/build/bin/rex server run --listen /run/rex/server.sock --state-dir /var/lib/rex --socket-mode 0660 --log-format text' > /usr/lib/systemd/system/rex.service
systemctl daemon-reload
"#;
        let (ok, out, err) = run(r5_setup_build);
        assert!(ok, "setup R5-1 User=build: {out}\n{err}");
        let build_uid = run("id -u build").1.trim().to_owned();
        let build_look = look("OLO_REX_MODE=shared\nOLO_REX_USER=alice");
        assert_eq!(build_look.rex_limits.blocked, None, "User=build unit must succeed: {:?}", build_look.rex_limits);
        let (_, uid_log, _) = run("cat /var/tmp/rex-probe-dir/uid.log 2>/dev/null || true");
        let probed_uids: Vec<&str> = uid_log.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        assert!(!probed_uids.is_empty() && probed_uids.iter().all(|u| *u == build_uid.as_str()), "help probe must run ONLY as build ({build_uid}), never uid 0! got: {probed_uids:?}");
        // Now User=root with /home/build/bin/rex (unreadable to nobody): MUST refuse without writing any uid marker!
        let (ok, out, err) = run("set -e; rm -f /var/tmp/rex-probe-dir/uid.log; \
             printf '%s\\n' '[Unit]' 'Description=Rex Root' '[Service]' 'User=root' 'ExecStart=/home/build/bin/rex server run --listen /run/rex/server.sock --state-dir /var/lib/rex --socket-mode 0660 --log-format text' > /usr/lib/systemd/system/rex.service; \
             systemctl daemon-reload");
        assert!(ok, "setup R5-1 User=root: {out}\n{err}");
        let root_look = look("OLO_REX_MODE=shared\nOLO_REX_USER=alice");
        assert!(root_look.rex_limits.blocked.as_deref().is_some_and(|w| w.contains("as root")), "User=root private binary must be refused: {:?}", root_look.rex_limits);
        let (ok, out, _) = run(&direct_apply("shared"));
        assert!(!ok && out.contains("as root"), "Apply must refuse User=root private binary: {out}");
        assert!(!run("test -e /var/tmp/rex-probe-dir/uid.log").0, "User=root private binary MUST NEVER be executed as root");
        let (ok, out, err) = run("mv /var/tmp/rex.service.r5orig /usr/lib/systemd/system/rex.service && rm -rf /var/tmp/rex-probe-dir && systemctl daemon-reload");
        assert!(ok, "restore after R5-1: {out}\n{err}");

        // --- Case R5-3 (Finding 3): Pending administrator 20-site.conf NOT reloaded -> refused, nothing written ---
        let (ok, out, err) = run("set -e; mkdir -p /var/lib/rex-custom /etc/systemd/system/rex.service.d; chown rex:rex /var/lib/rex-custom; \
             printf '%s\\n' '[Service]' 'ExecStart=' 'ExecStart=/usr/bin/rex server run --listen /run/rex/server.sock --state-dir /var/lib/rex-custom --socket-mode 0660 --log-format text' > /etc/systemd/system/rex.service.d/20-site.conf");
        assert!(ok, "write pending 20-site.conf: {out}\n{err}");
        assert_eq!(run("systemctl show -p NeedDaemonReload --value rex.service").1.trim(), "yes", "systemd must report NeedDaemonReload=yes");
        let pending_admin = look("OLO_REX_MODE=shared");
        assert!(
            pending_admin.rex_limits.blocked.as_deref().is_some_and(|w| w.contains("hasn’t loaded yet") && w.contains("systemctl daemon-reload")),
            "expected NeedDaemonReload refusal, got {:?}",
            pending_admin.rex_limits
        );
        let (ok, out, _) = run(&direct_apply("shared"));
        assert!(!ok && out.contains("hasn’t loaded yet"), "direct apply must refuse pending unit change: {out}");
        assert!(!run("test -e /etc/systemd/system/rex.service.d/90-olo-limits.conf").0, "no file written when NeedDaemonReload=yes");

        // --- Case 1: Administrator drop-in with --state-dir (reloaded) -> refused, nothing written, server restarts as before ---
        let (ok, out, err) = run("systemctl daemon-reload");
        assert!(ok, "reload admin drop-in: {out}\n{err}");
        let blocked_admin = look("");
        assert!(
            blocked_admin.rex_limits.blocked.as_deref().is_some_and(|w| w.contains("/etc/systemd/system/rex.service.d/20-site.conf") && w.contains("sets this server’s command")),
            "expected admin drop-in refusal, got {:?}",
            blocked_admin.rex_limits
        );
        assert!(!blocked_admin.short().contains(&Tune::RexLimits) && Tune::RexLimits.apply(&blocked_admin).is_none());
        let (ok, out, _) = run(&direct_apply("shared"));
        assert!(!ok && out.contains("20-site.conf"), "direct apply must refuse admin drop-in: {out}");
        assert!(!run("test -e /etc/systemd/system/rex.service.d/90-olo-limits.conf").0, "no file written on refusal");
        let (ok, out, err) = run("systemctl restart rex.service && systemctl is-active rex.service");
        assert!(ok, "restart with admin drop-in: {out}\n{err}");
        let (_, eff, _) = run("systemctl show -p ExecStart --value rex.service");
        assert!(eff.contains("--state-dir /var/lib/rex-custom"), "admin --state-dir must survive untouched: {eff}");
        let (ok, out, err) = run("rm -f /etc/systemd/system/rex.service.d/20-site.conf && systemctl daemon-reload");
        assert!(ok, "clean admin drop-in: {out}\n{err}");

        // --- Case 2: Unit with a continuation line -> refused, nothing written ---
        let (ok, out, err) = run("set -e; cp /usr/lib/systemd/system/rex.service /var/tmp/rex.service.orig; \
             sed -i 's|^ExecStart=/usr/bin/rex server run |ExecStart=/usr/bin/rex server run \\\\\\n  # continued\\n  |' /usr/lib/systemd/system/rex.service; \
             systemctl daemon-reload");
        assert!(ok, "setup continuation unit: {out}\n{err}");
        let blocked_cont = look("");
        assert!(blocked_cont.rex_limits.blocked.as_deref().is_some_and(|w| w.contains("continuation lines")), "expected continuation refusal, got {:?}", blocked_cont.rex_limits);
        assert!(Tune::RexLimits.apply(&blocked_cont).is_none());
        let (ok, out, _) = run(&direct_apply("shared"));
        assert!(!ok && out.contains("continuation lines"), "direct apply must refuse continuation: {out}");
        assert!(!run("test -e /etc/systemd/system/rex.service.d/90-olo-limits.conf").0, "no file written on continuation refusal");
        let (ok, out, err) = run("mv /var/tmp/rex.service.orig /usr/lib/systemd/system/rex.service && systemctl daemon-reload && systemctl restart rex.service");
        assert!(ok, "restore clean unit: {out}\n{err}");

        // --- Case 3: Plain case -> applied, reloaded without restart, restarted server keeps saved session & enforces 512 tabs ---
        let pid_before = run("systemctl show -p MainPID --value rex.service").1.trim().to_owned();
        assert!(pid_before.parse::<u32>().is_ok_and(|p| p > 0), "MainPID: {pid_before}");
        // Create a real saved session before restarting to verify sessions survive restart.
        let (ok, out, err) =
            run("runuser -u rex -- env HOME=/var/lib/rex XDG_RUNTIME_DIR=/run/rex rex -S unix:///run/rex/server.sock --autostart=false new keep-across-restart -- /bin/sleep 120 >/dev/null");
        assert!(ok, "create session before restart: {out}\n{err}");

        let before = look("");
        assert_eq!(before.rex_limits.blocked, None, "{:?}", before.rex_limits);
        assert!(!before.of(Tune::RexLimits).met() && !before.of(Tune::RexLimits).kept && !before.rex_limits.stale);
        assert_eq!(before.rex_limits.running, Some(RexLimitValues::defaults()));

        let script = Tune::RexLimits.apply(&before).expect("apply script");
        let (ok, out, err) = run(&script);
        assert!(ok, "apply: {out}\n{err}");
        let pid_after = run("systemctl show -p MainPID --value rex.service").1.trim().to_owned();
        assert_eq!(pid_before, pid_after, "applying must never restart the running Rex server");

        let (_, eff, _) = run("systemctl show -p ExecStart --value rex.service");
        for flag in ["--max-windows 512", "--max-panes 128", "--max-sessions 128", "--max-blocks 4096"] {
            assert!(eff.contains(flag), "missing {flag} in {eff}");
        }
        let pending = look("");
        assert!(pending.of(Tune::RexLimits).met() && pending.of(Tune::RexLimits).kept && !pending.rex_limits.stale);
        assert_eq!(pending.rex_limits_pending_restart(), Some((RexLimitValues::defaults(), RexLimitValues::wanted())));

        // Restart inside container: running server picks up 512 tabs AND restores the saved session.
        // Disable per-terminal cgroup delegation via a non-ExecStart drop-in only during the 512-tab
        // burst so 512 consecutive cgroup_mkdir calls don't stall on host cgroup lock contention;
        // Olo's 90-olo-limits.conf and ExecStart remain untouched.
        let (ok, out, err) = run(
            "printf '%s\\n' '[Service]' 'Environment=REX_CGROUP_DELEGATED=0' > /etc/systemd/system/rex.service.d/10-test-nocgroup.conf && systemctl daemon-reload && systemctl restart rex.service && systemctl is-active rex.service",
        );
        assert!(ok, "restart rex.service: {out}\n{err}");
        let active = look("");
        assert_eq!(active.rex_limits.running, Some(RexLimitValues::wanted()));
        assert_eq!(active.rex_limits_pending_restart(), None);
        let (ok, sessions_after, err) = run("runuser -u rex -- env HOME=/var/lib/rex XDG_RUNTIME_DIR=/run/rex rex -S unix:///run/rex/server.sock --autostart=false ls");
        assert!(ok && sessions_after.contains("keep-across-restart"), "saved session must survive restart: {sessions_after}\n{err}");

        // Verify enforcement: 65th and 512th tabs succeed, 513th receives Rex's specific window-limit error,
        // and the same server still answers `rex ls` immediately after!
        // Run the entire loop inside a single `runuser` shell so PAM/logind isn't invoked 512 times.
        let tab_check = r#"set -e
runuser -u rex -- env HOME=/var/lib/rex XDG_RUNTIME_DIR=/run/rex REX_RUNTIME_DIR=/run/rex sh -e -c '
unset REX_SESSION REX_BLOCK SSH_AUTH_SOCK
sid=$(rex -S unix:///run/rex/server.sock --autostart=false new limtest --json -- /bin/sleep 300 | sed -n '\''s/.*"session_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p'\'')
[ -n "$sid" ] || { echo "failed to create session"; exit 1; }
trap '\''rex -S unix:///run/rex/server.sock --autostart=false kill "$sid" >/dev/null 2>&1 || true'\'' EXIT
i=2
while [ "$i" -le 512 ]; do
  rex -S unix:///run/rex/server.sock --autostart=false window new -s "$sid" --focus=false "t$i" -- /bin/sleep 300 >/dev/null
  if [ "$i" -eq 65 ]; then echo "tab 65: ok"; fi
  if [ "$i" -eq 512 ]; then echo "tab 512: ok"; fi
  i=$((i + 1))
done
if err=$(rex -S unix:///run/rex/server.sock --autostart=false window new -s "$sid" --focus=false t513 -- /bin/sleep 300 2>&1); then
  echo "ERROR: tab 513 unexpectedly succeeded"
  exit 1
else
  printf '\''tab 513 refused: %s\n'\'' "$err"
fi
alive=$(rex -S unix:///run/rex/server.sock --autostart=false ls -q)
printf '\''alive sessions:\n%s\n'\'' "$alive"
printf '\''%s\n'\'' "$alive" | grep -qxF "$sid"
'
"#;
        let (ok, out, err) = run(tab_check);
        println!("tab check:\n{out}");
        assert!(
            ok && out.contains("tab 65: ok") && out.contains("tab 512: ok") && out.contains("tab 513 refused:") && out.contains("limit reached") && out.contains("512 windows"),
            "513th tab must hit Rex's own window limit and server must keep answering: {out}\n{err}"
        );
        let (ok, out, err) = run("rm -f /etc/systemd/system/rex.service.d/10-test-nocgroup.conf && systemctl daemon-reload");
        assert!(ok, "clean test drop-in: {out}\n{err}");

        // --- Case R5-5 (Finding 8): Put Back when fragment has no ExecStart restores Olo's file and refuses ---
        let prior_olo = run("cat /etc/systemd/system/rex.service.d/90-olo-limits.conf").1;
        let (ok, out, err) = run("set -e; cp /usr/lib/systemd/system/rex.service /var/tmp/rex.service.orig5; sed -i '/^ExecStart=/d' /usr/lib/systemd/system/rex.service; systemctl daemon-reload");
        assert!(ok, "strip ExecStart from fragment: {out}\n{err}");
        assert_eq!(run("systemctl show -p LoadState --value rex.service").1.trim(), "loaded", "unit stays loaded while Olo file provides ExecStart");
        let (ok, out, _) = run(&format!("OLO_REX_MODE=shared\n{}", Tune::RexLimits.undo()));
        assert!(!ok && out.contains("without Olo’s file rex.service has no command: left as it was."), "Put Back must refuse and restore when fragment lacks ExecStart: {out}");
        assert_eq!(run("cat /etc/systemd/system/rex.service.d/90-olo-limits.conf").1, prior_olo, "Olo's file must be restored verbatim");
        assert_eq!(run("systemctl show -p LoadState --value rex.service").1.trim(), "loaded", "rollback reload must leave rex.service loaded");
        assert!(run("systemctl show -p ExecStart --value rex.service").1.contains("--max-windows 512"), "rollback must restore Olo's ExecStart");
        let (ok, out, err) = run("mv /var/tmp/rex.service.orig5 /usr/lib/systemd/system/rex.service && systemctl daemon-reload");
        assert!(ok, "restore fragment after R5-5: {out}\n{err}");

        // Put Back removes only Olo's file, leaves rex.service.d intact, does NOT restart Rex,
        // and LOOK reports pending decrease (512 now, 64 after Rex restarts).
        assert!(!run("test -e /etc/systemd/user/rex.service.d/90-olo-limits.conf").0, "shared apply must NEVER write under /etc/systemd/user");
        let pid_before_undo = run("systemctl show -p MainPID --value rex.service").1.trim().to_owned();
        let (ok, out, err) = run(&format!("OLO_REX_MODE=shared\n{}", Tune::RexLimits.undo()));
        assert!(ok, "undo: {out}\n{err}");
        assert_eq!(run("systemctl show -p MainPID --value rex.service").1.trim(), pid_before_undo, "undo must not restart Rex");
        assert!(!run("test -e /etc/systemd/system/rex.service.d/90-olo-limits.conf").0);
        assert!(run("test -d /etc/systemd/system/rex.service.d").0, "undo must remove no folder");
        let after_undo = look("");
        assert!(!after_undo.of(Tune::RexLimits).kept && !after_undo.of(Tune::RexLimits).met());
        assert_eq!(after_undo.rex_limits_pending_restart(), Some((RexLimitValues::wanted(), RexLimitValues::defaults())));

        // --- Case 4 (Round 4): Two accounts on both Fedora 44 and Debian ---
        // Create second account `bob` with his own 50-bob.conf ExecStart drop-in; apply for `alice`;
        // verify alice's own ~/.config file is written as alice, bob's loaded command is untouched,
        // nothing of Olo's exists under /etc/systemd/user, and direct non-root Apply/Undo as alice works!
        let (ok, out, err) = run("useradd -m -s /bin/bash bob && loginctl enable-linger alice bob");
        assert!(ok, "setup alice & bob linger: {out}\n{err}");
        for acct in ["alice", "bob"] {
            let mut has_bus = false;
            for _ in 0..40 {
                if run(&format!("test -S /run/user/$(id -u {acct})/bus")).0 {
                    has_bus = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            assert!(has_bus, "{acct}'s systemd user bus did not start inside {image}");
        }
        let bob_setup = r#"set -e
bob_uid=$(id -u bob)
runuser -u bob -- env HOME=/home/bob XDG_RUNTIME_DIR="/run/user/$bob_uid" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$bob_uid/bus" sh -e -c '
  mkdir -p "$HOME/.config/systemd/user/rex.service.d"
  printf "%s\n" "[Service]" "ExecStart=" "ExecStart=/usr/bin/rex server run --log-format text --log-level debug --max-windows 96" > "$HOME/.config/systemd/user/rex.service.d/50-bob.conf"
  systemctl --user daemon-reload
'
"#;
        let (ok, out, err) = run(bob_setup);
        assert!(ok, "setup bob 50-bob.conf: {out}\n{err}");
        let bob_show = r#"bob_uid=$(id -u bob); runuser -u bob -- env HOME=/home/bob XDG_RUNTIME_DIR="/run/user/$bob_uid" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$bob_uid/bus" systemctl --user show -p ExecStart --value rex.service"#;
        let (ok, bob_before, err) = run(bob_show);
        assert!(ok && bob_before.contains("--log-level debug --max-windows 96"), "bob initial ExecStart: {bob_before}\n{err}");

        // --- Case R5-2 (Finding 2): Personal connection via production `manage::look` & `manage::act`
        //     with a LIVE ACTIVE shared `rex.service` running beside alice's personal service! ---
        // Production runner reports `rex_scope() == Some(RexScope::User)` and `account() == Some("alice")`:
        // LOOK must pick User (never Shared), Apply writes ONLY alice's ~/.config file as alice,
        // `/etc/systemd/system/rex.service.d/90-olo-limits.conf` stays ABSENT, bob's 50-bob.conf stays untouched,
        // and Put Back removes only alice's file!
        assert_eq!(run("systemctl is-active rex.service").1.trim(), "active", "shared rex.service must be live during personal-connection test");
        struct ContainerRunner {
            container: String,
            account: String,
            scope: RexScope,
        }
        impl crate::provision::Runner for ContainerRunner {
            async fn run(&mut self, _root: bool, command: &str, _said: &mut (dyn FnMut(&str) + Send)) -> Result<crate::provision::Ran, crate::provision::Failed> {
                let out = std::process::Command::new("podman").args(["exec", "-i", &self.container, "sh", "-c", command]).output().map_err(|e| e.to_string())?;
                let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
                let output = if stderr.is_empty() { stdout.clone() } else { format!("{stdout}{stderr}") };
                Ok(crate::provision::Ran { status: out.status.code().map(|c| c as u32), output, stdout, truncated: false })
            }
            fn account(&self) -> Option<&str> {
                Some(&self.account)
            }
            fn rex_scope(&self) -> Option<RexScope> {
                Some(self.scope)
            }
        }
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let mut runner = ContainerRunner { container: name.clone(), account: "alice".into(), scope: RexScope::User };
        let prod_before = rt.block_on(crate::manage::look(&mut runner, false)).expect("production manage::look");
        let u_before = prod_before.tuning.as_ref().expect("tuning from manage::look").clone();
        assert_eq!(u_before.rex_limits.scope, RexScope::User, "personal connection beside live shared server MUST pick User scope");
        assert_eq!(u_before.rex_limits.user.as_deref(), Some("alice"));
        assert_eq!(u_before.rex_limits.file_usr.as_deref(), Some("/home/alice/.config/systemd/user/rex.service.d/90-olo-limits.conf"));
        assert_eq!(u_before.rex_limits.blocked, None, "{:?}", u_before.rex_limits);

        let prod_after = rt.block_on(crate::manage::act(&mut runner, &prod_before, &crate::manage::Action::Tune(vec![Tune::RexLimits]), &mut |_| {})).expect("production personal Apply");
        let (ok, owner, err) = run("stat -c '%U' /home/alice/.config/systemd/user/rex.service.d/90-olo-limits.conf");
        assert!(ok && owner.trim() == "alice", "alice's file must be owned by alice (written as alice), got {owner:?}\n{err}");
        assert!(!run("test -e /etc/systemd/user/rex.service.d/90-olo-limits.conf").0, "nothing of Olo's may exist under /etc/systemd/user");
        assert!(!run("test -e /etc/systemd/system/rex.service.d/90-olo-limits.conf").0, "personal connection beside live shared server MUST NOT write system file");
        let u_after = prod_after.tuning.as_ref().unwrap();
        assert!(u_after.of(Tune::RexLimits).met() && u_after.of(Tune::RexLimits).kept, "{:?}", u_after.rex_limits);
        let (ok, bob_after, err) = run(bob_show);
        assert!(ok && bob_after == bob_before, "bob's loaded command must stay untouched!\nbefore: {bob_before}\nafter:  {bob_after}\n{err}");

        // Put Back via production `manage::act(Untune(RexLimits))`: removes ONLY alice's file, leaves shared & bob untouched.
        let prod_undone = rt.block_on(crate::manage::act(&mut runner, &prod_after, &crate::manage::Action::Untune(Tune::RexLimits), &mut |_| {})).expect("production personal Untune");
        assert!(!run("test -e /home/alice/.config/systemd/user/rex.service.d/90-olo-limits.conf").0);
        assert!(run("test -d /home/alice/.config/systemd/user/rex.service.d").0);
        assert!(!run("test -e /etc/systemd/system/rex.service.d/90-olo-limits.conf").0);
        assert!(!prod_undone.tuning.as_ref().unwrap().of(Tune::RexLimits).kept);
        let (ok, bob_after_undo, _) = run(bob_show);
        assert!(ok && bob_after_undo == bob_before);

        // Non-root direct execution as alice (no root required!): Apply and Put Back both succeed!
        let alice_apply = Tune::RexLimits.apply(&u_before).unwrap();
        let alice_apply_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &alice_apply);
        let alice_undo_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, format!("OLO_REX_MODE=user\n{}", Tune::RexLimits.undo()));
        let nonroot_cmd = format!(
            r#"set -e
alice_uid=$(id -u alice)
runuser -u alice -- env -u XDG_CONFIG_HOME HOME=/home/alice USER=alice LOGNAME=alice XDG_RUNTIME_DIR="/run/user/$alice_uid" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$alice_uid/bus" sh -e -c '
  printf "%s" "{alice_apply_b64}" | base64 -d | sh
  test -f "$HOME/.config/systemd/user/rex.service.d/90-olo-limits.conf"
  systemctl --user show -p ExecStart --value rex.service | grep -q -- "--max-windows 512"
  printf "%s" "{alice_undo_b64}" | base64 -d | sh
  test ! -e "$HOME/.config/systemd/user/rex.service.d/90-olo-limits.conf"
  test -d "$HOME/.config/systemd/user/rex.service.d"
'
"#
        );
        let (ok, out, err) = run(&nonroot_cmd);
        assert!(ok, "non-root direct apply/undo as alice failed: {out}\n{err}");
        assert!(!run("test -e /etc/systemd/user/rex.service.d/90-olo-limits.conf").0);
        let (ok, bob_final, _) = run(bob_show);
        assert!(ok && bob_final == bob_before, "bob must remain untouched after alice non-root run");
    }
}
