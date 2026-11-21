//! Zero-trust Olo Desktop Bridge for local and remote Rex hosts:
//!
//! - Local shell `path_prepend` directory exposing Olo's native `xdg-open` (`olo-open`),
//!   `sudo` (`olo-sudo`), and `op` (`olo-op`).
//! - Tier 1: `client.perform` / `client.call` handlers (`olo.open`, `olo.notify`,
//!   `olo.clipboard.set`, `olo.screen.*`, `olo.passwords.*`) gated strictly by
//!   per-host [`HostBridgePolicy`].
//! - Tier 2: Opt-in reverse-tunneled bridge server (`com.superlogical.tunnel.listen`
//!   forwarding to an ephemeral local TCP listener authenticated with a per-session
//!   random bearer token), enabling drop-in remote `xdg-open`, `op`, and `olo-screen`
//!   shims on any remote Linux host!

use std::{
    net::SocketAddr,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    sync::Arc,
};

use olo_kit_base::notify::{Note, NoteKind, Notifier};
use olo_xdg::open::Target;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

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

/// Request sent over the authenticated Tier 2 reverse bridge tunnel from a remote
/// host's `olo-rex-shim` (`xdg-open`, `op`, `olo-screen`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeRequest {
    pub token: String,
    pub cmd: String,
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

/// Callback used by the bridge to request an ephemeral `-L` port forward on a remote
/// Rex host before opening a remote `localhost:<port>` URL locally.
pub type LocalhostForwardFn = Arc<dyn Fn(String, u16) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<u16, String>> + Send>> + Send + Sync>;

/// Running local TCP server serving one remote host's Tier 2 Olo Bridge reverse tunnel.
pub struct BridgeServerHandle {
    pub local_addr: SocketAddr,
    #[allow(dead_code)]
    pub token: String,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Drop for BridgeServerHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

/// Start an ephemeral local `127.0.0.1:0` bridge server authenticated by a fresh
/// random UUID token and governed by `policy_fn`.
pub async fn start_bridge_server(host_label: String, policy_fn: Arc<dyn Fn() -> HostBridgePolicy + Send + Sync>, forward_localhost: LocalhostForwardFn) -> std::io::Result<BridgeServerHandle> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let local_addr = listener.local_addr()?;
    let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let expected_token = token.clone();
    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut rx => break,
                res = listener.accept() => {
                    let Ok((stream, _)) = res else { break };
                    let expected = expected_token.clone();
                    let label = host_label.clone();
                    let get_policy = policy_fn.clone();
                    let fwd = forward_localhost.clone();
                    tokio::spawn(async move {
                        let (r, mut w) = stream.into_split();
                        let mut reader = BufReader::new(r);
                        let mut line = String::new();
                        if reader.read_line(&mut line).await.is_ok() && !line.trim().is_empty() {
                            let resp = match serde_json::from_str::<BridgeRequest>(line.trim()) {
                                Ok(req) if req.token == expected => {
                                    let policy = get_policy();
                                    handle_bridge_request(&label, &policy, req, &fwd).await
                                }
                                Ok(_) => BridgeResponse::err("unauthorized bridge token"),
                                Err(e) => BridgeResponse::err(format!("invalid bridge request: {e}")),
                            };
                            if let Ok(mut bytes) = serde_json::to_vec(&resp) {
                                bytes.push(b'\n');
                                let _ = w.write_all(&bytes).await;
                            }
                        }
                    });
                }
            }
        }
    });

    Ok(BridgeServerHandle { local_addr, token, shutdown: Some(tx) })
}

