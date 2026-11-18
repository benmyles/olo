//! An app's own files: its settings (in the config folder) and what it
//! remembers (in the state folder), as JSON. Read whole or not at all;
//! written beside the file (`.NAME.saving.PID.N`, the writer's own) and
//! renamed over it, so neither a reader nor a crash halfway through ever
//! sees half a file. A file made new is the person's alone (0600); one
//! there already keeps its permissions.
//!
//! [`save`] writes off the caller's thread, one file after another in the
//! order saved (so the last of quick saves to a file is the one kept, and
//! only it is written); the kit writes what's still to be written as the
//! app quits or re-execs ([`flush`]).

use std::{
    io::{self, Write as _},
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    sync::{OnceLock, mpsc},
    time::Duration,
};

use serde::{Serialize, de::DeserializeOwned};

/// The JSON at `path` as a `T`, or None: no file, unreadable, or not a `T`.
pub fn load<T: DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// `bytes` in place of `path`, whole (its folder made if need be), on the
/// calling thread.
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path.file_name().ok_or_else(|| io::Error::other(format!("{}: no file name", path.display())))?;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    // A sibling of this writer's own: two programs keeping the same file
    // (a window and its service; every agent screen's notification
    // centre, until 2026-11-18) wrote through one ".NAME.saving", each
    // over the other's bytes, and the second rename found nothing.
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = dir.join(format!(".{}.saving.{}.{}", name.to_string_lossy(), std::process::id(), WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let mode = std::fs::metadata(path).map_or(0o600, |m| m.permissions().mode() & 0o7777);
    let written = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&tmp)?;
        // (A file left from a crash has the mode it was made with.)
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        f.write_all(bytes)?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// `value` as JSON (laid out to read: people look in these files) in
/// place of `path`, soon, off this thread.
pub fn save<T: Serialize + ?Sized>(path: impl Into<PathBuf>, value: &T) {
    if let Ok(bytes) = serde_json::to_vec_pretty(value) {
        save_bytes(path, bytes);
    }
}

/// `bytes` in place of `path`, soon, off this thread (JSON an app lays
/// out itself, or keeps small).
pub fn save_bytes(path: impl Into<PathBuf>, bytes: Vec<u8>) {
    let _ = writer().send(Job::Write(path.into(), bytes));
}

/// Waits (a second at most) until everything saved so far is written.
pub fn flush() {
    let Some(writer) = WRITER.get() else { return };
    let (done, wait) = mpsc::channel();
    if writer.send(Job::Flush(done)).is_ok() {
        let _ = wait.recv_timeout(Duration::from_secs(1));
    }
}

enum Job {
    Write(PathBuf, Vec<u8>),
    Flush(mpsc::Sender<()>),
}

static WRITER: OnceLock<mpsc::Sender<Job>> = OnceLock::new();

/// The thread that writes what's saved, started the first time.
fn writer() -> &'static mpsc::Sender<Job> {
    WRITER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        let _ = std::thread::Builder::new().name("store".into()).spawn(move || {
            while let Ok(first) = rx.recv() {
                let jobs: Vec<Job> = std::iter::once(first).chain(rx.try_iter()).collect();
                for (i, job) in jobs.iter().enumerate() {
                    match job {
                        // Saved again since: only the last is written.
                        Job::Write(path, _) if jobs[i + 1..].iter().any(|j| matches!(j, Job::Write(p, _) if p == path)) => {}
                        Job::Write(path, bytes) => {
                            if let Err(e) = write(path, bytes) {
                                tracing::warn!("keeping {}: {e}", path.display());
                            }
                        }
                        Job::Flush(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            }
        });
        tx
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two writers of one file (two programs keeping the same file, as
    /// every agent screen's notification centre did the person's
    /// history): neither fails, the file is always the whole of one's,
    /// and nothing is left beside it.
    #[test]
    fn two_writers_of_one_file_never_mix() {
        let dir = Path::new("/var/tmp").join(format!("olo-store-two-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("history.json");
        let (a, b) = (vec![b'a'; 200_000], vec![b'b'; 150_000]);
        let writers: Vec<_> = [a.clone(), b.clone()]
            .into_iter()
            .map(|bytes| {
                let path = path.clone();
                std::thread::spawn(move || (0..150).map(|_| write(&path, &bytes)).filter(Result::is_err).count())
            })
            .collect();
        let mut read = 0;
        while writers.iter().any(|w| !w.is_finished()) {
            if let Ok(got) = std::fs::read(&path) {
                assert!(got == a || got == b, "a file of {} bytes, neither writer's", got.len());
                read += 1;
            }
        }
        for w in writers {
            assert_eq!(w.join().unwrap(), 0, "a write failed");
        }
        assert!(read > 0);
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left, [std::ffi::OsString::from("history.json")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn written_whole_and_private() {
        let dir = Path::new("/var/tmp").join(format!("olo-store-test-{}", std::process::id()));
        let path = dir.join("deeper").join("settings.json");
        save(&path, &serde_json::json!({"size": 12}));
        save(&path, &serde_json::json!({"size": 14}));
        flush();
        assert_eq!(load::<serde_json::Value>(&path), Some(serde_json::json!({"size": 14})));
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        // One there already keeps its permissions; nothing's left beside.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write(&path, b"{\"size\":16}").unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o644);
        assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        // Not there, or not JSON of that shape: none.
        assert_eq!(load::<serde_json::Value>(&dir.join("none.json")), None);
        assert_eq!(load::<Vec<u32>>(&path), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
