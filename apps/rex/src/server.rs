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
    provision::{Failed, RootBy},
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
    /// How root's work reaches root: with sudo where the account may use
    /// it without a password, else a root login; none until the host's
    /// first look asks (`provision::can_sudo`).
    pub root_by: Option<RootBy>,
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
    /// Identifies each confirmation opened so its scroll view starts at the
    /// top when opened and keeps its position across redraws of the same one.
    pub confirm_id: u64,
}

impl Managed {
    /// Who runs root's work, as it is said to the person, once, over what
    /// is run ("Rex runs this as {}:"): `root@HOST`, or root through the
    /// account's sudo ("root, with sudo as alice": "as alice with sudo"
    /// read as broken, both looks).
    pub fn root(&self) -> String {
        match self.root_by {
            Some(RootBy::Sudo) => olo_kit::tr!("root, with sudo as {}", self.target.user.as_deref().unwrap_or_default()).into(),
            _ => format!("root@{}", self.target.host),
        }
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
    /// Opened via Manage Host's Update… button: once the first look
    /// finishes, go straight into the Update confirmation unless the
    /// server was already confirmed up to date.
    pending_update: bool,
}

/// Whose record of a host this is. There are two places a host is
/// looked after as root: the Server window (its Rex server), and the
/// Host window's Tweaks tab (the machine's own settings: Ben,
/// 2026-11-26, of the limits that were a page of the Server window:
/// "they really belong in manage host"). Each has a record of its own,
/// so the two windows can be of different hosts, and one's work is
/// never taken for the other's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Of {
    Server,
    Tweaks,
}

static LIVE: [Mutex<Option<Live>>; 2] = [Mutex::new(None), Mutex::new(None)];
static TURNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static CONFIRMS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_confirm() -> u64 {
    CONFIRMS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

thread_local! {
    /// Whose record what is asked on this thread is of ([`for_tweaks`]).
    static OF: std::cell::Cell<Of> = const { std::cell::Cell::new(Of::Server) };
}

/// Runs `f` for the Host window's Tweaks tab's record: everything this
/// module is asked within it (`open`, `current`, `ask`, `go`, `back`,
/// `close`…) is of that record, and the work it begins stays of it
/// wherever it runs (work knows its record by its turn).
pub fn for_tweaks<R>(f: impl FnOnce() -> R) -> R {
    OF.with(|of| {
        let was = of.replace(Of::Tweaks);
        let done = f();
        of.set(was);
        done
    })
}

/// The windows that show `of`'s record are told it changed.
fn told(of: Of) {
    match of {
        Of::Server => crate::ui::server_changed(),
        Of::Tweaks => crate::ui::tweaks_changed(),
    }
}

/// Whose record is meant here, now.
fn which() -> Of {
    OF.with(std::cell::Cell::get)
}

fn live_of(of: Of) -> olo_sync::MutexGuard<'static, Option<Live>> {
    LIVE[of as usize].lock().unwrap_or_else(|e| e.into_inner())
}

/// The record the work of turn `turn` is of (the Server window's where
/// it is nobody's any more).
fn of_turn(turn: u64) -> Of {
    if live_of(Of::Tweaks).as_ref().is_some_and(|l| l.turn == turn) { Of::Tweaks } else { Of::Server }
}

/// The next piece of work's number.
fn next_turn() -> u64 {
    TURNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

/// Whether the work of turn `turn` is still what the window is at.
fn ours(turn: u64) -> bool {
    live_of(of_turn(turn)).as_ref().is_some_and(|l| l.turn == turn)
}

fn live() -> olo_sync::MutexGuard<'static, Option<Live>> {
    live_of(which())
}

/// The host being looked after, if one is.
pub fn current() -> Option<Managed> {
    live().as_ref().map(|l| l.managed.clone())
}

