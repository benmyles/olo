//! Fingerprint verification and enrolment through fprintd (Touch ID on Apple silicon),
//! over D-Bus. `verify` blocks until a match, a failure or [`Stop`]: run it
//! on its own thread and listen to the events it reports.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use zbus::blocking::Proxy;

use crate::{Result, bus};

const DEST: &str = "net.reactivated.Fprint";
const DEVICE: &str = "net.reactivated.Fprint.Device";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The sensor is listening ("press" or "swipe").
    Ready {
        swipe: bool,
    },
    /// A bad read: say this and keep listening.
    Retry(String),
    /// A finger that isn't enrolled; still listening.
    NoMatch,
    Match,
    /// The sensor gave up (unplugged, an error); stop offering it.
    Failed(String),
}

/// Stops a running [`verify`] (when the password got there first).
#[derive(Clone, Default)]
pub struct Stop(Arc<AtomicBool>);

impl Stop {
    pub fn stop(&self) {
        self.0.store(true, Ordering::SeqCst);
        // Wake verify: stopping the scan makes fprintd report and return.
        if let Ok(dev) = device() {
            let _ = dev.call_method("VerifyStop", &());
        }
    }

    /// Mark it stopped without telling fprintd.
    pub fn stop_flag(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped()
    }

    fn stopped(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// fprintd's bus: the system bus, or (debug builds) a test's own
/// (`OLO_FPRINT_BUS`, an address, as `OLO_FACE_BUS` is the face
/// service's: the `fake-readers` example stands in there).
fn conn() -> Result<&'static zbus::blocking::Connection> {
    if cfg!(debug_assertions)
        && let Ok(address) = std::env::var("OLO_FPRINT_BUS")
        && !address.is_empty()
    {
        static TEST: std::sync::OnceLock<zbus::blocking::Connection> = std::sync::OnceLock::new();
        if let Some(c) = TEST.get() {
            return Ok(c);
        }
        let c = zbus::blocking::connection::Builder::address(address.as_str())?.build()?;
        return Ok(TEST.get_or_init(|| c));
    }
    bus::system()
}

fn device() -> Result<Proxy<'static>> {
    let conn = conn()?;
    let path: zvariant::OwnedObjectPath = bus::call(conn, DEST, "/net/reactivated/Fprint/Manager", "net.reactivated.Fprint.Manager", "GetDefaultDevice", &())?;
    Proxy::new(conn, DEST, path, DEVICE)
}

/// Whether `user` has any enrolled fingerprints on the default sensor.
pub fn enrolled(user: &str) -> Result<bool> {
    let dev = device()?;
    match dev.call::<_, _, Vec<String>>("ListEnrolledFingers", &(user,)) {
        Ok(f) => Ok(!f.is_empty()),
        Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "net.reactivated.Fprint.Error.NoEnrolledPrints" => Ok(false),
        Err(e) => Err(e),
    }
}

