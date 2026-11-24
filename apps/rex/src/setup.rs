//! Setting a host up to run Rex, from the Add Host window: the host is
//! asked how it stands, the person is shown what it needs (each command
//! before it is run) and agrees, the steps are carried out over SSH (as
//! the account, and as root where a step is root's) with what they write
//! shown as it comes, and the host, looked at again and found as it
//! should be, is added.
//!
//! What a host needs and how it is done is `olo_rex_client::provision`'s;
//! here is where it stands for the window ([`Stage`]) and the work on the
//! hub's own threads, never the window's. One host at a time is set up
//! (there is one Add Host window).
//!
//! **Root is asked for only to set the server up**: the connection that
//! is kept is the account's. Where the account may use sudo without a
//! password (asked once, `provision::can_sudo`, as the host is looked
//! at), root's steps run with sudo over the account's own connection
//! ([`RootBy::Sudo`]: a cloud machine's `fedora@` or `ubuntu@`, where
//! root can't sign in; Ben, 2026-11-24). Else root is signed in with
//! keys alone, the agent's (Olo Passwords', unless another agent is
//! set); if root there takes none of them, the person chooses which key
//! to offer ([`Stage::RootKey`]: a server lets a client try only a few
//! keys, so with many in the agent the right one may never be reached),
//! told that a root login or an account that can use sudo is needed,
//! and no password is ever asked for. Root's connection is dropped when
//! the work ends, however it ends.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use olo_rex_client::{
    AgentKey, ServerMode, SshTarget,
    provision::{self, Failed, Outcome, RootBy, Step},
};
use olo_sync::Mutex;

use crate::hub::RexHub;

/// Where a host's setting up stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    /// Asking the host how it stands.
    Looking,
    /// What it needs, for the person to agree to.
    Confirm { steps: Vec<Step>, ends_sessions: bool },
    /// Signing in as root, before anything is changed.
    Root { steps: Vec<Step> },
    /// Root there took none of the keys offered: which to offer.
    RootKey { steps: Vec<Step>, keys: Vec<AgentKey>, chosen: usize },
    /// Step `at` of them is running; `said`, the last line it wrote.
    Working { steps: Vec<Step>, at: usize, said: String, stopping: bool },
    /// It didn't go: `what`, in a few words (the window's heading);
    /// `why`, what Rex has to say of it (how things stand there, what to
    /// do); `said`, what the host said, in its own words (or nothing).
    /// `icon`: the alert's (something stopped; or only: look at this).
    Failed { what: String, why: String, said: String, icon: &'static str },
    /// The host is as it should be, and added.
    Done,
}

/// A host being set up.
#[derive(Clone, Debug, PartialEq)]
pub struct Setup {
    /// The name it will go by.
    pub name: String,
    pub target: SshTarget,
    pub mode: ServerMode,
    pub stage: Stage,
    /// The key root is offered (its fingerprint); none: the agent's, in turn.
    pub root_key: Option<String>,
    /// How root's steps reach root: with sudo where the account may use
    /// it without a password (asked as the host is looked at), else a
    /// root login.
    pub root_by: RootBy,
}

impl Setup {
    /// The host, as the person typed it (no account, no port).
    pub fn host(&self) -> &str {
        &self.target.host
    }

    /// `root@HOST`, as it is said to the person.
    pub fn root(&self) -> String {
        format!("root@{}", self.target.host)
    }

    /// The account that connects, as it is said to the person.
    pub fn account(&self) -> &str {
        self.target.user.as_deref().unwrap_or_default()
    }
}

struct Live {
    setup: Setup,
    /// Counts each piece of work begun: one overtaken says nothing more.
    turn: u64,
    stop: Arc<AtomicBool>,
    task: Option<tokio::task::AbortHandle>,
}

static LIVE: Mutex<Option<Live>> = Mutex::new(None);

fn live() -> olo_sync::MutexGuard<'static, Option<Live>> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The host being set up, if one is.
pub fn current() -> Option<Setup> {
    live().as_ref().map(|l| l.setup.clone())
}

/// The stage, changed by the work of turn `turn` (nothing if another
/// has begun since); the windows are told.
fn set(turn: u64, stage: Stage) {
    {
        let mut live = live();
        let Some(live) = live.as_mut().filter(|l| l.turn == turn) else { return };
        live.setup.stage = stage;
    }
    crate::ui::setup_changed();
}

