//! Client for Rex's local files Unix socket (`rex://HOST/PATH`).
//!
//! All calls are synchronous blocking I/O meant to run on Tracker's worker
//! threads (never on the UI looper). When `OLO_TRACKER_DEMO_REX=1` is set
//! (for headless visual QA and `ui-lint`), returns a deterministic fixture
//! host (`dev-box`) without requiring a live Rex instance.

use std::{
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use olo_kit_base::tracker::RexPath;
use serde_json::{Value, json};

use crate::{
    fs::{Entry, Shown, kind_for},
    ops::Progress,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostState {
    Connected,
    Connecting,
    Reconnecting,
    Disconnected,
    Error(String),
    RexNotRunning,
    Unknown,
}

impl HostState {
    pub fn from_wire(state: &str, err: Option<String>) -> Self {
        match state {
            "connected" => HostState::Connected,
            "connecting" => HostState::Connecting,
            "reconnecting" => HostState::Reconnecting,
            "disconnected" => HostState::Disconnected,
            "error" => HostState::Error(err.unwrap_or_else(|| "connection error".into())),
            _ => HostState::Unknown,
        }
    }

    pub fn is_connected(&self) -> bool {
        *self == HostState::Connected
    }
}

#[derive(Debug, Clone)]
pub struct HostInfo {
    pub label: String,
    pub state: HostState,
}

#[derive(Debug, Clone)]
pub struct ListResult {
    pub host: String,
    pub resolved: String,
    pub free_bytes: Option<u64>,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone)]
pub struct StatResult {
    pub host: String,
    pub resolved: String,
    pub exists: bool,
    pub entry: Option<Entry>,
}

fn demo_mode() -> bool {
    std::env::var("OLO_TRACKER_DEMO_REX").ok().as_deref() == Some("1")
}

fn demo_entry(host: &str, dir: &str, name: &str, is_dir: bool, size: u64, age_secs: u64, mode: u32) -> Entry {
    let rp = RexPath::new(host, dir).join(name);
    let path = rp.to_path_buf();
    let kind = kind_for(Path::new(name), is_dir, mode);
    let modified = SystemTime::now().checked_sub(Duration::from_secs(age_secs));
    Entry { path, name: name.to_owned(), is_dir, is_link: false, size, modified, mode, kind, app_icon: None, downloading: None, shown: Shown::default() }
}

fn demo_list(rex: &RexPath, hidden: bool) -> Result<ListResult, String> {
    if rex.host.eq_ignore_ascii_case("build-gpu") {
        return Err("“build-gpu” isn’t connected in Rex.".into());
    }
    let resolved = if rex.path == "~" || rex.path == "/home/dev" {
        "/home/dev".to_owned()
    } else if let Some(rest) = rex.path.strip_prefix("~/") {
        format!("/home/dev/{rest}")
    } else {
        rex.path.clone()
    };
    let host = if rex.host.eq_ignore_ascii_case("dev-box") { "dev-box".to_owned() } else { rex.host.clone() };
    let mut entries = match resolved.as_str() {
        "/home/dev/src" => vec![
            demo_entry(&host, &resolved, "olo", true, 0, 120, 0o755),
            demo_entry(&host, &resolved, "jato", true, 0, 3600, 0o755),
            demo_entry(&host, &resolved, "Cargo.toml", false, 4_120, 600, 0o644),
            demo_entry(&host, &resolved, "README.md", false, 14_800, 7200, 0o644),
            demo_entry(&host, &resolved, "deploy.sh", false, 1_940, 1800, 0o755),
        ],
        _ => vec![
            demo_entry(&host, &resolved, "src", true, 0, 180, 0o755),
            demo_entry(&host, &resolved, "docs", true, 0, 2400, 0o755),
            demo_entry(&host, &resolved, "notes", true, 0, 900, 0o755),
            demo_entry(&host, &resolved, "README.md", false, 12_480, 420, 0o644),
            demo_entry(&host, &resolved, "Cargo.toml", false, 3_920, 1400, 0o644),
            demo_entry(&host, &resolved, "benchmark.csv", false, 98_432, 3100, 0o644),
            demo_entry(&host, &resolved, "architecture.png", false, 418_200, 5400, 0o644),
            demo_entry(&host, &resolved, "run-tests.sh", false, 2_150, 8600, 0o755),
        ],
    };
    if hidden {
        entries.push(demo_entry(&host, &resolved, ".bashrc", false, 1_120, 86_400, 0o644));
    }
    Ok(ListResult { host, resolved, free_bytes: Some(142 * 1024 * 1024 * 1024), entries })
}

fn connect() -> Result<UnixStream, String> {
    let sock = olo_kit_base::tracker::rex_files_socket();
    let stream = UnixStream::connect(&sock).map_err(|_| "Rex isn’t running. Open Rex to browse files on remote hosts.".to_owned())?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(15)));
    Ok(stream)
}

