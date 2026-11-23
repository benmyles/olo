//! Looking after a host's Rex server from Rex: the Server window's
//! state. The host is signed in to as root (with keys alone, as when a
//! host is set up: `setup`), asked how its server stands, and shown: its
//! services, who may use it, its log. Each thing done to it is put to
//! the person first, with what is run and what it costs, done on the
//! hub's threads with what it writes shown as it comes, and followed by
//! a fresh look at the host, so the window shows what the host says.
//!
//! What a server is and what is done to one is
//! `olo_rex_client::manage`'s; here is where the window stands
//! ([`Stage`]). One host at a time (there is one Server window). Root's
//! connection is kept while the window is open, so each thing asked is
//! answered at once, and dropped when it closes.

use std::sync::Arc;

use olo_rex_client::{
    AgentKey, ServerMode, SshTarget,
    manage::{self, Action, LogRange, Person, Server, Service},
    provision::Failed,
};
use olo_sync::Mutex;

use crate::hub::RexHub;

/// Where the window stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    /// Signing in as root and asking how the server stands.
    Looking,
    /// Root there took none of the keys offered: which to offer.
    RootKey { keys: Vec<AgentKey>, chosen: usize },
    /// How it stands is shown.
    Shown,
    /// `action` put to the person: what it runs, what it costs.
    /// (`choose`: somebody is to be chosen for it first, of
    /// [`Managed::could_join`]: an account to allow, a person to add.)
    Confirm { action: Action, choose: bool },
    /// It is being done; `said`, the last line it wrote.
    Working { action: Action, said: String },
    /// It didn't go: `what`, in a few words (the heading); `why`, what
    /// Rex has to say of it; `said`, what the host said, in its own
    /// words (or nothing). `icon`: the alert's. `log`: the service
    /// whose log may say more (Show Log).
    Failed { what: String, why: String, said: String, icon: &'static str, log: Option<Service> },
}

/// Which page of the window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Server,
    /// Who may use it: the login server's accounts, or the `rex`
    /// group's members where the server is the shared one.
    People,
    Log,
}

impl Page {
    pub const ALL: [Page; 3] = [Page::Server, Page::People, Page::Log];
}

/// A host whose server is being looked after.
#[derive(Clone, Debug, PartialEq)]
pub struct Managed {
    /// The name the host goes by in Rex.
    pub name: String,
    /// The account's own connection; root's is the same host's.
    pub target: SshTarget,
    /// The key root is offered (its fingerprint); none: the agent's, in turn.
    pub root_key: Option<String>,
    pub stage: Stage,
    /// The server as the host last said it stands, and when it said so
    /// (how long a service has run is counted on from then).
    pub server: Option<Server>,
    pub read: Option<std::time::Instant>,
    pub page: Page,
    /// The log shown: of which service, over which range, whether it
    /// is being read, and when it last was.
    pub log: Option<Vec<String>>,
    pub log_read: Option<std::time::Instant>,
    pub log_of: Service,
    pub log_range: LogRange,
    pub log_reading: bool,
}

impl Managed {
    /// `root@HOST`, as it is said to the person: once, over what is run.
    pub fn root(&self) -> String {
        format!("root@{}", self.target.host)
    }

    /// Whether the host's server is the shared one: its people are the
    /// `rex` group's members, not a file's accounts.
    pub fn shared(&self) -> bool {
        self.server.as_ref().is_some_and(|s| s.mode() == Some(ServerMode::Shared))
    }

    /// The people who could be let in on the People page: those the
    /// login server doesn't list, or those not in the `rex` group.
    pub fn could_join(&self) -> Vec<Person> {
        let Some(server) = &self.server else { return Vec::new() };
        if self.shared() { server.not_members().into_iter().cloned().collect() } else { server.not_listed().into_iter().cloned().collect() }
    }

    /// Letting `person` in, as this host's server lets people in.
    fn letting_in(&self, person: &Person) -> Action {
        if self.shared() { Action::Join(person.name.clone()) } else { Action::Allow(person.clone()) }
    }
}

struct Live {
    managed: Managed,
    /// The piece of work it is at: one overtaken says nothing more.
    /// (Counted for the program, not for the host: work begun for a
    /// host whose window has closed isn't taken for the next one's.)
    turn: u64,
    task: Option<tokio::task::AbortHandle>,
}

