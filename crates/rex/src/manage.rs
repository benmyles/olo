//! A host's Rex server looked after from here, as root over SSH: how it
//! stands (which kind of server runs there, for how long, in which
//! version, who may use it), and the few things done to it (a service
//! started, stopped or restarted; an account let in or taken out; Rex
//! brought up to date). Ben, 2026-11-22: "build in rex server management
//! to our rex client so that i can manage the users, services etc. of
//! the running rex server (if i have a root@ key)".
//!
//! It stands on [`provision`](crate::provision): the same runner over
//! SSH (root signed in with keys alone, the agent's or one chosen), the
//! same reading of a host's facts, and the same accounts file, changed
//! in place ([`Accounts`]). Everything it does is one [`Action`], which
//! says what it is, what it runs, exactly, and what it costs (a shared
//! server stopped ends every session on it) before it is done; after
//! each the host is looked at again, so what the window shows is what
//! the host says, never what was hoped. What it knows of Rex's servers
//! is from Rex's own documentation (Server Modes, Login Server, Shared
//! Server, Upgrading).

use crate::accounts::Accounts;
use crate::provision::{self, Facts, Failed, Ran, Runner, ServerMode, Unit, shell_word};

/// The login server's unit, and the shared server's.
pub const LOGIN_SERVICE: &str = "rex-login-server.service";
pub const SHARED_SERVICE: &str = "rex.service";

/// One of Rex's system services on the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Service {
    /// Every account its own server, behind one door: `rex-login-server`.
    Login,
    /// One server for everybody in the `rex` group: `rex`.
    Shared,
}

impl Service {
    pub fn unit(self) -> &'static str {
        match self {
            Service::Login => LOGIN_SERVICE,
            Service::Shared => SHARED_SERVICE,
        }
    }

    /// Its socket's unit, which starts it when somebody connects.
    pub fn socket(self) -> &'static str {
        match self {
            Service::Login => "rex-login-server.socket",
            Service::Shared => "rex.socket",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Service::Login => "Login server",
            Service::Shared => "Shared server",
        }
    }

    pub fn mode(self) -> ServerMode {
        match self {
            Service::Login => ServerMode::Login,
            Service::Shared => ServerMode::Shared,
        }
    }

    /// Whether stopping or restarting it ends the sessions on it. The
    /// shared server's sessions are its own; the login server only
    /// lets accounts in to their own servers, which go on without it
    /// (Rex's documentation, Upgrading, "What Restarts").
    pub fn holds_sessions(self) -> bool {
        self == Service::Shared
    }
}

/// An account on the host that a person signs in to (not a service's).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub name: String,
    pub uid: u32,
}

/// How a host's Rex server stands, asked as root ([`look`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Server {
    /// The host's facts, as root sees them.
    pub facts: Facts,
    /// For how many seconds each service has run, if it does (by the
    /// host's own clock: how long the machine has been up, less when
    /// in that the service began).
    pub login_up: Option<u64>,
    pub shared_up: Option<u64>,
    /// Whether each service ended by itself, badly (systemd's "failed");
    /// then how many seconds ago, and the last thing it wrote (its own
    /// words for why, as a rule).
    pub login_failed: bool,
    pub shared_failed: bool,
    pub login_ended: Option<u64>,
    pub shared_ended: Option<u64>,
    pub login_last: Option<String>,
    pub shared_last: Option<String>,
    /// The sessions open there now: on each account's own server that
    /// runs (a login server's accounts), and on the shared server (as
    /// `rex`, the account it runs as). An account that isn't here has
    /// no server running.
    pub sessions: Vec<(String, usize)>,
    /// Whether the host answered the question of sessions at all.
    pub asked_sessions: bool,
    /// The `rex` group's members: who may use the shared server.
    pub members: Vec<String>,
    /// The people with accounts on the host.
    pub people: Vec<Person>,
    /// The package's version there, and a newer one its repository
    /// offers, where the host's package manager says.
    pub installed: Option<String>,
    pub newer: Option<String>,
    /// Whether Rex's repository was asked just now ([`look`] with
    /// `check`): then no `newer` means there is none, not that nobody
    /// has looked.
    pub checked: bool,
    /// The login server's accounts file, if it could be read as one.
    pub accounts: Option<Accounts>,
    /// Why it couldn't, if it is there and isn't one.
    pub accounts_trouble: Option<String>,
    /// How the host's limits stand ([`crate::tune`]), where it said.
    pub tuning: Option<crate::tune::Tuning>,
}

impl Server {
    /// The kind of server that runs there for everybody, if one does.
    pub fn mode(&self) -> Option<ServerMode> {
        match (self.state(Service::Login).used(), self.state(Service::Shared).used()) {
            (true, _) => Some(ServerMode::Login),
            (false, true) => Some(ServerMode::Shared),
            (false, false) => None,
        }
    }

    /// A service's state: running if it or its socket is (a socket
    /// that waits is a server that answers), kept if either is enabled.
    pub fn state(&self, service: Service) -> Unit {
        let (unit, socket) = match service {
            Service::Login => (self.facts.login, self.facts.login_socket),
            Service::Shared => (self.facts.shared, self.facts.shared_socket),
        };
        Unit { active: unit.active || socket.active, enabled: unit.enabled || socket.enabled }
    }

    pub fn up(&self, service: Service) -> Option<u64> {
        match service {
            Service::Login => self.login_up,
            Service::Shared => self.shared_up,
        }
    }

    pub fn failed(&self, service: Service) -> bool {
        match service {
            Service::Login => self.login_failed,
            Service::Shared => self.shared_failed,
        }
    }

    /// How long ago a failed service ended, in seconds, and the last
    /// thing it wrote.
    pub fn ended(&self, service: Service) -> Option<u64> {
        match service {
            Service::Login => self.login_ended,
            Service::Shared => self.shared_ended,
        }
    }

    pub fn last_words(&self, service: Service) -> Option<&str> {
        match service {
            Service::Login => self.login_last.as_deref(),
            Service::Shared => self.shared_last.as_deref(),
        }
    }

    /// The sessions open on `account`'s own server, if it runs.
    pub fn sessions_of(&self, account: &str) -> Option<usize> {
        self.sessions.iter().find(|(name, _)| name == account).map(|(_, n)| *n)
    }

    /// The sessions open through a service: the shared server's own,
    /// or those of every account's server behind the login server.
    /// None where the host couldn't say.
    pub fn sessions_on(&self, service: Service) -> Option<usize> {
        match service {
            Service::Shared => self.sessions_of("rex"),
            Service::Login if self.sessions.is_empty() && !self.asked_sessions => None,
            Service::Login => Some(self.sessions.iter().filter(|(name, _)| name != "rex").map(|(_, n)| n).sum()),
        }
    }

    /// Rex's version there as it is shown: the package's own, the one
    /// its package manager and its repository speak of (`rex version`
    /// spells the same build otherwise: "0.1.0-snapshot.2506" is the
    /// package "0.1.0~alpha.2506"); `rex version`'s, where Rex isn't
    /// a package there.
    pub fn version(&self) -> Option<String> {
        self.installed.clone().or_else(|| self.facts.version.as_deref().map(|v| v.strip_prefix("rex ").unwrap_or(v).trim().to_owned()))
    }

    /// The people who aren't in the `rex` group yet.
    pub fn not_members(&self) -> Vec<&Person> {
        self.people.iter().filter(|p| !self.members.contains(&p.name)).collect()
    }

    /// The people the login server's accounts file doesn't list.
    pub fn not_listed(&self) -> Vec<&Person> {
        let listed: Vec<String> = self.accounts.as_ref().map(|a| a.accounts().into_iter().map(|a| a.name).collect()).unwrap_or_default();
        self.people.iter().filter(|p| !listed.contains(&p.name)).collect()
    }
}

/// What more is asked of the host than [`provision::PROBE`] asks: `key=value`
/// lines, read by [`Server::add`]. POSIX sh, as root. (Each Rex server
/// that runs is asked for its sessions as the account it runs as, at
/// the socket it was told to listen on where it was told one: the
/// shared server's isn't where the `rex` account's own would be.) It
/// changes nothing
/// of the host's; with `CHECK` set before it, it has the package
/// manager ask Rex's own repository what is there (apt: that source's
/// lists alone, read again), so that a newer version is known of.
pub const MORE: &str = r#"for u in rex-login-server.service rex.service; do
  if systemctl is-active --quiet "$u" 2>/dev/null; then printf 'began.%s=%s\n' "$u" "$(systemctl show -p ActiveEnterTimestampMonotonic --value "$u" 2>/dev/null)"; fi
  if systemctl is-failed --quiet "$u" 2>/dev/null; then
    printf 'failed.%s=1\n' "$u"
    printf 'ended.%s=%s\n' "$u" "$(systemctl show -p InactiveEnterTimestampMonotonic --value "$u" 2>/dev/null)"
    printf 'last.%s=%s\n' "$u" "$(journalctl -u "$u" -t rex -n 1 --no-pager -o cat 2>/dev/null | tail -n 1)"
  fi
done
printf 'uptime=%s\n' "$(cut -d' ' -f1 /proc/uptime 2>/dev/null)"
printf 'members=%s\n' "$(getent group rex 2>/dev/null | cut -d: -f4)"
getent passwd 2>/dev/null | awk -F: '$3 >= 1000 && $3 < 60000 && $7 !~ /(nologin|false)$/ { printf "person=%s:%s\n", $1, $3 }'
if command -v runuser >/dev/null 2>&1; then
  printf 'sessions.asked=1\n'
  ps -eo user:32=,args= 2>/dev/null | awk '$2 ~ /(^|\/)rex$/ && $3 == "server" && $4 == "run" { at = ""; for (i = 5; i < NF; i++) if ($i == "--listen") at = $(i + 1); print $1, at }' | sort -u | while read -r u at; do
    if open=$(runuser -u "$u" -- rex --autostart=false --timeout 5s ${at:+-S "$at"} ls -q 2>/dev/null); then printf 'sessions=%s:%s\n' "$u" "$(printf '%s' "$open" | grep -c .)"; fi
  done
fi
if command -v dpkg-query >/dev/null 2>&1; then
  if [ -n "${CHECK:-}" ]; then
    src=$(grep -rl 'pkgs.superlogical.com/rex' /etc/apt/sources.list.d 2>/dev/null | head -n 1)
    if [ -n "$src" ] && apt-get update -o Dir::Etc::sourcelist="$src" -o Dir::Etc::sourceparts=- -o APT::Get::List-Cleanup=0 >/dev/null 2>&1; then printf 'checked=1\n'; fi
  fi
  printf 'installed=%s\n' "$(dpkg-query -W -f='${Version}' rex 2>/dev/null)"
  printf 'candidate=%s\n' "$(apt-cache policy rex 2>/dev/null | sed -n 's/^ *Candidate: //p')"
elif command -v rpm >/dev/null 2>&1; then
  printf 'installed=%s\n' "$(rpm -q --qf '%{VERSION}-%{RELEASE}' rex 2>/dev/null | grep -v 'not installed')"
fi
true
"#;