/// Begins a piece of work for the host being set up, on the hub's
/// threads; the stage is `stage` meanwhile.
fn work<F, Fut>(hub: &Arc<RexHub>, stage: Stage, f: F)
where
    F: FnOnce(Arc<RexHub>, Setup, u64, Arc<AtomicBool>) -> Fut,
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
        live.setup.stage = stage;
        (live.setup.clone(), live.turn, live.stop.clone())
    };
    let task = hub.spawn(f(hub.clone(), begun.0, begun.1, begun.2));
    if let Some(live) = live().as_mut().filter(|l| l.turn == begun.1) {
        live.task = Some(task);
    }
    crate::ui::setup_changed();
}

/// Sets `target`'s host up to run Rex in `mode` and adds it as `name`:
/// it is looked at first, and nothing is changed there before the
/// person has agreed ([`go`]).
pub fn begin(hub: &Arc<RexHub>, name: String, target: SshTarget, mode: ServerMode) {
    // (Signed in to with the agent Rex is set to use, here and for root.)
    let target = hub.with_agent(target);
    *live() = Some(Live { setup: Setup { name, target, mode, stage: Stage::Looking, root_key: None, root_by: RootBy::Login }, turn: 0, stop: Arc::new(AtomicBool::new(false)), task: None });
    look(hub);
}

/// Asks the host how it stands: as it should be, it is added; else what
/// it needs is put to the person, with how root's steps will reach root
/// (with sudo, where the account may use it without a password: asked
/// only where a step is root's).
fn look(hub: &Arc<RexHub>) {
    work(hub, Stage::Looking, |hub, setup, turn, _| async move {
        let mut runner = hub.over_ssh(setup.target.clone(), None, RootBy::Login);
        match provision::needs(&mut runner, setup.mode).await {
            Ok((_, steps)) if steps.is_empty() => finish(&hub, &setup, turn),
            Ok((_, steps)) => {
                let by = if steps.iter().any(|s| s.root) && provision::can_sudo(&mut runner).await { RootBy::Sudo } else { RootBy::Login };
                if let Some(live) = live().as_mut().filter(|l| l.turn == turn) {
                    live.setup.root_by = by;
                }
                set(turn, Stage::Confirm { ends_sessions: provision::ends_sessions(&steps), steps });
            }
            Err(failed) => set(turn, could_not_look(&setup, &failed)),
        }
    });
}

fn could_not_look(setup: &Setup, failed: &Failed) -> Stage {
    let account = setup.target.user.as_deref().unwrap_or_default();
    if failed.refused {
        // (As root refused is said, and with its icon: the same thing.)
        return Stage::Failed {
            what: olo_kit::tr!("{} didn’t accept your keys for {}", setup.host(), account).into(),
            why: olo_kit::tr!("Check the account name, and that one of your keys is in ~{}/.ssh/authorized_keys there.", account).into(),
            said: String::new(),
            icon: "key",
        };
    }
    Stage::Failed { what: olo_kit::tr!("Couldn’t look at {}", setup.host()).into(), why: String::new(), said: failed.why.clone(), icon: "stop" }
}

/// A step's title after "Couldn’t" ("Install Rex’s package": "install
/// Rex’s package").
fn after_couldnt(title: &str) -> String {
    let mut letters = title.chars();
    letters.next().map(|first| first.to_lowercase().chain(letters).collect()).unwrap_or_default()
}

/// How the host stands with the steps before `at` done.
fn done_so_far(setup: &Setup, at: usize) -> String {
    match at {
        0 => olo_kit::tr!("Nothing was changed on {}.", setup.host()).into(),
        1 => olo_kit::words::tr("Step 1 is done. Try Again carries on from step 2.").into(),
        n => olo_kit::tr!("Steps 1 to {} are done. Try Again carries on from step {}.", n, n + 1).into(),
    }
}

/// The host as it should be: added under its name, and connected.
fn finish(hub: &Arc<RexHub>, setup: &Setup, turn: u64) {
    let id = format!("host-{}", uuid::Uuid::new_v4().simple());
    hub.add_and_connect_host(id, setup.name.clone(), provision::address(setup.mode, &setup.target), true);
    set(turn, Stage::Done);
}

