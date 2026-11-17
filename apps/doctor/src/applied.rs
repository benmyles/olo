//! What Olo Doctor has done that can be undone (Ben, 2026-11-16:
//! "everything in olo doctor that it does must be reversible, e.g. for
//! any fix/suggestion applied i should be able to undo/restore to the
//! previous behavior"): for each fix or suggestion applied, the steps
//! that put back what it changed (`fix::Step::inverse`, worked out as
//! each step ran: the file as it was, the unit as it stood, the value
//! it had, the packages it added), kept with Doctor's state
//! (`~/.local/state/olo/doctor/applied.json`), and listed in the window
//! under Fixed with Undo, and by `olo-doctor --fix` as `applied:ID`.
//!
//! Undone, an entry goes. One whose finding is found again goes too
//! (the fix isn't in force: there's nothing of it to undo), as does one
//! a suggestion has its own undo for (brush, the start-up screen: theirs
//! knows more than a file put back).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    checks::{Fix, Report, Undo},
    fix::Step,
};

/// The id an applied fix's undo goes by: the finding's, after this.
pub const PREFIX: &str = "applied:";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Applied {
    /// The finding it fixed.
    pub id: String,
    pub title: String,
    /// What its button said ("Present as Hands-Free").
    pub label: String,
    /// When (Unix seconds).
    pub at: u64,
    /// Its undo needs root, as it did.
    pub root: bool,
    /// What puts it back; and what of it can't be.
    pub undo: Vec<Step>,
    #[serde(default)]
    pub kept: Vec<String>,
}

impl Applied {
    /// "Today, “Present as Hands-Free”. Undo: writing /etc/… and
    /// restarting bluetooth.service.", and what of it can't be undone.
    pub fn line(&self) -> String {
        let lower = |s: String| {
            let mut c = s.chars();
            c.next().map(|f| f.to_lowercase().chain(c).collect::<String>()).unwrap_or_default()
        };
        let what: Vec<String> = self.undo.iter().map(|s| lower(s.describe())).collect();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let mut line = format!("{}, “{}”.", olo_kit::when::day(self.at as i64, now), self.label);
        if !what.is_empty() {
            line.push_str(&format!(" Undo: {}.", crate::app::and_list(&what)));
        }
        for k in &self.kept {
            line.push(' ');
            line.push_str(k);
        }
        line
    }
}

fn path() -> PathBuf {
    match std::env::var_os("OLO_DOCTOR_APPLIED") {
        Some(p) => p.into(),
        None => olo_xdg::state_dir().join("doctor/applied.json"),
    }
}

pub fn all() -> Vec<Applied> {
    std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(all: &[Applied]) -> std::io::Result<()> {
    let path = path();
    if all.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("new");
    std::fs::write(&tmp, serde_json::to_vec_pretty(all).unwrap_or_default())?;
    std::fs::rename(tmp, path)
}

/// A fix applied: kept (the last of a finding's, should it be fixed
/// twice). One with nothing to undo and nothing to say isn't kept.
pub fn add(entry: Applied) -> std::io::Result<()> {
    if entry.undo.is_empty() && entry.kept.is_empty() {
        return Ok(());
    }
    let mut all = all();
    all.retain(|a| a.id != entry.id);
    all.push(entry);
    save(&all)
}

/// The entry `id` (a finding's) is undone, or stale: gone.
pub fn remove(id: &str) -> std::io::Result<()> {
    let mut all = all();
    let before = all.len();
    all.retain(|a| a.id != id);
    if all.len() == before { Ok(()) } else { save(&all) }
}

/// The finding an undo's `id` is for, if it's one of these.
pub fn of(id: &str) -> Option<&str> {
    id.strip_prefix(PREFIX)
}

/// Into `report`: each applied fix that can be undone, as an undo
/// (`applied:ID`). Those whose finding is found again, or that a
/// suggestion has an undo of its own for, are dropped (and forgotten).
pub fn into_report(report: &mut Report) {
    let stale = |a: &Applied| report.findings.iter().chain(&report.ignored).any(|f| f.id == a.id) || report.undo.iter().any(|u| u.id.contains(&a.id));
    let (gone, kept): (Vec<Applied>, Vec<Applied>) = all().into_iter().partition(|a| stale(a));
    if !gone.is_empty() {
        let _ = save(&kept);
    }
    for a in kept {
        if a.undo.is_empty() {
            continue;
        }
        report.undo.push(Undo { id: format!("{PREFIX}{}", a.id), title: a.title.clone(), fix: Fix { label: "Undo".into(), root: a.root, steps: a.undo.clone() } });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kept, listed as undos, replaced by a later one of the same
    /// finding, gone once undone; one whose finding is back is dropped.
    #[test]
    fn what_was_applied_is_kept_until_undone() {
        let dir = PathBuf::from(format!("/var/tmp/olo-doctor-applied-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: this test's own variable, read only by it.
        unsafe { std::env::set_var("OLO_DOCTOR_APPLIED", dir.join("applied.json")) };
        let entry = |id: &str, content: &str| Applied {
            id: id.into(),
            title: format!("{id} was wrong"),
            label: "Fix It".into(),
            at: 1_794_900_000,
            root: true,
            undo: vec![Step::WriteFile { path: "/etc/x.conf".into(), content: content.into(), mode: 0o644 }],
            kept: Vec::new(),
        };
        assert!(all().is_empty());
        add(entry("a", "old")).unwrap();
        add(entry("b", "old")).unwrap();
        add(entry("a", "older")).unwrap();
        assert_eq!(all().iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        // Nothing to undo and nothing to say: not kept.
        add(Applied { undo: Vec::new(), ..entry("c", "") }).unwrap();
        assert_eq!(all().len(), 2);
        let mut report = Report::default();
        into_report(&mut report);
        assert_eq!(report.undo.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(), ["applied:b", "applied:a"]);
        assert!(report.undo[1].fix.root && matches!(&report.undo[1].fix.steps[..], [Step::WriteFile { content, .. }] if content == "older"));
        assert_eq!(of("applied:b"), Some("b"));
        // b's finding is found again: its fix isn't in force, and its
        // undo is dropped and forgotten.
        let mut report = Report::default();
        report.findings.push(crate::checks::Finding { id: "b".into(), severity: crate::checks::Severity::Problem, title: "b is wrong".into(), detail: String::new(), fix: None });
        into_report(&mut report);
        assert_eq!(report.undo.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(), ["applied:a"]);
        assert_eq!(all().len(), 1);
        remove("a").unwrap();
        assert!(all().is_empty() && !dir.join("applied.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
