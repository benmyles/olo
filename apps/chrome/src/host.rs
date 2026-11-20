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
//! A Chrome with several profiles open has a host for each, and one
//! socket for agents: the host that has it asks the others for their
//! profiles' windows too (each on its own socket, `hosts_dir`), and when
//! it goes another takes the socket (`keep_answering`).
//! There too, the command `open-in-front URL` (choose.rs: Open in Which
//! Window?) has the extension open a link in a new tab of the window
//! Chrome had the focus in last (`{open_in_front: URL}`): Chrome's own
//! hand-over opens one in a window of its choosing, not the one in front.
//!
//! One-time codes texted to the person's phone reach the tab waiting for
//! one through it (codes.rs): `{code: …}` to the extension, `{code_waiting}`
//! and `{code_outcome}` from it.
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

/// Links the extension was asked to open in a window of its profile's
/// (`open_in_window`), waiting for whether it did, by number.
type Opening = Arc<Mutex<HashMap<u64, mpsc::Sender<bool>>>>;

/// How long the extension has to say whether it opened a link.
const OPEN_WAIT: Duration = Duration::from_millis(800);

/// Where the hosts of a Chrome's profiles listen for links to open
/// (`open_in_window`): a socket a host, named for Chrome's process and
/// the host's own, in a folder of theirs under the runtime folder.
pub fn hosts_dir() -> std::path::PathBuf {
    olo_xdg::runtime_dir().join("olo-chrome-hosts")
}

/// This host's own socket: `{chrome}-{me}.sock` in [`hosts_dir`].
fn own_socket() -> std::path::PathBuf {
    hosts_dir().join(format!("{}-{}.sock", std::os::unix::process::parent_id(), std::process::id()))
}

/// This host's own socket, answered: what's asked of one profile of a
/// Chrome that may have several open (a host for each). Each asking is a
/// line of JSON, answered with one:
///
/// - `{"tree": true}`: this profile's windows' widget trees, for the
///   host that has Chrome's socket for agents to put with its own
///   ([`others_trees`]).
/// - `{url, title, width, height, must_match, focused}`: a link to open
///   in a window of this profile's, as the extension takes it, answered
///   `{"ok": true}` if the extension opened it. Only once its extension
///   says it can (`{can: ["open_in_window"]}`, `can_open`): an older one
///   opens the link whatever it's asked and never answers, so it's
///   answered `{"ok": false}` for, and whoever asked goes on to the next.
///
/// A socket a host, not the one Chrome's process has for agents
/// (`commands::serve`, which one host has): a window is one profile's,
/// so whoever opens a link asks each in turn.
fn own_socket_answered(out: Arc<Mutex<std::io::Stdout>>, opening: Opening, waiting: Waiting, can_open: Arc<AtomicBool>) {
    let dir = hosts_dir();
    let path = own_socket();
    let listening = std::fs::create_dir_all(&dir).and_then(|()| {
        // (Sockets of Chromes that have gone, and of hosts that have:
        // swept as a host starts.)
        for old in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = old.file_name().to_string_lossy().into_owned();
            let pids = name.strip_suffix(".sock").and_then(|n| n.split_once('-')).and_then(|(chrome, host)| Some((chrome.parse::<u32>().ok()?, host.parse::<u32>().ok()?)));
            let gone = pids.is_some_and(|(chrome, host)| [chrome, host].iter().any(|pid| !std::path::Path::new(&format!("/proc/{pid}")).exists()));
            if gone {
                let _ = std::fs::remove_file(old.path());
            }
        }
        let _ = std::fs::remove_file(&path);
        std::os::unix::net::UnixListener::bind(&path)
    });
    let listener = match listening {
        Ok(l) => l,
        Err(e) => {
            eprintln!("olo-chrome: can't answer for this profile's windows: {e}");
            return;
        }
    };
    let next = Arc::new(AtomicU64::new(1));
    let _ = std::thread::Builder::new().name("own-socket".into()).spawn(move || {
        for s in listener.incoming().flatten() {
            let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
            let mut line = String::new();
            if std::io::BufRead::read_line(&mut std::io::BufReader::new(&s), &mut line).is_err() {
                continue;
            }
            let Ok(ask) = serde_json::from_str::<Value>(&line) else { continue };
            // (Its trees take as long as the extension does: asked on a
            // thread of their own, so a link isn't kept waiting for them.)
            if ask.get("tree").is_some() {
                let (out, waiting) = (out.clone(), waiting.clone());
                let _ = std::thread::Builder::new().name("own-trees".into()).spawn(move || {
                    let trees = own_trees(&out, &waiting);
                    if let Ok(answer) = serde_json::to_string(&trees) {
                        let _ = (&s).write_all(format!("{answer}\n").as_bytes());
                    }
                });
                continue;
            }
            let web = ask["url"].as_str().is_some_and(|u| ["http://", "https://"].iter().any(|p| u.starts_with(p)));
            let ok = web && can_open.load(Ordering::Relaxed) && {
                let n = next.fetch_add(1, Ordering::Relaxed);
                let (tx, rx) = mpsc::channel();
                opening.lock().unwrap_or_else(|e| e.into_inner()).insert(n, tx);
                send(&out, &json!({ "open_in_front": ask, "n": n }));
                let ok = rx.recv_timeout(OPEN_WAIT).unwrap_or(false);
                opening.lock().unwrap_or_else(|e| e.into_inner()).remove(&n);
                ok
            };
            let _ = (&s).write_all(format!("{}\n", json!({ "ok": ok })).as_bytes());
        }
    });
}

