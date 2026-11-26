//! What is said between the agents service on a host
//! (`olo-rex-link agent serve`) and those that ask it: the `olo agent`
//! command there, and the Olo desktop following it. One JSON object a
//! line, over the service's Unix socket.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model::NativeSubagent;
use super::state::SubagentLiveState;

/// The protocol's number. A service that is asked by a later one hands
/// over: it writes down what it knows and ends, and the asker starts
/// its own build's (after an upgrade of Olo put a new helper on the
/// host, its service takes over at the first command, the children
/// untouched).
pub const PROTO: u32 = 1;

/// What is asked of the service.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Request {
    pub proto: u32,
    /// `start`, `list`, `status`, `read`, `watch`, `send`, `approve`,
    /// `stop`, `prune`, `focus` (the `olo agent` commands); `follow`
    /// (the desktop's: everything, then each change), `adopt` (children
    /// an earlier Olo's desktop kept), `policy` (which child a pane or a
    /// process is, and what it may ask of the desktop), `ping`,
    /// `retire`.
    pub op: String,
    /// The command's arguments as typed (`olo agent OP ARG…`)…
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// …or its structured form (`rex do olo.agent.OP --args '{…}'`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub obj: Option<Value>,
    /// Where it was asked from: the folder, and the Rex pane and session
    /// (`REX_BLOCK`, `REX_SESSION`) of the asking shell.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cwd: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub block: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session: String,
    /// What a child with `--desktop inherit` takes of the asking shell's
    /// environment (`SSH_AUTH_SOCK`, `BROWSER`, the desktop commands'
    /// folder on `PATH`): what the desktop gave its parent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inherit: Vec<(String, String)>,
}

/// What the service writes the moment it has read a request and is
/// about to do what it asks, before the answer: an empty line. An asker
/// whose connection ends before it knows nothing was done (it asks the
/// next service, whatever it asked); one whose connection ends after it
/// and before the answer doesn't know what was, and says so rather than
/// ask twice for something that mustn't happen twice.
pub const TAKEN: &str = "";

/// The service's answer to everything but `follow`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stdout: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stderr: String,
    /// The command's status (0 where `ok`).
    #[serde(default)]
    pub code: i32,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

impl Response {
    pub fn ok(stdout: String, data: Value) -> Self {
        Self { ok: true, stdout, stderr: String::new(), code: 0, data }
    }

    /// A refusal or failure: `what` is the command's name (`olo agent
    /// WHAT: …`), 1 a failure, 2 a mistake in what was asked, 69 what
    /// can't be done from here.
    pub fn err(what: &str, msg: impl AsRef<str>, code: i32) -> Self {
        Self { ok: false, stdout: String::new(), stderr: format!("olo agent {what}: {}\n", msg.as_ref()), code, data: Value::Null }
    }
}

/// A child as the service tells it to one that follows: what is kept
/// of it, and how it stands now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentView {
    #[serde(flatten)]
    pub entry: NativeSubagent,
    pub state: SubagentLiveState,
    #[serde(default)]
    pub summary: Option<String>,
}

/// What a follower is sent: everything once, then each change.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Frame {
    /// Every child the service knows, the exited it keeps too.
    Snapshot { proto: u32, cursor: u64, agents: Vec<AgentView> },
    /// A child started, changed or exited.
    Agent { agent: Box<AgentView> },
    /// A child's record was removed (`prune`).
    Gone { id: String },
}

/// Where a service keeps its things, for the Rex server at `server`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// Its folder under the account's state.
    pub dir: PathBuf,
    pub registry: PathBuf,
    pub events: PathBuf,
    /// Held by the one service that runs.
    pub lock: PathBuf,
    pub log: PathBuf,
    /// Where it listens.
    pub socket: PathBuf,
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

/// The account's own runtime folder, whatever the environment says: a
/// command in a Rex pane and one run over SSH (which may have no
/// `XDG_RUNTIME_DIR`) must find the same socket.
fn runtime_dir(uid: u32) -> Option<PathBuf> {
    let dir = PathBuf::from(format!("/run/user/{uid}"));
    dir.is_dir().then_some(dir)
}

fn state_home(home: &Path) -> PathBuf {
    std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(".local/state"))
}

