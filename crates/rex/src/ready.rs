//! A host's account made ready in one go. Ben, 2026-11-22: "have a way
//! in the rex app where i can have rex auto-setup my user on a remote
//! server like install claude code, agy, codex, copy my local
//! settings, configure the remote ones to go through token pool, and
//! so on. do it in a way where we can extend this over time too, e.g.
//! choosing what repos to clone and how to set them up and so on to
//! get everything up and running for a user as quickly and easily as
//! possible on a new remote server".
//!
//! Each [`Step`] is one thing the account is given. The host is asked
//! how each stands ([`LOOK`], [`Account::read`]), the Host window shows
//! it, and the steps that aren't done are run one after another
//! ([`Step::run`]), each shown as it runs and each safe to run again.
//! Everything is the account's own: nothing is done as root, nothing
//! outside its home.
//!
//! Written so a step is a small thing to add: a variant, its words, a
//! line of [`LOOK`], a script. The agents are put there by their
//! makers' own installers, fetched on the host (Claude Code's from
//! claude.ai, Codex's from chatgpt.com, the Antigravity CLI's from
//! antigravity.google: each puts its program in `~/.local/bin`); the
//! host's own Token Pool, where there is one, points Claude Code and
//! Codex at itself (`olo-pool-host`).

use crate::provision::shell_word;

/// One thing an account is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Step {
    /// `~/.local/bin` on the path of the account's shells: where the
    /// agents' installers put them, and where Rex is.
    Path,
    /// Claude Code.
    Claude,
    /// Codex.
    Codex,
    /// The Antigravity CLI (`agy`).
    Antigravity,
    /// Git told who the person is (their name and address, as this
    /// computer's git has them).
    Git,
}

/// Where each agent's installer is fetched from: its maker's own.
/// (`OLO_REX_READY_FROM`, a folder's address: the same file names under
/// it, for Olo's tests, which install stand-ins.)
const CLAUDE: &str = "https://claude.ai/install.sh";
const CODEX: &str = "https://chatgpt.com/codex/install.sh";
const ANTIGRAVITY: &str = "https://antigravity.google/cli/install.sh";

impl Step {
    /// In the order they are run: the path first, so that what the
    /// installers check for is so.
    pub const ALL: [Step; 5] = [Step::Path, Step::Claude, Step::Codex, Step::Antigravity, Step::Git];

    pub fn id(self) -> &'static str {
        match self {
            Step::Path => "path",
            Step::Claude => "claude",
            Step::Codex => "codex",
            Step::Antigravity => "agy",
            Step::Git => "git",
        }
    }

    pub fn of(id: &str) -> Option<Step> {
        Step::ALL.into_iter().find(|s| s.id() == id)
    }

    /// What it is called in the window.
    pub fn title(self) -> &'static str {
        match self {
            Step::Path => "Program folder",
            Step::Claude => "Claude Code",
            Step::Codex => "Codex",
            Step::Antigravity => "Antigravity",
            Step::Git => "Git identity",
        }
    }

    /// What doing it does, in a line.
    pub fn why(self) -> &'static str {
        match self {
            Step::Path => "~/.local/bin: where the agents and Rex are put, so your shells there find them.",
            Step::Claude => "Anthropic’s coding agent.",
            Step::Codex => "OpenAI’s coding agent.",
            Step::Antigravity => "Google’s coding agent (agy).",
            Step::Git => "Your name and address on commits made there, as on this computer.",
        }
    }

    /// The script that does it, for `sh` as the account. `who`: the
    /// person's name and address for git, where this computer knows
    /// them ([`Step::Git`] can't be done without). `from`: a folder's
    /// address to fetch the installers from instead of their makers'
    /// (tests).
    pub fn run(self, who: Option<&(String, String)>, from: Option<&str>) -> Option<String> {
        let fetched = |name: &str, real: &str, shell: &str| {
            let url = match from {
                Some(folder) => format!("{}/{name}", folder.trim_end_matches('/')),
                None => real.to_owned(),
            };
            // (Fetched whole, then run: a download that broke off isn't
            // half a script run. With nothing to read from, so an
            // installer that would ask takes its own answers.)
            format!(
                r#"set -eu
command -v curl > /dev/null 2>&1 || {{ echo "curl isn't installed on this host: the installer is fetched with it." >&2; exit 1; }}
export PATH="$HOME/.local/bin:$PATH"
f=$(mktemp)
trap 'rm -f "$f"' EXIT
curl -fsSL --retry 2 -o "$f" {url}
{shell} "$f" < /dev/null
"#,
                url = shell_word(&url),
            )
        };
        Some(match self {
            Step::Path => PATH.to_owned(),
            Step::Claude => fetched("claude.sh", CLAUDE, "bash"),
            Step::Codex => fetched("codex.sh", CODEX, "sh"),
            Step::Antigravity => fetched("agy.sh", ANTIGRAVITY, "bash"),
            Step::Git => {
                let (name, email) = who?;
                format!(
                    "set -eu\ncommand -v git > /dev/null 2>&1 || {{ echo \"git isn't installed on this host.\" >&2; exit 1; }}\ngit config --global user.name {}\ngit config --global user.email {}\n",
                    shell_word(name),
                    shell_word(email)
                )
            }
        })
    }
}

