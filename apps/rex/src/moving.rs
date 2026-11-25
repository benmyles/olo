//! Moving an account from one host to another, from Rex (Ben, 2026-11-25: "i
//! want the ability to migrate a rex user from one host to another … and so
//! for the user the action session should then move to the new server in
//! the rex client and all be as seamless as possible").
//!
//! What a move is and how each step is done on the hosts is
//! `olo_rex_client::migrate`'s (and the helper's, `olo-rex-link move`); here
//! is where it stands for the Move window ([`Stage`]) and the work on the
//! hub's own threads, never the window's, in the order the real test drives
//! it (crates/rex/tests/moving.rs): both hosts looked at and the source's
//! sessions read; the person shown the plan, with `/var/tmp` brought by
//! default (Ben: "make it an option checked/on by default"); the first pass
//! while the source works (its home in parts, in parallel); the sessions read
//! again, then the source stopped (its files all left where they are: "we
//! don't want to delete the original copy"), the second pass; the
//! destination made right; the sessions made again there with each agent
//! resumed and its message typed at its prompt; the Token Pool's sign-ins
//! carried; Rex's windows on the destination; the move's key taken away.
//!
//! Each step is done once: a journal on disk says which are done (in Rex's
//! own state folder), so Try Again goes on from where a move stopped, even
//! after Rex itself was closed, and a source stopped by a move is never
//! connected to at Rex's start until the move is gone on or gone back from.
//! Go Back starts the source again as it was. One move at a time.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use olo_rex_client::{RexEndpoint, SshTarget, migrate, provision::RootBy};
use olo_sync::Mutex;
use serde::{Deserialize, Serialize};

use crate::hub::RexHub;

/// What the move is, as looked at (and kept in its journal).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Plan {
    pub from: migrate::Facts,
    pub to: migrate::Facts,
    pub captured: Vec<migrate::CapturedSession>,
    pub panes: Vec<migrate::Pane>,
    /// The Token Pool's program was on the source (its sign-ins carried to
    /// the destination).
    pub had_pool: bool,
}

/// What the person chose in Confirm.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Choices {
    /// The account's folders in `/var/tmp` brought too (on by default).
    pub var_tmp: bool,
    /// `~/.cache` brought too (off: another system's caches don't fit).
    pub cache: bool,
}

impl Default for Choices {
    fn default() -> Choices {
        Choices { var_tmp: true, cache: false }
    }
}

/// Where a move stands.
#[derive(Clone, Debug)]
pub enum Stage {
    /// Which host it goes to (the person's other hosts reached over SSH).
    Choosing { choices: Vec<(String, String)>, chosen: usize },
    /// Both hosts looked at, the source's sessions read: nothing changed.
    Looking { what: String, step: u8 },
    /// It can't be made: why, each a sentence; whether another host could
    /// be chosen.
    Refused { why: Vec<String>, more: bool },
    /// The plan, for the person to agree to.
    Confirm { plan: Box<Plan>, choices: Choices },
    /// The first pass, the source still working: the copy's own status.
    Copying { status: serde_json::Value },
    /// The source stopped and everything after: step `at` of [`SWITCHING`].
    Switching { at: usize, status: serde_json::Value },
    /// Moved: what came of it, and what is the person's to do.
    Done { said: Vec<String> },
    /// It didn't go: in a few words, and why; `stopped`: the source was
    /// stopped (Go Back starts it again).
    Failed { what: String, why: String, stopped: bool },
    /// Going back: the source started again.
    GoingBack,
    /// Gone back: the source as it was.
    WentBack { said: Vec<String> },
}

/// The steps after the first pass, as the window lists them.
pub const SWITCHING: usize = 7;

/// Step `at` of the switch, in words, its hosts named.
pub fn switching_step(at: usize, from: &str, to: &str) -> String {
    match at {
        0 => olo_kit::tr!("Stopping everything on {}", from).to_string(),
        1 => olo_kit::words::tr("Copying what changed since the first copy").to_string(),
        2 => olo_kit::tr!("Setting up your home on {}", to).to_string(),
        3 => olo_kit::tr!("Making your sessions again on {}", to).to_string(),
        4 => olo_kit::tr!("Starting the agents on {}", to).to_string(),
        5 => olo_kit::words::tr("Moving Rex over").to_string(),
        _ => olo_kit::words::tr("Tidying up").to_string(),
    }
}

/// A move under way, as the window shows it.
#[derive(Clone, Debug)]
pub struct Move {
    pub source: String,
    pub source_label: String,
    pub dest: Option<String>,
    pub dest_label: String,
    pub stage: Stage,
}

/// What a move has done, kept on disk as it goes: a step is never done
/// twice, a move left by Rex closing is gone on or gone back from.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Journal {
    pub id: String,
    pub source: String,
    pub source_label: String,
    pub dest: String,
    pub dest_label: String,
    pub plan: Plan,
    pub choices: Choices,
    pub public: String,
    pub way: Option<migrate::Way>,
    /// The steps done, by name.
    pub done: Vec<String>,
    /// The sessions made again: each source session's id, its new id.
    pub sessions: Vec<(String, String)>,
    /// Each source block's new block (and its session).
    pub blocks: HashMap<String, (String, String)>,
    /// The source's crontab as its stop saved it.
    pub crontab: Option<String>,
    /// What the steps after said, for Done.
    pub said: Vec<String>,
    pub finished: bool,
}

impl Journal {
    fn has(&self, step: &str) -> bool {
        self.done.iter().any(|d| d == step)
    }

    fn stopped(&self) -> bool {
        self.has("stopped")
    }
}

fn journal_dir() -> PathBuf {
    olo_xdg::state_dir().join("rex/moves")
}

fn save(j: &Journal) {
    let dir = journal_dir();
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join(format!("{}.json", j.id));
    let temp = dir.join(format!(".{}.json.new", j.id));
    if let Ok(text) = serde_json::to_vec(j)
        && std::fs::write(&temp, text).is_ok()
    {
        let _ = std::fs::rename(&temp, &file);
    }
}

fn forget_journal(id: &str) {
    let _ = std::fs::remove_file(journal_dir().join(format!("{id}.json")));
}

/// The moves Rex was in the middle of when it closed (none finished).
fn unfinished() -> Vec<Journal> {
    let Ok(rd) = std::fs::read_dir(journal_dir()) else { return Vec::new() };
    rd.filter_map(|e| serde_json::from_slice::<Journal>(&std::fs::read(e.ok()?.path()).ok()?).ok()).filter(|j| !j.finished).collect()
}

