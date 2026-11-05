//! FIDO2 security keys (a YubiKey and the like), natively: CTAPHID over
//! the key's hidraw node (`hid`, `ctap`), the CTAP2 Olo uses (`cred`,
//! `pin`), and the account password sealed by a key's `hmac-secret`
//! (`seal`), so a touch of the key can stand for typing it: Terminal's
//! sudo window, Passwords, the polkit agent and the lock screen ask
//! [`unlock`] beside their password field.
//!
//! Setting a key up ([`set_up`]) takes the password once (checked by the
//! caller, through PAM) and two touches: a credential made for Olo on the
//! key (not discoverable, `hmac-secret` on), then its secret for a random
//! salt, which seals the password (XChaCha20-Poly1305). Unlocking finds
//! the key holding one of the credentials set up (asked without a touch),
//! asks it for that secret (a touch) and opens the password. The key's
//! FIDO interface is open to the person at their seat (systemd's
//! uaccess), so none of this needs root, and nothing of it runs without
//! a touch of the key itself.
//!
//! Everything here blocks for as long as the key waits for its touch (up
//! to 30 seconds): call it on a worker thread, with a `cancel` flag the
//! window sets when it goes.

pub mod cred;
pub mod ctap;
pub mod hid;
pub mod pin;
pub mod seal;
pub mod soft;

use std::{
    fmt,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

use zeroize::Zeroizing;

pub use ctap::Info;
pub use hid::Device;
pub use seal::Sealed;

use ctap::{Channel, Wait};
use hid::Transport;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Timeout,
    Cancelled,
    /// A CTAPHID error.
    Hid(u8),
    /// A CTAP2 status.
    Ctap(u8),
    Cbor,
    Protocol(&'static str),
    Random,
    /// The key can't make a credential with a secret (no `hmac-secret`).
    NoHmacSecret,
    /// The key has a PIN, which making a credential needs.
    PinNeeded,
    /// No key is set up.
    NotSetUp,
    /// No key set up is plugged in.
    NoKey,
    /// The secret didn't open the password: another key, or the file
    /// changed.
    WrongKey,
    Damaged,
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) if e.kind() == std::io::ErrorKind::PermissionDenied => write!(f, "Olo isn't allowed to open the key (its device node isn't yours)."),
            Error::Io(e) if e.kind() == std::io::ErrorKind::NotConnected => write!(f, "The key was unplugged."),
            Error::Io(e) => write!(f, "Couldn't talk to the key: {e}."),
            Error::Timeout => write!(f, "The key didn't answer in time."),
            Error::Cancelled => write!(f, "Cancelled."),
            Error::Hid(ctap::HID_ERR_BUSY) => write!(f, "The key is busy with another request."),
            Error::Hid(c) => write!(f, "The key had a USB error ({c:#04x})."),
            Error::Ctap(ctap::ERR_USER_ACTION_TIMEOUT) => write!(f, "The key wasn't touched in time."),
            Error::Ctap(ctap::ERR_OPERATION_DENIED) => write!(f, "The key said no."),
            Error::Ctap(ctap::ERR_PIN_REQUIRED) => write!(f, "The key wants its PIN, which Olo doesn't ask for yet."),
            Error::Ctap(c) => write!(f, "The key refused ({c:#04x})."),
            Error::Cbor => write!(f, "The key's answer couldn't be read."),
            Error::Protocol(what) => write!(f, "The key answered something unexpected ({what})."),
            Error::Random => write!(f, "No random numbers to be had."),
            Error::NoHmacSecret => write!(f, "This key can't keep a secret for Olo (it has no hmac-secret)."),
            Error::PinNeeded => write!(f, "This key has a PIN, which Olo doesn't ask for yet."),
            Error::NotSetUp => write!(f, "No security key is set up."),
            Error::NoKey => write!(f, "None of the security keys set up is plugged in."),
            Error::WrongKey => write!(f, "That key doesn't open the password kept for it."),
            Error::Damaged => write!(f, "The security keys file is damaged."),
        }
    }
}

impl std::error::Error for Error {}

/// A key, real or the software one.
type Key = Box<dyn Transport>;

impl Transport for Box<dyn Transport> {
    fn send(&mut self, r: &[u8; hid::REPORT]) -> std::io::Result<()> {
        (**self).send(r)
    }
    fn recv(&mut self, wait: Duration) -> std::io::Result<Option<[u8; hid::REPORT]>> {
        (**self).recv(wait)
    }
}

/// Every key plugged in (debug builds' `OLO_FIDO_SOFT` key instead, if
/// set). Reads /sys only.
pub fn devices() -> Vec<Device> {
    if soft_set() {
        return vec![soft::Soft::device()];
    }
    hid::devices()
}