static LIVE: Mutex<Option<Live>> = Mutex::new(None);
static TURNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The next piece of work's number.
fn next_turn() -> u64 {
    TURNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

/// Whether the work of turn `turn` is still what the window is at.
fn ours(turn: u64) -> bool {
    live().as_ref().is_some_and(|l| l.turn == turn)
}

fn live() -> olo_sync::MutexGuard<'static, Option<Live>> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The host being looked after, if one is.
pub fn current() -> Option<Managed> {
    live().as_ref().map(|l| l.managed.clone())
}

/// Changes what the work of turn `turn` found (nothing if another has
/// begun since); the windows are told.
fn set(turn: u64, change: impl FnOnce(&mut Managed)) {
    {
        let mut live = live();
        let Some(live) = live.as_mut().filter(|l| l.turn == turn) else { return };
        change(&mut live.managed);
    }
    crate::ui::server_changed();
}

/// Begins a piece of work for the host, on the hub's threads; the stage
/// is `stage` meanwhile.
fn work<F, Fut>(hub: &Arc<RexHub>, stage: Stage, f: F)
where
    F: FnOnce(Arc<RexHub>, Managed, u64) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let begun = {
        let mut live = live();
        let Some(live) = live.as_mut() else { return };
        if let Some(task) = live.task.take() {
            task.abort();
        }
        live.turn = next_turn();
        live.managed.stage = stage;
        (live.managed.clone(), live.turn)
    };
    let task = hub.spawn(f(hub.clone(), begun.0, begun.1));
    if let Some(live) = live().as_mut().filter(|l| l.turn == begun.1) {
        live.task = Some(task);
    }
    crate::ui::server_changed();
}

/// Rex's demo (`OLO_REX_DEMO=1`): nothing is connected to, and the
/// window shows a made-up host's server ([`demo`]).
pub fn demo_mode() -> bool {
    static DEMO: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DEMO.get_or_init(|| std::env::var("OLO_REX_DEMO").ok().as_deref() == Some("1"))
}

/// Opens `target`'s host's server, which goes by `name` here: root is
/// signed in and the host asked how its server stands. Nothing of the
/// server's is changed by it.
pub fn open(hub: &Arc<RexHub>, name: String, target: SshTarget) {
    if demo_mode() {
        show_demo("login");
        crate::ui::server_changed();
        return;
    }
    // (Signed in to with the agent Rex is set to use.)
    let target = hub.with_agent(target);
    // (Asked for again while it shows: the window comes to the front,
    // and root isn't signed in a second time.)
    if current().is_some_and(|now| now.target == target && !matches!(now.stage, Stage::Failed { .. } | Stage::RootKey { .. })) {
        crate::ui::server_changed();
        return;
    }
    close(hub);
    let managed = Managed {
        name,
        target,
        root_key: None,
        stage: Stage::Looking,
        server: None,
        read: None,
        page: Page::Server,
        log: None,
        log_read: None,
        log_of: Service::Login,
        log_range: LogRange::default(),
        log_reading: false,
    };
    *live() = Some(Live { managed, turn: next_turn(), task: None });
    look(hub);
}

/// Asks the host, as root, how its server stands (and its package
/// manager, whether Rex's repository has a newer version).
fn look(hub: &Arc<RexHub>) {
    work(hub, Stage::Looking, |hub, managed, turn| async move {
        let mut runner = hub.over_ssh(managed.target.clone(), managed.root_key.clone());
        match manage::look(&mut runner, true).await {
            Ok(server) => set(turn, |m| shown(m, server)),
            Err(failed) => {
                let stage = could_not_look(&hub, &managed, failed).await;
                set(turn, |m| m.stage = stage);
            }
        }
    });
}

/// The server read: shown, with the log of the service that runs.
fn shown(managed: &mut Managed, mut server: Server) {
    // (What Rex's repository said as the window opened still holds
    // where this look didn't ask it: the same version is installed.)
    if !server.checked
        && let Some(was) = managed.server.as_ref().filter(|was| was.checked && was.installed == server.installed)
    {
        server.checked = true;
        server.newer = was.newer.clone();
    }
    managed.log_of = if server.state(Service::Shared).used() && !server.state(Service::Login).used() { Service::Shared } else { Service::Login };
    managed.log = None;
    managed.server = Some(server);
    managed.read = Some(std::time::Instant::now());
    managed.stage = Stage::Shown;
}

