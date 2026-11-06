//! The native messaging host Chrome starts for the Olo Passwords
//! extension (`olo-chrome chrome-extension://ID/`). Messages are a
//! little-endian u32 length and JSON, on stdin and stdout: `{id, request,
//! start}` (a keychain request; with `start`, Passwords is started first
//! if it isn't running, and a fresh start's lock is waited out) or `{id,
//! open: true}` (start Passwords or bring it forward); each is answered
//! with `{id, response}` or `{id, error}`, on a thread of its own, so a
//! Fill waiting for an unlock holds up nothing else. Only what a browser
//! needs passes: logins, fill, check, save, unlock, show and passkeys.
//! `{cancel: ID}` gives up on request ID (the page went away), so
//! Passwords takes down the passkey alert it shows for it. `{id,
//! profile: TOKEN}` says which Chrome profile the extension is in, and
//! who is signed in to it (profile.rs).
//!
//! `{downloads}` and `{downloaded}` tell of Chrome's downloads, which
//! Deskbar, Tracker and Olo's notifications show (downloads.rs).
//!
//! It also answers agents' questions about Chrome's windows (a kit app's
//! `tree` socket, for Chrome's process): `{tree: n}` goes to the
//! extension, which answers `{tree: n, windows}` (background.js, tree.js).
//!
//! In olo-slack's Chrome, `{slack: …}` joins Slack to Olo (slack.rs), and
//! the socket also has Slack's commands for the palette.

use olo_sync::Mutex;
use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use olo_keychain::{CallError, Request, Response};
use olo_kit_base::commands::{self, WindowTree};
use serde_json::{Value, json};

use crate::downloads::{self, Download, Downloads};
use crate::profile;
use crate::slack::{self, Slack};
use zeroize::Zeroize;

/// The largest message taken from Chrome (Slack's Chrome: a picture for
/// the Viewer, in base64).
const MAX_IN: usize = 1 << 20;
const MAX_IN_SLACK: usize = slack::MAX_PICTURE / 3 * 4 + (64 << 10);

/// Tree questions waiting for the extension's answer, by number.
type Waiting = Arc<Mutex<HashMap<u64, mpsc::Sender<Vec<WindowTree>>>>>;

/// Requests waiting for Passwords' answer, by the extension's id: their
/// connections, to end on `{cancel}`.
static ASKING: Mutex<Option<HashMap<u64, std::os::unix::net::UnixStream>>> = Mutex::new(None);

/// Requests being answered, and whether an upgrade is waiting for them:
/// the old version goes once none is (the extension connects again,
/// starting the new one), so a Fill waiting for an unlock isn't cut off.
static ANSWERING: AtomicUsize = AtomicUsize::new(0);
static UPGRADED: AtomicBool = AtomicBool::new(false);

/// Gone for the new version, if upgraded and nothing is being answered.
fn make_way() {
    if UPGRADED.load(Ordering::SeqCst) && ANSWERING.load(Ordering::SeqCst) == 0 {
        std::process::exit(0);
    }
}

pub fn run() {
    // (Before any thread starts: it changes our environment.)
    let slack_here = slack::take_env();
    olo_kit_base::restore::on_upgrade(|| {
        UPGRADED.store(true, Ordering::SeqCst);
        make_way();
    });
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let trees: Waiting = Arc::default();
    let mut slack = slack_here.map(|here| Slack::new(here, out.clone()));
    serve_trees(out.clone(), trees.clone(), slack.as_ref());
    if let Some(s) = &slack {
        s.hello();
    }
    let max_in = if slack.is_some() { MAX_IN_SLACK } else { MAX_IN };
    // (Made when Chrome first tells of a download.)
    let mut downloads: Option<Downloads> = None;
    let mut input = std::io::stdin().lock();
    loop {
        let mut len = [0u8; 4];
        // Chrome closed the pipe: the extension is done with us.
        if input.read_exact(&mut len).is_err() {
            return;
        }
        let n = u32::from_le_bytes(len) as usize;
        if n > max_in {
            eprintln!("olo-chrome: a message of {n} bytes from Chrome is too big");
            return;
        }
        let mut buf = vec![0u8; n];
        if input.read_exact(&mut buf).is_err() {
            return;
        }
        let message: Result<Value, _> = serde_json::from_slice(&buf);
        buf.zeroize();
        // The extension's answer to a tree question.
        if let Ok(m) = &message
            && let Some(n) = m.get("tree").and_then(Value::as_u64)
        {
            let windows = m.get("windows").cloned().map(serde_json::from_value).and_then(Result::ok).unwrap_or_default();
            if let Some(tx) = trees.lock().unwrap_or_else(|e| e.into_inner()).remove(&n) {
                let _ = tx.send(windows);
            }
            continue;
        }
        // olo-slack's Chrome's Slack (slack.rs).
        if let Ok(m) = &message
            && let Some(s) = &mut slack
            && let Some(sm) = m.get("slack")
        {
            s.message(sm);
            continue;
        }
        // Chrome's downloads (downloads.rs).
        if let Ok(m) = &message
            && (m.get("downloads").is_some() || m.get("downloaded").is_some())
        {
            let whose = if slack.is_some() { downloads::SLACK } else { downloads::CHROME };
            let downloads = downloads.get_or_insert_with(|| Downloads::new(whose));
            if let Some(list) = m.get("downloads").cloned().and_then(|v| serde_json::from_value::<Vec<Download>>(v).ok()) {
                downloads.going(&list);
            }
            if let Some(d) = m.get("downloaded").cloned().and_then(|v| serde_json::from_value::<Download>(v).ok()) {
                downloads.done(&d);
            }
            continue;
        }
        // The page gave up on a request.
        if let Ok(m) = &message
            && let Some(n) = m.get("cancel").and_then(Value::as_u64)
        {
            if let Some(stream) = ASKING.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default().remove(&n) {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
            continue;
        }
        let out = out.clone();
        ANSWERING.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            let reply = match message {
                Ok(m) => answer(&m),
                Err(e) => json!({ "id": null, "error": format!("not JSON: {e}") }),
            };
            send(&out, &reply);
            ANSWERING.fetch_sub(1, Ordering::SeqCst);
            make_way();
        });
    }
}