/// The person agreed (or chose a key for root, or asks again): root is
/// signed in first where a step is root's, so that nothing is changed
/// if it can't be, then the steps are carried out.
pub fn go(hub: &Arc<RexHub>) {
    let Some(setup) = current() else { return };
    let steps = match &setup.stage {
        Stage::Confirm { steps, .. } | Stage::Root { steps } => steps.clone(),
        Stage::RootKey { steps, keys, chosen } => {
            // (The key chosen, by its fingerprint.)
            if let Some(live) = live().as_mut() {
                live.setup.root_key = keys.get(*chosen).map(|k| k.fingerprint.clone());
            }
            steps.clone()
        }
        _ => return,
    };
    work(hub, Stage::Root { steps: steps.clone() }, |hub, setup, turn, stop| async move {
        let mut runner = hub.over_ssh(setup.target.clone(), setup.root_key.clone(), setup.root_by);
        // (Root reached before anything is changed: signed in, or sudo
        // run; a sudo that asks for a password fails at once, and says
        // so.)
        let reached = if steps.iter().any(|s| s.root) {
            match provision::Runner::run(&mut runner, true, "true", &mut |_| {}).await {
                Ok(ran) if !ran.ok() => Err(Failed { refused: false, why: ran.output.trim().to_owned() }),
                Ok(_) => Ok(()),
                Err(failed) => Err(failed),
            }
        } else {
            Ok(())
        };
        if let Err(failed) = reached {
            hub.forget_ssh(&setup.target.as_root(setup.root_key.clone())).await;
            if failed.refused {
                let keys = hub.agent_keys().await;
                // (The one tried, if one was chosen; else the first.)
                let chosen = setup.root_key.as_ref().and_then(|tried| keys.iter().position(|k| k.fingerprint == *tried)).unwrap_or(0);
                set(turn, Stage::RootKey { steps, keys, chosen });
            } else if setup.root_by == RootBy::Sudo {
                set(turn, Stage::Failed { what: olo_kit::tr!("Couldn’t use sudo as {}", setup.account()).into(), why: done_so_far(&setup, 0), said: failed.why, icon: "stop" });
            } else {
                set(turn, Stage::Failed { what: olo_kit::tr!("Couldn’t reach {}", setup.root()).into(), why: done_so_far(&setup, 0), said: failed.why, icon: "stop" });
            }
            return;
        }
        set(turn, Stage::Working { steps: steps.clone(), at: 0, said: String::new(), stopping: false });
        // (What a step writes comes fast, a package's hundreds of lines:
        // the window is told of a line at most ten times a second.)
        let mut told = std::time::Instant::now();
        let outcome = provision::set_up(&mut runner, setup.mode, &steps, &stop, &mut |at, line| {
            let step_begins = line.is_none();
            {
                let mut live = live();
                let Some(live) = live.as_mut().filter(|l| l.turn == turn) else { return };
                let stopping = matches!(live.setup.stage, Stage::Working { stopping: true, .. });
                live.setup.stage = Stage::Working { steps: steps.clone(), at, said: line.unwrap_or_default().trim().to_owned(), stopping };
            }
            if step_begins || told.elapsed() >= std::time::Duration::from_millis(100) {
                told = std::time::Instant::now();
                crate::ui::setup_changed();
            }
        })
        .await;
        // Root's connection is for this and nothing else; and the
        // account's is made anew, as the member of whatever group it was
        // put in.
        hub.forget_ssh(&setup.target.as_root(setup.root_key.clone())).await;
        hub.forget_ssh(&setup.target).await;
        match outcome {
            Outcome::Done => finish(&hub, &setup, turn),
            Outcome::Stopped { before } => set(
                turn,
                Stage::Failed {
                    what: olo_kit::tr!("Stopped before step {} of {}", before + 1, steps.len()).into(),
                    why: olo_kit::words::tr("What was done stays done. Setting it up again carries on from there.").into(),
                    said: String::new(),
                    icon: "info",
                },
            ),
            Outcome::Failed { at, failed } if at < steps.len() => {
                set(
                    turn,
                    Stage::Failed {
                        what: olo_kit::tr!("Couldn’t {}", after_couldnt(&olo_kit::words::of(steps[at].title.clone()))).into(),
                        why: done_so_far(&setup, at),
                        said: failed.why,
                        icon: "stop",
                    },
                );
            }
            Outcome::Failed { failed, .. } => {
                set(turn, Stage::Failed { what: olo_kit::tr!("Couldn’t look at {} again", setup.host()).into(), why: olo_kit::words::tr("Every step ran.").into(), said: failed.why, icon: "stop" })
            }
            Outcome::Left(left) => set(
                turn,
                Stage::Failed {
                    what: olo_kit::tr!("{} still isn’t set up", setup.host()).into(),
                    why: olo_kit::tr!("Every step ran, and it still needs: {}.", left.iter().map(|s| after_couldnt(&olo_kit::words::of(s.title.clone()))).collect::<Vec<_>>().join("; ")).into(),
                    said: String::new(),
                    icon: "warning",
                },
            ),
        }
    });
}