/// The hosts not to connect to as Rex starts: each a source a move stopped
/// and hasn't finished (connecting would start its Rex again, mid-move).
pub fn held_hosts() -> Vec<String> {
    unfinished().into_iter().filter(Journal::stopped).map(|j| j.source).collect()
}

struct Live {
    mv: Move,
    journal: Journal,
    turn: u64,
    stop: Arc<AtomicBool>,
    task: Option<tokio::task::AbortHandle>,
}

static LIVE: Mutex<Option<Live>> = Mutex::new(None);

fn live() -> olo_sync::MutexGuard<'static, Option<Live>> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The move under way, if there is one.
pub fn current() -> Option<Move> {
    live().as_ref().map(|l| l.mv.clone())
}

/// The stage, changed by the work of turn `turn` (nothing if another has
/// begun since); the Move window is told.
fn set(turn: u64, stage: Stage) {
    {
        let mut live = live();
        let Some(live) = live.as_mut().filter(|l| l.turn == turn) else { return };
        live.mv.stage = stage;
    }
    crate::ui::move_changed();
}

/// The journal changed by the work of turn `turn`, and saved.
fn journal(turn: u64, f: impl FnOnce(&mut Journal)) {
    let mut live = live();
    let Some(live) = live.as_mut().filter(|l| l.turn == turn) else { return };
    f(&mut live.journal);
    save(&live.journal);
}

fn journal_of(turn: u64) -> Option<Journal> {
    live().as_ref().filter(|l| l.turn == turn).map(|l| l.journal.clone())
}

/// Begins a piece of the move's work on the hub's threads; the stage is
/// `stage` meanwhile.
fn work<F, Fut>(hub: &Arc<RexHub>, stage: Stage, f: F)
where
    F: FnOnce(Arc<RexHub>, Move, u64, Arc<AtomicBool>) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let begun = {
        let mut live = live();
        let Some(live) = live.as_mut() else { return };
        if let Some(task) = live.task.take() {
            task.abort();
        }
        live.turn += 1;
        live.stop = Arc::new(AtomicBool::new(false));
        live.mv.stage = stage;
        (live.mv.clone(), live.turn, live.stop.clone())
    };
    let task = hub.spawn(f(hub.clone(), begun.0, begun.1, begun.2));
    if let Some(live) = live().as_mut().filter(|l| l.turn == begun.1) {
        live.task = Some(task);
    }
    crate::ui::move_changed();
}

/// A host of the hub's reached over SSH: its name here and where it is
/// (falls back to its saved address when disconnected or held after a stop).
fn ssh_host(hub: &RexHub, key: &str) -> Option<(String, SshTarget)> {
    let target = {
        let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        let host = st.hosts.get(key).filter(|h| !h.is_local)?;
        match &host.endpoint {
            Some(RexEndpoint::Ssh { target, .. }) => (host.label.clone(), target.clone()),
            _ => match crate::setup::address(&host.endpoint_raw) {
                crate::setup::Address::Ssh(target) => (host.label.clone(), target),
                _ => return None,
            },
        }
    };
    Some((target.0, hub.with_agent(target.1)))
}

fn control(hub: &RexHub, key: &str) -> Option<olo_rex_client::ControlClient> {
    hub.state.lock().unwrap_or_else(|e| e.into_inner()).hosts.get(key).and_then(|h| h.control.clone())
}

/// A move's id: when, and a few random letters.
fn new_id() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut bytes = [0u8; 3];
    // SAFETY: getrandom into a buffer of ours (zeros where it fails: the
    // time tells two moves apart).
    unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), 0) };
    format!("m{now}-{:02x}{:02x}{:02x}", bytes[0], bytes[1], bytes[2])
}

/// As Rex starts: a move it was in the middle of shown again, to go on with
/// or go back from (its source held meanwhile, [`held_hosts`]). Whether
/// there was one.
pub fn resume_unfinished() -> bool {
    let Some(j) = unfinished().into_iter().next() else { return false };
    if live().is_some() {
        return false;
    }
    let stopped = j.stopped();
    let mv = Move {
        source: j.source.clone(),
        source_label: j.source_label.clone(),
        dest: Some(j.dest.clone()),
        dest_label: j.dest_label.clone(),
        stage: Stage::Failed {
            what: if stopped { "Rex closed in the middle of the move".into() } else { "Rex closed before the move was done".into() },
            why: if stopped {
                olo_kit::tr!("{} is stopped, its files all still there. Try Again goes on from where it was; Go Back starts {} again as it was.", j.source_label, j.source_label).to_string()
            } else {
                olo_kit::tr!("Nothing on {} was stopped. Try Again goes on with the copy.", j.source_label).to_string()
            },
            stopped,
        },
    };
    *live() = Some(Live { mv, journal: j, turn: 0, stop: Arc::new(AtomicBool::new(false)), task: None });
    true
}

/// Begins moving the account on `source`: which host it goes to is asked
/// first (the person's other hosts reached over SSH and connected).
/// The hosts `source` could move to: connected over SSH, by name.
fn choices_for(hub: &Arc<RexHub>, source: &str) -> Vec<(String, String)> {
    let st = hub.state.lock().unwrap_or_else(|e| e.into_inner());
    let mut c: Vec<(String, String)> =
        st.hosts.values().filter(|h| h.key != source && !h.is_local && matches!(h.endpoint, Some(RexEndpoint::Ssh { .. })) && h.control.is_some()).map(|h| (h.key.clone(), h.label.clone())).collect();
    c.sort_by(|a, b| a.1.cmp(&b.1));
    c
}

pub fn begin(hub: &Arc<RexHub>, source: &str) {
    let Some((label, _)) = ssh_host(hub, source) else { return };
    let choices = choices_for(hub, source);
    let id = new_id();
    *live() = Some(Live {
        mv: Move { source: source.to_owned(), source_label: label.clone(), dest: None, dest_label: String::new(), stage: Stage::Choosing { choices, chosen: 0 } },
        journal: Journal { id, source: source.to_owned(), source_label: label, ..Journal::default() },
        turn: 0,
        stop: Arc::new(AtomicBool::new(false)),
        task: None,
    });
    crate::ui::move_changed();
}

/// The destination picked in the window's menu (by its place).
pub fn pick(chosen: usize) {
    if let Some(live) = live().as_mut()
        && let Stage::Choosing { chosen: at, .. } = &mut live.mv.stage
    {
        *at = chosen;
    }
    crate::ui::move_changed();
}

