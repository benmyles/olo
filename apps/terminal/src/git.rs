//! Git repositories and their worktrees, in process (libgit2, through
//! git2; no `git` or `gh`): which repository a tab's folder is in and on
//! which branch, a repository's worktrees, making one (a branch of its
//! name from where the main worktree is), renaming and removing one; and
//! the pull request GitHub has for a branch, with its checks.
//!
//! A thread (`Watcher`) looks at every tab's folder each second and says
//! when its repository or branch changes; another asks GitHub about the
//! branches the tabs are on, every minute (every five without a token).

use olo_sync::Mutex;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use olo_kit_base::Proxy;

use crate::io;

/// The repository a folder is in.
#[derive(Debug, Clone, PartialEq)]
pub struct Repo {
    /// The main repository's folder name ("olo").
    pub name: String,
    /// Its main worktree (the folder `.git` is in).
    pub main: PathBuf,
    /// The worktree the folder is in (the main one, or a linked one).
    pub here: PathBuf,
    /// A linked worktree, not the main one.
    pub linked: bool,
    /// The branch checked out (None: detached).
    pub branch: Option<String>,
    /// GitHub's owner and name for its `origin` (and `upstream`), if it's
    /// there.
    pub github: Vec<(String, String)>,
    /// Its git folders: the worktree's own, and the repository's.
    gitdir: PathBuf,
    common: PathBuf,
}

impl Repo {
    /// When what `look` read last changed: the worktree's HEAD (its
    /// branch) and the repository's config (its remotes).
    fn stamp(&self) -> Stamp {
        vec![mtime(&self.gitdir.join("HEAD")), mtime(&self.common.join("config"))]
    }
}

type Stamp = Vec<Option<std::time::SystemTime>>;

fn mtime(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// When the repository's worktrees or branches last changed: one made or
/// removed, or any of them on another branch.
fn worktrees_stamp(common: &Path) -> Stamp {
    let dir = common.join("worktrees");
    let refs = [common.join("HEAD"), dir.clone(), common.join("refs/heads"), common.join("packed-refs")];
    let mut stamp: Stamp = refs.iter().map(|p| mtime(p)).collect();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut heads: Vec<PathBuf> = entries.flatten().map(|e| e.path().join("HEAD")).collect();
        heads.sort();
        stamp.extend(heads.iter().map(|h| mtime(h)));
    }
    stamp
}

/// One of a repository's worktrees.
#[derive(Debug, Clone, PartialEq)]
pub struct Worktree {
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    /// The main worktree.
    pub main: bool,
}

/// The repository `dir` is in, if any.
pub fn look(dir: &Path) -> Option<Repo> {
    let repo = git2::Repository::discover(dir).ok()?;
    let here = repo.workdir()?.to_path_buf();
    let main = main_of(&repo)?;
    let name = main.file_name()?.to_string_lossy().trim_end_matches(".git").to_owned();
    let branch = repo.head().ok().filter(|h| h.is_branch()).and_then(|h| h.shorthand().map(str::to_owned));
    let github = ["origin", "upstream"]
        .iter()
        .filter_map(|r| repo.find_remote(r).ok().and_then(|r| r.url().and_then(github_of)))
        .fold(Vec::new(), |mut v, g| {
            if !v.contains(&g) {
                v.push(g);
            }
            v
        });
    Some(Repo {
        name,
        linked: repo.is_worktree(),
        main: tidy(&main),
        here: tidy(&here),
        branch,
        github,
        gitdir: repo.path().to_path_buf(),
        common: repo.commondir().to_path_buf(),
    })
}

/// The main worktree of the repository `repo` is (in) one of.
fn main_of(repo: &git2::Repository) -> Option<PathBuf> {
    let common = repo.commondir();
    let common = common.canonicalize().unwrap_or_else(|_| common.to_path_buf());
    // `…/x/.git` for a repository with a worktree; a bare one is its own.
    match common.file_name().and_then(|n| n.to_str()) {
        Some(".git") => common.parent().map(Path::to_path_buf),
        _ => Some(common),
    }
}

