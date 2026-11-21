//! Native in-process Git repository & worktree inspection (`libgit2` via `git2`)
//! for local Rex blocks, running on a lightweight background worker thread so
//! the UI never touches disk on frame boundaries.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use olo_sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitInfo {
    pub repo_name: String,
    pub branch: Option<String>,
    pub dirty_count: usize,
    pub worktree_root: PathBuf,
    pub is_linked_worktree: bool,
    pub github_slug: Option<(String, String)>,
}

/// Cached repository status per working directory path.
#[derive(Default)]
pub struct GitCache {
    entries: Mutex<HashMap<PathBuf, (Instant, Option<GitInfo>)>>,
}

impl GitCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Return cached git metadata if fresh (< 3 s), else trigger a background
    /// refresh and return any previously cached value immediately (zero UI blocking).
    pub fn get_or_refresh(self: &Arc<Self>, dir: &Path, on_update: impl FnOnce() + Send + 'static) -> Option<GitInfo> {
        let key = dir.to_path_buf();
        let mut guard = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, info)) = guard.get(&key) {
            if at.elapsed() < Duration::from_secs(3) {
                return info.clone();
            }
            let cached = info.clone();
            // Bump timestamp immediately so we don't spawn duplicate refreshes.
            guard.insert(key.clone(), (Instant::now(), cached.clone()));
            drop(guard);
            self.spawn_probe(key, on_update);
            return cached;
        }
        guard.insert(key.clone(), (Instant::now(), None));
        drop(guard);
        self.spawn_probe(key, on_update);
        None
    }

    fn spawn_probe(self: &Arc<Self>, dir: PathBuf, on_update: impl FnOnce() + Send + 'static) {
        let cache = self.clone();
        let _ = std::thread::Builder::new().name("rex-git".into()).spawn(move || {
            let info = inspect_repo(&dir);
            let changed = {
                let mut guard = cache.entries.lock().unwrap_or_else(|e| e.into_inner());
                let prev = guard.get(&dir).and_then(|(_, i)| i.clone());
                guard.insert(dir, (Instant::now(), info.clone()));
                prev != info
            };
            if changed {
                on_update();
            }
        });
    }
}

pub fn inspect_repo(dir: &Path) -> Option<GitInfo> {
    if !dir.exists() {
        return None;
    }
    let repo = git2::Repository::discover(dir).ok()?;
    let workdir = repo.workdir()?.to_path_buf();
    let common = repo.commondir().canonicalize().unwrap_or_else(|_| repo.commondir().to_path_buf());
    let main_root = match common.file_name().and_then(|n| n.to_str()) {
        Some(".git") => common.parent().map(Path::to_path_buf).unwrap_or(workdir.clone()),
        _ => workdir.clone(),
    };
    let repo_name = main_root.file_name().map(|n| n.to_string_lossy().trim_end_matches(".git").to_owned()).unwrap_or_default();
    let branch = repo.head().ok().and_then(|h| if h.is_branch() { h.shorthand().map(str::to_owned) } else { h.target().map(|oid| format!("@{}", &oid.to_string()[..7])) });

    let mut status_opts = git2::StatusOptions::new();
    status_opts.include_untracked(true).recurse_untracked_dirs(false).include_ignored(false);
    let dirty_count = repo.statuses(Some(&mut status_opts)).map(|s| s.len()).unwrap_or(0);

    let github_slug = ["origin", "upstream"].iter().find_map(|r| repo.find_remote(r).ok()?.url().and_then(parse_github_slug));

    Some(GitInfo { repo_name, branch, dirty_count, worktree_root: workdir, is_linked_worktree: repo.is_worktree(), github_slug })
}

fn parse_github_slug(url: &str) -> Option<(String, String)> {
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
        .or_else(|| url.strip_prefix("https://github.com/"))
        .or_else(|| url.strip_prefix("http://github.com/"))
        .or_else(|| url.strip_prefix("git://github.com/"))?;
    let clean = rest.trim_end_matches('/').trim_end_matches(".git");
    let (owner, repo) = clean.split_once('/')?;
    (!owner.is_empty() && !repo.is_empty() && !repo.contains('/')).then(|| (owner.to_owned(), repo.to_owned()))
}
