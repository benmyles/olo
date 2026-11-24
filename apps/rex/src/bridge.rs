//! Olo's bridge for Rex's hosts: what programs in a Rex session may
//! ask of the Olo desktop the session is shown on.
//!
//! - On this computer: a folder put first on the PATH of Rex's own
//!   shells with Olo's `xdg-open` (`olo-open`), `sudo` (`olo-sudo`)
//!   and `op` (`olo-op`).
//! - On a far host: nothing of the bridge's is installed. The Olo app
//!   offers its actions to the host's Rex server, as every Rex client
//!   offers its own, and a program in a session there asks with Rex's
//!   own command: `rex do olo.open target=URL`, `rex do olo.op
//!   --args '{"args":[…]}'`, `rex do olo.screen --args '{"args":[…]}'`
//!   (`rex actions` lists them). Each is answered with what the
//!   command wrote here. The one way, and Rex's (Ben, 2026-11-23: "we
//!   should leverage rex as much as we can to do things 'the rex way'
//!   … making the olo functionality available via rex. and we don't
//!   need that PLUS another way just the one best way"). Until then
//!   shims were put in the account's home there (`xdg-open`, `op`,
//!   `olo-screen`) that looked for a port and a token nothing set, in
//!   a folder no session had on its PATH: they did nothing (an agent
//!   there: "The olo-screen and xdg-open commands the skill describes
//!   aren't installed on this machine"), and where a host still has
//!   them they are taken off as it is connected to ([`OLD_SHIMS`]).
//! - What a host may ask is its permissions' to say
//!   ([`HostBridgePolicy`], Preferences ▸ Rex), read at each request.

use std::{
    collections::HashMap,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use olo_xdg::open::Target;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{links::parse_localhost_url, settings::HostBridgePolicy};

/// Prepare `~/.local/state/olo/rex/bin` with symlinks to `olo-open` (`xdg-open`),
/// `olo-sudo` (`sudo`), and `olo-op` (`op`) so local Rex shells seamlessly use
/// Olo's native Chrome window picker, sudo sheets, and Olo Passwords!
pub fn ensure_local_bin_dir() -> Option<PathBuf> {
    let dir = olo_xdg::state_dir().join("rex").join("bin");
    std::fs::create_dir_all(&dir).ok()?;
    let pairs = [("xdg-open", "olo-open"), ("sudo", "olo-sudo"), ("op", "olo-op")];
    for (link_name, prog) in pairs {
        if let Some(target) = olo_xdg::find_program(prog) {
            let link = dir.join(link_name);
            let needs = std::fs::read_link(&link).ok().as_ref() != Some(&target);
            if needs {
                let _ = std::fs::remove_file(&link);
                let _ = symlink(&target, &link);
            }
        }
    }
    Some(dir)
}

/// What a session asks of the desktop: the command (`open`, `op`,
/// `screen`, `notify`) and its arguments as they were given there.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BridgeRequest {
    pub cmd: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub block_id: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BridgeResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// Files for the host, named by the command that made them (never
    /// found in what it printed: an item's value naming a file of this
    /// computer isn't one): fetched in pieces ([`Calls::piece`]).
    #[serde(skip)]
    pub carried: Vec<Carried>,
}

/// A file of an answer's, for the host.
#[derive(Debug, Clone, Default)]
pub struct Carried {
    /// What the answer calls it (an Agent Screen's picture: its path
    /// here, which the host writes in its own place).
    pub path: String,
    /// Where on the host it goes, as the command there named it (`op
    /// read -o FILE`); none: the host's own place for it.
    pub to: Option<String>,
    /// It is what the command printed (not text: a document's bytes).
    pub stdout: bool,
    pub bytes: Vec<u8>,
}

impl BridgeResponse {
    pub fn err(msg: impl Into<String>) -> Self {
        Self { ok: false, stdout: None, stderr: Some(msg.into()), data: None, carried: Vec::new() }
    }
}

/// What a host's `op` is given here, so that nothing of this computer's
/// is its to name: the files its flags read are the host's, sent with
/// the request (`payload.files`, base64 by the path the host gave), and
/// written for the run in a folder of its own here; those it writes
/// (`-o`) are written there and carried back; what it reads on standard
/// input is the host's (`payload.stdin`). `op run`, which runs its
/// command where `op` runs, isn't carried: here it would run on this
/// computer. (Before 2026-11-24 a host's `op run` ran here, `-i` read
/// this computer's files and `-o` wrote them.)
pub struct OpCarry {
    pub args: Vec<String>,
    pub stdin: Option<Vec<u8>>,
    /// The host's path for each file the command writes, and where it
    /// is written here.
    pub outputs: Vec<(String, PathBuf)>,
    pub dir: PathBuf,
}

/// `op`'s flags that read a file, and those that write one.
const OP_READS: [&str; 4] = ["i", "in-file", "template", "env-file"];
const OP_WRITES: [&str; 2] = ["o", "out-file"];

impl OpCarry {
    pub fn of(args: &[String], payload: &Value, dir: PathBuf) -> Result<OpCarry, String> {
        use base64::Engine as _;
        let decode = |b64: &str| base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()).map_err(|e| format!("a file sent oddly: {e}"));
        // (Any `run` before `--`, which flags' values may stand before:
        // `olo-op` refuses it exactly too, knowing its own flags.)
        if args.iter().take_while(|a| *a != "--").any(|a| a == "run") {
            return Err("`op run` isn't carried to the Olo desktop: its command would run there. Run the command here with the values from `olo op read` or `olo op inject`.".into());
        }
        let stdin = match payload.get("stdin").and_then(Value::as_str) {
            Some(b64) => Some(decode(b64)?),
            None => None,
        };
        let files = payload.get("files").and_then(Value::as_object);
        let (mut out, mut outputs, mut made) = (Vec::with_capacity(args.len()), Vec::new(), false);
        let mut it = args.iter().peekable();
        while let Some(a) = it.next() {
            let flag = a.strip_prefix("--").or_else(|| a.strip_prefix('-').filter(|f| !f.is_empty()));
            let (name, attached) = match flag.map(|f| f.split_once('=').map_or((f, None), |(k, v)| (k, Some(v)))) {
                Some((name, attached)) if OP_READS.contains(&name) || OP_WRITES.contains(&name) => (name, attached),
                _ => {
                    out.push(a.clone());
                    continue;
                }
            };
            let Some(value) = attached.map(str::to_owned).or_else(|| it.next().cloned()) else {
                return Err(format!("{a} needs a file"));
            };
            if !made {
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
                made = true;
            }
            let here = dir.join(format!("{}-{}", if OP_READS.contains(&name) { "in" } else { "out" }, out.len()));
            if OP_READS.contains(&name) {
                let b64 = files.and_then(|f| f.get(&value)).and_then(Value::as_str).ok_or_else(|| format!("the host's file {value} wasn't sent with the request"))?;
                use std::os::unix::fs::OpenOptionsExt;
                let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&here).map_err(|e| e.to_string())?;
                std::io::Write::write_all(&mut f, &decode(b64)?).map_err(|e| e.to_string())?;
            } else {
                outputs.push((value, here.clone()));
            }
            out.push(format!("--{name}={}", here.display()));
        }
        Ok(OpCarry { args: out, stdin, outputs, dir })
    }
}

