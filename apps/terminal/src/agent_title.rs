//! Coding agents keep their tab's title current (Preferences ▸ Terminal
//! ▸ "Let coding agents keep their tab’s title current", off by
//! default): a Claude Code or Codex in a tab has a tool, `set_tab_title`,
//! and is told (agent_context.rs's hook) to keep the title saying what
//! it's working on.
//!
//! The tool is an MCP server, `olo-terminal --mcp` (JSON-RPC over its
//! stdin and stdout), which both agents run outside their sandboxes: on,
//! Terminal names it in Claude Code's `.claude.json` (`mcpServers`, with
//! an allow rule for the tool in its `settings.json`) and in Codex's
//! `config.toml` (`mcp_servers`); off, it takes it out again, leaving the
//! rest of each file as it was. The server finds its tab as the hook does
//! (Claude Code runs it under the tab's shell; Codex in its daemon, so by
//! the folder the agent gives, else its own) and hands the title to that
//! tab's Terminal (`--set-tab-title SHELL TITLE`, through its
//! single-instance socket on the tab's display). The title shows in the
//! tab as a program's own title would, ahead of it, until the agent
//! leaves the tab or gives an empty one.

use std::{
    io::{BufRead, Write},
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use toml_edit::{DocumentMut, Item};

use crate::agent_context::{self, TabInfo};

/// What the server is run with.
pub const MCP_FLAG: &str = "--mcp";
/// What Terminal is handed (`--set-tab-title SHELL TITLE`).
pub const SET_FLAG: &str = "--set-tab-title";
/// The server's name in the agents' settings, and so the tool's prefix.
const SERVER: &str = "olo-terminal";
const TOOL: &str = "set_tab_title";
/// A title is a few words: longer is cut.
pub const MOST: usize = 60;

/// Told at the start of a session (the hook).
pub const INSTRUCTIONS: &str = "This session runs in a tab of Olo's Terminal. Keep the tab's title saying what you are working on: call the set_tab_title tool (the olo-terminal MCP server) with a few words, such as \"Fix login redirect\", when you start on a task and whenever the work moves on to something else, passing your working directory as cwd.";
/// Told with each prompt.
pub const REMINDER: &str = "(Olo Terminal: if this changes what you are working on, update the tab's title with set_tab_title.)";

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

/// The tab this server is for: under its shell (Claude Code), else in
/// `cwd` (Codex: what the agent gives, else the server's own).
fn find_tab(cwd: Option<&Path>) -> Option<TabInfo> {
    let snapshot = agent_context::read_all()?;
    let ancestors = agent_context::ancestors();
    let own = std::env::current_dir().ok();
    agent_context::tab_of(&snapshot, &ancestors, cwd.or(own.as_deref())).cloned()
}

/// Set `tab`'s title through its Terminal.
fn hand_over(tab: &TabInfo, title: &str) -> Result<(), String> {
    let shell = tab.shell.ok_or("the tab has no shell")?;
    let display = tab.display.as_deref().unwrap_or_default();
    let args = [SET_FLAG.to_owned(), shell.to_string(), title.to_owned()];
    if olo_kit::instance::forward_on(crate::APP_ID, display, &args) { Ok(()) } else { Err("Terminal isn't answering".into()) }
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
type Set<'a> = dyn FnMut(&str, Option<&Path>) -> Result<String, String> + 'a;

/// The answer to one request (None for a notification), `set` doing the
/// tool's work.
fn answer(msg: &Value, set: &mut Set) -> Option<Value> {
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
pub fn serve() -> std::process::ExitCode {
    let mut set = |title: &str, cwd: Option<&Path>| -> Result<String, String> {
        let tab = find_tab(cwd).ok_or("This session isn't in an Olo Terminal tab (or Terminal's setting for it is off).")?;
        hand_over(&tab, title)?;
        Ok(if title.is_empty() { "The tab's title is its own again.".into() } else { format!("The tab is titled \u{201c}{title}\u{201d}.") })
    };
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => answer(&msg, &mut set),
            Err(_) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}})),
        };
        if let Some(reply) = reply
            && (writeln!(out, "{reply}").is_err() || out.flush().is_err())
        {
            break;
        }
    }
    std::process::ExitCode::SUCCESS
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

/// `v` (Claude Code's global configuration) naming the server (run as
/// `exe`), or not; the rest as it was. Whether it changed.
fn claude_server(v: &mut Value, exe: &str, on: bool) -> bool {
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
        servers[SERVER] = json!({"type": "stdio", "command": exe, "args": [MCP_FLAG], "env": {}});
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

/// `doc` (Codex's `config.toml`) naming the server, or not.
fn codex_server(doc: &mut DocumentMut, exe: &str, on: bool) -> bool {
    let before = doc.to_string();
    if on {
        if !doc.get("mcp_servers").is_some_and(Item::is_table) {
            let mut t = toml_edit::Table::new();
            t.set_implicit(true);
            doc.insert("mcp_servers", Item::Table(t));
        }
        let servers = doc["mcp_servers"].as_table_mut().expect("a table");
        let mut ours = toml_edit::Table::new();
        ours.insert("command", toml_edit::value(exe));
        let mut args = toml_edit::Array::new();
        args.push(MCP_FLAG);
        ours.insert("args", toml_edit::value(args));
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
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).map(|m| m.permissions().mode()).unwrap_or(0o600);
    let tmp = path.with_extension("olo-tmp");
    std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode)))
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| format!("{}: {e}", path.display()))
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

/// Name the server (this program, `exe`) to each agent set up here, or
/// take it out. Claude Code rewrites its global file now and then: read
/// just before, and checked after (put back if a write of its own took
/// it away).
pub fn register(on: bool, exe: &Path) -> Result<(), String> {
    let exe = exe.display().to_string();
    let (claude, global) = claude_files();
    if claude.is_dir() {
        for _ in 0..3 {
            edit_json(&global, on, |v| claude_server(v, &exe, on))?;
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
        if codex_server(&mut doc, &exe, on) {
            replace(&path, doc.to_string().as_bytes())?;
            tracing::info!(file = %path.display(), on, "coding agents' tab title tool");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(claude_server(&mut claude, "/x/olo-terminal", true));
        assert_eq!(claude["mcpServers"][SERVER]["args"][0], MCP_FLAG);
        assert!(!claude_server(&mut claude, "/x/olo-terminal", true));
        assert!(claude_server(&mut claude, "/x/olo-terminal", false));
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
        assert!(codex_server(&mut doc, "/x/olo-terminal", true));
        let on = doc.to_string();
        assert!(on.contains("[mcp_servers.olo-terminal]") && on.contains("command = \"/x/olo-terminal\"") && on.contains("args = [\"--mcp\"]"), "{on}");
        assert!(codex_server(&mut doc, "/x/olo-terminal", false));
        assert_eq!(doc.to_string(), text);
        let mut fresh: DocumentMut = "".parse().unwrap();
        codex_server(&mut fresh, "/x/olo-terminal", true);
        codex_server(&mut fresh, "/x/olo-terminal", false);
        assert_eq!(fresh.to_string(), "");
    }
}