impl Server {
    /// What [`MORE`] printed, added to the facts.
    fn add(&mut self, output: &str) {
        let mut candidate = None;
        let (mut began, mut ended, mut uptime) = ((None, None), (None, None), None);
        for line in output.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim();
            match key {
                "began.rex-login-server.service" => began.0 = micros(value),
                "began.rex.service" => began.1 = micros(value),
                "uptime" => uptime = value.parse::<f64>().ok(),
                "failed.rex-login-server.service" => self.login_failed = true,
                "failed.rex.service" => self.shared_failed = true,
                "ended.rex-login-server.service" => ended.0 = micros(value),
                "ended.rex.service" => ended.1 = micros(value),
                "last.rex-login-server.service" => self.login_last = last_words(value),
                "last.rex.service" => self.shared_last = last_words(value),
                "sessions.asked" => self.asked_sessions = true,
                "sessions" => {
                    if let Some((name, open)) = value.rsplit_once(':')
                        && let Ok(open) = open.parse()
                    {
                        self.sessions.push((name.to_owned(), open));
                    }
                }
                "checked" => self.checked = true,
                "members" => self.members = value.split(',').map(str::trim).filter(|m| !m.is_empty()).map(str::to_owned).collect(),
                "person" => {
                    if let Some((name, uid)) = value.rsplit_once(':')
                        && let Ok(uid) = uid.parse()
                    {
                        self.people.push(Person { name: name.to_owned(), uid });
                    }
                }
                "installed" if !value.is_empty() => self.installed = Some(value.to_owned()),
                "candidate" if !value.is_empty() && value != "(none)" => candidate = Some(value.to_owned()),
                _ => {}
            }
        }
        self.people.sort_by_key(|person| person.uid);
        let up = |began: Option<u64>| Some((uptime? - began? as f64 / 1e6).max(0.0) as u64);
        (self.login_up, self.shared_up) = (up(began.0), up(began.1));
        (self.login_ended, self.shared_ended) = (up(ended.0), up(ended.1));
        self.newer = candidate.filter(|c| self.installed.as_deref().is_some_and(|i| i != c));
    }
}

/// The last line a service wrote, as its state's line says it: without
/// the time Rex writes first, and without the level it writes next
/// ("2026/11/23 01:06:20 ERROR the rex account has no shell": "the rex
/// account has no shell").
fn last_words(line: &str) -> Option<String> {
    let said = own_time_off(line.trim());
    let said = ["ERROR", "FATAL", "WARN", "INFO", "DEBUG", "error:"].iter().find_map(|level| said.strip_prefix(level)).map_or(said, str::trim_start);
    (!said.is_empty()).then(|| said.to_owned())
}

/// A count of microseconds as systemd prints one (alone, or after its
/// property's name where `--value` isn't known).
fn micros(value: &str) -> Option<u64> {
    value.rsplit('=').next()?.trim().parse().ok().filter(|m| *m > 0)
}

/// How the host's server stands, asked as root. `check`: Rex's
/// repository is asked what it has, too, so a newer version is known of
/// (when the window opens, and after an update: not after every
/// service started or account let in).
pub async fn look(runner: &mut impl Runner, check: bool) -> Result<Server, Failed> {
    let facts = provision::probe(runner, true).await?;
    let mut server = Server { facts, ..Server::default() };
    let script = if check { format!("CHECK=1\n{MORE}") } else { MORE.to_owned() };
    let ran = runner.run(true, &format!("sh -c {}", shell_word(&script)), &mut |_| {}).await?;
    if ran.ok() {
        server.add(&ran.output);
    }
    const TUNE_LOOK_MAX_OUTPUT: usize = 4 * 1024 * 1024;
    const TUNE_LOOK_MAX_LINE: usize = 64 * 1024;
    let scope = runner.rex_scope();
    let mode_val = match scope {
        Some(crate::tune::RexScope::Shared) => "shared",
        Some(crate::tune::RexScope::User) => "user",
        None => "skip",
    };
    let account_val = runner.account().filter(|a| !a.is_empty() && *a != "root").unwrap_or("");
    let look_script = format!("OLO_REX_MODE={mode_val}\nOLO_REX_USER={}\n{}", shell_word(account_val), crate::tune::LOOK);
    if let Ok(Ok(ran)) =
        tokio::time::timeout(std::time::Duration::from_secs(30), runner.run_bounded(true, &format!("sh -c {}", shell_word(&look_script)), TUNE_LOOK_MAX_OUTPUT, TUNE_LOOK_MAX_LINE, &mut |_| {})).await
        && ran.ok()
    {
        let mut tuning = crate::tune::Tuning::read(ran.stdout_only());
        match scope {
            None => tuning.block_rex("This host connects to a custom Rex socket; raise its limits where that server is configured."),
            Some(expected) if ran.truncated || tuning.rex_limits.scope != expected => {
                tuning.block_rex("Olo couldn’t read this host’s Rex service.");
            }
            Some(_) => {}
        }
        server.tuning = Some(tuning);
    }
    match provision::read_accounts(runner).await {
        Ok(file) => server.accounts = Some(file),
        Err(failed) if failed.refused => return Err(failed),
        Err(failed) => server.accounts_trouble = Some(failed.why),
    }
    Ok(server)
}

/// One thing done to a host's server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Started now, and whenever the machine starts.
    Start(Service),
    /// Stopped, and not started again at boot or when somebody connects.
    Stop(Service),
    Restart(Service),
    /// An account let in through the login server, as itself (its own
    /// uid on that machine), and given itself when it names no account.
    Allow(Person),
    /// An account taken out of the login server's file.
    Remove(String),
    /// A person added to the `rex` group: the shared server is theirs
    /// too, from their next sign-in.
    Join(String),
    /// Taken out of it.
    Leave(String),
    /// Rex's package brought up to date, by its installer.
    Update,
    /// Limits raised for heavy work ([`crate::tune`]): each kind named,
    /// where the host is short of it.
    Tune(Vec<crate::tune::Tune>),
    /// One kind put back as the host had it.
    Untune(crate::tune::Tune),
}

/// How many accounts a question names, of those brush would be given
/// to: past them, how many more.
const SHOWN_ACCOUNTS: usize = 6;

/// A line of what a change of the machine's settings changes, as the
/// question that asks it shows it: a table under "Now" and "New"
/// ([`Action::changes`]). It was text in columns, set whole in the
/// fixed-width face: a head that read as one more row, a tweak's name
/// set as code, and the two values a pair of spaces apart at the well's
/// left with a third of it empty beside them (both reviewers,
/// 2026-11-27).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// A name over the lines under it: a tweak's, where several are
    /// applied at once; "Accounts here".
    Group(String),
    /// One thing that changes: what it is now and what it will be.
    /// `plain`: its name is words (a tweak's own, "New accounts"), not
    /// a setting's as the host has it; `inside`: under a group;
    /// `number`: its values are amounts, set to the right edge of their
    /// columns (words and paths begin at the left of theirs).
    Value { name: String, now: String, new: String, plain: bool, inside: bool, number: bool },
    /// Words under the lines: "and 12 more accounts".
    Note(String),
}

impl Action {
    /// What it is, in a few words: its button, and after "Couldn't" if
    /// it fails.
    pub fn title(&self) -> String {
        match self {
            Action::Start(s) => format!("Start the {}", s.name().to_lowercase()),
            Action::Stop(s) => format!("Stop the {}", s.name().to_lowercase()),
            Action::Restart(s) => format!("Restart the {}", s.name().to_lowercase()),
            Action::Allow(p) => format!("Allow {} to sign in", p.name),
            Action::Remove(name) => format!("Remove {name}"),
            Action::Join(name) => format!("Add {name} to the rex group"),
            Action::Leave(name) => format!("Take {name} out of the rex group"),
            Action::Update => "Update Rex".to_owned(),
            Action::Tune(kinds) if kinds.len() == 1 => format!("Apply “{}”", kinds[0].title()),
            Action::Tune(_) => "Apply the suggested settings".to_owned(),
            Action::Untune(kind) => format!("Put back “{}”", kind.title()),
        }
    }

    /// What it is while it is being done ("Stopping the shared server").
    pub fn doing(&self) -> String {
        match self {
            Action::Start(s) => format!("Starting the {}", s.name().to_lowercase()),
            Action::Stop(s) => format!("Stopping the {}", s.name().to_lowercase()),
            Action::Restart(s) => format!("Restarting the {}", s.name().to_lowercase()),
            Action::Allow(p) => format!("Allowing {} to sign in", p.name),
            Action::Remove(name) => format!("Removing {name}"),
            Action::Join(name) => format!("Adding {name} to the rex group"),
            Action::Leave(name) => format!("Taking {name} out of the rex group"),
            Action::Update => "Updating Rex".to_owned(),
            Action::Tune(kinds) if kinds.len() == 1 => format!("Applying “{}”", kinds[0].title()),
            Action::Tune(_) => "Applying the suggested settings".to_owned(),
            Action::Untune(kind) => format!("Putting back “{}”", kind.title()),
        }
    }