/// Puts `~/.local/bin` on the path of the account's shells: a line in
/// the files its shells read as they start, those that are there (and
/// `~/.profile`, made if no file for a sign-in's shell is), marked as
/// Olo's and put once. A shell that has it already is left alone
/// ([`LOOK`] asks a sign-in's shell).
const PATH: &str = r##"set -eu
mkdir -p "$HOME/.local/bin"
line='case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) PATH="$HOME/.local/bin:$PATH"; export PATH ;; esac  # Olo: programs in ~/.local/bin'
put() {
  grep -q 'Olo: programs in ~/.local/bin' "$1" 2> /dev/null && return 0
  printf '\n%s\n' "$line" >> "$1"
}
any=
for f in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile" "$HOME/.zprofile"; do
  [ -f "$f" ] && { put "$f"; any=1; }
done
[ -n "$any" ] || put "$HOME/.profile"
for f in "$HOME/.bashrc" "$HOME/.zshrc"; do
  [ -f "$f" ] && put "$f"
done
true
"##;

/// `script` run so that it ends when whoever asked for it lets go: a
/// command run over SSH goes on by itself after its channel closes (an
/// installer half way through a download would, to its end). The
/// script runs in a group of its own with nothing to read; beside it
/// something reads what the command was given to read, which ends
/// when the channel does, and then ends the whole group. What the
/// script answers is what this answers.
// (Its process group ended with `kill -TERM "-$p"`: dash, /bin/sh on
// Debian and Ubuntu hosts, takes no `--` there ("Illegal number: -"), and
// its fallback ended the script alone, what it had started going on to
// its end. Found on the dev server, 2026-11-24.)
pub fn cancellable(script: &str) -> String {
    format!(
        r#"exec 3<&0
if command -v setsid > /dev/null 2>&1; then setsid sh -c {script} < /dev/null & else sh -c {script} < /dev/null & fi
p=$!
( cat <&3 > /dev/null 2>&1; kill -TERM "-$p" 2> /dev/null || kill -TERM "$p" 2> /dev/null ) &
w=$!
wait "$p"
rc=$?
kill "$w" 2> /dev/null
exit $rc
"#,
        script = shell_word(script)
    )
}

/// A line an installer wrote, as it is said in a window: what was
/// last written on it (a progress bar draws over itself with carriage
/// returns), without a terminal's colours and movements, trimmed.
pub fn plain_line(line: &str) -> String {
    let line = line.rsplit('\r').find(|part| !part.trim().is_empty()).unwrap_or("");
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // (CSI: to its final byte; another escape: its one letter.)
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            } else {
                chars.next();
            }
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out.trim().to_owned()
}

/// How an account stands, as last heard.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Account {
    /// `~/.local/bin` is on the path of a sign-in's shell.
    pub path: bool,
    /// Each agent's version, where it is there.
    pub claude: Option<String>,
    pub codex: Option<String>,
    pub antigravity: Option<String>,
    /// Who git takes the person for there: a name and an address
    /// (either may be empty).
    pub git: Option<(String, String)>,
    /// What the steps need of the host: curl to fetch the installers,
    /// git for its identity.
    pub has_curl: bool,
    pub has_git: bool,
}

impl Account {
    /// What a host answered [`LOOK`].
    pub fn read(output: &str) -> Account {
        let said = |name: &str| output.lines().find_map(|line| line.strip_prefix(name).and_then(|rest| rest.strip_prefix(' ')).map(str::trim));
        let version = |name: &str| said(name).filter(|v| !v.is_empty() && *v != "-").map(version_said);
        let yes = |name: &str| said(name) == Some("yes");
        Account {
            path: yes("path"),
            claude: version("claude"),
            codex: version("codex"),
            antigravity: version("agy"),
            git: said("git-name").zip(said("git-email")).map(|(n, e)| (n.to_owned(), e.to_owned())).filter(|(n, e)| !n.is_empty() || !e.is_empty()),
            has_curl: yes("has-curl"),
            has_git: yes("has-git"),
        }
    }

