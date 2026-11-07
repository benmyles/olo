//! Olo Suggestions' suggestions: what Olo suggests for the best of it,
//! none of it needed (Olo works the same without), each applied with a
//! button and undone with another. Looked at as Olo Doctor's checks are
//! (a `Report`: what's suggested as findings, what's applied as fine,
//! with its undo), applied by the same steps and helper.
//!
//! - **brush as the login shell**: brush (github.com/reubeno/brush), the
//!   bash-compatible shell written in Rust, built with Olo (`olo-brush`,
//!   installed with Olo's programs) and copied to /usr/local/bin/brush,
//!   listed in /etc/shells, tried, and made the login shell through
//!   AccountsService. A copy older than Olo's build is suggested again
//!   (Update brush); undone, the shell is the one it was before (kept
//!   in ~/.config/olo/suggestions.json as it's applied), else bash.
//! - **Programs worth having** (`TOOLS`): just (justfiles), uv (Python's
//!   packages and versions), mise (each project's tools at their
//!   versions) and worktrunk (`wt`, git worktrees): suggested while
//!   they're on no folder of PATH (nor ~/.local/bin, ~/.cargo/bin), from
//!   the system's repositories through PackageKit where Fedora carries
//!   them (just, uv), else from their maker's latest GitHub release into
//!   ~/.local/bin, its SHA-256 checked (`releases`). Installed, they're
//!   said to be; Olo doesn't take them away.
//! - **Worktrunk's worktrees where Terminal makes them**: with `wt`
//!   there, its `worktree-path` (~/.config/worktrunk/config.toml) set to
//!   Terminal's place (Preferences ▸ Terminal,
//!   `<place>/<repository>/<branch>`), the rest of its config as it was;
//!   undone, the value it had (or none) again.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    checks::{Finding, Fine, Fix, Report, Severity, Undo},
    fix::Step,
};

/// Where brush is installed as a login shell.
pub use olo_system::accounts::BRUSH;
use olo_system::accounts::login_shell;
const BRUSH_ID: &str = "brush-shell";

/// What's suggested now, and what's applied. (`OLO_SUGGESTIONS_DEMO=
/// applied`: everything applied, made up, to look at headless without
/// changing the computer.)
/// What Olo Suggestions suggests now, what's ignored set aside (ignored.rs).
pub fn look() -> Report {
    let mut report = scan();
    report.set_aside(&crate::ignored::of(crate::app::Kind::Suggestions));
    report
}

pub fn scan() -> Report {
    let mut r = Report::default();
    if std::env::var("OLO_SUGGESTIONS_DEMO").as_deref() == Ok("applied") {
        applied_brush(&mut r, "person".into(), PathBuf::from("/bin/bash"));
        return r;
    }
    brush(&mut r);
    tools(&mut r);
    worktrunk_place(&mut r);
    r
}

/// brush is the login shell: so said, with its undo (`previous` again).
fn applied_brush(r: &mut Report, user: String, previous: PathBuf) {
    let name = previous.file_name().map_or_else(|| previous.display().to_string(), |n| n.to_string_lossy().into_owned());
    let title = "brush is your login shell".to_owned();
    r.fine.push(Fine { title: title.clone(), detail: "New shells, Terminal's too, are brush. bash scripts run as before.".into() });
    r.undo.push(Undo { id: format!("undo:{BRUSH_ID}"), title, fix: Fix { label: format!("Use {name} Again"), root: true, steps: vec![Step::SetLoginShell { user, shell: previous }] } });
}

/// Before `id` is applied: what it changes is kept, to be put back.
pub fn before(id: &str) {
    if id == WORKTRUNK_PLACE_ID {
        let mut s = saved();
        s.previous_worktree_path = worktree_path(&read_worktrunk_config());
        save(&s);
        return;
    }
    if id == BRUSH_ID
        && let Some((_, shell)) = login_shell()
        && shell != Path::new(BRUSH)
    {
        let mut s = saved();
        s.previous_shell = Some(shell);
        save(&s);
    }
}

/// What applying changed, to undo it.
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    previous_shell: Option<PathBuf>,
    /// worktrunk's `worktree-path` before Olo set it (None: it had none).
    #[serde(default)]
    previous_worktree_path: Option<String>,
}

fn state_file() -> PathBuf {
    olo_xdg::config_dir().join("suggestions.json")
}