    /// The word on the button that does it.
    pub fn verb(&self) -> &'static str {
        match self {
            Action::Start(_) => "Start",
            Action::Stop(_) => "Stop",
            Action::Restart(_) => "Restart",
            Action::Allow(_) => "Allow",
            Action::Remove(_) | Action::Leave(_) => "Remove",
            Action::Join(_) => "Add",
            Action::Update => "Update",
            Action::Tune(_) => "Apply",
            Action::Untune(_) => "Put Back",
        }
    }

    /// What it does, in a sentence, where its title doesn't say it all:
    /// said before it is done, over what it costs.
    pub fn what(&self, server: &Server) -> String {
        self.what_on(server, None)
    }

    /// Like [`Self::what`], naming `host` where a shared server's scope is
    /// stated ("everyone on dev-box shares").
    pub fn what_on(&self, server: &Server, host: Option<&str>) -> String {
        let where_on = host.filter(|h| !h.is_empty()).unwrap_or("this host");
        match self {
            Action::Start(_) => "It starts now, and whenever the machine starts.".to_owned(),
            Action::Stop(_) => "It stays stopped, even when the machine restarts, until you start it here.".to_owned(),
            Action::Restart(Service::Login) => "Open sessions keep running.".to_owned(),
            Action::Restart(Service::Shared) => String::new(),
            Action::Allow(p) => format!("Their sessions will run as {}, with that account’s own files and rights.", p.name),
            Action::Remove(_) => String::new(),
            Action::Join(name) => format!("Then {name} can use the shared server, the next time they sign in. Every terminal on it runs as the rex account."),
            Action::Leave(name) => format!("{name} can no longer use the shared server, the next time they sign in. Their open sessions keep running until they end."),
            Action::Update => {
                let replaces = match (&server.installed, &server.newer) {
                    (Some(installed), Some(_)) => format!("This replaces {installed}."),
                    _ => "Rex’s installer installs the newest version there is.".to_owned(),
                };
                if server.state(Service::Shared).active { replaces } else { format!("{replaces} Open sessions keep running.") }
            }
            // (One kind: what applying it does besides, then how it is
            // kept. Several: said once for all of them; each kind's
            // own note, one after another, was a wall of words over
            // the list of what changes.)
            Action::Tune(kinds) => match kinds.as_slice() {
                [crate::tune::Tune::Shell] => {
                    format!(
                        "{} It can be put back with Put Back… in Tweaks: new accounts get the shell they were given before, and each account Rex changed gets its own again.",
                        crate::tune::Tune::Shell.note().unwrap_or_default()
                    )
                }
                [crate::tune::Tune::RexLimits] => {
                    let tuning = server.tuning.as_ref();
                    let shared = tuning.is_some_and(|t| t.rex_limits.scope == crate::tune::RexScope::Shared);
                    let user = tuning.and_then(|t| t.rex_limits.user.as_deref()).filter(|a| !a.is_empty() && *a != "you");
                    let stale_only = tuning.is_some_and(|t| t.rex_limits.stale && t.of(crate::tune::Tune::RexLimits).met());
                    let pending = tuning.is_some_and(|t| t.rex_limits_pending_restart().is_some());
                    let pending_note = if pending { " The table shows what is configured for the next start; the running server still holds its earlier limits." } else { "" };
                    match (shared, stale_only, user) {
                        (true, true, _) => format!(
                            "Rex’s service changed; Olo writes its limit file again from the new one, keeping your limits. This changes the Rex server everyone on {where_on} shares. They take effect when it next starts. Restarting it ends the programs in everyone’s tabs, so Rex leaves that to you: restart it from the Server window when you’re ready.{pending_note}"
                        ),
                        (true, false, _) => format!(
                            "This changes the Rex server everyone on {where_on} shares. The new limits take effect when it next starts. Restarting it ends the programs in everyone’s tabs, so Rex leaves that to you: restart it from the Server window when you’re ready. You can put the old limits back with Put Back… in Tweaks.{pending_note}"
                        ),
                        (false, true, Some(acct)) => format!(
                            "Rex’s service changed; Olo writes {acct}’s limit file again from the new one, keeping your limits. Only {acct}’s Rex server on this host is changed; other accounts aren’t touched. Restarting ends every tab’s program.{pending_note}\nTo restart: systemctl --user restart rex.service"
                        ),
                        (false, true, None) => format!(
                            "Rex’s service changed; Olo writes your limit file again from the new one, keeping your limits. Only your Rex server on this host is changed; other accounts aren’t touched. Restarting ends every tab’s program.{pending_note}\nTo restart: systemctl --user restart rex.service"
                        ),
                        (false, false, Some(acct)) => format!(
                            "Only {acct}’s Rex server on this host is changed; other accounts aren’t touched. The new limits take effect when it next starts. You can put the old limits back with Put Back… in Tweaks. Restarting ends every tab’s program.{pending_note}\nTo restart: systemctl --user restart rex.service"
                        ),
                        (false, false, None) => format!(
                            "Only your Rex server on this host is changed; other accounts aren’t touched. The new limits take effect when it next starts. You can put the old limits back with Put Back… in Tweaks. Restarting ends every tab’s program.{pending_note}\nTo restart: systemctl --user restart rex.service"
                        ),
                    }
                }
                [kind] => format!("{} The new values are kept across reboots and can be put back with Put Back… in Tweaks.", kind.note().unwrap_or_default()).trim().to_owned(),
                kinds => {
                    let mut said = Vec::new();
                    if kinds.contains(&crate::tune::Tune::Sessions) {
                        said.push("Connections already open aren’t touched.".to_owned());
                    }
                    if kinds.iter().any(|kind| matches!(kind, crate::tune::Tune::Files | crate::tune::Tune::TempFiles)) {
                        said.push("Open files and temporary files take effect from each program’s next sign-in or start.".to_owned());
                    }
                    if kinds.contains(&crate::tune::Tune::RexLimits) {
                        let rex_note = match server.tuning.as_ref() {
                            Some(t) if t.rex_limits.scope == crate::tune::RexScope::User => match t.rex_limits.user.as_deref().filter(|a| !a.is_empty() && *a != "you") {
                                Some(acct) => format!("{acct}’s Rex server gets its new limits when it next starts. Restarting it ends every tab’s program, so it isn’t restarted for you."),
                                None => "Your Rex server gets its new limits when it next starts. Restarting it ends every tab’s program, so it isn’t restarted for you.".to_owned(),
                            },
                            Some(_) => format!(
                                "Rex’s server, shared by everyone on {where_on}, gets its new limits when it next starts. Restarting it ends the programs in everyone’s tabs, so it isn’t restarted for you."
                            ),
                            None => "Rex’s server gets its new limits when it next starts. Restarting it ends every tab’s program, so it isn’t restarted for you.".to_owned(),
                        };
                        said.push(rex_note);
                    }
                    said.push("Everything is kept across reboots and can be put back with Put Back… in Tweaks. Nothing already set higher is lowered.".to_owned());
                    said.join(" ")
                }
            },
            // (Of brush itself, what is true on this host: taken off, or
            // kept for who still signs in to it. It said "stays
            // installed while an account still signs in to it" over a
            // well that removed it: Opus's look.)
            Action::Untune(crate::tune::Tune::Shell) => {
                let back = "New accounts get the shell they were given before, and each account Rex changed gets its own again.";
                match server.tuning.as_ref() {
                    Some(tuning) if !tuning.brush_goes() => {
                        let users: Vec<&str> = tuning.brush_users.iter().filter(|user| !tuning.was_accounts.iter().any(|(name, _)| name == *user)).map(String::as_str).collect();
                        format!("{back} Brush stays installed: {} still {} it.", users.join(", "), if users.len() == 1 { "uses" } else { "use" })
                    }
                    _ => format!("{back} Brush is then taken off."),
                }
            }
            Action::Untune(crate::tune::Tune::TempFiles) => "Programs use /tmp again, from their next sign-in or start.".to_owned(),
            Action::Untune(crate::tune::Tune::RexLimits) => match server.tuning.as_ref() {
                Some(t) if t.rex_limits.scope == crate::tune::RexScope::User => {
                    let mut msg = match t.rex_limits.user.as_deref().filter(|a| !a.is_empty() && *a != "you") {
                        Some(acct) => format!(
                            "Only {acct}’s limit file is removed; other accounts aren’t touched. {acct}’s Rex server goes back to its own limits when it next starts. The running server isn’t restarted."
                        ),
                        None => "Only your limit file is removed; other accounts aren’t touched. Your Rex server goes back to its own limits when it next starts. The running server isn’t restarted."
                            .to_owned(),
                    };
                    if t.rex_limits.kept_sys {
                        msg.push_str(&format!(" The shared server’s limit file ({}) stays as it is.", crate::tune::REX_SYSTEM_DROPIN));
                    }
                    msg
                }
                Some(t) => {
                    let mut msg = "Olo’s file is removed, and the shared Rex server goes back to its limits when it next starts. It isn’t restarted.".to_owned();
                    if t.rex_limits.kept_usr {
                        match t.rex_limits.user.as_deref().filter(|a| !a.is_empty() && *a != "you") {
                            Some(acct) => msg.push_str(&format!(" {acct}’s own Rex server keeps its limits.")),
                            None => msg.push_str(" Your own Rex server keeps its limits."),
                        }
                    }
                    msg
                }
                None => "Olo’s file is removed, and Rex goes back to its limits when it next starts. It isn’t restarted.".to_owned(),
            },
            Action::Untune(kind) => format!("Olo’s {} taken away, and the host’s own settings again.", if kind.files().len() == 1 { "file is" } else { "files are" }),
        }
    }

    /// What it costs, where it costs something: said before it is done.
    pub fn warning(&self, server: &Server) -> Option<String> {
        match self {
            Action::Stop(s) | Action::Restart(s) if s.holds_sessions() && server.state(*s).active => {
                Some("This ends every session on the shared server, and every program running in them.".to_owned())
            }
            Action::Stop(Service::Login) => Some("Nobody can sign in through it until it is started again. Open sessions keep running.".to_owned()),
            Action::Remove(name) => Some(format!("{name} can no longer sign in through the login server. Their open sessions keep running until they end.")),
            // (Only what it can cost: what it merely does is said
            // with the rest, in `what`.)
            Action::Tune(kinds) => kinds.iter().find_map(|k| k.cost()).map(str::to_owned),
            Action::Update if server.state(Service::Shared).active => Some("Updating restarts the shared server, which ends every session on it and every program running in them.".to_owned()),
            _ => None,
        }
    }

    /// What is run on the host, as root, as it is shown before it is
    /// run: a command; or, for the accounts file, what is changed in it.
    pub fn shown(&self, server: &Server) -> String {
        match self {
            Action::Allow(p) => format!("\"accounts\": {{ \"{0}\": {{ \"principals\": [\"peer:{1}\"] }} }}\n\"defaults\": {{ \"peer:{1}\": \"{0}\" }}", p.name, p.uid),
            Action::Remove(name) => format!("\"{name}\" is taken out of \"accounts\" and \"defaults\""),
            // (What changes, every setting, from what the host has to
            // what it will, in columns: the script that does it is a
            // page long. Several kinds: each under its name.)
            Action::Tune(kinds) => {
                let Some(tuning) = &server.tuning else { return String::new() };
                let short: Vec<(crate::tune::Tune, Vec<(&str, String, String)>)> = kinds
                    .iter()
                    .map(|kind| {
                        // (Brush: who is given it, each with the shell
                        // it has now; past a handful, how many more.)
                        if *kind == crate::tune::Tune::Shell && !tuning.of(*kind).met() {
                            let mut rows = vec![("new accounts", tuning.default_shell.clone().unwrap_or_else(|| "?".to_owned()), crate::tune::BRUSH.to_owned())];
                            if tuning.everyone {
                                rows.extend(tuning.accounts.iter().take(SHOWN_ACCOUNTS).map(|(name, shell)| (name.as_str(), shell.clone(), crate::tune::BRUSH.to_owned())));
                            }
                            return (*kind, rows);
                        }
                        if *kind == crate::tune::Tune::RexLimits && tuning.rex_limits.blocked.is_some() {
                            return (*kind, Vec::new());
                        }
                        if *kind == crate::tune::Tune::RexLimits && tuning.of(*kind).met() && tuning.rex_limits.stale {
                            return (*kind, vec![("Rex service file", "out of date".to_owned(), "updated".to_owned())]);
                        }
                        let rows = tuning
                            .of(*kind)
                            .values
                            .iter()
                            .filter(|v| !v.met())
                            .map(|v| (v.name, v.now.map_or("?".to_owned(), |now| crate::tune::said(v.name, now)), crate::tune::said(v.name, v.wanted)));
                        (*kind, rows.collect())
                    })
                    .filter(|(_, rows): &(_, Vec<_>)| !rows.is_empty())
                    .collect();
                // (Several kinds: a kind that is one setting is one
                // line, by the kind's own name; a kind of several has
                // its name over them, and they stand a step in.)
                let several = short.len() > 1;
                // (Temporary files' setting has no name of the host's:
                // by the tweak's own, alone too.)
                let one = |kind: &crate::tune::Tune| (several && tuning.of(*kind).values.len() == 1 && *kind != crate::tune::Tune::Shell) || *kind == crate::tune::Tune::TempFiles;
                let short: Vec<(crate::tune::Tune, Vec<(&str, String, String)>)> =
                    short.into_iter().map(|(kind, rows)| if one(&kind) { (kind, rows.into_iter().map(|(_, a, b)| (kind.title(), a, b)).collect()) } else { (kind, rows) }).collect();
                let step = |kind: &crate::tune::Tune| if several && !one(kind) { 2 } else { 0 };
                // (Shells are paths: begun at one edge, not ended at one. And the
                // two columns a step further apart than a name is from them: two
                // spaces between "524,288" and "1,048,576" read as one number.)
                let paths = short.iter().all(|(kind, _)| *kind == crate::tune::Tune::Shell);
                let cell = |text: &str, wide: usize| if paths { format!("{text:<wide$}") } else { format!("{text:>wide$}") };
                let wide = |of: &dyn Fn(&crate::tune::Tune, &(&str, String, String)) -> usize, least: usize| {
                    short.iter().flat_map(|(kind, rows)| rows.iter().map(move |row| of(kind, row))).fold(least, usize::max)
                };
                let (name, now, new) = (wide(&|kind, r| step(kind) + r.0.chars().count(), 0), wide(&|_, r| r.1.chars().count(), 3), wide(&|_, r| r.2.chars().count(), 3));
                let mut lines = vec![format!("{:name$}  {}    {}", "", cell("now", now), cell("new", new))];
                for (kind, rows) in &short {
                    let step = step(kind);
                    if step > 0 {
                        lines.push(kind.title().to_owned());
                    }
                    let room = name - step;
                    lines.extend(rows.iter().map(|(n, a, b)| format!("{:step$}{n:room$}  {}    {}", "", cell(a, now), cell(b, new))));
                    if *kind == crate::tune::Tune::Shell && tuning.everyone && tuning.accounts.len() > SHOWN_ACCOUNTS {
                        lines.push(format!("{:step$}and {} more accounts", "", tuning.accounts.len() - SHOWN_ACCOUNTS));
                    }
                }
                lines.join("\n")
            }
            Action::Untune(crate::tune::Tune::RexLimits) => match server.tuning.as_ref() {
                Some(t) if t.rex_limits.scope == crate::tune::RexScope::User => t.rex_limits.file_usr.clone().unwrap_or_else(|| crate::tune::REX_USER_DROPIN_FALLBACK.to_owned()),
                _ => crate::tune::REX_SYSTEM_DROPIN.to_owned(),
            },
            Action::Untune(kind) => kind.files().join("\n"),
            other => other.command(server).unwrap_or_default(),
        }
    }

    /// What it changes, line by line, for the kinds of action that
    /// change the machine's settings: every setting from what the host
    /// has to what it will have. None for the rest, which show what
    /// they run ([`Action::shown`]).
    pub fn changes(&self, server: &Server) -> Option<Vec<Change>> {
        use crate::tune::{BRUSH, TEMP_DIR, Tune};
        let tuning = server.tuning.as_ref()?;
        let value = |name: &str, now: String, new: String, plain: bool, inside: bool, number: bool| Change::Value { name: name.to_owned(), now, new, plain, inside, number };
        let unset = || "not set".to_owned();
        match self {
            Action::Tune(kinds) => {
                let short: Vec<Tune> = kinds
                    .iter()
                    .copied()
                    .filter(|kind| {
                        if *kind == Tune::RexLimits && tuning.rex_limits.blocked.is_some() {
                            return false;
                        }
                        !tuning.of(*kind).met() || (*kind == Tune::RexLimits && tuning.rex_limits.stale)
                    })
                    .collect();
                let several = short.len() > 1;
                let mut lines = Vec::new();
                for kind in short {
                    let stands = tuning.of(kind);
                    let amounts =
                        || stands.values.iter().filter(|v| !v.met()).map(|v| (v.name, v.now.map_or("?".to_owned(), |now| crate::tune::said(v.name, now)), crate::tune::said(v.name, v.wanted)));
                    match kind {
                        // Brush: who is given it. New accounts; and, chosen, the
                        // accounts there are, each with the shell it has.
                        Tune::Shell => {
                            lines.push(value("New accounts", tuning.default_shell.clone().unwrap_or_else(|| "?".to_owned()), BRUSH.to_owned(), true, false, false));
                            if tuning.everyone && !tuning.accounts.is_empty() {
                                lines.push(Change::Group("Accounts here".to_owned()));
                                lines.extend(tuning.accounts.iter().take(SHOWN_ACCOUNTS).map(|(name, shell)| value(name, shell.clone(), BRUSH.to_owned(), false, true, false)));
                                if tuning.accounts.len() > SHOWN_ACCOUNTS {
                                    lines.push(Change::Note(format!("and {} more accounts", tuning.accounts.len() - SHOWN_ACCOUNTS)));
                                }
                            }
                        }
                        // Rex server limits drop-in gone stale after a Rex upgrade,
                        // with all four numeric limits already met.
                        Tune::RexLimits if stands.met() && tuning.rex_limits.stale => {
                            let label = if several { kind.title() } else { "Rex service file" };
                            lines.push(value(label, "out of date".to_owned(), "updated".to_owned(), true, false, false));
                        }
                        // Temporary files, by itself: each variable Rex sets. Among
                        // others: one line of it, by its own name.
                        Tune::TempFiles if !several => lines.extend(tuning.temp.iter().map(|(name, now)| value(name, now.clone().unwrap_or_else(unset), TEMP_DIR.to_owned(), false, false, false))),
                        Tune::TempFiles => lines.extend(amounts().map(|(_, now, new)| value(kind.title(), now, new, true, false, true))),
                        // A kind that is one setting, among others: one line, by the
                        // kind's own name.
                        _ if several && stands.values.len() == 1 => lines.extend(amounts().map(|(_, now, new)| value(kind.title(), now, new, true, false, true))),
                        _ => {
                            if several {
                                lines.push(Change::Group(kind.title().to_owned()));
                            }
                            lines.extend(amounts().map(|(name, now, new)| value(name, now, new, false, several, true)));
                        }
                    }
                }
                Some(lines)
            }
            // Brush put back: what new accounts are given again, each
            // account Rex changed that still has it, and whether brush
            // itself goes.
            Action::Untune(Tune::Shell) => {
                let mut lines =
                    vec![value("New accounts", tuning.default_shell.clone().unwrap_or_else(|| BRUSH.to_owned()), tuning.was_default.clone().unwrap_or_else(|| "?".to_owned()), true, false, false)];
                let back: Vec<&(String, String)> = tuning.was_accounts.iter().filter(|(name, _)| tuning.brush_users.contains(name)).collect();
                if !back.is_empty() {
                    lines.push(Change::Group("Accounts Rex changed".to_owned()));
                    lines.extend(back.iter().take(SHOWN_ACCOUNTS).map(|(name, had)| value(name, BRUSH.to_owned(), had.clone(), false, true, false)));
                    if back.len() > SHOWN_ACCOUNTS {
                        lines.push(Change::Note(format!("and {} more accounts", back.len() - SHOWN_ACCOUNTS)));
                    }
                }
                lines.push(value("Brush", "installed".to_owned(), if tuning.brush_goes() { "removed".to_owned() } else { "stays".to_owned() }, true, false, false));
                Some(lines)
            }
            Action::Untune(Tune::TempFiles) => Some(tuning.temp.iter().map(|(name, now)| value(name, now.clone().unwrap_or_else(unset), unset(), false, false, false)).collect()),
            Action::Untune(Tune::RexLimits) => {
                let back = tuning.rex_limits.without_olo?;
                let stands = tuning.of(Tune::RexLimits);
                let rows: Vec<Change> = stands
                    .values
                    .iter()
                    .filter_map(|v| {
                        let new = back.get(v.name)?;
                        if v.now == Some(new) {
                            return None;
                        }
                        Some(value(v.name, v.now.map_or("?".to_owned(), |now| crate::tune::said(v.name, now)), crate::tune::said(v.name, new), false, false, true))
                    })
                    .collect();
                (!rows.is_empty()).then_some(rows)
            }
            _ => None,
        }
    }

    /// Its command, for those that are one.
    pub fn command(&self, server: &Server) -> Option<String> {
        Some(match self {
            Action::Start(s) => format!("systemctl enable --now {}", s.unit()),
            Action::Stop(s) => format!("systemctl disable --now {} {}", s.socket(), s.unit()),
            Action::Restart(s) => format!("systemctl restart {}", s.unit()),
            Action::Join(name) => format!("usermod --append --groups rex {}", shell_word(name)),
            Action::Leave(name) => format!("gpasswd --delete {} rex", shell_word(name)),
            Action::Update => {
                // (Its installer again, on the channel it came from, as
                // Rex's Upgrading page has it: it finds the host's
                // package manager and has it upgrade the package.)
                let options = server.facts.channel().map(|c| format!(" --channel {c}")).unwrap_or_default();
                let fetch = match server.facts.fetch {
                    Some(provision::Fetch::Wget) => "wget -qO-",
                    _ => "curl -fsSL",
                };
                format!("{fetch} {} | sh -s --{options}", provision::INSTALLER).trim_end_matches("--").trim().to_owned()
            }
            Action::Tune(kinds) => {
                let tuning = server.tuning.as_ref()?;
                let scripts: Vec<(crate::tune::Tune, String)> = kinds.iter().copied().filter_map(|k| k.apply(tuning).map(|s| (k, s))).collect();
                if scripts.is_empty() {
                    return None;
                }
                if scripts.len() == 1 {
                    return Some(format!("( {} )", scripts[0].1.trim_end()));
                }
                // (Each kind runs in a fresh `sh -c` shell so `set -e` inside
                // each script stays active — POSIX disables `set -e` inside
                // `if ! (...)` — and every failure is collected so a later
                // success can never hide an earlier failure.)
                let mut out = String::from("set +e; rc=0; failed=''\n");
                for (kind, script) in scripts {
                    let title = shell_word(kind.title());
                    out.push_str(&format!("sh -c {}\nif [ \"$?\" != 0 ]; then rc=1; failed=\"${{failed:+$failed, }}\"{title}; fi\n", shell_word(script.trim_end())));
                }
                out.push_str("if [ \"$rc\" != 0 ]; then printf 'Rex: Couldn’t apply: %s.\\n' \"$failed\"; exit \"$rc\"; fi");
                out
            }
            Action::Untune(kind) => {
                let mut script = kind.undo();
                if *kind == crate::tune::Tune::RexLimits
                    && let Some(tuning) = server.tuning.as_ref()
                {
                    let mode = match tuning.rex_limits.scope {
                        crate::tune::RexScope::Shared => "shared",
                        crate::tune::RexScope::User => "user",
                    };
                    let user = tuning.rex_limits.user.as_deref().filter(|u| !u.is_empty() && *u != "root").unwrap_or("");
                    script = format!("OLO_REX_MODE={mode}\nOLO_REX_USER={}\n{script}", shell_word(user));
                }
                script
            }
            Action::Allow(_) | Action::Remove(_) => return None,
        })
    }
}