/// A choice of Confirm's changed.
pub fn choose_option(var_tmp: Option<bool>, cache: Option<bool>) {
    if let Some(live) = live().as_mut()
        && let Stage::Confirm { choices, .. } = &mut live.mv.stage
    {
        if let Some(v) = var_tmp {
            choices.var_tmp = v;
        }
        if let Some(c) = cache {
            choices.cache = c;
        }
    }
    crate::ui::move_changed();
}

/// The destination chosen (by its place among the choices): both hosts
/// are looked at.
pub fn choose(hub: &Arc<RexHub>, chosen: usize) {
    {
        let mut live = live();
        let Some(live) = live.as_mut() else { return };
        let Stage::Choosing { choices, .. } = &live.mv.stage else { return };
        let Some((key, label)) = choices.get(chosen).cloned() else { return };
        live.mv.dest = Some(key.clone());
        live.mv.dest_label = label.clone();
        (live.journal.dest, live.journal.dest_label) = (key, label);
    }
    look(hub);
}

/// Closes the window's move: before the source is stopped, nothing there
/// changed but the key, taken away again; a move that stopped it isn't
/// closed this way (it is gone on, or gone back from).
pub fn cancel(hub: &Arc<RexHub>) {
    let Some(mut was) = live().take() else { return };
    was.stop.store(true, Ordering::Relaxed);
    if let Some(task) = was.task.take() {
        task.abort();
    }
    let j = was.journal;
    if j.stopped() && !j.finished {
        *live() = Some(Live {
            mv: Move {
                // (The title says it, and what each button does under it.)
                stage: Stage::Failed { what: String::new(), why: String::new(), stopped: true },
                ..was.mv
            },
            journal: j,
            turn: was.turn + 1,
            stop: Arc::new(AtomicBool::new(false)),
            task: None,
        });
        crate::ui::move_changed();
        return;
    }
    if !j.finished {
        // (If the move failed after taking the pool off or disconnecting the
        // source, put both back before forgetting the journal.)
        if j.plan.had_pool && j.has("pool_off") {
            let _ = crate::pool::act(&j.source, "install");
        }
        if crate::hub::let_go().iter().any(|h| h == &j.source) {
            crate::hub::set_let_go(crate::hub::let_go().into_iter().filter(|h| *h != j.source).collect());
            hub.connect_host(&j.source);
        }
    }
    if !j.finished
        && j.has("allowed")
        && let (Some((_, src)), Some((_, dst))) = (ssh_host(hub, &j.source), ssh_host(hub, &j.dest))
    {
        let (mut s, mut d) = (hub.over_ssh(src, None, RootBy::Sudo), hub.over_ssh(dst, None, RootBy::Sudo));
        let (from, to, id, kept) = (j.plan.from.clone(), j.plan.to.clone(), j.id.clone(), j.has("kept"));
        hub.spawn(async move {
            // (The copy runs on the source by itself: ended first.)
            let _ = migrate::halt_pass(&mut s, crate::link::ID, &from, &id).await;
            if kept {
                let _ = migrate::restore_destination(&mut d, crate::link::ID, &from, &to, &id).await;
            }
            let _ = migrate::clean_up(&mut s, &mut d, crate::link::ID, &from, &to, &id).await;
        });
    }
    forget_journal(&j.id);
    crate::ui::move_changed();
}

/// Both hosts looked at and the source's sessions read; the plan put to
/// the person (or why it can't be made).
fn look(hub: &Arc<RexHub>) {
    work(hub, Stage::Looking { what: olo_kit::words::tr("Putting Olo’s helper on both hosts").to_string(), step: 1 }, |hub, mv, turn, _| async move {
        let failed = |what: &'static str, why: String| set(turn, Stage::Failed { what: olo_kit::words::tr(what).to_string(), why, stopped: false });
        let looking = |step: u8, what: String| set(turn, Stage::Looking { what, step });
        let Some(dest) = mv.dest.clone() else { return };
        let (Some((_, src)), Some((_, dst))) = (ssh_host(&hub, &mv.source), ssh_host(&hub, &dest)) else {
            return failed("A host isn’t reached", olo_kit::words::tr("Both hosts must be connected in Rex to move an account.").to_string());
        };
        let (mut s, mut d) = (hub.over_ssh(src.clone(), None, RootBy::Sudo), hub.over_ssh(dst.clone(), None, RootBy::Sudo));
        for (runner, target) in [(&s, &src), (&d, &dst)] {
            match olo_rex_client::helper::ensure(&runner.pool, target, None, crate::link::ID, &crate::link::BUILDS).await {
                Ok(olo_rex_client::helper::Stands::NoBuild(_)) => {
                    return failed("Olo’s helper has no build for a host", olo_kit::words::tr("A move needs Olo’s helper on both hosts, built for their machines.").to_string());
                }
                Ok(_) => {}
                Err(e) => return failed("Olo’s helper couldn’t be put on a host", e.why),
            }
        }
        looking(2, olo_kit::words::tr("Looking at both hosts").to_string());
        let look = |out: Result<olo_rex_client::provision::Ran, olo_rex_client::provision::Failed>| -> Result<migrate::Facts, String> { migrate::Facts::read(&out.map_err(|f| f.why)?.output) };
        use olo_rex_client::provision::Runner as _;
        let to = match look(d.run(false, &migrate::look_command(crate::link::ID), &mut |_| {}).await) {
            Ok(f) => f,
            Err(e) => return failed("The new host couldn’t be looked at", e),
        };
        let plain = match look(s.run(false, &migrate::look_command(crate::link::ID), &mut |_| {}).await) {
            Ok(f) => f,
            Err(e) => return failed("The old host couldn’t be looked at", e),
        };
        looking(3, olo_kit::tr!("Measuring {}’s home", mv.source_label).to_string());
        let excluded = migrate::excluded(&plain, &to, false);
        let from = match look(s.run(false, &migrate::look_sized_command(crate::link::ID, &excluded), &mut |_| {}).await) {
            Ok(f) => f,
            Err(e) => return failed("The old host couldn’t be measured", e),
        };
        // (With /var/tmp it may not fit where without it it does: then
        // it's left out to begin with, not refused.)
        let without = migrate::refusals(&from, &to, false, &mv.dest_label);
        if !without.is_empty() {
            return set(turn, Stage::Refused { why: without, more: choices_for(&hub, &mv.source).len() > 1 });
        }
        looking(4, olo_kit::tr!("Reading {}’s sessions", mv.source_label).to_string());
        let Some(ctrl) = control(&hub, &mv.source) else { return failed("The old host isn’t connected", String::new()) };
        let (captured, panes) = match read_panes(&ctrl, &mut s).await {
            Ok(p) => p,
            Err(e) => return failed("Its sessions couldn’t be read", e),
        };
        let had_pool = crate::pool::hosts().is_ok_and(|h| h.get(&mv.source).is_some_and(|p| p.wanted));
        let plan = Box::new(Plan { from, to, captured, panes, had_pool });
        let choices = Choices { var_tmp: migrate::refusals(&plan.from, &plan.to, true, &mv.dest_label).is_empty(), ..Choices::default() };
        journal(turn, |j| j.plan = (*plan).clone());
        set(turn, Stage::Confirm { plan, choices });
    });
}

