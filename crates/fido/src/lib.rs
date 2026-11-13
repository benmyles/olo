//! FIDO2 security keys (a YubiKey and the like), natively: CTAPHID over
//! the key's hidraw node (`hid`, `ctap`), the CTAP2 Olo uses (`cred`,
//! `pin`), and the account password sealed by a key's `hmac-secret`
//! (`seal`), so a touch of the key can stand for typing it: Terminal's
//! sudo window, Passwords and the polkit agent ask [`unlock`] beside
//! their password field. Never the lock screen or the login screen: a
//! key left in a computer someone finds mustn't let them in.
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
    /// The key has a PIN, which making a credential needs: asked for.
    PinNeeded,
    /// The PIN wasn't the key's (how many more it takes, if it said).
    PinWrong(Option<u32>),
    /// The key wants its PIN at every use (`alwaysUv`), which a touch
    /// standing for a password can't have.
    PinEveryTime,
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
            // (A YubiKey gives up on a touch with "operation denied".)
            Error::Ctap(ctap::ERR_USER_ACTION_TIMEOUT | ctap::ERR_OPERATION_DENIED) => write!(f, "The key wasn't touched in time."),
            Error::Ctap(ctap::ERR_PIN_REQUIRED) => write!(f, "The key wants its PIN."),
            Error::Ctap(ctap::ERR_PIN_BLOCKED) => write!(f, "The key's PIN is blocked after too many wrong tries: it can only be reset (which erases the key)."),
            Error::Ctap(ctap::ERR_PIN_AUTH_BLOCKED) => write!(f, "The key stopped taking PINs for now: unplug it, plug it in again and try once more."),
            Error::Ctap(ctap::ERR_PIN_POLICY_VIOLATION) => write!(f, "The key doesn't take that PIN (too short?)."),
            Error::Ctap(c) => write!(f, "The key refused ({c:#04x})."),
            Error::Cbor => write!(f, "The key's answer couldn't be read."),
            Error::Protocol(what) => write!(f, "The key answered something unexpected ({what})."),
            Error::Random => write!(f, "No random numbers to be had."),
            Error::NoHmacSecret => write!(f, "This key can't keep a secret for Olo (it has no hmac-secret)."),
            Error::PinNeeded => write!(f, "This key has a PIN: type it to set the key up."),
            Error::PinWrong(Some(1)) => write!(f, "That isn't the key's PIN. One more wrong PIN blocks it."),
            Error::PinWrong(Some(n)) => write!(f, "That isn't the key's PIN ({n} tries left)."),
            Error::PinWrong(None) => write!(f, "That isn't the key's PIN."),
            Error::PinEveryTime => write!(f, "This key asks for its PIN at every use, so a touch can't stand for your password."),
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

/// How many times setting up asks for each touch: the key stops waiting
/// for one after about 30 seconds (a YubiKey answers "operation denied"),
/// and someone finding where to touch it may need longer (Ben's first
/// setup ran out while he touched the wrong part of his key).
pub const TOUCH_TRIES: u32 = 3;

/// Which touch setting up wants, as its key starts waiting for it: the
/// first (a credential made on the key) or the second (its secret, which
/// locks the password), and whether it's asked again (the key stopped
/// waiting with no touch).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asked {
    pub touch: u8,
    pub again: bool,
}

/// `f` (a request a touch answers), asked again when the key stops
/// waiting for its touch, [`TOUCH_TRIES`] in all, `touch` told each time.
fn touched<T>(which: u8, cancel: &AtomicBool, touch: &(dyn Fn(Asked) + Sync), mut f: impl FnMut(&Wait) -> Result<T>) -> Result<T> {
    let mut n = 0;
    loop {
        let say = move || touch(Asked { touch: which, again: n > 0 });
        let wait = Wait { cancel, touch: &say, limit: Duration::from_secs(40) };
        match f(&wait) {
            Err(Error::Ctap(ctap::ERR_OPERATION_DENIED | ctap::ERR_USER_ACTION_TIMEOUT)) if n + 1 < TOUCH_TRIES && !cancel.load(std::sync::atomic::Ordering::Relaxed) => n += 1,
            r => return r,
        }
    }
}

