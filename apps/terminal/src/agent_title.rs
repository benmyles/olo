//! Coding agents keep their tab's title current (Preferences ▸ Terminal
//! ▸ "Let coding agents keep their tab’s title current", off by
//! default): a Claude Code or Codex in a tab has a tool, `set_tab_title`,
//! and is told (agent_context.rs's hook) to keep the title saying what
//! it's working on.
//!
//! The tool is an MCP server the session serves over loopback HTTP
//! (`title_service.rs`, `olo-terminal --mcp-service`, one of the real
//! session's services): on, Terminal names its URL in Claude Code's
//! `.claude.json` (`mcpServers`, with an allow rule for the tool in its
//! `settings.json`) and in Codex's `config.toml` (`mcp_servers`); off,
//! it takes it out again, leaving the rest of each file as it was. Over
//! HTTP, an agent reconnects by itself whenever the server was away
//! (Claude Code never starts a stdio server again once it has gone: its
//! tool was lost for the rest of the session, 2026-11-06), and the
//! service upgrades without a gap. It finds the tab from the process
//! that asked (Claude Code is under the tab's shell; Codex's daemon by
//! the folder the agent gives) and hands the title to that tab's
//! Terminal (`--set-tab-title SHELL TITLE`, through its single-instance
//! socket on the tab's display). The title shows in the tab as a
//! program's own title would, ahead of it, until the agent leaves the
//! tab or gives an empty one. `olo-terminal --mcp` serves the same over
//! stdin and stdout, for an agent whose settings still name it.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use toml_edit::{DocumentMut, Item};

use crate::agent_context::{self, TabInfo};

/// What the server is run with.
pub const MCP_FLAG: &str = "--mcp";
/// What Terminal is handed (`--set-tab-title SHELL TITLE`).
pub const SET_FLAG: &str = "--set-tab-title";
/// What an agent runs in its shell to set its tab's title without the
/// tool (`olo-terminal --tab-title WORDS…`): `from_shell`.
pub const TITLE_FLAG: &str = "--tab-title";
/// The server's name in the agents' settings, and so the tool's prefix.
const SERVER: &str = "olo-terminal";
const TOOL: &str = "set_tab_title";
/// A title is a few words: longer is cut.
pub const MOST: usize = 60;

/// Told at the start of a session (the hook).
pub const INSTRUCTIONS: &str = "This session runs in a tab of Olo's Terminal. Keep the tab's title saying what you are working on: call the set_tab_title tool (the olo-terminal MCP server) with a few words, such as \"Fix login redirect\", when you start on a task and whenever the work moves on to something else, passing your working directory as cwd. If that tool isn't available (an agent can lose its connection to the server for the rest of a session), run `olo-terminal --tab-title \"Fix login redirect\"` in your shell instead: it does the same, and never opens a window.";
/// Told with each prompt.
pub const REMINDER: &str =
    "(Olo Terminal: if this changes what you are working on, update the tab's title with set_tab_title, or by running `olo-terminal --tab-title \"…\"` if that tool isn't available.)";

fn tool() -> Value {
    json!({
        "name": TOOL,
        "title": "Set Terminal Tab Title",
        "description": "Set the title of the Olo Terminal tab this session runs in: a few words saying what you are working on now (\"Fix login redirect\", \"Review PR 412\"). Keep it current: set it when you start on a task and whenever the work changes. An empty title gives the tab its own back.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "A few words, under 40 characters."},
                "cwd": {"type": "string", "description": "Your working directory (finds your tab when the agent runs outside it)."}
            },
            "required": ["title"]
        },
        "annotations": {"title": "Set Terminal Tab Title", "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
    })
}

/// The tab of the agent that is `lineage` (it and its ancestors): under
/// its shell (Claude Code), else in `cwd` (Codex: what the agent gives,
/// else `own`, the folder it's in).
fn find_tab(lineage: &[i32], cwd: Option<&Path>, own: Option<&Path>) -> Option<TabInfo> {
    let snapshot = agent_context::read_all()?;
    agent_context::tab_of(&snapshot, lineage, cwd.or(own), agent_context::own_agent(lineage)).cloned()
}