/// The numbers tree questions to the extension go by.
static TREE_ASKED: AtomicU64 = AtomicU64::new(1);

/// This profile's windows' widget trees, asked of its extension (none,
/// if it doesn't answer in `TREE_WAIT_FOR`).
fn own_trees(out: &Mutex<std::io::Stdout>, waiting: &Waiting) -> Vec<WindowTree> {
    let n = TREE_ASKED.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    waiting.lock().unwrap_or_else(|e| e.into_inner()).insert(n, tx);
    send(out, &json!({ "tree": n }));
    let got = rx.recv_timeout(commands::TREE_WAIT_FOR).unwrap_or_default();
    waiting.lock().unwrap_or_else(|e| e.into_inner()).remove(&n);
    got
}

/// The windows' trees of the other profiles of Chrome `chrome`: each of
/// its hosts but `me` asked on its own socket in `dir`, all at once (each
/// may take as long as its extension does), their answers put together.
/// A host that has gone, or says nothing in time, adds nothing.
fn others_trees(dir: &std::path::Path, chrome: u32, me: u32) -> Vec<WindowTree> {
    use std::io::BufRead as _;
    let (prefix, mine) = (format!("{chrome}-"), format!("{chrome}-{me}.sock"));
    let mut hosts: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".sock") && n != mine))
        .collect();
    hosts.sort();
    let ask = |host: &std::path::Path| -> Vec<WindowTree> {
        let Ok(mut s) = std::os::unix::net::UnixStream::connect(host) else { return Vec::new() };
        let _ = s.set_read_timeout(Some(commands::TREE_WAIT_FOR + Duration::from_millis(300)));
        if s.write_all(b"{\"tree\":true}\n").is_err() {
            return Vec::new();
        }
        let mut answer = String::new();
        let _ = std::io::BufReader::new(&s).read_line(&mut answer);
        serde_json::from_str(&answer).unwrap_or_default()
    };
    std::thread::scope(|scope| {
        let asked: Vec<_> = hosts.iter().map(|host| scope.spawn(|| ask(host))).collect();
        asked.into_iter().flat_map(|a| a.join().unwrap_or_default()).collect()
    })
}

/// Another host takes Chrome's socket for agents when the one that had
/// it has gone (its profile's last window closed): told by the hosts'
/// folder changing (a host takes its own socket away as it ends), each
/// one left looks whether the agents' socket still has someone behind
/// it, and answers it itself if not. Asleep otherwise: nothing is asked
/// on a clock. (Two that take it at once: the later has it, as when two
/// start.)
fn keep_answering(out: Arc<Mutex<std::io::Stdout>>, waiting: Waiting) {
    let chrome = std::os::unix::process::parent_id();
    let _ = std::thread::Builder::new().name("agents-socket".into()).spawn(move || {
        let (tx, rx) = mpsc::channel();
        let Some(_watch) = olo_kit_base::watch::watch([hosts_dir()], move |_| {
            let _ = tx.send(());
        }) else {
            return;
        };
        while rx.recv().is_ok() {
            // (What came with it, its own doing too: looked at once.)
            while rx.try_recv().is_ok() {}
            if !commands::answered(chrome) {
                serve_trees(out.clone(), waiting.clone(), None);
            }
        }
    });
}

/// Requests waiting for Passwords' answer, by the extension's id: their
/// connections, to end on `{cancel}`.
static ASKING: Mutex<Option<HashMap<u64, std::os::unix::net::UnixStream>>> = Mutex::new(None);

