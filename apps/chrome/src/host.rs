//! The native messaging host Chrome starts for the Tabula Passwords
//! extension (`tabula-chrome chrome-extension://ID/`). Messages are a
//! little-endian u32 length and JSON, on stdin and stdout: `{id, request,
//! start}` (a keychain request; with `start`, Passwords is started first
//! if it isn't running, and a fresh start's lock is waited out) or `{id,
//! open: true}` (start Passwords or bring it forward); each is answered
//! with `{id, response}` or `{id, error}`, on a thread of its own, so a
//! Fill waiting for an unlock holds up nothing else. Only what a browser
//! needs passes: logins, fill, check, save, unlock and show.

use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tabula_keychain::{CallError, Request, Response};
use zeroize::Zeroize;

/// The largest message taken from Chrome.
const MAX_IN: usize = 1 << 20;

pub fn run() {
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let mut input = std::io::stdin().lock();
    loop {
        let mut len = [0u8; 4];
        // Chrome closed the pipe: the extension is done with us.
        if input.read_exact(&mut len).is_err() {
            return;
        }
        let n = u32::from_le_bytes(len) as usize;
        if n > MAX_IN {
            eprintln!("tabula-chrome: a message of {n} bytes from Chrome is too big");
            return;
        }
        let mut buf = vec![0u8; n];
        if input.read_exact(&mut buf).is_err() {
            return;
        }
        let message: Result<Value, _> = serde_json::from_slice(&buf);
        buf.zeroize();
        let out = out.clone();
        std::thread::spawn(move || {
            let reply = match message {
                Ok(m) => answer(&m),
                Err(e) => json!({ "id": null, "error": format!("not JSON: {e}") }),
            };
            send(&out, &reply);
        });
    }
}

fn send(out: &Mutex<std::io::Stdout>, reply: &Value) {
    let Ok(mut bytes) = serde_json::to_vec(reply) else { return };
    let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
    let _ = out.write_all(&(bytes.len() as u32).to_le_bytes());
    let _ = out.write_all(&bytes);
    let _ = out.flush();
    bytes.zeroize();
}

fn allowed(r: &Request) -> bool {
    matches!(
        r,
        Request::Logins { .. } | Request::Fill { .. } | Request::Check { .. } | Request::Save { .. } | Request::Unlock | Request::Show { .. }
    )
}

/// Start Passwords (or bring it forward: it is a single instance).
fn open_passwords() -> Result<(), String> {
    let program = tabula_kit::find_program("tabula-passwords").ok_or("Passwords isn't installed.")?;
    tabula_kit::open::spawn(&[program.into_os_string()]).map_err(|e| format!("Passwords couldn't be started: {e}"))
}

fn answer(m: &Value) -> Value {
    let id = m.get("id").cloned().unwrap_or(Value::Null);
    if m.get("open").and_then(Value::as_bool) == Some(true) {
        return match open_passwords() {
            Ok(()) => json!({ "id": id, "response": { "type": "shown" } }),
            Err(e) => json!({ "id": id, "error": e }),
        };
    }
    let request: Request = match m.get("request").cloned().map(serde_json::from_value) {
        Some(Ok(r)) => r,
        Some(Err(e)) => return json!({ "id": id, "error": format!("not a request Passwords knows: {e}") }),
        None => return json!({ "id": id, "error": "no request" }),
    };
    if !allowed(&request) {
        return json!({ "id": id, "error": "not a request for browsers" });
    }
    let start = m.get("start").and_then(Value::as_bool) == Some(true);
    match ask(&request, start) {
        Ok(response) => json!({ "id": id, "response": serde_json::to_value(&response).unwrap_or(Value::Null) }),
        Err(e) => json!({ "id": id, "error": e.to_string() }),
    }
}

/// Ask Passwords; with `start`, start it if it isn't running (then wait
/// for it, and for the unlock it begins with).
fn ask(request: &Request, start: bool) -> Result<Response, CallError> {
    let mut result = tabula_keychain::call(request);
    if !start || !matches!(result, Err(CallError::NotRunning)) {
        return result;
    }
    if let Err(e) = open_passwords() {
        eprintln!("tabula-chrome: {e}");
        return result;
    }
    let until = Instant::now() + Duration::from_secs(10);
    while matches!(result, Err(CallError::NotRunning)) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(200));
        result = tabula_keychain::call(request);
    }
    if matches!(request, Request::Logins { .. } | Request::Check { .. })
        && matches!(result, Ok(Response::Locked))
        && let Ok(Response::Unlocked) = tabula_keychain::call(&Request::Unlock)
    {
        result = tabula_keychain::call(request);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_browser_requests_pass() {
        let m = json!({ "id": 3, "request": { "type": "search", "query": "bank" } });
        assert_eq!(answer(&m), json!({ "id": 3, "error": "not a request for browsers" }));
        let m = json!({ "id": 4, "request": { "type": "nonsense" } });
        assert!(answer(&m)["error"].as_str().unwrap().starts_with("not a request Passwords knows"));
        assert!(allowed(&Request::Logins { url: "https://a.example".into() }));
    }
}
