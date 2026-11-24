//! Setting a host up to run Rex: what the host says of how it stands,
//! what each way of running Rex there needs done, and the doing of it.
//!
//! Rex runs on a Linux host in one of three ways (its "server modes"):
//!
//! - **Personal**: your own server, as your account. Any `rex` command
//!   starts it; `rex server install` starts it at boot and keeps it while
//!   you are logged out (a systemd user unit, and lingering). No root.
//! - **Shared**: one server for a few people who share an account, the
//!   `rex` system account (`rex.service`); members of the `rex` group
//!   reach its socket, `/run/rex/server.sock`.
//! - **Login**: several people, each as their own account
//!   (`rex-login-server.service`, as root): it knows a caller by how they
//!   connect, looks them up in `/etc/rex/accounts.jsonc`, and hands the
//!   connection to that account's own server, started in a login session.
//!   Over SSH a person reaches that same server of theirs directly.
//!
//! The shared server and the login server come with Rex's package (its
//! system units, the `rex` account, the accounts file, the login
//! server's PAM policy), installed by Rex's own installer as root; the
//! personal server needs only the `rex` program, which the installer
//! puts in `~/.local/bin` for an account that has no root.
//!
//! Everything here is from Rex's own instructions (its units' comments,
//! `rex server install --help`, its installer, its documentation's
//! Server Modes, Shared Server, Login Server and Remote Access pages),
//! and the steps are shown to the person, command by command, before
//! any is run: [`plan`] says what a host needs, [`carry_out`] does one
//! step through whatever runs commands there ([`Runner`]: SSH, as the
//! account or as root).

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde::{Deserialize, Serialize};

use crate::{accounts::Accounts, ssh::SshTarget};

/// Rex's installer: adds Rex's repository to the system's package
/// manager and installs the `rex` package (as root), or with `--user`
/// puts the program in `~/.local/bin` (as anyone).
pub const INSTALLER: &str = "https://pkgs.superlogical.com/rex/install.sh";
/// The login server's accounts.
pub const ACCOUNTS_FILE: &str = "/etc/rex/accounts.jsonc";
/// The shared server's socket.
pub const SHARED_SOCKET: &str = "/run/rex/server.sock";

/// How Rex runs on a host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerMode {
    #[default]
    Personal,
    Shared,
    Login,
}

impl ServerMode {
    pub const ALL: [ServerMode; 3] = [ServerMode::Personal, ServerMode::Shared, ServerMode::Login];

    /// Its name, as Rex's documentation has it.
    pub fn name(self) -> &'static str {
        match self {
            ServerMode::Personal => "Personal",
            ServerMode::Shared => "Shared",
            ServerMode::Login => "Login",
        }
    }

    /// Who it is for, in a few words.
    pub fn who(self) -> &'static str {
        match self {
            ServerMode::Personal => "Just you",
            ServerMode::Shared => "A few people sharing one account",
            ServerMode::Login => "Several people, each as themselves",
        }
    }

    /// What it is, in a sentence, for under the choice.
    pub fn says(self) -> &'static str {
        match self {
            ServerMode::Personal => "Your own Rex server there, as your account. No root needed.",
            ServerMode::Shared => "One Rex server run as the rex account, for the people in its group. Setting it up needs root.",
            ServerMode::Login => "A login server gives each person a Rex server of their own account. Setting it up needs root.",
        }
    }
}

/// A systemd unit, as `systemctl is-active` and `is-enabled` have it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Unit {
    pub active: bool,
    pub enabled: bool,
}

impl Unit {
    /// In use at all: running, or to be started at boot or on demand.
    pub fn used(self) -> bool {
        self.active || self.enabled
    }
}

/// What fetches the installer there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fetch {
    Curl,
    Wget,
}

/// The login server's accounts file, as the account asking could see it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AccountsSeen {
    /// There is none (Rex's package isn't installed).
    #[default]
    Missing,
    /// There, and not this account's to read.
    Unreadable,
    Text(String),
}

/// What a host says of itself, asked as one account ([`PROBE`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// `uname -s`.
    pub os: String,
    /// `/etc/os-release`'s `ID`.
    pub distro: String,
    pub user: String,
    pub uid: u32,
    pub groups: Vec<String>,
    /// The `rex` an SSH command finds: on the PATH, or in `~/.local/bin`.
    pub rex: Option<String>,
    /// Its `rex version`'s first line ("rex 0.1.0-snapshot.2506").
    pub version: Option<String>,
    /// A copy in `/usr/local/bin` (installed alone, without services).
    pub bare: bool,
    /// Rex's package is installed (its system units are there).
    pub packaged: bool,
    /// systemd runs the machine.
    pub systemd: bool,
    pub shared: Unit,
    pub shared_socket: Unit,
    pub login: Unit,
    pub login_socket: Unit,
    /// The account's own `rex.socket` (a user unit).
    pub own: Unit,
    /// The account's units run while it is logged out.
    pub linger: bool,
    /// The `rex` account's shell, if there is such an account.
    pub rex_shell: Option<String>,
    pub fetch: Option<Fetch>,
    pub accounts: AccountsSeen,
}

/// The script a host answers with: `key=value` lines, read by
/// [`Facts::parse`]. POSIX sh; it changes nothing.
pub const PROBE: &str = r#"os=$(uname -s); printf 'os=%s\n' "$os"
[ -r /etc/os-release ] && . /etc/os-release; printf 'distro=%s\n' "${ID:-}"
printf 'user=%s\n' "$(id -un)"; printf 'uid=%s\n' "$(id -u)"; printf 'groups=%s\n' "$(id -Gn)"
rex=$(command -v rex 2>/dev/null); [ -z "$rex" ] && [ -x "$HOME/.local/bin/rex" ] && rex="$HOME/.local/bin/rex"
printf 'rex=%s\n' "$rex"
[ -n "$rex" ] && printf 'version=%s\n' "$("$rex" version 2>/dev/null | head -n 1)"
[ -x /usr/local/bin/rex ] && echo bare=1
{ [ -e /usr/lib/systemd/system/rex-login-server.service ] || [ -e /lib/systemd/system/rex-login-server.service ]; } && echo packaged=1
if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
  echo systemd=1
  for u in rex.service rex.socket rex-login-server.service rex-login-server.socket; do printf 'unit.%s=%s,%s\n' "$u" "$(systemctl is-active "$u" 2>/dev/null)" "$(systemctl is-enabled "$u" 2>/dev/null)"; done
  printf 'own=%s,%s\n' "$(systemctl --user is-active rex.socket 2>/dev/null)" "$(systemctl --user is-enabled rex.socket 2>/dev/null)"
  printf 'linger=%s\n' "$(loginctl show-user "$(id -un)" -p Linger --value 2>/dev/null)"
fi
printf 'rexshell=%s\n' "$(getent passwd rex 2>/dev/null | cut -d: -f7)"
if command -v curl >/dev/null 2>&1; then echo fetch=curl; elif command -v wget >/dev/null 2>&1; then echo fetch=wget; fi
if [ -r /etc/rex/accounts.jsonc ]; then printf 'accounts=%s\n' "$(base64 < /etc/rex/accounts.jsonc | tr -d '\n')"; elif [ -e /etc/rex/accounts.jsonc ]; then echo accounts=unreadable; fi
true
"#;

