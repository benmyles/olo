//! Files kept in step with a host (HACKING, Rex, "Files kept in step"): a
//! session of the engine (`olo-rex-sync`) between this computer and Olo's
//! helper on the host (`olo-rex-link sync`), over the SSH connection the
//! host is reached by (`SshSessionPool::exec_io`). The engine works with
//! plain `Read` and `Write`: it runs on a blocking thread of its own,
//! driving the channel's stream from there ([`Carried`]), which counts what
//! goes each way (for a progress bar) and stops at a word ([`Stop`]).

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use olo_rex_sync::index::{Filter, Index};
use olo_rex_sync::twoway::{Conflict, apply_mutations, decode_index_frame, plan as plan_twoway, post_sync_baseline};
use olo_rex_sync::wire::{FolderSource, Options, PathError, Report, Source};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::RexError;
use crate::provision::shell_word;
use crate::ssh::{SshAskpass, SshSessionPool, SshTarget};
use crate::transport::AsyncReadWrite;

/// A folder on the host, and how what goes into it is kept: its path
/// (under the home, `$HOME`, unless whole), what is taken away (`delete`: a mirror),
/// what is left (`keep_newer`), what is passed over; `scoped`: only the
/// files sent are looked at there (the home itself, say), nothing taken
/// away.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostFolder {
    pub path: String,
    pub delete: bool,
    pub keep_newer: bool,
    pub exclude: Vec<String>,
    pub shallow: bool,
    pub scoped: bool,
}

impl HostFolder {
    /// The helper's command on the host for `side` ("receive" or "send").
    fn command(&self, helper_id: &str, side: &str) -> String {
        let mut c = format!("{} sync {side} {}", crate::helper::file(helper_id), shell_word(&self.path));
        if self.delete {
            c.push_str(" --delete");
        }
        if self.keep_newer {
            c.push_str(" --keep-newer");
        }
        if self.shallow {
            c.push_str(" --shallow");
        }
        if self.scoped {
            c.push_str(" --scoped");
        }
        for p in &self.exclude {
            c.push_str(" --exclude ");
            c.push_str(&shell_word(p));
        }
        c
    }

    /// The helper's command to scan and stream the host folder's index.
    fn index_command(&self, helper_id: &str, create: bool) -> String {
        let mut c = format!("{} sync index {}", crate::helper::file(helper_id), shell_word(&self.path));
        if create {
            c.push_str(" --create");
        }
        if self.shallow {
            c.push_str(" --shallow");
        }
        for p in &self.exclude {
            c.push_str(" --exclude ");
            c.push_str(&shell_word(p));
        }
        c
    }

    /// The helper's watch command on the host matching this folder's filter.
    pub fn watch_command(&self, helper_id: &str) -> String {
        let mut c = format!("{} sync watch {}", crate::helper::file(helper_id), shell_word(&self.path));
        if self.shallow {
            c.push_str(" --shallow");
        }
        for p in &self.exclude {
            c.push_str(" --exclude ");
            c.push_str(&shell_word(p));
        }
        c
    }

    /// The engine's options on this side, as the folder says them.
    pub fn options(&self) -> Options {
        Options {
            delete_extra: self.delete,
            keep_newer: self.keep_newer,
            filter: Filter { recursive: !self.shallow, exclude: self.exclude.clone(), ..Default::default() },
            cache_path: None,
            scoped: self.scoped,
            // (This computer's side keeps nothing of its own from a host's
            // files: what it receives is the person's to have.)
            protect: Vec::new(),
            progress: None,
        }
    }
}

/// How far a session is, and a word to stop it: shared with whoever shows
/// it (bytes each way as the channel carries them).
#[derive(Clone, Debug, Default)]
pub struct Stop {
    pub stop: Arc<AtomicBool>,
    pub sent: Arc<AtomicU64>,
    pub received: Arc<AtomicU64>,
}