/// How long the app takes to answer one action: the Rex server waits
/// thirty seconds at most for a client's answer (and its command line
/// ten, unless told `--timeout`). What takes longer (a question
/// Passwords asks the person, a screen being started) is answered
/// `pending` with a name to wait by (`rex do olo.wait id=…`).
pub const ANSWER_WITHIN: Duration = Duration::from_secs(8);
/// A picture goes to the host in pieces of so many bytes (a request
/// and its answer through a Rex server are a megabyte at most; this is
/// under that once written as text, and most pictures of a screen are
/// one piece).
pub const PIECE: usize = 600 * 1024;
/// The largest file sent to a host for one answer, and how long what
/// was not fetched or asked after is kept.
const FILE_MOST: u64 = 32 << 20;
const KEPT_FOR: Duration = Duration::from_secs(300);

/// What sessions have asked that isn't over: commands still
/// running here, and the pictures their answers named, until the host
/// has fetched them.
pub struct Calls {
    /// [`ANSWER_WITHIN`] (shorter in a test).
    within: Duration,
    running: std::sync::Mutex<HashMap<String, (Instant, tokio::task::JoinHandle<BridgeResponse>)>>,
    files: std::sync::Mutex<HashMap<String, (Instant, Arc<Vec<u8>>)>>,
}

impl Default for Calls {
    fn default() -> Self {
        Calls::answering_within(ANSWER_WITHIN)
    }
}

impl Calls {
    pub fn answering_within(within: Duration) -> Self {
        Calls { within, running: Default::default(), files: Default::default() }
    }

    /// `work`'s answer as the asker is given it, if it comes within
    /// [`ANSWER_WITHIN`]; else a name to wait for it by ([`Self::wait`]).
    pub async fn answer(&self, work: impl std::future::Future<Output = BridgeResponse> + Send + 'static) -> Value {
        self.settle(uuid::Uuid::new_v4().simple().to_string(), tokio::spawn(work)).await
    }

    /// The same for a command asked before and not answered in time.
    pub async fn wait(&self, id: &str) -> Value {
        let running = self.running.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        match running {
            Some((_, work)) => self.settle(id.to_owned(), work).await,
            None => json!(BridgeResponse::err("Olo Bridge: that command is no longer waited for")),
        }
    }

    async fn settle(&self, id: String, mut work: tokio::task::JoinHandle<BridgeResponse>) -> Value {
        match tokio::time::timeout(self.within, &mut work).await {
            Ok(Ok(response)) => self.with_files(response),
            Ok(Err(e)) => json!(BridgeResponse::err(format!("Olo Bridge: the command failed here: {e}"))),
            Err(_) => {
                let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
                // (One nobody asked after again is stopped.)
                running.retain(|_, (since, work)| {
                    let keep = since.elapsed() < KEPT_FOR;
                    if !keep {
                        work.abort();
                    }
                    keep
                });
                running.insert(id.clone(), (Instant::now(), work));
                json!({ "pending": id })
            }
        }
    }

    /// The answer with the files it carries made fetchable (a file of
    /// this computer is no use to a program on the host): each listed
    /// under `files` with a name to fetch it by ([`Self::piece`]: `rex do
    /// olo.file id=…`, whose answer holds it in base64), and where on the
    /// host it goes (`to`) or that it is what the command printed
    /// (`stdout`).
    fn with_files(&self, mut response: BridgeResponse) -> Value {
        let mut files = Vec::new();
        let carried = std::mem::take(&mut response.carried);
        if !carried.is_empty() {
            let mut kept = self.files.lock().unwrap_or_else(|e| e.into_inner());
            kept.retain(|_, (since, _)| since.elapsed() < KEPT_FOR);
            for c in carried {
                let id = uuid::Uuid::new_v4().simple().to_string();
                let mut listed = json!({ "path": c.path, "id": id, "size": c.bytes.len() });
                if let Some(to) = c.to {
                    listed["to"] = json!(to);
                }
                if c.stdout {
                    listed["stdout"] = json!(true);
                }
                files.push(listed);
                kept.insert(id, (Instant::now(), Arc::new(c.bytes)));
            }
        }
        let mut answer = json!(response);
        if !files.is_empty() {
            answer["files"] = Value::Array(files);
        }
        answer
    }

    /// A piece of a file an answer named, from `offset`: its bytes in
    /// base64, how many they are, and whether they are the last.
    pub fn piece(&self, id: &str, offset: usize) -> Value {
        use base64::Engine as _;
        let mut kept = self.files.lock().unwrap_or_else(|e| e.into_inner());
        let Some((_, bytes)) = kept.get(id).cloned() else {
            return json!(BridgeResponse::err("Olo Bridge: that file is no longer kept"));
        };
        let from = offset.min(bytes.len());
        let to = (from + PIECE).min(bytes.len());
        let done = to == bytes.len();
        if done {
            kept.remove(id);
        }
        json!({ "ok": true, "base64": base64::engine::general_purpose::STANDARD.encode(&bytes[from..to]), "count": to - from, "done": done })
    }
}

/// A picture file of this computer's, to carry (none past the most).
fn picture(path: &str) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    (meta.is_file() && meta.len() <= FILE_MOST).then(|| std::fs::read(path).ok()).flatten()
}

