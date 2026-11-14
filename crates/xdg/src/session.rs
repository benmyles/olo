//! Whether a program is in the person's real session: the one on a seat,
//! in front of them, where a prompt for the administrator's password, a
//! finger or a face has someone to answer it. A test's session and an
//! agent's screen are sessions too, with nobody there.
//!
//! The session says which display is the person's, in a file
//! ([`real_session_file`]: olo-session on a seat writes it as it starts
//! and as it resumes after an upgrade, and takes it away as it ends), and
//! a program is in the real session if that's the display it is on,
//! however it came to be started. It used to go by a variable handed
//! down from olo-session alone (`OLO_REAL_SESSION`), which whatever
//! wasn't its descendant by an unbroken line of environments didn't
//! have: app_server was started a few lines before the variable was set,
//! so the palette it started, and every program opened from the palette,
//! were told they weren't at the computer. Olo Doctor's fixes and
//! Preferences' GPU settings refused to ask for the administrator there
//! (Ben, 2026-11-12: "it should just ask me for the elevated perms").
//! The variable still counts where no session has said (one from before
//! the file, a program with no display of its own).

use std::path::PathBuf;

/// What a real session keeps in the runtime folder while it runs: its
/// display's name ("wayland-1") and its process id, a line each.
pub fn real_session_file() -> PathBuf {
    crate::runtime_dir().join("olo-real-session")
}

/// This process is the real session on `display` (olo-session's to say).
pub fn mark_real(display: &str) -> std::io::Result<()> {
    mark_at(&real_session_file(), display)
}

/// This process's mark, taken away (another session's is left alone).
pub fn unmark_real() {
    unmark_at(&real_session_file());
}

fn mark_at(path: &std::path::Path, display: &str) -> std::io::Result<()> {
    // (Whole or not at all: read by every program that asks.)
    let tmp = path.with_extension(format!("new-{}", std::process::id()));
    std::fs::write(&tmp, format!("{display}\n{}\n", std::process::id()))?;
    std::fs::rename(&tmp, path)
}

fn unmark_at(path: &std::path::Path) {
    if read(path).is_some_and(|(_, pid)| pid == std::process::id()) {
        let _ = std::fs::remove_file(path);
    }
}

fn read(path: &std::path::Path) -> Option<(String, u32)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let display = lines.next().filter(|d| !d.is_empty())?.to_owned();
    Some((display, lines.next()?.trim().parse().ok()?))
}

/// The display of the real session running here, if one is: its mark is
/// there and the process that made it still is olo-session (a mark left
/// by one that crashed says nothing).
pub fn real_display() -> Option<String> {
    let (display, pid) = read(&real_session_file())?;
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    (comm.trim() == "olo-session").then_some(display)
}

/// The display this program is on, as its environment says: its
/// session's socket's (`OLO_SOCKET`, "…/olo-wayland-1.sock"), else
/// `WAYLAND_DISPLAY`'s name.
pub fn own_display() -> Option<String> {
    display_of(std::env::var("OLO_SOCKET").ok().as_deref(), std::env::var("WAYLAND_DISPLAY").ok().as_deref())
}

fn display_of(socket: Option<&str>, wayland: Option<&str>) -> Option<String> {
    let name = |p: &str| std::path::Path::new(p).file_name().and_then(|n| n.to_str()).map(str::to_owned);
    let from_socket = socket.and_then(name).and_then(|n| Some(n.strip_prefix("olo-")?.strip_suffix(".sock")?.to_owned()));
    from_socket.or_else(|| wayland.filter(|w| !w.is_empty()).and_then(name))
}

/// Whether this program is in the person's real session (agent screens
/// aside: `olo_system::env::at_the_computer` for that).
pub fn real() -> bool {
    decide(real_display().as_deref(), own_display().as_deref(), std::env::var_os("OLO_REAL_SESSION").is_some())
}

/// By the display, where the session has said which is the person's and
/// this program is on one; else by what it was handed (`handed`:
/// `OLO_REAL_SESSION`).
fn decide(real: Option<&str>, mine: Option<&str>, handed: bool) -> bool {
    match (real, mine) {
        (Some(real), Some(mine)) => real == mine,
        _ => handed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_the_display_where_the_session_says_which_is_the_persons() {
        // Started by something that never had the variable (the palette
        // under an app_server started before it was set): on the
        // person's display all the same.
        assert!(decide(Some("wayland-1"), Some("wayland-1"), false));
        // A test's session or an agent's screen, whatever was handed
        // down from the shell that started it.
        assert!(!decide(Some("wayland-1"), Some("wayland-4"), true));
        // No session has said (an older one), or no display of its own:
        // what it was handed.
        assert!(decide(None, Some("wayland-1"), true));
        assert!(!decide(None, Some("wayland-1"), false));
        assert!(decide(Some("wayland-1"), None, true));
        assert!(!decide(Some("wayland-1"), None, false));
    }

    #[test]
    fn a_programs_display() {
        assert_eq!(display_of(Some("/run/user/1000/olo-wayland-1.sock"), Some("wayland-9")).as_deref(), Some("wayland-1"));
        assert_eq!(display_of(None, Some("wayland-2")).as_deref(), Some("wayland-2"));
        assert_eq!(display_of(None, Some("/run/user/1000/wayland-3")).as_deref(), Some("wayland-3"));
        assert_eq!(display_of(Some("/somewhere/else.sock"), Some("wayland-2")).as_deref(), Some("wayland-2"));
        assert_eq!(display_of(None, Some("")), None);
        assert_eq!(display_of(None, None), None);
    }

    #[test]
    fn a_mark_is_its_makers() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-xdg-session-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("olo-real-session");
        mark_at(&path, "wayland-1").unwrap();
        assert_eq!(read(&path), Some(("wayland-1".to_owned(), std::process::id())));
        // Another session's (a later login's) isn't ours to take away.
        std::fs::write(&path, "wayland-1\n1\n").unwrap();
        unmark_at(&path);
        assert_eq!(read(&path), Some(("wayland-1".to_owned(), 1)));
        mark_at(&path, "wayland-0").unwrap();
        unmark_at(&path);
        assert!(!path.exists());
        // Half a mark says nothing.
        std::fs::write(&path, "wayland-1\n").unwrap();
        assert_eq!(read(&path), None);
        std::fs::write(&path, "").unwrap();
        assert_eq!(read(&path), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
