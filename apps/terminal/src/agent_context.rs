//! Coding agents told about their tab's group (Preferences ▸ Terminal ▸
//! "Tell coding agents about the repositories in their tab group", off
//! by default): a Claude Code or Codex in a tab of a group whose tabs
//! are in more than one repository (a group per feature, the feature
//! spanning repositories) is told the group's name and those
//! repositories and worktrees, its own marked, and nothing assumed about
//! what they are for.
//!
//! Through the agents' own hooks, both of which take a command's output
//! as context: on, Terminal puts `olo-terminal --agent-context` in Claude
//! Code's `settings.json` and Codex's `hooks.json` (`SessionStart`, and
//! `UserPromptSubmit` to keep it current: printed again only when the
//! group changed); off, it takes them out again, leaving the rest of
//! each file as it was. Codex asks the person once to trust the hook
//! (`/hooks`). While on, Terminal keeps its tabs' groups in a file of its
//! own (`$XDG_RUNTIME_DIR/olo-terminal-groups-<display>.json`, every two seconds
//! when they change: each tab's shell's pid, group, folder and agent),
//! which the hook reads: Claude Code runs it under the tab's shell (its
//! pid is an ancestor), Codex in its daemon, so there the tab is found
//! by the session's folder (one running Codex first).
//!
//! The same hook tells them to keep their tab's title current when that
//! setting is on (agent_title.rs: at the start, then a line with each
//! prompt); the file is kept while either is on, and says which.

use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// What marks the hook as Olo's in an agent's settings.
pub const FLAG: &str = "--agent-context";

/// The events it is a hook of.
const EVENTS: [&str; 2] = ["SessionStart", "UserPromptSubmit"];

/// A tab, as the hook needs it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TabInfo {
    /// Its shell's pid.
    pub shell: Option<i32>,
    /// Its group's name.
    pub group: Option<String>,
    pub cwd: Option<PathBuf>,
    /// The coding agent running in it ("Claude Code", "Codex").
    pub agent: Option<String>,
    /// Its Terminal's display (`wayland-1`), to reach it there.
    #[serde(default)]
    pub display: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    pub tabs: Vec<TabInfo>,
    /// Agents are told about their groups (from before the titles: on).
    pub groups: bool,
    /// Agents keep their tabs' titles current (agent_title.rs).
    pub titles: bool,
}

impl Default for Snapshot {
    fn default() -> Self {
        Snapshot { tabs: Vec::new(), groups: true, titles: false }
    }
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| home().join(".cache"))
}

/// This Terminal's (one for each Olo session, by its display: Agent
/// Screens run Terminals of their own).
fn snapshot_path() -> PathBuf {
    let display: String = olo_xdg::display_name().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    runtime_dir().join(format!("{SNAPSHOT}-{display}.json"))
}

const SNAPSHOT: &str = "olo-terminal-groups";

/// Every Terminal's tabs (each session's), and what agents are told (any
/// Terminal's settings: they're the person's, one for all).
pub fn read_all() -> Option<Snapshot> {
    let mut tabs = Vec::new();
    let (mut any, mut groups, mut titles) = (false, false, false);
    for e in std::fs::read_dir(runtime_dir()).ok()?.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with(SNAPSHOT) && name.ends_with(".json")) {
            continue;
        }
        if let Some(s) = std::fs::read(e.path()).ok().and_then(|b| serde_json::from_slice::<Snapshot>(&b).ok()) {
            any = true;
            groups |= s.groups;
            titles |= s.titles;
            // (Not a Terminal's that has gone: its shells have.)
            tabs.extend(s.tabs.into_iter().filter(|t| t.shell.is_none_or(|p| Path::new(&format!("/proc/{p}")).exists())));
        }
    }
    any.then_some(Snapshot { tabs, groups, titles })
}

/// Keep `s` for the hook (a new file put in place of the old, for this
/// user only).
pub fn write(s: &Snapshot) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = snapshot_path();
    let tmp = path.with_extension("json.tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    std::io::Write::write_all(&mut f, &serde_json::to_vec(s).map_err(std::io::Error::other)?)?;
    std::fs::rename(tmp, path)
}