/// A host's `op` run here as [`OpCarry`] has it: what it wrote to its
/// `-o` files and, where it isn't text, what it printed, carried back.
async fn run_op(op: &Path, carry: &OpCarry, host_label: &str) -> BridgeResponse {
    use tokio::io::AsyncWriteExt;
    let mut command = tokio::process::Command::new(op);
    // (`OLO_REX_OP_DIR`: the only folder it may read or write a file in.)
    command.args(&carry.args).env("OLO_REX_REMOTE_HOST", host_label).env("OLO_REX_OP_DIR", &carry.dir).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    // (Its folder, so no path it is given means one of this computer's.)
    if carry.dir.is_dir() {
        command.current_dir(&carry.dir);
    }
    command.stdin(if carry.stdin.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() });
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => return BridgeResponse::err(format!("failed to invoke olo-op: {e}")),
    };
    if let (Some(input), Some(mut pipe)) = (&carry.stdin, child.stdin.take()) {
        let input = input.clone();
        tokio::spawn(async move {
            let _ = pipe.write_all(&input).await;
        });
    }
    let o = match child.wait_with_output().await {
        Ok(o) => o,
        Err(e) => return BridgeResponse::err(format!("failed to invoke olo-op: {e}")),
    };
    let mut carried: Vec<Carried> = carry.outputs.iter().filter_map(|(to, here)| Some(Carried { path: to.clone(), to: Some(to.clone()), stdout: false, bytes: std::fs::read(here).ok()? })).collect();
    let stdout = match String::from_utf8(o.stdout) {
        Ok(text) => Some(text),
        Err(e) => {
            carried.push(Carried { path: "stdout".into(), to: None, stdout: true, bytes: e.into_bytes() });
            None
        }
    };
    BridgeResponse { ok: o.status.success(), stdout, stderr: Some(String::from_utf8_lossy(&o.stderr).into_owned()), data: Some(json!({ "code": o.status.code() })), carried }
}

/// The picture files a command's output names: every `"…/x.png"` in
/// it that is a whole path (Agent Screens answer in JSON, a picture's
/// path under `picture`; a path alone on a line is one too).
fn pictures_named(out: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut note = |path: &str| {
        if path.starts_with('/') && path.ends_with(".png") && !found.iter().any(|f| f == path) {
            found.push(path.to_owned());
        }
    };
    fn walk(v: &Value, note: &mut dyn FnMut(&str)) {
        match v {
            Value::String(s) => note(s),
            Value::Array(a) => a.iter().for_each(|v| walk(v, note)),
            Value::Object(o) => o.values().for_each(|v| walk(v, note)),
            _ => {}
        }
    }
    match serde_json::from_str::<Value>(out) {
        Ok(v) => walk(&v, &mut note),
        Err(_) => out.lines().for_each(|line| note(line.trim())),
    }
    found
}

/// A program of Olo's the bridge runs for a host (`olo-op`,
/// `olo-screen`). (`OLO_REX_BRIDGE_BIN`: looked for in that folder
/// first, for the rig's stand-ins.)
fn program(name: &str) -> Option<PathBuf> {
    std::env::var_os("OLO_REX_BRIDGE_BIN").map(|dir| PathBuf::from(dir).join(name)).filter(|p| p.is_file()).or_else(|| olo_xdg::find_program(name))
}

/// Callback used by the bridge to request an ephemeral `-L` port forward on a remote
/// Rex host before opening a remote `localhost:<port>` URL locally.
pub type LocalhostForwardFn = Arc<dyn Fn(String, u16) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<u16, String>> + Send>> + Send + Sync>;

/// What a session asked, done here as its host's permissions allow.
pub async fn handle_bridge_request(
    host_label: &str,
    policy: &HostBridgePolicy,
    req: BridgeRequest,
    forward_localhost: &LocalhostForwardFn,
    notify: &(dyn Fn(&BridgeRequest) + Sync),
) -> BridgeResponse {
    match req.cmd.as_str() {
        "open" => {
            if !policy.allow_open_url {
                return BridgeResponse::err(format!("Olo Bridge: URL opening is disabled for host '{host_label}' (see Preferences ▸ Rex)"));
            }
            let Some(target) = req.args.first().cloned() else {
                return BridgeResponse::err("missing URL/path argument");
            };
            match open_url_with_autoforward(&target, policy.allow_localhost_autoforward, Some(forward_localhost)).await {
                Ok(opened) => BridgeResponse { ok: true, stdout: Some(format!("Opened {opened}\n")), stderr: None, data: Some(json!({ "opened": opened })), carried: Vec::new() },
                Err(e) => BridgeResponse::err(e),
            }
        }
        "op" => {
            if !policy.allow_passwords_op {
                return BridgeResponse::err(format!("Olo Bridge: Passwords (`op`) forwarding is disabled for host '{host_label}'"));
            }
            // Olo's `olo-op` answers it here, asking Passwords (which asks
            // the person, naming the host), with nothing of this
            // computer's for the host to name: `OpCarry`.
            let Some(op_bin) = program("olo-op") else {
                return BridgeResponse::err("olo-op is not installed on this Olo desktop");
            };
            let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
            let dir = runtime.join(format!("olo-rex-op-{}", uuid::Uuid::new_v4().simple()));
            let carry = match OpCarry::of(&req.args, &req.payload, dir) {
                Ok(c) => c,
                Err(e) => return BridgeResponse::err(format!("Olo Bridge: {e}")),
            };
            let answer = run_op(&op_bin, &carry, host_label).await;
            // (Its folder, with what the host sent, gone with the run.)
            let _ = std::fs::remove_dir_all(&carry.dir);
            answer
        }
        "screen" => {
            if !policy.allow_agent_screens {
                return BridgeResponse::err(format!("Olo Bridge: Agent Screens (`olo-screen`) is disabled for host '{host_label}'"));
            }
            let Some(screen_bin) = program("olo-screen") else {
                return BridgeResponse::err("olo-screen is not installed on this Olo desktop");
            };
            // Auto-rewrite any remote localhost URL argument through a `-L` tunnel!
            let mut rewritten_args = Vec::with_capacity(req.args.len());
            for a in &req.args {
                if policy.allow_localhost_autoforward
                    && let Some((h, p, tmpl)) = parse_localhost_url(a)
                    && let Ok(local_port) = forward_localhost(h, p).await
                {
                    rewritten_args.push(tmpl.replace("{PORT}", &local_port.to_string()));
                } else {
                    rewritten_args.push(a.clone());
                }
            }
            let out = tokio::process::Command::new(screen_bin).args(&rewritten_args).stdin(std::process::Stdio::null()).output().await;
            match out {
                Ok(o) => {
                    let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
                    // (Its pictures, named in what it printed: Agent
                    // Screens' own, and the only files carried so.)
                    let carried = pictures_named(&stdout).into_iter().filter_map(|path| picture(&path).map(|bytes| Carried { path, to: None, stdout: false, bytes })).collect();
                    BridgeResponse {
                        ok: o.status.success(),
                        stdout: Some(stdout),
                        stderr: Some(String::from_utf8_lossy(&o.stderr).into_owned()),
                        data: Some(json!({ "code": o.status.code() })),
                        carried,
                    }
                }
                Err(e) => BridgeResponse::err(format!("failed to invoke olo-screen: {e}")),
            }
        }
        "notify" => {
            notify(&req);
            BridgeResponse { ok: true, stdout: None, stderr: None, data: None, carried: Vec::new() }
        }
        other => BridgeResponse::err(format!("unknown Olo Bridge command '{other}'")),
    }
}

