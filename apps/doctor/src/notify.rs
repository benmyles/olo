//! At login: the session runs `olo-doctor --notify` once. After the
//! session has settled it looks the computer over, and when there is a
//! problem the person hasn't been told about it says so in a
//! notification, whose click opens Olo Doctor. A problem is told once;
//! one fixed and back later is told again.

use std::{collections::BTreeSet, path::PathBuf, sync::mpsc, time::Duration};

use olo_kit_base::notify::{Note, NoteEvent, Notifier};
use serde::{Deserialize, Serialize};

use crate::checks::{self, Finding, Roots, Severity};

/// Long enough for the displays, Deskbar and login items to be up.
const SETTLE: Duration = Duration::from_secs(20);

#[derive(Default, Serialize, Deserialize)]
struct Told {
    ids: BTreeSet<String>,
}

fn told_path() -> PathBuf {
    olo_xdg::state_dir().join("doctor.json")
}

/// The problems to tell of now, and what is remembered as told after.
fn news<'a>(findings: &'a [Finding], told: &Told) -> (Vec<&'a Finding>, Told) {
    let problems: Vec<&Finding> = findings.iter().filter(|f| f.severity == Severity::Problem).collect();
    let new = problems.iter().filter(|f| !told.ids.contains(&f.id)).copied().collect();
    // What isn't found now is forgotten, to be told again if it comes back.
    (new, Told { ids: problems.iter().map(|f| f.id.clone()).collect() })
}

pub fn run() {
    std::thread::sleep(SETTLE);
    let report = checks::scan(&Roots::real());
    let path = told_path();
    let told: Told = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let (new, remember) = news(&report.findings, &told);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, serde_json::to_string(&remember).unwrap_or_default());
    if new.is_empty() {
        return;
    }
    let (tx, rx) = mpsc::channel();
    let notes = Notifier::with_events("Olo Doctor", "olo-doctor", move |e| {
        let _ = tx.send(e);
    });
    let summary = if new.len() == 1 { "Olo Doctor found a problem".to_owned() } else { format!("Olo Doctor found {} problems", new.len()) };
    let body = new.iter().map(|f| f.title.as_str()).collect::<Vec<_>>().join("\n");
    notes.show(None, Note::new(summary).body(body).icon("doctor").action("default", "Show").action("show", "Show in Olo Doctor"));
    // Blocked here (nothing polled) until the person answers or it goes.
    while let Ok(e) = rx.recv() {
        match e {
            NoteEvent::Action { .. } => {
                let exe = std::env::current_exe().map(|p| PathBuf::from(p.to_string_lossy().trim_end_matches(" (deleted)"))).unwrap_or_else(|_| "olo-doctor".into());
                let _ = olo_xdg::open::spawn_app(&[exe.into_os_string()], "olo.doctor");
                return;
            }
            NoteEvent::Closed { .. } => return,
            NoteEvent::Replied { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::Severity;

    fn finding(id: &str, severity: Severity) -> Finding {
        Finding { id: id.into(), severity, title: id.into(), detail: String::new(), fix: None }
    }

    #[test]
    fn problems_are_told_once_and_again_when_they_come_back() {
        let now = [finding("remapper:keyd", Severity::Problem), finding("login", Severity::Advice), finding("rule:input", Severity::Problem)];
        let told = Told { ids: ["remapper:keyd".to_owned(), "autosuspend:3-1.1".to_owned()].into() };
        let (new, remember) = news(&now, &told);
        assert_eq!(new.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["rule:input"], "keyd was told; advice never is");
        assert_eq!(remember.ids, ["remapper:keyd".to_owned(), "rule:input".to_owned()].into(), "the receiver's, gone, is forgotten");
        let (new, _) = news(&now, &remember);
        assert!(new.is_empty());
    }
}
