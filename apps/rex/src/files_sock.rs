//! Local Unix socket server exposing Rex's connected SSH hosts to Olo Tracker
//! (`rex://HOST/PATH`).
//!
//! Tracker never opens its own SSH connections: all listing, stat, mkdir,
//! rename, copy, delete, file read/write, and `olo-rex-sync` transfers go
//! through Rex's existing `SshSessionPool` and `olo-rex-link` helper on each
//! host. Every connection is authenticated via `SO_PEERCRED` (same UID only)
//! and bound with mode `0600` at [`olo_kit_base::tracker::rex_files_socket`].

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use base64::Engine as _;
use olo_rex_client::{
    RexEndpoint, SshAskpass, SshSessionPool, SshTarget,
    helper::{Asked, ask as helper_ask, file as helper_file, put as helper_put},
    provision::shell_word,
    sync::{HostFolder, Stop, receive_from, receive_paths, send_paths, send_to},
};
use olo_rex_sync::wire::FolderSource;
use olo_sync::Mutex;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
};

use crate::hub::{HostConnState, HostHelper, HubState};

struct ResolvedHost {
    key: String,
    label: String,
    target: SshTarget,
    pool: Arc<SshSessionPool>,
    askpass: Option<Arc<dyn SshAskpass>>,
    helper_ready: bool,
}

fn conn_state_str(s: &HostConnState) -> (&'static str, Option<String>) {
    match s {
        HostConnState::Connected => ("connected", None),
        HostConnState::Connecting => ("connecting", None),
        HostConnState::Reconnecting { last_error, .. } => ("reconnecting", (!last_error.is_empty()).then(|| last_error.clone())),
        HostConnState::Disconnected => ("disconnected", None),
        HostConnState::Error(e) => ("error", Some(e.clone())),
    }
}

fn find_host_key(st: &HubState, host: &str) -> Option<String> {
    if let Some(h) = st.hosts.values().find(|h| !h.is_local && h.label == host) {
        return Some(h.key.clone());
    }
    if let Some(h) = st.hosts.values().find(|h| !h.is_local && h.label.eq_ignore_ascii_case(host)) {
        return Some(h.key.clone());
    }
    st.hosts.get(host).filter(|h| !h.is_local).map(|h| h.key.clone())
}

fn resolve_host(state: &Arc<Mutex<HubState>>, host: &str) -> Result<ResolvedHost, String> {
    let st = state.lock().unwrap_or_else(|e| e.into_inner());
    let Some(key) = find_host_key(&st, host) else {
        return Err(format!("“{host}” isn’t one of Rex’s hosts."));
    };
    let h = &st.hosts[&key];
    match &h.conn_state {
        HostConnState::Connected => {}
        HostConnState::Connecting => return Err(format!("Connecting to “{}”…", h.label)),
        HostConnState::Reconnecting { .. } => return Err(format!("Reconnecting to “{}”…", h.label)),
        HostConnState::Disconnected => return Err(format!("“{}” isn’t connected in Rex.", h.label)),
        HostConnState::Error(e) => return Err(format!("“{}” isn’t connected: {e}", h.label)),
    }
    let Some(RexEndpoint::Ssh { target, pool, askpass }) = h.endpoint.clone() else {
        return Err(format!("“{}” isn’t reached over SSH.", h.label));
    };
    let helper_ready = matches!(h.helper, HostHelper::Used | HostHelper::Ready);
    Ok(ResolvedHost { key, label: h.label.clone(), target, pool, askpass, helper_ready })
}