impl Paths {
    /// For the Rex server whose socket is `server`, in the account
    /// whose home is `home` and number is `uid`. `OLO_REX_AGENTS_DIR`
    /// puts all of it in one folder (Olo's own tests).
    pub fn of(server: &Path, home: &Path, uid: u32) -> Self {
        let key = format!("{:016x}", fnv(server.as_os_str().as_encoded_bytes()));
        if let Some(dir) = std::env::var_os("OLO_REX_AGENTS_DIR").map(PathBuf::from).filter(|p| p.is_absolute()) {
            let dir = dir.join(&key);
            return Self { registry: dir.join("registry.json"), events: dir.join("events.jsonl"), lock: dir.join("lock"), log: dir.join("service.log"), socket: dir.join("sock"), dir };
        }
        let dir = state_home(home).join("olo/rex-agents").join(&key);
        let socket = match runtime_dir(uid) {
            Some(rt) => rt.join("olo-rex-agents").join(format!("{key}.sock")),
            None => dir.join("sock"),
        };
        Self { registry: dir.join("registry.json"), events: dir.join("events.jsonl"), lock: dir.join("lock"), log: dir.join("service.log"), socket, dir }
    }
}

/// The local Rex server's socket: `REX_SERVER_SOCKET` where set, else
/// the first that is there of the places Rex puts it.
pub fn server_socket(home: &Path, uid: u32) -> Option<PathBuf> {
    if let Some(named) = std::env::var_os("REX_SERVER_SOCKET").filter(|s| !s.is_empty()) {
        let named = named.to_string_lossy().into_owned();
        return Some(PathBuf::from(named.strip_prefix("unix://").unwrap_or(&named)));
    }
    let mut places = vec![PathBuf::from(format!("/run/user/{uid}/rex/server.sock"))];
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute()) {
        places.push(rt.join("rex/server.sock"));
        places.push(rt.join("rex/rex.sock"));
    }
    places.push(home.join(".local/state/rex/server.sock"));
    places.push(home.join(".local/state/rex/rex.sock"));
    places.push(PathBuf::from("/run/rex/server.sock"));
    places.into_iter().find(|p| {
        use std::os::unix::fs::FileTypeExt as _;
        std::fs::metadata(p).is_ok_and(|m| m.file_type().is_socket())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_and_an_answer_are_one_line_each() {
        let req = Request { proto: PROTO, op: "send".into(), args: vec!["sa-1".into(), "two\nlines".into()], cwd: "/w".into(), block: "block:1".into(), ..Default::default() };
        let line = serde_json::to_string(&req).unwrap();
        assert!(!line.contains('\n'), "{line}");
        let back: Request = serde_json::from_str(&line).unwrap();
        assert_eq!(back.args[1], "two\nlines");
        assert_eq!(back.block, "block:1");
        // (What an older asker leaves out is nothing.)
        let bare: Request = serde_json::from_str(r#"{"proto":1,"op":"list"}"#).unwrap();
        assert!(bare.args.is_empty() && bare.obj.is_none() && bare.inherit.is_empty());
        let no = Response::err("stop", "no such child", 1);
        assert_eq!(no.stderr, "olo agent stop: no such child\n");
        assert_eq!(serde_json::from_str::<Response>(&serde_json::to_string(&no).unwrap()).unwrap().code, 1);
    }

    #[test]
    fn a_server_has_a_folder_and_a_socket_of_its_own() {
        let (a, b) = (Paths::of(Path::new("/run/user/1000/rex/server.sock"), Path::new("/home/a"), 4_000_000_000), Paths::of(Path::new("/var/tmp/x/rex.sock"), Path::new("/home/a"), 4_000_000_000));
        assert_ne!(a.dir, b.dir);
        assert_ne!(a.socket, b.socket);
        // (No runtime folder for that account: beside the registry.)
        assert_eq!(a.socket, a.dir.join("sock"));
        assert!(a.registry.starts_with(&a.dir) && a.events.starts_with(&a.dir) && a.lock.starts_with(&a.dir));
        // (The same for whoever asks, in a pane or over SSH.)
        assert_eq!(a, Paths::of(Path::new("/run/user/1000/rex/server.sock"), Path::new("/home/a"), 4_000_000_000));
    }
}