async fn could_not_look(hub: &Arc<RexHub>, managed: &Managed, failed: Failed) -> Stage {
    hub.forget_ssh(&managed.target.as_root(managed.root_key.clone())).await;
    if failed.refused {
        let keys = hub.agent_keys().await;
        let chosen = managed.root_key.as_ref().and_then(|tried| keys.iter().position(|k| k.fingerprint == *tried)).unwrap_or(0);
        return Stage::RootKey { keys, chosen };
    }
    Stage::Failed { what: olo_kit::tr!("Couldn’t look at {}", managed.name).into(), why: olo_kit::tr!("Nothing was changed on {}.", managed.name).into(), said: failed.why, icon: "stop", log: None }
}

/// A key chosen for root in the menu.
pub fn choose_key(i: usize) {
    if let Some(Live { managed: Managed { stage: Stage::RootKey { chosen, keys }, .. }, .. }) = live().as_mut()
        && i < keys.len()
    {
        *chosen = i;
    }
    crate::ui::server_changed();
}

/// Looks again: with the key chosen for root, if one was being chosen.
pub fn again(hub: &Arc<RexHub>) {
    if demo_mode() {
        return;
    }
    {
        let mut live = live();
        let Some(live) = live.as_mut() else { return };
        if let Stage::RootKey { keys, chosen } = &live.managed.stage {
            live.managed.root_key = keys.get(*chosen).map(|k| k.fingerprint.clone());
        }
    }
    look(hub);
}

/// Puts `action` to the person, from the page shown.
pub fn ask(action: Action) {
    if let Some(live) = live().as_mut()
        && live.managed.stage == Stage::Shown
    {
        live.managed.stage = Stage::Confirm { action, choose: false };
    }
    crate::ui::server_changed();
}

/// Puts letting somebody in to the person (Allow… or Add…, on the
/// People page): the first of those who could be, another to choose in
/// the alert. Nothing, where there is nobody to let in.
pub fn ask_to_let_in() {
    if let Some(live) = live().as_mut()
        && live.managed.stage == Stage::Shown
        && let Some(first) = live.managed.could_join().first()
    {
        live.managed.stage = Stage::Confirm { action: live.managed.letting_in(first), choose: true };
    }
    crate::ui::server_changed();
}

/// Somebody else chosen to be let in, in the alert that asks.
pub fn choose_person(i: usize) {
    if let Some(live) = live().as_mut()
        && matches!(live.managed.stage, Stage::Confirm { choose: true, .. })
        && let Some(person) = live.managed.could_join().get(i)
    {
        live.managed.stage = Stage::Confirm { action: live.managed.letting_in(person), choose: true };
    }
    crate::ui::server_changed();
}

/// Back to the page shown: from what was asked, or from what didn't go
/// (where the server is known; else there is nothing to go back to).
pub fn back() {
    if let Some(live) = live().as_mut()
        && live.managed.server.is_some()
        && matches!(live.managed.stage, Stage::Confirm { .. } | Stage::Failed { .. })
    {
        live.managed.stage = Stage::Shown;
    }
    crate::ui::server_changed();
}

/// From what didn't go to the log that may say why.
pub fn show_log(hub: &Arc<RexHub>) {
    let of = {
        let mut live = live();
        let Some(live) = live.as_mut().filter(|l| l.managed.server.is_some()) else { return };
        let Stage::Failed { log: Some(of), .. } = live.managed.stage else { return };
        live.managed.stage = Stage::Shown;
        live.managed.page = Page::Log;
        of
    };
    read_log(hub, Some(of), None);
}

/// A step's title after "Couldn’t" ("Stop the shared server": "stop the
/// shared server").
fn after_couldnt(title: &str) -> String {
    let mut letters = title.chars();
    letters.next().map(|first| first.to_lowercase().chain(letters).collect()).unwrap_or_default()
}

/// How a service that was to run stands after it didn't: said under
/// what didn't go.
fn still(server: Option<&Server>, action: &Action) -> String {
    let nothing_else = olo_kit::words::tr("Nothing else was changed.").to_string();
    match (server, action) {
        (Some(server), Action::Start(service) | Action::Restart(service)) if !server.state(*service).active => {
            let stopped = match service {
                Service::Login => olo_kit::words::tr("The login server isn’t running."),
                Service::Shared => olo_kit::words::tr("The shared server isn’t running."),
            };
            format!("{stopped} {nothing_else}")
        }
        _ => nothing_else,
    }
}

