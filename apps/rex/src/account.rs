//! The Account window's state: a far host's account made ready for
//! work in one go (Ben, 2026-11-22: "have a way in the rex app where i
//! can have rex auto-setup my user on a remote server like install
//! claude code, agy, codex … get everything up and running for a user
//! as quickly and easily as possible on a new remote server").
//!
//! What the account is given is a list of [`Item`]s in groups: the
//! steps of `olo_rex_client::ready` (programs found by name, the three
//! agents, git's identity), and Olo's skill for agents, which is a
//! file in the account's own home too (it was a section of the Host
//! window). How each stands is the hub's to know (`HostStands`, asked
//! of the host over SSH); here is which host the window is of, the
//! item being done now with the line its installer wrote last, those
//! waiting behind it, and what the host said of one that couldn't be
//! done. One is done at a time, off the connection's own path, shown
//! as it goes and stopped on Stop; one that fails doesn't keep the
//! rest from being tried. One host at a time (there is one Account
//! window); what is being done goes on with the window closed, and is
//! there when it is opened again.

use std::{sync::Arc, time::Instant};

use olo_rex_client::ready::Step;
use olo_sync::Mutex;

use crate::hub::{HostConnState, HostStands, HostThing, RexHub};

/// One thing an account is given, as the window lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    Step(Step),
    /// The person's settings for their agents, as on this computer
    /// (`olo_rex_client::carry`).
    Carry,
    /// The agents' sessions used lately, copied to resume them there
    /// (`olo_rex_client::sessions`): by its own Copy only, never a part
    /// of the run of all that is left, nor counted in how ready the
    /// account is (a week of them can be gigabytes, and none is needed).
    Sessions,
    /// Olo's skill for agents (`bridge::SKILL`), in the account's
    /// `~/.claude`.
    Skill,
    /// The person's repositories, all of them as one row: cloned and
    /// set up (`olo_rex_client::repos`; the Repositories window has
    /// each by itself).
    Repos,
}

/// The window's groups, each with its items in their order (and the
/// order all of them are done in). A group is a heading and its rows:
/// what is to come (the person's settings for their agents copied,
/// their repositories cloned) is a row here that opens a window of
/// its own, never a list inside this one.
pub const GROUPS: [(&str, &[Item]); 3] = [
    ("Programs", &[Item::Step(Step::Path), Item::Step(Step::Claude), Item::Step(Step::Codex), Item::Step(Step::Antigravity)]),
    ("Settings", &[Item::Step(Step::Git), Item::Carry, Item::Sessions, Item::Skill]),
    ("Repositories", &[Item::Repos]),
];

impl Item {
    pub fn all() -> impl Iterator<Item = Item> {
        GROUPS.into_iter().flat_map(|(_, items)| items.iter().copied())
    }

    pub fn id(self) -> &'static str {
        match self {
            Item::Step(step) => step.id(),
            Item::Carry => "settings",
            Item::Sessions => "sessions",
            Item::Skill => "skill",
            Item::Repos => "repos",
        }
    }

    pub fn of(id: &str) -> Option<Item> {
        Item::all().find(|item| item.id() == id)
    }

    /// What it is called on its row.
    pub fn title(self) -> &'static str {
        match self {
            Item::Step(step) => step.title(),
            Item::Carry => "Agent settings",
            Item::Sessions => "Agent sessions",
            Item::Skill => "Skill for agents",
            Item::Repos => "Your repositories",
        }
    }

    /// What it is, in a line under its row.
    pub fn why(self) -> &'static str {
        match self {
            Item::Step(step) => step.why(),
            Item::Carry => "Settings, instructions, skills and commands. Never a sign-in or a key.",
            Item::Sessions => "Claude Code’s, Codex’s and Antigravity’s sessions, to resume them there.",
            Item::Skill => "Tells Claude Code and other agents there how to use the bridge and Rex.",
            Item::Repos => "The ones you choose, cloned in your home folder there and set up once.",
        }
    }

    /// Whether it is there on `host`, as last heard; none: not known
    /// (not asked, not answered, or not connected). `who`: this
    /// computer's name and address for git.
    pub fn done(self, host: &HostStands, who: Option<&(String, String)>) -> Option<bool> {
        if host.conn != HostConnState::Connected || !host.ssh {
            return None;
        }
        match self {
            Item::Step(step) => host.account.as_ref().map(|account| account.done(step, who)),
            Item::Carry => host.carried.map(|(to_copy, ..)| to_copy == 0),
            Item::Sessions => host.sessions.map(|s| s.sessions == 0),
            Item::Skill => host.skill,
            Item::Repos => host.repos_there.as_ref().map(|there| there.iter().all(olo_rex_client::repos::There::done)),
        }
    }
}