/// Does `action` on the host, each line it writes `said` as it comes,
/// then looks at the host again. What didn't go says why (and, for a
/// service, what its log says).
pub async fn act(runner: &mut impl Runner, server: &Server, action: &Action, said: &mut (dyn FnMut(&str) + Send)) -> Result<Server, Failed> {
    match action {
        Action::Allow(person) => {
            let mut file = provision::read_accounts(runner).await?;
            let principal = format!("peer:{}", person.uid);
            let mut changed = file.allow(&person.name, &principal)?;
            if !file.defaults().iter().any(|(p, _)| *p == principal) {
                changed |= file.set_default(&principal, Some(&person.name))?;
            }
            if changed {
                provision::write_accounts(runner, &file, said).await?;
            }
        }
        Action::Remove(name) => {
            let mut file = provision::read_accounts(runner).await?;
            // (What it was given by default goes with it, `remove`'s
            // doing: a default that names no account is one the server
            // refuses.)
            if file.remove(name)? {
                provision::write_accounts(runner, &file, said).await?;
            }
        }
        other => {
            let touches_rex = matches!(other, Action::Untune(crate::tune::Tune::RexLimits)) || matches!(other, Action::Tune(kinds) if kinds.contains(&crate::tune::Tune::RexLimits));
            let mut effective = server.clone();
            let command = if touches_rex {
                let Some(scope) = runner.rex_scope() else {
                    return Err("This host connects to a custom Rex socket; raise its limits where that server is configured.".to_owned().into());
                };
                if let Some(tuning) = effective.tuning.as_mut() {
                    if tuning.rex_limits.scope != scope {
                        return Err("This host’s Rex server mode doesn’t match the connection; look again.".to_owned().into());
                    }
                    if let Some(acct) = runner.account().filter(|a| !a.is_empty() && *a != "root") {
                        tuning.rex_limits.user = Some(acct.to_owned());
                    }
                }
                if matches!(other, Action::Untune(crate::tune::Tune::RexLimits)) {
                    let mode = match scope {
                        crate::tune::RexScope::Shared => "shared",
                        crate::tune::RexScope::User => "user",
                    };
                    let acct = runner.account().or_else(|| effective.tuning.as_ref().and_then(|t| t.rex_limits.user.as_deref())).filter(|a| !a.is_empty() && *a != "root").unwrap_or("");
                    format!("OLO_REX_MODE={mode}\nOLO_REX_USER={}\n{}", shell_word(acct), crate::tune::Tune::RexLimits.undo())
                } else {
                    other.command(&effective).ok_or_else(|| Failed::from("nothing to run".to_owned()))?
                }
            } else {
                other.command(server).ok_or_else(|| Failed::from("nothing to run".to_owned()))?
            };
            // (A script of several lines goes to a shell as one word.)
            let command = if matches!(other, Action::Tune(_) | Action::Untune(_)) { format!("sh -c {}", shell_word(&command)) } else { command };
            let ran = runner.run(true, &command, said).await?;
            if !ran.ok() {
                // (What the host said last, a line a line: three of
                // them run into one read as one message cut in odd
                // places, "command not found sh: line 25:".)
                let mut why = last_lines(&ran, "\n");
                if let Action::Start(s) | Action::Restart(s) = other
                    && let Ok(log) = journal(runner, *s, LogRange::Last(12)).await
                    && !log.is_empty()
                {
                    why = format!("{why}\n{}", log.join("\n"));
                }
                return Err(why.into());
            }
        }
    }
    // (Limits are a sign-in's: the connection that changed them keeps
    // the ones it began with, and looked at through it the open files
    // stood as before, "1 of 3 to change" after Apply with no error.
    // Ben, 2026-11-23: "i just applied all limits but open files didn't
    // change and it didn't give me an error or anything". Looked at
    // through a new sign-in.)
    if matches!(action, Action::Tune(_) | Action::Untune(_)) {
        runner.afresh().await;
    }
    look(runner, *action == Action::Update).await
}