/// Summary of one two-way reconciliation pass ([`sync_both`]).
#[derive(Clone, Debug, Default)]
pub struct TwoWayReport {
    /// Total regular files present after syncing.
    pub files: u64,
    /// Files/symlinks pulled from host to desktop.
    pub pulled: u64,
    /// Files/symlinks pushed from desktop to host.
    pub pushed: u64,
    /// Paths deleted on desktop.
    pub removed_local: u64,
    /// Paths deleted on host.
    pub removed_remote: u64,
    /// Total wire bytes sent to the host.
    pub bytes_sent: u64,
    /// Total wire bytes received from the host.
    pub bytes_received: u64,
    /// Conflicts preserved side by side in this pass.
    pub conflicts: Vec<Conflict>,
    /// All conflict copy paths present in the synced tree after this pass.
    pub all_conflicts: Vec<String>,
    /// Per-path transfer errors (baseline is NOT advanced when non-empty).
    pub errors: Vec<PathError>,
}

impl TwoWayReport {
    pub fn wire_bytes(&self) -> u64 {
        self.bytes_sent + self.bytes_received
    }

    pub fn changed(&self) -> bool {
        self.pulled > 0 || self.pushed > 0 || self.removed_local > 0 || self.removed_remote > 0 || !self.conflicts.is_empty()
    }
}

/// The channel's stream as the engine takes it: blocking reads and writes
/// on this thread, driven on the runtime; refused once stopped.
struct Carried {
    stream: Pin<Box<dyn AsyncReadWrite + Send>>,
    rt: tokio::runtime::Handle,
    stop: Stop,
}

impl Carried {
    fn stopped(&self) -> io::Result<()> {
        if self.stop.stop.load(Ordering::Relaxed) { Err(io::Error::new(io::ErrorKind::Interrupted, "stopped")) } else { Ok(()) }
    }
}

impl Read for Carried {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stopped()?;
        let (stream, rt) = (&mut self.stream, &self.rt);
        let n = rt.block_on(stream.read(buf))?;
        self.stop.received.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

impl Write for Carried {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stopped()?;
        let (stream, rt) = (&mut self.stream, &self.rt);
        let n = rt.block_on(stream.write(buf))?;
        self.stop.sent.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        let (stream, rt) = (&mut self.stream, &self.rt);
        rt.block_on(stream.flush())
    }
}

fn failed(e: io::Error, what: &str) -> RexError {
    // (The helper not there, or built without keeping files in step:
    // its channel ends before the session's first answer.)
    if e.kind() == io::ErrorKind::UnexpectedEof {
        return RexError::Protocol(format!("{what}: the host's helper ended the session (is it there, and built to keep files in step?)"));
    }
    RexError::Io(e)
}

/// What `source` has here, sent into `there` on the host (the host's
/// helper receives).
pub async fn send_to(
    pool: &SshSessionPool,
    target: &SshTarget,
    askpass: Option<Arc<dyn SshAskpass>>,
    helper_id: &str,
    source: Box<dyn Source + Send>,
    there: &HostFolder,
    stop: Stop,
) -> Result<Report, RexError> {
    let stream = pool.exec_io(target, askpass, &there.command(helper_id, "receive")).await?;
    let (rt, options) = (tokio::runtime::Handle::current(), there.options());
    let ran = tokio::task::spawn_blocking(move || olo_rex_sync::wire::send(Carried { stream, rt, stop }, &*source, options)).await;
    ran.map_err(|e| RexError::Protocol(format!("sending to {}: {e}", target.display())))?.map_err(|e| failed(e, &format!("sending to {}", target.display())))
}

/// The host's folder `there` received into `here` on this computer (the
/// host's helper sends). What `there` says to delete or keep is this
/// side's to do, as the receiver.
pub async fn receive_from(
    pool: &SshSessionPool,
    target: &SshTarget,
    askpass: Option<Arc<dyn SshAskpass>>,
    helper_id: &str,
    there: &HostFolder,
    here: PathBuf,
    cache: Option<PathBuf>,
    stop: Stop,
) -> Result<Report, RexError> {
    let sending = HostFolder { delete: false, keep_newer: false, ..there.clone() };
    let stream = pool.exec_io(target, askpass, &sending.command(helper_id, "send")).await?;
    let rt = tokio::runtime::Handle::current();
    let options = Options { cache_path: cache, ..there.options() };
    let ran = tokio::task::spawn_blocking(move || olo_rex_sync::wire::receive(Carried { stream, rt, stop }, &here, options)).await;
    ran.map_err(|e| RexError::Protocol(format!("receiving from {}: {e}", target.display())))?.map_err(|e| failed(e, &format!("receiving from {}", target.display())))
}