fn soft_set() -> bool {
    cfg!(debug_assertions) && std::env::var_os("OLO_FIDO_SOFT").is_some()
}

fn open(d: &Device) -> Result<Channel<Key>> {
    let key: Key = if d.path.as_os_str() == "soft" { Box::new(soft::Soft::from_env().ok_or(Error::NoKey)?) } else { Box::new(hid::Hidraw::open(d)?) };
    Channel::open(key)
}

/// A key plugged in, as Preferences shows it.
#[derive(Debug)]
pub struct Plugged {
    pub device: Device,
    /// What it says of itself, or why it couldn't be asked.
    pub info: std::result::Result<Info, String>,
    /// Whether one of `user`'s keys set up is this one.
    pub set_up: bool,
}

/// Each key plugged in: what it is, and whether it's set up for `user`.
/// Asks each key (no touch); blocks a moment.
pub fn plugged(user: &str) -> Vec<Plugged> {
    let keys = seal::load(user);
    devices()
        .into_iter()
        .map(|device| {
            let asked = open(&device).and_then(|mut ch| {
                let info = ch.get_info()?;
                let never = AtomicBool::new(false);
                let wait = Wait { cancel: &never, touch: &|| {}, limit: Duration::from_secs(3) };
                let mut set_up = false;
                for k in &keys {
                    if cred::holds(&mut ch, &k.credential_id()?, &wait)? {
                        set_up = true;
                        break;
                    }
                }
                Ok((info, set_up))
            });
            match asked {
                Ok((info, set_up)) => Plugged { device, info: Ok(info), set_up },
                Err(e) => Plugged { device, info: Err(e.to_string()), set_up: false },
            }
        })
        .collect()
}

/// Set key `device` up for `user`'s `password` (checked already): two
/// touches, `touch` called as each is wanted. The caller keeps the
/// result (`seal::save`).
pub fn set_up(device: &Device, user: &str, password: &str, cancel: &AtomicBool, touch: &(dyn Fn() + Sync)) -> Result<Sealed> {
    let wait = Wait { cancel, touch, limit: Duration::from_secs(40) };
    let mut ch = busy_retry(cancel, || open(device))?;
    let info = ch.get_info()?;
    if !info.hmac_secret() {
        return Err(Error::NoHmacSecret);
    }
    if info.make_credential_needs_pin() {
        return Err(Error::PinNeeded);
    }
    let made = cred::make(&mut ch, user, &wait)?;
    let mut salt = [0u8; 32];
    getrandom::fill(&mut salt).map_err(|_| Error::Random)?;
    let secret = cred::secret(&mut ch, info.pin_protocol(), &made.id, &salt, &wait)?;
    seal::seal(&device.name, &made.aaguid, user, &made.id, &salt, &secret, password)
}

/// Whether a touch could stand for `user`'s password now: a key set up,
/// and a key plugged in (not asked: reads /sys and the file only).
pub fn ready(user: &str) -> bool {
    !seal::load(user).is_empty() && !devices().is_empty()
}