/// The source's sessions read whole, each pane's agent and the session it
/// is in found (its screen told to the helper: two panes in one folder are
/// told apart by it).
async fn read_panes(ctrl: &olo_rex_client::ControlClient, s: &mut olo_rex_client::provision::OverSsh) -> Result<(Vec<migrate::CapturedSession>, Vec<migrate::Pane>), String> {
    use olo_rex_client::provision::Runner as _;
    let sessions = ctrl.session_list().await.map_err(|e| e.to_string())?.payload;
    let mut captured = Vec::new();
    for session in &sessions {
        captured.push(migrate::capture(ctrl, &session.session_id).await.map_err(|e| e.to_string())?);
    }
    let pids: Vec<u32> = captured.iter().flat_map(|c| c.panes.iter().filter_map(|p| p.pid)).collect();
    let screens: HashMap<u32, Vec<String>> = captured.iter().flat_map(|c| c.panes.iter()).filter_map(|p| Some((p.pid?, migrate::fingerprint(&p.screen)))).collect();
    let out = s.run(false, &migrate::look_pane_command(crate::link::ID, &pids, &screens), &mut |_| {}).await.map_err(|f| f.why)?;
    let panes = migrate::Pane::read_all(&out.output)?;
    Ok((captured, panes))
}

/// The person agreed (or Try Again): the move made, each step it hasn't
/// done yet.
pub fn go(hub: &Arc<RexHub>) {
    {
        let mut live = live();
        let Some(live) = live.as_mut() else { return };
        if let Stage::Confirm { choices, .. } = &live.mv.stage {
            live.journal.choices = *choices;
        }
        save(&live.journal);
    }
    work(hub, Stage::Copying { status: serde_json::Value::Null }, move |hub, mv, turn, stop| async move {
        if let Err((what, why)) = run(&hub, &mv, turn, &stop).await {
            let stopped = journal_of(turn).is_some_and(|j| j.stopped());
            set(turn, Stage::Failed { what, why, stopped });
        }
    });
}

/// Go Back: what the move stopped on the source started again as it was,
/// the destination as it was before the move, Rex on the source again.
pub fn go_back(hub: &Arc<RexHub>) {
    work(hub, Stage::GoingBack, |hub, _mv, turn, _| async move {
        let Some(j) = journal_of(turn) else { return };
        let (Some((_, src)), Some((_, dst))) = (ssh_host(&hub, &j.source), ssh_host(&hub, &j.dest)) else {
            return set(
                turn,
                Stage::Failed { what: olo_kit::words::tr("A host isn’t reached").to_string(), why: olo_kit::words::tr("Both hosts must be in Rex to go back.").to_string(), stopped: j.stopped() },
            );
        };
        let (mut s, mut d) = (hub.over_ssh(src, None, RootBy::Sudo), hub.over_ssh(dst, None, RootBy::Sudo));
        let (from, to, id) = (&j.plan.from, &j.plan.to, j.id.as_str());
        let mut said = Vec::new();
        if j.stopped()
            && let Err(e) = migrate::restart_source(&mut s, from, id).await
        {
            said.push(olo_kit::tr!("{} couldn’t all be started again: {}", j.source_label, e).to_string());
        }
        if j.has("kept")
            && let Err(e) = migrate::restore_destination(&mut d, crate::link::ID, from, to, id).await
        {
            said.push(olo_kit::tr!("{} couldn’t all be put back: {}", j.dest_label, e).to_string());
        }
        let _ = migrate::clean_up(&mut s, &mut d, crate::link::ID, from, to, id).await;
        if j.plan.had_pool && j.has("pool_off") {
            let _ = crate::pool::act(&j.source, "install");
        }
        crate::hub::set_let_go(crate::hub::let_go().into_iter().filter(|h| *h != j.source).collect());
        hub.connect_host(&j.source);
        forget_journal(id);
        said.insert(0, olo_kit::tr!("Your services, timers, crontab and the Rex server on {} are running again.", j.source_label).to_string());
        if j.has("kept") {
            said.insert(1, olo_kit::tr!("Your own files on {} are back as they were; what was copied stays there.", j.dest_label).to_string());
        }
        set(turn, Stage::WentBack { said });
    });
}

