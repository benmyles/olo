//! Fingerprint verification through fprintd (Touch ID on Apple silicon),
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
    Ready { swipe: bool },
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

    fn stopped(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

fn device() -> Result<Proxy<'static>> {
    let conn = bus::system()?;
    let path: zvariant::OwnedObjectPath =
        bus::call(conn, DEST, "/net/reactivated/Fprint/Manager", "net.reactivated.Fprint.Manager", "GetDefaultDevice", &())?;
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
                    r if r.starts_with("verify-") && !matches!(r, "verify-disconnected" | "verify-unknown-error") => {
                        on(Event::Retry(describe(r).into()))
                    }
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
    fn words() {
        assert_eq!(describe("verify-retry-scan"), "Try again");
        assert_eq!(describe("verify-no-match"), "Fingerprint not recognised");
        assert_eq!(describe("something-new"), "The fingerprint sensor isn't working");
    }
}