/// Off: no file for the hook to find.
pub fn clear() {
    let _ = std::fs::remove_file(snapshot_path());
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Claude Code's settings file (`CLAUDE_CONFIG_DIR` or ~/.claude), and
/// Codex's hooks (`CODEX_HOME` or ~/.codex): each only if that agent is
/// set up here (its folder there).
fn hook_files() -> Vec<PathBuf> {
    let claude = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home().join(".claude"));
    let codex = std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex"));
    [(claude, "settings.json"), (codex, "hooks.json")].into_iter().filter(|(dir, _)| dir.is_dir()).map(|(dir, file)| dir.join(file)).collect()
}

/// `v` (an agent's settings) with Olo's hook (`command`) on each event,
/// or none of it; anything else as it was. Whether it changed.
fn set_hook(v: &mut Value, command: &str, on: bool) -> bool {
    let before = v.clone();
    if !v.is_object() {
        *v = json!({});
    }
    let ours = |entry: &Value| entry["hooks"].as_array().is_some_and(|hs| hs.iter().any(|h| h["command"].as_str().is_some_and(|c| c.contains(FLAG))));
    let root = v.as_object_mut().expect("an object");
    if !root.get("hooks").is_some_and(Value::is_object) {
        root.insert("hooks".into(), json!({}));
    }
    let hooks = root["hooks"].as_object_mut().expect("an object");
    for event in EVENTS {
        let mut list: Vec<Value> = hooks.get(event).and_then(Value::as_array).cloned().unwrap_or_default();
        list.retain(|e| !ours(e));
        if on {
            list.push(json!({"hooks": [{"type": "command", "command": command}]}));
        }
        if list.is_empty() {
            hooks.shift_remove(event);
        } else {
            hooks.insert(event.into(), Value::Array(list));
        }
    }
    if hooks.is_empty() {
        root.shift_remove("hooks");
    }
    *v != before
}

/// Put Olo's hook (this program, `exe`) in the agents' settings, or take
/// it out.
pub fn set_hooks(on: bool, exe: &Path) -> Result<(), String> {
    let command = format!("{} {FLAG}", exe.display());
    for path in hook_files() {
        let mut v: Value = match std::fs::read(&path) {
            Ok(b) if !b.iter().all(u8::is_ascii_whitespace) => serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", path.display()))?,
            Ok(_) => json!({}),
            Err(_) if !on => continue,
            Err(_) => json!({}),
        };
        if !set_hook(&mut v, &command, on) {
            continue;
        }
        let mut bytes = serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        let tmp = path.with_extension("json.olo-tmp");
        std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, &path)).map_err(|e| format!("{}: {e}", path.display()))?;
        tracing::info!(file = %path.display(), on, "coding agents' hook for tab groups");
    }
    Ok(())
}

/// This process's ancestors, its parent first.
pub fn ancestors() -> Vec<i32> {
    let mut all = Vec::new();
    let mut pid = std::process::id() as i32;
    while let Some(p) = parent(pid).filter(|&p| p > 1 && all.len() < 64) {
        all.push(p);
        pid = p;
    }
    all
}

/// Process `pid`'s parent.
fn parent(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
}

/// The tab the agent is in: the one whose shell is an ancestor of this
/// process (`ancestors`), else one in the session's folder (`cwd`), one
/// running an agent first.
pub fn tab_of<'a>(s: &'a Snapshot, ancestors: &[i32], cwd: Option<&Path>) -> Option<&'a TabInfo> {
    if let Some(t) = s.tabs.iter().find(|t| t.shell.is_some_and(|p| ancestors.contains(&p))) {
        return Some(t);
    }
    let cwd = cwd?;
    let here = |t: &&TabInfo| t.cwd.as_deref() == Some(cwd);
    let near = |t: &&TabInfo| t.cwd.as_deref().is_some_and(|c| cwd.starts_with(c) || c.starts_with(cwd));
    s.tabs
        .iter()
        .filter(here)
        .find(|t| t.agent.is_some())
        .or_else(|| s.tabs.iter().find(here))
        .or_else(|| s.tabs.iter().filter(near).find(|t| t.agent.is_some()))
        .or_else(|| s.tabs.iter().find(near))
}