/// Changes what the work of turn `turn` found (nothing if another has
/// begun since); the windows are told.
fn set(turn: u64, change: impl FnOnce(&mut Managed)) {
    let of = of_turn(turn);
    {
        let mut live = live_of(of);
        let Some(live) = live.as_mut().filter(|l| l.turn == turn) else { return };
        change(&mut live.managed);
    }
    told(of);
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
    told(which());
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
        told(which());
        return;
    }
    // (Signed in to with the agent Rex is set to use.)
    let target = hub.with_agent(target);
    // (Asked for again while it shows: the window comes to the front,
    // and root isn't signed in a second time.)
    if current().is_some_and(|now| now.target == target && !matches!(now.stage, Stage::Failed { .. } | Stage::RootKey { .. })) {
        told(which());
        return;
    }
    close(hub);
    let managed = Managed {
        name,
        target,
        root_key: None,
        root_by: None,
        stage: Stage::Looking,
        server: None,
        read: None,
        page: Page::Server,
        log: None,
        log_read: None,
        log_of: Service::Login,
        log_range: LogRange::default(),
        log_reading: false,
        confirm_id: 0,
    };
    *live() = Some(Live { managed, turn: next_turn(), task: None, pending_update: false });
    look(hub);
}

/// Opens the Server window and steps straight into updating Rex once
/// the host's server has been looked at (or at once if already shown).
pub fn open_for_update(hub: &Arc<RexHub>, name: String, target: SshTarget) {
    if demo_mode() {
        show_demo("login");
        ask(Action::Update);
        return;
    }
    let resolved = hub.with_agent(target.clone());
    if current().is_some_and(|now| now.target == resolved && now.stage == Stage::Shown) {
        ask(Action::Update);
        return;
    }
    open(hub, name, target);
    if let Some(live) = live().as_mut() {
        live.pending_update = true;
    }
}