/// The fingers `user` has enrolled ("right-index-finger", …).
pub fn fingers(user: &str) -> Result<Vec<String>> {
    match device()?.call::<_, _, Vec<String>>("ListEnrolledFingers", &(user,)) {
        Ok(f) => Ok(f),
        Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "net.reactivated.Fprint.Error.NoEnrolledPrints" => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Whether PAM's message `text` is pam_fprintd's, listening: it says so
/// ("Place your finger on the fingerprint reader", "Swipe your right
/// index finger across …") only once it has claimed the sensor, so a
/// second PAM conversation started after it finds the sensor taken, and
/// its pam_fprintd goes straight on to the password. sudo's window and
/// the polkit agent start one so (a security key, or typing, answers it
/// while the finger is listened for).
pub fn listening(text: &str) -> bool {
    text.to_lowercase().contains("finger")
}

/// Whether there's a fingerprint sensor at all.
pub fn available() -> bool {
    device().is_ok()
}

/// fprintd's finger names, in the order a hand is drawn.
pub const FINGERS: [&str; 10] = [
    "left-little-finger",
    "left-ring-finger",
    "left-middle-finger",
    "left-index-finger",
    "left-thumb",
    "right-thumb",
    "right-index-finger",
    "right-middle-finger",
    "right-ring-finger",
    "right-little-finger",
];

/// "right-index-finger" → "Right index finger".
pub fn finger_name(finger: &str) -> String {
    let words = finger.replace('-', " ");
    let mut c = words.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

/// Forget one enrolled finger of `user`.
pub fn delete(user: &str, finger: &str) -> Result<()> {
    let dev = device()?;
    dev.call_method("Claim", &(user,))?;
    let r = dev.call_method("DeleteEnrolledFinger", &(finger,)).map(|_| ());
    let _ = dev.call_method("Release", &());
    r
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnrollEvent {
    /// Listening; this many good touches are needed in all.
    Ready {
        stages: u32,
    },
    /// One more good touch.
    Stage,
    /// A bad touch: say this and keep going.
    Retry(String),
    Done,
    Failed(String),
}

fn describe_enroll(result: &str) -> &'static str {
    match result {
        "enroll-retry-scan" => "Try again",
        "enroll-swipe-too-short" => "Swipe was too short; try again",
        "enroll-finger-not-centered" => "Centre your finger on the sensor",
        "enroll-remove-and-retry" => "Lift your finger and try again",
        "enroll-duplicate" => "That finger is already enrolled",
        "enroll-data-full" => "The sensor has no room for more fingerprints",
        "enroll-disconnected" => "The fingerprint sensor was disconnected",
        _ => "Enrolling didn't work",
    }
}

/// Enrol `finger` for `user`: touch after touch until fprintd has enough
/// (Ok(true)), `stop` is used (Ok(false)) or it fails. Blocks.
pub fn enroll(user: &str, finger: &str, stop: &Stop, mut on: impl FnMut(EnrollEvent)) -> Result<bool> {
    let dev = device()?;
    dev.call_method("Claim", &(user,))?;
    let result = (|| {
        let stages = dev.get_property::<i32>("num-enroll-stages").unwrap_or(0).max(1) as u32;
        let mut signals = dev.receive_signal("EnrollStatus")?;
        dev.call_method("EnrollStart", &(finger,))?;
        on(EnrollEvent::Ready { stages });
        loop {
            let Some(msg) = signals.next() else { return Ok(false) };
            if stop.stopped() {
                return Ok(false);
            }
            let (result, done): (String, bool) = msg.body().deserialize()?;
            match result.as_str() {
                "enroll-completed" => {
                    on(EnrollEvent::Done);
                    return Ok(true);
                }
                "enroll-stage-passed" => on(EnrollEvent::Stage),
                r @ ("enroll-retry-scan" | "enroll-swipe-too-short" | "enroll-finger-not-centered" | "enroll-remove-and-retry") => on(EnrollEvent::Retry(describe_enroll(r).into())),
                r => {
                    on(EnrollEvent::Failed(describe_enroll(r).into()));
                    return Ok(false);
                }
            }
            if done {
                return Ok(false);
            }
        }
    })();
    let _ = dev.call_method("EnrollStop", &());
    let _ = dev.call_method("Release", &());
    result
}

/// Stop a running [`enroll`].
pub fn stop_enroll(stop: &Stop) {
    stop.0.store(true, Ordering::SeqCst);
    if let Ok(dev) = device() {
        let _ = dev.call_method("EnrollStop", &());
    }
}

/// Plain words for fprintd's results.
pub fn describe(result: &str) -> &'static str {
    match result {
        "verify-retry-scan" => "Try again",
        "verify-swipe-too-short" => "Swipe was too short; try again",
        "verify-finger-not-centered" => "Centre your finger on the sensor",
        "verify-remove-and-retry" => "Lift your finger and try again",
        "verify-no-match" => "Fingerprint not recognised",
        "verify-disconnected" => "The fingerprint sensor was disconnected",
        _ => "The fingerprint sensor isn't working",
    }
}

/// The sensor, claimed and not listening, until this is dropped: so a
/// PAM stack with pam_fprintd first (sudo's) can't have it and goes
/// straight on to the password.
pub struct Held(Proxy<'static>);

impl Drop for Held {
    fn drop(&mut self) {
        let _ = self.0.call_method("Release", &());
    }
}

/// Claim the sensor for `user` (see [`Held`]).
pub fn hold(user: &str) -> Result<Held> {
    let dev = device()?;
    dev.call_method("Claim", &(user,))?;
    Ok(Held(dev))
}

/// Listen for an enrolled finger of `user` until it matches (Ok(true)),
/// `stop` is used (Ok(false)) or the sensor fails.
pub fn verify(user: &str, stop: &Stop, mut on: impl FnMut(Event)) -> Result<bool> {
    let dev = device()?;
    dev.call_method("Claim", &(user,))?;
    let result = (|| {
        let swipe = dev.get_property::<String>("scan-type").is_ok_and(|s| s == "swipe");
        let mut signals = dev.receive_signal("VerifyStatus")?;
        while !stop.stopped() {
            dev.call_method("VerifyStart", &("any",))?;
            on(Event::Ready { swipe });
            // One scan: statuses until `done`.
            loop {
                let Some(msg) = signals.next() else { return Ok(false) };
                if stop.stopped() {
                    return Ok(false);
                }
                let (result, done): (String, bool) = msg.body().deserialize()?;
                match result.as_str() {
                    "verify-match" => {
                        on(Event::Match);
                        return Ok(true);
                    }
                    "verify-no-match" => on(Event::NoMatch),
                    r if r.starts_with("verify-") && !matches!(r, "verify-disconnected" | "verify-unknown-error") => on(Event::Retry(describe(r).into())),
                    r => {
                        on(Event::Failed(describe(r).into()));
                        return Ok(false);
                    }
                }
                if done {
                    break;
                }
            }
            let _ = dev.call_method("VerifyStop", &());
        }
        Ok(false)
    })();
    let _ = dev.call_method("VerifyStop", &());
    let _ = dev.call_method("Release", &());
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Claims the sensor, listens for a moment with no finger, stops and
    /// releases it. Needs permission to verify (root, or an active session).
    #[test]
    #[ignore]
    fn live_start_and_stop() {
        let user = std::env::var("FPRINT_USER").unwrap_or_else(|_| crate::auth::current_user().unwrap());
        assert!(enrolled(&user).unwrap(), "{user} has no fingerprints");
        let stop = Stop::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let s = stop.clone();
        std::thread::spawn(move || {
            let mut events = Vec::new();
            let r = verify(&user, &s, |e| events.push(e));
            let _ = tx.send((r.map_err(|e| e.to_string()), events));
        });
        std::thread::sleep(std::time::Duration::from_millis(1500));
        stop.stop();
        let (r, events) = rx.recv_timeout(std::time::Duration::from_secs(5)).expect("verify returned after stop");
        eprintln!("{r:?} {events:?}");
        assert_eq!(r, Ok(false));
        assert!(matches!(events.first(), Some(Event::Ready { swipe: false })));
        // Released: another claim works.
        let dev = device().unwrap();
        dev.call_method("Claim", &(crate::auth::current_user().unwrap(),)).unwrap();
        dev.call_method("Release", &()).unwrap();
    }

    #[test]
    fn finger_names() {
        assert_eq!(finger_name("right-index-finger"), "Right index finger");
        assert_eq!(finger_name("left-thumb"), "Left thumb");
        assert_eq!(FINGERS.len(), 10);
    }

    #[test]
    fn words() {
        assert_eq!(describe("verify-retry-scan"), "Try again");
        assert_eq!(describe("verify-no-match"), "Fingerprint not recognised");
        assert_eq!(describe("something-new"), "The fingerprint sensor isn't working");
    }
}