/// `p` without the trailing slash libgit2 gives a workdir.
fn tidy(p: &Path) -> PathBuf {
    p.components().collect()
}

/// A remote's owner and name on GitHub ("git@github.com:o/r.git",
/// "https://github.com/o/r", "ssh://git@github.com/o/r.git").
pub fn github_of(url: &str) -> Option<(String, String)> {
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
        .or_else(|| url.strip_prefix("https://github.com/"))
        .or_else(|| url.strip_prefix("http://github.com/"))
        .or_else(|| url.strip_prefix("git://github.com/"))?;
    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    let (owner, name) = rest.split_once('/')?;
    (!owner.is_empty() && !name.is_empty() && !name.contains('/')).then(|| (owner.to_owned(), name.to_owned()))
}

/// The worktrees of the repository whose main worktree is `main`: the
/// main one first, then the others by name.
pub fn worktrees(main: &Path) -> Vec<Worktree> {
    let Ok(repo) = git2::Repository::open(main) else { return Vec::new() };
    let branch_of = |r: &git2::Repository| r.head().ok().filter(|h| h.is_branch()).and_then(|h| h.shorthand().map(str::to_owned));
    let mut out = vec![Worktree {
        name: main.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        path: main.to_path_buf(),
        branch: branch_of(&repo),
        main: true,
    }];
    let mut linked: Vec<Worktree> = repo
        .worktrees()
        .map(|names| {
            names
                .iter()
                .flatten()
                .filter_map(|n| repo.find_worktree(n).ok())
                .filter(|w| w.validate().is_ok())
                .map(|w| Worktree {
                    name: w.path().file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(),
                    path: tidy(w.path()),
                    branch: git2::Repository::open_from_worktree(&w).ok().as_ref().and_then(branch_of),
                    main: false,
                })
                .collect()
        })
        .unwrap_or_default();
    linked.sort_by_key(|w| w.name.to_lowercase());
    out.extend(linked);
    out
}

/// The repository's local branches, by name.
pub fn branches(main: &Path) -> Vec<String> {
    let Ok(repo) = git2::Repository::open(main) else { return Vec::new() };
    let Ok(list) = repo.branches(Some(git2::BranchType::Local)) else { return Vec::new() };
    let mut names: Vec<String> = list.flatten().filter_map(|(b, _)| b.name().ok().flatten().map(str::to_owned)).collect();
    names.sort();
    names
}

/// What a worktree's name may be: a folder and a branch name both.
pub fn valid_name(name: &str) -> Result<(), String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("Give it a name.".into());
    }
    if n.contains(['/', '\\']) || n.starts_with(['.', '-']) || n.ends_with(".lock") || n.contains("..") {
        return Err("A name can't have a slash, start with a dot or a dash, or end in .lock.".into());
    }
    if n.chars().any(|c| c.is_whitespace() || c.is_control() || "~^:?*[@{".contains(c)) {
        return Err("A name can't have spaces or any of ~ ^ : ? * [ @ {.".into());
    }
    Ok(())
}

/// Where a new worktree `name` of repository `repo` goes, under
/// `location` (`~` for the home folder).
pub fn place(location: &str, repo: &str, name: &str) -> PathBuf {
    let location = match location.strip_prefix('~') {
        Some(rest) => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest.trim_start_matches('/')),
        None => PathBuf::from(location),
    };
    location.join(repo).join(name)
}

/// How far making a worktree has got, and whether to stop.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
    pub cancel: AtomicBool,
}

/// Files checked out at a time, between looks at `Progress::cancel`.
const BATCH: usize = 400;