    /// Whether `step` is done. `who`: the name and address this
    /// computer's git has, which the host's is to be.
    pub fn done(&self, step: Step, who: Option<&(String, String)>) -> bool {
        match step {
            Step::Path => self.path,
            Step::Claude => self.claude.is_some(),
            Step::Codex => self.codex.is_some(),
            Step::Antigravity => self.antigravity.is_some(),
            // (Told who the person is, by whoever: a host whose git
            // has another name isn't said to be undone by this
            // computer's.)
            Step::Git => self.git.as_ref().is_some_and(|(name, email)| !name.is_empty() && !email.is_empty()) || (who.is_none() && self.git.is_some()),
        }
    }

    /// What `step` has, to say beside it: an agent's version, git's
    /// name and address.
    pub fn has(&self, step: Step) -> Option<String> {
        match step {
            Step::Path => None,
            Step::Claude => self.claude.clone(),
            Step::Codex => self.codex.clone(),
            Step::Antigravity => self.antigravity.clone(),
            Step::Git => self.git.as_ref().map(|(name, email)| match (name.is_empty(), email.is_empty()) {
                (false, false) => format!("{name} <{email}>"),
                (false, true) => name.clone(),
                _ => email.clone(),
            }),
        }
    }

    /// The steps not done yet that can be, in the order they are run.
    pub fn left(&self, who: Option<&(String, String)>) -> Vec<Step> {
        Step::ALL.into_iter().filter(|s| !self.done(*s, who) && (*s != Step::Git || who.is_some())).collect()
    }
}

/// A program's version out of what it says of itself: the first word
/// that begins with a figure ("2.1.331 (Claude Code)", "codex-cli
/// 0.172.0", "1.5.15"); what it said, where there is none.
fn version_said(said: &str) -> String {
    said.split_whitespace().find(|w| w.starts_with(|c: char| c.is_ascii_digit())).unwrap_or(said).trim_end_matches([',', ';']).to_owned()
}

/// Asks a host, as the account, how it stands: what [`Account::read`]
/// reads. (The path is a sign-in's shell's, asked of the account's own
/// shell: this command's is the SSH server's. An agent is looked for
/// where its installer puts it, then by name; each is asked its
/// version, and given ten seconds to say it.)
pub const LOOK: &str = r##"sh_of=${SHELL:-/bin/sh}
case "$( "$sh_of" -lc 'echo ":$PATH:"' 2> /dev/null < /dev/null | tail -n 1 )" in *":$HOME/.local/bin:"*) echo "path yes" ;; *) echo "path no" ;; esac
PATH="$HOME/.local/bin:$PATH"
for a in claude codex agy; do
  if command -v "$a" > /dev/null 2>&1; then
    v=$( (timeout 10 "$a" --version 2> /dev/null < /dev/null || true) | head -n 1 )
    echo "$a ${v:-there}"
  else
    echo "$a -"
  fi
done
if command -v git > /dev/null 2>&1; then
  echo "has-git yes"
  echo "git-name $(git config --global user.name 2> /dev/null || true)"
  echo "git-email $(git config --global user.email 2> /dev/null || true)"
else
  echo "has-git no"
fi
command -v curl > /dev/null 2>&1 && echo "has-curl yes" || echo "has-curl no"
true
"##;

