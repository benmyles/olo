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
    if let Ok(ran) = runner.run(true, &format!("sh -c {}", shell_word(crate::tune::LOOK)), &mut |_| {}).await
        && ran.ok()
    {
        server.tuning = Some(crate::tune::Tuning::read(&ran.output));
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
            Action::Tune(_) => "Apply the suggested limits".to_owned(),
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
            Action::Tune(_) => "Applying the suggested limits".to_owned(),
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
            Action::Tune(kinds) => {
                let mut said: Vec<&str> = kinds.iter().filter_map(|k| k.note()).collect();
                said.push("Each is kept across restarts and can be put back from this page. Nothing that is already higher there is lowered.");
                said.join(" ")
            }
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
                let wide = |of: &dyn Fn(&(&str, String, String)) -> usize, least: usize| short.iter().flat_map(|(_, rows)| rows.iter().map(of)).fold(least, usize::max);
                let (name, now, new) = (wide(&|r| r.0.chars().count(), 0), wide(&|r| r.1.chars().count(), 3), wide(&|r| r.2.chars().count(), 3));
                // (Under a kind's name its settings stand a step in.)
                let step = if short.len() > 1 { "  " } else { "" };
                let mut lines = vec![format!("{step}{:name$}  {:>now$}  {:>new$}", "", "now", "new")];
                for (kind, rows) in &short {
                    if short.len() > 1 {
                        lines.push(kind.title().to_owned());
                    }
                    lines.extend(rows.iter().map(|(n, a, b)| format!("{step}{n:name$}  {a:>now$}  {b:>new$}")));
                }
                lines.join("\n")
            }
            Action::Untune(kind) => kind.files().join("\n"),
            other => other.command(server).unwrap_or_default(),
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
                let scripts: Vec<String> = kinds.iter().filter_map(|k| k.apply(tuning)).collect();
                if scripts.is_empty() {
                    return None;
                }
                // (Each kind by itself: one that can't be applied
                // doesn't keep the next from it.)
                scripts.iter().map(|s| format!("( {} )", s.trim_end())).collect::<Vec<_>>().join("\n")
            }
            Action::Untune(kind) => kind.undo(),
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
            let command = other.command(server).ok_or_else(|| Failed::from("nothing to run".to_owned()))?;
            // (A script of several lines goes to a shell as one word.)
            let command = if matches!(other, Action::Tune(_) | Action::Untune(_)) { format!("sh -c {}", shell_word(&command)) } else { command };
            let ran = runner.run(true, &command, said).await?;
            if !ran.ok() {
                let mut why = last_lines(&ran);
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

fn last_lines(ran: &Ran) -> String {
    let last: Vec<&str> = ran.output.lines().map(str::trim).filter(|l| !l.is_empty()).rev().take(3).collect();
    let why = last.into_iter().rev().collect::<Vec<_>>().join(" ");
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
        return Err(format!("its log couldn't be read ({})", last_lines(&ran)).into());
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
            vm.dirty_background_bytes 0\nvm.dirty_bytes 0\nvm.dirty_expire_centisecs 3000\nvm.swappiness 10\nvm.vfs_cache_pressure 50\n";
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
                "SSH sessions",
                "MaxSessions 10 1,024",
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
        assert!(all.what(&server).contains("open connections stay") && !all.what(&server).contains("sign-in"), "what it does besides: of the kinds applied alone");
        // What is run is each kind's script, each by itself.
        let command = all.command(&server).unwrap();
        assert_eq!(command.matches("\n( set -e").count() + usize::from(command.starts_with("( set -e")), 3);
        assert_eq!(Action::Untune(Tune::Files).shown(&server).lines().count(), 4);
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
            Ok(self.rules.iter().find(|(has, _)| command.contains(has)).map(|(_, ran)| ran.clone()).unwrap_or(Ran { status: Some(0), output: String::new() }))
        }
    }

    fn says(output: &str) -> Ran {
        Ran { status: Some(0), output: output.to_owned() }
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
        h.rules.insert(0, ("systemctl enable --now rex.service", Ran { status: Some(1), output: "Job for rex.service failed.\n".into() }));
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
}