/// Make worktree `name` of `main` at `path`, on a new branch `name` from
/// where the main worktree is (or on branch `name`, if there is one and
/// no worktree has it out), as `git worktree add` does: git's record of
/// it in the repository, its `.git` file, then its files, a batch at a
/// time so `progress` shows and stops it. Stopped or failed, nothing of
/// it is left (nor its branch, if this made it).
pub fn add(main: &Path, name: &str, path: &Path, progress: &Progress) -> Result<PathBuf, String> {
    let msg = |e: git2::Error| e.message().to_owned();
    valid_name(name)?;
    if path.exists() {
        return Err(format!("{} is there already.", path.display()));
    }
    let repo = git2::Repository::open(main).map_err(msg)?;
    let admin = repo.commondir().join("worktrees").join(name);
    if admin.exists() || repo.find_worktree(name).is_ok() {
        return Err(format!("The repository has a worktree called {name} already."));
    }
    let (branch, made) = match repo.find_branch(name, git2::BranchType::Local) {
        Ok(b) => (b, false),
        Err(_) => {
            let head = repo.head().and_then(|h| h.peel_to_commit()).map_err(msg)?;
            (repo.branch(name, &head, false).map_err(msg)?, true)
        }
    };
    if !made && let Some(w) = worktrees(main).into_iter().find(|w| w.branch.as_deref() == Some(name)) {
        return Err(format!("Branch {name} is checked out in {} already.", w.path.display()));
    }
    let refname = branch.get().name().unwrap_or_default().to_owned();
    let commit = branch.get().peel_to_commit().map_err(msg)?;
    let undo = |e: String| {
        let _ = std::fs::remove_dir_all(path);
        let _ = std::fs::remove_dir_all(&admin);
        if made && let Ok(mut b) = repo.find_branch(name, git2::BranchType::Local) {
            let _ = b.delete();
        }
        Err(e)
    };
    let made_dirs = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(admin.parent().unwrap_or(&admin))?;
        std::fs::create_dir(&admin)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir(path)?;
        let path = path.canonicalize()?;
        let admin = admin.canonicalize()?;
        std::fs::write(path.join(".git"), format!("gitdir: {}\n", admin.display()))?;
        std::fs::write(admin.join("commondir"), "../..\n")?;
        std::fs::write(admin.join("gitdir"), format!("{}\n", path.join(".git").display()))?;
        std::fs::write(admin.join("HEAD"), format!("ref: {refname}\n"))
    })();
    if let Err(e) = made_dirs {
        return undo(format!("Can't make {}: {e}", path.display()));
    }
    let wt = match git2::Repository::open(path) {
        Ok(r) => r,
        Err(e) => return undo(msg(e)),
    };
    // The worktree's own objects (the same ones, through it).
    let tree = match wt.find_commit(commit.id()).and_then(|c| c.tree()) {
        Ok(t) => t,
        Err(e) => return undo(msg(e)),
    };
    let mut files: Vec<String> = Vec::new();
    let _ = tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
        if entry.kind() != Some(git2::ObjectType::Tree) {
            files.push(format!("{dir}{}", entry.name().unwrap_or_default()));
        }
        git2::TreeWalkResult::Ok
    });
    progress.total.store(files.len(), Ordering::Relaxed);
    for (n, batch) in files.chunks(BATCH).enumerate() {
        if progress.cancel.load(Ordering::Relaxed) {
            return undo("Stopped.".into());
        }
        let base = n * BATCH;
        let mut co = git2::build::CheckoutBuilder::new();
        co.force().disable_pathspec_match(true);
        for f in batch {
            co.path(f);
        }
        co.progress(|_, done, _| progress.done.store(base + done, Ordering::Relaxed));
        if let Err(e) = wt.checkout_tree(tree.as_object(), Some(&mut co)) {
            return undo(msg(e));
        }
    }
    progress.done.store(files.len(), Ordering::Relaxed);
    Ok(path.to_path_buf())
}

/// The linked worktree of `main` at `path`.
fn find(repo: &git2::Repository, path: &Path) -> Result<git2::Worktree, String> {
    let names = repo.worktrees().map_err(|e| e.message().to_owned())?;
    names
        .iter()
        .flatten()
        .filter_map(|n| repo.find_worktree(n).ok())
        .find(|w| tidy(w.path()) == tidy(path))
        .ok_or_else(|| "That isn't one of the repository's worktrees.".to_owned())
}