impl Facts {
    /// What [`PROBE`] printed.
    pub fn parse(output: &str) -> Result<Facts, String> {
        let mut facts = Facts::default();
        let unit = |v: &str| {
            let (active, enabled) = v.split_once(',').unwrap_or((v, ""));
            // ("enabled-runtime", "static", "alias" and the rest aren't "to be started".)
            Unit { active: active == "active", enabled: enabled == "enabled" }
        };
        let mut seen_user = false;
        for line in output.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim();
            match key.trim() {
                "os" => facts.os = value.to_owned(),
                "distro" => facts.distro = value.to_owned(),
                "user" => {
                    facts.user = value.to_owned();
                    seen_user = true;
                }
                "uid" => facts.uid = value.parse().map_err(|_| format!("the host gave {value:?} for a user id"))?,
                "groups" => facts.groups = value.split_whitespace().map(str::to_owned).collect(),
                "rex" => facts.rex = (!value.is_empty()).then(|| value.to_owned()),
                "version" => facts.version = (!value.is_empty()).then(|| value.to_owned()),
                "bare" => facts.bare = value == "1",
                "packaged" => facts.packaged = value == "1",
                "systemd" => facts.systemd = value == "1",
                "unit.rex.service" => facts.shared = unit(value),
                "unit.rex.socket" => facts.shared_socket = unit(value),
                "unit.rex-login-server.service" => facts.login = unit(value),
                "unit.rex-login-server.socket" => facts.login_socket = unit(value),
                "own" => facts.own = unit(value),
                "linger" => facts.linger = value == "yes",
                "rexshell" => facts.rex_shell = (!value.is_empty()).then(|| value.to_owned()),
                "fetch" => {
                    facts.fetch = match value {
                        "curl" => Some(Fetch::Curl),
                        "wget" => Some(Fetch::Wget),
                        _ => None,
                    }
                }
                "accounts" if value == "unreadable" => facts.accounts = AccountsSeen::Unreadable,
                "accounts" => {
                    let bytes = B64.decode(value).map_err(|_| "the host's accounts file didn't come through whole".to_owned())?;
                    facts.accounts = AccountsSeen::Text(String::from_utf8_lossy(&bytes).into_owned());
                }
                _ => {}
            }
        }
        if !seen_user || facts.user.is_empty() || facts.os.is_empty() {
            return Err("the host didn't answer the questions asked of it".into());
        }
        Ok(facts)
    }

    /// The channel its Rex is of, if its version says ("snapshot",
    /// "preview"; a release says none).
    pub fn channel(&self) -> Option<&'static str> {
        let version = self.version.as_deref()?;
        ["snapshot", "preview"].into_iter().find(|c| version.contains(c))
    }

    /// How Rex runs there now, as far as its units say: the login server
    /// or the shared one if either is in use, else the account's own
    /// where there is a Rex at all.
    pub fn mode(&self) -> Option<ServerMode> {
        if self.login.used() || self.login_socket.used() {
            Some(ServerMode::Login)
        } else if self.shared.used() || self.shared_socket.used() {
            Some(ServerMode::Shared)
        } else {
            self.rex.is_some().then_some(ServerMode::Personal)
        }
    }

    /// Whether the accounts file lets this account in as itself (by its
    /// user id on the machine); none if it can't be read.
    pub fn listed(&self) -> Option<bool> {
        match &self.accounts {
            AccountsSeen::Missing => Some(false),
            AccountsSeen::Unreadable => None,
            AccountsSeen::Text(text) => {
                let file = Accounts::parse(text).ok()?;
                let principal = self.principal();
                Some(file.accounts().iter().any(|a| a.name == self.user && a.principals.iter().any(|p| *p == principal)))
            }
        }
    }

    /// How the login server knows this account on its own machine.
    pub fn principal(&self) -> String {
        format!("peer:{}", self.uid)
    }

    /// Its own server starts at boot and stays while it is logged out.
    pub fn own_server_stays(&self) -> bool {
        self.own.enabled && self.linger
    }
}

/// What a step does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Does {
    /// A shell command.
    Run(String),
    /// The login server's accounts file changed: `account` let in by
    /// `principal`, and given it when it asks for no account.
    List { account: String, principal: String },
}

/// One thing to do on a host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// As root; else as the account that connects.
    pub root: bool,
    /// What it is, in a few words ("Install Rex’s package"): its line in
    /// the list of steps, beside the bar while it runs, and after
    /// "Couldn’t" if it fails.
    pub title: String,
    /// What it is for, where its title doesn't say (or empty).
    pub what: String,
    pub does: Does,
}

impl Step {
    /// What is run, as it is shown before it is run: the command; or,
    /// for the accounts file ([`Step::adds_to`]), the entries added.
    pub fn shown(&self) -> String {
        match &self.does {
            Does::Run(command) => command.clone(),
            Does::List { account, principal } => {
                format!("\"accounts\": {{ {}: {{ \"principals\": [{}] }} }},\n\"defaults\": {{ {}: {} }}", json(account), json(principal), json(principal), json(account))
            }
        }
    }

    /// The file `shown` is added to, where it isn't a command.
    pub fn adds_to(&self) -> Option<&'static str> {
        matches!(self.does, Does::List { .. }).then_some(ACCOUNTS_FILE)
    }

    /// Whether it ends sessions that are running there: the shared
    /// server stopped, whose sessions are its own. (The login server
    /// stopped ends none: each account's server outlives it.)
    pub fn ends_sessions(&self) -> bool {
        matches!(&self.does, Does::Run(c) if c.starts_with("systemctl disable --now rex.socket rex.service"))
    }

    /// What to be afraid of in it, said apart from what it is for.
    pub fn warning(&self) -> Option<&'static str> {
        self.ends_sessions().then_some("Every session open on the shared server ends.")
    }

    /// The service it starts or has read its files again, whose own
    /// log says why if it won't.
    pub fn service(&self) -> Option<&'static str> {
        let Does::Run(command) = &self.does else { return None };
        if !command.starts_with("systemctl enable --now ") && !command.starts_with("systemctl reload ") {
            return None;
        }
        ["rex-login-server.service", "rex.service"].into_iter().find(|unit| command.ends_with(unit))
    }
}

fn json(s: &str) -> String {
    serde_json::Value::String(s.to_owned()).to_string()
}

fn user(title: &str, what: &str, command: impl Into<String>) -> Step {
    Step { root: false, title: title.to_owned(), what: what.to_owned(), does: Does::Run(command.into()) }
}

fn root(title: &str, what: &str, command: impl Into<String>) -> Step {
    Step { root: true, title: title.to_owned(), what: what.to_owned(), does: Does::Run(command.into()) }
}

/// The installer run with `options` (after `sh -s --`; none: as it is).
fn installer(fetch: Fetch, options: &str) -> String {
    let get = match fetch {
        Fetch::Curl => format!("curl -fsSL {INSTALLER}"),
        Fetch::Wget => format!("wget -qO- {INSTALLER}"),
    };
    if options.is_empty() { format!("{get} | sh") } else { format!("{get} | sh -s -- {options}") }
}

