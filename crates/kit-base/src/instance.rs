//! One process per app, as on BeOS: launching an app that is already
//! running hands its arguments to the running copy, which opens the new
//! window(s) itself.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
};

use crate::Proxy;

/// A launch forwarded from another process.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Launch {
    pub args: Vec<String>,
    /// The launching process's working directory.
    pub cwd: Option<PathBuf>,
    /// Told when the app's UI thread has taken it (olo-ui does, as it
    /// hands it to the app): the launching process waits for that, and
    /// starts a copy of its own if it doesn't come.
    #[serde(skip)]
    pub taken: Option<std::sync::mpsc::Sender<()>>,
}

impl Launch {
    /// The app has it ([`Launch::taken`]): false if the launching process
    /// gave up waiting (it starts a copy of its own, so this one is not to
    /// open it too).
    pub fn take(&self) -> bool {
        self.taken.as_ref().is_none_or(|tx| tx.send(()).is_ok())
    }
}

/// What a copy says first as a launch connects: it answers whether it
/// took the launch ("ok", "no"). A copy from before said nothing.
const HELLO: &str = "olo-instance 2";

/// How long a launch waits for the running copy to take it.
const TAKEN_WITHIN: std::time::Duration = std::time::Duration::from_secs(3);

fn socket_path(app_id: &str) -> PathBuf {
    socket_on(app_id, &olo_xdg::display_name())
}

fn socket_on(app_id: &str, display: &str) -> PathBuf {
    let rt = olo_xdg::runtime_dir();
    let display = if display.is_empty() { "wayland-0" } else { display };
    rt.join(format!("{app_id}-{display}.sock"))
}

/// If `app_id` is already running, hand it `args` and return true.
pub fn forward(app_id: &str, args: &[String]) -> bool {
    send(socket_path(app_id), args)
}

/// [`forward`] to the copy running on `display` (its name, `wayland-1`),
/// from a program that may have no display of its own (one a coding
/// agent's daemon started).
pub fn forward_on(app_id: &str, display: &str, args: &[String]) -> bool {
    send(socket_on(app_id, display), args)
}

/// Hands the launch over, true once the running copy took it. A copy
/// that was quitting (its last window closed, its loop ending) still
/// accepted connections and dropped what came: nothing opened. Now it
/// greets, then answers whether its UI thread took the launch; no
/// answer, and this process starts a copy of its own. A copy from before
/// that (no greeting: mid-upgrade) is trusted as it was.
fn send(path: PathBuf, args: &[String]) -> bool {
    let Ok(mut s) = UnixStream::connect(path) else { return false };
    let launch = Launch { args: args.to_vec(), cwd: std::env::current_dir().ok(), taken: None };
    let line = serde_json::to_string(&launch).unwrap_or_default();
    if !(s.write_all(line.as_bytes()).is_ok() && s.write_all(b"\n").is_ok()) {
        return false;
    }
    let _ = s.set_read_timeout(Some(TAKEN_WITHIN));
    let mut reader = BufReader::new(s);
    let mut hello = String::new();
    // No greeting: a copy quitting (its socket still accepting as it
    // goes), so this process opens its own. (Copies from before the
    // greeting read the launch and closed: they're long gone, and taking
    // the close for "taken" left launches with no window at all.)
    if reader.read_line(&mut hello).is_err() || hello.trim() != HELLO {
        return false;
    }
    let mut answer = String::new();
    reader.read_line(&mut answer).is_ok() && answer.trim() == "ok"
}

/// Whether `app_id` is running in this session (its copy is listening).
pub fn running(app_id: &str) -> bool {
    UnixStream::connect(socket_path(app_id)).is_ok()
}

/// Accept forwarded launches; each arrives in `App::message` as a
/// [`Launch`].
pub fn listen(app_id: &str, proxy: Proxy) {
    listen_at(socket_path(app_id), proxy);
}

/// The socket this process listens on, and its inode: [`close`] removes
/// it only while it's still this process's.
static LISTENING: std::sync::Mutex<Option<(PathBuf, u64)>> = std::sync::Mutex::new(None);

/// Stop taking launches: the app is quitting. Its socket goes at once, so
/// a launch from now on starts a copy of its own straight away rather
/// than waiting for this one to answer (Preferences took seconds to open
/// just after its last window closed).
pub fn close() {
    use std::os::unix::fs::MetadataExt as _;
    let Some((path, ino)) = LISTENING.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    // (A copy started since has its own socket there: left alone.)
    if std::fs::metadata(&path).is_ok_and(|m| m.ino() == ino) {
        let _ = std::fs::remove_file(&path);
    }
}

fn listen_at(path: PathBuf, proxy: Proxy) {
    use std::os::unix::fs::MetadataExt as _;
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else { return };
    if let Ok(m) = std::fs::metadata(&path) {
        *LISTENING.lock().unwrap_or_else(|e| e.into_inner()) = Some((path.clone(), m.ino()));
    }
    std::thread::spawn(move || {
        for mut s in listener.incoming().flatten() {
            let _ = s.write_all(format!("{HELLO}\n").as_bytes());
            let mut line = String::new();
            let Ok(reader) = s.try_clone() else { continue };
            if BufReader::new(reader).read_line(&mut line).is_ok()
                && let Ok(mut launch) = serde_json::from_str::<Launch>(&line)
            {
                // (Answered once the UI thread has it, or not: the app
                // quitting drops it, unanswered.)
                let (tx, rx) = std::sync::mpsc::channel();
                launch.taken = Some(tx);
                proxy.send(launch);
                let answer = if rx.recv_timeout(TAKEN_WITHIN).is_ok() { "ok\n" } else { "no\n" };
                let _ = s.write_all(answer.as_bytes());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn at(name: &str) -> PathBuf {
        socket_on(&format!("olo-test-{name}-{}", std::process::id()), "test")
    }

    /// A launch the app takes is answered so; one dropped (the app's loop
    /// ending) isn't, at once, and the launching process opens a copy of
    /// its own.
    #[test]
    fn a_launch_is_taken_or_refused() {
        let taken = at("taken");
        listen_at(
            taken.clone(),
            Proxy::new(|m| {
                if let Some(launch) = m.downcast_ref::<Launch>() {
                    assert!(launch.take());
                }
            }),
        );
        assert!(send(taken.clone(), &["a".into()]));
        assert!(send(taken.clone(), &["b".into()]));

        let dropped = at("dropped");
        listen_at(dropped.clone(), Proxy::new(|_| {}));
        let t = Instant::now();
        assert!(!send(dropped.clone(), &["a".into()]));
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());

        // A socket that takes the connection and closes it, saying
        // nothing (a copy quitting): not taken.
        let mute = at("mute");
        let listener = UnixListener::bind(&mute).unwrap();
        std::thread::spawn(move || {
            for s in listener.incoming().flatten() {
                drop(s);
            }
        });
        assert!(!send(mute.clone(), &["a".into()]));
        let _ = std::fs::remove_file(mute);

        assert!(!send(at("nobody"), &[]));
        let _ = std::fs::remove_file(taken);
        let _ = std::fs::remove_file(dropped);
    }

    /// A launch the launching process gave up on isn't to be opened too.
    #[test]
    fn a_launch_given_up_on_is_not_taken() {
        let (tx, rx) = std::sync::mpsc::channel();
        let launch = Launch { args: vec![], cwd: None, taken: Some(tx) };
        drop(rx);
        assert!(!launch.take());
        assert!(Launch { args: vec![], cwd: None, taken: None }.take());
    }
}
