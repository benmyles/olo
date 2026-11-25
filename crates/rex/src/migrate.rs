//! An account moved from one host to another (Ben, 2026-11-25: "i want the
//! ability to migrate a rex user from one host to another, and it should
//! copy all the user's home dir files including agent
//! settings/logs/auth/state etc and just work seamlessly … machine to
//! machine copy (don't go via the user's desktop) … start back up the
//! agent sessions on the new host including custom terminal panes … and
//! so for the user the action session should then move to the new server
//! in the rex client"; "it must also support the case where the source is
//! running eg ubuntu and the dest is eg fedora").
//!
//! Built a piece at a time, each usable on its own. This one: what each
//! host says of itself for a move ([`LOOK`], [`Facts`]), read as its
//! account, changing nothing.

/// The script a host answers a move's look with: `key=value` lines, read by
/// [`Facts::read`]. POSIX sh; it changes nothing.
pub const LOOK: &str = r#"[ -r /etc/os-release ] && . /etc/os-release
printf 'system=%s\n' "${ID:-}"; printf 'system_name=%s\n' "${PRETTY_NAME:-${NAME:-}}"
printf 'machine=%s\n' "$(uname -m)"
printf 'user=%s\n' "$(id -un)"; printf 'uid=%s\n' "$(id -u)"; printf 'gid=%s\n' "$(id -g)"
printf 'home=%s\n' "$HOME"
printf 'free=%s\n' "$(df -Pk "$HOME" 2> /dev/null | awk 'NR == 2 {print $4 * 1024}')"
for t in rsync zstd gzip tar ssh ssh-keygen crontab; do command -v "$t" > /dev/null 2>&1 && printf 'has=%s\n' "$t"; done
case " $(id -Gn 2> /dev/null) " in *" wheel "*|*" sudo "*|*" admin "*) echo admin=1 ;; esac
for w in "${XDG_CONFIG_HOME:-$HOME/.config}"/systemd/user/*.wants/*; do [ -e "$w" ] || [ -L "$w" ] && printf 'enabled=%s\n' "${w##*/}"; done
if command -v systemctl > /dev/null 2>&1 && systemctl --user show-environment > /dev/null 2>&1; then
  systemctl --user list-units --type=service,socket,timer,path --state=active --no-legend --plain 2> /dev/null | awk '{print "active=" $1}'
fi
crontab -l > /dev/null 2>&1 && echo crontab=1
true
"#;

/// What a host says of itself for a move ([`LOOK`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// `/etc/os-release`'s `ID` ("ubuntu", "fedora") and `PRETTY_NAME`.
    pub system: String,
    pub system_name: String,
    /// `uname -m`: a program built for one machine doesn't run on another.
    pub machine: String,
    pub user: String,
    pub uid: u32,
    pub gid: u32,
    /// The account's home: where it differs, paths in what moves are
    /// rewritten.
    pub home: String,
    /// Bytes free on the home's filesystem.
    pub free: u64,
    /// The programs a move uses that are there (rsync, zstd, gzip, tar,
    /// ssh, ssh-keygen, crontab).
    pub has: Vec<String>,
    /// The account is an administrator there (in `wheel`, `sudo` or
    /// `admin`): told by its groups, sudo never tried to look (a refused
    /// sudo may be logged, or mailed to the host's administrators).
    pub admin: bool,
    /// The account's own systemd user units enabled (in its own
    /// `~/.config/systemd/user/*.wants`: not the system's own user units),
    /// and the services, sockets, timers and paths running for it.
    pub enabled: Vec<String>,
    pub active: Vec<String>,
    /// The account has a crontab.
    pub crontab: bool,
}