/// Asks the host, as root, how its server stands (and its package
/// manager, whether Rex's repository has a newer version). The first
/// look asks first how root is reached: with sudo, where the account
/// may use it without a password, else a root login (Ben, 2026-11-24).
fn look(hub: &Arc<RexHub>) {
    work(hub, Stage::Looking, |hub, managed, turn| async move {
        let by = match managed.root_by {
            Some(by) => by,
            None => {
                let mut account = hub.over_ssh(managed.target.clone(), None, RootBy::Login);
                let by = if olo_rex_client::provision::can_sudo(&mut account).await { RootBy::Sudo } else { RootBy::Login };
                set(turn, |m| m.root_by = Some(by));
                by
            }
        };
        let mut runner = hub.over_ssh(managed.target.clone(), managed.root_key.clone(), by);
        match manage::look(&mut runner, true).await {
            Ok(server) => {
                hub.set_server_newer(&managed.target, server.newer.clone());
                {
                    let mut guard = live_of(of_turn(turn));
                    if let Some(live) = guard.as_mut().filter(|l| l.turn == turn) {
                        let wants_update = std::mem::take(&mut live.pending_update);
                        let should_confirm = wants_update && (server.newer.is_some() || !server.checked);
                        shown(&mut live.managed, server);
                        if should_confirm {
                            live.managed.confirm_id = next_confirm();
                            live.managed.stage = Stage::Confirm { action: Action::Update, choose: false };
                        }
                    }
                }
                told(of_turn(turn));
            }
            Err(failed) => {
                let mut stage = could_not_look(&hub, &managed, failed).await;
                // (On the Host window's tab it is the host's settings
                // that were to be read, with Read Settings.)
                if let (Of::Tweaks, Stage::Failed { what, .. }) = (of_turn(turn), &mut stage) {
                    *what = olo_kit::tr!("Couldn’t read {}’s settings", managed.name).into();
                }
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
    told(which());
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
        live.managed.confirm_id = next_confirm();
        live.managed.stage = Stage::Confirm { action, choose: false };
    }
    told(which());
}

/// Puts letting somebody in to the person (Allow… or Add…, on the
/// People page): the first of those who could be, another to choose in
/// the alert. Nothing, where there is nobody to let in.
pub fn ask_to_let_in() {
    if let Some(live) = live().as_mut()
        && live.managed.stage == Stage::Shown
        && let Some(first) = live.managed.could_join().first()
    {
        live.managed.confirm_id = next_confirm();
        live.managed.stage = Stage::Confirm { action: live.managed.letting_in(first), choose: true };
    }
    told(which());
}

/// Somebody else chosen to be let in, in the alert that asks.
pub fn choose_person(i: usize) {
    if let Some(live) = live().as_mut()
        && matches!(live.managed.stage, Stage::Confirm { choose: true, .. })
        && let Some(person) = live.managed.could_join().get(i)
    {
        live.managed.confirm_id = next_confirm();
        live.managed.stage = Stage::Confirm { action: live.managed.letting_in(person), choose: true };
    }
    told(which());
}

/// In the question that gives a host brush: whether the accounts there
/// are get it too (`olo_rex_client::tune::Tuning::everyone`).
pub fn everyone(on: bool) {
    if let Some(live) = live().as_mut()
        && matches!(live.managed.stage, Stage::Confirm { .. })
        && let Some(tuning) = live.managed.server.as_mut().and_then(|server| server.tuning.as_mut())
    {
        tuning.everyone = on;
    }
    told(which());
}

/// Back to the page shown: from what was asked, or from what didn't go
/// (where the server is known; else there is nothing to go back to).
pub fn back() {
    if let Some(live) = live().as_mut()
        && live.managed.server.is_some()
        && matches!(live.managed.stage, Stage::Confirm { .. } | Stage::Failed { .. })
    {
        live.managed.stage = Stage::Shown;
        // (A choice made in a question is that question's.)
        if let Some(tuning) = live.managed.server.as_mut().and_then(|server| server.tuning.as_mut()) {
            tuning.everyone = false;
        }
    }
    told(which());
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
        // (Several tweaks at once are each applied by itself: one
        // that couldn't be doesn't keep the others from it, and
        // "nothing else was changed" would be untrue of them.)
        (_, Action::Tune(kinds)) if kinds.len() > 1 => olo_kit::words::tr("What could be applied was: the list shows how each stands now.").to_string(),
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
        let mut runner = hub.over_ssh(managed.target.clone(), managed.root_key.clone(), managed.root_by.unwrap_or_default());
        let mut last_told = std::time::Instant::now();
        let done = manage::act(&mut runner, &server, &action, &mut |line| {
            {
                let mut live = live_of(of_turn(turn));
                let Some(live) = live.as_mut().filter(|l| l.turn == turn) else { return };
                // (What a script here says it is at, less its mark.)
                live.managed.stage = Stage::Working { action: action.clone(), said: olo_rex_client::tune::shown_line(line).to_owned() };
            }
            if last_told.elapsed() >= std::time::Duration::from_millis(100) {
                last_told = std::time::Instant::now();
                told(of_turn(turn));
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
                    // (What a script of Olo's said of why it stopped is the
                    // alert's own words, before how things stand; what the
                    // host's tools said stays as it was said, in the well.
                    // "Brush couldn't be downloaded from GitHub." was set in
                    // the well as if the host had printed it: Opus's look.)
                    let (own, tools) = if matches!(action, Action::Tune(_) | Action::Untune(_)) { olo_rex_client::tune::parted(&failed.why) } else { (String::new(), failed.why.clone()) };
                    // (Olo's own sentence says what became of it, "it was
                    // taken out again", "nothing was installed": "nothing
                    // else was changed" after it read as if something had
                    // been, both looks.)
                    let why = if own.is_empty() { still(now.as_ref(), &action) } else { own };
                    if let Some(now) = now {
                        m.server = Some(now);
                        m.read = Some(std::time::Instant::now());
                        m.log = None;
                    }
                    let log = match &action {
                        Action::Start(service) | Action::Restart(service) | Action::Stop(service) => Some(*service),
                        _ => None,
                    };
                    m.stage = Stage::Failed { what: olo_kit::tr!("Couldn’t {} on {}", after_couldnt(&olo_kit::words::of(action.title())), m.name).into(), why, said: tools, icon: "stop", log };
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
    told(which());
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
    told(which());
    if demo_mode() {
        return;
    }
    let reader = hub.clone();
    // (Whose record it was asked of: where it is read, nobody says.)
    let whose = which();
    hub.spawn(async move {
        let mut runner = reader.over_ssh(managed.target.clone(), managed.root_key.clone(), managed.root_by.unwrap_or_default());
        let lines = match manage::journal(&mut runner, of, over).await {
            Ok(lines) => lines,
            Err(failed) => vec![failed.why],
        };
        {
            let mut live = live_of(whose);
            // (Asked of another service or range meanwhile: that one's answer is the one shown.)
            if let Some(live) = live.as_mut().filter(|l| l.managed.target == managed.target && l.managed.log_of == of && l.managed.log_range == over) {
                live.managed.log = Some(lines);
                live.managed.log_read = Some(std::time::Instant::now());
                live.managed.log_reading = false;
            }
        }
        // (The window closed while it was read: the connection the
        // reading made for root isn't left behind.)
        if !live_of(whose).as_ref().is_some_and(|now| now.managed.target == managed.target) {
            reader.forget_ssh(&managed.target.as_root(managed.root_key.clone())).await;
        }
        told(whose);
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
/// `key`, `nokeys`; each with `-sudo` after it, the account using sudo
/// for root's work.
pub fn demo(named: &str) -> Option<Managed> {
    let (named, root_by) = match named.strip_suffix("-sudo") {
        Some(named) => (named, RootBy::Sudo),
        None => (named, RootBy::Login),
    };
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
        tuning: Some(limits(0)),
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
        // (An account with a name, for the pictures: "you" read as a
        // pronoun in "with sudo as you", Opus's look.)
        target: SshTarget::parse("ssh://alice@build.example").ok()?,
        root_key: None,
        root_by: Some(root_by),
        stage: Stage::Shown,
        server: Some(login.clone()),
        read: Some(std::time::Instant::now()),
        page: Page::Server,
        log: None,
        log_read: None,
        log_of: Service::Login,
        log_range: LogRange::default(),
        log_reading: false,
        confirm_id: 0,
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
        // (The Host window's Tweaks, `OLO_REX_DEMO_TWEAKS`: a machine as
        // it comes; one with some applied; one made ready; what is
        // asked before one is applied, before all are, before one is
        // put back, and of brush, with the accounts there are or
        // without; one being applied; and one that couldn't be.)
        "limits" => base,
        "limits-some" => Managed { server: Some(Server { tuning: Some(limits(2)), ..login }), ..base },
        "limits-restart" => {
            let mut tuning = limits(olo_rex_client::tune::Tune::ALL.len() - 1);
            tuning.rex_limits.scope = olo_rex_client::tune::RexScope::Shared;
            tuning.rex_limits.running = Some(olo_rex_client::tune::RexLimitValues::defaults());
            Managed { server: Some(Server { tuning: Some(tuning), ..login }), ..base }
        }
        "limits-restart-user" => {
            let mut tuning = limits(olo_rex_client::tune::Tune::ALL.len() - 1);
            tuning.rex_limits.scope = olo_rex_client::tune::RexScope::User;
            tuning.rex_limits.user = Some("alice".into());
            tuning.rex_limits.running = Some(olo_rex_client::tune::RexLimitValues::defaults());
            Managed { server: Some(Server { tuning: Some(tuning), ..login }), ..base }
        }
        "limits-put-back-pending" => {
            let mut tuning = limits(olo_rex_client::tune::Tune::ALL.len() - 2);
            tuning.rex_limits.scope = olo_rex_client::tune::RexScope::Shared;
            tuning.rex_limits.running = Some(olo_rex_client::tune::RexLimitValues::wanted());
            Managed { server: Some(Server { tuning: Some(tuning), ..login }), ..base }
        }
        "limits-blocked" => {
            let mut tuning = limits(olo_rex_client::tune::Tune::ALL.len() - 2);
            tuning.rex_limits.blocked = Some("/etc/systemd/system/rex.service.d/20-site.conf sets this server’s command: raise its limits there.".into());
            Managed { server: Some(Server { tuning: Some(tuning), ..login }), ..base }
        }
        "limits-stale" => {
            let mut tuning = limits(olo_rex_client::tune::Tune::ALL.len());
            tuning.rex_limits.stale = true;
            tuning.rex_limits.stale_was = Some("/usr/bin/rex server run --log-format text".into());
            tuning.rex_limits.stale_now = Some("/usr/bin/rex server run --log-format text --new-flag".into());
            Managed { server: Some(Server { tuning: Some(tuning), ..login }), ..base }
        }
        "limits-ready" => Managed { server: Some(Server { tuning: Some(limits(olo_rex_client::tune::Tune::ALL.len())), ..login }), ..base },
        "confirm-limits" => {
            Managed { stage: Stage::Confirm { action: Action::Tune(olo_rex_client::tune::Tune::ALL.into_iter().filter(|t| *t != olo_rex_client::tune::Tune::Shell).collect()), choose: false }, ..base }
        }
        "confirm-limit" => Managed { stage: Stage::Confirm { action: Action::Tune(vec![olo_rex_client::tune::Tune::Kernel]), choose: false }, ..base },
        "confirm-writeback" => Managed { stage: Stage::Confirm { action: Action::Tune(vec![olo_rex_client::tune::Tune::Writeback]), choose: false }, ..base },
        "confirm-tmp" => Managed { stage: Stage::Confirm { action: Action::Tune(vec![olo_rex_client::tune::Tune::TempFiles]), choose: false }, ..base },
        "confirm-rex" => Managed { stage: Stage::Confirm { action: Action::Tune(vec![olo_rex_client::tune::Tune::RexLimits]), choose: false }, ..base },
        "confirm-rex-user" => {
            let mut tuning = limits(0);
            tuning.rex_limits.scope = olo_rex_client::tune::RexScope::User;
            tuning.rex_limits.user = Some("alice".into());
            tuning.rex_limits.file_usr = Some("/home/alice/.config/systemd/user/rex.service.d/90-olo-limits.conf".into());
            Managed { stage: Stage::Confirm { action: Action::Tune(vec![olo_rex_client::tune::Tune::RexLimits]), choose: false }, server: Some(Server { tuning: Some(tuning), ..login }), ..base }
        }
        "confirm-shell" => Managed { stage: Stage::Confirm { action: Action::Tune(vec![olo_rex_client::tune::Tune::Shell]), choose: false }, ..base },
        "confirm-shell-everyone" => Managed {
            stage: Stage::Confirm { action: Action::Tune(vec![olo_rex_client::tune::Tune::Shell]), choose: false },
            server: Some(Server { tuning: Some(olo_rex_client::tune::Tuning { everyone: true, ..limits(0) }), ..login }),
            ..base
        },
        "confirm-put-back" => Managed {
            stage: Stage::Confirm { action: Action::Untune(olo_rex_client::tune::Tune::TempFiles), choose: false },
            server: Some(Server { tuning: Some(limits(olo_rex_client::tune::Tune::ALL.len())), ..login }),
            ..base
        },
        "confirm-put-back-rex" => Managed {
            stage: Stage::Confirm { action: Action::Untune(olo_rex_client::tune::Tune::RexLimits), choose: false },
            server: Some(Server { tuning: Some(limits(olo_rex_client::tune::Tune::ALL.len())), ..login }),
            ..base
        },
        "confirm-put-back-rex-user" => {
            // Personal server whose unit already has higher --max-windows (1,024)
            // and --max-blocks (8,192), so Put Back only changes the 2 differing
            // limits (--max-panes and --max-sessions) as the connected account.
            let mut tuning = limits(olo_rex_client::tune::Tune::ALL.len());
            tuning.rex_limits.scope = olo_rex_client::tune::RexScope::User;
            tuning.rex_limits.user = Some("alice".into());
            tuning.rex_limits.file_usr = Some("/home/alice/.config/systemd/user/rex.service.d/90-olo-limits.conf".into());
            tuning.rex_limits.kept_sys = false;
            tuning.rex_limits.kept_usr = true;
            tuning.rex_limits.without_olo = Some(olo_rex_client::tune::RexLimitValues { windows: 1024, panes: 64, sessions: 64, blocks: 8192 });
            tuning.rex_limits.running = Some(olo_rex_client::tune::RexLimitValues { windows: 1024, panes: 128, sessions: 128, blocks: 8192 });
            if let Some(stands) = tuning.stands.iter_mut().find(|s| s.tune == olo_rex_client::tune::Tune::RexLimits) {
                for v in &mut stands.values {
                    match v.name {
                        "--max-windows" => v.now = Some(1024),
                        "--max-blocks" => v.now = Some(8192),
                        _ => {}
                    }
                }
            }
            Managed { stage: Stage::Confirm { action: Action::Untune(olo_rex_client::tune::Tune::RexLimits), choose: false }, server: Some(Server { tuning: Some(tuning), ..login }), ..base }
        }
        "confirm-put-back-shell" => Managed {
            stage: Stage::Confirm { action: Action::Untune(olo_rex_client::tune::Tune::Shell), choose: false },
            server: Some(Server { tuning: Some(limits(olo_rex_client::tune::Tune::ALL.len())), ..login }),
            ..base
        },
        "working-limits" => Managed { stage: Stage::Working { action: Action::Tune(vec![olo_rex_client::tune::Tune::Shell]), said: "Downloading brush 0.5.0 from GitHub".into() }, ..base },
        // (And the host couldn't be looked at: nothing of it is known.)
        "failed-look" => Managed {
            stage: Stage::Failed {
                what: olo_kit::tr!("Couldn’t read {}’s settings", base.name).into(),
                why: olo_kit::tr!("Nothing was changed on {}.", base.name).into(),
                said: "ssh: connect to host build.example port 22: Connection timed out".into(),
                icon: "stop",
                log: None,
            },
            server: None,
            ..base
        },
        "failed-limits" => Managed {
            stage: Stage::Failed {
                what: olo_kit::tr!("Couldn’t apply “{}” on {}", olo_kit::words::tr("Brush shell"), base.name).into(),
                why: olo_kit::words::tr("Brush couldn’t be downloaded from GitHub; nothing was installed.").into(),
                said: "curl: (6) Could not resolve host: github.com".into(),
                icon: "stop",
                log: None,
            },
            ..base
        },
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

/// The demo's machine (64 GB, its `/tmp` in memory, three people's
/// accounts on bash) with the first `applied` kinds of tweaks applied,
/// the rest as a machine comes.
fn limits(applied: usize) -> olo_rex_client::tune::Tuning {
    use olo_rex_client::tune::{Tune, Tuning};
    let comes = "ram 67108864\nMaxSessions 10\nopen files 524288\nDefaultLimitNOFILE 524288\nDefaultTasksMax 76000\nfs.file-max 9223372036854775807\nfs.inotify.max_user_watches 524288\n\
        fs.inotify.max_user_instances 128\nfs.inotify.max_queued_events 16384\nfs.aio-max-nr 65536\nkernel.pid_max 4194304\nvm.max_map_count 1048576\nvm.dirty_background_bytes 0\nvm.dirty_bytes 0\n\
        vm.dirty_expire_centisecs 3000\nvm.swappiness 60\nvm.vfs_cache_pressure 100\ntemporary files 0\n\
        rex.v1.mode=shared\nrex.v1.kept.system=0\nrex.v1.kept.user=0\nrex.v1.status.system=ok\nrex.v1.unit.system=/usr/bin/rex server run --log-format text\nrex.v1.loaded.system=/usr/bin/rex server run --log-format text\nrex.v1.stale.system=0\nrex.v1.running.system=b64:L3Vzci9iaW4vcmV4AHNlcnZlcgBydW4ALS1sb2ctZm9ybWF0AHRleHQA\n\
        default shell /bin/bash\nbrush 0\naccount alice /bin/bash\naccount bo /bin/bash\naccount carmen /usr/bin/zsh\n";
    let mut tuning = Tuning::read(comes);
    for stands in tuning.stands.iter_mut().filter(|s| Tune::ALL.iter().position(|t| *t == s.tune).is_some_and(|at| at < applied)) {
        stands.kept = true;
        for value in &mut stands.values {
            value.now = Some(value.wanted);
        }
    }
    if tuning.of(Tune::RexLimits).kept {
        tuning.rex_limits.kept_sys = true;
        tuning.rex_limits.kept_usr = true;
        tuning.rex_limits.running = Some(olo_rex_client::tune::RexLimitValues::wanted());
    }
    if tuning.of(Tune::Shell).kept {
        tuning.default_shell = Some(olo_rex_client::tune::BRUSH.to_owned());
        tuning.brush_there = true;
        // (Applied with two of its three accounts: what Put Back… gives back.)
        tuning.was_default = Some("/bin/bash".to_owned());
        tuning.was_accounts = vec![("alice".to_owned(), "/bin/bash".to_owned()), ("carmen".to_owned(), "/usr/bin/zsh".to_owned())];
        tuning.brush_users = vec!["alice".to_owned(), "carmen".to_owned()];
        tuning.accounts.retain(|(name, _)| name == "bo");
    }
    if tuning.of(Tune::TempFiles).kept {
        for (_, value) in &mut tuning.temp {
            *value = Some(olo_rex_client::tune::TEMP_DIR.to_owned());
        }
    }
    tuning
}

/// Shows the demo's stage `named` in the window (Rex's demo only).
pub fn show_demo(named: &str) -> bool {
    let Some(managed) = demo(named) else { return false };
    *live() = Some(Live { managed, turn: next_turn(), task: None, pending_update: false });
    true
}

/// The demo's stage `named` as the Tweaks of the host called `host`,
/// reached as `target` (Rex's demo only: the Host window's pictures).
pub fn show_demo_tweaks(named: &str, host: &str, target: SshTarget) -> bool {
    let Some(mut managed) = demo(named) else { return false };
    // (The demo's words name its own host: this one's, here.)
    let was = managed.name.clone();
    if let Stage::Failed { what, why, said, .. } = &mut managed.stage {
        *what = what.replace(&was, host);
        *why = why.replace(&was, host);
        *said = said.replace(&format!("{was}.example"), &target.host);
    }
    if let Some(tuning) = managed.server.as_mut().and_then(|s| s.tuning.as_mut())
        && tuning.rex_limits.scope == olo_rex_client::tune::RexScope::User
    {
        tuning.rex_limits.user = target.user.clone();
        tuning.rex_limits.file_usr = target.user.as_deref().map(|u| format!("/home/{u}/.config/systemd/user/rex.service.d/90-olo-limits.conf"));
    }
    managed.name = host.to_owned();
    managed.target = target;
    *live_of(Of::Tweaks) = Some(Live { managed, turn: next_turn(), task: None, pending_update: false });
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

    /// The demo's stages of the Host window's Tweaks tab.
    const TWEAK_STAGES: [&str; 23] = [
        "limits",
        "limits-some",
        "limits-restart",
        "limits-restart-user",
        "limits-put-back-pending",
        "limits-blocked",
        "limits-stale",
        "limits-ready",
        "confirm-limits",
        "confirm-limit",
        "confirm-writeback",
        "confirm-tmp",
        "confirm-rex",
        "confirm-rex-user",
        "confirm-shell",
        "confirm-shell-everyone",
        "confirm-put-back",
        "confirm-put-back-rex",
        "confirm-put-back-rex-user",
        "confirm-put-back-shell",
        "working-limits",
        "failed-look",
        "failed-limits",
    ];

    /// (The window's state is one for the program: these go one at a time.)
    static ONE: Mutex<()> = Mutex::new(());

    /// The Server window and the Host window's Tweaks tab each have a
    /// record of a host of their own: what is asked of one isn't asked
    /// of the other, a piece of work knows which is its own wherever it
    /// runs, and one gone leaves the other.
    #[test]
    fn the_tweaks_tab_has_a_record_of_its_own() {
        use olo_rex_client::tune::Tune;
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        assert!(show_demo("login"));
        assert!(show_demo_tweaks("limits", "dev-box", SshTarget::parse("ssh://you@dev-box.example").unwrap()));
        assert_eq!((current().unwrap().name.as_str(), for_tweaks(current).unwrap().name.as_str()), ("build", "dev-box"));
        // A tweak asked on the tab is the tab's question: the Server window shows its pages still.
        let kernel = Action::Tune(vec![Tune::Kernel]);
        for_tweaks(|| ask(kernel.clone()));
        assert_eq!((current().unwrap().stage, for_tweaks(current).unwrap().stage), (Stage::Shown, Stage::Confirm { action: kernel, choose: false }));
        // And the other way: the window's question, left on the tab, is the window's still.
        ask(Action::Update);
        for_tweaks(back);
        assert_eq!((current().unwrap().stage, for_tweaks(current).unwrap().stage), (Stage::Confirm { action: Action::Update, choose: false }, Stage::Shown));
        // (What is asked after `for_tweaks` is the Server window's again.)
        assert_eq!(which(), Of::Server);
        // A piece of work finds its record by its number: on a worker's thread nothing says whose it is.
        let (of_server, of_tweaks) = (live_of(Of::Server).as_ref().unwrap().turn, live_of(Of::Tweaks).as_ref().unwrap().turn);
        assert_eq!((of_turn(of_server), of_turn(of_tweaks)), (Of::Server, Of::Tweaks));
        set(of_tweaks, |m| m.stage = Stage::Looking);
        assert_eq!((current().unwrap().stage, for_tweaks(current).unwrap().stage), (Stage::Confirm { action: Action::Update, choose: false }, Stage::Looking));
        assert!(ours(of_server) && ours(of_tweaks));
        // One gone, the other stays; and work of the one gone changes nothing.
        *live_of(Of::Tweaks) = None;
        set(of_tweaks, |m| m.stage = Stage::Shown);
        assert!(ours(of_server) && !ours(of_tweaks) && for_tweaks(current).is_none());
        assert_eq!(current().unwrap().stage, Stage::Confirm { action: Action::Update, choose: false });
        *live_of(Of::Server) = None;
    }

    /// Brush's question: the accounts there are, chosen in it; what is
    /// shown and what is run follow the choice, which ends with the
    /// question.
    #[test]
    fn the_accounts_there_are_are_chosen_in_brushs_question() {
        use olo_rex_client::tune::Tune;
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        assert!(show_demo_tweaks("limits", "dev-box", SshTarget::parse("ssh://you@dev-box.example").unwrap()));
        let server = || for_tweaks(current).unwrap().server.unwrap();
        let chosen = || server().tuning.unwrap().everyone;
        // (Not outside a question.)
        for_tweaks(|| everyone(true));
        assert!(!chosen());
        let brush = Action::Tune(vec![Tune::Shell]);
        for_tweaks(|| ask(brush.clone()));
        assert!(!brush.shown(&server()).contains("carmen") && brush.command(&server()).unwrap().contains("if [ 0 = 1 ]"));
        for_tweaks(|| everyone(true));
        assert!(chosen() && brush.shown(&server()).contains("carmen") && brush.command(&server()).unwrap().contains("if [ 1 = 1 ]"));
        for_tweaks(back);
        assert!(!chosen(), "a choice made in a question is that question's");
        *live_of(Of::Tweaks) = None;
    }

    /// Who runs root's work, said over what is run: root signed in, or
    /// the account with sudo where it may use it.
    #[test]
    fn roots_work_is_said_as_whoever_runs_it() {
        let login = demo("confirm").unwrap();
        assert_eq!((login.root_by, login.root().as_str()), (Some(RootBy::Login), "root@build.example"));
        let sudo = demo("confirm-sudo").unwrap();
        assert_eq!((sudo.root_by, sudo.root().as_str()), (Some(RootBy::Sudo), "root, with sudo as alice"));
        assert_eq!(sudo.stage, login.stage);
    }

    #[test]
    fn what_is_asked_is_put_to_the_person_and_can_be_left() {
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        assert!(show_demo("login"));
        let stop = Action::Stop(Service::Login);
        ask(stop.clone());
        let first_confirm = current().unwrap().confirm_id;
        assert_eq!(current().unwrap().stage, Stage::Confirm { action: stop.clone(), choose: false });
        back();
        assert_eq!(current().unwrap().stage, Stage::Shown);
        ask(stop);
        assert_ne!(current().unwrap().confirm_id, first_confirm, "re-opening a confirmation gives it a new id so its scroll starts at the top");
        back();
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
        use olo_rex_client::tune::Tune;
        assert_eq!(still(Some(&running), &Action::Tune(vec![Tune::Kernel])), "Nothing else was changed.");
        assert_eq!(still(Some(&running), &Action::Tune(vec![Tune::Kernel, Tune::Memory])), "What could be applied was: the list shows how each stands now.");
        for stage in DEMO_STAGES.into_iter().chain(TWEAK_STAGES) {
            assert!(demo(stage).is_some(), "{stage}");
        }
        // (A tweak's stage names the host whose window shows it.)
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        assert!(show_demo_tweaks("failed-look", "dev-box", SshTarget::parse("ssh://you@dev-box.example").unwrap()));
        let Stage::Failed { what, why, said, .. } = for_tweaks(current).unwrap().stage else { panic!("it failed") };
        assert_eq!((what.as_str(), why.as_str()), ("Couldn’t read dev-box’s settings", "Nothing was changed on dev-box."));
        assert!(said.contains("dev-box.example") && !said.contains("build"), "{said}");
        // Finding 22: personal demo stages align header account, paragraph, caption actor and drop-in path to one account.
        for stage in ["confirm-rex-user", "confirm-put-back-rex-user"] {
            assert!(show_demo_tweaks(stage, "dev-box", SshTarget::parse("ssh://you@dev-box.example").unwrap()));
            let m = for_tweaks(current).unwrap();
            let s = m.server.as_ref().unwrap();
            let t = s.tuning.as_ref().unwrap();
            assert_eq!(t.rex_limits.user.as_deref(), Some("you"), "{stage}");
            assert_eq!(t.rex_limits.file_usr.as_deref(), Some("/home/you/.config/systemd/user/rex.service.d/90-olo-limits.conf"), "{stage}");
            let Stage::Confirm { action, .. } = &m.stage else { panic!("{stage} must be Confirm") };
            let what = action.what(s);
            assert!(!what.contains("alice") && !what.contains("you’s"), "{stage}: {what}");
            assert_eq!(action.shown(s).contains("alice"), false, "{stage}");
        }
        *live_of(Of::Tweaks) = None;
    }
}