fn call(req: Value) -> Result<Value, String> {
    let mut stream = connect()?;
    let mut line = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
    line.push(b'\n');
    stream.write_all(&line).map_err(|e| format!("Couldn’t reach Rex: {e}"))?;
    stream.flush().map_err(|e| format!("Couldn’t reach Rex: {e}"))?;
    let mut reader = BufReader::new(stream);
    let mut resp_line = String::new();
    let n = reader.read_line(&mut resp_line).map_err(|e| format!("Rex didn’t answer: {e}"))?;
    if n == 0 {
        return Err("Rex closed the connection.".into());
    }
    let val: Value = serde_json::from_str(resp_line.trim()).map_err(|e| format!("Unexpected response from Rex: {e}"))?;
    if val.get("ok").and_then(Value::as_bool) == Some(true) { Ok(val) } else { Err(val.get("error").and_then(Value::as_str).unwrap_or("Remote operation failed.").to_owned()) }
}

fn entry_from_json(host: &str, parent_dir: &str, v: &Value) -> Option<Entry> {
    let name = v.get("name")?.as_str()?.to_owned();
    let is_dir = v.get("is_dir").and_then(Value::as_bool).unwrap_or(false);
    let is_link = v.get("is_link").and_then(Value::as_bool).unwrap_or(false);
    let size = v.get("size").and_then(Value::as_u64).unwrap_or(0);
    let mode = v.get("mode").and_then(Value::as_u64).unwrap_or(if is_dir { 0o755 } else { 0o644 }) as u32;
    let mtime_secs = v.get("mtime_secs").and_then(Value::as_i64);
    let mtime_nanos = v.get("mtime_nanos").and_then(Value::as_u64).unwrap_or(0) as u32;
    let modified = mtime_secs.and_then(|s| if s >= 0 { UNIX_EPOCH.checked_add(Duration::new(s as u64, mtime_nanos)) } else { UNIX_EPOCH.checked_sub(Duration::from_secs(s.unsigned_abs())) });
    let rp = if name == "/" { RexPath::new(host, "/") } else { RexPath::new(host, parent_dir).join(&name) };
    let kind = kind_for(Path::new(&name), is_dir, mode);
    Some(Entry { path: rp.to_path_buf(), name, is_dir, is_link, size: if is_dir { 0 } else { size }, modified, mode, kind, app_icon: None, downloading: None, shown: Shown::default() })
}

fn entry_at_path_from_json(host: &str, full_path: &str, v: &Value) -> Option<Entry> {
    let name = v.get("name")?.as_str()?.to_owned();
    let is_dir = v.get("is_dir").and_then(Value::as_bool).unwrap_or(false);
    let is_link = v.get("is_link").and_then(Value::as_bool).unwrap_or(false);
    let size = v.get("size").and_then(Value::as_u64).unwrap_or(0);
    let mode = v.get("mode").and_then(Value::as_u64).unwrap_or(if is_dir { 0o755 } else { 0o644 }) as u32;
    let mtime_secs = v.get("mtime_secs").and_then(Value::as_i64);
    let mtime_nanos = v.get("mtime_nanos").and_then(Value::as_u64).unwrap_or(0) as u32;
    let modified = mtime_secs.and_then(|s| if s >= 0 { UNIX_EPOCH.checked_add(Duration::new(s as u64, mtime_nanos)) } else { UNIX_EPOCH.checked_sub(Duration::from_secs(s.unsigned_abs())) });
    let rp = RexPath::new(host, full_path);
    let kind = kind_for(Path::new(&name), is_dir, mode);
    Some(Entry { path: rp.to_path_buf(), name, is_dir, is_link, size: if is_dir { 0 } else { size }, modified, mode, kind, app_icon: None, downloading: None, shown: Shown::default() })
}