/// What `facts`' host needs done for Rex to run there in `mode` and for
/// this account to use it: nothing, if it is so already. Or why it can't
/// be done from here.
///
/// A change of mode ends sessions: the shared server's when the login
/// server takes its place, and the other way about ([`ends_sessions`]).
pub fn plan(mode: ServerMode, facts: &Facts) -> Result<Vec<Step>, String> {
    if facts.os != "Linux" {
        return Err(format!("This sets Rex up on Linux. That machine runs {}: see Rex's documentation for it.", if facts.os.is_empty() { "something else" } else { &facts.os }));
    }
    if facts.uid == 0 {
        return Err("Connect as an account of your own, not as root: root is asked for only to set the server up.".into());
    }
    let mut steps = Vec::new();
    let needs_fetch = |what: &str| facts.fetch.ok_or_else(|| format!("That machine has neither curl nor wget to fetch {what} with. Install one of them there first."));
    // The account's own server, at boot and while it is logged out: by
    // the `rex` that will be there then (the package's, where a server
    // that needs it is set up; else the one found, or the account's own
    // install).
    let rex_then = if mode == ServerMode::Personal || (facts.packaged && !facts.bare) { facts.rex.clone().unwrap_or_else(|| "~/.local/bin/rex".into()) } else { "/usr/bin/rex".to_owned() };
    let own_server = |steps: &mut Vec<Step>| {
        if facts.systemd && !facts.own_server_stays() {
            steps.push(user("Keep your Rex server running", "It starts when the machine starts, and stays while you are logged out.", format!("{rex_then} server install")));
        }
    };
    if mode == ServerMode::Personal {
        if facts.rex.is_none() {
            steps.push(user("Install Rex for your account", "Rex’s own installer puts it in ~/.local/bin.", installer(needs_fetch("Rex’s installer")?, "--user")));
        }
        own_server(&mut steps);
        return Ok(steps);
    }
    if !facts.systemd {
        return Err(format!("The {} server needs systemd, which doesn't run that machine. Rex's documentation says how to set it up with OpenRC.", mode.name().to_lowercase()));
    }
    // The package, and one copy of Rex.
    if !facts.packaged {
        // (The channel it has is kept: a copy installed alone records
        // none, and the installer would take its default.)
        let options = facts.channel().map(|c| format!("--channel {c}")).unwrap_or_default();
        steps.push(root(
            "Install Rex’s package",
            "Rex’s installer adds Rex’s repository to the system’s package manager and installs the rex package, so Rex is updated with the rest of the system.",
            installer(needs_fetch("Rex’s installer")?, &options),
        ));
    }
    if facts.bare {
        steps.push(root("Remove the other copy of Rex", "Removes the copy in /usr/local/bin, so only the package’s Rex runs.", "rm -f /usr/local/bin/rex"));
    }
    match mode {
        ServerMode::Login => {
            let listed = facts.listed() == Some(true);
            if !listed {
                steps.push(Step {
                    root: true,
                    title: format!("Allow {} to sign in", facts.user),
                    what: format!("The login server lets in only the accounts its file lists. Adds these entries to {ACCOUNTS_FILE}:"),
                    does: Does::List { account: facts.user.clone(), principal: facts.principal() },
                });
            }
            if facts.shared.used() || facts.shared_socket.used() {
                steps.push(root("Stop the shared server", "The login server takes its place.", "systemctl disable --now rex.socket rex.service"));
            }
            if !(facts.login.used() || facts.login_socket.used()) || !(facts.login.active || facts.login_socket.active) {
                steps.push(root("Start the login server", "Now, and whenever the machine starts.", "systemctl enable --now rex-login-server.service"));
            } else if !listed && facts.login.active {
                steps.push(root("Reload the login server", "It reads its accounts again.", "systemctl reload rex-login-server.service"));
            }
            own_server(&mut steps);
        }
        ServerMode::Shared => {
            // (The package makes the account with no shell, and every
            // terminal on the shared server would end as it began.)
            if !facts.packaged || facts.rex_shell.as_deref().is_none_or(|s| s.ends_with("nologin") || s.ends_with("false")) {
                steps.push(root("Give the rex account a shell", "Terminals on the shared server run as rex, which the package makes without one.", "usermod --shell /bin/bash rex"));
            }
            if facts.login.used() || facts.login_socket.used() {
                steps.push(root(
                    "Stop the login server",
                    "The shared server takes its place, in a /run/rex cleared of what the login server left. Each account’s own server keeps running.",
                    "systemctl disable --now rex-login-server.socket rex-login-server.service && rm -rf /run/rex",
                ));
            }
            if !(facts.shared.used() || facts.shared_socket.used()) || !(facts.shared.active || facts.shared_socket.active) {
                steps.push(root("Start the shared server", "Now, and whenever the machine starts.", "systemctl enable --now rex.service"));
            }
            if !facts.groups.iter().any(|g| g == "rex") {
                steps.push(root(
                    &format!("Add {} to the rex group", facts.user),
                    "Members of the rex group may use the shared server.",
                    format!("usermod --append --groups rex {}", shell_word(&facts.user)),
                ));
            }
        }
        ServerMode::Personal => unreachable!("handled above"),
    }
    Ok(steps)
}

/// Whether carrying `steps` out ends sessions that are running there (a
/// server stopped for another to take its place).
pub fn ends_sessions(steps: &[Step]) -> bool {
    steps.iter().any(Step::ends_sessions)
}

/// The address to reach Rex at on `target`'s host in `mode`: the
/// account's own server, or the shared server's socket.
pub fn address(mode: ServerMode, target: &SshTarget) -> String {
    let mut target = target.clone();
    target.socket_path = (mode == ServerMode::Shared).then(|| SHARED_SOCKET.to_owned());
    target.display()
}

/// `s` as one word of a shell command.
pub fn shell_word(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-./@:+".contains(&b)) { s.to_owned() } else { format!("'{}'", s.replace('\'', "'\\''")) }
}

/// What a command gave back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ran {
    /// Its exit status (none: it ended by a signal, or the connection did).
    pub status: Option<u32>,
    /// What it wrote, both of its outputs, in order.
    pub output: String,
}

impl Ran {
    pub fn ok(&self) -> bool {
        self.status == Some(0)
    }
}

/// Why something on a host wasn't done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failed {
    /// The host let nobody in as the one asked for with the keys offered
    /// (root, nearly always: another key may do, [`OverSsh::root_key`]).
    pub refused: bool,
    pub why: String,
}

impl From<String> for Failed {
    fn from(why: String) -> Failed {
        Failed { refused: false, why }
    }
}

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.why)
    }
}

/// What runs commands on the host: as the account that connects, or as
/// root. Each line a command writes goes to `said` as it comes.
pub trait Runner {
    fn run(&mut self, root: bool, command: &str, said: &mut (dyn FnMut(&str) + Send)) -> impl std::future::Future<Output = Result<Ran, Failed>> + Send;