async fn ensure_helper(state: &Arc<Mutex<HubState>>, host: &ResolvedHost) -> Result<(), String> {
    if host.helper_ready {
        return Ok(());
    }
    if !crate::link::carried() || !crate::link::syncs() {
        return Err(format!("Olo’s helper isn’t built with remote file support for “{}”.", host.label));
    }
    match helper_ask(&host.pool, &host.target, host.askpass.clone(), crate::link::ID, &crate::link::BUILDS).await {
        Ok(Ok(Asked::Here)) => {}
        Ok(Ok(Asked::Needs(build))) => {
            helper_put(&host.pool, &host.target, host.askpass.clone(), crate::link::ID, build).await.map_err(|e| format!("Couldn’t install Olo’s helper on “{}”: {}", host.label, e.why))?;
        }
        Ok(Err(kind)) => return Err(format!("No Olo helper build is available for “{}” ({kind}).", host.label)),
        Err(e) => return Err(format!("Couldn’t reach “{}”: {}", host.label, e.why)),
    }
    let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(hr) = st.hosts.get_mut(&host.key)
        && !matches!(hr.helper, HostHelper::Used)
    {
        hr.helper = HostHelper::Ready;
    }
    Ok(())
}

fn parse_helper_json(ran_output: &str, host_label: &str) -> Result<Value, String> {
    let trimmed = ran_output.trim();
    let line = trimmed.lines().rev().find(|l| l.trim().starts_with('{')).unwrap_or(trimmed);
    let val: Value = serde_json::from_str(line).map_err(|_| if trimmed.is_empty() { format!("“{host_label}” closed the connection before answering.") } else { trimmed.to_owned() })?;
    if val.get("ok").and_then(Value::as_bool) == Some(true) { Ok(val) } else { Err(val.get("error").and_then(Value::as_str).unwrap_or("remote filesystem operation failed").to_owned()) }
}

async fn run_fs_json(state: &Arc<Mutex<HubState>>, host: &ResolvedHost, subcmd: &str) -> Result<Value, String> {
    ensure_helper(state, host).await?;
    let cmd = format!("{} fs {subcmd}", helper_file(crate::link::ID));
    let ran = host.pool.exec(&host.target, host.askpass.clone(), &cmd, &mut |_| {}).await.map_err(|e| e.to_string())?;
    parse_helper_json(&ran.output, &host.label)
}

fn cache_file_for(host_label: &str, remote_path: &str) -> PathBuf {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (host_label, remote_path).hash(&mut h);
    let host_dir: String = host_label.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_') { c } else { '_' }).collect();
    let name = remote_path.rsplit('/').find(|s| !s.is_empty() && *s != "." && *s != "..").unwrap_or("file");
    olo_xdg::cache_dir().join("tracker-rex").join(host_dir).join(format!("{:016x}", h.finish())).join(name)
}

async fn write_line(w: &mut tokio::net::unix::OwnedWriteHalf, v: &Value) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(v).unwrap_or_else(|_| b"{\"ok\":false}\n".to_vec());
    bytes.push(b'\n');
    w.write_all(&bytes).await?;
    w.flush().await
}

fn parent_and_leaf(path: &str) -> Option<(String, String)> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return None;
    }
    match trimmed.rfind('/') {
        Some(0) => Some(("/".to_owned(), trimmed[1..].to_owned())),
        Some(i) => Some((trimmed[..i].to_owned(), trimmed[i + 1..].to_owned())),
        None => Some(("~".to_owned(), trimmed.to_owned())),
    }
}