/// The person agreed: the action is done, and the host looked at again.
pub fn go(hub: &Arc<RexHub>) {
    let Some(Managed { stage: Stage::Confirm { action, .. }, server: Some(server), .. }) = current() else { return };
    if demo_mode() {
        return back();
    }
    work(hub, Stage::Working { action: action.clone(), said: String::new() }, |hub, managed, turn| async move {
        let mut runner = hub.over_ssh(managed.target.clone(), managed.root_key.clone());
        let mut told = std::time::Instant::now();
        let done = manage::act(&mut runner, &server, &action, &mut |line| {
            {
                let mut live = live();
                let Some(live) = live.as_mut().filter(|l| l.turn == turn) else { return };
                live.managed.stage = Stage::Working { action: action.clone(), said: line.trim().to_owned() };
            }
            if told.elapsed() >= std::time::Duration::from_millis(100) {
                told = std::time::Instant::now();
                crate::ui::server_changed();
            }
        })
        .await;
        // (The window closed while it ran: it was let finish, and
        // root's connection, kept for it, goes now.)
        if !ours(turn) {
            hub.forget_ssh(&managed.target.as_root(managed.root_key.clone())).await;
            return;
        }
        match done {
            Ok(server) => set(turn, |m| shown(m, server)),
            Err(failed) => {
                // (How it stands now, whatever became of the action: a
                // service that didn't start is shown stopped.)
                let now = manage::look(&mut runner, false).await.ok();
                set(turn, |m| {
                    let why = still(now.as_ref(), &action);
                    if let Some(now) = now {
                        m.server = Some(now);
                        m.read = Some(std::time::Instant::now());
                        m.log = None;
                    }
                    let log = match &action {
                        Action::Start(service) | Action::Restart(service) | Action::Stop(service) => Some(*service),
                        _ => None,
                    };
                    m.stage = Stage::Failed { what: olo_kit::tr!("Couldn’t {} on {}", after_couldnt(&olo_kit::words::of(action.title())), m.name).into(), why, said: failed.why, icon: "stop", log };
                });
            }
        }
    });
}

/// Another page is shown; the Log page reads its log the first time.
pub fn page(hub: &Arc<RexHub>, page: Page) {
    let read = {
        let mut live = live();
        let Some(live) = live.as_mut() else { return };
        live.managed.page = page;
        page == Page::Log && live.managed.log.is_none() && !live.managed.log_reading && live.managed.server.is_some() && !demo_mode()
    };
    if read {
        read_log(hub, None, None);
    }
    crate::ui::server_changed();
}

/// Reads the log again (`service`: another's than the one shown;
/// `range`: over another range). It doesn't stand in the way of the
/// page: the old lines stay until the new come.
pub fn read_log(hub: &Arc<RexHub>, service: Option<Service>, range: Option<LogRange>) {
    let Some((managed, of, over)) = ({
        let mut live = live();
        live.as_mut().map(|live| {
            if let Some(service) = service.filter(|s| *s != live.managed.log_of) {
                live.managed.log_of = service;
                live.managed.log = None;
            }
            if let Some(range) = range {
                live.managed.log_range = range;
            }
            live.managed.log_reading = !demo_mode();
            (live.managed.clone(), live.managed.log_of, live.managed.log_range)
        })
    }) else {
        return;
    };
    crate::ui::server_changed();
    if demo_mode() {
        return;
    }
    let reader = hub.clone();
    hub.spawn(async move {
        let mut runner = reader.over_ssh(managed.target.clone(), managed.root_key.clone());
        let lines = match manage::journal(&mut runner, of, over).await {
            Ok(lines) => lines,
            Err(failed) => vec![failed.why],
        };
        {
            let mut live = live();
            // (Asked of another service or range meanwhile: that one's answer is the one shown.)
            if let Some(live) = live.as_mut().filter(|l| l.managed.target == managed.target && l.managed.log_of == of && l.managed.log_range == over) {
                live.managed.log = Some(lines);
                live.managed.log_read = Some(std::time::Instant::now());
                live.managed.log_reading = false;
            }
        }
        // (The window closed while it was read: the connection the
        // reading made for root isn't left behind.)
        if !current().is_some_and(|now| now.target == managed.target) {
            reader.forget_ssh(&managed.target.as_root(managed.root_key.clone())).await;
        }
        crate::ui::server_changed();
    });
}