/// Open a URL or file via Olo's native `olo_xdg::open` (triggering Olo Chrome's
/// window picker for web URLs), automatically creating a `-L` forward when a remote
/// `localhost:<port>` URL is given and `autoforward` is allowed.
pub async fn open_url_with_autoforward(raw: &str, autoforward: bool, forward_localhost: Option<&LocalhostForwardFn>) -> Result<String, String> {
    let final_target = if autoforward
        && let Some(fwd) = forward_localhost
        && let Some((host, port, tmpl)) = parse_localhost_url(raw)
    {
        let local_port = fwd(host, port).await?;
        tmpl.replace("{PORT}", &local_port.to_string())
    } else {
        raw.to_owned()
    };

    let parsed = Target::parse(&final_target);
    let target_clone = final_target.clone();
    tokio::task::spawn_blocking(move || olo_xdg::open::open(&parsed)).await.map_err(|e| format!("open task failed: {e}"))?.map_err(|e| format!("failed to open {target_clone}: {e}"))?;
    Ok(final_target)
}

/// Query Olo Passwords for an `op://` secret reference on a worker thread.
pub fn fetch_op_reference(reference: &str, purpose: &str) -> Result<String, String> {
    let req = olo_keychain::Request::Reference { reference: reference.to_owned(), purpose: purpose.to_owned() };
    match olo_keychain::call(&req) {
        Ok(olo_keychain::Response::Secrets { ref values }) => values.first().cloned().ok_or_else(|| "No matching secret returned from Olo Passwords".to_owned()),
        Ok(olo_keychain::Response::Locked) => Err("Olo Passwords is locked".to_owned()),
        Ok(olo_keychain::Response::Declined) => Err("Request declined in Olo Passwords".to_owned()),
        Ok(olo_keychain::Response::Error { ref message }) => Err(message.clone()),
        Ok(_) => Err("Unexpected response from Olo Passwords".to_owned()),
        Err(e) => Err(format!("Could not reach Olo Passwords: {e}")),
    }
}

/// Asks a host whether the skill for agents is on it (what
/// [`generate_remote_skill_install_script`] puts there), and whether
/// the shims of before are ([`OLD_SHIMS`]): a word for each that is.
pub const PRESENT: &str = r#"sh -c 'if [ -d "$HOME/.local/share/olo-rex-bridge" ]; then echo shims; fi; if [ -f "$HOME/.claude/skills/olo-rex-bridge/SKILL.md" ]; then echo skill; echo "skill-of $(cat "$HOME/.claude/skills/olo-rex-bridge/.olo-skill" 2> /dev/null)"; fi; if [ -f "$HOME/.local/share/olo/rex-commands/.olo-commands" ]; then echo commands; echo "commands-of $(cat "$HOME/.local/share/olo/rex-commands/.olo-commands" 2> /dev/null)"; fi'"#;

