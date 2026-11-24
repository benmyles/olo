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

use olo_rex_sync::wire::{Options, Report, Source};
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

    /// The engine's options on this side, as the folder says them.
    fn options(&self) -> Options {
        Options {
            delete_extra: self.delete,
            keep_newer: self.keep_newer,
            filter: olo_rex_sync::index::Filter { recursive: !self.shallow, exclude: self.exclude.clone(), ..Default::default() },
            cache_path: None,
            scoped: self.scoped,
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
    }
}