/// Set the title of the tab of the agent that is `lineage` (`cwd` the
/// folder it says it's in, `own` the one it is): what to tell it, or
/// why not.
pub fn set_for(lineage: &[i32], title: &str, cwd: Option<&Path>, own: Option<&Path>) -> Result<String, String> {
    let tab = find_tab(lineage, cwd, own).ok_or("This session isn't in an Olo Terminal tab (or Terminal's setting for it is off).")?;
    hand_over(&tab, title)?;
    Ok(if title.is_empty() { "The tab's title is its own again.".into() } else { format!("The tab is titled \u{201c}{title}\u{201d}.") })
}

/// `olo-terminal --tab-title WORDS…`: the tool's work from an agent's
/// shell, for an agent that hasn't the tool (Claude Code keeps the MCP
/// settings a session started with and never starts a stdio server again,
/// nor an HTTP one after five tries; the person shouldn't have to
/// reconnect it). The tab is found as the service finds it, from this
/// process's ancestors (the agent's shell, the agent, the tab's shell)
/// and the folder it runs in. Never opens a window: it's handled before
/// anything is handed to a running Terminal.
pub fn from_shell(words: &[String]) -> std::process::ExitCode {
    let title = tidy(&words.join(" "));
    let own = std::env::current_dir().ok();
    match set_for(&agent_context::ancestors(), &title, own.as_deref(), own.as_deref()) {
        Ok(said) => {
            println!("{said}");
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("olo-terminal: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Set `tab`'s title through its Terminal.
fn hand_over(tab: &TabInfo, title: &str) -> Result<(), String> {
    let shell = tab.shell.ok_or("the tab has no shell")?;
    let display = tab.display.as_deref().unwrap_or_default();
    let args = [SET_FLAG.to_owned(), shell.to_string(), title.to_owned()];
    if olo_kit_base::instance::forward_on(crate::APP_ID, display, &args) { Ok(()) } else { Err("Terminal isn't answering".into()) }
}

/// A title as shown: one line, trimmed, at most `MOST` characters.
pub fn tidy(title: &str) -> String {
    let one: String = title.split_whitespace().collect::<Vec<_>>().join(" ");
    match one.char_indices().nth(MOST) {
        Some((i, _)) => format!("{}…", one[..i].trim_end()),
        None => one,
    }
}

/// What sets a title (given the folder the agent says it's in): what to
/// tell the agent, or why it couldn't.
pub type Set<'a> = dyn FnMut(&str, Option<&Path>) -> Result<String, String> + 'a;

/// The answer to one request (None for a notification), `set` doing the
/// tool's work.
pub fn answer(msg: &Value, set: &mut Set) -> Option<Value> {
    let id = msg.get("id").cloned()?;
    let ok = |result: Value| json!({"jsonrpc": "2.0", "id": id.clone(), "result": result});
    let fail = |code: i32, message: &str| json!({"jsonrpc": "2.0", "id": id.clone(), "error": {"code": code, "message": message}});
    Some(match msg["method"].as_str().unwrap_or_default() {
        "initialize" => ok(json!({
            "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": SERVER, "title": "Olo Terminal", "version": env!("CARGO_PKG_VERSION")}
        })),
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({"tools": [tool()]})),
        "tools/call" if msg["params"]["name"] == TOOL => {
            let args = &msg["params"]["arguments"];
            let Some(title) = args["title"].as_str() else { return Some(fail(-32602, "title is a string")) };
            let cwd = args["cwd"].as_str().map(PathBuf::from);
            let (text, error) = match set(&tidy(title), cwd.as_deref()) {
                Ok(text) => (text, false),
                Err(e) => (e, true),
            };
            ok(json!({"content": [{"type": "text", "text": text}], "isError": error}))
        }
        "tools/call" => fail(-32602, "no such tool"),
        _ => fail(-32601, "method not found"),
    })
}

/// `olo-terminal --mcp`: the server, until its agent closes its stdin.
/// Neither agent starts a server again once it has gone (Claude Code
/// till `/mcp` reconnects it, Codex till the session ends), so it stays
/// as it is through upgrades: it doesn't become the new version
/// (olo-session's SIGURG is left ignored; olo-session leaves alone what
/// an agent started). A Codex server ended a few minutes after it had
/// re-executed for the 22:27 upgrade on 2026-11-05, nothing saying why
/// (the same in a test, with Codex's environment, carried on), and its
/// session's titles failed from then on ("Transport closed"). The
/// handover to Terminal (`--set-tab-title` through its instance socket)
/// is kept the same from version to version. Its life is noted in
/// `log_path()`: when it started, for whom, and how it ended.
pub fn serve() -> std::process::ExitCode {
    let lineage = agent_context::ancestors();
    let own = std::env::current_dir().ok();
    let mut set = |title: &str, cwd: Option<&Path>| set_for(&lineage, title, cwd, own.as_deref());
    note_life();
    let mut out = std::io::stdout().lock();
    let mut why = "";
    let code = serve_on(0, None, &mut out, &mut set, &mut || {}, &mut why);
    note(&format!("ended: {why}"));
    code
}

/// Where the servers' lives are noted (a few lines each; kept short).
pub fn log_path() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/state"));
    state.join("olo/terminal-mcp.log")
}

/// A line in the log: when, this process, `what`.
fn note(what: &str) {
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // (Kept to its last 64 KB.)
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 64 * 1024)
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        let keep = &text[text.len() - 32 * 1024..];
        let keep = keep.find('\n').map_or(keep, |i| &keep[i + 1..]);
        let _ = std::fs::write(&path, keep);
    }
    let when = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{when} pid {} {what}", std::process::id());
    }
}