/// Which key root is to be offered ([`Stage::RootKey`]).
pub fn choose_key(i: usize) {
    if let Some(Live { setup: Setup { stage: Stage::RootKey { chosen, keys, .. }, .. }, .. }) = live().as_mut()
        && i < keys.len()
    {
        *chosen = i;
    }
    crate::ui::setup_changed();
}

/// After it didn't go: the host is looked at again, and what it still
/// needs is put to the person.
pub fn again(hub: &Arc<RexHub>) {
    if current().is_some() {
        look(hub);
    }
}

/// Stops the work after the step it is at (never in one).
pub fn stop() {
    if let Some(live) = live().as_mut()
        && let Stage::Working { stopping, .. } = &mut live.setup.stage
    {
        live.stop.store(true, Ordering::Relaxed);
        *stopping = true;
    }
    crate::ui::setup_changed();
}

/// Lets the host being set up go (back to the form, or the window
/// closed): work that changes nothing there is ended at once; a step
/// being carried out is let finish, and no more are begun.
pub fn back() {
    // (Taken in a statement of its own: the lock isn't held past it.)
    let gone = live().take();
    if let Some(mut gone) = gone {
        gone.stop.store(true, Ordering::Relaxed);
        if !matches!(gone.setup.stage, Stage::Working { .. })
            && let Some(task) = gone.task.take()
        {
            task.abort();
        }
    }
    crate::ui::setup_changed();
}