impl Item {
    /// Whether it is one of what `host`'s account is to have: the
    /// repositories only where the person has chosen some.
    pub fn applies(self, host: &HostStands) -> bool {
        self != Item::Repos || !host.repos.is_empty()
    }

    /// Whether it counts in how ready the account is (not the
    /// sessions: [`Item::Sessions`]).
    pub fn counted(self) -> bool {
        self != Item::Sessions
    }
}

/// How much of the account is ready on `host`: how many of the items
/// are there, of how many, and the names of those that aren't. None
/// where that isn't known.
pub fn tally(host: &HostStands) -> Option<(usize, usize, Vec<&'static str>)> {
    let stands: Vec<(Item, bool)> = Item::all().filter(|item| item.applies(host) && item.counted()).map(|item| item.done(host, None).map(|done| (item, done))).collect::<Option<_>>()?;
    let left: Vec<&'static str> = stands.iter().filter(|(_, done)| !done).map(|(item, _)| olo_kit::words::tr(item.title())).collect();
    Some((stands.len() - left.len(), stands.len(), left))
}

/// What is being done now.
#[derive(Clone, Debug, PartialEq)]
pub struct Doing {
    pub item: Item,
    /// The line its script wrote last.
    pub line: String,
    /// Taken off, not put there (the skill).
    pub removing: bool,
    /// Of the repositories, the one being cloned now, by its name.
    pub of: Option<String>,
}

/// What the window is of, and at.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    /// The host's key in the hub.
    pub key: String,
    /// The name it goes by, as the window opened (its title's).
    pub name: String,
    pub doing: Option<Doing>,
    /// Those to be done after it, in their order.
    pub queue: Vec<Item>,
    /// How many have ended since something was last begun with
    /// nothing under way: the one being done is the next of the run
    /// ("Setting up 2 of 3…").
    pub ran: usize,
    /// What couldn't be done, the last time, and what the host said.
    pub failed: Vec<(Item, String)>,
    /// Of the repositories, the one a run couldn't clone, by its name.
    pub failed_repo: Option<String>,
    /// The host is being asked again how the account stands.
    pub checking: bool,
    /// The person's name and address for git, as this computer's git
    /// has them; none: it doesn't say.
    pub who: Option<(String, String)>,
}

impl Shown {
    pub fn failed(&self, item: Item) -> Option<&str> {
        self.failed.iter().find(|(i, _)| *i == item).map(|(_, said)| said.as_str())
    }

    pub fn doing(&self, item: Item) -> Option<&Doing> {
        self.doing.as_ref().filter(|doing| doing.item == item)
    }

    /// Something is being done, or waits to be.
    pub fn busy(&self) -> bool {
        self.doing.is_some() || !self.queue.is_empty()
    }
}

static LIVE: Mutex<Option<Shown>> = Mutex::new(None);
/// What stops the step being done now, and when the window was last
/// told of a line (an installer's bar writes many a second: the window
/// is told of ten).
static RUNNING: Mutex<(Option<tokio::task::AbortHandle>, Option<Instant>)> = Mutex::new((None, None));

fn live() -> olo_sync::MutexGuard<'static, Option<Shown>> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The host the window is of, if it is open (or something is still
/// being done to its account).
pub fn current() -> Option<Shown> {
    live().clone()
}

/// This computer's name and address for git, from git's own files (the
/// person's, then the one under the config folder). Rex's demo has a
/// made-up one.
fn this_computers_git() -> Option<(String, String)> {
    if crate::server::demo_mode() {
        return Some(("You".to_owned(), "you@example.org".to_owned()));
    }
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    let config = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(".config"));
    [home.join(".gitconfig"), config.join("git/config")].iter().find_map(|path| std::fs::read_to_string(path).ok().and_then(|text| olo_rex_client::ready::who_in(&text)))
}

/// The window is of the host `key`, which goes by `name`, from now.
/// (What is being done to another's account goes on to its end, and is
/// no longer shown.) The host is asked how its account stands.
pub fn open(hub: &Arc<RexHub>, key: &str, name: &str) {
    {
        let mut live = live();
        match live.as_mut().filter(|shown| shown.key == key) {
            Some(shown) => shown.name = name.to_owned(),
            None => *live = Some(Shown { key: key.to_owned(), name: name.to_owned(), ..demo() }),
        }
    }
    // (Git's file is read off the window's thread.)
    let of = key.to_owned();
    std::thread::spawn(move || {
        let who = this_computers_git();
        if let Some(shown) = live().as_mut().filter(|shown| shown.key == of) {
            shown.who = who;
        }
        crate::ui::account_changed();
    });
    check(hub);
}

