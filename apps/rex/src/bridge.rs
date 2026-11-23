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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl BridgeResponse {
    pub fn err(msg: impl Into<String>) -> Self {
        Self { ok: false, stdout: None, stderr: Some(msg.into()), data: None }
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

    /// The answer with the pictures it names made fetchable: a file of
    /// this computer is no use to a program on the host, so each
    /// picture file the command's output names (as Agent Screens name
    /// theirs: a path ending `.png`) is read now and listed under
    /// `files` with a name to fetch it by ([`Self::piece`]: `rex do
    /// olo.file id=…`, whose answer holds it in base64).
    fn with_files(&self, response: BridgeResponse) -> Value {
        let mut files = Vec::new();
        if let Some(out) = response.stdout.as_deref() {
            let mut kept = self.files.lock().unwrap_or_else(|e| e.into_inner());
            kept.retain(|_, (since, _)| since.elapsed() < KEPT_FOR);
            for path in pictures_named(out) {
                let Ok(meta) = std::fs::metadata(&path) else { continue };
                if !meta.is_file() || meta.len() > FILE_MOST {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&path) else { continue };
                let id = uuid::Uuid::new_v4().simple().to_string();
                files.push(json!({ "path": path, "id": id, "size": bytes.len() }));
                kept.insert(id, (Instant::now(), Arc::new(bytes)));
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
                Ok(opened) => BridgeResponse { ok: true, stdout: Some(format!("Opened {opened}\n")), stderr: None, data: Some(json!({ "opened": opened })) },
                Err(e) => BridgeResponse::err(e),
            }
        }
        "op" => {
            if !policy.allow_passwords_op {
                return BridgeResponse::err(format!("Olo Bridge: Passwords (`op`) forwarding is disabled for host '{host_label}'"));
            }
            // Forward `op` arguments to Olo's native `olo-op` binary on the local desktop,
            // which talks directly to Olo Passwords' keychain socket and shows Olo's native
            // approval / biometric sheet!
            let Some(op_bin) = program("olo-op") else {
                return BridgeResponse::err("olo-op is not installed on this Olo desktop");
            };
            let out = tokio::process::Command::new(op_bin).args(&req.args).env("OLO_REX_REMOTE_HOST", host_label).stdin(std::process::Stdio::null()).output().await;
            match out {
                Ok(o) => BridgeResponse {
                    ok: o.status.success(),
                    stdout: Some(String::from_utf8_lossy(&o.stdout).into_owned()),
                    stderr: Some(String::from_utf8_lossy(&o.stderr).into_owned()),
                    data: Some(json!({ "code": o.status.code() })),
                },
                Err(e) => BridgeResponse::err(format!("failed to invoke olo-op: {e}")),
            }
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
                Ok(o) => BridgeResponse {
                    ok: o.status.success(),
                    stdout: Some(String::from_utf8_lossy(&o.stdout).into_owned()),
                    stderr: Some(String::from_utf8_lossy(&o.stderr).into_owned()),
                    data: Some(json!({ "code": o.status.code() })),
                },
                Err(e) => BridgeResponse::err(format!("failed to invoke olo-screen: {e}")),
            }
        }
        "notify" => {
            notify(&req);
            BridgeResponse { ok: true, stdout: None, stderr: None, data: None }
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
pub const PRESENT: &str = r#"sh -c 'if [ -d "$HOME/.local/share/olo-rex-bridge" ]; then echo shims; fi; if [ -f "$HOME/.claude/skills/olo-rex-bridge/SKILL.md" ]; then echo skill; echo "skill-of $(cat "$HOME/.claude/skills/olo-rex-bridge/.olo-skill" 2> /dev/null)"; fi'"#;

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
        let quick = calls.answer(async { BridgeResponse { ok: true, stdout: Some("done\n".into()), stderr: None, data: None } }).await;
        assert_eq!((quick["ok"].as_bool(), quick["stdout"].as_str()), (Some(true), Some("done\n")));
        let slow = calls
            .answer(async move {
                tokio::time::sleep(within * 2 + within / 2).await;
                BridgeResponse { ok: true, stdout: Some("at last\n".into()), stderr: None, data: None }
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
        let answer = calls.answer(async move { BridgeResponse { ok: true, stdout: Some(out), stderr: None, data: None } }).await;
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