fn last_lines(ran: &Ran, between: &str) -> String {
    let last: Vec<&str> = ran.output.lines().map(str::trim).filter(|l| !l.is_empty()).rev().take(3).collect();
    let why = last.into_iter().rev().collect::<Vec<_>>().join(between);
    match (ran.status, why.is_empty()) {
        (Some(status), true) => format!("it ended with status {status}"),
        (Some(_), false) => why,
        (None, true) => "the connection ended before it finished".into(),
        (None, false) => format!("the connection ended before it finished ({why})"),
    }
}

/// Which of a service's log is read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LogRange {
    /// Since it last started (what led up to its start too).
    #[default]
    SinceStart,
    Today,
    /// Its last lines, this many.
    Last(usize),
}

impl LogRange {
    /// The ranges the Log page offers, in its menu's order.
    pub const ALL: [LogRange; 3] = [LogRange::SinceStart, LogRange::Today, LogRange::Last(MOST_LINES)];

    pub fn name(self) -> &'static str {
        match self {
            LogRange::SinceStart => "Since it last started",
            LogRange::Today => "Today",
            LogRange::Last(_) => "The last 500 lines",
        }
    }
}

/// The most lines of a log that are read at once.
pub const MOST_LINES: usize = 500;

/// A service's log over `range`, oldest first, as it is shown
/// ([`log_lines`]).
pub async fn journal(runner: &mut impl Runner, service: Service, range: LogRange) -> Result<Vec<String>, Failed> {
    let unit = service.unit();
    let command = match range {
        // (From when it left "inactive": its "Starting…" line is its
        // start's first. Where it never started, its last lines.)
        LogRange::SinceStart => format!(
            "s=$(systemctl show -p InactiveExitTimestamp --value {unit} 2>/dev/null); if [ -n \"$s\" ]; then journalctl -u {unit} -n {MOST_LINES} --no-pager -o short-iso --since \"$s\" 2>&1; else journalctl -u {unit} -n {MOST_LINES} --no-pager -o short-iso 2>&1; fi"
        ),
        LogRange::Today => format!("journalctl -u {unit} -n {MOST_LINES} --no-pager -o short-iso --since today 2>&1"),
        LogRange::Last(lines) => format!("journalctl -u {unit} -n {lines} --no-pager -o short-iso 2>&1"),
    };
    let ran = runner.run(true, &command, &mut |_| {}).await?;
    if !ran.ok() {
        return Err(format!("its log couldn't be read ({})", last_lines(&ran, " ")).into());
    }
    Ok(log_lines(&ran.output))
}

/// The journal's lines as they are shown: each its time to the second
/// (the host's own) and what was written, under a line of its day
/// wherever the day changes ("2026-11-22"). The host's name, the same
/// on every line, is left out; so is the name of what wrote it where
/// that is Rex itself (the log is that service's: only another's lines,
/// systemd's, say whose they are), and the time Rex writes before its
/// own words, the same as the journal's.
pub fn log_lines(journal: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut day = "";
    for line in journal.lines().map(str::trim_end).filter(|l| !l.is_empty() && !l.starts_with("-- ")) {
        let mut parts = line.splitn(3, ' ');
        match (parts.next(), parts.next(), parts.next()) {
            (Some(at), Some(_host), Some(rest)) if at.len() >= 19 && at.as_bytes()[10] == b'T' && at.is_char_boundary(19) => {
                if &at[..10] != day {
                    day = &at[..10];
                    lines.push(day.to_owned());
                }
                let said = match rest.split_once(": ") {
                    Some((by, said)) if by.starts_with("rex") && !by.contains(' ') => own_time_off(said),
                    _ => rest,
                };
                lines.push(format!("{} {said}", &at[11..19]));
            }
            _ => lines.push(line.to_owned()),
        }
    }
    lines
}