/// Which skill this Olo carries, by its words and its command's: what
/// the install leaves beside it on the host (`.olo-skill`), so that a
/// host with another Olo's skill is seen to have it and given this
/// one (an agent goes by what the skill there says, and one from
/// before said to do by hand what the command now does).
pub fn skill_stamp() -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in SKILL.bytes().chain(SKILL_OLO.bytes()).chain(SKILL_LUA.bytes()) {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// Whether what a host answered [`PRESENT`] says its skill is this
/// Olo's ([`skill_stamp`]).
pub fn skill_is_current(output: &str) -> bool {
    output.lines().any(|l| l.trim().strip_prefix("skill-of ").is_some_and(|stamp| stamp.trim() == skill_stamp()))
}

/// Takes off a host the shims Olo put there until 2026-11-23, which
/// did nothing (the module's head).
pub const OLD_SHIMS: &str = r#"rm -rf "$HOME/.local/share/olo-rex-bridge""#;

/// Takes the skill for agents off a host (what
/// [`generate_remote_skill_install_script`] put there; the skill that
/// Rex's own `rex skill setup` installs is Rex's, and stays).
pub const REMOVE_SKILL: &str = r#"rm -rf "$HOME/.claude/skills/olo-rex-bridge""#;

/// What a host answered [`PRESENT`]: whether the old shims are there,
/// and the skill.
pub fn present(output: &str) -> (bool, bool) {
    (output.lines().any(|l| l.trim() == "shims"), output.lines().any(|l| l.trim() == "skill"))
}

/// The skill for agents on a far host: what the Olo app offers through
/// Rex and how to ask for it; and its one command, `olo`, with the Lua
/// that `rex do` runs for it (which Olo desktop among the server's
/// clients, an answer that is pending waited for, a picture fetched
/// whole and written on the host: each took an agent a loop or a guess
/// by hand, "A helper on this host that saves a picture by its ID in
/// one step would make this much easier", 2026-11-23).
pub const SKILL: &str = include_str!("bridge_skill.md");
pub const SKILL_OLO: &str = include_str!("bridge_olo.sh");
pub const SKILL_LUA: &str = include_str!("bridge_olo.lua");

/// A shell script that puts [`SKILL`] on a far host with its command,
/// for Claude Code and the agents that read its skills.
pub fn generate_remote_skill_install_script() -> String {
    format!(
        r#"set -eu
SKILL_DIR="$HOME/.claude/skills/olo-rex-bridge"
mkdir -p "$SKILL_DIR"
cat <<'OLO_SKILL_EOF' > "$SKILL_DIR/SKILL.md"
{SKILL}OLO_SKILL_EOF
cat <<'OLO_SKILL_EOF' > "$SKILL_DIR/olo"
{SKILL_OLO}OLO_SKILL_EOF
cat <<'OLO_SKILL_EOF' > "$SKILL_DIR/olo.lua"
{SKILL_LUA}OLO_SKILL_EOF
chmod +x "$SKILL_DIR/olo"
echo '{stamp}' > "$SKILL_DIR/.olo-skill"
if command -v rex >/dev/null 2>&1; then
  rex skill setup 2>/dev/null || true
fi
echo "Installed Olo's skill for agents in $SKILL_DIR"
"#,
        stamp = skill_stamp()
    )
}

/// The desktop commands on a host (Ben, 2026-11-23: "have the rex client
/// support installing shims for xdg-open and other desktop integration
/// commands so that apps running remotely can operate like they're on the
/// local desktop"): one script ([`SHIM`]) under each of these names in a
/// folder of the account's home, beside the `olo` command it asks through,
/// put first on PATH by the shells' startup, and only in a Rex session.
/// A link or a file opened, a notification, the clipboard both ways, a
/// secret of Passwords: on the Olo desktop the session is shown on, as the
/// host may (Preferences ▸ Rex); anywhere else, the host's own command.
pub const SHIM: &str = include_str!("bridge_shim.sh");
pub const COMMANDS: [&str; 10] = ["xdg-open", "x-www-browser", "sensible-browser", "www-browser", "notify-send", "wl-copy", "wl-paste", "xclip", "xsel", "op"];
/// Where they go, under the account's home.
pub const COMMANDS_DIR: &str = ".local/share/olo/rex-commands";
/// The line each shell's startup file is given (and only that line is
/// taken off again): the folder first on PATH, and `$BROWSER` its
/// `xdg-open`, in a Rex session alone.
const COMMANDS_LINE: &str = r#"[ -n "${REX_SESSION:-}" ] && [ -d "$HOME/.local/share/olo/rex-commands" ] && case ":$PATH:" in *":$HOME/.local/share/olo/rex-commands:"*) ;; *) PATH="$HOME/.local/share/olo/rex-commands:$PATH"; BROWSER=xdg-open; export PATH BROWSER ;; esac # Olo Rex: desktop commands"#;
/// The same for fish, in a file of its own.
const COMMANDS_FISH: &str = "if set -q REX_SESSION; and test -d $HOME/.local/share/olo/rex-commands; and not contains $HOME/.local/share/olo/rex-commands $PATH; set -gx PATH $HOME/.local/share/olo/rex-commands $PATH; set -gx BROWSER xdg-open; end # Olo Rex: desktop commands\n";

/// Which desktop commands this Olo carries (what the install leaves
/// beside them, `.olo-commands`): a host with another Olo's is given
/// this one's as it is connected to.
pub fn commands_stamp() -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in SHIM.bytes().chain(SKILL_OLO.bytes()).chain(SKILL_LUA.bytes()).chain(COMMANDS_LINE.bytes()) {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// What a host answered [`PRESENT`]: whether the desktop commands are
/// there, and whether they are this Olo's.
pub fn commands_there(output: &str) -> (bool, bool) {
    let there = output.lines().any(|l| l.trim() == "commands");
    let current = output.lines().any(|l| l.trim().strip_prefix("commands-of ").is_some_and(|stamp| stamp.trim() == commands_stamp()));
    (there, there && current)
}

/// A shell script that puts the desktop commands on a far host: the
/// folder, its files, and the line in each shell's startup that there is
/// (`~/.profile` always, for a login shell's PATH), once.
pub fn install_commands_script() -> String {
    let links: String = COMMANDS.iter().map(|c| format!("ln -sf olo-desktop-command \"$DIR/{c}\"\n")).collect();
    format!(
        r#"set -eu
DIR="$HOME/{COMMANDS_DIR}"
mkdir -p "$DIR"
cat <<'OLO_COMMANDS_EOF' > "$DIR/olo"
{SKILL_OLO}OLO_COMMANDS_EOF
cat <<'OLO_COMMANDS_EOF' > "$DIR/olo.lua"
{SKILL_LUA}OLO_COMMANDS_EOF
cat <<'OLO_COMMANDS_EOF' > "$DIR/olo-desktop-command"
{SHIM}OLO_COMMANDS_EOF
chmod +x "$DIR/olo" "$DIR/olo-desktop-command"
{links}LINE='{line}'
for rc in "$HOME/.profile" "$HOME/.bashrc" "$HOME/.zshrc"; do
  case $rc in "$HOME/.profile") ;; *) [ -f "$rc" ] || continue ;; esac
  grep -qxF "$LINE" "$rc" 2> /dev/null || {{ sed -i '/# Olo Rex: desktop commands$/d' "$rc" 2> /dev/null || true; printf '%s\n' "$LINE" >> "$rc"; }}
done
if [ -d "$HOME/.config/fish" ]; then
  mkdir -p "$HOME/.config/fish/conf.d"
  printf '{fish}' > "$HOME/.config/fish/conf.d/olo-rex.fish"
fi
echo '{stamp}' > "$DIR/.olo-commands"
echo "Installed Olo's desktop commands in $DIR"
"#,
        line = COMMANDS_LINE.replace('\'', "'\\''"),
        fish = COMMANDS_FISH,
        stamp = commands_stamp()
    )
}

/// Takes the desktop commands off a host: their folder, and the line of
/// theirs in each shell's startup (nothing else of those files).
pub const REMOVE_COMMANDS: &str = r#"rm -rf "$HOME/.local/share/olo/rex-commands"; for rc in "$HOME/.profile" "$HOME/.bashrc" "$HOME/.zshrc"; do [ -f "$rc" ] && sed -i '/# Olo Rex: desktop commands$/d' "$rc"; done; rm -f "$HOME/.config/fish/conf.d/olo-rex.fish""#;