/// The move, each step it hasn't done yet (the journal says), in order.
async fn run(hub: &Arc<RexHub>, mv: &Move, turn: u64, stop: &AtomicBool) -> Result<(), (String, String)> {
    let fail = |what: &'static str, why: String| (olo_kit::words::tr(what).to_string(), why);
    let j0 = journal_of(turn).ok_or_else(|| fail("The move was given up", String::new()))?;
    let (Some((_, src)), Some((_, dst))) = (ssh_host(hub, &j0.source), ssh_host(hub, &j0.dest)) else {
        return Err(fail("A host isn’t reached", olo_kit::words::tr("Both hosts must be in Rex to move an account.").to_string()));
    };
    let (mut s, mut d) = (hub.over_ssh(src.clone(), None, RootBy::Sudo), hub.over_ssh(dst.clone(), None, RootBy::Sudo));
    let id = j0.id.clone();
    let done = |step: &'static str| journal(turn, |j| j.done.push(step.to_owned()));
    let has = |step: &str| journal_of(turn).is_some_and(|j| j.has(step));
    let mut plan = j0.plan.clone();
    let (from, to) = (plan.from.clone(), plan.to.clone());
    let excluded = migrate::excluded(&from, &to, j0.choices.cache);
    let var_tmp = j0.choices.var_tmp && from.var_tmp_count() > 0;

    // Make sure this build's helper is on both hosts (e.g. Try Again after
    // an Olo upgrade) and `rsync` is installed on both hosts.
    for (runner, target) in [(&s, &src), (&d, &dst)] {
        match olo_rex_client::helper::ensure(&runner.pool, target, None, crate::link::ID, &crate::link::BUILDS).await {
            Ok(olo_rex_client::helper::Stands::NoBuild(_)) => {
                return Err(fail("Olo’s helper has no build for a host", olo_kit::words::tr("A move needs Olo’s helper on both hosts, built for their machines.").to_string()));
            }
            Ok(_) => {}
            Err(e) => return Err(fail("Olo’s helper couldn’t be put on a host", e.why)),
        }
    }
    if !has("rsync") {
        migrate::ensure_rsync(&mut s, &from).await.map_err(|e| fail("rsync isn’t ready on the old host", e))?;
        migrate::ensure_rsync(&mut d, &to).await.map_err(|e| fail("rsync isn’t ready on the new host", e))?;
        done("rsync");
    }

    // The move's key, let in there; a way from the source to the
    // destination (the name and port Rex reaches it by, then its own sshd's
    // port, where Rex reaches it through another); the destination's own
    // files kept aside.
    if !has("allowed") {
        let public = migrate::make_key(&mut s, crate::link::ID, &from, &id).await.map_err(|e| fail("The move’s key couldn’t be made", e))?;
        migrate::allow(&mut d, crate::link::ID, &id, &public, var_tmp).await.map_err(|e| fail("The move’s key couldn’t be let in", e))?;
        journal(turn, |j| j.public = public);
        done("allowed");
    }
    if !has("reached") {
        let verified = d.pool.verified(&dst).ok_or_else(|| fail("The new host’s key isn’t known", String::new()))?;
        let user = dst.user.clone().unwrap_or_else(|| to.user.clone());
        let mut ways = vec![migrate::Way { user: user.clone(), host: verified.hostname.clone(), port: verified.port, host_key: verified.key.clone() }];
        if let Some(port) = to.ssh_port.filter(|p| *p != verified.port) {
            ways.push(migrate::Way { user, host: verified.hostname.clone(), port, host_key: verified.key.clone() });
        }
        let way = migrate::reach(&mut s, crate::link::ID, &from, &to, &id, &ways).await.map_err(|e| fail("The old host can’t reach the new one", e))?;
        journal(turn, |j| j.way = Some(way));
        done("reached");
    }
    if !has("kept") {
        migrate::keep_destination(&mut d, crate::link::ID, &id, &excluded).await.map_err(|e| fail("The new host’s own files couldn’t be kept aside", e))?;
        done("kept");
    }
    let way = journal_of(turn).and_then(|j| j.way).ok_or_else(|| fail("The old host can’t reach the new one", String::new()))?;

    // The first pass, the source working meanwhile.
    if !has("pass1") {
        migrate::start_pass(&mut s, crate::link::ID, &from, &to, &id, &way, 1, &excluded, var_tmp).await.map_err(|e| fail("The copy didn’t start", e))?;
        let first = follow(&mut s, &from, &id, turn, stop, 1).await?;
        if first["phase"] != "done" {
            return Err(fail("The copy didn’t finish", first["error"].as_str().unwrap_or_default().to_owned()));
        }
        done("pass1");
    }

    // From here the source is stopped: the rest goes through.
    let step = |at: usize| set(turn, Stage::Switching { at, status: serde_json::Value::Null });
    step(0);
    if !has("stopped") {
        // (Read again: the source went on working through the first pass.)
        if let Some(ctrl) = control(hub, &j0.source)
            && let Ok((captured, panes)) = read_panes(&ctrl, &mut s).await
        {
            (plan.captured, plan.panes) = (captured, panes);
            let (c, p) = (plan.captured.clone(), plan.panes.clone());
            journal(turn, |j| (j.plan.captured, j.plan.panes) = (c, p));
        }
        if plan.had_pool && !has("pool_off") {
            crate::pool::act(&j0.source, &format!("move-to-{}", j0.dest)).map_err(|e| fail("The Token Pool’s sign-ins couldn’t be taken back from the old host", e))?;
            wait_pool(&j0.source).await;
            done("pool_off");
        }
        // (Held: never connected to again while the move goes on, nor at
        // Rex's start. Wait for the host's SSH sessions to finish closing
        // before running stop_source on the same pool.)
        hub.disconnect_host_and_wait(&j0.source).await;
        let agents: Vec<u32> = plan.panes.iter().filter_map(|p| p.agent.as_ref().map(|a| a.pid)).collect();
        match migrate::stop_source(&mut s, crate::link::ID, &from, &id, &agents).await {
            Ok(left) if left.is_empty() => {}
            Ok(left) => return Err(fail("Something on the old host wouldn’t stop", left.iter().filter_map(|l| l["name"].as_str()).collect::<Vec<_>>().join(", "))),
            Err(e) => return Err(fail("The old host couldn’t be stopped", e)),
        }
        let crontab = migrate::saved_crontab(&mut s, &from, &id).await.ok().flatten();
        journal(turn, |j| j.crontab = crontab);
        done("stopped");
    }
    step(1);
    if !has("pass2") {
        let public = journal_of(turn).map(|j| j.public).unwrap_or_default();
        let _ = migrate::allow(&mut d, crate::link::ID, &id, &public, var_tmp).await;
        migrate::start_pass(&mut s, crate::link::ID, &from, &to, &id, &way, 2, &excluded, var_tmp).await.map_err(|e| fail("The last copy didn’t start", e))?;
        let second = follow(&mut s, &from, &id, turn, stop, 2).await?;
        if second["phase"] != "done" {
            return Err(fail("The last copy didn’t finish", second["error"].as_str().unwrap_or_default().to_owned()));
        }
        if let Some(gone) = second["report"]["also_failed"].as_array().filter(|g| !g.is_empty()) {
            let paths = gone.iter().filter_map(|g| g["path"].as_str()).collect::<Vec<_>>().join(", ");
            journal(turn, |j| j.said.push(olo_kit::tr!("Not copied from /var/tmp: {}.", paths).to_string()));
        }
        done("pass2");
    }
    if !has("quiet") {
        let _ = migrate::linger_off(&mut s, &from).await;
        let _ = migrate::mask_quadlets(&mut s, &from).await;
        done("quiet");
    }

    // The destination made right.
    step(2);
    if !has("fixed") {
        let mut said: Vec<String> = Vec::new();
        if let Ok(n) = migrate::merge_keys(&mut s, &mut d, crate::link::ID, &id).await
            && n > 0
        {
            said.push(olo_kit::tr!("{} SSH keys for {} now sign in to {} too.", n, mv.source_label, mv.dest_label).to_string());
        }
        let fixed = migrate::fix_destination(&mut d, crate::link::ID, &from, &to, &id, var_tmp).await.unwrap_or_default();
        if let Some(mentions) = fixed["mentions"].as_array().filter(|m| !m.is_empty()) {
            said.push(olo_kit::tr!("These still name the old home, left as they are: {}.", mentions.iter().filter_map(|m| m.as_str()).take(8).collect::<Vec<_>>().join(", ")).to_string());
        }
        if let Ok(Some(e)) = migrate::relabel(&mut d, &to).await {
            said.push(olo_kit::tr!("Some files couldn’t be labelled for SELinux: {}", e).to_string());
        }
        if let Ok(Some(note)) = migrate::login_shell(&mut d, &from, &to).await {
            said.push(note);
        }
        let linked = migrate::compat_link(&mut d, &from, &to).await.unwrap_or(false);
        let crontab = journal_of(turn).and_then(|j| j.crontab);
        if let Ok(failed) = migrate::units_and_crontab(&mut d, &from, &to, &id, crontab.as_deref()).await
            && !failed.is_empty()
        {
            said.push(olo_kit::tr!("These didn’t start on {}: {}.", mv.dest_label, failed.join(", ")).to_string());
        }
        let _ = migrate::path_on(&mut d).await;
        let ran: Vec<String> = plan.panes.iter().filter_map(|p| p.agent.as_ref().map(|a| a.agent.clone())).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        if let Ok(broken) = migrate::agents_broken(&mut d, &ran).await
            && !broken.is_empty()
        {
            said.push(olo_kit::tr!("These don’t start on {} (built for another machine?): {}. Set Up This Host’s Account installs them again.", mv.dest_label, broken.join(", ")).to_string());
        }
        journal(turn, |j| {
            j.said.extend(said);
            if linked {
                j.done.push("linked".into());
            }
        });
        done("fixed");
    }
    let linked = has("linked");

    // The sessions made again, each pane's command typed, each agent's
    // message when it is at its prompt.
    step(3);
    let dctrl = control(hub, &j0.dest).ok_or_else(|| fail("The new host isn’t connected", String::new()))?;
    let path = |p: &str| -> String {
        match p.strip_prefix(&from.home) {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{}{rest}", to.home),
            _ => p.to_owned(),
        }
    };
    let (mut runs, mut run_it) = (HashMap::new(), HashMap::new());
    for c in &plan.captured {
        let (r, _, it) = migrate::pane_runs(c, &plan.panes, &path);
        runs.extend(r);
        run_it.extend(it);
    }
    if !has("rebuilt") {
        let cwds: Vec<String> = runs.values().map(|r: &migrate::PaneRun| r.cwd.clone()).filter(|c| !c.is_empty()).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        let there = migrate::dirs_there(&mut d, &cwds).await.unwrap_or_default();
        for c in &plan.captured {
            if journal_of(turn).is_some_and(|j| j.sessions.iter().any(|(old, _)| *old == c.session_id)) {
                continue;
            }
            let (sid, made) = migrate::rebuild(&dctrl, c, &runs, &there).await.map_err(|e| fail("A session couldn’t be made again", e.to_string()))?;
            journal(turn, |j| {
                j.sessions.push((c.session_id.clone(), sid.clone()));
                j.blocks.extend(made.into_iter().map(|(old, new)| (old, (sid.clone(), new))));
            });
        }
        done("rebuilt");
    }
    let blocks = journal_of(turn).map(|j| j.blocks).unwrap_or_default();
    if !has("typed") {
        for (old, (sid, new)) in &blocks {
            let Some(command) = runs.get(old).and_then(|r: &migrate::PaneRun| r.command.clone()) else { continue };
            settled(&dctrl, sid, new, 15).await;
            let text = migrate::command_text(&command);
            // (What isn't run again is left typed as a comment: an Enter
            // never runs it by mistake.)
            let typed = if run_it.get(old).copied().unwrap_or(false) { format!("{text}\r") } else { format!("# {text}") };
            let _ = dctrl.terminal_write(sid, new, &typed).await;
        }
        done("typed");
    }
    step(4);
    if !has("told") {
        let when = olo_kit::tr!("at {}", time_now()).to_string();
        let told = migrate::Told {
            from_label: &mv.source_label,
            to_label: &mv.dest_label,
            from: from.clone(),
            to: to.clone(),
            when: &when,
            linked,
            var_tmp,
            ports: &from.listening,
            ..migrate::Told::default()
        };
        let mut not_typed = Vec::new();
        for c in &plan.captured {
            for cp in &c.panes {
                let Some(agent) = cp.pid.and_then(|pid| plan.panes.iter().find(|p| p.pid == pid)).and_then(|p| p.agent.as_ref()) else { continue };
                let Some((sid, new)) = blocks.get(&cp.block_id) else { continue };
                let gone = runs.get(&cp.block_id).map(|r| r.cwd.clone()).filter(|c| !c.is_empty() && !c.starts_with(&to.home) && !c.starts_with("/var/tmp"));
                let told = migrate::Told { folder_gone: gone.as_deref(), ..told.clone() };
                // (Typed only once it is idle at its prompt: a question
                // shown first, a trust dialog, an update, would take the
                // keys.)
                if until(&dctrl, sid, new, 90, |screen| at_prompt(&agent.agent, screen)).await {
                    let _ = dctrl.terminal_write(sid, new, &migrate::message(&told)).await;
                } else {
                    not_typed.push(if cp.label.is_empty() { agent.agent.clone() } else { cp.label.clone() });
                }
            }
        }
        if !not_typed.is_empty() {
            journal(turn, |j| j.said.push(olo_kit::tr!("Not at their prompts in time, so the move’s message isn’t typed in: {}.", not_typed.join(", ")).to_string()));
        }
        done("told");
    }

    // The Token Pool's sign-ins on the destination; Rex's windows there; the
    // source remembered as moved.
    step(5);
    if plan.had_pool && !has("pool_on") {
        // (Done says where the sign-ins are, as Confirm said they'd go.)
        let said = if crate::pool::act(&j0.dest, "install").is_err() {
            olo_kit::tr!("The Token Pool couldn’t be put on {}: put it there from its Hosts window.", mv.dest_label).to_string()
        } else {
            olo_kit::tr!("Your sign-ins in the Token Pool are on {} now.", mv.dest_label).to_string()
        };
        journal(turn, |j| j.said.push(said));
        done("pool_on");
    }
    let sessions: HashMap<String, String> = journal_of(turn).map(|j| j.sessions.into_iter().collect()).unwrap_or_default();
    crate::ui::host_moved(j0.source.clone(), j0.dest.clone(), sessions);
    let (source_id, dest_id) = (j0.source.clone(), j0.dest.clone());
    olo_kit::settings::update(move |s| {
        if let Some(h) = s.rex.hosts.iter_mut().find(|h| h.id == source_id) {
            h.connect_at_start = false;
            h.moved_to = Some(dest_id);
        }
    });

    // The key taken away; the source's crontab and a record of what was
    // stopped kept there.
    step(6);
    if let Err(e) = migrate::clean_up(&mut s, &mut d, crate::link::ID, &from, &to, &id).await {
        journal(turn, |j| j.said.push(olo_kit::tr!("Not everything of the move’s was tidied away: {}", e).to_string()));
    }
    journal(turn, |j| j.finished = true);
    forget_journal(&id);
    let j = journal_of(turn).unwrap_or_default();
    let agents = plan.panes.iter().filter(|p| p.agent.is_some()).count();
    let panes = olo_kit::words::count(j.blocks.len() as u64, "1 pane", "{n} panes");
    let mut said = vec![if j.sessions.len() == 1 {
        olo_kit::tr!("1 session with {} is back on {}.", panes, mv.dest_label).to_string()
    } else {
        olo_kit::tr!("{} sessions with {} are back on {}.", j.sessions.len(), panes, mv.dest_label).to_string()
    }];
    if agents > 0 {
        said.push(olo_kit::words::count(agents as u64, "1 agent carried on where it left off.", "{n} agents carried on where they left off."));
    }
    said.push(olo_kit::tr!("On {}, everything is stopped; its files are still there.", mv.source_label).to_string());
    said.extend(j.said);
    set(turn, Stage::Done { said });
    Ok(())
}

