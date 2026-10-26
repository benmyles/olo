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
}

fn socket_path(app_id: &str) -> PathBuf {
    let rt = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    rt.join(format!("{app_id}-{display}.sock"))
}

/// If `app_id` is already running, hand it `args` and return true.
pub fn forward(app_id: &str, args: &[String]) -> bool {
    let Ok(mut s) = UnixStream::connect(socket_path(app_id)) else { return false };
    let launch = Launch { args: args.to_vec(), cwd: std::env::current_dir().ok() };
    let line = serde_json::to_string(&launch).unwrap_or_default();
    s.write_all(line.as_bytes()).is_ok() && s.write_all(b"\n").is_ok()
}

/// Accept forwarded launches; each arrives in `App::message` as a
/// [`Launch`].
pub fn listen(app_id: &str, proxy: Proxy) {
    let path = socket_path(app_id);
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else { return };
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(s).read_line(&mut line).is_ok()
                && let Ok(launch) = serde_json::from_str::<Launch>(&line)
            {
                proxy.send(launch);
            }
        }
    });
}
