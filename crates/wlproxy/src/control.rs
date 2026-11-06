//! The proxy's control socket, `$XDG_RUNTIME_DIR/olo-wlproxy-PID.sock`:
//! moving programs between app_servers (`olo-screen take` and `give`).
//! A line a request, answered with a line (`list`: a line a program, then
//! an empty one), then the connection closes:
//!
//! - `list` → `PID UPSTREAM` for each program, UPSTREAM the socket of the
//!   app_server it is on (`-` for the proxy's own).
//! - `move PID PATH` → `ok`, or `error: WHY`: program PID to the
//!   app_server listening for proxied programs at PATH (another session's
//!   `olo-wayland-N-proxied`; the proxy's own, or `-`, brings it back).
//! - `home PATH` → `ok N`: every program on the app_server at PATH back
//!   to the proxy's own (a screen about to stop).
//!
//! Requests are small and answered at once, on the proxy's one thread;
//! a client that doesn't say its line within half a second is dropped.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    time::Duration,
};

use crate::client::Client;

pub struct Control {
    pub listener: UnixListener,
    path: PathBuf,
}

/// Where proxy `pid` listens for control.
pub fn path_for(pid: u32) -> PathBuf {
    olo_xdg::runtime_dir().join(format!("olo-wlproxy-{pid}.sock"))
}

pub fn listen() -> Option<Control> {
    let path = path_for(std::process::id());
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            crate::log(format!("no control socket ({e}): programs can't be moved to other screens"));
            return None;
        }
    };
    listener.set_nonblocking(true).ok()?;
    Some(Control { listener, path })
}

pub fn remove(control: &Option<Control>) {
    if let Some(c) = control {
        let _ = std::fs::remove_file(&c.path);
    }
}

/// Answer whoever is waiting.
pub fn serve(control: &Control, clients: &mut [Client], home: &Path) {
    while let Ok((s, _)) = control.listener.accept() {
        let _ = s.set_nonblocking(false);
        let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
        let _ = s.set_write_timeout(Some(Duration::from_millis(500)));
        let mut line = String::new();
        if BufReader::new(&s).read_line(&mut line).is_err() {
            continue;
        }
        let answer = answer(line.trim(), clients, home);
        let _ = (&s).write_all(answer.as_bytes());
    }
}

fn same(a: &Path, b: &Path) -> bool {
    a == b || std::fs::canonicalize(a).ok().zip(std::fs::canonicalize(b).ok()).is_some_and(|(a, b)| a == b)
}

fn answer(line: &str, clients: &mut [Client], home: &Path) -> String {
    let (command, rest) = line.split_once(' ').unwrap_or((line, ""));
    let (first, last) = rest.split_once(' ').map_or((rest, None), |(a, b)| (a, Some(b)));
    match (command, first, last) {
        ("list", "", None) => {
            let mut out = String::new();
            for c in clients.iter() {
                let up = c.away.as_ref().map_or("-".to_owned(), |p| p.display().to_string());
                out.push_str(&format!("{} {up}\n", c.pid));
            }
            out.push('\n');
            out
        }
        ("move", pid, Some(to)) => {
            let Ok(pid) = pid.parse::<i32>() else { return "error: no such program\n".into() };
            let to = PathBuf::from(to);
            let to = (to.as_os_str() != "-" && !same(&to, home)).then_some(to);
            if let Some(t) = &to
                && UnixStream::connect(t).is_err()
            {
                return format!("error: nothing is listening at {}\n", t.display());
            }
            let mut found = false;
            for c in clients.iter_mut().filter(|c| c.pid == pid) {
                found = true;
                if let Err(e) = c.move_to(to.clone()) {
                    return format!("error: {e}\n");
                }
            }
            if !found {
                return "error: no such program\n".into();
            }
            let to = to.as_ref().map_or("its own".to_owned(), |t| t.display().to_string());
            crate::log(format!("pid {pid}: moved to the app_server at {to}"));
            "ok\n".into()
        }
        ("home", _, _) if !rest.is_empty() => {
            let from = PathBuf::from(rest);
            let mut n = 0;
            for c in clients.iter_mut().filter(|c| c.away.as_ref().is_some_and(|a| same(a, &from))) {
                if c.move_to(None).is_ok() {
                    n += 1;
                }
            }
            if n > 0 {
                crate::log(format!("{n} program(s) back from the app_server at {}", from.display()));
            }
            format!("ok {n}\n")
        }
        _ => "error: unknown request\n".into(),
    }
}