/// The name and address in a git configuration's text (`[user]`'s
/// `name` and `email`): what this computer's git takes the person for,
/// read from its file (`~/.gitconfig`, or git's under the config
/// folder) without asking git.
pub fn who_in(config: &str) -> Option<(String, String)> {
    let (mut in_user, mut name, mut email) = (false, None, None);
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_user = line.trim_start_matches('[').trim_end_matches(']').trim().eq_ignore_ascii_case("user");
            continue;
        }
        if !in_user || line.starts_with(['#', ';']) {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim();
        // (A value in quotes is what is between them; a comment after
        // one that isn't, is none of it.)
        let value = match value.strip_prefix('"') {
            Some(rest) => rest.split('"').next().unwrap_or(rest).to_owned(),
            None => {
                let ends = [value.find(" #"), value.find(" ;")].into_iter().flatten().min().unwrap_or(value.len());
                value[..ends].trim().to_owned()
            }
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "name" => name = Some(value),
            "email" => email = Some(value),
            _ => {}
        }
    }
    name.zip(email).filter(|(n, e)| !n.is_empty() && !e.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_host_says_of_its_account_is_read() {
        let fresh = Account::read("path no\nclaude -\ncodex -\nagy -\nhas-git yes\ngit-name \ngit-email \nhas-curl yes\n");
        assert_eq!(fresh, Account { has_curl: true, has_git: true, ..Account::default() });
        let who = ("A Person".to_owned(), "person@example.org".to_owned());
        assert_eq!(fresh.left(Some(&who)), Step::ALL);
        // (Git's identity can't be given where this computer has none.)
        assert_eq!(fresh.left(None), [Step::Path, Step::Claude, Step::Codex, Step::Antigravity]);

        let ready = Account::read("path yes\nclaude 2.1.331 (Claude Code)\ncodex codex-cli 0.172.0\nagy 1.5.15\nhas-git yes\ngit-name A Person\ngit-email person@example.org\nhas-curl yes\n");
        assert_eq!((ready.claude.as_deref(), ready.codex.as_deref(), ready.antigravity.as_deref()), (Some("2.1.331"), Some("0.172.0"), Some("1.5.15")));
        assert_eq!(ready.has(Step::Git).as_deref(), Some("A Person <person@example.org>"));
        assert!(ready.left(Some(&who)).is_empty() && ready.path);
        // (A host whose git has another name is done: it isn't this computer's to change.)
        let other = Account::read("git-name Somebody Else\ngit-email else@example.org\nhas-git yes\n");
        assert!(other.done(Step::Git, Some(&who)));
        // (An agent that is there and says nothing of its version.)
        assert_eq!(Account::read("claude there\n").claude.as_deref(), Some("there"));
        // (A host that answers nothing: nothing is known to be there.)
        assert_eq!(Account::read(""), Account::default());
    }

    #[test]
    fn the_steps_scripts_fetch_from_the_makers_and_quote_what_they_are_given() {
        let claude = Step::Claude.run(None, None).unwrap();
        assert!(claude.contains("curl -fsSL --retry 2 -o \"$f\" https://claude.ai/install.sh") && claude.contains("bash \"$f\" < /dev/null"));
        assert!(Step::Codex.run(None, None).unwrap().contains("https://chatgpt.com/codex/install.sh"));
        assert!(Step::Antigravity.run(None, None).unwrap().contains("https://antigravity.google/cli/install.sh"));
        // (From a folder of stand-ins, for Olo's tests.)
        assert!(Step::Codex.run(None, Some("http://127.0.0.1:27881/")).unwrap().contains(" http://127.0.0.1:27881/codex.sh\n"));
        // (Git's, with a name that a shell would take apart.)
        let who = ("Zoë O'Neil".to_owned(), "zoe@example.org".to_owned());
        let git = Step::Git.run(Some(&who), None).unwrap();
        assert!(git.contains("git config --global user.name 'Zoë O'\\''Neil'\n") && git.contains("git config --global user.email zoe@example.org\n"), "{git}");
        assert_eq!(Step::Git.run(None, None), None);
        for step in Step::ALL {
            assert_eq!(Step::of(step.id()), Some(step));
        }
    }

    #[test]
    fn an_installers_line_is_said_plain() {
        assert_eq!(plain_line("\u{1b}[1;32m✔\u{1b}[0m Installed \u{1b}[2Kclaude 2.1.331  "), "✔ Installed claude 2.1.331");
        // (A bar drawn over itself: what it came to.)
        assert_eq!(plain_line("  10% ####\r  55% #########\r 100% ################"), "100% ################");
        assert_eq!(plain_line("Downloading…\r"), "Downloading…");
        assert_eq!(plain_line("\u{1b}[?25l\u{1b}[?25h"), "");
        assert_eq!(plain_line(""), "");
    }

    #[test]
    fn this_computers_git_says_who_the_person_is() {
        let config = "[core]\n\teditor = vi\n[user]\n\tname = A Person\n\temail = \"person@example.org\" # mine\n[alias]\n\tname = not-this\n";
        assert_eq!(who_in(config), Some(("A Person".to_owned(), "person@example.org".to_owned())));
        assert_eq!(who_in("[User]\nemail=p@example.org\nname=P ; the short one\n"), Some(("P".to_owned(), "p@example.org".to_owned())));
        assert_eq!(who_in("[user]\n\tname = Only A Name\n"), None);
        assert_eq!(who_in(""), None);
    }

    /// A script let go of ends, with what it started: the channel's
    /// end is the end of what the command reads.
    #[test]
    fn a_script_let_go_of_ends_with_what_it_started() {
        use std::io::Write;
        let mark = format!("/var/tmp/olo-ready-mark-{}", std::process::id());
        let _ = std::fs::remove_file(&mark);
        // (It would write its mark after two seconds, were it left to.)
        let script = format!("sh -c 'sleep 2; echo late > {mark}' & wait");
        let mut child = std::process::Command::new("sh").arg("-c").arg(cancellable(&script)).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let began = std::time::Instant::now();
        drop(child.stdin.take());
        let status = child.wait().unwrap();
        assert!(began.elapsed() < std::time::Duration::from_millis(1500), "it ended when let go of, not at its own end: {:?}", began.elapsed());
        assert!(!status.success());
        std::thread::sleep(std::time::Duration::from_millis(2200));
        assert!(!std::path::Path::new(&mark).exists(), "what it started went on to its end");
        // (And one left to itself answers as the script does.)
        let mut kept = std::process::Command::new("sh").arg("-c").arg(cancellable("echo said; exit 3")).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
        let held = kept.stdin.take().unwrap();
        let out = kept.wait_with_output().unwrap();
        drop(held);
        assert_eq!((out.status.code(), String::from_utf8_lossy(&out.stdout).trim().to_owned()), (Some(3), "said".to_owned()));
        let _ = std::io::stdout().flush();
    }

    /// The scripts against a real machine: a scratch home on this one
    /// by default (`sh`), or whatever `OLO_READY_EXEC` says runs a
    /// script given as its last argument (a container: `podman exec -i
    /// NAME sh -c`). The path's step and git's, twice each (safe to run
    /// again); the agents' installers only where `OLO_READY_AGENTS`
    /// says to fetch them (hundreds of MB, from their makers').
    #[test]
    fn the_scripts_do_what_they_say_on_a_real_machine() {
        // (On disk: /tmp is memory here.)
        let home = std::path::PathBuf::from(format!("/var/tmp/olo-ready-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let exec = std::env::var("OLO_READY_EXEC").ok();
        let run = |script: &str| -> (bool, String) {
            let mut command = match &exec {
                Some(exec) => {
                    let mut words = exec.split_whitespace();
                    let mut c = std::process::Command::new(words.next().unwrap());
                    c.args(words);
                    c
                }
                None => {
                    let mut c = std::process::Command::new("sh");
                    c.arg("-c").env("HOME", &home).env("SHELL", "/bin/sh").env_remove("XDG_CONFIG_HOME").env("GIT_CONFIG_NOSYSTEM", "1");
                    c
                }
            };
            let out = command.arg(script).output().unwrap();
            (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
        };
        let look = || Account::read(&run(LOOK).1);
        let who = ("Zoë O'Neil".to_owned(), "zoe@example.org".to_owned());

        let before = look();
        if exec.is_none() {
            assert!(!before.path && before.git.is_none(), "a scratch home: {before:?}");
        }
        for round in 0..2 {
            for step in [Step::Path, Step::Git] {
                let (ok, said) = run(&step.run(Some(&who), None).unwrap());
                assert!(ok, "{step:?}, round {round}: {said}");
            }
        }
        let after = look();
        assert!(after.path, "a sign-in's shell has ~/.local/bin: {after:?}");
        assert_eq!(after.git, Some(who.clone()));
        if exec.is_none() {
            // (Put once, however often it is run.)
            let profile = std::fs::read_to_string(home.join(".profile")).unwrap();
            assert_eq!(profile.matches("Olo: programs in ~/.local/bin").count(), 1, "{profile}");
        }
        if std::env::var_os("OLO_READY_AGENTS").is_some() {
            for step in [Step::Claude, Step::Codex, Step::Antigravity] {
                let (ok, said) = run(&step.run(None, None).unwrap());
                assert!(ok, "{step:?}: {}", said.lines().rev().take(12).collect::<Vec<_>>().join(" | "));
                assert!(look().done(step, None), "{step:?} is there after its installer: {:?}", look());
            }
            eprintln!("the agents there: {:?}", look());
        }
        let _ = std::fs::remove_dir_all(&home);
    }
}