/// The window closed (or another host's server is opened in it): work
/// that changes nothing on the host is ended at once, and root's
/// connection dropped; a command being run there is let finish (cut,
/// a package would be left half installed), nothing more is said of
/// it, and root's connection goes when it has ([`go`]).
pub fn close(hub: &Arc<RexHub>) {
    let gone = live().take();
    if let Some(mut gone) = gone {
        if matches!(gone.managed.stage, Stage::Working { .. }) {
            return;
        }
        if let Some(task) = gone.task.take() {
            task.abort();
        }
        let root = gone.managed.target.as_root(gone.managed.root_key.clone());
        let hub2 = hub.clone();
        hub.spawn(async move { hub2.forget_ssh(&root).await });
    }
}

/// A made-up host's server, for the window's pictures (Rex's demo:
/// nothing is connected to). `named`: `looking`, `login`, `shared`,
/// `stopped`, `failed-service`, `unused`, `current`, `people`,
/// `people-empty`, `members`, `log`, `log-reading`, `log-empty`,
/// `confirm`, `confirm-stop`, `confirm-update`, `working`, `failed`,
/// `key`, `nokeys`.
pub fn demo(named: &str) -> Option<Managed> {
    use olo_rex_client::accounts::Accounts;
    use olo_rex_client::provision::{Facts, Unit};
    let on = Unit { active: true, enabled: true };
    let people = vec![Person { name: "alice".into(), uid: 1000 }, Person { name: "bo".into(), uid: 1001 }, Person { name: "carmen".into(), uid: 1002 }, Person { name: "dev".into(), uid: 1003 }];
    let facts = |login: Unit, shared: Unit| Facts {
        os: "Linux".into(),
        distro: "ubuntu".into(),
        user: "root".into(),
        rex: Some("/usr/bin/rex".into()),
        version: Some("rex 0.1.0-snapshot.2506".into()),
        packaged: true,
        systemd: true,
        login,
        login_socket: Unit::default(),
        shared,
        shared_socket: Unit::default(),
        fetch: Some(olo_rex_client::provision::Fetch::Curl),
        ..Facts::default()
    };
    let mut accounts = Accounts::parse(olo_rex_client::accounts::EMPTY).ok()?;
    for person in &people[..2] {
        let principal = format!("peer:{}", person.uid);
        accounts.allow(&person.name, &principal).ok()?;
        accounts.set_default(&principal, Some(&person.name)).ok()?;
    }
    let login = Server {
        facts: facts(on, Unit::default()),
        login_up: Some(2 * 3600 + 9 * 60),
        people: people.clone(),
        installed: Some("0.1.0~alpha.2506".into()),
        newer: Some("0.1.0~alpha.2510".into()),
        checked: true,
        sessions: vec![("alice".into(), 2), ("bo".into(), 1)],
        asked_sessions: true,
        accounts: Some(accounts),
        ..Server::default()
    };
    let shared = Server {
        facts: facts(Unit::default(), on),
        shared_up: Some(2 * 86_400 + 4 * 3600),
        members: vec!["alice".into(), "bo".into()],
        people: people.clone(),
        installed: Some("0.1.0~alpha.2510".into()),
        checked: true,
        sessions: vec![("rex".into(), 3)],
        asked_sessions: true,
        ..Server::default()
    };
    let log: Vec<String> = [
        "2026-11-22",
        "14:03:10 systemd[1]: Starting rex-login-server.service - Rex Login Server...",
        "14:03:11 INFO  login server starting accounts=/etc/rex/accounts.jsonc",
        "14:03:11 systemd[1]: Started rex-login-server.service - Rex Login Server.",
        "14:05:42 INFO  account server started account=alice",
        "15:31:09 INFO  account server started account=bo",
        "16:12:55 WARN  sign-in refused: no account lists the caller",
        "16:13:20 ERROR account server for bo exited: status 1",
        "23:58:02 systemd[1]: Reloading rex-login-server.service - Rex Login Server...",
        "23:58:02 INFO  account policy reloaded revoked_requests=0",
        "2026-11-23",
        "00:00:14 systemd[1]: Reloaded rex-login-server.service - Rex Login Server.",
    ]
    .iter()
    .map(|l| (*l).to_owned())
    .collect();
    let keys = vec![
        AgentKey { name: "work laptop".into(), kind: "ED25519".into(), fingerprint: "SHA256:Zk1nC2aqD0b8u3mY5pQeR7tUxNcA".into() },
        AgentKey { name: "build servers".into(), kind: "ED25519".into(), fingerprint: "SHA256:9fT2LmQw4hXc7Vb1KdJrE6sNuPzB".into() },
    ];
    let base = Managed {
        name: "build".into(),
        // (As the demo's Hosts window has it.)
        target: SshTarget::parse("ssh://you@build.example").ok()?,
        root_key: None,
        stage: Stage::Shown,
        server: Some(login.clone()),
        read: Some(std::time::Instant::now()),
        page: Page::Server,
        log: None,
        log_read: None,
        log_of: Service::Login,
        log_range: LogRange::default(),
        log_reading: false,
    };
    let enabled = Unit { active: false, enabled: true };
    Some(match named {
        "looking" => Managed { stage: Stage::Looking, server: None, ..base },
        "login" => base,
        "shared" => Managed { server: Some(shared), log_of: Service::Shared, ..base },
        // (It ended by itself and wasn't started again; it failed; neither kind is in use.)
        "stopped" => Managed { server: Some(Server { facts: facts(enabled, Unit::default()), login_up: None, sessions: Vec::new(), ..login }), ..base },
        "failed-service" => Managed {
            server: Some(Server {
                facts: facts(enabled, Unit::default()),
                login_up: None,
                login_failed: true,
                login_ended: Some(4 * 60 + 10),
                login_last: Some("could not read /etc/rex/accounts.jsonc: line 12, column 3: a comma is missing after the account \"alice\"".into()),
                sessions: Vec::new(),
                ..login
            }),
            ..base
        },
        "unused" => Managed {
            server: Some(Server { facts: facts(Unit::default(), Unit::default()), login_up: None, sessions: Vec::new(), newer: None, installed: Some("0.1.0~alpha.2510".into()), ..login }),
            ..base
        },
        // (Nothing newer in the repository.)
        "current" => Managed { server: Some(Server { newer: None, installed: Some("0.1.0~alpha.2510".into()), ..login }), ..base },
        "people" => Managed { page: Page::People, ..base },
        // (Nobody listed yet: as the package ships the file.)
        "people-empty" => Managed { server: Some(Server { accounts: Accounts::parse(olo_rex_client::accounts::EMPTY).ok(), sessions: Vec::new(), ..login }), page: Page::People, ..base },
        "members" => Managed { server: Some(shared), page: Page::People, log_of: Service::Shared, ..base },
        "log" => Managed { page: Page::Log, log: Some(log), log_read: Some(std::time::Instant::now()), ..base },
        "log-reading" => Managed { page: Page::Log, log: None, log_reading: true, ..base },
        "log-empty" => Managed { page: Page::Log, log: Some(Vec::new()), log_read: Some(std::time::Instant::now()), log_range: LogRange::Today, ..base },
        "confirm" => Managed { stage: Stage::Confirm { action: Action::Allow(people[2].clone()), choose: true }, page: Page::People, ..base },
        "confirm-stop" => Managed { stage: Stage::Confirm { action: Action::Stop(Service::Shared), choose: false }, server: Some(shared), log_of: Service::Shared, ..base },
        "confirm-update" => Managed { stage: Stage::Confirm { action: Action::Update, choose: false }, ..base },
        "working" => Managed { stage: Stage::Working { action: Action::Update, said: "Unpacking rex (0.1.0~alpha.2510) over (0.1.0~alpha.2506) ...".into() }, ..base },
        "failed" => Managed {
            stage: Stage::Failed {
                what: "Couldn’t start the shared server on build".into(),
                why: "The shared server isn’t running. Nothing else was changed.".into(),
                said: "Job for rex.service failed because the control process exited with error code.\n16:20:03 error: the rex account has no shell".into(),
                icon: "stop",
                log: Some(Service::Shared),
            },
            server: Some(Server { facts: facts(Unit::default(), Unit::default()), login_up: None, sessions: Vec::new(), ..login }),
            ..base
        },
        "key" => Managed { stage: Stage::RootKey { keys, chosen: 0 }, server: None, ..base },
        "nokeys" => Managed { stage: Stage::RootKey { keys: Vec::new(), chosen: 0 }, server: None, ..base },
        _ => return None,
    })
}