/// Note the start (for which program), and the end if a signal brings
/// it (written from the handler: a fixed line, then exit).
fn note_life() {
    let ppid = std::os::unix::process::parent_id();
    let parent = std::fs::read_to_string(format!("/proc/{ppid}/comm")).unwrap_or_default();
    note(&format!("started for {} (pid {ppid}), version {}", parent.trim(), env!("CARGO_PKG_VERSION")));
    static LOG: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
    static LINE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let Ok(f) = std::fs::OpenOptions::new().append(true).open(log_path()) else { return };
    let _ = LINE.set(format!("- pid {} ended by a signal\n", std::process::id()).into_bytes());
    LOG.store(std::os::fd::IntoRawFd::into_raw_fd(f), std::sync::atomic::Ordering::Relaxed);
    extern "C" fn ended(_: libc::c_int) {
        let fd = LOG.load(std::sync::atomic::Ordering::Relaxed);
        if let Some(line) = LINE.get() {
            // SAFETY: write(2) and _exit(2) are async-signal-safe; the
            // line was made before the handler was set.
            unsafe { libc::write(fd, line.as_ptr().cast(), line.len()) };
        }
        // SAFETY: as above.
        unsafe { libc::_exit(0) };
    }
    for sig in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
        // SAFETY: a handler that only writes and exits.
        unsafe { libc::signal(sig, ended as *const () as libc::sighandler_t) };
    }
}