/// Rename the worktree at `path` to `new` (its folder, beside where it
/// is; and its branch, if that has its old name). Its shells follow it:
/// a folder moved keeps its programs in it.
pub fn rename(main: &Path, path: &Path, new: &str) -> Result<PathBuf, String> {
    valid_name(new)?;
    let repo = git2::Repository::open(main).map_err(|e| e.message().to_owned())?;
    let wt = find(&repo, path)?;
    let old = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let to = path.with_file_name(new);
    if to.exists() {
        return Err(format!("{} is there already.", to.display()));
    }
    // Where git keeps it: its gitdir file says where its folder is.
    let admin = repo.commondir().join("worktrees").join(wt.name().unwrap_or(&old));
    std::fs::rename(path, &to).map_err(|e| format!("Can't move it: {e}"))?;
    let gitdir = format!("{}\n", to.join(".git").display());
    if let Err(e) = std::fs::write(admin.join("gitdir"), gitdir) {
        let _ = std::fs::rename(&to, path);
        return Err(format!("Can't tell git where it went: {e}"));
    }
    if let Ok(mut b) = repo.find_branch(&old, git2::BranchType::Local)
        && repo.find_branch(new, git2::BranchType::Local).is_err()
    {
        let _ = b.rename(new, false);
    }
    Ok(to)
}

/// How many files in the worktree at `path` have changes not committed
/// (untracked ones too).
pub fn changes(path: &Path) -> usize {
    let Ok(repo) = git2::Repository::open(path) else { return 0 };
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(false).include_ignored(false);
    repo.statuses(Some(&mut opts)).map(|s| s.len()).unwrap_or(0)
}

/// Remove the worktree at `path`: its folder and git's record of it
/// (its branch stays). `tidy`: its folder is in the repository's folder
/// in the worktrees location, which goes too if that leaves it empty.
pub fn remove(main: &Path, path: &Path, tidy: bool) -> Result<(), String> {
    let repo = git2::Repository::open(main).map_err(|e| e.message().to_owned())?;
    let wt = find(&repo, path)?;
    let mut opts = git2::WorktreePruneOptions::new();
    opts.valid(true).working_tree(true).locked(false);
    wt.prune(Some(&mut opts)).map_err(|e| e.message().to_owned())?;
    // (libgit2 leaves an emptied folder behind at times.)
    if path.exists() {
        std::fs::remove_dir_all(path).map_err(|e| format!("Can't remove {}: {e}", path.display()))?;
    }
    if tidy && let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir(parent);
    }
    Ok(())
}

// ---- GitHub ---------------------------------------------------------------

/// A pull request's state, as GitHub shows it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PrState {
    Open,
    Draft,
    Merged,
    Closed,
}

/// Its checks, all together.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Checks {
    Passing,
    Failing,
    Pending,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pr {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: PrState,
    pub checks: Option<Checks>,
}

impl Pr {
    /// "#12 open, checks passing", for screen readers and agents.
    pub fn describe(&self) -> String {
        let state = match self.state {
            PrState::Open => "open",
            PrState::Draft => "draft",
            PrState::Merged => "merged",
            PrState::Closed => "closed",
        };
        let checks = match self.checks {
            Some(Checks::Passing) => ", checks passing",
            Some(Checks::Failing) => ", checks failing",
            Some(Checks::Pending) => ", checks running",
            None => "",
        };
        format!("pull request #{} {state}{checks}", self.number)
    }
}

/// A branch on GitHub, asked about in a repository there: `head`'s
/// `branch` (`head` is `origin`'s owner, so a fork's pull request is found
/// in `upstream`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrKey {
    pub owner: String,
    pub repo: String,
    pub head: String,
    pub branch: String,
}

/// Where GitHub's API is (`OLO_GITHUB_API` for tests).
fn api() -> String {
    std::env::var("OLO_GITHUB_API").unwrap_or_else(|_| "https://api.github.com".into()).trim_end_matches('/').to_owned()
}