impl Facts {
    /// What [`LOOK`] printed.
    pub fn read(output: &str) -> Result<Facts, String> {
        let mut f = Facts::default();
        let mut seen = false;
        for line in output.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim();
            match key.trim() {
                "system" => f.system = value.to_owned(),
                "system_name" => f.system_name = value.to_owned(),
                "machine" => f.machine = value.to_owned(),
                "user" => {
                    f.user = value.to_owned();
                    seen = true;
                }
                "uid" => f.uid = value.parse().map_err(|_| format!("the host gave {value:?} for a user id"))?,
                "gid" => f.gid = value.parse().map_err(|_| format!("the host gave {value:?} for a group id"))?,
                "home" => f.home = value.to_owned(),
                "free" => f.free = value.parse().unwrap_or(0),
                "has" => f.has.push(value.to_owned()),
                "admin" => f.admin = value == "1",
                "enabled" => f.enabled.push(value.to_owned()),
                "active" => f.active.push(value.to_owned()),
                "crontab" => f.crontab = value == "1",
                _ => {}
            }
        }
        if !seen || f.home.is_empty() {
            return Err("the host didn't say who it is (no account or home)".into());
        }
        Ok(f)
    }

    /// Whether program `name` is there.
    pub fn has(&self, name: &str) -> bool {
        self.has.iter().any(|h| h == name)
    }

    /// Whether a program built on `other`'s machine runs here.
    pub fn same_machine(&self, other: &Facts) -> bool {
        self.machine == other.machine
    }
}