/// Fetch the host folder's current [`Index`] (compressed, without chunk lists)
/// using `olo-rex-link sync index`. Returns `(index, wire_bytes)`.
pub async fn fetch_index(pool: &SshSessionPool, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>, helper_id: &str, there: &HostFolder, create: bool) -> Result<(Index, u64), RexError> {
    let mut stream = pool.exec_io(target, askpass, &there.index_command(helper_id, create)).await?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.map_err(RexError::Io)?;
    let n = raw.len() as u64;
    if raw.is_empty() {
        return Err(RexError::Protocol(format!("indexing {}: the host's helper returned nothing (missing folder or older helper)", there.path)));
    }
    let idx = tokio::task::spawn_blocking(move || decode_index_frame(&raw[..]))
        .await
        .map_err(|e| RexError::Protocol(format!("decoding index: {e}")))?
        .map_err(|e| failed(e, &format!("indexing {}", there.path)))?;
    Ok((idx, n))
}

/// Run `olo-rex-link sync mutate ROOT` on the host to copy conflict losers,
/// remove deleted entries, and create/chmod directories atomically.
pub async fn mutate_host(
    pool: &SshSessionPool,
    target: &SshTarget,
    askpass: Option<Arc<dyn SshAskpass>>,
    helper_id: &str,
    path: &str,
    copy: &[(String, String)],
    remove: &[String],
    mkdir: &[(String, u32)],
) -> Result<u64, RexError> {
    if copy.is_empty() && remove.is_empty() && mkdir.is_empty() {
        return Ok(0);
    }
    let cmd = format!("{} sync mutate {}", crate::helper::file(helper_id), shell_word(path));
    let payload = serde_json::to_vec(&serde_json::json!({ "copy": copy, "remove": remove, "mkdir": mkdir })).map_err(|e| RexError::Protocol(format!("serialize mutate: {e}")))?;
    let mut stream = pool.exec_io(target, askpass, &cmd).await?;
    stream.write_all(&payload).await.map_err(RexError::Io)?;
    stream.shutdown().await.map_err(RexError::Io)?;
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).await.map_err(RexError::Io)?;
    let ok = serde_json::from_slice::<serde_json::Value>(&reply).ok().and_then(|v| v.get("ok").and_then(|b| b.as_bool())).unwrap_or(false);
    if !ok {
        let said = String::from_utf8_lossy(&reply).trim().to_owned();
        return Err(RexError::Protocol(format!("mutating {path}: {}", if said.is_empty() { "helper failed".into() } else { said })));
    }
    Ok((payload.len() + reply.len()) as u64)
}

/// Pull an explicit set of relative paths from host `there` into `here` via
/// scoped `olo-rex-sync` transfer without touching any other files or caches.
pub async fn receive_paths(
    pool: &SshSessionPool,
    target: &SshTarget,
    askpass: Option<Arc<dyn SshAskpass>>,
    helper_id: &str,
    there_path: &str,
    here: PathBuf,
    paths: &[String],
    stop: Stop,
) -> Result<Report, RexError> {
    if paths.is_empty() {
        return Ok(Report::default());
    }
    let mut cmd = format!("{} sync send {} --no-cache", crate::helper::file(helper_id), shell_word(there_path));
    let inline_bytes: usize = paths.iter().map(|p| p.len() + 12).sum();
    if paths.len() <= 64 && inline_bytes < 8192 {
        for p in paths {
            cmd.push_str(" --only ");
            cmd.push_str(&shell_word(p));
        }
    } else {
        // Large pull set: stage NUL-separated path list in a temp file on the
        // host that `olo-rex-link sync send --only-file` reads and unlinks.
        let mut stage = pool.exec_io(target, askpass.clone(), "tmp=$(mktemp /var/tmp/olo-sync-only.XXXXXX) || exit 1; cat > \"$tmp\" && printf '%s\\n' \"$tmp\"").await?;
        let mut buf = Vec::with_capacity(paths.iter().map(|p| p.len() + 1).sum());
        for p in paths {
            buf.extend_from_slice(p.as_bytes());
            buf.push(0);
        }
        stage.write_all(&buf).await.map_err(RexError::Io)?;
        stage.shutdown().await.map_err(RexError::Io)?;
        let mut out = String::new();
        stage.read_to_string(&mut out).await.map_err(RexError::Io)?;
        let tmp = out.trim();
        if tmp.is_empty() {
            return Err(RexError::Protocol("couldn't stage --only-file on host".into()));
        }
        cmd.push_str(" --only-file ");
        cmd.push_str(&shell_word(tmp));
    }
    let stream = pool.exec_io(target, askpass, &cmd).await?;
    let rt = tokio::runtime::Handle::current();
    let options = Options { scoped: true, cache_path: None, ..Default::default() };
    let ran = tokio::task::spawn_blocking(move || olo_rex_sync::wire::receive(Carried { stream, rt, stop }, &here, options)).await;
    ran.map_err(|e| RexError::Protocol(format!("pulling from {}: {e}", target.display())))?.map_err(|e| failed(e, &format!("pulling from {}", target.display())))
}