/// Shows the demo's stage `named` in the window (Rex's demo only).
pub fn show_demo(named: &str) -> bool {
    let Some(managed) = demo(named) else { return false };
    *live() = Some(Live { managed, turn: next_turn(), task: None });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stages of the demo.
    const DEMO_STAGES: [&str; 20] = [
        "looking",
        "login",
        "shared",
        "stopped",
        "failed-service",
        "unused",
        "current",
        "people",
        "people-empty",
        "members",
        "log",
        "log-reading",
        "log-empty",
        "confirm",
        "confirm-stop",
        "confirm-update",
        "working",
        "failed",
        "key",
        "nokeys",
    ];

    /// (The window's state is one for the program: these go one at a time.)
    static ONE: Mutex<()> = Mutex::new(());

    #[test]
    fn what_is_asked_is_put_to_the_person_and_can_be_left() {
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        assert!(show_demo("login"));
        let stop = Action::Stop(Service::Login);
        ask(stop.clone());
        assert_eq!(current().unwrap().stage, Stage::Confirm { action: stop, choose: false });
        back();
        assert_eq!(current().unwrap().stage, Stage::Shown);
        // Nothing is asked over what is being asked, or while it looks.
        assert!(show_demo("looking"));
        ask(Action::Update);
        assert_eq!(current().unwrap().stage, Stage::Looking);
        // With no server read there is nothing to go back to.
        back();
        assert_eq!(current().unwrap().stage, Stage::Looking);
        *live() = None;
    }

    /// Work begun for one host is never taken for another's: each
    /// piece has a number of its own, whichever host it is for.
    #[test]
    fn a_piece_of_work_is_told_from_every_other() {
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        assert!(show_demo("login"));
        let first = live().as_ref().unwrap().turn;
        assert!(ours(first));
        assert_ne!(first, 0);
        // (The window closed, and opened on another host's server.)
        *live() = None;
        assert!(!ours(first));
        assert!(show_demo("shared"));
        // (Read in a statement of its own: the lock isn't held past it.)
        let second = live().as_ref().unwrap().turn;
        assert!(!ours(first) && ours(second));
        *live() = None;
    }

    #[test]
    fn who_is_let_in_is_chosen_in_the_alert_and_goes_by_the_kind_of_server() {
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        let names = |m: &Managed| m.could_join().into_iter().map(|p| p.name).collect::<Vec<_>>();
        assert_eq!(names(&demo("people").unwrap()), vec!["carmen", "dev"]);
        assert_eq!(names(&demo("members").unwrap()), vec!["carmen", "dev"]);
        // A login server's accounts are allowed; the first who could be, then another chosen.
        assert!(show_demo("people"));
        ask_to_let_in();
        assert_eq!(current().unwrap().stage, Stage::Confirm { action: Action::Allow(Person { name: "carmen".into(), uid: 1002 }), choose: true });
        choose_person(1);
        assert_eq!(current().unwrap().stage, Stage::Confirm { action: Action::Allow(Person { name: "dev".into(), uid: 1003 }), choose: true });
        // A shared server's people are added to its group.
        assert!(show_demo("members"));
        ask_to_let_in();
        assert_eq!(current().unwrap().stage, Stage::Confirm { action: Action::Join("carmen".into()), choose: true });
        // What is asked without a choice has nobody chosen into it.
        back();
        ask(Action::Restart(Service::Shared));
        choose_person(1);
        assert_eq!(current().unwrap().stage, Stage::Confirm { action: Action::Restart(Service::Shared), choose: false });
        *live() = None;
    }

    #[test]
    fn what_didnt_go_says_how_the_service_stands_and_every_stage_is_there() {
        assert_eq!(after_couldnt("Stop the shared server"), "stop the shared server");
        let stopped = demo("unused").unwrap().server.unwrap();
        assert_eq!(still(Some(&stopped), &Action::Start(Service::Shared)), "The shared server isn’t running. Nothing else was changed.");
        let running = demo("login").unwrap().server.unwrap();
        assert_eq!(still(Some(&running), &Action::Restart(Service::Login)), "Nothing else was changed.");
        assert_eq!(still(None, &Action::Update), "Nothing else was changed.");
        for stage in DEMO_STAGES {
            assert!(demo(stage).is_some(), "{stage}");
        }
    }
}