/// The server's loop: requests read from `input` (a line each) into a
/// buffer of its own, so none is half-read in a reader's buffer when it
/// becomes the new version (`become_new`, once `upgrade` is readable and
/// what came has been answered).
fn serve_on(input: std::os::fd::RawFd, upgrade: Option<std::os::fd::RawFd>, out: &mut dyn Write, set: &mut Set, become_new: &mut dyn FnMut(), why: &mut &'static str) -> std::process::ExitCode {
    let mut pending: Vec<u8> = Vec::new();
    let mut upgrading = false;
    let mut ended = false;
    loop {
        // Every whole line that came, answered.
        while let Some(n) = pending.iter().position(|&b| b == b'\n').or(if ended && !pending.is_empty() { Some(pending.len() - 1) } else { None }) {
            let line: Vec<u8> = pending.drain(..=n).collect();
            let line = String::from_utf8_lossy(&line);
            if line.trim().is_empty() {
                continue;
            }
            let reply = match serde_json::from_str::<Value>(line.trim()) {
                Ok(msg) => answer(&msg, set),
                Err(_) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}})),
            };
            if let Some(reply) = reply
                && (writeln!(out, "{reply}").is_err() || out.flush().is_err())
            {
                *why = "its agent stopped reading (a reply couldn't be written)";
                return std::process::ExitCode::SUCCESS;
            }
        }
        if ended {
            *why = "its agent closed it (its input ended)";
            return std::process::ExitCode::SUCCESS;
        }
        // Asked to upgrade, nothing half-read: the new version carries on.
        if upgrading && pending.is_empty() {
            upgrading = false;
            become_new();
        }
        let mut fds = [libc::pollfd { fd: input, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: upgrade.unwrap_or(-1), events: libc::POLLIN, revents: 0 }];
        // SAFETY: two pollfds of ours.
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            *why = "waiting for its input failed";
            return std::process::ExitCode::FAILURE;
        }
        if fds[1].revents & libc::POLLIN != 0 {
            let mut b = [0u8; 16];
            // SAFETY: reading the signal's pipe into a buffer of ours.
            unsafe { libc::read(fds[1].fd, b.as_mut_ptr().cast(), b.len()) };
            upgrading = true;
        }
        if fds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            let mut buf = [0u8; 8192];
            // SAFETY: reading the input into a buffer of ours.
            let n = unsafe { libc::read(input, buf.as_mut_ptr().cast(), buf.len()) };
            match n {
                0 => ended = true,
                n if n > 0 => pending.extend_from_slice(&buf[..n as usize]),
                _ if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => {}
                _ => ended = true,
            }
        }
    }
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Claude Code's folder (`CLAUDE_CONFIG_DIR` or ~/.claude), and its
/// global configuration (`.claude.json` in `CLAUDE_CONFIG_DIR`, else in
/// the home folder), where servers for every project are named.
fn claude_files() -> (PathBuf, PathBuf) {
    match std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from) {
        Some(dir) => (dir.clone(), dir.join(".claude.json")),
        None => (home().join(".claude"), home().join(".claude.json")),
    }
}

fn codex_dir() -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex"))
}

/// The tool's permission in Claude Code ("mcp__SERVER__TOOL").
fn permission() -> String {
    format!("mcp__{SERVER}__{TOOL}")
}

/// `v` (Claude Code's global configuration) naming the server (at
/// `url`), or not; the rest as it was. Whether it changed.
fn claude_server(v: &mut Value, url: &str, on: bool) -> bool {
    let before = v.clone();
    if !v.is_object() {
        *v = json!({});
    }
    let root = v.as_object_mut().expect("an object");
    if on {
        let servers = root.entry("mcpServers").or_insert_with(|| json!({}));
        if !servers.is_object() {
            *servers = json!({});
        }
        servers[SERVER] = json!({"type": "http", "url": url});
    } else if let Some(servers) = root.get_mut("mcpServers").and_then(Value::as_object_mut) {
        servers.shift_remove(SERVER);
        if servers.is_empty() {
            root.shift_remove("mcpServers");
        }
    }
    *v != before
}