/// Chrome's windows' widget trees for agents (Agent Screens' `tree`),
/// answered on Chrome's own process's socket (Chrome starts us), so they
/// are found as a kit app's are: each is asked of the extension.
fn serve_trees(out: Arc<Mutex<std::io::Stdout>>, waiting: Waiting, slack: Option<&Slack>) {
    let chrome = std::os::unix::process::parent_id();
    let next = AtomicU64::new(1);
    let trees = move || {
        let n = next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        waiting.lock().unwrap_or_else(|e| e.into_inner()).insert(n, tx);
        send(&out, &json!({ "tree": n }));
        let got = rx.recv_timeout(commands::TREE_WAIT_FOR).unwrap_or_default();
        waiting.lock().unwrap_or_else(|e| e.into_inner()).remove(&n);
        got
    };
    let served = match slack {
        // Slack's channels and people, Jump To… and Search…, for the palette.
        Some(s) => commands::serve(chrome, commands::Served { app_id: "olo.slack".into(), trees: Box::new(trees), menus: Box::new(s.menus()), invoke: Box::new(s.invoke()) }),
        None => commands::serve_trees(chrome, "google-chrome", trees),
    };
    if let Err(e) = served {
        eprintln!("olo-chrome: can't answer for Chrome's widget trees: {e}");
    }
}

pub(crate) fn send(out: &Mutex<std::io::Stdout>, reply: &Value) {
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
        Request::Logins { .. }
            | Request::Fill { .. }
            | Request::Check { .. }
            | Request::Save { .. }
            | Request::Unlock
            | Request::Show { .. }
            | Request::Passkeys { .. }
            | Request::PasskeyCreate { .. }
            | Request::PasskeySign { .. }
    )
}

/// Start Passwords (or bring it forward: it is a single instance).
fn open_passwords() -> Result<(), String> {
    let program = olo_xdg::find_program("olo-passwords").ok_or("Passwords isn't installed.")?;
    olo_xdg::open::spawn(&[program.into_os_string()]).map_err(|e| format!("Passwords couldn't be started: {e}"))
}

fn answer(m: &Value) -> Value {
    let id = m.get("id").cloned().unwrap_or(Value::Null);
    // The popup's colours: the look in use now.
    if m.get("palette").and_then(Value::as_bool) == Some(true) {
        return json!({ "id": id, "response": { "type": "palette", "css": crate::extension::palette_css() } });
    }
    // Which profile the extension is in, by the token it keeps
    // (profile.rs): who is signed in to Chrome there.
    if let Some(token) = m.get("profile").and_then(Value::as_str) {
        let p = profile::this_one(token).unwrap_or_default();
        return json!({ "id": id, "response": { "type": "profile", "dir": p.dir, "email": p.email, "name": p.name } });
    }
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
    let n = id.as_u64();
    let result = ask(&request, start, n);
    if let Some(n) = n {
        ASKING.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default().remove(&n);
    }
    match result {
        Ok(response) => json!({ "id": id, "response": serde_json::to_value(&response).unwrap_or(Value::Null) }),
        Err(e) => json!({ "id": id, "error": e.to_string() }),
    }
}

/// Ask Passwords, keeping the connection under the extension's id `n`
/// while it waits, for `{cancel}`.
fn call(request: &Request, n: Option<u64>) -> Result<Response, CallError> {
    match n {
        Some(n) => olo_keychain::call_keeping(request, |stream| {
            ASKING.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default().insert(n, stream);
        }),
        None => olo_keychain::call(request),
    }
}

/// Ask Passwords; with `start`, start it if it isn't running (then wait
/// for it, and for the unlock it begins with).
fn ask(request: &Request, start: bool, n: Option<u64>) -> Result<Response, CallError> {
    let mut result = call(request, n);
    if !start || !matches!(result, Err(CallError::NotRunning)) {
        return result;
    }
    if let Err(e) = open_passwords() {
        eprintln!("olo-chrome: {e}");
        return result;
    }
    let until = Instant::now() + Duration::from_secs(10);
    while matches!(result, Err(CallError::NotRunning)) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(200));
        result = call(request, n);
    }
    if matches!(request, Request::Logins { .. } | Request::Check { .. })
        && matches!(result, Ok(Response::Locked))
        && let Ok(Response::Unlocked) = olo_keychain::call(&Request::Unlock)
    {
        result = olo_keychain::call(request);
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