/// Set key `device` up for `user`'s `password` (checked already): two
/// touches, `touch` told as each is wanted (each asked again if the key
/// stops waiting for it). A key with a PIN takes `pin` (once: the touches
/// after need none); without it, `Error::PinNeeded`. The caller keeps the
/// result (`seal::save`).
pub fn set_up(device: &Device, user: &str, password: &str, pin: Option<&str>, cancel: &AtomicBool, touch: &(dyn Fn(Asked) + Sync)) -> Result<Sealed> {
    let mut ch = busy_retry(cancel, || open(device))?;
    let info = ch.get_info()?;
    if !info.hmac_secret() {
        return Err(Error::NoHmacSecret);
    }
    if info.option("alwaysUv") == Some(true) {
        return Err(Error::PinEveryTime);
    }
    let token = match (info.make_credential_needs_pin(), pin) {
        (false, _) => None,
        (true, None) => return Err(Error::PinNeeded),
        (true, Some(pin)) => {
            let wait = Wait { cancel, touch: &|| {}, limit: Duration::from_secs(10) };
            Some(pin::token(&mut ch, info.pin_protocol(), info.option("pinUvAuthToken") == Some(true), pin, cred::RP, &wait)?)
        }
    };
    let made = touched(1, cancel, touch, |wait| cred::make(&mut ch, user, token.as_ref(), wait))?;
    let mut salt = [0u8; 32];
    getrandom::fill(&mut salt).map_err(|_| Error::Random)?;
    let secret = touched(2, cancel, touch, |wait| cred::secret(&mut ch, info.pin_protocol(), &made.id, &salt, wait))?;
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
            let made = cred::make(&mut ch, "ben", None, &wait).unwrap();
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

    /// A touch the key stopped waiting for is asked for again, up to
    /// TOUCH_TRIES times, each said (which touch, and again); then it
    /// gives up, saying the key wasn't touched in time.
    #[test]
    fn a_missed_touch_is_asked_for_again() {
        let never = AtomicBool::new(false);
        let asked = std::sync::Mutex::new(Vec::new());
        let touch = |a: Asked| asked.lock().unwrap().push(a);
        let mut key = soft::Soft::new([4; 32], Some(Duration::from_millis(30)));
        key.misses = 2;
        let mut ch = Channel::open(key).unwrap();
        let made = touched(1, &never, &touch, |w| cred::make(&mut ch, "ben", None, w)).unwrap();
        touched(2, &never, &touch, |w| cred::secret(&mut ch, 2, &made.id, &[1; 32], w)).unwrap();
        // (The two missed asked no touch of their own: answered at once.)
        assert_eq!(*asked.lock().unwrap(), [Asked { touch: 1, again: true }, Asked { touch: 2, again: false }]);
        // Missed every time: given up, as one not touched in time.
        let mut key = soft::Soft::new([4; 32], Some(Duration::from_millis(30)));
        key.misses = TOUCH_TRIES as usize;
        let mut ch = Channel::open(key).unwrap();
        let e = touched(1, &never, &touch, |w| cred::make(&mut ch, "ben", None, w)).unwrap_err();
        assert!(matches!(e, Error::Ctap(ctap::ERR_OPERATION_DENIED)) && e.to_string() == "The key wasn't touched in time.", "{e:?}");
    }

    /// A key with a PIN: its credential made only with the PIN's token
    /// (2.0's getPinToken and 2.1's with permissions, both protocols); a
    /// wrong PIN said with the tries left, blocked after the last; the
    /// secret after needing no PIN.
    #[test]
    fn a_key_with_a_pin() {
        let never = AtomicBool::new(false);
        let wait = Wait { cancel: &never, touch: &|| {}, limit: Duration::from_secs(5) };
        for (protocol, permissions) in [(1, false), (2, false), (2, true)] {
            let mut ch = Channel::open(soft::Soft::new([7; 32], Some(Duration::from_millis(10))).with_pin("2468")).unwrap();
            let info = ch.get_info().unwrap();
            assert!(info.pin_set() && info.make_credential_needs_pin());
            // Without its PIN: refused.
            assert!(matches!(cred::make(&mut ch, "ben", None, &wait), Err(Error::Ctap(ctap::ERR_PIN_REQUIRED))));
            // A wrong PIN: said, with the tries left.
            let e = pin::token(&mut ch, protocol, permissions, "1357", cred::RP, &wait).err().unwrap();
            assert!(matches!(e, Error::PinWrong(Some(7))), "{e:?}");
            assert_eq!(e.to_string(), "That isn't the key's PIN (7 tries left).");
            // Its PIN: a token that makes the credential, whose secret then
            // needs no PIN.
            let t = pin::token(&mut ch, protocol, permissions, "2468", cred::RP, &wait).unwrap();
            let made = cred::make(&mut ch, "ben", Some(&t), &wait).unwrap();
            let secret = cred::secret(&mut ch, protocol, &made.id, &[3; 32], &wait).unwrap();
            let mut again = Channel::open(soft::Soft::new([7; 32], Some(Duration::from_millis(10))).with_pin("2468")).unwrap();
            assert_eq!(*cred::secret(&mut again, protocol, &made.id, &[3; 32], &wait).unwrap(), *secret);
        }
        // Eight wrong PINs: blocked, and said so.
        let mut ch = Channel::open(soft::Soft::new([7; 32], None).with_pin("2468")).unwrap();
        for left in (1..8).rev() {
            assert!(matches!(pin::token(&mut ch, 2, true, "0000", cred::RP, &wait), Err(Error::PinWrong(Some(n))) if n == left));
        }
        let e = pin::token(&mut ch, 2, true, "0000", cred::RP, &wait).err().unwrap();
        assert!(matches!(e, Error::Ctap(ctap::ERR_PIN_BLOCKED)), "{e:?}");
        assert!(e.to_string().contains("blocked"));
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
        assert!(matches!(cred::make(&mut ch, "ben", None, &wait), Err(Error::Cancelled)));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