/// Check if a local directory path is safe to open in Olo Tracker.
pub fn open_folder_in_tracker(path: &Path) {
    let p = path.to_path_buf();
    let _ = std::thread::Builder::new().name("rex-tracker".into()).spawn(move || {
        if let Err(e) = olo_kit_base::tracker::open(&p) {
            tracing::warn!("open {} in Tracker failed: {e}", p.display());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_notice_keeps_the_callers_pane() {
        let (sent, mut delivered) = tokio::sync::mpsc::unbounded_channel();
        let notify = move |req: &BridgeRequest| {
            sent.send((req.session_id.clone(), req.block_id.clone(), req.payload.clone())).unwrap();
        };
        let forward: LocalhostForwardFn = Arc::new(|_, _| Box::pin(async { unreachable!() }));
        let payload = json!({"title":"Ready","body":"Output is available."});
        let req = BridgeRequest { cmd: "notify".into(), session_id: Some("work".into()), block_id: Some("two".into()), payload: payload.clone(), ..Default::default() };
        assert!(handle_bridge_request("Fixture", &HostBridgePolicy::remote_default(), req, &forward, &notify).await.ok);
        assert_eq!(delivered.try_recv().unwrap(), (Some("work".into()), Some("two".into()), payload));
    }

    /// What isn't granted is refused, by name, and nothing is run.
    #[tokio::test]
    async fn what_a_host_isnt_granted_is_refused() {
        let forward: LocalhostForwardFn = Arc::new(|_, _| Box::pin(async { unreachable!() }));
        for (cmd, word) in [("open", "URL opening"), ("op", "Passwords"), ("screen", "Agent Screens")] {
            let req = BridgeRequest { cmd: cmd.into(), args: vec!["x".into()], ..Default::default() };
            let answer = handle_bridge_request("build", &HostBridgePolicy::remote_default(), req, &forward, &|_| {}).await;
            assert!(!answer.ok && answer.stderr.as_deref().is_some_and(|e| e.contains(word) && e.contains("build")), "{cmd}: {answer:?}");
        }
    }

    /// A command that answers in time is answered at once; one that
    /// doesn't is answered with a name, and by that name when it has.
    #[tokio::test]
    async fn a_long_command_is_waited_for_by_name() {
        let within = Duration::from_millis(80);
        let calls = Calls::answering_within(within);
        let quick = calls.answer(async { BridgeResponse { ok: true, stdout: Some("done\n".into()), stderr: None, data: None, carried: Vec::new() } }).await;
        assert_eq!((quick["ok"].as_bool(), quick["stdout"].as_str()), (Some(true), Some("done\n")));
        let slow = calls
            .answer(async move {
                tokio::time::sleep(within * 2 + within / 2).await;
                BridgeResponse { ok: true, stdout: Some("at last\n".into()), stderr: None, data: None, carried: Vec::new() }
            })
            .await;
        let id = slow["pending"].as_str().expect("a name to wait by").to_owned();
        let again = calls.wait(&id).await;
        assert_eq!(again["pending"].as_str(), Some(id.as_str()), "still at it after a second wait");
        let over = calls.wait(&id).await;
        assert_eq!(over["stdout"].as_str(), Some("at last\n"));
        assert_eq!(calls.wait(&id).await["ok"].as_bool(), Some(false), "asked after once it was answered");
    }

    /// A picture an answer names goes to the host in pieces, whole.
    #[tokio::test]
    async fn a_picture_named_is_fetched_in_pieces() {
        use base64::Engine as _;
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-rex-bridge-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let picture = dir.join("look.png");
        let bytes: Vec<u8> = (0..PIECE * 2 + 1234).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(&picture, &bytes).unwrap();
        let calls = Calls::default();
        let out = json!({"after": {"picture": picture, "windows": []}, "note": "/not/there.png"}).to_string();
        // (As the screen's answer carries them: the pictures it names
        // that are there.)
        let carried = pictures_named(&out).into_iter().filter_map(|path| super::picture(&path).map(|bytes| Carried { path, to: None, stdout: false, bytes })).collect();
        let answer = calls.answer(async move { BridgeResponse { ok: true, stdout: Some(out), stderr: None, data: None, carried } }).await;
        let files = answer["files"].as_array().expect("the picture listed");
        assert_eq!(files.len(), 1, "only a file that is there");
        assert_eq!((files[0]["path"].as_str(), files[0]["size"].as_u64()), (picture.to_str(), Some(bytes.len() as u64)));
        let id = files[0]["id"].as_str().unwrap();
        let (mut got, mut pieces) = (Vec::new(), 0);
        loop {
            let piece = calls.piece(id, got.len());
            got.extend(base64::engine::general_purpose::STANDARD.decode(piece["base64"].as_str().unwrap()).unwrap());
            pieces += 1;
            if piece["done"].as_bool() == Some(true) {
                break;
            }
        }
        assert_eq!((got == bytes, pieces), (true, 3));
        assert_eq!(calls.piece(id, 0)["ok"].as_bool(), Some(false), "fetched once");
        assert_eq!(pictures_named("/var/tmp/a.png\nnot a path.png\n/var/tmp/b.txt\n"), vec!["/var/tmp/a.png".to_owned()]);
        // An answer that only names a picture (an item's value, `op`'s
        // output) carries nothing: only what its command carries.
        let named = json!({ "value": picture }).to_string();
        let answer = calls.answer(async move { BridgeResponse { ok: true, stdout: Some(named), stderr: None, data: None, carried: Vec::new() } }).await;
        assert!(answer.get("files").is_none(), "{answer}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A host's `op` names nothing of this computer's: `op run` isn't
    /// carried; a file its flags read is the host's, sent with the
    /// request (else refused), written here in a folder of its own; one
    /// it writes is written there and carried back to the host's path.
    #[test]
    fn a_hosts_op_reaches_no_file_of_this_computer() {
        use base64::Engine as _;
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-op-carry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let args = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        // `op run`: refused, whatever comes after.
        assert!(OpCarry::of(&args(&["run", "--", "sh", "-c", "id"]), &Value::Null, dir.clone()).is_err());
        assert!(OpCarry::of(&args(&["--account", "x", "run", "--", "id"]), &Value::Null, dir.clone()).is_err());
        // A file it reads that wasn't sent: refused, never this
        // computer's file of that name.
        let e = OpCarry::of(&args(&["inject", "-i", "/etc/passwd"]), &Value::Null, dir.clone()).err().unwrap();
        assert!(e.contains("wasn't sent"), "{e}");
        assert!(OpCarry::of(&args(&["item", "create", "--template=/home/me/t.json"]), &json!({}), dir.clone()).is_err());
        // Sent: written here, the argument this copy's path.
        let payload = json!({ "files": { "tpl.json": b64(br#"{"title":"T"}"#) }, "stdin": b64(b"from the host") });
        let c = OpCarry::of(&args(&["item", "create", "--template", "tpl.json", "--vault", "V", "-o", "out.json", "username=x"]), &payload, dir.clone()).unwrap();
        assert_eq!(c.stdin.as_deref(), Some(&b"from the host"[..]));
        let template = c.args.iter().find_map(|a| a.strip_prefix("--template=")).unwrap();
        assert!(template.starts_with(dir.to_str().unwrap()), "{template}");
        assert_eq!(std::fs::read_to_string(template).unwrap(), r#"{"title":"T"}"#);
        // (Its other arguments as they were.)
        assert!(c.args.contains(&"--vault".to_string()) && c.args.contains(&"V".to_string()) && c.args.contains(&"username=x".to_string()));
        // Written: there, and carried back to where the host said.
        let (to, here) = &c.outputs[0];
        assert_eq!(to, "out.json");
        assert!(here.starts_with(&dir) && c.args.contains(&format!("--o={}", here.display())));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod present_tests {
    use super::*;

    /// What is asked of a host is what its shell runs as meant, and
    /// what the skill's script puts there is what it finds: run here,
    /// in a scratch home. The shims of before are seen and taken off.
    #[test]
    fn a_host_is_asked_what_it_has_and_the_skill_is_what_it_finds() {
        let home = std::path::PathBuf::from(format!("/var/tmp/olo-rex-present-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".local/share/olo-rex-bridge/bin")).unwrap();
        let run = |command: &str| {
            let out = std::process::Command::new("sh").arg("-c").arg(command).env("HOME", &home).output().unwrap();
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        assert_eq!(present(&run(PRESENT)), (true, false));
        run(&format!("sh -c {}", crate::hub::shell_quote(OLD_SHIMS)));
        assert_eq!(present(&run(PRESENT)), (false, false));
        // (The skill's script goes on to `rex skill setup` where there
        // is a `rex`: not in this home's way, with no PATH of its own.
        // Put there the way it is put over SSH: the script as one
        // quoted word of a command, its own quotes within.)
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("sh -c {}", crate::hub::shell_quote(&generate_remote_skill_install_script())))
            .env("HOME", &home)
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(present(&run(PRESENT)), (false, true));
        // (And it is seen to be this Olo's; another's isn't.)
        assert!(skill_is_current(&run(PRESENT)));
        std::fs::write(home.join(".claude/skills/olo-rex-bridge/.olo-skill"), "0000\n").unwrap();
        assert!(!skill_is_current(&run(PRESENT)) && present(&run(PRESENT)).1);
        std::fs::remove_file(home.join(".claude/skills/olo-rex-bridge/.olo-skill")).unwrap();
        assert!(!skill_is_current(&run(PRESENT)) && present(&run(PRESENT)).1, "one from before had no stamp");
        assert_eq!(std::fs::read_to_string(home.join(".claude/skills/olo-rex-bridge/SKILL.md")).unwrap(), SKILL, "the skill as written, to the letter");
        assert_eq!(std::fs::read_to_string(home.join(".claude/skills/olo-rex-bridge/olo.lua")).unwrap(), SKILL_LUA);
        // (Its command, run with a stand-in for `rex` that says what it
        // was given: its arguments reach the Lua as they were typed,
        // whatever is in them.)
        let rex = home.join("fake-rex");
        std::fs::write(&rex, "#!/bin/sh\necho \"rex: $*\"\necho \"cmd=$OLO_BRIDGE_CMD argc=$OLO_BRIDGE_ARGC\"\ni=1; while [ $i -le \"$OLO_BRIDGE_ARGC\" ]; do eval \"printf '[%s]\\\\n' \\\"\\$OLO_BRIDGE_ARG_$i\\\"\"; i=$((i + 1)); done\n").unwrap();
        std::fs::set_permissions(&rex, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let said = std::process::Command::new(home.join(".claude/skills/olo-rex-bridge/olo"))
            .args(["screen", "type", "agent", "two words", "it's \"quoted\" $HOME", "", "a\nb"])
            .env("HOME", &home)
            .env("OLO_BRIDGE_REX", &rex)
            .env("TMPDIR", home.join("tmp"))
            .output()
            .unwrap();
        let lua = home.join(".claude/skills/olo-rex-bridge/olo.lua");
        assert_eq!(String::from_utf8_lossy(&said.stdout), format!("rex: --timeout 60s do {}\ncmd=screen argc=6\n[type]\n[agent]\n[two words]\n[it's \"quoted\" $HOME]\n[]\n[a\nb]\n", lua.display()));
        // (Every action the skill names is one the app offers, and it
        // names every one the app offers.)
        let offered: Vec<&str> = crate::keymap::bridge_catalog().iter().map(|a| a.id).collect();
        for action in &offered {
            assert!(SKILL.contains(&format!("rex do {action} ")), "the skill says how to ask for {action}");
        }
        let named: std::collections::BTreeSet<&str> = SKILL.split("rex do ").skip(1).filter_map(|rest| rest.split_whitespace().next()).filter(|word| word.starts_with("olo.")).collect();
        for action in named {
            assert!(offered.contains(&action), "the app offers {action}, which the skill names");
        }
        run(&format!("sh -c {}", crate::hub::shell_quote(REMOVE_SKILL)));
        assert_eq!(present(&run(PRESENT)), (false, false));
        let _ = std::fs::remove_dir_all(&home);
        assert_eq!(present("Linux\n"), (false, false));
    }
}

#[cfg(test)]
mod commands_tests {
    use super::*;

    /// The desktop commands as a host has them: installed the way they
    /// are over SSH (into a scratch home, its shells' startup files
    /// given their one line), seen and kept in step, and taken off with
    /// nothing else of those files.
    #[test]
    fn put_on_a_host_seen_and_taken_off() {
        let home = std::path::PathBuf::from(format!("/var/tmp/olo-rex-commands-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join(".bashrc"), "alias ll='ls -l'\n").unwrap();
        let run = |command: &str| {
            let out = std::process::Command::new("sh").arg("-c").arg(command).env("HOME", &home).env("PATH", "/usr/bin:/bin").output().unwrap();
            (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
        };
        assert_eq!(commands_there(&run(PRESENT).1), (false, false));
        let (ok, _, err) = run(&format!("sh -c {}", crate::hub::shell_quote(&install_commands_script())));
        assert!(ok, "{err}");
        assert_eq!(commands_there(&run(PRESENT).1), (true, true));
        for name in COMMANDS {
            let link = std::fs::read_link(home.join(COMMANDS_DIR).join(name)).unwrap();
            assert_eq!(link, std::path::PathBuf::from("olo-desktop-command"), "{name}");
        }
        // (Its line once in each startup file there, and .profile made;
        // a second install adds none.)
        run(&format!("sh -c {}", crate::hub::shell_quote(&install_commands_script())));
        let bashrc = std::fs::read_to_string(home.join(".bashrc")).unwrap();
        assert!(bashrc.starts_with("alias ll='ls -l'\n"), "{bashrc}");
        assert_eq!(bashrc.matches("# Olo Rex: desktop commands").count(), 1, "{bashrc}");
        assert_eq!(std::fs::read_to_string(home.join(".profile")).unwrap().matches("# Olo Rex: desktop commands").count(), 1);
        assert!(!home.join(".zshrc").exists(), "a startup file the shell there doesn't have isn't made");
        // (In a Rex session alone, the folder first on PATH.)
        let path_in = |session: bool| {
            let mut c = std::process::Command::new("sh");
            c.arg("-c").arg(". \"$HOME/.profile\"; printf %s \"$PATH\"").env("HOME", &home).env("PATH", "/usr/bin:/bin");
            if session {
                c.env("REX_SESSION", "session:1");
            } else {
                c.env_remove("REX_SESSION");
            }
            String::from_utf8_lossy(&c.output().unwrap().stdout).into_owned()
        };
        assert!(path_in(true).starts_with(&format!("{}/{COMMANDS_DIR}:", home.display())), "{}", path_in(true));
        assert_eq!(path_in(false), "/usr/bin:/bin");
        // (Another Olo's are seen not to be this one's.)
        std::fs::write(home.join(COMMANDS_DIR).join(".olo-commands"), "0000\n").unwrap();
        assert_eq!(commands_there(&run(PRESENT).1), (true, false));
        let (ok, _, err) = run(&format!("sh -c {}", crate::hub::shell_quote(REMOVE_COMMANDS)));
        assert!(ok, "{err}");
        assert_eq!(commands_there(&run(PRESENT).1), (false, false));
        assert_eq!(std::fs::read_to_string(home.join(".bashrc")).unwrap(), "alias ll='ls -l'\n");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// What each command does, with stand-ins for `olo` (what it was
    /// asked, and the status it answers) and for the host's own
    /// commands (what they were given).
    #[test]
    fn each_asks_the_desktop_or_is_the_hosts_own() {
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-rex-shim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (shims, own) = (dir.join("shims"), dir.join("own"));
        std::fs::create_dir_all(&shims).unwrap();
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(shims.join("olo-desktop-command"), SHIM).unwrap();
        // (`olo`: writes what it was asked; answers OLO_STATUS, and for
        // `paste` what OLO_PASTED holds.)
        std::fs::write(shims.join("olo"), "#!/bin/sh\nprintf '%s|' \"$@\" >> \"$LOG\"; echo >> \"$LOG\"; [ \"$1\" = paste ] && printf '%s' \"$OLO_PASTED\"; exit ${OLO_STATUS:-0}\n").unwrap();
        for name in COMMANDS {
            std::os::unix::fs::symlink("olo-desktop-command", shims.join(name)).unwrap();
            // (The host's own: says it ran, its arguments and its input.)
            std::fs::write(
                own.join(name),
                format!("#!/bin/sh\nprintf 'own {name}: ' >> \"$LOG\"; [ $# -gt 0 ] && printf '%s|' \"$@\" >> \"$LOG\"; input=$(cat); [ -n \"$input\" ] && printf 'in=%s' \"$input\" >> \"$LOG\"; echo >> \"$LOG\"\n"),
            )
            .unwrap();
        }
        for f in std::fs::read_dir(&shims).unwrap().chain(std::fs::read_dir(&own).unwrap()) {
            let p = f.unwrap().path();
            if !p.is_symlink() {
                std::os::unix::fs::PermissionsExt::set_mode(&mut std::fs::metadata(&p).unwrap().permissions(), 0o755);
                std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
            }
        }
        let log = dir.join("log");
        let run = |line: &str, session: bool, status: i32, input: Option<&str>| -> (i32, String, String) {
            let _ = std::fs::remove_file(&log);
            let mut c = std::process::Command::new("sh");
            c.arg("-c")
                .arg(line)
                .current_dir(&dir)
                .env("PATH", format!("{}:{}:/usr/bin:/bin", shims.display(), own.display()))
                .env("LOG", &log)
                .env("OLO_STATUS", status.to_string())
                .env("OLO_PASTED", "from the desktop");
            if session {
                c.env("REX_SESSION", "session:1");
            } else {
                c.env_remove("REX_SESSION");
            }
            c.stdin(if input.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() }).stdout(std::process::Stdio::piped());
            let mut child = c.spawn().unwrap();
            if let Some(text) = input {
                std::io::Write::write_all(&mut child.stdin.take().unwrap(), text.as_bytes()).unwrap();
            }
            let out = child.wait_with_output().unwrap();
            (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), std::fs::read_to_string(&log).unwrap_or_default())
        };
        // Outside a Rex session: the host's own, as given.
        assert_eq!(run("notify-send -u low Hi there", false, 0, None).2, "own notify-send: -u|low|Hi|there|\n");
        // In one: asked of the desktop, its options left to it.
        assert_eq!(run("notify-send -u low -i dialog Hi 'the body'", true, 0, None).2, "notify|Hi|the body|\n");
        // Refused or no desktop (69): the host's own, with what it was given.
        assert_eq!(run("notify-send Hi", true, 69, None).2, "notify|Hi||\nown notify-send: Hi|\n");
        // A failure of the desktop's own is the command's: not tried again here.
        assert_eq!(run("notify-send Hi", true, 3, None).0, 3);
        // A link, and a file made whole.
        assert_eq!(run("xdg-open https://example.com", true, 0, None).2, "open|https://example.com|\n");
        std::fs::write(dir.join("report.txt"), "x").unwrap();
        assert_eq!(run("xdg-open report.txt", true, 0, None).2, format!("open|{}/report.txt|\n", std::fs::canonicalize(&dir).unwrap().display()));
        // The clipboard: what comes in, its last newline kept (or taken
        // with -n); the host's own given it again when refused.
        assert_eq!(run("wl-copy", true, 0, Some("two\nlines\n")).2, "clipboard|two\nlines\n|\n");
        assert_eq!(run("wl-copy -n", true, 0, Some("one\n")).2, "clipboard|one|\n");
        assert_eq!(run("wl-copy", true, 69, Some("kept")).2, "clipboard|kept|\nown wl-copy: in=kept\n");
        assert_eq!(run("xclip -selection clipboard", true, 0, Some("x")).2, "clipboard|x|\n");
        let (code, out, _) = run("wl-paste", true, 0, None);
        assert_eq!((code, out.as_str()), (0, "from the desktop\n"));
        assert_eq!(run("wl-paste -n", true, 0, None).1, "from the desktop");
        assert_eq!(run("xclip -o -selection clipboard", true, 0, None).1, "from the desktop");
        assert_eq!(run("xsel --clipboard --output", true, 0, None).1, "from the desktop");
        assert_eq!(run("wl-paste", true, 69, None).2, "paste|\nown wl-paste: \n");
        // Passwords' `op`, as given.
        assert_eq!(run("op read op://Vault/Item/field", true, 0, None).2, "op|read|op://Vault/Item/field|\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