/// The demo's window in a state of its own, for its pictures
/// (`OLO_REX_DEMO_ACCOUNT`: `doing`, an item under way and one
/// waiting; `failed`, one that couldn't be done; `asking`, before the
/// host has answered).
fn demo() -> Shown {
    if !crate::server::demo_mode() {
        return Shown::default();
    }
    let who = this_computers_git();
    match std::env::var("OLO_REX_DEMO_ACCOUNT").ok().as_deref() {
        Some("doing") => Shown {
            doing: Some(Doing { item: Item::Step(Step::Codex), line: "Downloading Codex 0.172.0 for x86_64-unknown-linux-musl…  62%".into(), removing: false, of: None }),
            queue: vec![Item::Step(Step::Antigravity), Item::Carry, Item::Skill],
            who,
            ..Shown::default()
        },
        Some("failed") => Shown { failed: vec![(Item::Step(Step::Codex), "curl: (6) Could not resolve host: chatgpt.com".into())], who, ..Shown::default() },
        Some("asking") => Shown { checking: true, who, ..Shown::default() },
        Some("cloning") => Shown {
            doing: Some(Doing { item: Item::Repos, line: "Receiving objects:  62% (1240/2000), 3.10 MiB | 2.40 MiB/s".into(), removing: false, of: Some("website".into()) }),
            who,
            ..Shown::default()
        },
        // (The agents' sessions being copied, how far it is.)
        Some("copying") => Shown { doing: Some(Doing { item: Item::Sessions, line: "38%  ·  0.5 of 1.2 GB".into(), removing: false, of: None }), who, ..Shown::default() },
        Some("copyfailed") => Shown { failed: vec![(Item::Sessions, "tar: .claude/projects: Cannot mkdir: No space left on device".into())], who, ..Shown::default() },
        Some("clonefailed") => Shown { failed: vec![(Item::Repos, "git@git.example.org: Permission denied (publickey).".into())], failed_repo: Some("website".into()), who, ..Shown::default() },
        _ => Shown { who, ..Shown::default() },
    }
}

/// The window closed: with nothing being done, it is of no host.
pub fn close() {
    let mut live = live();
    if live.as_ref().is_some_and(|shown| !shown.busy()) {
        *live = None;
    }
}

/// Asks the window's host again how its account stands (and whether
/// the skill is there).
pub fn check(hub: &Arc<RexHub>) {
    let key = {
        let mut live = live();
        let Some(shown) = live.as_mut().filter(|shown| !shown.checking) else { return };
        // (The demo's "asking" stays asking: there is nobody to answer.)
        if crate::server::demo_mode() {
            return;
        }
        shown.checking = true;
        shown.key.clone()
    };
    crate::ui::account_changed();
    let (of, again) = (key.clone(), hub.clone());
    hub.ask_account(&key, move || {
        let of_skill = of.clone();
        again.check_host(&of_skill.clone(), move || {
            if let Some(shown) = live().as_mut().filter(|shown| shown.key == of_skill) {
                shown.checking = false;
            }
            crate::ui::account_changed();
        });
    });
}

/// `items` done on the window's host, one after another, behind
/// whatever is being done there already.
pub fn run(hub: &Arc<RexHub>, items: &[Item]) {
    {
        let mut live = live();
        let Some(shown) = live.as_mut() else { return };
        if !shown.busy() {
            shown.ran = 0;
        }
        for &item in items {
            if shown.doing(item).is_some() || shown.queue.contains(&item) {
                continue;
            }
            shown.queue.push(item);
            shown.failed.retain(|(i, _)| *i != item);
        }
    }
    next(hub);
}

/// The skill taken off the window's host (the one thing here that is).
pub fn remove_skill(hub: &Arc<RexHub>) {
    let key = {
        let mut live = live();
        let Some(shown) = live.as_mut().filter(|shown| !shown.busy()) else { return };
        shown.doing = Some(Doing { item: Item::Skill, line: String::new(), removing: true, of: None });
        shown.failed.retain(|(i, _)| *i != Item::Skill);
        shown.key.clone()
    };
    crate::ui::account_changed();
    let (of, again) = (key.clone(), hub.clone());
    hub.change_host(&key, HostThing::Skill, false, move |result| ended(&again, &of, Item::Skill, result));
}

/// `item` ended on the host `key`: how, and on to the next.
fn ended(hub: &Arc<RexHub>, key: &str, item: Item, result: Result<(), String>) {
    if let Some(shown) = live().as_mut().filter(|shown| shown.key == key) {
        let of = shown.doing(item).and_then(|doing| doing.of.clone());
        if shown.doing(item).is_some() {
            shown.doing = None;
            shown.ran += 1;
        }
        if item == Item::Repos {
            shown.failed_repo = of.filter(|_| result.is_err());
        }
        if let Err(said) = result {
            shown.failed.retain(|(i, _)| *i != item);
            shown.failed.push((item, said));
        }
    }
    RUNNING.lock().unwrap_or_else(|e| e.into_inner()).0 = None;
    // (The last of a run: the host asked how all of it stands now. One
    // item's doing changes another's standing: the skill's set-up
    // writes into the settings that were just copied.)
    if live().as_ref().is_some_and(|shown| shown.key == key && shown.queue.is_empty()) {
        check(hub);
    }
    next(hub);
}