async fn handle_client(stream: UnixStream, state: Arc<Mutex<HubState>>) {
    if let Ok(cred) = stream.peer_cred() {
        // SAFETY: geteuid is a pure read of the current process's effective UID.
        let my_uid = unsafe { libc::geteuid() };
        if cred.uid() != my_uid {
            return;
        }
    }
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    match reader.read_line(&mut line).await {
        Ok(0) | Err(_) => return,
        Ok(_) => {}
    }
    let req: Value = match serde_json::from_str(line.trim()) {
        Ok(v) => v,
        Err(e) => {
            let _ = write_line(&mut write_half, &json!({ "ok": false, "error": format!("invalid request: {e}") })).await;
            return;
        }
    };
    let op = req.get("op").and_then(Value::as_str).unwrap_or("");
    let str_arg = |k: &str| req.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let bool_arg = |k: &str| req.get(k).and_then(Value::as_bool).unwrap_or(false);

    match op {
        "hosts" => {
            let hosts: Vec<Value> = {
                let st = state.lock().unwrap_or_else(|e| e.into_inner());
                st.host_order
                    .iter()
                    .filter_map(|k| st.hosts.get(k))
                    .filter(|h| !h.is_local)
                    .map(|h| {
                        let (state_s, err) = conn_state_str(&h.conn_state);
                        json!({
                            "key": h.key,
                            "label": h.label,
                            "state": state_s,
                            "error": err,
                            "ssh": matches!(h.endpoint, Some(RexEndpoint::Ssh { .. })) || h.endpoint_raw.starts_with("ssh://"),
                            "helper": matches!(h.helper, HostHelper::Used | HostHelper::Ready),
                        })
                    })
                    .collect()
            };
            let _ = write_line(&mut write_half, &json!({ "ok": true, "hosts": hosts })).await;
        }
        "host_state" => {
            let host = str_arg("host");
            let resp = {
                let st = state.lock().unwrap_or_else(|e| e.into_inner());
                match find_host_key(&st, &host).and_then(|k| st.hosts.get(&k)) {
                    Some(h) => {
                        let (state_s, err) = conn_state_str(&h.conn_state);
                        json!({ "ok": true, "found": true, "key": h.key, "label": h.label, "state": state_s, "error": err })
                    }
                    None => json!({ "ok": true, "found": false }),
                }
            };
            let _ = write_line(&mut write_half, &resp).await;
        }
        "list" => {
            let host_name = str_arg("host");
            let path = str_arg("path");
            let hidden = bool_arg("hidden");
            let resp = match resolve_host(&state, &host_name) {
                Ok(host) => {
                    let sub = if hidden { format!("list {} --hidden", shell_word(&path)) } else { format!("list {}", shell_word(&path)) };
                    match run_fs_json(&state, &host, &sub).await {
                        Ok(mut v) => {
                            v["host"] = json!(host.label);
                            v
                        }
                        Err(e) => json!({ "ok": false, "error": e }),
                    }
                }
                Err(e) => json!({ "ok": false, "error": e }),
            };
            let _ = write_line(&mut write_half, &resp).await;
        }
        "stat" => {
            let host_name = str_arg("host");
            let path = str_arg("path");
            let resp = match resolve_host(&state, &host_name) {
                Ok(host) => match run_fs_json(&state, &host, &format!("stat {}", shell_word(&path))).await {
                    Ok(mut v) => {
                        v["host"] = json!(host.label);
                        v
                    }
                    Err(e) => json!({ "ok": false, "error": e }),
                },
                Err(e) => json!({ "ok": false, "error": e }),
            };
            let _ = write_line(&mut write_half, &resp).await;
        }
        "mkdir" => {
            let host_name = str_arg("host");
            let path = str_arg("path");
            let unique = bool_arg("unique");
            let resp = match resolve_host(&state, &host_name) {
                Ok(host) => {
                    let sub = if unique { format!("mkdir {} --unique", shell_word(&path)) } else { format!("mkdir {}", shell_word(&path)) };
                    match run_fs_json(&state, &host, &sub).await {
                        Ok(mut v) => {
                            v["host"] = json!(host.label);
                            v
                        }
                        Err(e) => json!({ "ok": false, "error": e }),
                    }
                }
                Err(e) => json!({ "ok": false, "error": e }),
            };
            let _ = write_line(&mut write_half, &resp).await;
        }
        "rename" => {
            let host_name = str_arg("host");
            let from = str_arg("from");
            let to = str_arg("to");
            let unique = bool_arg("unique");
            let resp = match resolve_host(&state, &host_name) {
                Ok(host) => {
                    let sub = if unique { format!("rename {} {} --unique", shell_word(&from), shell_word(&to)) } else { format!("rename {} {}", shell_word(&from), shell_word(&to)) };
                    match run_fs_json(&state, &host, &sub).await {
                        Ok(mut v) => {
                            v["host"] = json!(host.label);
                            v
                        }
                        Err(e) => json!({ "ok": false, "error": e }),
                    }
                }
                Err(e) => json!({ "ok": false, "error": e }),
            };
            let _ = write_line(&mut write_half, &resp).await;
        }
        "copy_remote" => {
            let host_name = str_arg("host");
            let from = str_arg("from");
            let to = str_arg("to");
            let resp = match resolve_host(&state, &host_name) {
                Ok(host) => match run_fs_json(&state, &host, &format!("copy {} {}", shell_word(&from), shell_word(&to))).await {
                    Ok(mut v) => {
                        v["host"] = json!(host.label);
                        v
                    }
                    Err(e) => json!({ "ok": false, "error": e }),
                },
                Err(e) => json!({ "ok": false, "error": e }),
            };
            let _ = write_line(&mut write_half, &resp).await;
        }
        "remove" => {
            let host_name = str_arg("host");
            let path = str_arg("path");
            let resp = match resolve_host(&state, &host_name) {
                Ok(host) => match run_fs_json(&state, &host, &format!("remove {}", shell_word(&path))).await {
                    Ok(v) => v,
                    Err(e) => json!({ "ok": false, "error": e }),
                },
                Err(e) => json!({ "ok": false, "error": e }),
            };
            let _ = write_line(&mut write_half, &resp).await;
        }
        "write_file" => {
            let host_name = str_arg("host");
            let path = str_arg("path");
            let unique = bool_arg("unique");
            let b64 = str_arg("base64");
            let bytes = match base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()) {
                Ok(b) => b,
                Err(e) => {
                    let _ = write_line(&mut write_half, &json!({ "ok": false, "error": format!("invalid base64: {e}") })).await;
                    return;
                }
            };
            let resp = match resolve_host(&state, &host_name) {
                Ok(host) => match ensure_helper(&state, &host).await {
                    Ok(()) => {
                        let flag = if unique { " --unique" } else { "" };
                        let cmd = format!("{} fs write {}{flag}", helper_file(crate::link::ID), shell_word(&path));
                        match host.pool.put(&host.target, host.askpass.clone(), &cmd, &bytes).await {
                            Ok(ran) => match parse_helper_json(&ran.output, &host.label) {
                                Ok(mut v) => {
                                    v["host"] = json!(host.label);
                                    v
                                }
                                Err(e) => json!({ "ok": false, "error": e }),
                            },
                            Err(e) => json!({ "ok": false, "error": e.to_string() }),
                        }
                    }
                    Err(e) => json!({ "ok": false, "error": e }),
                },
                Err(e) => json!({ "ok": false, "error": e }),
            };
            let _ = write_line(&mut write_half, &resp).await;
        }
        "read_to_cache" => {
            let host_name = str_arg("host");
            let path = str_arg("path");
            let max_bytes = req.get("max_bytes").and_then(Value::as_u64);
            let resp = handle_read_to_cache(&state, &host_name, &path, max_bytes).await;
            let _ = write_line(&mut write_half, &resp).await;
        }
        "download" => {
            let host_name = str_arg("host");
            let remote_path = str_arg("remote_path");
            let local_dest_dir = PathBuf::from(str_arg("local_dest_dir"));
            let local_name = str_arg("local_name");
            let is_dir = bool_arg("is_dir");
            handle_download(&state, &host_name, &remote_path, local_dest_dir, local_name, is_dir, reader, &mut write_half).await;
        }
        "upload" => {
            let host_name = str_arg("host");
            let local_path = PathBuf::from(str_arg("local_path"));
            let remote_dest_dir = str_arg("remote_dest_dir");
            let remote_name = str_arg("remote_name");
            let is_dir = bool_arg("is_dir");
            handle_upload(&state, &host_name, local_path, remote_dest_dir, remote_name, is_dir, reader, &mut write_half).await;
        }
        other => {
            let _ = write_line(&mut write_half, &json!({ "ok": false, "error": format!("unknown op '{other}'") })).await;
        }
    }
}