/// What an address typed into Add Host is.
#[derive(Clone, Debug, PartialEq)]
pub enum Address {
    Empty,
    /// A host reached over SSH as an account of the person's own.
    Ssh(SshTarget),
    /// An SSH address that won't do, and why (said under the field).
    Unfit(&'static str),
    /// Any other address (a tailnet name, a URL): added as it is.
    Other,
}

/// What `typed` is. An SSH address names the account to connect as, and
/// not root: root is asked for only to set a server up, and a
/// connection kept as root would run every terminal there as root.
pub fn address(typed: &str) -> Address {
    let typed = typed.trim();
    if typed.is_empty() {
        return Address::Empty;
    }
    if !typed.starts_with("ssh://") {
        return Address::Other;
    }
    match SshTarget::parse(typed) {
        Err(_) => Address::Unfit("That isn’t an SSH address: ssh://you@host"),
        Ok(target) => match target.user.as_deref() {
            None => Address::Unfit("Name the account to connect as: ssh://you@host"),
            Some("root") => Address::Unfit("Connect as your own account, not as root."),
            Some(_) => Address::Ssh(target),
        },
    }
}

/// A host made up in the stage `named`, for pictures of the window and
/// its lint (`OLO_REX_DEMO_SETUP`): nothing is connected to.
pub fn demo(named: &str) -> Option<Setup> {
    // (A stage's name ending "-sudo": the account may use sudo, and root's
    // steps are to be run with it.)
    let (named, root_by) = match named.strip_suffix("-sudo") {
        Some(named) => (named, RootBy::Sudo),
        None => (named, RootBy::Login),
    };
    let target = SshTarget::parse("ssh://alice@build.example.com").ok()?;
    let bare = provision::Facts {
        os: "Linux".into(),
        distro: "ubuntu".into(),
        user: "alice".into(),
        uid: 1000,
        groups: vec!["alice".into()],
        rex: Some("/usr/local/bin/rex".into()),
        version: Some("rex 0.1.0-snapshot.2506".into()),
        bare: true,
        systemd: true,
        fetch: Some(provision::Fetch::Curl),
        ..Default::default()
    };
    let steps = provision::plan(ServerMode::Login, &bare).ok()?;
    let keys = vec![
        AgentKey { name: "work laptop".into(), kind: "ED25519".into(), fingerprint: "SHA256:Zk1nC2aqb0rmf7JdXw9u4T5yV8sL3hPeGiYoRtUxNcA".into() },
        AgentKey { name: "build servers".into(), kind: "RSA".into(), fingerprint: "SHA256:q7LmP0sVd3xYbN8cK5eT1uRzW2aHj6gFoI9yC4nBvXk".into() },
    ];
    let stage = match named {
        "looking" => Stage::Looking,
        "confirm" => Stage::Confirm { ends_sessions: false, steps },
        "confirm-ends" => {
            let shared = provision::Facts {
                rex: Some("/usr/bin/rex".into()),
                bare: false,
                packaged: true,
                shared: provision::Unit { active: true, enabled: true },
                groups: vec!["alice".into(), "rex".into()],
                rex_shell: Some("/bin/bash".into()),
                ..bare
            };
            let steps = provision::plan(ServerMode::Login, &shared).ok()?;
            Stage::Confirm { ends_sessions: provision::ends_sessions(&steps), steps }
        }
        "root" => Stage::Root { steps },
        "key" => Stage::RootKey { steps, keys, chosen: 0 },
        "nokeys" => Stage::RootKey { steps, keys: Vec::new(), chosen: 0 },
        "working" => Stage::Working { steps, at: 0, said: "Unpacking rex (0.1.0~alpha.2506) ...".into(), stopping: false },
        "stopping" => Stage::Working {
            steps,
            at: 3,
            said: "Created symlink '/etc/systemd/system/multi-user.target.wants/rex-login-server.service' → '/usr/lib/systemd/system/rex-login-server.service'.".into(),
            stopping: true,
        },
        "failed" => Stage::Failed {
            what: "Couldn’t install Rex’s package".into(),
            why: "Nothing was changed on build.example.com.".into(),
            said: "E: Could not get lock /var/lib/dpkg/lock-frontend. It is held by process 4312 (unattended-upgr)".into(), icon: "stop" },
        "failed-later" => Stage::Failed {
            what: "Couldn’t start the login server".into(),
            why: "Steps 1 to 3 are done. Try Again carries on from step 4.".into(),
            said: "Job for rex-login-server.service failed because the control process exited with error code.\nSee \"systemctl status rex-login-server.service\" and \"journalctl -xeu rex-login-server.service\" for details.".into(), icon: "stop" },
        "refused" => could_not_look(&Setup { name: String::new(), target: target.clone(), mode: ServerMode::Login, stage: Stage::Looking, root_key: None, root_by }, &Failed { refused: true, why: String::new() }),
        _ => return None,
    };
    Some(Setup { name: "build".into(), target, mode: ServerMode::Login, stage, root_key: None, root_by })
}

/// Puts a made-up host in a stage (the pictures' and the lint's).
pub fn show_demo(named: &str) -> bool {
    let Some(setup) = demo(named) else { return false };
    *live() = Some(Live { setup, turn: 0, stop: Arc::new(AtomicBool::new(false)), task: None });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ssh_address_names_an_account_that_isnt_root() {
        assert_eq!(address(""), Address::Empty);
        assert_eq!(address("   "), Address::Empty);
        assert_eq!(address("build.tailnet.ts.net"), Address::Other);
        assert_eq!(address("https://build.example.com:7297"), Address::Other);
        assert!(matches!(address("ssh://alice@build.example.com"), Address::Ssh(t) if t.user.as_deref() == Some("alice") && t.host == "build.example.com"));
        assert!(matches!(address(" ssh://alice@10.0.0.5:2222 "), Address::Ssh(t) if t.port == Some(2222)));
        assert!(matches!(address("ssh://build.example.com"), Address::Unfit(why) if why.contains("Name the account")));
        assert!(matches!(address("ssh://root@build.example.com"), Address::Unfit(why) if why.contains("your own account")));
        assert!(matches!(address("ssh://@build.example.com"), Address::Unfit(_)));
        assert!(matches!(address("ssh://"), Address::Unfit(_)));
    }

    #[test]
    fn every_stage_can_be_pictured() {
        for named in ["looking", "confirm", "confirm-ends", "root", "key", "nokeys", "working", "stopping", "failed", "failed-later", "refused"] {
            let setup = demo(named).unwrap_or_else(|| panic!("no demo of {named}"));
            assert_eq!((setup.host(), setup.root().as_str(), setup.root_by), ("build.example.com", "root@build.example.com", RootBy::Login));
            // (Each with root's work done through sudo, as the account.)
            let sudo = demo(&format!("{named}-sudo")).unwrap_or_else(|| panic!("no demo of {named}-sudo"));
            assert_eq!((sudo.account(), sudo.root_by, &sudo.stage), ("alice", RootBy::Sudo, &setup.stage));
        }
        assert!(matches!(demo("confirm").unwrap().stage, Stage::Confirm { steps, ends_sessions: false } if steps.len() == 5 && steps.iter().filter(|s| s.root).count() == 4));
        assert!(matches!(demo("confirm-ends").unwrap().stage, Stage::Confirm { ends_sessions: true, .. }));
        assert!(
            matches!(demo("refused").unwrap().stage, Stage::Failed { what, why, said, icon } if what == "build.example.com didn’t accept your keys for alice" && why.contains("~alice/.ssh/authorized_keys") && said.is_empty() && icon == "key")
        );
        assert_eq!(
            (after_couldnt("Install Rex’s package"), after_couldnt("Allow alice to sign in"), after_couldnt("")),
            ("install Rex’s package".to_owned(), "allow alice to sign in".to_owned(), String::new())
        );
        assert!(demo("nothing").is_none());
    }
}
