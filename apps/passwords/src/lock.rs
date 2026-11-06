//! Locking: Passwords opens locked and locks again when idle. Touch ID
//! (fprintd) unlocks it, a touch of a security key set up (`olo_fido`:
//! the password it keeps, checked as one typed), a look (face unlock,
//! `olo_system::face`: olo-faced looks, once each time the window takes
//! the keyboard), or the account password (PAM), each on a thread. The
//! sensor, the key and the camera are waited on only while the locked
//! window has the keyboard, so none is held from sudo or the lock screen.

use olo_system::{auth, fingerprint};
use olo_ui::Proxy;
use zeroize::{Zeroize, Zeroizing};

pub enum LockMsg {
    Finger(fingerprint::Event),
    /// No sensor, no enrolled finger, or it failed.
    NoFinger,
    Password(Result<(), auth::AuthError>),
    /// A security key set up is plugged in and waits for a touch.
    KeyWaiting,
    /// The key's password, or why not.
    Key(Result<Zeroizing<String>, String>),
    /// How a look went.
    Face(olo_system::face::Look),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Finger {
    /// Not looked for yet.
    Idle,
    /// Verifying, with what to show.
    Listening(String),
    /// Unavailable: password only.
    Off,
}

pub struct Lock {
    pub locked: bool,
    pub password: String,
    pub checking: bool,
    pub error: Option<String>,
    pub finger: Finger,
    stop: Option<fingerprint::Stop>,
    /// A touch of a security key waited for, and what to say of it.
    key: Option<olo_fido::Touch>,
    pub key_note: Option<String>,
    /// A look asked for since the window last took the keyboard.
    looked: bool,
    user: String,
    proxy: Proxy,
}

impl Lock {
    pub fn new(proxy: Proxy, locked: bool) -> Lock {
        let finger = if std::env::var_os("OLO_NO_FINGERPRINT").is_some() { Finger::Off } else { Finger::Idle };
        Lock { locked, password: String::new(), checking: false, error: None, finger, stop: None, key: None, key_note: None, looked: false, user: auth::current_user().unwrap_or_default(), proxy }
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    pub fn lock(&mut self) {
        self.locked = true;
        self.error = None;
        self.password.zeroize();
        self.password.clear();
    }

    /// Listen for a finger, and wait for a key, while the locked window
    /// has the keyboard.
    pub fn focus(&mut self, focused: bool) {
        match (focused && self.locked, &self.stop) {
            (true, None) if self.finger != Finger::Off => self.start_fingerprint(),
            (false, Some(_)) => self.stop_fingerprint(),
            _ => {}
        }
        match (focused && self.locked, &self.key) {
            (true, None) => self.wait_for_key(),
            (false, Some(_)) => {
                self.key = None;
                self.key_note = None;
            }
            _ => {}
        }
        // A look, once each time it takes the keyboard (the camera's
        // light isn't on and off as the window is).
        if focused && self.locked && !self.looked {
            self.looked = true;
            let proxy = self.proxy.clone();
            std::thread::Builder::new().name("face".into()).spawn(move || proxy.send(LockMsg::Face(olo_system::face::verify(3000)))).expect("thread");
        } else if !focused {
            self.looked = false;
        }
    }

    fn wait_for_key(&mut self) {
        let (p, proxy) = (self.proxy.clone(), self.proxy.clone());
        self.key = Some(olo_fido::touch(self.user.clone(), move || p.send(LockMsg::KeyWaiting), move |r| proxy.send(LockMsg::Key(r.map_err(|e| e.to_string())))));
    }

    fn start_fingerprint(&mut self) {
        let stop = fingerprint::Stop::default();
        self.stop = Some(stop.clone());
        let (user, proxy) = (self.user.clone(), self.proxy.clone());
        std::thread::Builder::new()
            .name("fingerprint".into())
            .spawn(move || {
                if !matches!(fingerprint::enrolled(&user), Ok(true)) {
                    proxy.send(LockMsg::NoFinger);
                    return;
                }
                let p = proxy.clone();
                if let Err(e) = fingerprint::verify(&user, &stop, move |e| p.send(LockMsg::Finger(e))) {
                    tracing::info!("fingerprint: {e}");
                    proxy.send(LockMsg::NoFinger);
                }
            })
            .expect("thread");
    }

    fn stop_fingerprint(&mut self) {
        if let Some(stop) = self.stop.take() {
            // fprintd's VerifyStop is a D-Bus call: not on the UI thread.
            std::thread::spawn(move || stop.stop());
        }
        if self.finger != Finger::Off {
            self.finger = Finger::Idle;
        }
    }

    pub fn submit(&mut self) {
        if self.checking || self.password.is_empty() {
            return;
        }
        self.checking = true;
        self.error = None;
        let (user, mut password, proxy) = (self.user.clone(), std::mem::take(&mut self.password), self.proxy.clone());
        std::thread::Builder::new()
            .name("pam".into())
            .spawn(move || {
                let result = auth::check_password(&user, &password);
                password.zeroize();
                proxy.send(LockMsg::Password(result));
            })
            .expect("thread");
    }

    /// Returns true when this unlocked it.
    pub fn message(&mut self, msg: LockMsg) -> bool {
        match msg {
            LockMsg::Finger(fingerprint::Event::Match) => return self.unlock(),
            LockMsg::Finger(fingerprint::Event::Ready { swipe }) => self.finger = Finger::Listening(if swipe { "Swipe your finger" } else { "Touch ID or enter your password" }.into()),
            LockMsg::Finger(fingerprint::Event::Retry(s)) => self.finger = Finger::Listening(s),
            LockMsg::Finger(fingerprint::Event::NoMatch) => self.finger = Finger::Listening("Not recognised. Try again".into()),
            LockMsg::Finger(fingerprint::Event::Failed(_)) | LockMsg::NoFinger => {
                self.stop = None;
                self.finger = Finger::Off;
            }
            LockMsg::Password(Ok(())) => {
                self.checking = false;
                return self.unlock();
            }
            LockMsg::Password(Err(e)) => {
                self.checking = false;
                self.error = Some(match e {
                    auth::AuthError::Denied(_) => "That password isn’t right.".into(),
                    other => other.to_string(),
                });
            }
            LockMsg::KeyWaiting => self.key_note = Some(olo_fido::TOUCH_OR_TYPE.into()),
            LockMsg::Key(Ok(password)) => {
                // Checked as one typed (a password changed since the key
                // was set up doesn't do).
                self.key_note = None;
                self.password.zeroize();
                self.password = password.to_string();
                self.submit();
            }
            LockMsg::Key(Err(why)) => {
                self.key = None;
                self.key_note = Some(format!("{why} Type your password."));
            }
            LockMsg::Face(olo_system::face::Look::Match) => return self.unlock(),
            LockMsg::Face(olo_system::face::Look::NotYou) => self.key_note = Some("Your face wasn’t recognised. Type your password.".into()),
            LockMsg::Face(_) => {}
        }
        false
    }

    /// Unlocked while app_server approves without asking; true if it was
    /// locked.
    pub fn approved(&mut self) -> bool {
        self.unlock()
    }

    fn unlock(&mut self) -> bool {
        if !self.locked {
            return false;
        }
        self.locked = false;
        self.error = None;
        self.stop_fingerprint();
        self.key = None;
        self.key_note = None;
        true
    }
}