    /// Root's connection is let go of: what is run next signs in
    /// anew. For looking at what only a new sign-in is given (its
    /// limits: a connection keeps the ones it began with).
    fn afresh(&mut self) -> impl std::future::Future<Output = ()> + Send {
        async {}
    }
}

/// How root's commands reach root on a host (Ben, 2026-11-24: "it
/// shouldn't only work with a root@ login it should also support using
/// any provided user account e.g. fedora@ that has the ability to run
/// commands as sudo without a password").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RootBy {
    /// Signed in as `root@HOST`, with keys alone (the agent's, or the one
    /// the person chose).
    #[default]
    Login,
    /// Over the account's own connection, with `sudo -n`: an account that
    /// may run commands as root without a password ([`can_sudo`]).
    Sudo,
}

/// `command` as root runs it through sudo, over the account's
/// connection: never asking for a password (`-n`: one asked for fails
/// at once), with root's home as its home and where it starts (`-H`,
/// and `cd`), as a `root@` login's command would. Shown to the person
/// as it is run: the confirm list's wells have it whole.
pub fn by_sudo(command: &str) -> String {
    format!("sudo -n -H sh -c {}", shell_word(&format!("cd && {command}")))
}

/// Whether the account that connects may run commands as root with sudo,
/// without a password (`sudo -n true`; an account that is root itself
/// needn't). Asked only where root's work is to be done: a sudo refused
/// may be logged there, or mailed to the host's administrators.
pub async fn can_sudo(runner: &mut impl Runner) -> bool {
    runner.run(false, "sudo -n true", &mut |_| {}).await.is_ok_and(|ran| ran.ok())
}

/// Commands run over SSH: as the account of `target`, and as root on the
/// same host, as `root_by` says: signed in as root with keys alone (the
/// agent's; `root_key`, the one the person chose, by its fingerprint), or
/// with sudo over the account's own connection.
pub struct OverSsh {
    pub pool: std::sync::Arc<crate::ssh::SshSessionPool>,
    pub target: SshTarget,
    pub root_key: Option<String>,
    pub root_by: RootBy,
    pub askpass: Option<std::sync::Arc<dyn crate::ssh::SshAskpass>>,
}

impl Runner for OverSsh {
    async fn run(&mut self, root: bool, command: &str, said: &mut (dyn FnMut(&str) + Send)) -> Result<Ran, Failed> {
        let (target, command) = match (root, self.root_by) {
            (true, RootBy::Login) => (self.target.as_root(self.root_key.clone()), command.to_owned()),
            (true, RootBy::Sudo) => (self.target.clone(), by_sudo(command)),
            (false, _) => (self.target.clone(), command.to_owned()),
        };
        self.pool.exec(&target, self.askpass.clone(), &command, said).await.map_err(|e| match e {
            crate::error::RexError::SshRefused(who) => Failed { refused: true, why: format!("{who} let nobody in with the keys offered") },
            other => Failed { refused: false, why: other.to_string() },
        })
    }

    async fn afresh(&mut self) {
        // (Through sudo, each command is a new session of root's, given
        // what a new sign-in is: there is no connection of root's.)
        if self.root_by == RootBy::Login {
            self.pool.close(&self.target.as_root(self.root_key.clone())).await;
        }
    }
}

/// The host's facts, asked as the account or as root.
pub async fn probe(runner: &mut impl Runner, root: bool) -> Result<Facts, Failed> {
    let ran = runner.run(root, &format!("sh -c {}", shell_word(PROBE)), &mut |_| {}).await?;
    if !ran.ok() {
        return Err(format!("the host couldn't be asked how it stands ({})", ran.output.lines().last().unwrap_or("no answer").trim()).into());
    }
    Ok(Facts::parse(&ran.output)?)
}

/// Does one step. What it wrote is `said` line by line; a step that
/// fails says why (its last lines).
pub async fn carry_out(runner: &mut impl Runner, step: &Step, said: &mut (dyn FnMut(&str) + Send)) -> Result<(), Failed> {
    match &step.does {
        Does::Run(command) => {
            let ran = runner.run(step.root, command, said).await?;
            if ran.ok() { Ok(()) } else { Err(failure(&ran).into()) }
        }
        Does::List { account, principal } => {
            let mut file = read_accounts(runner).await?;
            let mut changed = file.allow(account, principal)?;
            // (Asking for no account, it gets its own.)
            if !file.defaults().iter().any(|(p, _)| p == principal) {
                changed |= file.set_default(principal, Some(account))?;
            }
            if changed {
                write_accounts(runner, &file, said).await?;
            }
            Ok(())
        }
    }
}

/// What a host needs done for Rex to run there in `mode`, asked as the
/// account: how it stands, and the steps (none: it is as it should be).
pub async fn needs(runner: &mut impl Runner, mode: ServerMode) -> Result<(Facts, Vec<Step>), Failed> {
    let facts = probe(runner, false).await?;
    let steps = plan(mode, &facts)?;
    Ok((facts, steps))
}

/// How setting a host up ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every step done, and the host, looked at again, needs nothing.
    Done,
    /// Stopped by the person before step `before` (those before it are
    /// done, and stay done).
    Stopped { before: usize },
    /// Step `at` didn't finish.
    Failed { at: usize, failed: Failed },
    /// Every step ran, and the host still says it needs these.
    Left(Vec<Step>),
}

/// Carries `steps` out in turn, saying before each which it is
/// (`progress(at, None)`) and each line it writes (`progress(at,
/// Some(line))`), then looks at the host again. `stop` is looked at
/// between steps, never in one: a package half installed is worse than
/// one installed.
pub async fn set_up(runner: &mut impl Runner, mode: ServerMode, steps: &[Step], stop: &std::sync::atomic::AtomicBool, progress: &mut (dyn FnMut(usize, Option<&str>) + Send)) -> Outcome {
    for (at, step) in steps.iter().enumerate() {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            return Outcome::Stopped { before: at };
        }
        progress(at, None);
        if let Err(mut failed) = carry_out(runner, step, &mut |line| progress(at, Some(line))).await {
            // (systemd says only that a service failed, and where to
            // look: its log's last lines are looked up for the person.)
            if let Some(unit) = step.service().filter(|_| !failed.refused)
                && let Ok(log) = runner.run(true, &format!("journalctl -u {unit} -n 12 --no-pager -o cat 2>&1"), &mut |_| {}).await
                && log.ok()
                && !log.output.trim().is_empty()
            {
                failed.why = log.output.trim().to_owned();
            }
            return Outcome::Failed { at, failed };
        }
    }
    match needs(runner, mode).await {
        Ok((_, left)) if left.is_empty() => Outcome::Done,
        Ok((_, left)) => Outcome::Left(left),
        Err(failed) => Outcome::Failed { at: steps.len(), failed },
    }
}

fn failure(ran: &Ran) -> String {
    let last: Vec<&str> = ran.output.lines().map(str::trim).filter(|l| !l.is_empty()).rev().take(3).collect();
    let why = last.into_iter().rev().collect::<Vec<_>>().join(" ");
    match (ran.status, why.is_empty()) {
        (Some(status), true) => format!("it ended with status {status}"),
        (Some(_), false) => why,
        (None, true) => "the connection ended before it finished".into(),
        (None, false) => format!("the connection ended before it finished ({why})"),
    }
}

