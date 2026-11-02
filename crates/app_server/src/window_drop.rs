//! A window moved by hand over another of its app's windows: the app is
//! told, on its kit socket (`olo-menus-PID.sock`, the kit's commands),
//! where it is over that window and where it was let go, so that a window
//! let go on another's tab strip becomes tabs there (the kit's
//! `App::window_dropped`). From a worker thread, the latest place only:
//! the compositor never waits on an app.

use olo_sync::Mutex;
use std::{
    io::Write,
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{OnceLock, mpsc},
    time::Duration,
};

use smithay::utils::{Logical, Point};

use crate::{
    shell::WindowElement,
    state::{AnvilState, Backend},
};

/// What a move tells an app: one line of its commands.
struct Note {
    pid: i32,
    line: String,
    /// Let go there: never dropped for a later one.
    dropped: bool,
}

fn socket(pid: i32) -> PathBuf {
    let rt = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    rt.join(format!("olo-menus-{pid}.sock"))
}

fn send(note: Note) {
    static TX: OnceLock<Mutex<mpsc::Sender<Note>>> = OnceLock::new();
    let tx = TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Note>();
        let _ = std::thread::Builder::new().name("window drops".into()).spawn(move || {
            while let Ok(first) = rx.recv() {
                // Places over a window since: only the last of a run counts.
                let mut notes = vec![first];
                while let Ok(n) = rx.try_recv() {
                    if let Some(last) = notes.last_mut().filter(|l| !l.dropped && !n.dropped && l.pid == n.pid) {
                        *last = n;
                    } else {
                        notes.push(n);
                    }
                }
                for n in notes {
                    if let Ok(mut s) = UnixStream::connect(socket(n.pid)) {
                        let _ = s.set_write_timeout(Some(Duration::from_millis(200)));
                        let _ = s.write_all(format!("{}\n", n.line).as_bytes());
                    }
                }
            }
        });
        Mutex::new(tx)
    });
    let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(note);
}

/// Over window `onto` at `at` (its content's coordinates), or over none
/// of its app's (`onto` None); or let go there.
pub fn tell(moved: &WindowElement, onto: Option<(&WindowElement, Point<f64, Logical>)>, dropped: bool) {
    let Some(pid) = moved.meta().owner.borrow().as_ref().map(|(pid, _)| *pid) else { return };
    let id = |w: &WindowElement| w.wl_surface().map(|s| smithay::reexports::wayland_server::Resource::id(&*s).protocol_id());
    let Some(window) = id(moved) else { return };
    let (onto, x, y) = match onto {
        Some((w, at)) => (id(w), at.x, at.y),
        None => (None, 0.0, 0.0),
    };
    let what = if dropped { "window_dropped" } else { "window_over" };
    let line = serde_json::json!({ what: { "window": window, "onto": onto, "x": x, "y": y } }).to_string();
    send(Note { pid, line, dropped });
}

impl<B: Backend> AnvilState<B> {
    /// Another window of `moved`'s app whose content the pointer, at `p`,
    /// is over (the top one there, not covered), and where in it.
    pub fn own_window_under(&self, moved: &WindowElement, p: Point<f64, Logical>) -> Option<(WindowElement, Point<f64, Logical>)> {
        let client = moved.meta().owner.borrow().as_ref().map(|(_, c)| c.clone())?;
        let target = self
            .space
            .elements()
            .rev()
            .filter(|w| *w != moved && !w.meta().covered.get())
            .find(|w| self.space.element_bbox(w).is_some_and(|b| b.to_f64().contains(p)))?;
        if target.meta().owner.borrow().as_ref().map(|(_, c)| c) != Some(&client) || target.tab_only() {
            return None;
        }
        let origin = self.space.element_location(target)? + target.client_render_offset();
        let at = p - origin.to_f64();
        let size = target.content_size();
        (at.x >= 0.0 && at.y >= 0.0 && at.x < size.w as f64 && at.y < size.h as f64).then(|| (target.clone(), at))
    }
}