/// A token for GitHub (`olo_system::github`: Olo's, `GH_TOKEN`, the
/// GitHub CLI's). Looked for once a minute at most (the keyring is a
/// D-Bus call).
fn token() -> Option<String> {
    static KNOWN: Mutex<Option<(Instant, Option<String>)>> = Mutex::new(None);
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, t)) = known.as_ref()
        && at.elapsed() < Duration::from_secs(60)
    {
        return t.clone();
    }
    let t = olo_system::github::token().map(|(t, _)| t);
    *known = Some((Instant::now(), t.clone()));
    SIGNED_IN.store(if t.is_some() { 1 } else { 2 }, std::sync::atomic::Ordering::Relaxed);
    t
}

/// Whether Olo has a GitHub sign-in, as last looked (0: not yet; 1: yes;
/// 2: no), for menus (which can't wait on the keyring).
static SIGNED_IN: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Known to have no GitHub sign-in (Preferences ▸ Integrations connects
/// one).
pub fn signed_out() -> bool {
    SIGNED_IN.load(std::sync::atomic::Ordering::Relaxed) == 2
}

fn agent() -> ureq::Agent {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .user_agent("Olo Terminal")
        .timeout_global(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn get_json(agent: &ureq::Agent, url: &str, token: Option<&str>) -> Result<serde_json::Value, String> {
    let mut req = agent.get(url).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28");
    if let Some(t) = token {
        req = req.header("Authorization", &format!("Bearer {t}"));
    }
    let resp = req.call().map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let body = resp.into_body().into_with_config().limit(4 << 20).read_to_string().map_err(|e| e.to_string())?;
    if status != 200 {
        return Err(format!("GitHub answered {status}"));
    }
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

/// The newest pull request from `key.branch` on GitHub (open ones first),
/// with its checks; None if there is none.
pub fn pull_request(key: &PrKey) -> Result<Option<Pr>, String> {
    let agent = agent();
    let api = api();
    // A token goes only over https (`OLO_GITHUB_API` could be anywhere).
    let token = token().filter(|_| api.starts_with("https://"));
    let base = format!("{api}/repos/{}/{}", encode(&key.owner), encode(&key.repo));
    let head = encode(&format!("{}:{}", key.head, key.branch));
    let list = get_json(&agent, &format!("{base}/pulls?head={head}&state=all&sort=updated&direction=desc&per_page=10"), token.as_deref())?;
    let prs = list.as_array().cloned().unwrap_or_default();
    let pick = prs.iter().find(|p| p["state"] == "open").or_else(|| prs.first());
    let Some(p) = pick else { return Ok(None) };
    let state = match (p["state"].as_str(), p["merged_at"].is_null(), p["draft"].as_bool().unwrap_or(false)) {
        (Some("open"), _, true) => PrState::Draft,
        (Some("open"), _, false) => PrState::Open,
        (_, false, _) => PrState::Merged,
        _ => PrState::Closed,
    };
    let sha = p["head"]["sha"].as_str().unwrap_or_default();
    let checks = if state == PrState::Open || state == PrState::Draft {
        get_json(&agent, &format!("{base}/commits/{sha}/check-runs?per_page=100"), token.as_deref()).ok().and_then(|c| checks_of(&c))
    } else {
        None
    };
    Ok(Some(Pr {
        number: p["number"].as_u64().unwrap_or(0),
        title: p["title"].as_str().unwrap_or_default().to_owned(),
        url: p["html_url"].as_str().unwrap_or_default().to_owned(),
        state,
        checks,
    }))
}

/// `s` for a URL: every byte but letters, digits and `-._~` as `%XX`.
fn encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// The check runs together: failing if any failed, running if any
/// haven't finished, else passing; None without any.
fn checks_of(v: &serde_json::Value) -> Option<Checks> {
    let runs = v["check_runs"].as_array()?;
    if runs.is_empty() {
        return None;
    }
    let failed = runs.iter().any(|r| matches!(r["conclusion"].as_str(), Some("failure" | "timed_out" | "cancelled" | "action_required" | "startup_failure")));
    let running = runs.iter().any(|r| r["status"].as_str() != Some("completed"));
    Some(if failed {
        Checks::Failing
    } else if running {
        Checks::Pending
    } else {
        Checks::Passing
    })
}

// ---- watching tabs ----------------------------------------------------------

/// A change: shell `.0` is now in repository `.1` (or none); a
/// repository's worktrees and branches (by its main one); or the pull
/// request for a branch.
pub enum Report {
    Repo(u64, Option<Repo>),
    Worktrees(PathBuf, Vec<Worktree>, Vec<String>),
    Pr(PrKey, Option<Pr>),
}

/// What the thread last told about a tab: its repository, its folder
/// then, what that was read from, and when.
#[derive(Default)]
struct Seen {
    reported: Option<Option<Repo>>,
    cwd: Option<PathBuf>,
    stamp: Stamp,
    looked: Option<Instant>,
}

/// Looks at the tabs it's given until they go. The list is only held
/// to copy it: the reading is done without it, so a slow disk never holds
/// up a new tab.
#[derive(Clone)]
pub struct Watcher {
    list: Tabs,
}

/// The tabs it looks at: each one's shell and its state.
type Tabs = Arc<Mutex<Vec<(u64, Weak<io::Shared>)>>>;

/// How often GitHub is asked about a branch, with a token and without
/// (60 an hour without one).
const PR_EVERY: Duration = Duration::from_secs(60);
const PR_EVERY_ANON: Duration = Duration::from_secs(300);
/// A folder in no repository is looked at again this often (`git init`).
const NONE_EVERY: Duration = Duration::from_secs(5);

impl Watcher {
    pub fn start(proxy: Proxy) -> Watcher {
        let list: Tabs = Arc::default();
        let w = Watcher { list: list.clone() };
        // GitHub, on a thread of its own: a slow answer mustn't hold up
        // noticing a `cd`.
        let (want_tx, want_rx) = mpsc::channel::<Vec<PrKey>>();
        let pr_proxy = proxy.clone();
        let _ = std::thread::Builder::new().name("github".into()).spawn(move || {
            let mut asked: HashMap<PrKey, Instant> = HashMap::new();
            let mut wanted: Vec<PrKey> = Vec::new();
            loop {
                match want_rx.recv_timeout(Duration::from_secs(10)) {
                    Ok(keys) => wanted = keys,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
                let every = if token().is_some() { PR_EVERY } else { PR_EVERY_ANON };
                asked.retain(|k, _| wanted.contains(k));
                for key in &wanted {
                    if asked.get(key).is_some_and(|t| t.elapsed() < every) {
                        continue;
                    }
                    asked.insert(key.clone(), Instant::now());
                    match pull_request(key) {
                        Ok(pr) => pr_proxy.send(Report::Pr(key.clone(), pr)),
                        Err(e) => tracing::debug!("terminal: GitHub about {}/{} {}: {e}", key.owner, key.repo, key.branch),
                    }
                }
            }
        });
        let _ = std::thread::Builder::new().name("git".into()).spawn(move || {
            let mut last_wanted: Vec<PrKey> = Vec::new();
            // Each repository's worktrees as last told, by its main one.
            let mut trees: HashMap<PathBuf, Stamp> = HashMap::new();
            let mut seen: HashMap<u64, Seen> = HashMap::new();
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let mut wanted: Vec<PrKey> = Vec::new();
                let mut repos: Vec<(PathBuf, PathBuf)> = Vec::new();
                let tabs = {
                    let mut list = list.lock().unwrap();
                    list.retain(|(_, s)| s.strong_count() > 0);
                    list.clone()
                };
                seen.retain(|shell, _| tabs.iter().any(|(s, _)| s == shell));
                for (shell, shared) in &tabs {
                    let Some(shared) = shared.upgrade() else { continue };
                    let w = seen.entry(*shell).or_default();
                    let cwd = shared.foreground().or(shared.shell()).and_then(|pid| std::fs::read_link(format!("/proc/{pid}/cwd")).ok());
                    // Read again only what may have changed.
                    let same = cwd == w.cwd
                        && match &w.reported {
                            Some(Some(r)) => r.stamp() == w.stamp,
                            Some(None) => w.looked.is_some_and(|t| t.elapsed() < NONE_EVERY),
                            None => false,
                        };
                    if !same {
                        let now = cwd.as_deref().and_then(look);
                        w.stamp = now.as_ref().map(Repo::stamp).unwrap_or_default();
                        w.looked = Some(Instant::now());
                        w.cwd = cwd;
                        if w.reported.as_ref() != Some(&now) {
                            w.reported = Some(now.clone());
                            proxy.send(Report::Repo(*shell, now));
                        }
                    }
                    let Some(Some(r)) = &w.reported else { continue };
                    if !repos.iter().any(|(m, _)| *m == r.main) {
                        repos.push((r.main.clone(), r.common.clone()));
                    }
                    if let Some(branch) = &r.branch
                        && let Some((head, _)) = r.github.first()
                    {
                        for (owner, repo) in &r.github {
                            let key = PrKey { owner: owner.clone(), repo: repo.clone(), head: head.clone(), branch: branch.clone() };
                            if !wanted.contains(&key) {
                                wanted.push(key);
                            }
                        }
                    }
                }
                trees.retain(|m, _| repos.iter().any(|(r, _)| r == m));
                for (main, common) in repos {
                    let stamp = worktrees_stamp(&common);
                    if trees.get(&main) != Some(&stamp) {
                        trees.insert(main.clone(), stamp);
                        let (list, names) = (worktrees(&main), branches(&main));
                        proxy.send(Report::Worktrees(main, list, names));
                    }
                }
                if wanted != last_wanted {
                    last_wanted = wanted.clone();
                    if want_tx.send(wanted).is_err() {
                        return;
                    }
                }
            }
        });
        w
    }

    pub fn watch(&self, shell: u64, shared: &Arc<io::Shared>) {
        let mut list = self.list.lock().unwrap();
        if !list.iter().any(|(s, _)| *s == shell) {
            list.push((shell, Arc::downgrade(shared)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes() {
        let gh = |o: &str, r: &str| Some((o.to_owned(), r.to_owned()));
        assert_eq!(github_of("git@github.com:ben/olo.git"), gh("ben", "olo"));
        assert_eq!(github_of("https://github.com/ben/olo"), gh("ben", "olo"));
        assert_eq!(github_of("https://github.com/ben/olo.git/"), gh("ben", "olo"));
        assert_eq!(github_of("ssh://git@github.com/ben/olo.git"), gh("ben", "olo"));
        assert_eq!(github_of("git@gitlab.com:ben/olo.git"), None);
        assert_eq!(github_of("https://github.com/ben"), None);
    }

    #[test]
    fn names() {
        assert!(valid_name("fix-tabs").is_ok());
        assert!(valid_name("").is_err());
        assert!(valid_name("a/b").is_err());
        assert!(valid_name("has space").is_err());
        assert!(valid_name(".hidden").is_err());
        assert!(valid_name("x.lock").is_err());
        assert_eq!(place("/w", "olo", "fix"), PathBuf::from("/w/olo/fix"));
    }

    #[test]
    fn encoded() {
        assert_eq!(encode("ben:fix/tabs+more&x"), "ben%3Afix%2Ftabs%2Bmore%26x");
        assert_eq!(encode("plain-name_1.2~"), "plain-name_1.2~");
    }

    #[test]
    fn worktrees_made_renamed_and_removed() {
        let dir = std::env::temp_dir().join(format!("olo-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let main = dir.join("proj");
        std::fs::create_dir_all(&main).unwrap();
        let repo = git2::Repository::init(&main).unwrap();
        std::fs::write(main.join("a.txt"), "hello\n").unwrap();
        // More than a batch, in folders.
        let mut index = repo.index().unwrap();
        for i in 0..850 {
            let f = format!("many/{}/f{i}.txt", i % 7);
            std::fs::create_dir_all(main.join(&f).parent().unwrap()).unwrap();
            std::fs::write(main.join(&f), format!("{i}\n")).unwrap();
            index.add_path(Path::new(&f)).unwrap();
        }
        index.add_path(Path::new("a.txt")).unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("Olo", "olo@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "first", &tree, &[]).unwrap();

        let r = look(&main.join(".")).unwrap();
        assert_eq!((r.name.as_str(), r.linked), ("proj", false));
        // Made: on a new branch of its name, with the files.
        let at = place(dir.join("worktrees").to_str().unwrap(), "proj", "fix");
        let progress = Progress::default();
        assert_eq!(add(&main, "fix", &at, &progress).unwrap(), at);
        assert_eq!((progress.done.load(Ordering::Relaxed), progress.total.load(Ordering::Relaxed)), (851, 851));
        let count = |d: &Path| (0..7).map(|k| std::fs::read_dir(d.join(format!("many/{k}"))).map_or(0, |r| r.count())).sum::<usize>();
        assert_eq!(count(&at), 850);
        assert!(at.join("many/2/f849.txt").is_file());
        // Its branch is out: not again, and nothing left of the try.
        let again = place(dir.join("worktrees").to_str().unwrap(), "proj", "fix-again");
        assert!(add(&main, "fix", &again, &Progress::default()).is_err());
        assert!(!again.exists());
        // Stopped: nothing left of it, nor its new branch.
        let stop = Progress::default();
        stop.cancel.store(true, Ordering::Relaxed);
        let stopped = dir.join("worktrees/proj/stopped");
        assert!(add(&main, "stopped", &stopped, &stop).is_err());
        assert!(!stopped.exists() && repo.find_branch("stopped", git2::BranchType::Local).is_err());
        // The main worktree's branch is out there.
        let head = repo.head().unwrap().shorthand().unwrap().to_owned();
        let e = add(&main, &head, &dir.join("worktrees/proj/h"), &Progress::default()).unwrap_err();
        assert!(e.contains("is checked out in"), "{e}");
        assert!(at.join("a.txt").is_file());
        let r = look(&at).unwrap();
        assert_eq!((r.name.as_str(), r.linked, r.branch.as_deref()), ("proj", true, Some("fix")));
        assert_eq!(tidy(&r.main), tidy(&main.canonicalize().unwrap()));
        let all = worktrees(&r.main);
        assert_eq!(all.iter().map(|w| (w.name.as_str(), w.main)).collect::<Vec<_>>(), [("proj", true), ("fix", false)]);
        // Changes counted.
        assert_eq!(changes(&at), 0);
        std::fs::write(at.join("b.txt"), "new\n").unwrap();
        assert_eq!(changes(&at), 1);
        // Renamed: folder and branch.
        let to = rename(&r.main, &at, "fix2").unwrap();
        assert!(to.join("a.txt").is_file() && !at.exists());
        let r2 = look(&to).unwrap();
        assert_eq!((r2.linked, r2.branch.as_deref()), (true, Some("fix2")));
        assert_eq!(worktrees(&r2.main).len(), 2);
        // Removed: folder and record (the branch stays).
        assert!(branches(&r2.main).contains(&"fix2".to_owned()));
        remove(&r2.main, &to, true).unwrap();
        assert!(!to.exists() && !to.parent().unwrap().exists());
        assert_eq!(worktrees(&r2.main).len(), 1);
        assert!(repo.find_branch("fix2", git2::BranchType::Local).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checks() {
        let v = |s: &str| serde_json::from_str::<serde_json::Value>(s).unwrap();
        assert_eq!(checks_of(&v(r#"{"check_runs":[]}"#)), None);
        assert_eq!(checks_of(&v(r#"{"check_runs":[{"status":"completed","conclusion":"success"}]}"#)), Some(Checks::Passing));
        assert_eq!(checks_of(&v(r#"{"check_runs":[{"status":"in_progress","conclusion":null},{"status":"completed","conclusion":"success"}]}"#)), Some(Checks::Pending));
        assert_eq!(checks_of(&v(r#"{"check_runs":[{"status":"in_progress"},{"status":"completed","conclusion":"failure"}]}"#)), Some(Checks::Failing));
    }
}