/// Push an explicit set of relative paths from `here` into host `there_path`
/// via scoped `olo-rex-sync` transfer without touching any other files or caches.
pub async fn send_paths(
    pool: &SshSessionPool,
    target: &SshTarget,
    askpass: Option<Arc<dyn SshAskpass>>,
    helper_id: &str,
    there_path: &str,
    here: PathBuf,
    paths: Vec<String>,
    stop: Stop,
) -> Result<Report, RexError> {
    if paths.is_empty() {
        return Ok(Report::default());
    }
    let cmd = format!("{} sync receive {} --scoped --no-cache", crate::helper::file(helper_id), shell_word(there_path));
    let stream = pool.exec_io(target, askpass, &cmd).await?;
    let rt = tokio::runtime::Handle::current();
    let source = FolderSource { root: here, filter: Filter { only: paths, ..Default::default() }, cache_path: None };
    let options = Options { scoped: true, ..Default::default() };
    let ran = tokio::task::spawn_blocking(move || olo_rex_sync::wire::send(Carried { stream, rt, stop }, &source, options)).await;
    ran.map_err(|e| RexError::Protocol(format!("pushing to {}: {e}", target.display())))?.map_err(|e| failed(e, &format!("pushing to {}", target.display())))
}

/// One complete 3-way reconciliation between `here` (desktop) and `there`
/// (host): scans both sides in parallel, preserves any conflicts side by side
/// without data loss, transfers only what changed, and advances `baseline_path`
/// once both sides converge cleanly.
#[allow(clippy::too_many_arguments)]
pub async fn sync_both(
    pool: &SshSessionPool,
    target: &SshTarget,
    askpass: Option<Arc<dyn SshAskpass>>,
    helper_id: &str,
    host_label: &str,
    there: &HostFolder,
    here: PathBuf,
    local_cache: PathBuf,
    baseline_path: PathBuf,
    create_remote: bool,
    stop: Stop,
    on_work: Option<Box<dyn FnOnce() + Send>>,
) -> Result<TwoWayReport, RexError> {
    let filter = there.options().filter;
    let here_scan = here.clone();
    let cache_scan = local_cache.clone();
    let filter_scan = filter.clone();
    let local_fut = async move {
        tokio::task::spawn_blocking(move || -> io::Result<Index> {
            std::fs::create_dir_all(&here_scan)?;
            let prev = Index::load(&cache_scan).ok();
            let idx = Index::scan(&here_scan, &filter_scan, prev.as_ref())?;
            let _ = idx.save(&cache_scan);
            Ok(idx)
        })
        .await
        .map_err(|e| RexError::Protocol(format!("scanning local folder: {e}")))?
        .map_err(RexError::Io)
    };
    let remote_fut = fetch_index(pool, target, askpass.clone(), helper_id, there, create_remote);
    let (local_idx, (remote_idx, index_bytes)) = tokio::try_join!(local_fut, remote_fut)?;

    let baseline = Index::load(&baseline_path).ok();
    let stamp = jiff::Zoned::now().strftime("%Y%m%d-%H%M%S").to_string();
    let plan = plan_twoway(baseline.as_ref(), &local_idx, &remote_idx, host_label, &stamp);

    let conflict_paths_in = |idx: &Index| -> Vec<String> { idx.entries.keys().filter(|p| olo_rex_sync::twoway::is_conflict_path(p)).cloned().collect() };

    let mut report = TwoWayReport { files: local_idx.file_count(), bytes_received: index_bytes, conflicts: plan.conflicts.clone(), all_conflicts: conflict_paths_in(&local_idx), ..Default::default() };

    if plan.is_empty() {
        if baseline.is_none() {
            let next_base = post_sync_baseline(&local_idx, &remote_idx, &plan);
            let _ = next_base.save(&baseline_path);
        }
        return Ok(report);
    }

    if let Some(cb) = on_work {
        cb();
    }

    // 1. Apply local & remote mutations in parallel (copies preserve losers first).
    let here_mut = here.clone();
    let l_copy = plan.local_copy.clone();
    let l_rem = plan.local_remove.clone();
    let l_mk = plan.local_mkdir.clone();
    let local_mut_fut = async move {
        if l_copy.is_empty() && l_rem.is_empty() && l_mk.is_empty() {
            return Ok(());
        }
        tokio::task::spawn_blocking(move || apply_mutations(&here_mut, &l_copy, &l_rem, &l_mk)).await.map_err(|e| RexError::Protocol(format!("local mutate: {e}")))?.map_err(RexError::Io)
    };
    let remote_mut_fut = mutate_host(pool, target, askpass.clone(), helper_id, &there.path, &plan.remote_copy, &plan.remote_remove, &plan.remote_mkdir);
    let ((), mut_bytes) = tokio::try_join!(local_mut_fut, remote_mut_fut)?;
    report.bytes_sent += mut_bytes;
    report.removed_local = plan.local_remove.len() as u64;
    report.removed_remote = plan.remote_remove.len() as u64;

    // 2. Pull host -> desktop, then push desktop -> host.
    if !plan.pull_paths.is_empty() {
        let pulled = receive_paths(pool, target, askpass.clone(), helper_id, &there.path, here.clone(), &plan.pull_paths, stop.clone()).await?;
        report.pulled = plan.pull_paths.len() as u64;
        report.bytes_sent += pulled.bytes_sent;
        report.bytes_received += pulled.bytes_received;
        report.errors.extend(pulled.errors);
    }
    if !plan.push_paths.is_empty() {
        let pushed = send_paths(pool, target, askpass.clone(), helper_id, &there.path, here.clone(), plan.push_paths.clone(), stop).await?;
        report.pushed = plan.push_paths.len() as u64;
        report.bytes_sent += pushed.bytes_sent;
        report.bytes_received += pushed.bytes_received;
        report.errors.extend(pushed.errors);
    }

    // 3. Advance baseline deterministically when both transfers succeeded cleanly,
    //    and refresh the local stat cache so the watcher echo costs zero reads.
    if report.errors.is_empty() {
        let next_base = post_sync_baseline(&local_idx, &remote_idx, &plan);
        report.files = next_base.file_count();
        report.all_conflicts = conflict_paths_in(&next_base);
        let _ = next_base.save(&baseline_path);
        let here_cache = here;
        let cache_update = local_cache;
        let _ = tokio::task::spawn_blocking(move || {
            if let Ok(updated) = Index::scan(&here_cache, &filter, Some(&local_idx)) {
                let _ = updated.save(&cache_update);
            }
        })
        .await;
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The helper's command, its folder and patterns quoted for the
    /// host's shell, its options as the folder says.
    #[test]
    fn the_helpers_command_is_the_folders() {
        let f = HostFolder { path: "src/my app".into(), delete: true, keep_newer: true, exclude: vec!["target".into(), "*.o".into()], shallow: false, scoped: false };
        let c = f.command("abc", "receive");
        assert!(c.starts_with("\"$HOME\"/.local/share/olo/rex-link/olo-rex-link-abc-$(uname -m) sync receive "), "{c}");
        assert!(c.ends_with(" --delete --keep-newer --exclude target --exclude '*.o'"), "{c}");
        assert!(c.contains("'src/my app'"), "{c}");
        let o = f.options();
        assert!(o.delete_extra && o.keep_newer && o.filter.recursive && o.filter.exclude.len() == 2);

        let w = f.watch_command("abc");
        assert!(w.contains(" sync watch ") && w.ends_with(" --exclude target --exclude '*.o'"), "{w}");
        let f_shallow = HostFolder { shallow: true, ..f };
        assert!(f_shallow.watch_command("abc").contains(" --shallow"));
        assert!(f_shallow.index_command("abc", true).contains(" --create --shallow"));
    }
}