/// The Token Pool's work on a host done (a minute at most).
async fn wait_pool(id: &str) {
    let until = Instant::now() + Duration::from_secs(60);
    while Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if crate::pool::hosts().is_ok_and(|h| h.get(id).is_none_or(|p| p.doing.is_none())) {
            return;
        }
    }
}

fn time_now() -> String {
    // SAFETY: localtime_r into a struct of ours.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    }
}

/// A pass followed till it ends, the window shown its status each half
/// second (in Copying for the first, Switching's second step for the
/// second). The copy goes on without the app: a link that drops a while is
/// waited for.
async fn follow(s: &mut olo_rex_client::provision::OverSsh, from: &migrate::Facts, id: &str, turn: u64, stop: &AtomicBool, pass: u32) -> Result<serde_json::Value, (String, String)> {
    let mut missed = 0;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err((olo_kit::words::tr("The move was given up").to_string(), String::new()));
        }
        match migrate::pass_status(s, crate::link::ID, from, id).await {
            Ok(status) => {
                missed = 0;
                if status["pass"] == pass && status["running"] == false && matches!(status["phase"].as_str(), Some("done" | "failed")) {
                    return Ok(status);
                }
                set(turn, if pass == 1 { Stage::Copying { status } } else { Stage::Switching { at: 1, status } });
            }
            Err(e) => {
                missed += 1;
                if missed > 240 {
                    return Err((olo_kit::words::tr("The old host stopped answering").to_string(), e));
                }
            }
        }
        // (Once a second while the source works on, twice while it is
        // stopped: each is a run of the helper there, which reads a small
        // file; the switch's end comes half a second sooner.)
        tokio::time::sleep(Duration::from_millis(if pass == 1 { 1000 } else { 500 })).await;
    }
}