async fn handle_read_to_cache(state: &Arc<Mutex<HubState>>, host_name: &str, path: &str, max_bytes: Option<u64>) -> Value {
    let host = match resolve_host(state, host_name) {
        Ok(h) => h,
        Err(e) => return json!({ "ok": false, "error": e }),
    };
    let stat = match run_fs_json(state, &host, &format!("stat {}", shell_word(path))).await {
        Ok(v) => v,
        Err(e) => return json!({ "ok": false, "error": e }),
    };
    let resolved = stat.get("resolved").and_then(Value::as_str).unwrap_or(path).to_owned();
    let Some(entry) = stat.get("entry").filter(|e| !e.is_null()) else {
        return json!({ "ok": false, "error": format!("“{path}” doesn’t exist on {}.", host.label) });
    };
    if entry.get("is_dir").and_then(Value::as_bool).unwrap_or(false) {
        return json!({ "ok": false, "error": format!("“{path}” is a folder.") });
    }
    let size = entry.get("size").and_then(Value::as_u64).unwrap_or(0);
    let mtime = entry.get("mtime_secs").and_then(Value::as_i64).unwrap_or(0);
    let mtime_ns = entry.get("mtime_nanos").and_then(Value::as_u64).unwrap_or(0);
    let stamp = format!("{size}:{mtime}:{mtime_ns}:{}", max_bytes.unwrap_or(0));

    let local = cache_file_for(&host.label, &resolved);
    let stamp_path = local.with_extension("olo-stamp");
    if local.is_file() && std::fs::read_to_string(&stamp_path).ok().as_deref() == Some(stamp.as_str()) {
        return json!({ "ok": true, "local": local.to_string_lossy(), "size": size, "cached": true });
    }
    if let Some(parent) = local.parent()
        && let Err(e) = tokio::fs::create_dir_all(parent).await
    {
        return json!({ "ok": false, "error": e.to_string() });
    }
    let mut cmd = format!("{} fs read {}", helper_file(crate::link::ID), shell_word(&resolved));
    if let Some(cap) = max_bytes {
        cmd.push_str(&format!(" --max {cap}"));
    }
    let mut stream = match host.pool.exec_io(&host.target, host.askpass.clone(), &cmd).await {
        Ok(s) => s,
        Err(e) => return json!({ "ok": false, "error": e.to_string() }),
    };
    let part = local.with_extension("olo-part");
    let mut file = match tokio::fs::File::create(&part).await {
        Ok(f) => f,
        Err(e) => return json!({ "ok": false, "error": e.to_string() }),
    };
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        match stream.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                if let Err(e) = file.write_all(&buf[..n]).await {
                    let _ = tokio::fs::remove_file(&part).await;
                    return json!({ "ok": false, "error": e.to_string() });
                }
                total += n as u64;
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&part).await;
                return json!({ "ok": false, "error": e.to_string() });
            }
        }
    }
    drop(file);
    if let Err(e) = tokio::fs::rename(&part, &local).await {
        let _ = tokio::fs::remove_file(&part).await;
        return json!({ "ok": false, "error": e.to_string() });
    }
    let _ = tokio::fs::write(&stamp_path, stamp.as_bytes()).await;
    json!({ "ok": true, "local": local.to_string_lossy(), "size": total, "cached": false })
}