/// `user`'s password, from a key set up for it, after a touch (`touch`
/// called as it's wanted). Blocks till then, `cancel`, or the key giving
/// up (about 30 seconds).
pub fn unlock(user: &str, cancel: &AtomicBool, touch: &(dyn Fn() + Sync)) -> Result<Zeroizing<String>> {
    let keys = seal::load(user);
    if keys.is_empty() {
        return Err(Error::NotSetUp);
    }
    let wait = Wait { cancel, touch, limit: Duration::from_secs(40) };
    let mut last = Error::NoKey;
    for device in devices() {
        let found = busy_retry(cancel, || {
            let mut ch = open(&device)?;
            let info = ch.get_info()?;
            for k in &keys {
                if cred::holds(&mut ch, &k.credential_id()?, &wait)? {
                    let secret = cred::secret(&mut ch, info.pin_protocol(), &k.credential_id()?, &k.salt()?, &wait)?;
                    return seal::open(k, &secret).map(Some);
                }
            }
            Ok(None)
        });
        match found {
            Ok(Some(password)) => return Ok(password),
            Ok(None) => {}
            Err(e @ (Error::Cancelled | Error::Ctap(_))) => return Err(e),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// What a window asking for the password says while a key could answer
/// for it.
pub const TOUCH_OR_TYPE: &str = "Touch your security key, or type your password.";

/// A touch of a key being waited for, beside a password field; dropping
/// it (the window going, the password typed) stops the wait.
pub struct Touch {
    cancel: std::sync::Arc<AtomicBool>,
}

impl Touch {
    /// Stop waiting.
    pub fn stop(&self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Drop for Touch {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Wait, on a thread of its own, for a touch of one of `user`'s keys set
/// up: if one is plugged in, `waiting` is called (on that thread) as the
/// wait begins (for the window to say `TOUCH_OR_TYPE`), then `done` with
/// the password or why not. Nothing is called if no key is set up and
/// plugged in, or once stopped.
pub fn touch(user: String, waiting: impl FnOnce() + Send + 'static, done: impl FnOnce(Result<Zeroizing<String>>) + Send + 'static) -> Touch {
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let stop = cancel.clone();
    let spawned = std::thread::Builder::new().name("security-key".into()).spawn(move || {
        if !ready(&user) {
            return;
        }
        waiting();
        let r = unlock(&user, &stop, &|| {});
        if !stop.load(std::sync::atomic::Ordering::Relaxed) && !matches!(r, Err(Error::Cancelled)) {
            done(r);
        }
    });
    if let Err(e) = spawned {
        tracing::warn!(?e, "no thread to wait for a security key on");
    }
    Touch { cancel }
}

/// `f` again while another program has the key (CTAPHID's busy), for up
/// to five seconds.
fn busy_retry<T>(cancel: &AtomicBool, mut f: impl FnMut() -> Result<T>) -> Result<T> {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        match f() {
            Err(Error::Hid(ctap::HID_ERR_BUSY)) if Instant::now() < until => {
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            r => return r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn soft(master: u8) -> Channel<soft::Soft> {
        Channel::open(soft::Soft::new([master; 32], Some(Duration::from_millis(30)))).unwrap()
    }

    #[test]
    fn info_from_the_soft_key() {
        let mut ch = soft(1);
        let info = ch.get_info().unwrap();
        assert!(info.hmac_secret() && !info.pin_set() && !info.make_credential_needs_pin());
        assert_eq!((info.pin_protocol(), info.firmware_text().as_deref()), (2, Some("5.8.0")));
        assert_eq!(ch.ping(b"olo").unwrap(), b"olo");
    }

    #[test]
    fn set_up_then_unlocked_by_its_key_only() {
        let never = AtomicBool::new(false);
        let touches = AtomicUsize::new(0);
        let touch = || {
            touches.fetch_add(1, Ordering::Relaxed);
        };
        let wait = Wait { cancel: &never, touch: &touch, limit: Duration::from_secs(5) };
        for protocol in [1, 2] {
            let mut ch = soft(2);
            let made = cred::make(&mut ch, "ben", &wait).unwrap();
            let salt = [5u8; 32];
            let secret = cred::secret(&mut ch, protocol, &made.id, &salt, &wait).unwrap();
            let s = seal::seal("Olo Soft Key", &made.aaguid, "ben", &made.id, &salt, &secret, "correct horse").unwrap();
            // The same key, another channel (another process): the same
            // secret, and the password.
            let mut again = soft(2);
            assert!(cred::holds(&mut again, &made.id, &wait).unwrap());
            let secret2 = cred::secret(&mut again, protocol, &s.credential_id().unwrap(), &s.salt().unwrap(), &wait).unwrap();
            assert_eq!(*secret, *secret2);
            assert_eq!(&*seal::open(&s, &secret2).unwrap(), "correct horse");
            // Another key doesn't hold it, and can't give its secret.
            let mut other = soft(3);
            assert!(!cred::holds(&mut other, &made.id, &wait).unwrap());
            assert!(matches!(cred::secret(&mut other, protocol, &made.id, &salt, &wait), Err(Error::Ctap(ctap::ERR_NO_CREDENTIALS))));
            // Another salt: another secret.
            let mut third = soft(2);
            assert_ne!(*cred::secret(&mut third, protocol, &made.id, &[6u8; 32], &wait).unwrap(), *secret);
        }
        // A touch asked for each make and each secret (not for `holds`).
        assert_eq!(touches.load(Ordering::Relaxed), 8);
    }

    #[test]
    fn nothing_waited_for_without_a_key_set_up() {
        // (No key set up for a user that isn't: neither callback.)
        let (tx, rx) = std::sync::mpsc::channel::<&str>();
        let tx2 = tx.clone();
        let t = touch("olo-nobody-at-all".into(), move || tx.send("waiting").unwrap(), move |_| tx2.send("done").unwrap());
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        drop(t);
    }

    #[test]
    fn cancelled_while_waiting_for_a_touch() {
        let cancel = AtomicBool::new(false);
        let mut ch = Channel::open(soft::Soft::new([4; 32], None)).unwrap();
        let start = Instant::now();
        let touch = || cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        let wait = Wait { cancel: &cancel, touch: &touch, limit: Duration::from_secs(10) };
        // The window going away as the key starts waiting: cancelled at once.
        assert!(matches!(cred::make(&mut ch, "ben", &wait), Err(Error::Cancelled)));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