/// A pane's screen read.
async fn screen(ctrl: &olo_rex_client::ControlClient, sid: &str, bid: &str) -> String {
    ctrl.terminal_format(sid, bid, "text").await.map(|r| r.payload).unwrap_or_default()
}

/// A pane's shell ready to be typed into: its screen drawn and still a
/// moment (its prompt shown), or `secs` gone by.
async fn settled(ctrl: &olo_rex_client::ControlClient, sid: &str, bid: &str, secs: u64) {
    let until = Instant::now() + Duration::from_secs(secs);
    let mut last = String::new();
    let mut same = 0;
    while Instant::now() < until {
        let now = screen(ctrl, sid, bid).await;
        if !now.trim().is_empty() && now == last {
            same += 1;
            if same >= 2 {
                return;
            }
        } else {
            same = 0;
        }
        last = now;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Whether a pane's screen comes to say `done` within `secs`.
async fn until(ctrl: &olo_rex_client::ControlClient, sid: &str, bid: &str, secs: u64, done: impl Fn(&str) -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs(secs);
    loop {
        if done(&screen(ctrl, sid, bid).await) {
            return true;
        }
        if Instant::now() > until {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Whether the agent is idle at its prompt on `screen` (its own words:
/// `olo_agent_screen`).
fn at_prompt(agent: &str, screen: &str) -> bool {
    use olo_agent_screen::{Agent, State, at_prompt, classify};
    let which = match agent {
        "claude" => Agent::Claude,
        "codex" => Agent::Codex,
        _ => Agent::Antigravity,
    };
    classify(which, screen) == State::Idle && at_prompt(which, screen)
}

/// A made-up move at stage `named`, for the Move window's pictures and its
/// lint (`OLO_REX_DEMO_MOVE`, the demo's): nothing is connected to, and
/// whatever is pressed changes nothing anywhere.
pub fn show_demo(named: &str) -> bool {
    let Some(mv) = demo(named) else { return false };
    let journal = Journal { id: "demo".into(), source: mv.source.clone(), source_label: mv.source_label.clone(), dest: "big-box".into(), dest_label: mv.dest_label.clone(), ..Journal::default() };
    *live() = Some(Live { mv, journal, turn: 0, stop: Arc::new(AtomicBool::new(false)), task: None });
    true
}

fn demo(named: &str) -> Option<Move> {
    let from = migrate::Facts {
        system: "ubuntu".into(),
        system_name: "Ubuntu 26.04 LTS".into(),
        machine: "x86_64".into(),
        user: "dev".into(),
        home: "/home/dev".into(),
        home_bytes: 720_000_000_000,
        home_files: 3_600_000,
        big: vec![
            migrate::Big { path: "worktrees/app".into(), bytes: 541_000_000_000 },
            migrate::Big { path: "src/olo".into(), bytes: 87_000_000_000 },
            migrate::Big { path: "src/site".into(), bytes: 47_000_000_000 },
        ],
        var_tmp: vec![
            migrate::VarTmpFolder { path: "/var/tmp/builds".into(), bytes: 120_000_000_000, files: 900_000, more: false },
            migrate::VarTmpFolder { path: "/var/tmp/notes".into(), bytes: 20_000_000, files: 300, more: false },
            migrate::VarTmpFolder { path: "/var/tmp/rigs".into(), bytes: 15_000_000_000, files: 40_000, more: false },
        ],
        now: 1,
        ..migrate::Facts::default()
    };
    let to = migrate::Facts {
        system: "fedora".into(),
        system_name: "Fedora Linux 44 (Server Edition)".into(),
        machine: "x86_64".into(),
        user: "dev".into(),
        home: "/home/dev".into(),
        free: 3_500_000_000_000,
        free_inodes: 200_000_000,
        home_dev: Some(1),
        var_tmp_dev: Some(1),
        now: 1,
        ..migrate::Facts::default()
    };
    let pane = |id: &str, label: &str, pid: u32| migrate::CapturedPane { block_id: id.into(), label: label.into(), pid: Some(pid), ..migrate::CapturedPane::default() };
    let agent = |pid: u32, name: &str, how: &str| migrate::Pane {
        pid,
        agent: Some(migrate::PaneAgent { agent: name.into(), pid: pid + 1, session: Some("s".into()), how: how.into(), ..migrate::PaneAgent::default() }),
        ..migrate::Pane::default()
    };
    let captured = vec![migrate::CapturedSession {
        session_id: "work".into(),
        label: "work".into(),
        panes: vec![pane("b1", "shell", 10), pane("b2", "claude", 20), pane("b3", "codex", 30), pane("b4", "agy", 40), pane("b5", "todo", 50)],
        ..migrate::CapturedSession::default()
    }];
    let program = |pid: u32, argv: &[&str]| migrate::Pane {
        pid,
        tree: vec![migrate::Program { pid, name: argv[0].into(), argv: argv.iter().map(|a| (*a).to_owned()).collect(), ..migrate::Program::default() }],
        agent: None,
    };
    let panes = vec![program(10, &["npm", "run", "dev"]), agent(20, "claude", "record"), agent(30, "codex", "daemon"), agent(40, "agy", "open"), program(50, &["tail", "-f", "/home/dev/todo.txt"])];
    let plan = Box::new(Plan { from, to, captured, panes, had_pool: true });
    let (source, source_label, dest_label) = ("build".to_owned(), "build".to_owned(), "big-box".to_owned());
    let stage = match named {
        "choosing" => Stage::Choosing { choices: vec![("big-box".into(), "big-box".into()), ("nas".into(), "nas".into())], chosen: 0 },
        "choosing-none" => Stage::Choosing { choices: Vec::new(), chosen: 0 },
        "looking" => Stage::Looking { what: "Measuring build’s home".into(), step: 3 },
        "confirm" => Stage::Confirm { plan, choices: Choices::default() },
        "confirm-tight" => {
            let mut plan = plan;
            plan.to.free = 800_000_000_000;
            Stage::Confirm { plan, choices: Choices::default() }
        }
        "refused" => Stage::Refused { why: vec!["Needs 793 GB with room to spare; only 400 GB is free on big-box. Free some space there, or choose another host.".into()], more: true },
        "copying-start" => Stage::Copying { status: serde_json::json!({"files": 0, "total_files": 0, "bytes": 0, "total_bytes": 0, "streams": 4}) },
        "copying" => Stage::Copying {
            status: serde_json::json!({"files": 1_250_000, "total_files": 4_540_300, "bytes": 310_000_000_000u64, "total_bytes": 855_020_000_000u64, "streams": 12, "rate": 610_000_000, "parts": 37}),
        },
        "switching" => Stage::Switching { at: 1, status: serde_json::json!({"files": 412, "total_files": 1310}) },
        "switching-later" => Stage::Switching { at: 4, status: serde_json::Value::Null },
        "done" => Stage::Done {
            said: vec![
                "1 session with 5 panes is back on big-box.".into(),
                "3 agents carried on where they left off.".into(),
                "On build, everything is stopped; its files are still there.".into(),
                "2 SSH keys for build now sign in to big-box too.".into(),
                "Your sign-ins in the Token Pool are on big-box now.".into(),
            ],
        },
        "failed" => Stage::Failed { what: "The copy didn’t finish".into(), why: "the copy ended early (unexpected end of file)".into(), stopped: false },
        "failed-stopped" => Stage::Failed { what: "The last copy didn’t finish".into(), why: "the copy ended early (unexpected end of file)".into(), stopped: true },
        "going-back" => Stage::GoingBack,
        "went-back" => Stage::WentBack {
            said: vec!["Your services, timers, crontab and the Rex server on build are running again.".into(), "Your own files on big-box are back as they were; what was copied stays there.".into()],
        },
        _ => return None,
    };
    Some(Move { source, source_label, dest: Some("big-box".into()), dest_label, stage })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every stage of the Move window can be pictured (the demo's).
    #[test]
    fn every_stage_can_be_pictured() {
        for named in DEMO_STAGES {
            assert!(demo(named).is_some(), "no demo of {named}");
        }
        assert!(demo("no such").is_none());
    }

    /// A journal read back is the one saved (Try Again after Rex closed
    /// goes on from it).
    #[test]
    fn a_journal_reads_back() {
        let j = Journal { id: "m1".into(), source: "a".into(), dest: "b".into(), done: vec!["allowed".into(), "stopped".into()], ..Journal::default() };
        let back: Journal = serde_json::from_slice(&serde_json::to_vec(&j).unwrap()).unwrap();
        assert!(back.has("allowed") && back.stopped() && !back.has("pass2"));
    }
}

/// The demo's stages, each a picture of the Move window.
#[cfg(test)]
pub const DEMO_STAGES: [&str; 15] = [
    "choosing",
    "choosing-none",
    "looking",
    "confirm",
    "confirm-tight",
    "refused",
    "copying-start",
    "copying",
    "switching",
    "switching-later",
    "done",
    "failed",
    "failed-stopped",
    "going-back",
    "went-back",
];