/// Requests being answered, and whether an upgrade is waiting for them:
/// the old version goes once none is (the extension connects again,
/// starting the new one), so a Fill waiting for an unlock isn't cut off.
static ANSWERING: AtomicUsize = AtomicUsize::new(0);
static UPGRADED: AtomicBool = AtomicBool::new(false);

/// This host's own socket, taken away as the host ends (Chrome closed
/// its pipe): the other hosts, following the folder, hear of it
/// ([`keep_answering`]).
struct OwnSocket;

impl Drop for OwnSocket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(own_socket());
    }
}

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
    let opening: Opening = Arc::default();
    let can_open = Arc::new(AtomicBool::new(false));
    let mut slack = slack_here.map(|here| Slack::new(here, out.clone()));
    // (Before Chrome's socket for agents: whoever has that asks this.)
    let _own = slack.is_none().then(|| {
        own_socket_answered(out.clone(), opening.clone(), trees.clone(), can_open.clone());
        OwnSocket
    });
    serve_trees(out.clone(), trees.clone(), slack.as_ref());
    if slack.is_none() {
        keep_answering(out.clone(), trees.clone());
    }
    if let Some(s) = &slack {
        s.hello();
    }
    let codes = crate::codes::Codes::follow(out.clone());
    let max_in = if slack.is_some() { MAX_IN_SLACK } else { MAX_IN };
    // (Made when Chrome first tells of a download.)
    let mut downloads: Option<Downloads> = None;
    // (Made when Chrome first says what's being looked at.)
    let mut bus: Option<zbus::blocking::Connection> = None;
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
        // What the extension can be asked (a newer one's first words).
        if let Ok(m) = &message
            && let Some(can) = m.get("can").and_then(Value::as_array)
        {
            if can.iter().any(|c| c == "open_in_window") {
                can_open.store(true, Ordering::Relaxed);
            }
            continue;
        }
        // Whether the extension opened a link it was asked to.
        if let Ok(m) = &message
            && let Some(n) = m.get("opened_in_front").and_then(Value::as_u64)
        {
            if let Some(tx) = opening.lock().unwrap_or_else(|e| e.into_inner()).remove(&n) {
                let _ = tx.send(m.get("ok").and_then(Value::as_bool).unwrap_or(false));
            }
            continue;
        }
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
        // One-time codes from the phone (codes.rs).
        if let Ok(m) = &message
            && codes.from_extension(m)
        {
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
        // The site the person is looking at: its notifications are read.
        if let Ok(m) = &message
            && let Some(host) = m.get("shown").and_then(Value::as_str)
        {
            if bus.is_none() {
                bus = zbus::blocking::Connection::session().ok();
            }
            if let Some(c) = &bus
                && let Err(e) = visited(c, host)
            {
                eprintln!("olo-chrome: couldn't tell Olo's notifications what's being looked at: {e}");
                bus = None;
            }
            continue;
        }
        // Chrome's downloads (downloads.rs).
        if let Ok(m) = &message
            && (m.get("downloads").is_some() || m.get("downloaded").is_some() || m.get("held").is_some())
        {
            let whose = if slack.is_some() { downloads::SLACK } else { downloads::CHROME };
            let downloads = downloads.get_or_insert_with(|| Downloads::new(whose, out.clone()));
            if let Some(d) = m.get("held").cloned().and_then(|v| serde_json::from_value::<Download>(v).ok()) {
                downloads.held(&d);
            }
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
    let (chrome, me) = (std::os::unix::process::parent_id(), std::process::id());
    let out_for_invoke = out.clone();
    // (Olo's Slack is a Chrome of one profile: its own trees are all.
    // Chrome's: every profile's, this one's asked of its extension while
    // the others' hosts are asked for theirs. It read only the windows of
    // the profile whose host started last.)
    let alone = slack.is_some();
    let trees = move || {
        if alone {
            return own_trees(&out, &waiting);
        }
        std::thread::scope(|scope| {
            let others = scope.spawn(|| others_trees(&hosts_dir(), chrome, me));
            let mut all = own_trees(&out, &waiting);
            all.extend(others.join().unwrap_or_default());
            all
        })
    };
    let served = match slack {
        // Slack's channels and people, Jump To… and Search…, for the palette.
        Some(s) => commands::serve(chrome, commands::Served { app_id: "olo.slack".into(), trees: Box::new(trees), menus: Box::new(s.menus()), invoke: Box::new(s.invoke()) }),
        None => {
            let out = out_for_invoke;
            // (`open-in-front {url, title, width, height}`: a web link, in
            // the window so named and sized.)
            let invoke = move |command: &str| {
                let Some(ask) = command.strip_prefix("open-in-front ").and_then(|a| serde_json::from_str::<Value>(a).ok()) else { return false };
                let web = ask["url"].as_str().is_some_and(|u| ["http://", "https://"].iter().any(|p| u.starts_with(p)));
                if web {
                    send(&out, &json!({ "open_in_front": ask }));
                }
                web
            };
            commands::serve(chrome, commands::Served { app_id: "google-chrome".into(), trees: Box::new(trees), menus: Box::new(Vec::new), invoke: Box::new(invoke) })
        }
    };
    match served {
        // (What this host answered it with before, when it had it and
        // another took it: ended, not left asleep at each taking.)
        Ok(serving) => {
            if let Some(before) = SERVING.lock().unwrap_or_else(|e| e.into_inner()).replace(serving) {
                before.stop();
            }
        }
        Err(e) => eprintln!("olo-chrome: can't answer for Chrome's widget trees: {e}"),
    }
}

/// Chrome's socket for agents as this host answers it (or did, until
/// another host took it).
static SERVING: Mutex<Option<commands::Serving>> = Mutex::new(None);

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
            | Request::Code { .. }
            | Request::Cards { .. }
            | Request::FillCard { .. }
            | Request::Check { .. }
            | Request::Save { .. }
            | Request::OtpScan { .. }
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

/// Tell Olo's notifications the person is looking at a page of `host`
/// (`com.olodesktop.Notifications1.VisitedSite`): the site's
/// notifications are read. Said and not waited for, and never starting
/// a notification service that isn't running.
fn visited(bus: &zbus::blocking::Connection, host: &str) -> zbus::Result<()> {
    use olo_protocol::notifications::{CONTROL, CONTROL_BUS, CONTROL_PATH};
    use zbus::message::{Flags, Message};
    let call = Message::method_call(CONTROL_PATH, "VisitedSite")?.destination(CONTROL_BUS)?.interface(CONTROL)?.with_flags(Flags::NoReplyExpected)?.with_flags(Flags::NoAutoStart)?.build(&(host,))?;
    bus.send(&call)
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

    /// The other profiles' hosts are each asked for their windows' trees
    /// on their own sockets, and what they answer is put together: not
    /// this host's own (it asks its extension itself), not another
    /// Chrome's, and a host that has gone, or answers with what isn't
    /// trees, adds nothing and holds nothing up.
    #[test]
    fn the_other_profiles_trees_are_gathered() {
        use std::io::{BufRead as _, Write as _};
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-chrome-trees-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let tree = |title: &str| WindowTree { title: title.into(), width: 800, height: 600, ..Default::default() };
        // A host: answers a tree asking with `answer`.
        let host = |name: &str, answer: String| {
            let listener = std::os::unix::net::UnixListener::bind(dir.join(name)).unwrap();
            std::thread::spawn(move || {
                for s in listener.incoming().flatten() {
                    let mut line = String::new();
                    std::io::BufReader::new(&s).read_line(&mut line).unwrap();
                    assert!(serde_json::from_str::<Value>(&line).unwrap().get("tree").is_some(), "{line}");
                    let _ = (&s).write_all(format!("{answer}\n").as_bytes());
                }
            });
        };
        host("700-1.sock", serde_json::to_string(&vec![tree("Mine")]).unwrap());
        host("700-2.sock", serde_json::to_string(&vec![tree("Work - Inbox"), tree("Work - Docs")]).unwrap());
        host("700-3.sock", serde_json::to_string(&vec![tree("Home - News")]).unwrap());
        host("700-4.sock", "not trees".into());
        host("900-1.sock", serde_json::to_string(&vec![tree("Another Chrome's")]).unwrap());
        // A host that ended: its socket left, nobody behind it.
        drop(std::os::unix::net::UnixListener::bind(dir.join("700-5.sock")).unwrap());
        let started = Instant::now();
        let titles: Vec<String> = others_trees(&dir, 700, 1).into_iter().map(|t| t.title).collect();
        assert_eq!(titles, ["Work - Inbox", "Work - Docs", "Home - News"]);
        assert!(started.elapsed() < Duration::from_secs(1), "nothing waited for: {:?}", started.elapsed());
        // With no others, none.
        assert!(others_trees(&dir, 900, 1).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