/// The first of those waiting begun, if nothing is being done.
fn next(hub: &Arc<RexHub>) {
    let (key, item, who) = {
        let mut live = live();
        let Some(shown) = live.as_mut().filter(|shown| shown.doing.is_none() && !shown.queue.is_empty()) else {
            drop(live);
            crate::ui::account_changed();
            return;
        };
        let item = shown.queue.remove(0);
        shown.doing = Some(Doing { item, line: String::new(), removing: false, of: None });
        (shown.key.clone(), item, shown.who.clone())
    };
    crate::ui::account_changed();
    let (of_line, of_done, again) = (key.clone(), key.clone(), hub.clone());
    let step = match item {
        Item::Step(step) => Some(step),
        Item::Repos | Item::Sessions => None,
        Item::Carry => {
            hub.carry_settings(&key, move |result| ended(&again, &of_done, item, result));
            return;
        }
        Item::Skill => {
            hub.change_host(&key, HostThing::Skill, true, move |result| ended(&again, &of_done, item, result));
            return;
        }
    };
    let said = move |line: &str| {
        if let Some(doing) = live().as_mut().filter(|shown| shown.key == of_line).and_then(|shown| shown.doing.as_mut()).filter(|doing| doing.item == item) {
            // (The clones' run names each as it begins on it.)
            if let Some(name) = line.strip_prefix("Cloning ").and_then(|rest| rest.strip_suffix('…')) {
                doing.of = Some(name.to_owned());
            }
            doing.line = line.to_owned();
        }
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if running.1.is_none_or(|told| told.elapsed() >= std::time::Duration::from_millis(100)) {
            running.1 = Some(Instant::now());
            drop(running);
            crate::ui::account_changed();
        }
    };
    let stop = match (step, item) {
        (Some(step), _) => hub.ready_step(&key, step, who, said, move |result| ended(&again, &of_done, item, result)),
        (None, Item::Sessions) => hub.copy_sessions(&key, said, move |result| ended(&again, &of_done, item, result)),
        (None, _) => hub.clone_repos(&key, said, move |result| ended(&again, &of_done, item, result)),
    };
    // (None: it couldn't be begun, `ended` has heard so already, and
    // the next may be under way by now with a stop of its own.)
    if stop.is_some() {
        RUNNING.lock().unwrap_or_else(|e| e.into_inner()).0 = stop;
    }
}

/// What is being done stopped (its script on the host ends with it;
/// what is there already stays), and what waited behind it let go;
/// the host asked how things stand now.
pub fn stop(hub: &Arc<RexHub>) {
    if let Some(stop) = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).0.take() {
        stop.abort();
    }
    if let Some(shown) = live().as_mut() {
        // (The skill's file is a moment's work with no stop of its
        // own: it ends by itself.)
        if shown.doing.as_ref().is_some_and(|doing| matches!(doing.item, Item::Step(_) | Item::Repos | Item::Sessions)) {
            shown.doing = None;
        }
        shown.queue.clear();
    }
    crate::ui::account_changed();
    check(hub);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the window shows of a host is kept while it is of that
    /// host, and while something is being done there the window's
    /// closing doesn't forget it.
    #[test]
    fn the_window_keeps_what_is_being_done_when_it_closes() {
        *live() = Some(Shown { key: "a".into(), name: "far".into(), ..Shown::default() });
        close();
        assert_eq!(current(), None, "nothing being done: of no host");
        let claude = Item::Step(Step::Claude);
        *live() = Some(Shown {
            key: "a".into(),
            name: "far".into(),
            doing: Some(Doing { item: claude, line: "Downloading…".into(), removing: false, of: None }),
            queue: vec![Item::Skill],
            ..Shown::default()
        });
        close();
        let kept = current().unwrap();
        assert!(kept.busy() && kept.doing(claude).is_some());
        assert_eq!(kept.failed(Item::Skill), None);
        *live() = None;
    }

    #[test]
    fn every_item_has_a_name_of_its_own_and_is_in_a_group() {
        let all: Vec<Item> = Item::all().collect();
        assert_eq!(all.len(), 9);
        for item in &all {
            assert_eq!(Item::of(item.id()), Some(*item));
        }
        // (Every step of the client's is an item here.)
        assert!(Step::ALL.into_iter().all(|step| all.contains(&Item::Step(step))));
    }
}