fn watch_cancel(mut reader: BufReader<tokio::net::unix::OwnedReadHalf>, flag: Arc<AtomicBool>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf = String::new();
        loop {
            buf.clear();
            match reader.read_line(&mut buf).await {
                Ok(0) | Err(_) => {
                    // Client disconnected: cancel the in-flight transfer.
                    flag.store(true, Ordering::Relaxed);
                    break;
                }
                Ok(_) => {
                    if buf.contains("\"cancel\"") {
                        flag.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
        }
    })
}

#[allow(clippy::too_many_arguments)]
async fn handle_download(
    state: &Arc<Mutex<HubState>>,
    host_name: &str,
    remote_path: &str,
    local_dest_dir: PathBuf,
    local_name: String,
    is_dir: bool,
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    write_half: &mut tokio::net::unix::OwnedWriteHalf,
) {
    let host = match resolve_host(state, host_name) {
        Ok(h) => h,
        Err(e) => {
            let _ = write_line(write_half, &json!({ "ok": false, "error": e })).await;
            return;
        }
    };
    if let Err(e) = ensure_helper(state, &host).await {
        let _ = write_line(write_half, &json!({ "ok": false, "error": e })).await;
        return;
    }

    let stop = Stop::default();
    let cancel_task = watch_cancel(reader, stop.stop.clone());
    let rx_counter = stop.received.clone();
    let target_local = local_dest_dir.join(&local_name);
    let target_local_ret = target_local.clone();
    let pool = host.pool.clone();
    let ssh_target = host.target.clone();
    let askpass = host.askpass.clone();
    let remote_path_owned = remote_path.to_owned();
    let stop_work = stop.clone();

    let mut work = tokio::spawn(async move {
        if is_dir {
            tokio::fs::create_dir_all(&target_local).await.map_err(|e| e.to_string())?;
            let folder = HostFolder { path: remote_path_owned, ..Default::default() };
            let report = receive_from(&pool, &ssh_target, askpass, crate::link::ID, &folder, target_local.clone(), None, stop_work).await.map_err(|e| e.to_string())?;
            if let Some(err) = report.errors.first() {
                return Err(format!("{}: {}", err.path, err.reason));
            }
            Ok::<_, String>((target_local, report.bytes_received))
        } else {
            let Some((remote_parent, remote_leaf)) = parent_and_leaf(&remote_path_owned) else {
                return Err(format!("invalid remote path: {remote_path_owned}"));
            };
            tokio::fs::create_dir_all(&local_dest_dir).await.map_err(|e| e.to_string())?;
            if remote_leaf == local_name && !target_local.exists() {
                let report = receive_paths(&pool, &ssh_target, askpass, crate::link::ID, &remote_parent, local_dest_dir, &[remote_leaf], stop_work).await.map_err(|e| e.to_string())?;
                if let Some(err) = report.errors.first() {
                    return Err(format!("{}: {}", err.path, err.reason));
                }
                Ok((target_local, report.bytes_received))
            } else {
                // Rename collision: stage in a sibling temp dir, then move atomically into place.
                let stage = local_dest_dir.join(format!(".olo-rx-{}", uuid::Uuid::new_v4().simple()));
                tokio::fs::create_dir_all(&stage).await.map_err(|e| e.to_string())?;
                let res = receive_paths(&pool, &ssh_target, askpass, crate::link::ID, &remote_parent, stage.clone(), &[remote_leaf.clone()], stop_work).await;
                let report = match res {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = tokio::fs::remove_dir_all(&stage).await;
                        return Err(e.to_string());
                    }
                };
                if let Some(err) = report.errors.first() {
                    let _ = tokio::fs::remove_dir_all(&stage).await;
                    return Err(format!("{}: {}", err.path, err.reason));
                }
                let staged = stage.join(&remote_leaf);
                if let Err(e) = tokio::fs::rename(&staged, &target_local).await {
                    let _ = tokio::fs::remove_dir_all(&stage).await;
                    return Err(e.to_string());
                }
                let _ = tokio::fs::remove_dir_all(&stage).await;
                Ok((target_local, report.bytes_received))
            }
        }
    });

    let mut tick = tokio::time::interval(Duration::from_millis(120));
    let mut last_bytes = 0u64;
    let final_res = loop {
        tokio::select! {
            res = &mut work => break res,
            _ = tick.tick() => {
                let cur = rx_counter.load(Ordering::Relaxed);
                if cur != last_bytes {
                    last_bytes = cur;
                    if write_line(write_half, &json!({ "event": "progress", "done_bytes": cur })).await.is_err() {
                        stop.stop.store(true, Ordering::Relaxed);
                    }
                }
            }
        }
    };
    cancel_task.abort();

    let resp = if stop.stop.load(Ordering::Relaxed) {
        let _ = remove_partial_local(&target_local_ret);
        json!({ "ok": false, "cancelled": true, "error": "cancelled" })
    } else {
        match final_res {
            Ok(Ok((path, bytes))) => json!({ "ok": true, "path": path.to_string_lossy(), "bytes": bytes }),
            Ok(Err(e)) => json!({ "ok": false, "error": e }),
            Err(e) => json!({ "ok": false, "error": e.to_string() }),
        }
    };
    let _ = write_line(write_half, &resp).await;
}