/// Where a tab is: its repository's worktree (its path, the repository's
/// main one if it's a linked worktree, its branch), else its folder.
#[derive(Debug, Clone, PartialEq)]
struct Place {
    path: PathBuf,
    describe: String,
}

fn place_of(dir: &Path) -> Place {
    match crate::git::look(dir) {
        Some(r) => {
            let branch = r.branch.as_deref().map(|b| format!(", branch {b}")).unwrap_or_default();
            let what = if r.linked { format!("a worktree of {} ({})", r.name, r.main.display()) } else { format!("repository {}", r.name) };
            Place { describe: format!("{} ({what}{branch})", r.here.display()), path: r.here }
        }
        None => Place { describe: format!("{} (a folder, not in a git repository)", dir.display()), path: dir.to_path_buf() },
    }
}

/// The block for `tab`'s group in `s`: None if it's in none, or all its
/// tabs are in the one place.
fn block(s: &Snapshot, tab: &TabInfo, place: &dyn Fn(&Path) -> Place) -> Option<String> {
    let group = tab.group.as_deref()?;
    let own = tab.cwd.as_deref().map(place);
    let mut places: Vec<Place> = Vec::new();
    for t in s.tabs.iter().filter(|t| t.group.as_deref() == Some(group)) {
        if let Some(p) = t.cwd.as_deref().map(place)
            && !places.iter().any(|q| q.path == p.path)
        {
            places.push(p);
        }
    }
    if places.len() < 2 {
        return None;
    }
    let lines: Vec<String> = places
        .iter()
        .map(|p| if own.as_ref().is_some_and(|o| o.path == p.path) { format!("- {} ← this session's", p.describe) } else { format!("- {}", p.describe) })
        .collect();
    Some(format!(
        "You are running in a tab of an Olo Terminal tab group named \"{group}\". The tabs in that group are in these places (yours included):\n{}",
        lines.join("\n")
    ))
}

/// What was last told to session `id` (to tell it again only when the
/// group changed).
fn told_path(id: &str) -> PathBuf {
    let id: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(80).collect();
    runtime_dir().join("olo-agent-context").join(id)
}

