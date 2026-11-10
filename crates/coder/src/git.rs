//! Native repository operations. Never invokes Git, hooks, credential helpers,
//! diff drivers or shell filters. Index transactions use Git's own lock protocol.
use crate::{model::ToolCall, tools};
use anyhow::{Context, Result, bail, ensure};
use gix::bstr::ByteSlice;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn open(root: &Path) -> Result<gix::Repository> {
    // Untrusted repository configuration must never turn a read into a process.
    let repo = gix::discover_opts(root, Default::default(), gix::open::Options::default().filter_config_section(|_| false).config_overrides(["core.fsmonitor=false"]))?;
    ensure!(repo.workdir().context("bare repositories have no workspace")?.canonicalize()? == root.canonicalize()?, "open the repository root as the workspace before using Git operations");
    Ok(repo)
}
fn relative(name: &str) -> Result<&str> {
    ensure!(!name.is_empty() && Path::new(name).components().all(|c| matches!(c, std::path::Component::Normal(v) if v != ".git")), "expected a relative repository file path");
    Ok(name)
}
fn blob(repo: &gix::Repository, id: gix::ObjectId) -> Result<Vec<u8>> {
    let object = repo.find_object(id)?;
    ensure!(object.kind == gix::object::Kind::Blob, "not a regular file blob");
    ensure!(object.data.len() <= 8 * 1024 * 1024, "file exceeds 8 MB preview limit");
    Ok(object.data.clone())
}
struct IndexLock {
    path: Option<PathBuf>,
    file: std::fs::File,
}
impl IndexLock {
    fn acquire(repo: &gix::Repository) -> Result<Self> {
        let path = repo.index_path().with_extension("lock");
        let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).context("repository index is busy; another Git operation owns index.lock")?;
        Ok(Self { path: Some(path), file })
    }
    fn save(&mut self, repo: &gix::Repository, index: &mut gix::index::File) -> Result<()> {
        index.remove_tree();
        index.write_to(&mut self.file, Default::default())?;
        self.file.sync_all()?;
        std::fs::rename(self.path.as_ref().context("index transaction already finished")?, repo.index_path())?;
        self.path = None;
        std::fs::File::open(repo.git_dir())?.sync_all()?;
        Ok(())
    }
}
impl Drop for IndexLock {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub fn run(root: &Path, call: &ToolCall, cancel: Arc<AtomicBool>) -> Result<Value> {
    ensure!(!cancel.load(Ordering::Relaxed), "cancelled");
    let a = &call.arguments;
    let op = tools::string(a, "operation")?;
    if op == "init" {
        ensure!(call.name == "repository_write", "initialization requires repository_write");
        ensure!(gix::discover(root).is_err(), "workspace is already inside a repository");
        gix::init(root)?;
        return Ok(json!({"initialized":root}));
    }
    let repo = open(root)?;
    ensure!(call.name != "repository_read" || matches!(op, "status" | "diff" | "log" | "branches" | "worktrees"), "mutation requires repository_write");
    match op {
        "status" => {
            let mut files = Vec::new();
            for item in repo.status(gix::progress::Discard)?.untracked_files(gix::status::UntrackedFiles::Files).should_interrupt_owned(cancel).into_iter([])? {
                let item = item?;
                let staged = matches!(item, gix::status::Item::TreeIndex(_));
                let state = match &item {
                    gix::status::Item::IndexWorktree(change) => format!("{:?}", change.summary()),
                    gix::status::Item::TreeIndex(change) => format!("{:?}", change).split_whitespace().next().unwrap_or("changed").into(),
                };
                files.push(json!({"path":item.location().to_str_lossy(),"staged":staged,"state":state}));
                if files.len() >= 2000 {
                    break;
                }
            }
            files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()).then(a["staged"].as_bool().cmp(&b["staged"].as_bool())));
            let truncated = files.len() >= 2000;
            Ok(
                json!({"truncated":truncated,"branch":repo.head_name()?.map(|n|n.shorten().to_str_lossy().into_owned()),"head":repo.head_id().ok().map(|id|id.to_string()),"files":files,"github":github_repository(&repo)}),
            )
        }
        "diff" => {
            let name = relative(tools::string(a, "path")?)?;
            let index = repo.index_or_empty()?;
            let staged = a["staged"].as_bool().unwrap_or(false);
            let indexed = index.entry_by_path(name.as_bytes().as_bstr()).map(|e| blob(&repo, e.id)).transpose()?.unwrap_or_default();
            let (old, new) = if staged {
                let old = match repo.head_tree().ok().map(|t| t.lookup_entry_by_path(name)).transpose()?.flatten() {
                    Some(e) => blob(&repo, e.object_id())?,
                    None => Vec::new(),
                };
                (old, indexed)
            } else {
                let target = root.join(name);
                let new = if target.exists() {
                    let safe = tools::path(root, name, false)?;
                    ensure!(std::fs::metadata(&safe)?.len() <= 8 * 1024 * 1024, "file exceeds 8 MB preview limit");
                    std::fs::read(safe)?
                } else {
                    Vec::new()
                };
                (indexed, new)
            };
            if old.contains(&0) || new.contains(&0) {
                return Ok(json!({"path":name,"binary":true,"old_bytes":old.len(),"new_bytes":new.len()}));
            }
            let old = String::from_utf8_lossy(&old);
            let new = String::from_utf8_lossy(&new);
            let diff = similar::TextDiff::from_lines(old.as_ref(), new.as_ref()).unified_diff().context_radius(3).header(&format!("a/{name}"), &format!("b/{name}")).to_string();
            Ok(json!({"path":name,"staged":staged,"diff":tools::clip(&diff,128000)}))
        }
        "log" => {
            let mut commits = Vec::new();
            for info in repo.head_id()?.ancestors().all()?.take(a["limit"].as_u64().unwrap_or(30).min(200) as usize) {
                ensure!(!cancel.load(Ordering::Relaxed), "cancelled");
                let commit = info?.object()?;
                let decoded = commit.decode()?;
                commits.push(json!({"id":commit.id.to_string(),"message":decoded.message.to_str_lossy(),"author":decoded.author()?.name.to_str_lossy()}));
            }
            Ok(json!({"commits":commits}))
        }
        "branches" => {
            let mut branches = Vec::new();
            for r in repo.references()?.local_branches()? {
                let r = r?;
                branches.push(json!({"name":r.name().shorten().to_str_lossy(),"target":format!("{:?}",r.target())}));
            }
            Ok(json!({"branches":branches}))
        }
        "worktrees" => {
            let mut worktrees = Vec::new();
            for r in repo.worktrees_including_main()? {
                let r = r?;
                worktrees.push(json!({"path":r.workdir(),"branch":r.head_name()?.map(|n|n.shorten().to_str_lossy().into_owned()),"head":r.head_id().ok().map(|id|id.to_string()),"current":r.git_dir()==repo.git_dir()}));
            }
            Ok(json!({"worktrees":worktrees}))
        }
        "stage" | "unstage" => {
            let paths = a["paths"].as_array().context("paths must be an explicit array of files")?;
            ensure!(!paths.is_empty() && paths.len() <= 2000, "select between 1 and 2000 files");
            let mut lock = IndexLock::acquire(&repo)?;
            let mut index = if repo.index_path().exists() { repo.open_index()? } else { repo.index_from_tree(&repo.object_hash().empty_tree())? };
            let head = repo.head_tree().ok();
            for path in paths {
                ensure!(!cancel.load(Ordering::Relaxed), "cancelled");
                let name = relative(path.as_str().context("path must be text")?)?;
                let entry = if op == "unstage" {
                    head.as_ref().map(|t| t.lookup_entry_by_path(name)).transpose()?.flatten().map(|e| (e.object_id(), gix::index::entry::Mode::from_bits_truncate(e.mode().value() as u32)))
                } else if root.join(name).exists() {
                    let safe = tools::path(root, name, false)?;
                    let meta = std::fs::symlink_metadata(root.join(name))?;
                    ensure!(meta.is_file(), "stage regular files individually; symlinks and directories need explicit handling");
                    ensure!(meta.len() <= 32 * 1024 * 1024, "file exceeds 32 MB stage limit");
                    use std::os::unix::fs::PermissionsExt;
                    let mode = if meta.permissions().mode() & 0o111 != 0 { gix::index::entry::Mode::FILE_EXECUTABLE } else { gix::index::entry::Mode::FILE };
                    let mut attrs = repo.attributes_only(&index, gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping)?;
                    let mut matched = attrs.selected_attribute_matches(["filter"]);
                    attrs.at_path(name, None)?.matching_attributes(&mut matched);
                    let filter = matched.iter_selected().find_map(|m| m.assignment.state.as_bstr().map(|v| v.to_string()));
                    let id = if let Some(filter) = filter {
                        ensure!(filter == "lfs", "file uses the custom Git filter {filter}; native staging cannot run external filter programs");
                        use sha2::{Digest, Sha256};
                        let bytes = std::fs::read(&safe)?;
                        // Existing LFS pointers are already in canonical Git form.
                        if bytes.starts_with(b"version https://git-lfs.github.com/spec/v1\n") && bytes.len() < 1024 {
                            repo.write_blob(&bytes)?.detach()
                        } else {
                            let hash = format!("{:x}", Sha256::digest(&bytes));
                            let object = repo.common_dir().join("lfs/objects").join(&hash[..2]).join(&hash[2..4]).join(&hash);
                            std::fs::create_dir_all(object.parent().unwrap())?;
                            if !object.exists() {
                                tools::atomic_write(&object, &bytes)?;
                            }
                            repo.write_blob(format!("version https://git-lfs.github.com/spec/v1\noid sha256:{hash}\nsize {}\n", bytes.len()))?.detach()
                        }
                    } else {
                        let (mut pipeline, filter_index) = repo.filter_pipeline(None)?;
                        let converted = pipeline.convert_to_git(std::fs::File::open(&safe)?, Path::new(name), &filter_index)?;
                        repo.write_blob_stream(converted)?.detach()
                    };
                    let after = std::fs::metadata(&safe)?;
                    ensure!(after.len() == meta.len() && after.modified()? == meta.modified()?, "file changed while staging; retry after reviewing it");
                    Some((id, mode))
                } else {
                    None
                };
                index.remove_entries(|_, p, _| p == name.as_bytes().as_bstr());
                if let Some((id, mode)) = entry {
                    index.dangerously_push_entry(Default::default(), id, gix::index::entry::Flags::empty(), mode, name.as_bytes().as_bstr());
                }
            }
            index.sort_entries();
            lock.save(&repo, &mut index)?;
            Ok(json!({"operation":op,"paths":paths}))
        }
        "commit" => {
            let _lock = IndexLock::acquire(&repo)?;
            let message = tools::string(a, "message")?;
            ensure!(!message.trim().is_empty(), "commit message is empty");
            let expected = tools::string(a, "expected_head")?;
            let head = repo.head_id().ok().map(|id| id.detach());
            ensure!(head.map(|id| id.to_string()).unwrap_or_default() == expected, "HEAD changed; refresh before committing");
            let index = repo.index_or_empty()?;
            let mut tree = repo.edit_tree(repo.object_hash().empty_tree())?;
            for entry in index.entries() {
                ensure!(entry.stage() == gix::index::entry::Stage::Unconflicted, "resolve index conflicts before committing");
                let kind = gix::objs::tree::EntryMode::try_from(entry.mode.bits()).map_err(|_| anyhow::anyhow!("invalid index mode"))?.kind();
                tree.upsert(entry.path(&index), kind, entry.id)?;
            }
            let tree = tree.write()?.detach();
            ensure!(repo.head_tree_id().ok().map(|id| id.detach()) != Some(tree), "no staged changes");
            ensure!(!cancel.load(Ordering::Relaxed), "cancelled");
            let id = repo.commit("HEAD", message, tree, head)?;
            Ok(json!({"commit":id.to_string(),"message":message}))
        }
        "branch" => {
            let branch = tools::string(a, "branch")?;
            let name = format!("refs/heads/{branch}");
            repo.reference(name, repo.head_id()?.detach(), gix::refs::transaction::PreviousValue::MustNotExist, "Olo Coder: create branch")?;
            Ok(json!({"branch":branch}))
        }
        "worktree_add" => {
            let name = relative(tools::string(a, "path")?)?;
            // Workspace-local destinations preserve the session's write boundary.
            let destination = tools::path(root, name, true)?;
            ensure!(!destination.exists(), "worktree destination already exists");
            let branch = tools::string(a, "branch")?;
            let full = format!("refs/heads/{branch}");
            let head = gix::worktree::add::Head::Attached(full.try_into()?);
            let (new, outcome) = repo.add_worktree(&destination, head, gix::progress::Discard, &cancel)?;
            Ok(json!({"path":new.workdir(),"branch":branch,"checkout":format!("{outcome:?}")}))
        }
        _ => bail!("unknown Git operation {op}"),
    }
}

fn github_repository(repo: &gix::Repository) -> Option<String> {
    let remote = repo.find_remote("origin").ok()?;
    let url = remote.url(gix::remote::Direction::Fetch)?;
    if url.host.as_deref() != Some("github.com") {
        return None;
    }
    let path = url.path.to_str().ok()?.trim_matches('/').trim_end_matches(".git");
    let parts = path.split('/').collect::<Vec<_>>();
    (parts.len() == 2 && parts.iter().all(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c)))).then(|| path.to_string())
}
