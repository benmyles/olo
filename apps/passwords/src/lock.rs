//! Locking: Passwords opens locked and locks again when idle. Touch ID
//! (fprintd) unlocks it, or the account password (PAM), both checked on
//! threads. The sensor is claimed only while the locked window has the
//! keyboard, so it never holds it from sudo or the lock screen.

use olo_kit::Proxy;
use olo_system::{auth, fingerprint};
use zeroize::Zeroize;

pub enum LockMsg {
    Finger(fingerprint::Event),
    /// No sensor, no enrolled finger, or it failed.
    NoFinger,
    Password(Result<(), auth::AuthError>),
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
    user: String,
    proxy: Proxy,
}

impl Lock {
    pub fn new(proxy: Proxy, locked: bool) -> Lock {
        let finger = if std::env::var_os("OLO_NO_FINGERPRINT").is_some() { Finger::Off } else { Finger::Idle };
        Lock {
            locked,
            password: String::new(),
            checking: false,
            error: None,
            finger,
            stop: None,
            user: auth::current_user().unwrap_or_default(),
            proxy,
        }
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

    /// Listen for a finger while the locked window has the keyboard.
    pub fn focus(&mut self, focused: bool) {
        match (focused && self.locked, &self.stop) {
            (true, None) if self.finger != Finger::Off => self.start_fingerprint(),
            (false, Some(_)) => self.stop_fingerprint(),
            _ => {}
        }
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
            LockMsg::Finger(fingerprint::Event::Ready { swipe }) => {
                self.finger = Finger::Listening(if swipe { "Swipe your finger" } else { "Touch ID or enter your password" }.into())
            }
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
        }
        false
    }

    fn unlock(&mut self) -> bool {
        if !self.locked {
            return false;
        }
        self.locked = false;
        self.error = None;
        self.stop_fingerprint();
        true
    }
}
