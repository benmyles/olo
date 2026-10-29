//! Approving requests without asking, for a while (Deskbar's key, Olo ▸
//! Approve Without Asking; `Request::SetAutoApprove`). While it is on,
//! Terminal answers sudo's password prompt itself (it asks for the
//! password this was turned on with, `Request::AutoApproveSecret`) and
//! Passwords unlocks for the apps asking, each saying so in a quiet
//! notification. Turning it on takes the account password, checked
//! through PAM on a thread; it ends at its time, when the screen locks
//! (sleeping locks first) and with app_server, and the password is
//! wiped then.

use std::{
    rc::Rc,
    time::{Duration, SystemTime},
};

use olo_protocol::Reply;
use smithay::reexports::calloop::channel::{self, Event as ChannelEvent};
use tracing::{info, warn};
use zeroize::Zeroizing;

use crate::{
    ipc::Conn,
    state::{AnvilState, Backend},
};

/// The shortest and longest it can be on for, in minutes.
pub const SHORTEST: u32 = 5;
pub const LONGEST: u32 = 24 * 60;

pub struct AutoApprove {
    pub until: SystemTime,
    secret: Zeroizing<String>,
}

impl std::fmt::Debug for AutoApprove {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the password.
        f.debug_struct("AutoApprove").field("until", &self.until).finish_non_exhaustive()
    }
}

impl<B: Backend + 'static> AnvilState<B> {
    /// `Request::SetAutoApprove`: the reply, or None when it comes later
    /// (once the password is checked).
    pub fn set_auto_approve(&mut self, minutes: Option<u32>, password: Option<String>, conn: Rc<Conn>) -> Option<Reply> {
        let password = password.map(Zeroizing::new);
        let Some(minutes) = minutes else {
            self.end_auto_approve("turned off");
            return Some(Reply::Ok);
        };
        let minutes = minutes.clamp(SHORTEST, LONGEST);
        let until = SystemTime::now() + Duration::from_secs(minutes as u64 * 60);
        if self.lock.locked() {
            return Some(Reply::Error { message: "The screen is locked.".into() });
        }
        if let Some(a) = self.auto_approve.as_mut() {
            // On already: only the length changes.
            a.until = until;
            info!(minutes, by = %conn.peer(), "approving without asking: for another while");
            self.power_changed();
            return Some(Reply::Ok);
        }
        let Some(password) = password.filter(|p| !p.is_empty()) else {
            return Some(Reply::Error { message: "Turning it on takes your password.".into() });
        };
        let Some(user) = olo_system::auth::current_user() else {
            return Some(Reply::Error { message: "Can't tell who you are.".into() });
        };
        let (tx, rx) = channel::channel::<Result<Zeroizing<String>, String>>();
        std::thread::Builder::new()
            .name("approve check".into())
            .spawn(move || {
                let checked = olo_system::auth::check_password(&user, &password).map(|()| password).map_err(|e| e.to_string());
                let _ = tx.send(checked);
            })
            .ok()?;
        let by = conn.peer();
        let _ = self.handle.insert_source(rx, move |event, _, state| {
            let ChannelEvent::Msg(checked) = event else { return };
            conn.send(&match checked {
                // Locked meanwhile: not now.
                Ok(_) if state.lock.locked() => Reply::Error { message: "The screen is locked.".into() },
                Ok(secret) => {
                    info!(minutes, by = %by, "approving without asking");
                    state.auto_approve = Some(AutoApprove { until, secret });
                    state.power_changed();
                    Reply::Ok
                }
                Err(message) => {
                    warn!(by = %by, "approving without asking: not turned on: {message}");
                    Reply::Error { message }
                }
            });
        });
        None
    }

    /// It ends (`why`, for the log); the password is wiped.
    pub fn end_auto_approve(&mut self, why: &str) {
        if self.auto_approve.take().is_some() {
            info!(why, "approving without asking: ended");
            self.power_changed();
        }
    }

    /// Every power tick: its time is up.
    pub fn auto_approve_tick(&mut self) {
        if self.auto_approve.as_ref().is_some_and(|a| SystemTime::now() >= a.until) {
            self.end_auto_approve("time's up");
        }
    }

    pub fn auto_approve_until(&self) -> Option<u64> {
        self.auto_approve.as_ref().and_then(|a| a.until.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_secs())
    }

    /// `Request::AutoApproveSecret`: the password, for Olo's Terminal only.
    pub fn auto_approve_secret(&self, conn: &Conn) -> Reply {
        let Some(a) = self.auto_approve.as_ref().filter(|a| SystemTime::now() < a.until) else {
            return Reply::Error { message: "Not approving without asking.".into() };
        };
        if !conn.peer_is_ours("olo-terminal") {
            warn!(by = %conn.peer(), "asked for the approval password: refused");
            return Reply::Error { message: "Only Terminal can have that.".into() };
        }
        Reply::Secret { password: a.secret.to_string() }
    }
}
