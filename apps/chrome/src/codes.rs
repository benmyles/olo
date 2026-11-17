//! One-time codes from the phone (`olo-phone --service`, which finds them
//! in text messages), for the extension to offer in the tab that is
//! waiting for one. The host stays connected to the phone service's
//! socket as a browser (`olo_protocol::phone::Request::Browser`) and
//! carries what each says to the other: the service asks which tabs are
//! waiting when a code comes (`{code: {any: ID}}` to the extension,
//! answered `{code_waiting: {id, tabs}}`), gives the code to the one
//! browser whose tab it chose (`{code: {id, tab, frame, code, from,
//! fill}}`), and says when one has gone (`{code: {gone: ID}}`); the
//! extension says by itself when a code field comes up (`{code_waiting:
//! {id: null, tabs}}`) and what came of a code it was given
//! (`{code_outcome: {id, outcome}}`). The service knows a browser on an
//! agent's screen by this process's `OLO_SCREEN`.
//!
//! Without the service (no real session, an agent's test world) nothing
//! happens: the host waits, asleep, for the service's state file to come
//! or change, and connects then. Nothing here keeps a code: it passes
//! through, and what held it is wiped.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    sync::{Arc, mpsc},
};

use olo_protocol::phone::{FromBrowser, PhoneState, Request, ToBrowser, Waiting};
use olo_sync::Mutex;
use serde_json::{Value, json};
use zeroize::Zeroize;

/// The way to the phone service, while there is one.
#[derive(Clone, Default)]
pub struct Codes {
    to: Arc<Mutex<Option<UnixStream>>>,
}

impl Codes {
    /// Follow the phone service from here on, on a thread of its own:
    /// what it says goes to the extension through `out`.
    pub fn follow(out: Arc<Mutex<std::io::Stdout>>) -> Codes {
        let codes = Codes::default();
        let to = codes.to.clone();
        let _ = std::thread::Builder::new().name("phone-codes".into()).spawn(move || {
            loop {
                if let Ok(stream) = connect() {
                    *to.lock().unwrap_or_else(|e| e.into_inner()) = stream.try_clone().ok();
                    for mut line in BufReader::new(&stream).lines().map_while(Result::ok) {
                        if let Ok(said) = serde_json::from_str::<ToBrowser>(&line) {
                            crate::host::send(&out, &to_extension(said));
                        }
                        line.zeroize();
                    }
                    *to.lock().unwrap_or_else(|e| e.into_inner()) = None;
                }
                // The service isn't there (or has gone): asleep until
                // its state file comes or changes, as it does when it
                // starts.
                let (tx, rx) = mpsc::channel();
                let Some(_watch) = olo_kit_base::watch::watch([PhoneState::path()], move |_| {
                    let _ = tx.send(());
                }) else {
                    return;
                };
                if rx.recv().is_err() {
                    return;
                }
            }
        });
        codes
    }

    /// What the extension says about codes, on to the service. True if
    /// `message` was about codes.
    pub fn from_extension(&self, message: &Value) -> bool {
        let said = if let Some(w) = message.get("code_waiting") {
            let tabs: Vec<Waiting> = w.get("tabs").cloned().and_then(|t| serde_json::from_value(t).ok()).unwrap_or_default();
            FromBrowser::Waiting { id: w.get("id").and_then(Value::as_u64), tabs }
        } else if let Some(o) = message.get("code_outcome") {
            let Some(id) = o.get("id").and_then(Value::as_u64) else { return true };
            FromBrowser::Outcome { id, outcome: o.get("outcome").and_then(Value::as_str).unwrap_or_default().to_owned() }
        } else {
            return false;
        };
        if let Some(stream) = &*self.to.lock().unwrap_or_else(|e| e.into_inner())
            && let Ok(mut line) = serde_json::to_vec(&said)
        {
            line.push(b'\n');
            let _ = (&*stream).write_all(&line);
        }
        true
    }
}

fn connect() -> std::io::Result<UnixStream> {
    let stream = UnixStream::connect(PhoneState::socket())?;
    let hello = Request::Browser { screen: std::env::var("OLO_SCREEN").ok().filter(|s| !s.is_empty()), pid: std::os::unix::process::parent_id() };
    let mut line = serde_json::to_vec(&hello).map_err(std::io::Error::other)?;
    line.push(b'\n');
    (&stream).write_all(&line)?;
    Ok(stream)
}

/// What the service says, as the extension hears it.
fn to_extension(said: ToBrowser) -> Value {
    match said {
        ToBrowser::Any { id } => json!({ "code": { "any": id } }),
        ToBrowser::Code { id, tab, frame, code, from, fill } => json!({ "code": { "id": id, "tab": tab, "frame": frame, "code": code, "from": from, "fill": fill } }),
        ToBrowser::Gone { id } => json!({ "code": { "gone": id } }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_each_says_to_the_other() {
        assert_eq!(to_extension(ToBrowser::Any { id: 4 }), json!({ "code": { "any": 4 } }));
        assert_eq!(
            to_extension(ToBrowser::Code { id: 4, tab: 12, frame: 3, code: "482913".into(), from: "Acme".into(), fill: true }),
            json!({ "code": { "id": 4, "tab": 12, "frame": 3, "code": "482913", "from": "Acme", "fill": true } })
        );
        assert_eq!(to_extension(ToBrowser::Gone { id: 4 }), json!({ "code": { "gone": 4 } }));
        // With no service, what the extension says is taken and goes
        // nowhere; what isn't about codes is left for the rest.
        let codes = Codes::default();
        assert!(codes.from_extension(&json!({ "code_waiting": { "id": null, "tabs": [{ "tab": 1, "host": "a.example" }] } })));
        assert!(codes.from_extension(&json!({ "code_outcome": { "id": 4, "outcome": "filled" } })));
        assert!(!codes.from_extension(&json!({ "id": 9, "request": { "type": "unlock" } })));
    }
}