/// `v` (Claude Code's `settings.json`) letting the tool be used without
/// asking each time, or not.
fn claude_allow(v: &mut Value, on: bool) -> bool {
    let before = v.clone();
    if !v.is_object() {
        *v = json!({});
    }
    let ours = permission();
    let root = v.as_object_mut().expect("an object");
    let permissions = root.entry("permissions").or_insert_with(|| json!({}));
    if !permissions.is_object() {
        *permissions = json!({});
    }
    let p = permissions.as_object_mut().expect("an object");
    let mut allow: Vec<Value> = p.get("allow").and_then(Value::as_array).cloned().unwrap_or_default();
    allow.retain(|a| a.as_str() != Some(ours.as_str()));
    if on {
        allow.push(Value::String(ours));
    }
    if allow.is_empty() {
        p.shift_remove("allow");
    } else {
        p.insert("allow".into(), Value::Array(allow));
    }
    if p.is_empty() {
        root.shift_remove("permissions");
    }
    *v != before
}

/// `doc` (Codex's `config.toml`) naming the server (at `url`), or not.
fn codex_server(doc: &mut DocumentMut, url: &str, on: bool) -> bool {
    let before = doc.to_string();
    if on {
        if !doc.get("mcp_servers").is_some_and(Item::is_table) {
            let mut t = toml_edit::Table::new();
            t.set_implicit(true);
            doc.insert("mcp_servers", Item::Table(t));
        }
        let servers = doc["mcp_servers"].as_table_mut().expect("a table");
        let mut ours = toml_edit::Table::new();
        ours.insert("url", toml_edit::value(url));
        servers.insert(SERVER, Item::Table(ours));
    } else if let Some(servers) = doc.get_mut("mcp_servers").and_then(Item::as_table_mut) {
        servers.remove(SERVER);
        if servers.is_empty() {
            doc.remove("mcp_servers");
        }
    }
    doc.to_string() != before
}

/// Write `bytes` in place of `path` (a new file put there: one being read
/// never sees half of it), keeping its permissions.
fn replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    olo_ui::store::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_json(path: &Path, on: bool) -> Result<Option<Value>, String> {
    match std::fs::read(path) {
        Ok(b) if !b.iter().all(u8::is_ascii_whitespace) => serde_json::from_slice(&b).map(Some).map_err(|e| format!("{}: {e}", path.display())),
        Ok(_) => Ok(Some(json!({}))),
        Err(_) if !on => Ok(None),
        Err(_) => Ok(Some(json!({}))),
    }
}

fn edit_json(path: &Path, on: bool, edit: impl Fn(&mut Value) -> bool) -> Result<(), String> {
    let Some(mut v) = read_json(path, on)? else { return Ok(()) };
    if !edit(&mut v) {
        return Ok(());
    }
    let mut bytes = serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    replace(path, &bytes)?;
    tracing::info!(file = %path.display(), on, "coding agents' tab title tool");
    Ok(())
}

