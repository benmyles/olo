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

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    checks::{Finding, Fine, Fix, Report, Severity, Undo},
    fix::Step,
};

/// Where brush is installed as a login shell.
pub const BRUSH: &str = "/usr/local/bin/brush";
const BRUSH_ID: &str = "brush-shell";

/// What's suggested now, and what's applied. (`OLO_SUGGESTIONS_DEMO=
/// applied`: everything applied, made up, to look at headless without
/// changing the computer.)
pub fn scan() -> Report {
    let mut r = Report::default();
    if std::env::var("OLO_SUGGESTIONS_DEMO").as_deref() == Ok("applied") {
        applied_brush(&mut r, "person".into(), PathBuf::from("/bin/bash"));
        return r;
    }
    brush(&mut r);
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

/// This user's name and login shell (the password database's).
fn login_shell() -> Option<(String, PathBuf)> {
    use std::ffi::CStr;
    // SAFETY: getuid has no preconditions; getpwuid_r writes into the
    // buffer it's given, and what it returns points into it.
    unsafe {
        let mut pw: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0u8; 4096];
        let mut out: *mut libc::passwd = std::ptr::null_mut();
        if libc::getpwuid_r(libc::getuid(), &mut pw, buf.as_mut_ptr().cast(), buf.len(), &mut out) != 0 || out.is_null() {
            return None;
        }
        let name = CStr::from_ptr(pw.pw_name).to_string_lossy().into_owned();
        let shell = PathBuf::from(CStr::from_ptr(pw.pw_shell).to_string_lossy().into_owned());
        Some((name, shell))
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
    let detail = "brush (github.com/reubeno/brush) is a shell compatible with bash, written in Rust as Olo is. New shells, Terminal's too, use it; bash scripts run as before, and it's undone with a button.";
    let Some(ours) = ours else {
        r.findings.push(Finding { id: BRUSH_ID.into(), severity: Severity::Advice, title: "Use brush as your shell".into(), detail: format!("{detail} (Olo's build of brush comes with its next upgrade.)"), fix: None });
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

#[cfg(test)]
mod tests {
    use super::*;

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