/// `olo-terminal --agent-context`: the hook. Claude Code or Codex gives
/// the session on stdin (its id, folder and event); what it prints is
/// added to the agent's context. Nothing printed: nothing to tell.
pub fn run() -> std::process::ExitCode {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let v: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let cwd = v["cwd"].as_str().map(PathBuf::from);
    let event = v["hook_event_name"].as_str().unwrap_or("SessionStart");
    let session = v["session_id"].as_str().unwrap_or("");
    let ancestors = ancestors();
    let read = read_all;
    // A tab just opened may not be in it yet (it's kept every two
    // seconds): a moment's grace, at the start only.
    let mut snapshot = read();
    let deadline = std::time::Instant::now() + Duration::from_millis(2500);
    while event == "SessionStart"
        && snapshot.is_some()
        && snapshot.as_ref().is_some_and(|s| tab_of(s, &ancestors, cwd.as_deref()).is_none())
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(250));
        snapshot = read();
    }
    let Some(s) = snapshot else { return std::process::ExitCode::SUCCESS };
    let Some(tab) = tab_of(&s, &ancestors, cwd.as_deref()) else { return std::process::ExitCode::SUCCESS };
    let mut said = Vec::new();
    // Its title: how at the start, a reminder with each prompt.
    if s.titles {
        said.push(if event == "UserPromptSubmit" { crate::agent_title::REMINDER } else { crate::agent_title::INSTRUCTIONS }.to_owned());
    }
    // Its group: again only when it changed.
    let text = if s.groups { block(&s, tab, &place_of).unwrap_or_default() } else { String::new() };
    let told = (!session.is_empty()).then(|| told_path(session));
    let same = event == "UserPromptSubmit" && told.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default() == text;
    if !same {
        if let Some(p) = told {
            let _ = p.parent().map(std::fs::create_dir_all);
            let _ = std::fs::write(p, &text);
        }
        if !text.is_empty() {
            said.push(text);
        }
    }
    if !said.is_empty() {
        println!("{}", said.join("\n\n"));
    }
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(shell: i32, group: Option<&str>, cwd: &str, agent: Option<&str>) -> TabInfo {
        TabInfo { shell: Some(shell), group: group.map(String::from), cwd: Some(cwd.into()), agent: agent.map(String::from), display: None }
    }

    fn place(p: &Path) -> Place {
        Place { path: p.to_path_buf(), describe: format!("{} (repository {})", p.display(), p.file_name().unwrap().to_string_lossy()) }
    }

    #[test]
    fn tells_the_group_and_its_places() {
        let s = Snapshot {
            tabs: vec![
                tab(10, Some("payments"), "/src/api", Some("Claude Code")),
                tab(11, Some("payments"), "/src/web", None),
                tab(12, Some("payments"), "/src/api", None),
                tab(13, None, "/src/other", Some("Codex")),
                tab(14, Some("docs"), "/src/docs", None),
            ],
            ..Default::default()
        };
        // Under shell 11's tab: its group, each place once, its own marked.
        let t = tab_of(&s, &[999, 11, 1], None).unwrap();
        let text = block(&s, t, &place).unwrap();
        assert!(text.contains("tab group named \"payments\""), "{text}");
        assert_eq!(text.lines().filter(|l| l.starts_with("- ")).count(), 2, "{text}");
        assert!(text.contains("- /src/web (repository web) ← this session's") && text.contains("- /src/api (repository api)\n"), "{text}");
        // Codex in its daemon: by folder, the tab running an agent first.
        let t = tab_of(&s, &[], Some(Path::new("/src/api"))).unwrap();
        assert_eq!(t.shell, Some(10));
        // In no group, or a group in one place: nothing.
        assert_eq!(block(&s, tab_of(&s, &[13], None).unwrap(), &place), None);
        assert_eq!(block(&s, tab_of(&s, &[14], None).unwrap(), &place), None);
        assert!(tab_of(&s, &[], Some(Path::new("/elsewhere"))).is_none());
    }

    #[test]
    fn knows_its_parent() {
        // SAFETY: getppid can't fail.
        assert_eq!(parent(std::process::id() as i32), Some(unsafe { libc::getppid() }));
    }

    #[test]
    fn puts_its_hook_in_and_takes_it_out() {
        let mut v: Value = serde_json::from_str(
            r#"{"model":"opus","hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"their-hook"}]}]},"env":{"A":"1"}}"#,
        )
        .unwrap();
        let original = v.clone();
        assert!(set_hook(&mut v, "/x/olo-terminal --agent-context", true));
        assert_eq!(v["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
        assert_eq!(v["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"], "/x/olo-terminal --agent-context");
        // Again: no change (one of ours each, not two).
        assert!(!set_hook(&mut v, "/x/olo-terminal --agent-context", true));
        // Moved: the new path in place of the old.
        assert!(set_hook(&mut v, "/y/olo-terminal --agent-context", true));
        assert_eq!(v["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
        // Off: as it was, keys in their order.
        assert!(set_hook(&mut v, "/y/olo-terminal --agent-context", false));
        assert_eq!(serde_json::to_string(&v).unwrap(), serde_json::to_string(&original).unwrap());
        // A file of only ours: empty again.
        let mut w = json!({});
        set_hook(&mut w, "/x/olo-terminal --agent-context", true);
        set_hook(&mut w, "/x/olo-terminal --agent-context", false);
        assert_eq!(w, json!({}));
    }
}