/// Name the server (the session's, at `title_service::url`) to each
/// agent set up here, or take it out. Claude Code rewrites its global
/// file now and then: read just before, and checked after (put back if
/// a write of its own took it away).
pub fn register(on: bool) -> Result<(), String> {
    let url = crate::title_service::url();
    let (claude, global) = claude_files();
    if claude.is_dir() {
        for _ in 0..3 {
            edit_json(&global, on, |v| claude_server(v, &url, on))?;
            std::thread::sleep(std::time::Duration::from_millis(300));
            let now = read_json(&global, on)?.unwrap_or_default();
            if now["mcpServers"].get(SERVER).is_some() == on {
                break;
            }
        }
        edit_json(&claude.join("settings.json"), on, |v| claude_allow(v, on))?;
    }
    let codex = codex_dir();
    if codex.is_dir() {
        let path = codex.join("config.toml");
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) if !on => return Ok(()),
            Err(_) => String::new(),
        };
        let mut doc: DocumentMut = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;
        if codex_server(&mut doc, &url, on) {
            replace(&path, doc.to_string().as_bytes())?;
            tracing::info!(file = %path.display(), on, "coding agents' tab title tool");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pipe: (read end, write end).
    fn pipe() -> (std::fs::File, std::fs::File) {
        use std::os::fd::FromRawFd as _;
        let mut fds = [0; 2];
        // SAFETY: a new pipe, both ends ours.
        unsafe {
            assert_eq!(libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC), 0);
            (std::fs::File::from_raw_fd(fds[0]), std::fs::File::from_raw_fd(fds[1]))
        }
    }

    /// The server's loop on pipes, `script` writing to its input and its
    /// upgrade signal: the replies, and how many had been written when it
    /// became the new version (each time).
    fn run(script: impl FnOnce(&mut std::fs::File, &mut std::fs::File) + Send + 'static) -> (Vec<Value>, Vec<usize>) {
        use std::os::fd::AsRawFd as _;
        let ((input, mut feed), (signal, mut poke)) = (pipe(), pipe());
        let writer = std::thread::spawn(move || script(&mut feed, &mut poke));
        let mut out: Vec<u8> = Vec::new();
        let replies = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut set = |_: &str, _: Option<&Path>| -> Result<String, String> { Ok("done".into()) };
        let counted = std::rc::Rc::new(std::cell::Cell::new(0usize));
        struct Counting<'a>(&'a mut Vec<u8>, std::rc::Rc<std::cell::Cell<usize>>);
        impl Write for Counting<'_> {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.1.set(self.1.get() + b.iter().filter(|&&c| c == b'\n').count());
                self.0.write(b)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let execs = replies.clone();
        let seen = counted.clone();
        serve_on(input.as_raw_fd(), Some(signal.as_raw_fd()), &mut Counting(&mut out, counted), &mut set, &mut || execs.borrow_mut().push(seen.get()), &mut "");
        writer.join().unwrap();
        let got = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        (got, replies.take())
    }

    fn sleep() {
        std::thread::sleep(std::time::Duration::from_millis(60));
    }

    #[test]
    fn an_upgrade_waits_for_a_message_half_read() {
        let (replies, execs) = run(|feed, poke| {
            // The first message whole and half the second, and the upgrade.
            feed.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n{\"jsonrpc\":\"2.0\",\"id\":2,").unwrap();
            poke.write_all(&[1]).unwrap();
            sleep();
            feed.write_all(b"\"method\":\"tools/list\"}\n").unwrap();
            sleep();
            feed.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}").unwrap();
        });
        assert_eq!(replies.iter().map(|r| r["id"].clone()).collect::<Vec<_>>(), [json!(1), json!(2), json!(3)]);
        assert_eq!(replies[1]["result"]["tools"][0]["name"], TOOL);
        // Not with the second half-read: once it was answered.
        assert_eq!(execs, [2]);
    }

    #[test]
    fn an_upgrade_while_waiting_goes_at_once() {
        let (replies, execs) = run(|feed, poke| {
            feed.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n").unwrap();
            sleep();
            poke.write_all(&[1]).unwrap();
            sleep();
            feed.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n").unwrap();
        });
        assert_eq!(replies.len(), 2);
        assert_eq!(execs, [1]);
    }

    #[test]
    fn the_words_name_the_command_for_an_agent_without_the_tool() {
        let command = format!("`olo-terminal {TITLE_FLAG} ");
        assert!(INSTRUCTIONS.contains(&command) && REMINDER.contains(&command), "{INSTRUCTIONS}\n{REMINDER}");
    }

    #[test]
    fn answers_as_a_tool_server() {
        let mut got: Vec<(String, Option<PathBuf>)> = Vec::new();
        let mut set = |t: &str, cwd: Option<&Path>| -> Result<String, String> {
            got.push((t.to_owned(), cwd.map(Path::to_path_buf)));
            Ok("done".into())
        };
        let init = answer(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}}), &mut set).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(init["result"]["capabilities"]["tools"].is_object());
        // Notifications get no answer.
        assert!(answer(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}), &mut set).is_none());
        let list = answer(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}), &mut set).unwrap();
        assert_eq!(list["result"]["tools"][0]["name"], TOOL);
        let call = json!({"jsonrpc": "2.0", "id": "x", "method": "tools/call", "params": {"name": TOOL, "arguments": {"title": "  Fix\nthe   login ", "cwd": "/src/api"}}});
        let r = answer(&call, &mut set).unwrap();
        assert_eq!(r["id"], "x");
        assert_eq!(r["result"]["isError"], false);
        let bad = answer(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "other"}}), &mut set).unwrap();
        assert!(bad["error"].is_object());
        assert_eq!(answer(&json!({"jsonrpc": "2.0", "id": 4, "method": "resources/list"}), &mut set).unwrap()["error"]["code"], -32601);
        assert_eq!(got, vec![("Fix the login".to_owned(), Some(PathBuf::from("/src/api")))]);
        let long = tidy(&"word ".repeat(30));
        assert!(long.chars().count() <= MOST + 1 && long.ends_with("word…"), "{long}");
    }

    #[test]
    fn names_itself_to_the_agents_and_leaves_the_rest() {
        let mut claude: Value = serde_json::from_str(r#"{"numStartups":3,"mcpServers":{"theirs":{"command":"x"}},"projects":{}}"#).unwrap();
        let original = claude.clone();
        let url = "http://127.0.0.1:17893/mcp";
        assert!(claude_server(&mut claude, url, true));
        assert_eq!(claude["mcpServers"][SERVER], json!({"type": "http", "url": url}));
        assert!(!claude_server(&mut claude, url, true));
        assert!(claude_server(&mut claude, url, false));
        assert_eq!(serde_json::to_string(&claude).unwrap(), serde_json::to_string(&original).unwrap());

        let mut settings: Value = serde_json::from_str(r#"{"permissions":{"allow":["Bash(ls:*)"]},"model":"opus"}"#).unwrap();
        let original = settings.clone();
        assert!(claude_allow(&mut settings, true));
        assert_eq!(settings["permissions"]["allow"][1], permission());
        assert!(!claude_allow(&mut settings, true));
        assert!(claude_allow(&mut settings, false));
        assert_eq!(settings, original);
        let mut empty = json!({});
        claude_allow(&mut empty, true);
        claude_allow(&mut empty, false);
        assert_eq!(empty, json!({}));

        let text = "model = \"gpt-5\"  # theirs\n\n[mcp_servers.docs]\ncommand = \"docs-mcp\"\n";
        let mut doc: DocumentMut = text.parse().unwrap();
        assert!(codex_server(&mut doc, url, true));
        let on = doc.to_string();
        assert!(on.contains("[mcp_servers.olo-terminal]") && on.contains(&format!("url = \"{url}\"")) && !on.contains("command = \"/x"), "{on}");
        assert!(codex_server(&mut doc, url, false));
        assert_eq!(doc.to_string(), text);
        let mut fresh: DocumentMut = "".parse().unwrap();
        codex_server(&mut fresh, url, true);
        codex_server(&mut fresh, url, false);
        assert_eq!(fresh.to_string(), "");
    }

    /// The stdio server an agent was given before is replaced by the
    /// session's, over HTTP.
    #[test]
    fn the_stdio_server_gives_way_to_the_session_s() {
        let url = "http://127.0.0.1:17893/mcp";
        let mut claude = json!({"mcpServers": {SERVER: {"type": "stdio", "command": "/home/x/.local/bin/olo-terminal", "args": [MCP_FLAG], "env": {}}}});
        assert!(claude_server(&mut claude, url, true));
        assert_eq!(claude["mcpServers"][SERVER], json!({"type": "http", "url": url}));
        let mut doc: DocumentMut = "[mcp_servers.olo-terminal]\ncommand = \"/home/x/.local/bin/olo-terminal\"\nargs = [\"--mcp\"]\n".parse().unwrap();
        assert!(codex_server(&mut doc, url, true));
        assert_eq!(doc.to_string(), format!("[mcp_servers.olo-terminal]\nurl = \"{url}\"\n"));
    }
}