/// Whether a line of [`log_lines`] is a day's ("2026-11-22"), not
/// something written.
pub fn is_day(line: &str) -> bool {
    let b = line.as_bytes();
    b.len() == 10 && b[4] == b'-' && b[7] == b'-' && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// A line of Rex's without the time it begins with ("2026/11/23
/// 01:06:20 INFO  …": "INFO  …").
fn own_time_off(said: &str) -> &str {
    let b = said.as_bytes();
    let timed = b.len() > 20 && b[4] == b'/' && b[7] == b'/' && b[10] == b' ' && b[13] == b':' && b[16] == b':' && b[19] == b' ' && b[..4].iter().all(u8::is_ascii_digit);
    if timed { said[20..].trim_start() } else { said }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a change of limits shows before it is done: every setting
    /// that is short, from what the host has to what it will, in
    /// columns; several kinds each under its name; and the cost said
    /// apart from what it merely does.
    #[test]
    fn a_change_of_limits_shows_every_setting_and_what_it_costs() {
        use crate::tune::{Tune, Tuning};
        let comes = "vm.dirty_background_ratio 10\nvm.dirty_ratio 20\nram 67108864\nMaxSessions 10\nopen files 1048576\nDefaultLimitNOFILE 1048576\nDefaultTasksMax infinity\nfs.file-max 9223372036854775807\n\
            fs.inotify.max_user_watches 524288\nfs.inotify.max_user_instances 8192\nfs.inotify.max_queued_events 65536\nfs.aio-max-nr 1048576\nkernel.pid_max 4194304\nvm.max_map_count 1048576\n\
            vm.dirty_background_bytes 0\nvm.dirty_bytes 0\nvm.dirty_expire_centisecs 3000\nvm.swappiness 10\nvm.vfs_cache_pressure 50\ntemporary files 1\n\
            rex.v1.mode=shared\nrex.v1.kept.system=0\nrex.v1.kept.user=0\nrex.v1.status.system=ok\nrex.v1.unit.system=/usr/bin/rex server run --log-format text\nrex.v1.loaded.system=/usr/bin/rex server run --log-format text --max-windows 512 --max-panes 128 --max-sessions 128 --max-blocks 4096\nrex.v1.stale.system=0\n\
            default shell /usr/local/bin/brush\nbrush there\nbrush 1\n";
        let server = Server { tuning: Some(Tuning::read(comes)), ..Server::default() };
        // (Its words, column by column; and the columns are columns:
        // every line of settings as long as the head's.)
        let words = |shown: &str| shown.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>();
        let in_columns = |shown: &str| {
            let wide = shown.lines().next().unwrap().chars().count();
            shown.lines().all(|l| l.chars().count() == wide || !l.contains("  "))
        };
        let one = Action::Tune(vec![Tune::Kernel]);
        assert_eq!(words(&one.shown(&server)), ["now new", "fs.inotify.max_user_watches 524,288 1,048,576"]);
        assert!(in_columns(&one.shown(&server)), "{}", one.shown(&server));
        assert_eq!((one.title().as_str(), one.verb(), one.warning(&server)), ("Apply “Kernel limits”", "Apply", None));
        let all = Action::Tune(server.tuning.as_ref().unwrap().short());
        assert_eq!(
            words(&all.shown(&server)),
            [
                "now new",
                // (A kind that is one setting: one line, by its own name.)
                "SSH sessions 10 1,024",
                "Kernel limits",
                "fs.inotify.max_user_watches 524,288 1,048,576",
                "Writing to disk",
                "vm.dirty_background_bytes not set 8 GB",
                "vm.dirty_bytes not set 32 GB",
                "vm.dirty_expire_centisecs 3,000 6,000",
            ]
        );
        assert!(in_columns(&all.shown(&server)), "{}", all.shown(&server));
        assert!(all.warning(&server).is_some_and(|w| w.contains("loses power")), "the one real cost, apart");
        assert!(all.what(&server).contains("Connections already open aren’t touched") && !all.what(&server).contains("sign-in"), "what it does besides: of the kinds applied alone");
        // One kind alone says its own note, whole.
        assert!(Action::Tune(vec![Tune::Files]).what(&server).starts_with("The open files hold from the next sign-in"));
        // What is run is each kind's script in its own `sh -c` shell so `set -e` stays active inside each tweak.
        let command = all.command(&server).unwrap();
        assert_eq!(command.matches("sh -c ").count(), 3, "{command}");
        assert!(command.contains("Rex: Couldn’t apply:"), "{command}");
        assert!(std::process::Command::new("sh").args(["-n", "-c", &command]).status().unwrap().success());
        // Verify the exact batch error-collection pattern: earlier failure is never hidden by later success.
        let batch_fail = std::process::Command::new("sh")
            .args([
                "-c",
                "rc=0; failed=''\nif ! ( exit 1 ); then rc=1; failed=\"${failed:+$failed, }\"'SSH sessions'; fi\nif ! ( exit 0 ); then rc=1; failed=\"${failed:+$failed, }\"'Rex server limits'; fi\nif [ \"$rc\" != 0 ]; then printf 'Rex: Couldn’t apply: %s.\\n' \"$failed\"; exit \"$rc\"; fi",
            ])
            .output()
            .unwrap();
        assert!(!batch_fail.status.success());
        assert_eq!(String::from_utf8_lossy(&batch_fail.stdout), "Rex: Couldn’t apply: SSH sessions.\n");
        assert_eq!(Action::Untune(Tune::Files).shown(&server).lines().count(), 4);
        // Temporary files and brush, on a host short of both: one line of one; and of brush, who is given
        // it, each with the shell it has, only where the person chose the accounts there are.
        let short = format!(
            "{}default shell /bin/bash\nbrush 0\naccount ana /bin/bash\naccount bo /usr/bin/zsh\n",
            comes.replace("temporary files 1", "temporary files 0").replace("default shell /usr/local/bin/brush\nbrush there\nbrush 1\n", "")
        );
        let mut server = Server { tuning: Some(Tuning::read(&short)), ..Server::default() };
        assert_eq!(words(&Action::Tune(vec![Tune::TempFiles]).shown(&server)), ["now new", "Temporary files in memory on disk"]);
        let brush = Action::Tune(vec![Tune::Shell]);
        assert_eq!(words(&brush.shown(&server)), ["now new", "new accounts /bin/bash /usr/local/bin/brush"]);
        server.tuning.as_mut().unwrap().everyone = true;
        assert_eq!(words(&brush.shown(&server)), ["now new", "new accounts /bin/bash /usr/local/bin/brush", "ana /bin/bash /usr/local/bin/brush", "bo /usr/bin/zsh /usr/local/bin/brush"]);
        assert!(in_columns(&brush.shown(&server)), "{}", brush.shown(&server));
        assert!(brush.command(&server).unwrap().contains("if [ 1 = 1 ]; then") && brush.what(&server).contains("checked against the build Olo knows"));
        // (All of the work's kinds at once on that host: the one-setting kinds a line each among the others.)
        let all = Action::Tune(vec![Tune::Sessions, Tune::Writeback, Tune::TempFiles]);
        assert_eq!(
            words(&all.shown(&server))[1..],
            [
                "SSH sessions 10 1,024",
                "Writing to disk",
                "vm.dirty_background_bytes not set 8 GB",
                "vm.dirty_bytes not set 32 GB",
                "vm.dirty_expire_centisecs 3,000 6,000",
                "Temporary files in memory on disk"
            ]
        );
        assert!(in_columns(&all.shown(&server)), "{}", all.shown(&server));
        assert!(all.what(&server).contains("temporary files take effect from each program’s next sign-in"));
        // The same as the question's table has it: line by line.
        let line = |c: &Change| match c {
            Change::Group(name) => format!("[{name}]"),
            Change::Value { name, now, new, plain, inside, number } => {
                format!("{}{name}: {now} -> {new}{}{}", if *inside { "  " } else { "" }, if *plain { " (plain)" } else { "" }, if *number { " #" } else { "" })
            }
            Change::Note(words) => format!("({words})"),
        };
        let table = |action: &Action, server: &Server| action.changes(server).unwrap().iter().map(line).collect::<Vec<_>>();
        assert_eq!(
            table(&all, &server),
            [
                "SSH sessions: 10 -> 1,024 (plain) #",
                "[Writing to disk]",
                "  vm.dirty_background_bytes: not set -> 8 GB #",
                "  vm.dirty_bytes: not set -> 32 GB #",
                "  vm.dirty_expire_centisecs: 3,000 -> 6,000 #",
                "Temporary files: in memory -> on disk (plain) #",
            ]
        );
        // One kind by itself: its settings, under no name; temporary files, each variable Rex sets.
        assert_eq!(table(&Action::Tune(vec![Tune::Writeback]), &server).len(), 3);
        assert_eq!(table(&Action::Tune(vec![Tune::TempFiles]), &server)[0], "TMPDIR: not set -> /var/tmp");
        assert_eq!(table(&Action::Tune(vec![Tune::TempFiles]), &server).len(), 5);
        // Brush: new accounts; and, chosen, the accounts there are under a name of their own.
        assert_eq!(
            table(&brush, &server),
            ["New accounts: /bin/bash -> /usr/local/bin/brush (plain)", "[Accounts here]", "  ana: /bin/bash -> /usr/local/bin/brush", "  bo: /usr/bin/zsh -> /usr/local/bin/brush"]
        );
        server.tuning.as_mut().unwrap().everyone = false;
        assert_eq!(table(&brush, &server), ["New accounts: /bin/bash -> /usr/local/bin/brush (plain)"]);
        // Put back: what was noted is what comes back, and brush goes unless somebody else signs in to it.
        let applied = format!(
            "{comes}temp TMPDIR /var/tmp\ntemp TMP /var/tmp\ntemp TEMP /var/tmp\ntemp TEMPDIR /var/tmp\ntemp CLAUDE_CODE_TMPDIR /var/tmp\nkept tmpdir\nkept shell\nwas default /bin/bash\nwas user ana /bin/bash\nbrush user ana\n"
        );
        let mut server = Server { tuning: Some(Tuning::read(&applied)), ..Server::default() };
        let back = Action::Untune(Tune::Shell);
        assert_eq!(
            table(&back, &server),
            ["New accounts: /usr/local/bin/brush -> /bin/bash (plain)", "[Accounts Rex changed]", "  ana: /usr/local/bin/brush -> /bin/bash", "Brush: installed -> removed (plain)"]
        );
        assert!(back.what(&server).ends_with("Brush is then taken off."));
        server.tuning.as_mut().unwrap().brush_users.push("carol".into());
        assert_eq!(table(&back, &server).last().unwrap(), "Brush: installed -> stays (plain)");
        assert!(back.what(&server).ends_with("Brush stays installed: carol still uses it."), "{}", back.what(&server));
        assert_eq!(table(&Action::Untune(Tune::TempFiles), &server)[0], "TMPDIR: /var/tmp -> not set");
        assert!(Action::Untune(Tune::Kernel).changes(&server).is_none() && Action::Update.changes(&server).is_none());
        // Rex server limits: short, put back, batch sentence, scope-aware copy, and stale refresh.
        let rex_short = comes.replace("--max-windows 512 --max-panes 128 --max-sessions 128 --max-blocks 4096", "");
        let rex_server = Server { tuning: Some(Tuning::read(&rex_short)), ..Server::default() };
        let apply_rex = Action::Tune(vec![Tune::RexLimits]);
        assert_eq!(table(&apply_rex, &rex_server), ["--max-windows: 64 -> 512 #", "--max-panes: 64 -> 128 #", "--max-sessions: 64 -> 128 #", "--max-blocks: 1,024 -> 4,096 #"]);
        assert_eq!(
            apply_rex.what_on(&rex_server, Some("dev-box")),
            "This changes the Rex server everyone on dev-box shares. The new limits take effect when it next starts. Restarting it ends the programs in everyone’s tabs, so Rex leaves that to you: restart it from the Server window when you’re ready. You can put the old limits back with Put Back… in Tweaks."
        );
        // User-scope host says only that account's server changes (never touches other accounts), gives the restart command, and does not point to the Server window.
        let rex_user_short = rex_short
            .replace("rex.v1.mode=shared", "rex.v1.mode=user\nrex.v1.user=alice\nrex.v1.file.user=/home/alice/.config/systemd/user/rex.service.d/90-olo-limits.conf")
            .replace(".system=", ".user=");
        let rex_user_server = Server { tuning: Some(Tuning::read(&rex_user_short)), ..Server::default() };
        assert!(
            apply_rex.what(&rex_user_server).contains("Only alice’s Rex server on this host is changed; other accounts aren’t touched.")
                && apply_rex.what(&rex_user_server).contains("Restarting ends every tab’s program.\nTo restart: systemctl --user restart rex.service")
                && apply_rex.what(&rex_user_server).contains("Put Back… in Tweaks")
                && !apply_rex.what(&rex_user_server).contains("back here")
                && !apply_rex.what(&rex_user_server).contains("Server window"),
            "{}",
            apply_rex.what(&rex_user_server)
        );
        assert_eq!(apply_rex.warning(&rex_server), None);
        let batch_with_rex = Action::Tune(vec![Tune::Sessions, Tune::RexLimits]);
        assert!(
            batch_with_rex.what_on(&rex_server, Some("dev-box")).contains("Rex’s server, shared by everyone on dev-box, gets its new limits when it next starts.")
                && batch_with_rex.what_on(&rex_server, Some("dev-box")).contains("Everything is kept across reboots and can be put back with Put Back… in Tweaks."),
            "{}",
            batch_with_rex.what_on(&rex_server, Some("dev-box"))
        );
        assert!(
            batch_with_rex.what(&rex_user_server).contains("alice’s Rex server gets its new limits when it next starts. Restarting it ends every tab’s program, so it isn’t restarted for you."),
            "{}",
            batch_with_rex.what(&rex_user_server)
        );
        assert_eq!(Action::Untune(Tune::RexLimits).shown(&rex_server), crate::tune::REX_SYSTEM_DROPIN);
        assert_eq!(Action::Untune(Tune::RexLimits).shown(&rex_user_server), "/home/alice/.config/systemd/user/rex.service.d/90-olo-limits.conf");
        assert_eq!(Action::Untune(Tune::RexLimits).what(&rex_server), "Olo’s file is removed, and the shared Rex server goes back to its limits when it next starts. It isn’t restarted.");
        // Put Back mentions the other scope's file when both scopes have an Olo file.
        let rex_user_both = Server { tuning: Some(Tuning::read(&format!("{rex_user_short}rex.v1.kept.system=1\n").replace("rex.v1.kept.user=0", "rex.v1.kept.user=1"))), ..Server::default() };
        assert!(
            Action::Untune(Tune::RexLimits).what(&rex_user_both).contains("Only alice’s limit file is removed; other accounts aren’t touched.")
                && Action::Untune(Tune::RexLimits).what(&rex_user_both).contains(crate::tune::REX_SYSTEM_DROPIN),
            "{}",
            Action::Untune(Tune::RexLimits).what(&rex_user_both)
        );
        let rex_shared_both =
            Server { tuning: Some(Tuning::read(&comes.replace("rex.v1.kept.system=0", "rex.v1.kept.system=1").replace("rex.v1.kept.user=0", "rex.v1.kept.user=1"))), ..Server::default() };
        assert_eq!(
            Action::Untune(Tune::RexLimits).what(&rex_shared_both),
            "Olo’s file is removed, and the shared Rex server goes back to its limits when it next starts. It isn’t restarted. Your own Rex server keeps its limits."
        );
        // Put Back shows Now -> New values going back to the unit file's own limits.
        let rex_applied = Server { tuning: Some(Tuning::read(&comes.replace("rex.v1.kept.system=0", "rex.v1.kept.system=1"))), ..Server::default() };
        assert_eq!(table(&Action::Untune(Tune::RexLimits), &rex_applied), ["--max-windows: 512 -> 64 #", "--max-panes: 128 -> 64 #", "--max-sessions: 128 -> 64 #", "--max-blocks: 4,096 -> 1,024 #"]);
        assert_eq!(Action::Untune(Tune::RexLimits).shown(&rex_applied), crate::tune::REX_SYSTEM_DROPIN);
        let rex_with_unit_hi = comes
            .replace("rex.v1.kept.system=0", "rex.v1.kept.system=1")
            .replace("rex.v1.unit.system=/usr/bin/rex server run --log-format text\n", "rex.v1.unit.system=/usr/bin/rex server run --log-format text --max-windows 1024 --max-blocks 8192\n");
        let rex_other_server = Server { tuning: Some(Tuning::read(&rex_with_unit_hi)), ..Server::default() };
        assert_eq!(
            table(&Action::Untune(Tune::RexLimits), &rex_other_server),
            ["--max-windows: 512 -> 1,024 #", "--max-panes: 128 -> 64 #", "--max-sessions: 128 -> 64 #", "--max-blocks: 4,096 -> 8,192 #"]
        );
        let rex_stale =
            comes.replace("rex.v1.kept.system=0", "rex.v1.kept.system=1").replace("rex.v1.stale.system=0", "rex.v1.stale.system=1\nrex.v1.pkg.system=/usr/bin/rex server run --log-format text --old");
        let stale_server = Server { tuning: Some(Tuning::read(&rex_stale)), ..Server::default() };
        assert_eq!(table(&apply_rex, &stale_server), ["Rex service file: out of date -> updated (plain)"]);
        assert!(
            apply_rex.what(&stale_server).contains("take effect when it next starts") && !apply_rex.what(&stale_server).contains("drop-in") && !apply_rex.what(&stale_server).contains("hold from"),
            "{}",
            apply_rex.what(&stale_server)
        );
    }

    /// A host that answers as written: each command run is kept, and
    /// answered by the first rule whose words it holds.
    struct Host {
        rules: Vec<(&'static str, Ran)>,
        ran: Vec<String>,
    }

    /// Limits changed are looked at through a new sign-in: the one that
    /// changed them still has the old ones.
    #[test]
    fn limits_are_looked_at_through_a_new_sign_in_after_they_change() {
        struct Signed(Host, Vec<&'static str>);
        impl Runner for Signed {
            async fn run(&mut self, root: bool, command: &str, said: &mut (dyn FnMut(&str) + Send)) -> Result<Ran, Failed> {
                self.1.push(if command.contains("90-olo") && command.contains("set -e") {
                    "changed"
                } else if command.contains("MaxSessions $(sshd") {
                    "looked"
                } else {
                    "other"
                });
                self.0.run(root, command, said).await
            }
            async fn afresh(&mut self) {
                self.1.push("signed in anew");
            }
        }
        let block =
            |work: std::pin::Pin<Box<dyn std::future::Future<Output = Result<Server, Failed>> + '_>>| tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(work);
        let mut host = Signed(Host { rules: vec![("os=", says(FACTS))], ran: Vec::new() }, Vec::new());
        let server = Server { tuning: Some(crate::tune::Tuning::read("ram 16000000\nopen files 1024\n")), ..Server::default() };
        block(Box::pin(act(&mut host, &server, &Action::Tune(vec![crate::tune::Tune::Files]), &mut |_| {}))).unwrap();
        let order: Vec<&str> = host.1.iter().copied().filter(|what| *what != "other").collect();
        assert_eq!(order, ["changed", "signed in anew", "looked"]);
        // (Nothing of the kind for what isn't a limit.)
        let mut host = Signed(Host { rules: vec![("os=", says(FACTS))], ran: Vec::new() }, Vec::new());
        block(Box::pin(act(&mut host, &server, &Action::Restart(Service::Login), &mut |_| {}))).unwrap();
        assert!(!host.1.contains(&"signed in anew"));
    }

    impl Runner for Host {
        async fn run(&mut self, root: bool, command: &str, _: &mut (dyn FnMut(&str) + Send)) -> Result<Ran, Failed> {
            assert!(root, "everything here is root's: {command}");
            self.ran.push(command.to_owned());
            Ok(self.rules.iter().find(|(has, _)| command.contains(has)).map(|(_, ran)| ran.clone()).unwrap_or(Ran { status: Some(0), ..Ran::default() }))
        }
    }

    fn says(output: &str) -> Ran {
        Ran { status: Some(0), output: output.to_owned(), stdout: output.to_owned(), ..Ran::default() }
    }

    const FACTS: &str = "os=Linux\ndistro=ubuntu\nuser=root\nuid=0\ngroups=root\nrex=/usr/bin/rex\nversion=rex 0.1.0-snapshot.2506\npackaged=1\nsystemd=1\nunit.rex.service=inactive,disabled\nunit.rex.socket=inactive,disabled\nunit.rex-login-server.service=active,enabled\nunit.rex-login-server.socket=active,enabled\nown=inactive,disabled\nlinger=no\nrexshell=/usr/sbin/nologin\nfetch=curl\n";
    const MORE_SAID: &str = "began.rex-login-server.service=4000000\nuptime=7204.51\nmembers=\nperson=alice:1000\nperson=bo:1001\nsessions.asked=1\nsessions=alice:2\nsessions=bo:0\ninstalled=0.1.0~alpha.2506\ncandidate=0.1.0~alpha.2510\n";

    fn host() -> Host {
        Host { rules: vec![("uname -s", says(FACTS)), ("ActiveEnterTimestampMonotonic", says(MORE_SAID))], ran: Vec::new() }
    }

    #[tokio::test]
    async fn a_login_server_is_read_as_it_stands() {
        let mut h = host();
        let server = look(&mut h, false).await.unwrap();
        assert_eq!(server.mode(), Some(ServerMode::Login));
        assert!(server.state(Service::Login).active && !server.state(Service::Shared).used());
        // (Up 7204 s, begun 4 s in: two hours.)
        assert_eq!(server.up(Service::Login), Some(7200));
        assert_eq!(server.up(Service::Shared), None);
        assert_eq!(server.people, vec![Person { name: "alice".into(), uid: 1000 }, Person { name: "bo".into(), uid: 1001 }]);
        assert_eq!((server.installed.as_deref(), server.newer.as_deref()), (Some("0.1.0~alpha.2506"), Some("0.1.0~alpha.2510")));
        // (The version shown is the package's, which its repository speaks of too; nobody asked the repository just now.)
        assert_eq!(server.version().as_deref(), Some("0.1.0~alpha.2506"));
        assert!(!server.checked && !server.failed(Service::Login));
        // The sessions open: on each account's own server, and through the login server in all.
        assert_eq!((server.sessions_of("alice"), server.sessions_of("bo"), server.sessions_of("carmen")), (Some(2), Some(0), None));
        assert_eq!((server.sessions_on(Service::Login), server.sessions_on(Service::Shared)), (Some(2), None));
        // (No accounts file yet: as the package ships it, nobody listed.)
        assert_eq!(server.not_listed().len(), 2);
        // Nothing was changed by looking.
        assert!(h.ran.iter().all(|c| !c.contains("systemctl enable") && !c.contains("usermod") && !c.contains(" > ")), "{:?}", h.ran);
    }

    #[tokio::test]
    async fn an_account_is_let_in_and_taken_out_again() {
        let mut h = host();
        let server = look(&mut h, false).await.unwrap();
        let alice = server.people[0].clone();
        let allow = Action::Allow(alice.clone());
        assert_eq!(allow.title(), "Allow alice to sign in");
        assert!(allow.shown(&server).contains("\"alice\": { \"principals\": [\"peer:1000\"] }"));
        act(&mut h, &server, &allow, &mut |_| {}).await.unwrap();
        // The file written holds her, and her default.
        let written = h.ran.iter().rev().find(|c| c.contains("base64 -d > /etc/rex/accounts.jsonc.new")).expect("the file is written").clone();
        let text = written_text(&written);
        let file = Accounts::parse(&text).unwrap();
        assert_eq!(file.accounts().iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), vec!["alice"]);
        assert_eq!(file.defaults(), vec![("peer:1000".to_owned(), "alice".to_owned())]);
        // Taken out: neither is left.
        // (After the rule for the facts: the question that asks them names the file too.)
        h.rules.push(("]; then base64 < /etc/rex/accounts.jsonc", says(&b64(&text))));
        let server = look(&mut h, false).await.unwrap();
        assert_eq!(server.not_listed().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["bo"]);
        let remove = Action::Remove("alice".into());
        assert!(remove.warning(&server).unwrap().contains("can no longer sign in"));
        act(&mut h, &server, &remove, &mut |_| {}).await.unwrap();
        let written = h.ran.iter().rev().find(|c| c.contains("base64 -d > /etc/rex/accounts.jsonc.new")).unwrap().clone();
        let file = Accounts::parse(&written_text(&written)).unwrap();
        assert!(file.accounts().is_empty() && file.defaults().is_empty(), "{}", file.text());
    }

    fn b64(text: &str) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(text)
    }

    /// The accounts file a write command carries.
    fn written_text(command: &str) -> String {
        use base64::Engine as _;
        let from = command.find("printf %s ").unwrap() + "printf %s ".len();
        let word = command[from..].split_whitespace().next().unwrap().trim_matches('\'');
        String::from_utf8(base64::engine::general_purpose::STANDARD.decode(word).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn what_ends_sessions_says_so_first_and_runs_what_it_shows() {
        let mut h = host();
        let mut server = look(&mut h, false).await.unwrap();
        // The login server restarted: sessions go on, nothing to warn of.
        let restart = Action::Restart(Service::Login);
        assert_eq!(restart.warning(&server), None);
        assert_eq!(restart.shown(&server), "systemctl restart rex-login-server.service");
        act(&mut h, &server, &restart, &mut |_| {}).await.unwrap();
        assert!(h.ran.iter().any(|c| c == "systemctl restart rex-login-server.service"));
        // A running shared server stopped, restarted or updated: every session ends, and it says so.
        server.facts.shared = Unit { active: true, enabled: true };
        for action in [Action::Stop(Service::Shared), Action::Restart(Service::Shared), Action::Update] {
            assert!(action.warning(&server).is_some_and(|w| w.contains("ends every session")), "{action:?}");
        }
        assert_eq!(Action::Stop(Service::Shared).shown(&server), "systemctl disable --now rex.socket rex.service");
        // Updating is Rex's installer again, on the channel the host is on.
        assert_eq!(Action::Update.shown(&server), "curl -fsSL https://pkgs.superlogical.com/rex/install.sh | sh -s -- --channel snapshot");
        // The group: added with usermod, taken out with gpasswd.
        assert_eq!(Action::Join("bo".into()).shown(&server), "usermod --append --groups rex bo");
        assert_eq!(Action::Leave("bo".into()).shown(&server), "gpasswd --delete bo rex");
    }

    #[tokio::test]
    async fn a_service_that_wont_start_says_what_its_log_says() {
        let mut h = host();
        let server = look(&mut h, false).await.unwrap();
        h.rules.insert(0, ("systemctl enable --now rex.service", Ran { status: Some(1), output: "Job for rex.service failed.\n".into(), ..Ran::default() }));
        h.rules.insert(0, ("journalctl -u rex.service", says("2026-11-22T14:10:00+0000 host rex[9]: error: the rex account has no shell\n")));
        let failed = act(&mut h, &server, &Action::Start(Service::Shared), &mut |_| {}).await.unwrap_err();
        assert!(failed.why.contains("Job for rex.service failed.") && failed.why.contains("14:10:00 error: the rex account has no shell"), "{}", failed.why);
    }

    #[test]
    fn the_log_is_shown_by_its_days_each_line_its_time_and_what_was_written() {
        // (As a real host's journal has them, 2026-11-23: systemd's lines, and Rex's with its own time before its words.)
        let journal = "-- Boot 1 --\n2026-11-22T23:59:58+00:00 build systemd[1]: Starting rex-login-server.service - Rex Login Server...\n2026-11-23T00:00:01+00:00 build rex[18036]: 2026/11/23 00:00:01 INFO  login server starting command=\"rex login-server\"\n2026-11-23T00:00:01+00:00 build systemd[1]: Started rex-login-server.service - Rex Login Server.\n2026-11-23T01:06:20+00:00 build rex[18036]: error: plain words\n";
        assert_eq!(
            log_lines(journal),
            [
                "2026-11-22",
                "23:59:58 systemd[1]: Starting rex-login-server.service - Rex Login Server...",
                "2026-11-23",
                "00:00:01 INFO  login server starting command=\"rex login-server\"",
                "00:00:01 systemd[1]: Started rex-login-server.service - Rex Login Server.",
                "01:06:20 error: plain words",
            ]
        );
        // (Anything else, as it came.)
        assert_eq!(log_lines("No journal files were found.\n"), ["No journal files were found."]);
        assert!(log_lines("").is_empty());
        assert!(is_day("2026-11-22") && !is_day("23:59:58 x") && !is_day("2026-11-2x") && !is_day(""));
        assert_eq!(last_words("error: plain words").as_deref(), Some("plain words"));
        assert_eq!(last_words("  "), None);
    }

    #[tokio::test]
    async fn the_repository_is_asked_only_when_told_to_and_what_failed_is_known() {
        let mut h = host();
        h.rules[1].1 = says(
            "failed.rex.service=1\nended.rex.service=7000000000\nlast.rex.service=2026/11/23 01:06:20 ERROR the rex account has no shell\nuptime=7240.2\nchecked=1\ninstalled=0.1.0~alpha.2506\ncandidate=0.1.0~alpha.2506\n",
        );
        let server = look(&mut h, true).await.unwrap();
        assert!(h.ran.iter().any(|c| c.contains("CHECK=1")), "{:?}", h.ran);
        assert!(server.checked && server.newer.is_none() && server.failed(Service::Shared) && !server.failed(Service::Login));
        // What failed says when (four minutes ago) and its own last words, without its time and its level.
        assert_eq!((server.ended(Service::Shared), server.last_words(Service::Shared)), (Some(240), Some("the rex account has no shell")));
        assert_eq!((server.ended(Service::Login), server.last_words(Service::Login)), (None, None));
        // Nobody could be asked about sessions there: unknown, not none.
        assert_eq!(server.sessions_on(Service::Login), None);
        h.ran.clear();
        look(&mut h, false).await.unwrap();
        assert!(h.ran.iter().all(|c| !c.contains("CHECK=1")));
        // The log's ranges: since it started, today, its last lines.
        for (range, has) in [(LogRange::SinceStart, "InactiveExitTimestamp"), (LogRange::Today, "--since today"), (LogRange::Last(12), "-n 12 ")] {
            h.ran.clear();
            journal(&mut h, Service::Login, range).await.unwrap();
            assert!(h.ran[0].contains(has) && h.ran[0].contains("journalctl -u rex-login-server.service"), "{}", h.ran[0]);
        }
    }

    #[test]
    fn a_count_of_microseconds_is_read_as_systemd_prints_it() {
        assert_eq!(micros("4000000"), Some(4_000_000));
        assert_eq!(micros("ActiveEnterTimestampMonotonic=4000000"), Some(4_000_000));
        // (A service that never began: nothing.)
        assert_eq!(micros("0"), None);
        assert_eq!(micros(""), None);
    }

    /// Finding 2 & Finding 13: connection scope (`Runner::rex_scope`) and
    /// account flow authoritatively into `look`, `act(Tune)` and
    /// `act(Untune)`; custom sockets refuse RexLimits; truncated LOOK output
    /// marks RexLimits unreadable with `kept = false`.
    #[tokio::test]
    async fn connection_scope_flows_into_look_apply_and_undo() {
        struct ScopedHost {
            scope: Option<crate::tune::RexScope>,
            account: Option<&'static str>,
            look_ran: Ran,
            ran: Vec<String>,
            used_bounded: bool,
        }
        impl Runner for ScopedHost {
            async fn run(&mut self, _root: bool, command: &str, _: &mut (dyn FnMut(&str) + Send)) -> Result<Ran, Failed> {
                self.ran.push(command.to_owned());
                if command.contains("uname -s") {
                    return Ok(says(FACTS));
                }
                if command.contains("ActiveEnterTimestampMonotonic") {
                    return Ok(says(MORE_SAID));
                }
                Ok(Ran { status: Some(0), ..Ran::default() })
            }
            async fn run_bounded(&mut self, root: bool, command: &str, max_output: usize, max_line: usize, said: &mut (dyn FnMut(&str) + Send)) -> Result<Ran, Failed> {
                self.used_bounded = true;
                assert_eq!((max_output, max_line), (4 * 1024 * 1024, 64 * 1024));
                self.ran.push(command.to_owned());
                let _ = (root, said);
                Ok(self.look_ran.clone())
            }
            fn account(&self) -> Option<&str> {
                self.account
            }
            fn rex_scope(&self) -> Option<crate::tune::RexScope> {
                self.scope
            }
        }

        let user_out = "ram 65536000\nrex.v1.mode=user\nrex.v1.user=alice\nrex.v1.kept.system=0\nrex.v1.kept.user=0\nrex.v1.status.user=ok\n\
            rex.v1.unit.user=/usr/bin/rex server run --log-format text\nrex.v1.loaded.user=/usr/bin/rex server run --log-format text\nrex.v1.stale.user=0\n";
        let mut h = ScopedHost { scope: Some(crate::tune::RexScope::User), account: Some("alice"), look_ran: says(user_out), ran: Vec::new(), used_bounded: false };
        let server = look(&mut h, false).await.unwrap();
        assert!(h.used_bounded, "manage::look must use run_bounded for the tweaks probe");
        let look_cmd = h.ran.iter().find(|c| c.contains("rex.v1.mode")).unwrap();
        assert!(look_cmd.contains("OLO_REX_MODE=user") && look_cmd.contains("OLO_REX_USER=alice"), "{look_cmd}");
        assert_eq!(server.tuning.as_ref().unwrap().rex_limits.scope, crate::tune::RexScope::User);

        // Apply and Untune carry authoritative user mode & account.
        h.ran.clear();
        act(&mut h, &server, &Action::Tune(vec![crate::tune::Tune::RexLimits]), &mut |_| {}).await.unwrap();
        let apply_cmd = h.ran.iter().find(|c| c.contains("_baked_mode=")).unwrap();
        assert!(apply_cmd.contains("_baked_mode=user") && apply_cmd.contains("_baked_user=alice"), "{apply_cmd}");

        h.ran.clear();
        act(&mut h, &server, &Action::Untune(crate::tune::Tune::RexLimits), &mut |_| {}).await.unwrap();
        let undo_cmd = h.ran.iter().find(|c| c.contains("rex_undo_one")).unwrap();
        assert!(undo_cmd.contains("OLO_REX_MODE=user") && undo_cmd.contains("OLO_REX_USER=alice"), "{undo_cmd}");

        // Shared runner refuses to act on a User-scoped tuning snapshot (scope mismatch).
        let mut shared_h = ScopedHost { scope: Some(crate::tune::RexScope::Shared), account: Some("alice"), look_ran: says(user_out), ran: Vec::new(), used_bounded: false };
        assert!(act(&mut shared_h, &server, &Action::Tune(vec![crate::tune::Tune::RexLimits]), &mut |_| {}).await.is_err());
        assert!(act(&mut shared_h, &server, &Action::Untune(crate::tune::Tune::RexLimits), &mut |_| {}).await.is_err());
        // Shared runner seeing a user-mode LOOK reply marks RexLimits unreadable!
        let mismatched = look(&mut shared_h, false).await.unwrap();
        let mt = mismatched.tuning.unwrap();
        assert_eq!(mt.rex_limits.blocked.as_deref(), Some("Olo couldn’t read this host’s Rex service."));
        assert!(!mt.of(crate::tune::Tune::RexLimits).kept);

        // Truncated LOOK output marks RexLimits unreadable with kept=false.
        let mut trunc_ran = says(&user_out.replace("rex.v1.kept.user=0", "rex.v1.kept.user=1"));
        trunc_ran.truncated = true;
        let mut trunc_h = ScopedHost { scope: Some(crate::tune::RexScope::User), account: Some("alice"), look_ran: trunc_ran, ran: Vec::new(), used_bounded: false };
        let ts = look(&mut trunc_h, false).await.unwrap();
        let tt = ts.tuning.unwrap();
        assert_eq!(tt.rex_limits.blocked.as_deref(), Some("Olo couldn’t read this host’s Rex service."));
        assert!(!tt.of(crate::tune::Tune::RexLimits).kept && !tt.short().contains(&crate::tune::Tune::RexLimits));

        // Custom/unidentified socket (`rex_scope() == None`): LOOK skips Rex inspection & blocks; act refuses.
        let mut custom_h = ScopedHost { scope: None, account: Some("alice"), look_ran: says("ram 65536000\n"), ran: Vec::new(), used_bounded: false };
        let cs = look(&mut custom_h, false).await.unwrap();
        let custom_look = custom_h.ran.iter().find(|c| c.contains("OLO_REX_MODE=")).unwrap();
        assert!(custom_look.contains("OLO_REX_MODE=skip"), "{custom_look}");
        assert!(cs.tuning.as_ref().unwrap().rex_limits.blocked.as_deref().is_some_and(|w| w.contains("custom Rex socket")));
        assert!(act(&mut custom_h, &server, &Action::Tune(vec![crate::tune::Tune::RexLimits]), &mut |_| {}).await.is_err());
        assert!(act(&mut custom_h, &server, &Action::Untune(crate::tune::Tune::RexLimits), &mut |_| {}).await.is_err());
    }

    /// Finding 14: an intermediate command failing inside one tweak in a
    /// production `Action::Tune` batch MUST abort that tweak (`set -e`) and
    /// make the batch exit non-zero naming that tweak, even when the next
    /// tweak succeeds.
    #[test]
    fn batch_tune_fails_when_an_intermediate_command_inside_a_tweak_fails() {
        use std::os::unix::fs::PermissionsExt as _;
        let comes = "ram 65536000\nMaxSessions 10\nopen files 1024\nDefaultLimitNOFILE 1024\nDefaultTasksMax 1000\n";
        let server = Server { tuning: Some(crate::tune::Tuning::read(comes)), ..Server::default() };
        let batch = Action::Tune(vec![crate::tune::Tune::Files, crate::tune::Tune::Sessions]).command(&server).unwrap();

        let root = std::path::PathBuf::from(format!("/var/tmp/olo-rex-batch-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["bin", "etc/security/limits.d", "etc/systemd/system.conf.d", "etc/systemd/user.conf.d", "etc/systemd/logind.conf.d", "etc/ssh/sshd_config.d"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("etc/ssh/sshd_config"), "Include /etc/ssh/sshd_config.d/*.conf\n").unwrap();
        let write_bin = |name: &str, body: &str| {
            let p = root.join("bin").join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        write_bin("sysctl", "echo 1048576");
        // Intermediate command in Tune::Files fails (`daemon-reexec` exits 1),
        // while `su` (the last command in Tune::Files) and `systemctl reload sshd` (Tune::Sessions) succeed!
        write_bin("systemctl", "if [ \"$1\" = daemon-reexec ]; then echo 'reexec failed' >&2; exit 1; fi\nexit 0");
        write_bin("su", "exit 0");
        write_bin("sshd", "exit 0");

        let rewritten = batch.replace("/etc/", &format!("{}/etc/", root.display()));
        let path = format!("{}/bin:{}", root.display(), std::env::var("PATH").unwrap_or_default());
        let out = std::process::Command::new("sh").args(["-c", &rewritten]).env("PATH", path).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let _ = std::fs::remove_dir_all(&root);
        assert!(!out.status.success(), "intermediate failure inside Tune::Files must fail the batch! stdout={stdout:?} stderr={stderr:?}");
        assert!(stdout.contains("Rex: Couldn’t apply: Open files and tasks."), "must name failing tweak, got stdout={stdout:?} stderr={stderr:?}");
        assert!(!stdout.contains("SSH sessions"), "succeeding second tweak must not be listed as failed: {stdout:?}");
    }

    /// Finding 22: Put Back's changes table lists and counts ONLY the Rex
    /// limits whose value differs from what the unit file has without Olo's
    /// drop-in, and avoids "you's" in personal-scope prose.
    #[test]
    fn put_back_rex_limits_lists_only_differing_values() {
        use crate::tune::{Tune, Tuning};
        let partial = "ram 65536000\nrex.v1.mode=user\nrex.v1.user=alice\nrex.v1.kept.system=0\nrex.v1.kept.user=1\nrex.v1.status.user=ok\n\
            rex.v1.unit.user=/usr/bin/rex server run --max-windows 1024 --max-blocks 8192\n\
            rex.v1.loaded.user=/usr/bin/rex server run --max-windows 1024 --max-panes 128 --max-sessions 128 --max-blocks 8192\n\
            rex.v1.stale.user=0\n";
        let server = Server { tuning: Some(Tuning::read(partial)), ..Server::default() };
        let changes = Action::Untune(Tune::RexLimits).changes(&server).unwrap();
        let names: Vec<(&str, &str, &str)> = changes
            .iter()
            .filter_map(|c| match c {
                Change::Value { name, now, new, .. } => Some((name.as_str(), now.as_str(), new.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(names, [("--max-panes", "128", "64"), ("--max-sessions", "128", "64")]);

        // If all four values already equal the unit file's, no fake table is shown.
        let same = "ram 65536000\nrex.v1.mode=user\nrex.v1.user=alice\nrex.v1.kept.system=0\nrex.v1.kept.user=1\nrex.v1.status.user=ok\n\
            rex.v1.unit.user=/usr/bin/rex server run --max-windows 1024 --max-panes 128 --max-sessions 128 --max-blocks 8192\n\
            rex.v1.loaded.user=/usr/bin/rex server run --max-windows 1024 --max-panes 128 --max-sessions 128 --max-blocks 8192\n\
            rex.v1.stale.user=0\n";
        let same_server = Server { tuning: Some(Tuning::read(same)), ..Server::default() };
        assert_eq!(Action::Untune(Tune::RexLimits).changes(&same_server), None);

        // User named "you" renders natural "your" rather than "you's".
        let you_server = Server { tuning: Some(Tuning::read(&partial.replace("rex.v1.user=alice", "rex.v1.user=you"))), ..Server::default() };
        let apply_text = Action::Tune(vec![Tune::RexLimits]).what(&you_server);
        let undo_text = Action::Untune(Tune::RexLimits).what(&you_server);
        assert!(apply_text.contains("Only your Rex server") && !apply_text.contains("you’s"), "{apply_text}");
        assert!(undo_text.contains("Only your limit file") && !undo_text.contains("you’s"), "{undo_text}");
    }
}