/// The script that says, for each pane's program by its pid (its
/// arguments: `sh -c CAPTURE sh PID…`), what Rex's own report of a pane
/// leaves out: its whole command line (an agent's flags among it:
/// `claude --dangerously-skip-permissions`), its folder, when it started
/// (clock ticks after boot: which of several agents in one folder began
/// last), and the agents' session files it holds open (Codex's rollout,
/// agy's conversation: Claude Code holds none). A line a program:
/// `pid=PID`, then `arg=…` each (escaped: `\\` and `\n`), `cwd=`,
/// `start=`, `open=` each. Read as the account; changes nothing.
pub const CAPTURE: &str = r#"for p in "$@"; do
  [ -r "/proc/$p/cmdline" ] || continue
  printf 'pid=%s\n' "$p"
  tr '\0' '\n' < "/proc/$p/cmdline" | sed 's/\\/\\\\/g; s/^/arg=/'
  printf 'cwd=%s\n' "$(readlink "/proc/$p/cwd" 2> /dev/null)"
  printf 'start=%s\n' "$(awk '{print $22}' "/proc/$p/stat" 2> /dev/null)"
  for f in /proc/"$p"/fd/*; do
    t=$(readlink "$f" 2> /dev/null) || continue
    case "$t" in
      "$HOME"/.codex/sessions/*.jsonl|"$HOME"/.gemini/antigravity-cli/conversations/*.db) printf 'open=%s\n' "$t" ;;
    esac
  done
done
true
"#;

/// A pane's program as [`CAPTURE`] says it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Program {
    pub pid: u32,
    /// Its whole command line, the program first.
    pub args: Vec<String>,
    pub cwd: String,
    /// When it started, in clock ticks after boot.
    pub start: u64,
    /// The agents' session files it holds open.
    pub open: Vec<String>,
}

impl Program {
    /// Every program [`CAPTURE`] told of, in its order.
    pub fn read_all(output: &str) -> Vec<Program> {
        let mut all: Vec<Program> = Vec::new();
        for line in output.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            if key == "pid" {
                all.push(Program { pid: value.parse().unwrap_or(0), ..Program::default() });
                continue;
            }
            let Some(p) = all.last_mut() else { continue };
            match key {
                "arg" => p.args.push(value.replace("\\\\", "\u{0}").replace("\\n", "\n").replace('\u{0}', "\\")),
                "cwd" => p.cwd = value.to_owned(),
                "start" => p.start = value.parse().unwrap_or(0),
                "open" => p.open.push(value.to_owned()),
                _ => {}
            }
        }
        all
    }

    /// The agent's own session it is in, where it holds the file open: Codex's
    /// rollout's id (`rollout-STAMP-ID.jsonl`), agy's conversation's
    /// (`ID.db`).
    pub fn open_session(&self) -> Option<String> {
        self.open.iter().find_map(|f| {
            let name = f.rsplit('/').next()?;
            if let Some(stem) = name.strip_suffix(".db") {
                return Some(stem.to_owned());
            }
            let stem = name.strip_suffix(".jsonl")?;
            // (The id is a UUID: its last five dash-separated parts.)
            let parts: Vec<&str> = stem.split('-').collect();
            (parts.len() >= 5).then(|| parts[parts.len() - 5..].join("-"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UBUNTU: &str = "system=ubuntu\nsystem_name=Ubuntu 26.04.1 LTS\nmachine=x86_64\nuser=ubuntu\nuid=1000\ngid=1000\nhome=/home/ubuntu\nfree=52428800000\nhas=rsync\nhas=zstd\nhas=gzip\nhas=tar\nhas=ssh\nhas=ssh-keygen\nhas=crontab\nadmin=1\nenabled=rex.socket\nenabled=olo-pool.service\nactive=rex.socket\nactive=rex.service\nactive=olo-pool.service\ncrontab=1\n";
    const FEDORA: &str = "system=fedora\nsystem_name=Fedora Linux 44 (Cloud Edition)\nmachine=aarch64\nuser=fedora\nuid=1000\ngid=1000\nhome=/home/fedora\nfree=1073741824\nhas=gzip\nhas=tar\nhas=ssh\nhas=ssh-keygen\nadmin=1\n";

    /// Each host's look is read whole; what a move needs to know of a pair
    /// (another system, another machine, another home) follows from it.
    #[test]
    fn a_hosts_look_is_read() {
        let (from, to) = (Facts::read(UBUNTU).unwrap(), Facts::read(FEDORA).unwrap());
        assert_eq!((from.system.as_str(), from.home.as_str(), from.free), ("ubuntu", "/home/ubuntu", 52_428_800_000));
        assert!(from.has("rsync") && from.has("zstd") && !to.has("rsync") && !to.has("zstd") && to.has("tar"));
        assert_eq!(from.enabled, ["rex.socket", "olo-pool.service"]);
        assert_eq!(from.active, ["rex.socket", "rex.service", "olo-pool.service"]);
        assert!(from.admin && to.admin && from.crontab && !to.crontab);
        assert!(LOOK.contains(r#"*" wheel "*|*" sudo "*|*" admin "*) echo admin=1"#) && !LOOK.contains("sudo -n"), "the look never tries sudo");
        assert!(!from.same_machine(&to), "x86_64 to aarch64");
        assert_eq!(to.system_name, "Fedora Linux 44 (Cloud Edition)");
        assert!(Facts::read("").is_err() && Facts::read("user=a\n").is_err(), "no account or home");
        assert!(Facts::read(&UBUNTU.replace("uid=1000", "uid=x")).is_err());
    }

    /// A pane's program as the capture says it: its whole command line
    /// (a line break and a backslash in an argument kept), its folder, when
    /// it started, and the session it holds open, Codex's by its rollout's
    /// id, agy's by its conversation's.
    #[test]
    fn a_panes_program_is_read() {
        let said = "pid=41\narg=claude\narg=--dangerously-skip-permissions\narg=a\\nb \\\\ c\ncwd=/home/ubuntu/proj-a\nstart=9000\npid=42\narg=codex\narg=--yolo\ncwd=/home/ubuntu/proj-b\nstart=9100\nopen=/home/ubuntu/.codex/sessions/2026/11/25/rollout-2026-11-25T01-02-03-0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b.jsonl\npid=43\narg=agy\nopen=/home/ubuntu/.gemini/antigravity-cli/conversations/3f2a7c1e-0000-4000-8000-123456789abc.db\n";
        let all = Program::read_all(said);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].args, ["claude", "--dangerously-skip-permissions", "a\nb \\ c"]);
        assert_eq!((all[0].cwd.as_str(), all[0].start, all[0].open_session()), ("/home/ubuntu/proj-a", 9000, None));
        assert_eq!(all[1].open_session().as_deref(), Some("0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b"));
        assert_eq!(all[2].open_session().as_deref(), Some("3f2a7c1e-0000-4000-8000-123456789abc"));
    }
}
