//! Leaving for Olo, from the last page: restarting the computer
//! (logind's `Reboot`, which polkit allows the person at the computer),
//! or logging out of the desktop Setup runs in (GNOME's session manager,
//! told not to ask again; else logind ends this login), so the login
//! screen offers Olo.

/// Restart (`restart`), or log out; off the window's thread (D-Bus).
pub fn leave(restart: bool) {
    std::thread::spawn(move || {
        let r = if restart { olo_system::power::reboot().map_err(|e| e.to_string()) } else { log_out() };
        if let Err(e) = r {
            tracing::warn!("setup: couldn't {}: {e}", if restart { "restart" } else { "log out" });
        }
    });
}

fn log_out() -> Result<(), String> {
    let session = zbus::blocking::Connection::session().map_err(|e| e.to_string())?;
    // GNOME's: 1, without asking again.
    if session.call_method(Some("org.gnome.SessionManager"), "/org/gnome/SessionManager", Some("org.gnome.SessionManager"), "Logout", &(1u32,)).is_ok() {
        return Ok(());
    }
    let id = std::env::var("XDG_SESSION_ID").map_err(|_| "no login session to end".to_owned())?;
    let system = zbus::blocking::Connection::system().map_err(|e| e.to_string())?;
    system.call_method(Some("org.freedesktop.login1"), "/org/freedesktop/login1", Some("org.freedesktop.login1.Manager"), "TerminateSession", &(id,)).map(|_| ()).map_err(|e| e.to_string())
}