fn remove_partial_local(p: &Path) -> std::io::Result<()> {
    if let Ok(md) = std::fs::symlink_metadata(p) {
        if md.is_dir() {
            std::fs::remove_dir_all(p)?;
        } else {
            std::fs::remove_file(p)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_upload(
    state: &Arc<Mutex<HubState>>,
    host_name: &str,
    local_path: PathBuf,
    remote_dest_dir: String,
    remote_name: String,
    is_dir: bool,
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    write_half: &mut tokio::net::unix::OwnedWriteHalf,
) {
    let host = match resolve_host(state, host_name) {
        Ok(h) => h,
        Err(e) => {
            let _ = write_line(write_half, &json!({ "ok": false, "error": e })).await;
            return;
        }
    };
    if let Err(e) = ensure_helper(state, &host).await {
        let _ = write_line(write_half, &json!({ "ok": false, "error": e })).await;
        return;
    }

    let remote_target = if remote_dest_dir == "/" { format!("/{remote_name}") } else { format!("{}/{remote_name}", remote_dest_dir.trim_end_matches('/')) };

    let stop = Stop::default();
    let cancel_task = watch_cancel(reader, stop.stop.clone());
    let tx_counter = stop.sent.clone();
    let pool = host.pool.clone();
    let ssh_target = host.target.clone();
    let askpass = host.askpass.clone();
    let host_label = host.label.clone();
    let stop_work = stop.clone();

    let mut work = tokio::spawn(async move {
        if is_dir {
            // Pick a unique remote directory path if target already exists.
            let mk_cmd = format!("{} fs mkdir {} --unique", helper_file(crate::link::ID), shell_word(&remote_target));
            let ran = pool.exec(&ssh_target, askpass.clone(), &mk_cmd, &mut |_| {}).await.map_err(|e| e.to_string())?;
            let mk_val = parse_helper_json(&ran.output, &host_label)?;
            let final_remote = mk_val.get("path").and_then(Value::as_str).unwrap_or(&remote_target).to_owned();
            let folder = HostFolder { path: final_remote.clone(), ..Default::default() };
            let source = Box::new(FolderSource { root: local_path, filter: Default::default(), cache_path: None });
            let report = send_to(&pool, &ssh_target, askpass, crate::link::ID, source, &folder, stop_work).await.map_err(|e| e.to_string())?;
            if let Some(err) = report.errors.first() {
                return Err(format!("{}: {}", err.path, err.reason));
            }
            Ok::<_, String>((final_remote, report.bytes_sent))
        } else {
            let local_parent = local_path.parent().ok_or_else(|| "invalid local path".to_owned())?.to_path_buf();
            let local_leaf = local_path.file_name().ok_or_else(|| "invalid local file name".to_owned())?.to_string_lossy().into_owned();
            // Check whether remote_target already exists so we never clobber an existing file.
            let stat_cmd = format!("{} fs stat {}", helper_file(crate::link::ID), shell_word(&remote_target));
            let stat_ran = pool.exec(&ssh_target, askpass.clone(), &stat_cmd, &mut |_| {}).await.map_err(|e| e.to_string())?;
            let stat_val = parse_helper_json(&stat_ran.output, &host_label)?;
            let exists = stat_val.get("exists").and_then(Value::as_bool).unwrap_or(false);
            let resolved_target = stat_val.get("resolved").and_then(Value::as_str).unwrap_or(&remote_target).to_owned();

            if !exists && local_leaf == remote_name {
                let report = send_paths(&pool, &ssh_target, askpass, crate::link::ID, &remote_dest_dir, local_parent, vec![local_leaf], stop_work).await.map_err(|e| e.to_string())?;
                if let Some(err) = report.errors.first() {
                    return Err(format!("{}: {}", err.path, err.reason));
                }
                Ok((resolved_target, report.bytes_sent))
            } else {
                // Stage in a temporary sibling dir on the host, then rename with --unique.
                let stage_remote = format!("{}/.olo-tx-{}", remote_dest_dir.trim_end_matches('/'), uuid::Uuid::new_v4().simple());
                let mk_cmd = format!("{} fs mkdir {}", helper_file(crate::link::ID), shell_word(&stage_remote));
                let _ = pool.exec(&ssh_target, askpass.clone(), &mk_cmd, &mut |_| {}).await.map_err(|e| e.to_string())?;
                let res = send_paths(&pool, &ssh_target, askpass.clone(), crate::link::ID, &stage_remote, local_parent, vec![local_leaf.clone()], stop_work).await;
                let report = match res {
                    Ok(r) => r,
                    Err(e) => {
                        let rm_cmd = format!("{} fs remove {}", helper_file(crate::link::ID), shell_word(&stage_remote));
                        let _ = pool.exec(&ssh_target, askpass, &rm_cmd, &mut |_| {}).await;
                        return Err(e.to_string());
                    }
                };
                if let Some(err) = report.errors.first() {
                    let rm_cmd = format!("{} fs remove {}", helper_file(crate::link::ID), shell_word(&stage_remote));
                    let _ = pool.exec(&ssh_target, askpass, &rm_cmd, &mut |_| {}).await;
                    return Err(format!("{}: {}", err.path, err.reason));
                }
                let staged_file = format!("{stage_remote}/{local_leaf}");
                let ren_cmd = format!("{} fs rename {} {} --unique", helper_file(crate::link::ID), shell_word(&staged_file), shell_word(&remote_target));
                let ren_ran = pool.exec(&ssh_target, askpass.clone(), &ren_cmd, &mut |_| {}).await.map_err(|e| e.to_string())?;
                let rm_cmd = format!("{} fs remove {}", helper_file(crate::link::ID), shell_word(&stage_remote));
                let _ = pool.exec(&ssh_target, askpass, &rm_cmd, &mut |_| {}).await;
                let ren_val = parse_helper_json(&ren_ran.output, &host_label)?;
                let final_remote = ren_val.get("path").and_then(Value::as_str).unwrap_or(&resolved_target).to_owned();
                Ok((final_remote, report.bytes_sent))
            }
        }
    });

    let mut tick = tokio::time::interval(Duration::from_millis(120));
    let mut last_bytes = 0u64;
    let final_res = loop {
        tokio::select! {
            res = &mut work => break res,
            _ = tick.tick() => {
                let cur = tx_counter.load(Ordering::Relaxed);
                if cur != last_bytes {
                    last_bytes = cur;
                    if write_line(write_half, &json!({ "event": "progress", "done_bytes": cur })).await.is_err() {
                        stop.stop.store(true, Ordering::Relaxed);
                    }
                }
            }
        }
    };
    cancel_task.abort();

    let resp = if stop.stop.load(Ordering::Relaxed) {
        json!({ "ok": false, "cancelled": true, "error": "cancelled" })
    } else {
        match final_res {
            Ok(Ok((path, bytes))) => json!({ "ok": true, "path": path, "host": host.label, "bytes": bytes }),
            Ok(Err(e)) => json!({ "ok": false, "error": e }),
            Err(e) => json!({ "ok": false, "error": e.to_string() }),
        }
    };
    let _ = write_line(write_half, &resp).await;
}

/// Start listening on the Rex files Unix socket for Tracker (`rex://HOST/PATH`).
pub fn spawn(rt: &tokio::runtime::Handle, state: Arc<Mutex<HubState>>) {
    let sock = olo_kit_base::tracker::rex_files_socket();
    rt.spawn(async move {
        if let Some(parent) = sock.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::remove_file(&sock);
        let listener = match UnixListener::bind(&sock) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("Rex files socket {} bind failed: {e}", sock.display());
                return;
            }
        };
        let _ = std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600));
        tracing::info!("Rex files socket listening at {}", sock.display());
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let st = state.clone();
                    tokio::spawn(handle_client(stream, st));
                }
                Err(e) => {
                    tracing::warn!("Rex files socket accept error: {e}");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    });
}