/// The login server's accounts file, read as root (as the package ships
/// it, where there is none yet).
pub async fn read_accounts(runner: &mut impl Runner) -> Result<Accounts, Failed> {
    let ran = runner.run(true, &format!("if [ -e {ACCOUNTS_FILE} ]; then base64 < {ACCOUNTS_FILE} | tr -d '\\n'; fi"), &mut |_| {}).await?;
    if !ran.ok() {
        return Err(format!("the accounts file couldn't be read ({})", failure(&ran)).into());
    }
    let text = ran.output.trim();
    if text.is_empty() {
        return Ok(Accounts::parse(crate::accounts::EMPTY)?);
    }
    let bytes = B64.decode(text).map_err(|_| "the accounts file didn't come through whole".to_owned())?;
    Ok(Accounts::parse(&String::from_utf8_lossy(&bytes)).map_err(|e| format!("{ACCOUNTS_FILE} isn't an accounts file Rex would read ({e}); it is left as it is"))?)
}

/// Writes the accounts file (beside it, then moved over it: never half
/// written), and has a running login server read it.
pub async fn write_accounts(runner: &mut impl Runner, file: &Accounts, said: &mut (dyn FnMut(&str) + Send)) -> Result<(), Failed> {
    let command = format!(
        "umask 022 && mkdir -p /etc/rex && printf %s {} | base64 -d > {ACCOUNTS_FILE}.new && mv {ACCOUNTS_FILE}.new {ACCOUNTS_FILE} && if systemctl is-active --quiet rex-login-server.service 2>/dev/null; then systemctl reload rex-login-server.service; fi",
        shell_word(&B64.encode(file.text()))
    );
    let ran = runner.run(true, &command, said).await?;
    if ran.ok() { Ok(()) } else { Err(format!("the accounts file couldn't be written ({})", failure(&ran)).into()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host as the real one answered (2026-11-22, an Ubuntu machine
    /// with a copy of Rex in /usr/local/bin and nothing else), and then
    /// as it answered once set up.
    const BARE: &str = "os=Linux\ndistro=ubuntu\nuser=alice\nuid=1001\ngroups=alice\nrex=/usr/local/bin/rex\nversion=rex 0.1.0-snapshot.2506\nbare=1\nsystemd=1\nunit.rex.service=inactive,not-found\nunit.rex.socket=inactive,not-found\nunit.rex-login-server.service=inactive,not-found\nunit.rex-login-server.socket=inactive,not-found\nown=inactive,disabled\nlinger=no\nrexshell=\nfetch=curl\n";
    const LOGIN: &str = "os=Linux\ndistro=ubuntu\nuser=alice\nuid=1001\ngroups=alice\nrex=/usr/bin/rex\nversion=rex 0.1.0-snapshot.2506\npackaged=1\nsystemd=1\nunit.rex.service=inactive,disabled\nunit.rex.socket=inactive,disabled\nunit.rex-login-server.service=active,enabled\nunit.rex-login-server.socket=inactive,disabled\nown=active,enabled\nlinger=yes\nrexshell=/usr/sbin/nologin\nfetch=curl\n";

    fn listed(facts: &str, account: &str, uid: u32) -> String {
        let mut file = Accounts::parse(crate::accounts::EMPTY).unwrap();
        file.allow(account, &format!("peer:{uid}")).unwrap();
        format!("{facts}accounts={}\n", B64.encode(file.text()))
    }

    fn shown(steps: &[Step]) -> Vec<String> {
        steps.iter().map(|s| format!("{}: {}", if s.root { "root" } else { "user" }, s.shown())).collect()
    }

    #[test]
    fn a_host_says_how_it_stands() {
        let bare = Facts::parse(BARE).unwrap();
        assert_eq!((bare.user.as_str(), bare.uid, bare.rex.as_deref(), bare.bare, bare.packaged, bare.systemd), ("alice", 1001, Some("/usr/local/bin/rex"), true, false, true));
        assert_eq!(bare.channel(), Some("snapshot"));
        assert_eq!(bare.mode(), Some(ServerMode::Personal));
        assert_eq!(bare.listed(), Some(false));
        assert!(!bare.own_server_stays());
        let login = Facts::parse(&listed(LOGIN, "alice", 1001)).unwrap();
        assert_eq!(login.mode(), Some(ServerMode::Login));
        assert_eq!(login.listed(), Some(true));
        assert!(login.own_server_stays() && login.packaged && !login.bare);
        assert_eq!(login.rex_shell.as_deref(), Some("/usr/sbin/nologin"));
        // Listed for another's user id isn't listed; an unreadable file isn't known.
        assert_eq!(Facts::parse(&listed(LOGIN, "alice", 1002)).unwrap().listed(), Some(false));
        assert_eq!(Facts::parse(&format!("{LOGIN}accounts=unreadable\n")).unwrap().listed(), None);
        // What isn't an answer.
        assert!(Facts::parse("").is_err());
        assert!(Facts::parse("bash: no such thing\n").is_err());
        assert!(Facts::parse(&BARE.replace("uid=1001", "uid=many")).is_err());
        // No Rex at all, nothing to say of a mode.
        let none = Facts::parse("os=Linux\ndistro=debian\nuser=bob\nuid=1000\ngroups=bob\nrex=\nsystemd=1\nfetch=wget\n").unwrap();
        assert_eq!((none.rex.clone(), none.mode(), none.channel(), none.fetch), (None, None, None, Some(Fetch::Wget)));
    }

    #[test]
    fn what_each_mode_needs_of_a_host_with_only_the_program() {
        let bare = Facts::parse(BARE).unwrap();
        assert_eq!(shown(&plan(ServerMode::Personal, &bare).unwrap()), ["user: /usr/local/bin/rex server install"]);
        assert_eq!(
            shown(&plan(ServerMode::Login, &bare).unwrap()),
            [
                "root: curl -fsSL https://pkgs.superlogical.com/rex/install.sh | sh -s -- --channel snapshot",
                "root: rm -f /usr/local/bin/rex",
                "root: \"accounts\": { \"alice\": { \"principals\": [\"peer:1001\"] } },\n\"defaults\": { \"peer:1001\": \"alice\" }",
                "root: systemctl enable --now rex-login-server.service",
                "user: /usr/bin/rex server install",
            ]
        );
        assert_eq!(
            shown(&plan(ServerMode::Shared, &bare).unwrap()),
            [
                "root: curl -fsSL https://pkgs.superlogical.com/rex/install.sh | sh -s -- --channel snapshot",
                "root: rm -f /usr/local/bin/rex",
                "root: usermod --shell /bin/bash rex",
                "root: systemctl enable --now rex.service",
                "root: usermod --append --groups rex alice",
            ]
        );
        assert!(!ends_sessions(&plan(ServerMode::Login, &bare).unwrap()));
    }

    #[test]
    fn every_step_has_a_short_title_and_says_what_it_adds_or_ends() {
        let bare = Facts::parse(BARE).unwrap();
        let login = Facts::parse(&listed(LOGIN, "alice", 1001)).unwrap();
        let mut all = Vec::new();
        for facts in [&bare, &login] {
            for mode in ServerMode::ALL {
                all.extend(plan(mode, facts).unwrap());
            }
        }
        assert!(all.len() > 12);
        for step in &all {
            assert!(!step.title.is_empty() && step.title.chars().count() <= 40 && !step.title.ends_with('.'), "{:?}", step.title);
            assert!(step.title.chars().next().unwrap().is_uppercase(), "{:?}", step.title);
            // (A sentence; or, where what is added follows it, one that leads to it.)
            assert!(step.what.is_empty() || step.what.ends_with('.') || (step.adds_to().is_some() && step.what.ends_with(':')), "{:?}", step.what);
        }
        let for_login = plan(ServerMode::Login, &bare).unwrap();
        assert_eq!(
            for_login.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(),
            ["Install Rex’s package", "Remove the other copy of Rex", "Allow alice to sign in", "Start the login server", "Keep your Rex server running"]
        );
        // The accounts step shows what it adds, and where; the others, commands.
        let lists: Vec<&Step> = all.iter().filter(|s| s.adds_to().is_some()).collect();
        assert!(!lists.is_empty() && lists.iter().all(|s| s.adds_to() == Some("/etc/rex/accounts.jsonc") && s.shown().lines().count() == 2));
        assert!(all.iter().filter(|s| s.adds_to().is_none()).all(|s| s.shown().lines().count() == 1));
        // The shared server stopped ends its sessions, and says so; the
        // login server stopped ends none.
        assert!(all.iter().any(|s| s.title == "Stop the login server") && all.iter().all(|s| !s.ends_sessions()));
        let shared = Facts::parse(
            &LOGIN
                .replace("unit.rex.service=inactive,disabled", "unit.rex.service=active,enabled")
                .replace("unit.rex-login-server.service=active,enabled", "unit.rex-login-server.service=inactive,disabled"),
        )
        .unwrap();
        let to_login = plan(ServerMode::Login, &shared).unwrap();
        assert_eq!(
            to_login.iter().filter(|s| s.ends_sessions()).map(|s| (s.title.as_str(), s.warning())).collect::<Vec<_>>(),
            [("Stop the shared server", Some("Every session open on the shared server ends."))]
        );
        assert!(all.iter().all(|s| s.warning().is_none()));
        // A service started or reloaded names it (its log says why it won't).
        assert_eq!(to_login.iter().filter_map(|s| s.service()).collect::<Vec<_>>(), ["rex-login-server.service"]);
        assert_eq!(all.iter().filter_map(|s| s.service()).collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>(), ["rex-login-server.service", "rex.service"]);
    }

    #[test]
    fn a_host_as_it_should_be_needs_nothing_and_a_change_of_mode_says_what_it_ends() {
        let login = Facts::parse(&listed(LOGIN, "alice", 1001)).unwrap();
        assert_eq!(plan(ServerMode::Login, &login).unwrap(), []);
        assert_eq!(plan(ServerMode::Personal, &login).unwrap(), []);
        // Another person on the same machine: listed, and their own server.
        let other = Facts::parse(&listed(
            &LOGIN
                .replace("user=alice", "user=bob")
                .replace("uid=1001", "uid=1002")
                .replace("groups=alice", "groups=bob")
                .replace("own=active,enabled", "own=inactive,disabled")
                .replace("linger=yes", "linger=no"),
            "alice",
            1001,
        ))
        .unwrap();
        assert_eq!(
            shown(&plan(ServerMode::Login, &other).unwrap()),
            [
                "root: \"accounts\": { \"bob\": { \"principals\": [\"peer:1002\"] } },\n\"defaults\": { \"peer:1002\": \"bob\" }",
                "root: systemctl reload rex-login-server.service",
                "user: /usr/bin/rex server install"
            ]
        );
        // To the shared server from the login server: it is stopped, and that ends sessions.
        let to_shared = plan(ServerMode::Shared, &login).unwrap();
        assert_eq!(
            shown(&to_shared),
            [
                "root: usermod --shell /bin/bash rex",
                "root: systemctl disable --now rex-login-server.socket rex-login-server.service && rm -rf /run/rex",
                "root: systemctl enable --now rex.service",
                "root: usermod --append --groups rex alice",
            ]
        );
        assert!(!ends_sessions(&to_shared));
        // And back: a shared server as it should be, then asked for the login server.
        let shared = Facts::parse(
            &LOGIN
                .replace("unit.rex.service=inactive,disabled", "unit.rex.service=active,enabled")
                .replace("unit.rex-login-server.service=active,enabled", "unit.rex-login-server.service=inactive,disabled")
                .replace("groups=alice", "groups=alice rex")
                .replace("/usr/sbin/nologin", "/bin/bash"),
        )
        .unwrap();
        assert_eq!(shared.mode(), Some(ServerMode::Shared));
        assert_eq!(plan(ServerMode::Shared, &shared).unwrap(), []);
        let to_login = plan(ServerMode::Login, &shared).unwrap();
        assert_eq!(
            shown(&to_login),
            [
                "root: \"accounts\": { \"alice\": { \"principals\": [\"peer:1001\"] } },\n\"defaults\": { \"peer:1001\": \"alice\" }",
                "root: systemctl disable --now rex.socket rex.service",
                "root: systemctl enable --now rex-login-server.service"
            ]
        );
        assert!(ends_sessions(&to_login));
        // The login server enabled by its socket, not yet started: as it should be.
        let on_demand = Facts::parse(&listed(
            &LOGIN
                .replace("unit.rex-login-server.service=active,enabled", "unit.rex-login-server.service=inactive,disabled")
                .replace("unit.rex-login-server.socket=inactive,disabled", "unit.rex-login-server.socket=active,enabled"),
            "alice",
            1001,
        ))
        .unwrap();
        assert_eq!(plan(ServerMode::Login, &on_demand).unwrap(), []);
    }

    #[test]
    fn what_cant_be_done_from_here_is_said() {
        let none = "os=Linux\ndistro=debian\nuser=bob\nuid=1000\ngroups=bob\nrex=\nsystemd=1\n";
        // No Rex and nothing to fetch it with.
        assert!(plan(ServerMode::Personal, &Facts::parse(none).unwrap()).unwrap_err().contains("curl nor wget"));
        // With wget, and no channel to keep.
        assert_eq!(
            shown(&plan(ServerMode::Personal, &Facts::parse(&format!("{none}fetch=wget\n")).unwrap()).unwrap()),
            ["user: wget -qO- https://pkgs.superlogical.com/rex/install.sh | sh -s -- --user", "user: ~/.local/bin/rex server install"]
        );
        assert_eq!(shown(&plan(ServerMode::Login, &Facts::parse(&format!("{none}fetch=wget\n")).unwrap()).unwrap())[0], "root: wget -qO- https://pkgs.superlogical.com/rex/install.sh | sh");
        // Another system; root as the account; no systemd for a server that needs it (the personal one still can).
        assert!(plan(ServerMode::Personal, &Facts::parse("os=FreeBSD\nuser=bob\nuid=1001\n").unwrap()).unwrap_err().contains("FreeBSD"));
        assert!(plan(ServerMode::Login, &Facts::parse(&BARE.replace("uid=1001", "uid=0").replace("user=alice", "user=root")).unwrap()).unwrap_err().contains("not as root"));
        let alpine = "os=Linux\ndistro=alpine\nuser=bob\nuid=1000\ngroups=bob\nrex=/usr/bin/rex\nversion=rex 0.1.0\nfetch=wget\n";
        assert!(plan(ServerMode::Login, &Facts::parse(alpine).unwrap()).unwrap_err().contains("systemd"));
        assert_eq!(plan(ServerMode::Personal, &Facts::parse(alpine).unwrap()).unwrap(), []);
    }

    #[test]
    fn the_address_is_the_accounts_own_server_or_the_shared_socket() {
        let target = SshTarget::parse("ssh://alice@build.example.com:2222").unwrap();
        assert_eq!(address(ServerMode::Personal, &target), "ssh://alice@build.example.com:2222");
        assert_eq!(address(ServerMode::Login, &target), "ssh://alice@build.example.com:2222");
        assert_eq!(address(ServerMode::Shared, &target), "ssh://alice@build.example.com:2222/run/rex/server.sock");
        // (A socket typed for another mode isn't kept.)
        assert_eq!(address(ServerMode::Login, &SshTarget::parse("ssh://alice@build.example.com/run/rex/server.sock").unwrap()), "ssh://alice@build.example.com");
        assert_eq!((shell_word("alice"), shell_word("a b"), shell_word("it's"), shell_word("")), ("alice".to_owned(), "'a b'".to_owned(), "'it'\\''s'".to_owned(), "''".to_owned()));
    }

    /// A host made up: what is run on it, kept, and its accounts file.
    #[derive(Default)]
    struct Host {
        ran: Vec<(bool, String)>,
        accounts: Option<String>,
        fails: Option<&'static str>,
        /// What it says of itself, each time it is asked (the last, from then on).
        says: Vec<String>,
        /// Root isn't let in.
        no_root: bool,
        /// A service won't start (what systemctl says), and its log.
        wont_start: Option<&'static str>,
        journal: Option<&'static str>,
        /// The account may use sudo without a password.
        sudo: bool,
    }

    impl Runner for Host {
        async fn run(&mut self, root: bool, command: &str, said: &mut (dyn FnMut(&str) + Send)) -> Result<Ran, Failed> {
            if root && self.no_root {
                return Err(Failed { refused: true, why: "root@host let nobody in with the keys offered".into() });
            }
            if !root && command == "sudo -n true" {
                self.ran.push((root, command.to_owned()));
                return Ok(if self.sudo { Ran { status: Some(0), output: String::new() } } else { Ran { status: Some(1), output: "sudo: a password is required\n".into() } });
            }
            if command.starts_with("sh -c '") && !self.says.is_empty() {
                let says = if self.says.len() > 1 { self.says.remove(0) } else { self.says[0].clone() };
                return Ok(Ran { status: Some(0), output: says });
            }
            self.ran.push((root, command.to_owned()));
            if command.starts_with("journalctl ") {
                return Ok(Ran { status: Some(if self.journal.is_some() { 0 } else { 1 }), output: self.journal.unwrap_or("No journal files were found.").to_owned() });
            }
            if let Some(why) = self.wont_start.filter(|_| command.starts_with("systemctl enable --now ")) {
                said(why);
                return Ok(Ran { status: Some(1), output: format!("{why}\n") });
            }
            if let Some(why) = self.fails.filter(|_| !command.starts_with("if [ -e") && command.contains("apt-get")) {
                said(why);
                return Ok(Ran { status: Some(1), output: format!("{why}\n") });
            }
            if command.starts_with("if [ -e /etc/rex/accounts.jsonc ]") {
                return Ok(Ran { status: Some(0), output: self.accounts.as_deref().map(|text| B64.encode(text)).unwrap_or_default() });
            }
            if let Some(rest) = command.strip_prefix("umask 022 && mkdir -p /etc/rex && printf %s ") {
                let encoded = rest.split_whitespace().next().unwrap().trim_matches('\'');
                self.accounts = Some(String::from_utf8(B64.decode(encoded).unwrap()).unwrap());
            }
            said("done");
            Ok(Ran { status: Some(0), output: "done\n".into() })
        }
    }

    fn block<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
    }

    /// Root's work through sudo: asked once, as the account, never with a
    /// password; a command for root sent as sudo's, from root's home, its
    /// words whole however they're quoted.
    #[test]
    fn root_is_reached_through_sudo_where_the_account_may_use_it() {
        let mut host = Host { sudo: true, ..Host::default() };
        assert!(block(can_sudo(&mut host)));
        assert_eq!(host.ran, [(false, "sudo -n true".to_owned())]);
        let mut host = Host::default();
        assert!(!block(can_sudo(&mut host)));
        assert_eq!(by_sudo("systemctl enable --now 'rex.socket'"), "sudo -n -H sh -c 'cd && systemctl enable --now '\\''rex.socket'\\'''");
        assert_eq!(RootBy::default(), RootBy::Login);
    }

    #[test]
    fn the_accounts_step_changes_the_file_in_place_once() {
        let step = Step { root: true, title: String::new(), what: String::new(), does: Does::List { account: "alice".into(), principal: "peer:1001".into() } };
        // No file yet (the package not there): made as shipped, with the account.
        let mut host = Host::default();
        let mut lines = Vec::new();
        block(carry_out(&mut host, &step, &mut |l| lines.push(l.to_owned()))).unwrap();
        let file = Accounts::parse(host.accounts.as_deref().unwrap()).unwrap();
        assert_eq!(file.accounts().iter().map(|a| (a.name.as_str(), a.principals.clone())).collect::<Vec<_>>(), [("alice", vec!["peer:1001".to_owned()])]);
        assert_eq!(file.defaults(), [("peer:1001".to_owned(), "alice".to_owned())]);
        assert!(host.ran.iter().all(|(root, _)| *root) && host.ran.len() == 2);
        // Done again: read, found as it should be, not written.
        host.ran.clear();
        block(carry_out(&mut host, &step, &mut |_| {})).unwrap();
        assert_eq!(host.ran.len(), 1);
        // An administrator's file: its comments kept, the account added.
        let mut host = Host {
            accounts: Some("// ours\n{\n  \"version\": 1,\n  \"accounts\": {\n    \"root\": { \"principals\": [\"tailscale:admin@example.com\"] }, // careful\n  },\n}\n".into()),
            ..Default::default()
        };
        block(carry_out(&mut host, &step, &mut |_| {})).unwrap();
        let text = host.accounts.unwrap();
        assert!(text.contains("// ours") && text.contains("// careful") && text.contains("\"alice\": { \"principals\": [\"peer:1001\"] },"), "{text}");
        // A file that isn't one is left, and said.
        let mut host = Host { accounts: Some("not json".into()), ..Default::default() };
        let why = block(carry_out(&mut host, &step, &mut |_| {})).unwrap_err().why;
        assert!(why.contains("left as it is"), "{why}");
        assert_eq!(host.accounts.as_deref(), Some("not json"));
    }

    #[test]
    fn a_host_is_set_up_step_by_step_and_looked_at_again() {
        use std::sync::atomic::AtomicBool;
        // A host with only the program, asked for the login server: after
        // its steps it answers as one set up.
        let mut host = Host { says: vec![BARE.to_owned(), listed(LOGIN, "alice", 1001)], ..Default::default() };
        let (facts, steps) = block(needs(&mut host, ServerMode::Login)).unwrap();
        assert_eq!((facts.user.as_str(), steps.len()), ("alice", 5));
        let mut seen = Vec::new();
        let outcome = block(set_up(&mut host, ServerMode::Login, &steps, &AtomicBool::new(false), &mut |at, line| seen.push((at, line.map(str::to_owned)))));
        assert_eq!(outcome, Outcome::Done);
        // Each step announced, then what it wrote; in order.
        assert_eq!(seen.iter().filter(|(_, line)| line.is_none()).map(|(at, _)| *at).collect::<Vec<_>>(), [0, 1, 2, 3, 4]);
        assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0));
        // As root where the step says, as the account for its own server.
        assert_eq!(
            host.ran.iter().map(|(root, c)| (*root, c.split_whitespace().next().unwrap().to_owned())).collect::<Vec<_>>(),
            [(true, "curl".into()), (true, "rm".into()), (true, "if".into()), (true, "umask".into()), (true, "systemctl".into()), (false, "/usr/bin/rex".into())]
        );
        assert!(Accounts::parse(host.accounts.as_deref().unwrap()).unwrap().accounts().iter().any(|a| a.name == "alice"));

        // Stopped by the person: between steps, and what is done stays.
        let mut host = Host { says: vec![BARE.to_owned()], ..Default::default() };
        let stop = AtomicBool::new(false);
        let outcome = block(set_up(&mut host, ServerMode::Login, &steps, &stop, &mut |at, line| {
            if at == 1 && line.is_none() {
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }));
        assert_eq!(outcome, Outcome::Stopped { before: 2 });
        assert_eq!(host.ran.len(), 2);

        // A step that fails: which, and why; nothing after it is run.
        let mut host = Host { says: vec![BARE.to_owned()], fails: Some("E: Unable to locate package rex"), ..Default::default() };
        let failing = [root("", "", "apt-get install rex"), root("", "", "systemctl enable --now rex-login-server.service")];
        assert_eq!(
            block(set_up(&mut host, ServerMode::Login, &failing, &AtomicBool::new(false), &mut |_, _| {})),
            Outcome::Failed { at: 0, failed: Failed { refused: false, why: "E: Unable to locate package rex".into() } }
        );
        assert_eq!(host.ran.len(), 1);

        // Root not let in: said as refused, at the first step that is root's.
        let mut host = Host { says: vec![BARE.to_owned()], no_root: true, ..Default::default() };
        let outcome = block(set_up(&mut host, ServerMode::Login, &steps, &AtomicBool::new(false), &mut |_, _| {}));
        assert!(matches!(&outcome, Outcome::Failed { at: 0, failed } if failed.refused), "{outcome:?}");

        // Every step ran and the host still wants something: said, not passed over.
        let mut host = Host { says: vec![BARE.to_owned()], ..Default::default() };
        let outcome = block(set_up(&mut host, ServerMode::Personal, &[user("", "", "true")], &AtomicBool::new(false), &mut |_, _| {}));
        assert!(matches!(&outcome, Outcome::Left(left) if left.len() == 1), "{outcome:?}");
    }

    #[test]
    fn a_service_that_wont_start_is_asked_why_in_its_own_log() {
        use std::sync::atomic::AtomicBool;
        let says = "Job for rex-login-server.service failed because the control process exited with error code.";
        let start = [root("Start the login server", "", "systemctl enable --now rex-login-server.service")];
        // Its log's last lines are what is said, where there are any.
        let mut host = Host {
            says: vec![BARE.to_owned()],
            wont_start: Some(says),
            journal: Some("loading accounts file: /etc/rex/accounts.jsonc: unexpected end of input\nrex-login-server.service: Failed with result 'exit-code'.\n"),
            ..Default::default()
        };
        let outcome = block(set_up(&mut host, ServerMode::Login, &start, &AtomicBool::new(false), &mut |_, _| {}));
        assert_eq!(
            outcome,
            Outcome::Failed {
                at: 0,
                failed: Failed { refused: false, why: "loading accounts file: /etc/rex/accounts.jsonc: unexpected end of input\nrex-login-server.service: Failed with result 'exit-code'.".into() }
            }
        );
        assert_eq!(
            host.ran.iter().map(|(root, c)| (*root, c.as_str())).collect::<Vec<_>>(),
            [(true, "systemctl enable --now rex-login-server.service"), (true, "journalctl -u rex-login-server.service -n 12 --no-pager -o cat 2>&1")]
        );
        // No log to be had: what systemctl said stands.
        let mut host = Host { says: vec![BARE.to_owned()], wont_start: Some(says), ..Default::default() };
        let outcome = block(set_up(&mut host, ServerMode::Login, &start, &AtomicBool::new(false), &mut |_, _| {}));
        assert_eq!(outcome, Outcome::Failed { at: 0, failed: Failed { refused: false, why: says.into() } });
        // A step that starts no service asks no log.
        let mut host = Host { says: vec![BARE.to_owned()], fails: Some("E: Unable to locate package rex"), journal: Some("unrelated"), ..Default::default() };
        block(set_up(&mut host, ServerMode::Login, &[root("", "", "apt-get install rex")], &AtomicBool::new(false), &mut |_, _| {}));
        assert_eq!(host.ran.len(), 1);
    }

    #[test]
    fn a_step_that_fails_says_why_in_its_own_words() {
        let mut host = Host { fails: Some("E: Unable to locate package rex"), ..Default::default() };
        let why = block(carry_out(&mut host, &root("", "", "apt-get install rex"), &mut |_| {})).unwrap_err();
        assert_eq!(why, Failed { refused: false, why: "E: Unable to locate package rex".into() });
        assert_eq!(failure(&Ran { status: Some(3), output: String::new() }), "it ended with status 3");
        assert_eq!(failure(&Ran { status: None, output: "half\n".into() }), "the connection ended before it finished (half)");
        // The probe, asked through a runner.
        struct Says(&'static str);
        impl Runner for Says {
            async fn run(&mut self, _: bool, command: &str, _: &mut (dyn FnMut(&str) + Send)) -> Result<Ran, Failed> {
                assert!(command.starts_with("sh -c '"));
                Ok(Ran { status: Some(0), output: self.0.to_owned() })
            }
        }
        assert_eq!(block(probe(&mut Says(BARE), false)).unwrap().user, "alice");
        assert!(block(probe(&mut Says("garbage"), false)).is_err());
    }
}