fn saved() -> Saved {
    std::fs::read(state_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(s: &Saved) {
    let path = state_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(s) {
        let tmp = path.with_extension("json.new");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }
}

/// Whether two files hold the same bytes (a copy that's current).
fn same_bytes(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else { return false };
    ma.len() == mb.len() && std::fs::read(a).ok() == std::fs::read(b).ok()
}

fn brush(r: &mut Report) {
    let Some((user, shell)) = login_shell() else { return };
    let ours = olo_xdg::find_program("olo-brush");
    let brush = Path::new(BRUSH);
    let current = ours.as_ref().is_some_and(|o| same_bytes(o, brush));
    if shell == brush && current {
        let previous = saved().previous_shell.filter(|p| p != brush && p.exists()).unwrap_or_else(|| PathBuf::from("/bin/bash"));
        return applied_brush(r, user, previous);
    }
    let detail =
        "brush (github.com/reubeno/brush) is a shell compatible with bash, written in Rust as Olo is. New shells, Terminal's too, use it; bash scripts run as before, and it's undone with a button.";
    let Some(ours) = ours else {
        r.findings.push(Finding {
            id: BRUSH_ID.into(),
            severity: Severity::Advice,
            title: "Use brush as your shell".into(),
            detail: format!("{detail} (Olo's build of brush comes with its next upgrade.)"),
            fix: None,
        });
        return;
    };
    let (title, label) = if shell == brush { ("Update brush", "Update brush") } else { ("Use brush as your shell", "Use brush") };
    let mut steps = vec![Step::CopyFile { from: ours, to: brush.to_path_buf(), mode: 0o755 }, Step::AddShell { path: brush.to_path_buf() }, Step::TryShell { path: brush.to_path_buf() }];
    if shell != brush {
        steps.push(Step::SetLoginShell { user, shell: brush.to_path_buf() });
    }
    let detail = if shell == brush { "Your login shell is brush, but the copy in /usr/local/bin is older than Olo's build: it's updated.".to_owned() } else { detail.to_owned() };
    r.findings.push(Finding { id: BRUSH_ID.into(), severity: Severity::Advice, title: title.into(), detail, fix: Some(Fix { label: label.into(), root: true, steps }) });
}

/// A program Olo suggests, and where it comes from.
struct Tool {
    id: &'static str,
    name: &'static str,
    program: &'static str,
    what: &'static str,
    from: From,
}

enum From {
    /// The system's repositories (PackageKit).
    Package(&'static str),
    /// Its maker's GitHub release.
    Release(crate::releases::Release),
}

/// The programs suggested, for this processor.
fn tool_list() -> Vec<Tool> {
    let (short, long) = if std::env::consts::ARCH == "aarch64" { ("arm64", "aarch64") } else { ("x64", "x86_64") };
    vec![
        Tool { id: "tool:just", name: "just", program: "just", what: "runs a project's commands from its justfile (“just test”, “just deploy”).", from: From::Package("just") },
        Tool { id: "tool:uv", name: "uv", program: "uv", what: "installs Python and its packages, fast, each project's apart.", from: From::Package("uv") },
        Tool {
            id: "tool:mise",
            name: "mise",
            program: "mise",
            what: "gives each project the versions of its tools it asks for (node, python, go and the rest), as it's entered.",
            from: From::Release(crate::releases::Release { repo: "jdx/mise".into(), asset: format!("mise-{{tag}}-linux-{short}"), sums: "SHASUMS256.txt".into(), member: None }),
        },
        Tool {
            id: "tool:worktrunk",
            name: "worktrunk",
            program: "wt",
            what: "makes, switches between and cleans up git worktrees (“wt switch -c feature”), one per branch, as Terminal's New Worktree does.",
            from: From::Release(crate::releases::Release {
                repo: "max-sixty/worktrunk".into(),
                asset: format!("worktrunk-{long}-unknown-linux-musl.tar.xz"),
                sums: "{asset}.sha256".into(),
                member: Some("wt".into()),
            }),
        },
    ]
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Where `program` is: a folder of PATH, else where installers put them
/// (~/.local/bin, ~/.cargo/bin), which a session's PATH may not have.
fn found(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    dirs.extend([home().join(".local/bin"), home().join(".cargo/bin")]);
    dirs.into_iter().map(|d| d.join(program)).find(|p| p.is_file())
}

fn tools(r: &mut Report) {
    for t in tool_list() {
        if let Some(at) = found(t.program) {
            let dir = at.parent().map(|d| match d.strip_prefix(home()) {
                Ok(rest) => format!("~/{}", rest.display()),
                Err(_) => d.display().to_string(),
            });
            r.fine.push(Fine { title: format!("{} is installed", t.name), detail: format!("{} {} It's in {}.", t.name, t.what, dir.unwrap_or_default()) });
            continue;
        }
        let (root, steps, whence) = match t.from {
            From::Package(name) => (true, vec![Step::InstallPackages { names: vec![name.into()] }], "from the system's repositories".to_owned()),
            From::Release(release) => {
                let whence = format!("from github.com/{} into ~/.local/bin, checked against what its maker published", release.repo);
                (false, vec![Step::InstallRelease { name: t.name.into(), release, to: home().join(".local/bin").join(t.program) }], whence)
            }
        };
        let label = format!("Install {}", t.name);
        r.findings.push(Finding {
            id: t.id.into(),
            severity: Severity::Advice,
            title: format!("Install {}", t.name),
            detail: format!("{} {} It's installed {whence}.", t.name, t.what),
            fix: Some(Fix { label, root, steps }),
        });
    }
}

const WORKTRUNK_PLACE_ID: &str = "worktrunk-place";

fn worktrunk_config_path() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(".config"));
    config.join("worktrunk/config.toml")
}

fn read_worktrunk_config() -> toml_edit::DocumentMut {
    std::fs::read_to_string(worktrunk_config_path()).ok().and_then(|t| t.parse().ok()).unwrap_or_default()
}

fn worktree_path(doc: &toml_edit::DocumentMut) -> Option<String> {
    doc.get("worktree-path").and_then(|v| v.as_str()).map(str::to_owned)
}

/// Terminal's place for worktrees as worktrunk's template: what New
/// Worktree… makes, `<place>/<repository>/<name>`.
fn terminal_template(place: &str) -> String {
    format!("{}/{{{{ repo }}}}/{{{{ branch | sanitize }}}}", place.trim_end_matches('/'))
}

/// Where Terminal makes worktrees (Preferences ▸ Terminal).
fn terminal_place() -> String {
    olo_protocol::Client::connect()
        .ok()
        .and_then(|mut c| match c.request(&olo_protocol::Request::GetSettings) {
            Ok(olo_protocol::Reply::Settings { settings }) => Some(settings.terminal.worktrees),
            _ => None,
        })
        .unwrap_or_else(|| olo_protocol::TerminalSettings::default().worktrees)
}

/// The config with `worktree-path` set to `value` (None: taken out), the
/// rest as it was.
fn with_worktree_path(mut doc: toml_edit::DocumentMut, value: Option<&str>) -> String {
    match value {
        Some(v) => {
            doc["worktree-path"] = toml_edit::value(v);
        }
        None => {
            doc.remove("worktree-path");
        }
    }
    doc.to_string()
}

fn worktrunk_place(r: &mut Report) {
    if found("wt").is_none() {
        return;
    }
    let place = terminal_place();
    let want = terminal_template(&place);
    let doc = read_worktrunk_config();
    let now = worktree_path(&doc);
    let path = worktrunk_config_path();
    let title = "Worktrunk makes worktrees where Terminal does".to_owned();
    if now.as_deref() == Some(want.as_str()) {
        let before = saved().previous_worktree_path;
        let content = with_worktree_path(doc, before.as_deref());
        let label = if before.is_some() { "Put Worktrunk's Place Back" } else { "Use Worktrunk's Own Place" };
        r.fine.push(Fine { title: title.clone(), detail: format!("In {place}/<repository>/<branch>, as Preferences ▸ Terminal has it: Terminal and “wt switch” make them in one place.") });
        r.undo.push(Undo { id: format!("undo:{WORKTRUNK_PLACE_ID}"), title, fix: Fix { label: label.into(), root: false, steps: vec![Step::WriteFile { path, content, mode: 0o644 }] } });
        return;
    }
    let where_now = match &now {
        Some(t) => format!("in {t}"),
        None => "beside each repository (its own place)".into(),
    };
    let content = with_worktree_path(doc, Some(&want));
    r.findings.push(Finding {
        id: WORKTRUNK_PLACE_ID.into(),
        severity: Severity::Advice,
        title: "Make worktrunk's worktrees where Terminal does".into(),
        detail: format!("worktrunk makes them {where_now}; Terminal's New Worktree… in {place}/<repository>/<name> (Preferences ▸ Terminal). Worktrunk is set to make them in Terminal's place, the rest of its settings left as they are."),
        fix: Some(Fix { label: "Use Terminal's Place".into(), root: false, steps: vec![Step::WriteFile { path, content, mode: 0o644 }] }),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktrunk_follows_terminals_place() {
        assert_eq!(terminal_template("~/worktrees"), "~/worktrees/{{ repo }}/{{ branch | sanitize }}");
        assert_eq!(terminal_template("/srv/trees/"), "/srv/trees/{{ repo }}/{{ branch | sanitize }}");
        // The rest of its config kept, comments too.
        let doc: toml_edit::DocumentMut = "# mine\ncommit-generation = \"x\"\n".parse().unwrap();
        let set = with_worktree_path(doc, Some("~/worktrees/{{ repo }}/{{ branch | sanitize }}"));
        assert!(set.contains("# mine") && set.contains("commit-generation = \"x\""), "{set}");
        let doc: toml_edit::DocumentMut = set.parse().unwrap();
        assert_eq!(worktree_path(&doc).as_deref(), Some("~/worktrees/{{ repo }}/{{ branch | sanitize }}"));
        assert!(!with_worktree_path(doc, None).contains("worktree-path"));
    }

    #[test]
    fn programs_suggested_or_said_installed() {
        let r = scan();
        for t in tool_list() {
            let suggested = r.find(t.id).is_some();
            let installed = r.fine.iter().any(|f| f.title == format!("{} is installed", t.name));
            assert!(suggested != installed, "{}: {r:?}", t.name);
        }
    }

    /// The suggestion, from whatever this computer has: either suggested
    /// (with brush's steps, root) or applied (with its undo), never both.
    #[test]
    fn brush_suggested_or_applied() {
        let r = scan();
        let found = r.find(BRUSH_ID).is_some();
        let undo = r.undo.iter().any(|u| u.id == format!("undo:{BRUSH_ID}"));
        assert!(found != undo, "{r:?}");
        if let Some(fix) = r.find(BRUSH_ID).and_then(|f| f.fix.as_ref()) {
            assert!(fix.root && fix.steps.iter().any(|s| matches!(s, Step::TryShell { .. })));
        }
    }
}