pub fn hosts() -> Result<Vec<HostInfo>, String> {
    if demo_mode() {
        return Ok(vec![HostInfo { label: "dev-box".into(), state: HostState::Connected }, HostInfo { label: "build-gpu".into(), state: HostState::Disconnected }]);
    }
    let val = call(json!({ "op": "hosts" }))?;
    let list = val
        .get("hosts")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|h| {
                    let label = h.get("label")?.as_str()?.to_owned();
                    let st = h.get("state").and_then(Value::as_str).unwrap_or("disconnected");
                    let err = h.get("error").and_then(Value::as_str).map(str::to_owned);
                    Some(HostInfo { label, state: HostState::from_wire(st, err) })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(list)
}

pub fn host_state(host: &str) -> HostState {
    if demo_mode() {
        return if host.eq_ignore_ascii_case("build-gpu") { HostState::Disconnected } else { HostState::Connected };
    }
    match call(json!({ "op": "host_state", "host": host })) {
        Ok(val) => {
            if val.get("found").and_then(Value::as_bool) != Some(true) {
                return HostState::Unknown;
            }
            let st = val.get("state").and_then(Value::as_str).unwrap_or("disconnected");
            let err = val.get("error").and_then(Value::as_str).map(str::to_owned);
            HostState::from_wire(st, err)
        }
        Err(e) if e.contains("isn’t running") => HostState::RexNotRunning,
        Err(e) => HostState::Error(e),
    }
}

pub fn list(rex: &RexPath, hidden: bool) -> Result<ListResult, String> {
    if demo_mode() {
        return demo_list(rex, hidden);
    }
    let val = call(json!({ "op": "list", "host": rex.host, "path": rex.path, "hidden": hidden }))?;
    let host = val.get("host").and_then(Value::as_str).unwrap_or(&rex.host).to_owned();
    let resolved = val.get("resolved").and_then(Value::as_str).unwrap_or(&rex.path).to_owned();
    let free_bytes = val.get("free_bytes").and_then(Value::as_u64);
    let entries = val.get("entries").and_then(Value::as_array).map(|arr| arr.iter().filter_map(|e| entry_from_json(&host, &resolved, e)).collect()).unwrap_or_default();
    Ok(ListResult { host, resolved, free_bytes, entries })
}

pub fn stat(rex: &RexPath) -> Result<StatResult, String> {
    if demo_mode() {
        let resolved = if rex.path == "~" { "/home/dev".into() } else { rex.path.clone() };
        let is_dir = !resolved.contains('.');
        let name = resolved.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("/");
        let entry = Entry {
            path: RexPath::new(&rex.host, &resolved).to_path_buf(),
            name: name.to_owned(),
            is_dir,
            is_link: false,
            size: if is_dir { 0 } else { 4096 },
            modified: Some(SystemTime::now()),
            mode: if is_dir { 0o755 } else { 0o644 },
            kind: kind_for(Path::new(name), is_dir, if is_dir { 0o755 } else { 0o644 }),
            app_icon: None,
            downloading: None,
            shown: Shown::default(),
        };
        return Ok(StatResult { host: rex.host.clone(), resolved, exists: true, entry: Some(entry) });
    }
    let val = call(json!({ "op": "stat", "host": rex.host, "path": rex.path }))?;
    let host = val.get("host").and_then(Value::as_str).unwrap_or(&rex.host).to_owned();
    let resolved = val.get("resolved").and_then(Value::as_str).unwrap_or(&rex.path).to_owned();
    let exists = val.get("exists").and_then(Value::as_bool).unwrap_or(false);
    let entry = val.get("entry").filter(|v| !v.is_null()).and_then(|v| entry_at_path_from_json(&host, &resolved, v));
    Ok(StatResult { host, resolved, exists, entry })
}

pub fn mkdir(rex: &RexPath, unique: bool) -> Result<(RexPath, Option<Entry>), String> {
    let val = call(json!({ "op": "mkdir", "host": rex.host, "path": rex.path, "unique": unique }))?;
    let host = val.get("host").and_then(Value::as_str).unwrap_or(&rex.host).to_owned();
    let path = val.get("path").and_then(Value::as_str).unwrap_or(&rex.path).to_owned();
    let entry = val.get("entry").filter(|v| !v.is_null()).and_then(|v| entry_at_path_from_json(&host, &path, v));
    Ok((RexPath::new(host, path), entry))
}

pub fn rename(from: &RexPath, to: &RexPath, unique: bool) -> Result<RexPath, String> {
    if !from.host.eq_ignore_ascii_case(&to.host) {
        return Err("Can’t rename across different hosts.".into());
    }
    let val = call(json!({ "op": "rename", "host": from.host, "from": from.path, "to": to.path, "unique": unique }))?;
    let host = val.get("host").and_then(Value::as_str).unwrap_or(&from.host).to_owned();
    let path = val.get("path").and_then(Value::as_str).unwrap_or(&to.path).to_owned();
    Ok(RexPath::new(host, path))
}

pub fn copy_remote(from: &RexPath, to: &RexPath) -> Result<RexPath, String> {
    if !from.host.eq_ignore_ascii_case(&to.host) {
        return Err("Can’t copy directly across different hosts.".into());
    }
    let val = call(json!({ "op": "copy_remote", "host": from.host, "from": from.path, "to": to.path }))?;
    let host = val.get("host").and_then(Value::as_str).unwrap_or(&from.host).to_owned();
    let path = val.get("path").and_then(Value::as_str).unwrap_or(&to.path).to_owned();
    Ok(RexPath::new(host, path))
}

pub fn remove(rex: &RexPath) -> Result<(), String> {
    let _ = call(json!({ "op": "remove", "host": rex.host, "path": rex.path }))?;
    Ok(())
}

pub fn read_to_cache(rex: &RexPath) -> Result<PathBuf, String> {
    let val = call(json!({ "op": "read_to_cache", "host": rex.host, "path": rex.path }))?;
    let local = val.get("local").and_then(Value::as_str).ok_or_else(|| "Rex didn’t return a cached file path.".to_owned())?;
    Ok(PathBuf::from(local))
}

pub fn write_file(rex: &RexPath, unique: bool, bytes: &[u8]) -> Result<RexPath, String> {
    let val = call(json!({
        "op": "write_file",
        "host": rex.host,
        "path": rex.path,
        "unique": unique,
        "base64": encode_base64(bytes),
    }))?;
    let host = val.get("host").and_then(Value::as_str).unwrap_or(&rex.host).to_owned();
    let path = val.get("path").and_then(Value::as_str).unwrap_or(&rex.path).to_owned();
    Ok(RexPath::new(host, path))
}

/// Download a file or folder from a Rex host to `local_dest_dir/local_name` via
/// Rex's `olo-rex-sync` transfer, streaming byte progress and honoring `cancel`.
pub fn download(rex: &RexPath, local_dest_dir: &Path, local_name: &str, is_dir: bool, progress: &Progress, cancel: &AtomicBool) -> io::Result<PathBuf> {
    let req = json!({
        "op": "download",
        "host": rex.host,
        "remote_path": rex.path,
        "local_dest_dir": local_dest_dir.to_string_lossy(),
        "local_name": local_name,
        "is_dir": is_dir,
    });
    let val = stream_transfer(req, progress, cancel)?;
    let path = val.get("path").and_then(Value::as_str).ok_or_else(|| io::Error::other("missing downloaded path"))?;
    Ok(PathBuf::from(path))
}

/// Upload a local file or folder to `remote_dest_dir/remote_name` on a Rex host
/// via Rex's `olo-rex-sync` transfer, streaming byte progress and honoring `cancel`.
pub fn upload(local_path: &Path, remote_dest_dir: &RexPath, remote_name: &str, is_dir: bool, progress: &Progress, cancel: &AtomicBool) -> io::Result<RexPath> {
    let req = json!({
        "op": "upload",
        "host": remote_dest_dir.host,
        "local_path": local_path.to_string_lossy(),
        "remote_dest_dir": remote_dest_dir.path,
        "remote_name": remote_name,
        "is_dir": is_dir,
    });
    let val = stream_transfer(req, progress, cancel)?;
    let host = val.get("host").and_then(Value::as_str).unwrap_or(&remote_dest_dir.host).to_owned();
    let path = val.get("path").and_then(Value::as_str).ok_or_else(|| io::Error::other("missing uploaded path"))?;
    Ok(RexPath::new(host, path))
}

fn stream_transfer(req: Value, progress: &Progress, cancel: &AtomicBool) -> io::Result<Value> {
    let sock = olo_kit_base::tracker::rex_files_socket();
    let mut stream = UnixStream::connect(&sock).map_err(|_| io::Error::other("Rex isn’t running. Open Rex to transfer files with remote hosts."))?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(15)));

    let mut line = serde_json::to_vec(&req).map_err(io::Error::other)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    stream.flush()?;

    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    let mut sent_cancel = false;
    let base_done = progress.done_bytes.load(Ordering::Relaxed);

    loop {
        if cancel.load(Ordering::Relaxed) && !sent_cancel {
            sent_cancel = true;
            let _ = writer.write_all(b"{\"op\":\"cancel\"}\n");
            let _ = writer.flush();
        }
        buf.clear();
        match reader.read_line(&mut buf) {
            Ok(0) => {
                return if cancel.load(Ordering::Relaxed) { Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled")) } else { Err(io::Error::other("Rex closed the connection during transfer.")) };
            }
            Ok(_) => {
                let val: Value = serde_json::from_str(buf.trim()).map_err(|e| io::Error::other(format!("Unexpected response from Rex: {e}")))?;
                if val.get("event").and_then(Value::as_str) == Some("progress") {
                    if let Some(done) = val.get("done_bytes").and_then(Value::as_u64) {
                        progress.done_bytes.store(base_done.saturating_add(done), Ordering::Relaxed);
                        progress.counting.store(false, Ordering::Relaxed);
                    }
                    continue;
                }
                if val.get("ok").and_then(Value::as_bool) == Some(true) {
                    return Ok(val);
                }
                if val.get("cancelled").and_then(Value::as_bool) == Some(true) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                }
                let err = val.get("error").and_then(Value::as_str).unwrap_or("Remote transfer failed.");
                return Err(io::Error::other(err.to_owned()));
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => continue,
            Err(e) => return Err(e),
        }
    }
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_encodes_rfc4648_vectors() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
    }
}
