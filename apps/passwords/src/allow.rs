//! The Allow alert's other answers. A terminal session's `op`, or its
//! first SSH signature, is allowed by what unlocks Passwords as well as
//! by the alert's Allow: Touch ID (fprintd), a touch of a security key
//! set up (`olo_fido`), or a look (face unlock). Each waits on a thread
//! of its own while the alert is open and stops as it goes (the sensor
//! and the key after `GIVE_UP` at most, so an alert left open holds
//! neither from sudo). None does where nobody sits at the computer
//! (`olo_system::env::readers_here`: a test session, an agent's screen).

use std::time::Duration;

use olo_system::{env, face, fingerprint};
use olo_ui::Proxy;

use crate::cli::Session;

/// How long the sensor is listened to for an alert left open (the key
/// gives up by itself in about as long).
const GIVE_UP: Duration = Duration::from_secs(40);

/// What can allow it besides the button.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ways {
    pub finger: bool,
    pub key: bool,
    pub face: bool,
}

impl Ways {
    /// For `user` now: a finger enrolled, a key set up and plugged in, a
    /// face enrolled and a camera. Blocks (fprintd and olo-faced are
    /// asked over D-Bus): call it from a worker. Asks nothing of the
    /// sensor, the key or the camera themselves.
    pub fn find(user: &str) -> Ways {
        Ways {
            finger: env::finger_here() && fingerprint::enrolled(user).unwrap_or(false),
            key: env::key_here() && olo_fido::ready(user),
            face: env::face_here() && {
                let s = face::status();
                s.camera && s.enrolled > 0
            },
        }
    }

    /// The sentence the alert ends with, saying what there is.
    pub fn sentence(&self) -> Option<String> {
        let ways: Vec<&str> = [(self.finger, "use Touch ID"), (self.key, "touch your security key"), (self.face, "look at the camera")].into_iter().filter(|(on, _)| *on).map(|(_, w)| w).collect();
        let list = match ways.as_slice() {
            [] => return None,
            [one] => (*one).to_owned(),
            [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
        };
        Some(format!("To allow it, you can also {list}."))
    }
}

/// One of them answered: `session` is allowed, by `how` ("Touch ID").
pub struct Allowed {
    pub session: Session,
    pub how: &'static str,
}

/// The waits for one alert; dropping it stops them.
pub struct Waits {
    key: Option<olo_fido::Touch>,
    finger: Option<fingerprint::Stop>,
}

impl Waits {
    /// Wait in each of `ways` for `user`, an answer coming to `proxy` as
    /// [`Allowed`]. A look is one, of up to 3 seconds, as the alert opens.
    pub fn start(ways: Ways, user: &str, session: Session, proxy: &Proxy) -> Waits {
        let key = ways.key.then(|| {
            let proxy = proxy.clone();
            olo_fido::touch(
                user.to_owned(),
                || {},
                move |r| {
                    if r.is_ok() {
                        proxy.send(Allowed { session, how: "your security key" });
                    }
                },
            )
        });
        let finger = ways.finger.then(|| {
            let stop = fingerprint::Stop::default();
            let (user, s, proxy) = (user.to_owned(), stop.clone(), proxy.clone());
            std::thread::Builder::new()
                .name("fingerprint".into())
                .spawn(move || match fingerprint::verify(&user, &s, |_| {}) {
                    Ok(true) => proxy.send(Allowed { session, how: "Touch ID" }),
                    Ok(false) => {}
                    Err(e) => tracing::info!("fingerprint: {e}"),
                })
                .expect("thread");
            let s = stop.clone();
            std::thread::Builder::new()
                .name("fingerprint-limit".into())
                .spawn(move || {
                    std::thread::sleep(GIVE_UP);
                    if !s.is_stopped() {
                        s.stop();
                    }
                })
                .expect("thread");
            stop
        });
        if ways.face {
            let proxy = proxy.clone();
            std::thread::Builder::new()
                .name("face".into())
                .spawn(move || {
                    if face::verify(3000) == face::Look::Match {
                        proxy.send(Allowed { session, how: "a look" });
                    }
                })
                .expect("thread");
        }
        Waits { key, finger }
    }
}

impl Drop for Waits {
    fn drop(&mut self) {
        self.key = None;
        if let Some(stop) = self.finger.take()
            && !stop.is_stopped()
        {
            // fprintd's VerifyStop is a D-Bus call: not on the UI thread.
            std::thread::spawn(move || stop.stop());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Ways;

    #[test]
    fn says_what_there_is() {
        assert_eq!(Ways::default().sentence(), None);
        assert_eq!(Ways { key: true, ..Ways::default() }.sentence().as_deref(), Some("To allow it, you can also touch your security key."));
        assert_eq!(Ways { finger: true, face: true, key: false }.sentence().as_deref(), Some("To allow it, you can also use Touch ID or look at the camera."));
        assert_eq!(Ways { finger: true, key: true, face: true }.sentence().as_deref(), Some("To allow it, you can also use Touch ID, touch your security key or look at the camera."));
    }
}
