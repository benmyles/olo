//! Who's using a repository: a computer (or a program) backing up or
//! restoring holds a shared lock; tidying it (`prune`) holds the only one.
//! A lock is a small sealed object (`locks/…`: host, process, time) kept
//! fresh every few minutes while it's held. One not refreshed for half an
//! hour is stale: its holder crashed, or slept (a laptop's lid closed
//! mid-backup), and is taken as gone. A holder that finds it went that long
//! without refreshing (it slept) takes its own lock as lost and stops before
//! writing anything more: what it wrote may have been tidied away meanwhile.
//! Taking a lock writes it first, then looks at everyone else's, so of two
//! taken at once that can't both be held, each sees the other and neither
//! goes on (the next try does).

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, Ordering::Relaxed},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    crypto::{Id, Key},
    repo::now,
    store::Store,
};

/// A lock not refreshed this long (seconds) is stale.
pub const STALE: i64 = 30 * 60;
/// Refreshed this often.
const REFRESH: Duration = Duration::from_secs(5 * 60);
/// A holder that went this long without refreshing takes its lock as lost
/// (well inside `STALE`: time for what it's doing to finish).
const LOST: i64 = 20 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Info {
    host: String,
    pid: u32,
    /// When it was last refreshed, seconds since 1970.
    time: i64,
    exclusive: bool,
}

/// What a held lock's holder checks before it writes.
pub(crate) struct Held {
    pub exclusive: bool,
    /// When it was last written, seconds since 1970.
    refreshed: AtomicI64,
    lost: AtomicBool,
}

impl Held {
    /// Still held: no gap since it was last refreshed long enough for
    /// someone else to have taken it as stale.
    pub fn check(&self) -> Result<()> {
        if self.lost.load(Relaxed) || now() - self.refreshed.load(Relaxed) > LOST {
            self.lost.store(true, Relaxed);
            return Err(Error::LockLost);
        }
        Ok(())
    }
}

/// A lock held (let go of when dropped).
pub struct Lock {
    pub(crate) held: Arc<Held>,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Lock {
    pub fn exclusive(&self) -> bool {
        self.held.exclusive
    }

    /// Still held (see [`Held::check`]).
    pub fn check(&self) -> Result<()> {
        self.held.check()
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// This computer's name.
pub fn host() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: a buffer of its length; the name is NUL-terminated in it.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } == 0;
    let name = if ok { String::from_utf8_lossy(&buf[..buf.iter().position(|b| *b == 0).unwrap_or(0)]).into_owned() } else { String::new() };
    if name.is_empty() { "this computer".into() } else { name }
}

fn write(store: &dyn Store, key: &Key, info: &Info) -> Result<String> {
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).expect("the system's random source failed");
    let name = format!("locks/{}", Id(raw).hex());
    let mut plain = Vec::new();
    ciborium::into_writer(info, &mut plain).expect("encodes");
    store.put(&name, &key.seal(&plain))?;
    Ok(name)
}

/// Stale: not refreshed for `STALE`, or this computer's and its process
/// gone.
fn stale(info: &Info, me: &str) -> bool {
    now() - info.time > STALE || (info.host == me && !std::path::Path::new(&format!("/proc/{}", info.pid)).exists())
}

/// Take a lock on the repository in `store`: shared (backing up, restoring)
/// or the only one (`exclusive`: tidying). Someone else's in the way: said
/// who (`Error::Busy`).
pub(crate) fn take(store: Arc<dyn Store>, key: Key, exclusive: bool) -> Result<Lock> {
    let me = host();
    let mut info = Info { host: me.clone(), pid: std::process::id(), time: now(), exclusive };
    let name = write(&*store, &key, &info)?;
    let mut busy = None;
    for (other, _) in store.list("locks")? {
        if other == name {
            continue;
        }
        let Ok(sealed) = store.get(&other) else { continue };
        let Some(theirs) = key.open(&sealed).and_then(|p| ciborium::from_reader::<Info, _>(&p[..]).ok()) else { continue };
        if stale(&theirs, &me) {
            let _ = store.delete(&other);
            continue;
        }
        if exclusive || theirs.exclusive {
            let who = if theirs.host == me { "another DiskBackup on this computer".to_string() } else { theirs.host.clone() };
            busy = Some(if theirs.exclusive { format!("{who} is tidying the backup") } else { format!("{who} is using the backup") });
        }
    }
    if let Some(why) = busy {
        let _ = store.delete(&name);
        return Err(Error::Busy(why));
    }
    let held = Arc::new(Held { exclusive, refreshed: AtomicI64::new(info.time), lost: AtomicBool::new(false) });
    let (stop, rx) = mpsc::channel::<()>();
    let thread = std::thread::Builder::new()
        .name("diskbackup-lock".into())
        .spawn({
            let held = held.clone();
            move || {
                let mut name = name;
                // Fresh every few minutes (a new object, then the old one
                // gone: objects are never changed), until let go of.
                while let Err(mpsc::RecvTimeoutError::Timeout) = rx.recv_timeout(REFRESH) {
                    if held.check().is_err() {
                        continue;
                    }
                    info.time = now();
                    if let Ok(new) = write(&*store, &key, &info) {
                        held.refreshed.store(info.time, Relaxed);
                        let _ = store.delete(&name);
                        name = new;
                    }
                }
                let _ = store.delete(&name);
            }
        })
        .map_err(Error::Io)?;
    Ok(Lock { held, stop: Some(stop), thread: Some(thread) })
}

/// The lock a repository's holder registered, checked before it writes.
#[derive(Default)]
pub(crate) struct Registered(Mutex<std::sync::Weak<Held>>);

impl Registered {
    pub fn set(&self, lock: &Lock) {
        if let Ok(mut w) = self.0.lock() {
            *w = Arc::downgrade(&lock.held);
        }
    }

    pub fn get(&self) -> Option<Arc<Held>> {
        self.0.lock().ok().and_then(|w| w.upgrade())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LocalStore;

    /// Shared with shared; the only one with none; let go of, taken; a
    /// stale one (a process gone) passed over.
    #[test]
    fn shared_and_exclusive() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-diskbackup-test-{}-lock", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store: Arc<dyn Store> = Arc::new(LocalStore::new(&dir));
        let key = Key::generate();
        let a = take(store.clone(), key.clone(), false).unwrap();
        let b = take(store.clone(), key.clone(), false).unwrap();
        assert!(matches!(take(store.clone(), key.clone(), true), Err(Error::Busy(_))));
        drop((a, b));
        assert!(store.list("locks").unwrap().is_empty());
        let x = take(store.clone(), key.clone(), true).unwrap();
        assert!(matches!(take(store.clone(), key.clone(), false), Err(Error::Busy(_))));
        assert!(x.check().is_ok());
        drop(x);
        // A crashed holder's (no such process here): stale at once.
        write(&*store, &key, &Info { host: host(), pid: u32::MAX - 1, time: now(), exclusive: true }).unwrap();
        let y = take(store.clone(), key.clone(), false).unwrap();
        assert_eq!(store.list("locks").unwrap().len(), 1);
        // Asleep too long: lost.
        y.held.refreshed.store(now() - LOST - 1, Relaxed);
        assert!(matches!(y.check(), Err(Error::LockLost)));
        drop(y);
        let _ = std::fs::remove_dir_all(dir);
    }
}
