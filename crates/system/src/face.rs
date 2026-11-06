//! Face unlock's client: what Passwords, the lock screen and Preferences
//! ask olo-faced (`org.olo.Face1` on the system bus; `OLO_FACE_BUS` in
//! tests). The service knows who's asking from the bus, so each asks
//! about its own user. Everything here blocks: call it from a worker.

use crate::Result;

const NAME: &str = "org.olo.Face1";
const PATH: &str = "/org/olo/Face1";

fn conn() -> Result<zbus::blocking::Connection> {
    match std::env::var("OLO_FACE_BUS") {
        Ok(a) if !a.is_empty() => Ok(zbus::blocking::connection::Builder::address(a.as_str())?.build()?),
        _ => Ok(zbus::blocking::Connection::system()?),
    }
}

fn user() -> String {
    crate::auth::current_user().unwrap_or_default()
}

/// What face unlock has for this user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// The service is installed (it answered).
    pub installed: bool,
    pub camera: bool,
    /// Looks enrolled (0: not set up).
    pub enrolled: usize,
}

/// Face unlock's state for this user (not installed if nothing answers:
/// the service is started by the bus when installed).
pub fn status() -> Status {
    let Ok(c) = conn() else { return Status::default() };
    let Ok(reply) = c.call_method(Some(NAME), PATH, Some(NAME), "Status", &(user(),)) else { return Status::default() };
    let text: String = reply.body().deserialize().unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    Status { installed: true, camera: v["camera"].as_bool().unwrap_or(false), enrolled: v["enrolled"].as_u64().unwrap_or(0) as usize }
}

/// How a look went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Match,
    /// A face, not this user's (or a picture of it).
    NotYou,
    NoFace,
    /// Not set up, no camera, no service.
    Unavailable,
}

/// Look for this user's face for up to `timeout_ms`.
pub fn verify(timeout_ms: u32) -> Look {
    let Ok(c) = conn() else { return Look::Unavailable };
    let Ok(reply) = c.call_method(Some(NAME), PATH, Some(NAME), "Verify", &(user(), timeout_ms)) else { return Look::Unavailable };
    match reply.body().deserialize::<String>().unwrap_or_default().as_str() {
        "match" => Look::Match,
        "no-match" | "flat" => Look::NotYou,
        "no-face" => Look::NoFace,
        _ => Look::Unavailable,
    }
}

/// Enrol this user's face (their `password` first): the service's
/// answer ("enrolled", "wrong-password", "no-face", "no-camera"), each
/// look as it's kept going to `progress` (so many of so many).
pub fn enroll(password: &str, progress: impl Fn(u32, u32) + Send + 'static) -> Result<String> {
    let c = conn()?;
    // The progress signals, heard on a connection of their own.
    let listen = c.clone();
    std::thread::spawn(move || {
        let Ok(proxy) = zbus::blocking::Proxy::new(&listen, NAME, PATH, NAME) else { return };
        let Ok(signals) = proxy.receive_signal("Progress") else { return };
        for m in signals {
            if let Ok((_, got, of)) = m.body().deserialize::<(String, u32, u32)>() {
                progress(got, of);
                if got >= of {
                    return;
                }
            }
        }
    });
    let reply = c.call_method(Some(NAME), PATH, Some(NAME), "Enroll", &(user(), password))?;
    reply.body().deserialize()
}

/// Forget this user's face (their `password` first).
pub fn forget(password: &str) -> Result<String> {
    let reply = conn()?.call_method(Some(NAME), PATH, Some(NAME), "Forget", &(user(), password))?;
    reply.body().deserialize()
}