async fn handle_bridge_request(host_label: &str, policy: &HostBridgePolicy, req: BridgeRequest, forward_localhost: &LocalhostForwardFn) -> BridgeResponse {
    match req.cmd.as_str() {
        "open" => {
            if !policy.allow_open_url {
                return BridgeResponse::err(format!("Olo Bridge: URL opening is disabled for host '{host_label}' (enable in Rex Settings → Olo Integration)"));
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
            let Some(op_bin) = olo_xdg::find_program("olo-op") else {
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
            let Some(screen_bin) = olo_xdg::find_program("olo-screen") else {
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
            let title = req.payload.get("title").and_then(|v| v.as_str()).unwrap_or("Rex");
            let body = req.payload.get("body").and_then(|v| v.as_str()).unwrap_or("");
            send_desktop_notification(title, body, None);
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

/// Dispatch an Olo desktop notification on a background thread (never blocks UI).
pub fn send_desktop_notification(title: &str, body: &str, focus_args: Option<Vec<String>>) {
    let title = title.to_owned();
    let body = body.to_owned();
    let _ = std::thread::Builder::new().name("rex-notify".into()).spawn(move || {
        let mut note = Note::new(&title).kind(NoteKind::Important).icon("rex");
        if !body.is_empty() {
            note = note.body(&body);
        }
        if let Some(args) = focus_args {
            note = note.action("default", "Focus Pane").shows(args);
        }
        let notifier = Notifier::new(olo_kit::proxy(), "Rex", "olo-rex");
        let _ = notifier.show(None, note);
    });
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

/// Generate a self-contained POSIX installer shell script that installs `xdg-open`,
/// `op`, and `olo-screen` remote shims under `~/.local/share/olo-rex-bridge/bin`
/// on a remote Linux host (talking back over `OLO_REX_BRIDGE_PORT` + `OLO_REX_BRIDGE_TOKEN`).
pub fn generate_remote_shim_install_script() -> String {
    r#"set -eu
DIR="$HOME/.local/share/olo-rex-bridge/bin"
mkdir -p "$DIR"
cat <<'EOF' > "$DIR/_olo_bridge_call"
#!/usr/bin/env python3
import json, os, socket, sys

port = int(os.environ.get("OLO_REX_BRIDGE_PORT", "0"))
token = os.environ.get("OLO_REX_BRIDGE_TOKEN", "")
if not port or not token:
    sys.stderr.write("olo-rex-bridge: not connected (enable Olo Bridge for this host in Olo Rex Settings)\n")
    sys.exit(1)

cmd = sys.argv[1]
args = sys.argv[2:]
req = json.dumps({"token": token, "cmd": cmd, "args": args}) + "\n"
with socket.create_connection(("127.0.0.1", port), timeout=120) as sock:
    sock.sendall(req.encode("utf-8"))
    sock.shutdown(socket.SHUT_WR)
    chunks = []
    while True:
        buf = sock.recv(65536)
        if not buf:
            break
        chunks.append(buf)
resp = json.loads(b"".join(chunks).decode("utf-8"))
if resp.get("stdout"):
    sys.stdout.write(resp["stdout"])
if resp.get("stderr"):
    sys.stderr.write(resp["stderr"])
sys.exit(0 if resp.get("ok") else 1)
EOF
chmod +x "$DIR/_olo_bridge_call"
for name in xdg-open op olo-screen; do
  case "$name" in
    xdg-open) sub="open" ;;
    op) sub="op" ;;
    olo-screen) sub="screen" ;;
  esac
  cat <<EOF > "$DIR/$name"
#!/bin/sh
exec "\$HOME/.local/share/olo-rex-bridge/bin/_olo_bridge_call" "$sub" "\$@"
EOF
  chmod +x "$DIR/$name"
done
echo "Installed Olo Rex Bridge shims in $DIR"
"#
    .to_owned()
}

/// Generate the remote agent skill documenting Rex (`rex skill setup`) + Olo
/// Desktop Bridge (`xdg-open`, `op`, `olo-screen`, `/var/tmp`).
pub fn generate_remote_skill_install_script() -> String {
    r#"set -eu
SKILL_DIR="$HOME/.claude/skills/olo-rex-bridge"
mkdir -p "$SKILL_DIR"
cat <<'EOF' > "$SKILL_DIR/SKILL.md"
---
name: olo-rex-bridge
description: Use Olo Rex Desktop Bridge shims (xdg-open, op, olo-screen) and Rex CLI commands from inside a Rex session connected to an Olo desktop.
---

# Olo Rex Desktop Bridge

When running inside a Rex session attached to **Olo Rex**:
- `xdg-open <url>`: Opens URLs in the person's Olo Chrome (automatically tunneling `http://localhost:<port>` over Rex `-L` when enabled!).
- `op <args>`: Queries the person's local Olo Passwords vault with native biometric/YubiKey approval (no credentials are ever stored on this remote host).
- `olo-screen <args>`: Drives headless Olo Agent Screens on the person's desktop (e.g. `olo-screen ensure NAME`, `olo-screen run NAME ...`, `olo-screen look NAME`) with remote `localhost` URLs automatically forwarded!
- `rex` CLI: Use `rex session`, `rex window`, `rex pane`, `rex layer`, `rex prompt` to manage workspaces natively.
EOF
if command -v rex >/dev/null 2>&1; then
  rex skill setup 2>/dev/null || true
fi
echo "Installed Olo Rex Bridge skill in $SKILL_DIR"
"#
    .to_owned()
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
