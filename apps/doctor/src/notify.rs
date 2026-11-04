//! At login: the session runs `olo-doctor --watch` beside Deskbar. After
//! the session has settled it looks the computer over, and when there is
//! a problem the person hasn't been told about it says so in a
//! notification, whose click opens Olo Doctor. A problem is told once;
//! one fixed and back later is told again. Then it follows the Wi-Fi
//! healer's report (`/run/olo/wifi-heal.json`, written by the root
//! service, apps/wifi-heal), by inotify, and tells of each time it reset
//! the Wi-Fi chip, and what came of it. (`--notify` is the look alone.)

use std::{collections::BTreeSet, path::PathBuf, sync::mpsc, time::Duration};

use olo_kit_base::notify::{Note, NoteEvent, Notifier};
use serde::{Deserialize, Serialize};

use crate::checks::{self, Finding, Severity};

/// Long enough for the displays, Deskbar and login items to be up.
const SETTLE: Duration = Duration::from_secs(20);
/// How often it looks again after (Ben: "run periodically like on startup
/// and then maybe every 15 mins"): a look reads sysfs and asks systemd
/// and PackageKit (its cached lists), a few hundred milliseconds of a
/// background thread four times an hour.
const EVERY: Duration = Duration::from_secs(15 * 60);

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Told {
    ids: BTreeSet<String>,
    /// The latest of the Wi-Fi healer's resets told of (its time).
    heals: u64,
}

/// The Wi-Fi healer's report, as much as is told of it.
const HEAL_REPORT: &str = "/run/olo/wifi-heal.json";

#[derive(Default, Deserialize)]
#[serde(default)]
struct Heals {
    heals: Vec<Heal>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Heal {
    at: u64,
    why: String,
    rejoined: Option<String>,
    error: Option<String>,
}

fn told_path() -> PathBuf {
    olo_xdg::state_dir().join("doctor.json")
}

/// The problems to tell of now, and what is remembered as told after.
fn news<'a>(findings: &'a [Finding], told: &Told) -> (Vec<&'a Finding>, Told) {
    let problems: Vec<&Finding> = findings.iter().filter(|f| f.severity == Severity::Problem).collect();
    let new = problems.iter().filter(|f| !told.ids.contains(&f.id)).copied().collect();
    // What isn't found now is forgotten, to be told again if it comes back.
    (new, Told { ids: problems.iter().map(|f| f.id.clone()).collect(), heals: told.heals })
}

fn read_told() -> Told {
    olo_kit_base::store::load(&told_path()).unwrap_or_default()
}

fn write_told(told: &Told) {
    let _ = olo_kit_base::store::write(&told_path(), &serde_json::to_vec(told).unwrap_or_default());
}

fn open_doctor() {
    let exe = std::env::current_exe().map(|p| PathBuf::from(p.to_string_lossy().trim_end_matches(" (deleted)"))).unwrap_or_else(|_| "olo-doctor".into());
    let _ = olo_xdg::open::spawn_app(&[exe.into_os_string()], "olo.doctor");
}

/// Look the computer over, and tell of problems not told before.
fn look(notes: &Notifier) -> bool {
    let report = checks::look();
    let mut told = read_told();
    let (new, remember) = news(&report.findings, &told);
    told.ids = remember.ids;
    write_told(&told);
    if new.is_empty() {
        return false;
    }
    let summary = if new.len() == 1 { "Olo Doctor found a problem".to_owned() } else { format!("Olo Doctor found {} problems", new.len()) };
    let body = new.iter().map(|f| f.title.as_str()).collect::<Vec<_>>().join("\n");
    notes.show(None, Note::new(summary).body(body).icon("doctor").action("default", "Show").action("show", "Show in Olo Doctor"));
    true
}

/// What the Wi-Fi healer did since last told, in a notification each.
fn tell_heals(notes: &Notifier) {
    let Some(report) = std::fs::read_to_string(HEAL_REPORT).ok().and_then(|t| serde_json::from_str::<Heals>(&t).ok()) else { return };
    let mut told = read_told();
    let since = told.heals;
    for h in report.heals.iter().filter(|h| h.at > since) {
        let note = match (&h.rejoined, &h.error) {
            (Some(network), _) => Note::new("Olo fixed the Wi-Fi").body(format!("The Wi-Fi chip had stopped joining networks ({}). Olo started it again and joined {network}.", h.why)),
            (None, Some(e)) => Note::new("Wi-Fi couldn't join").body(format!("{e}.")),
            // Under way: told once it has come to something.
            (None, None) => return,
        };
        notes.show(None, note.icon("doctor").action("default", "Show"));
        told.heals = h.at;
        write_told(&told);
    }
}

/// The session's: the look, then the Wi-Fi healer's news, for as long as
/// the session runs.
pub fn watch() -> ! {
    // (What it has told is on disk: the new version tells nothing again.)
    olo_kit_base::restore::exec_on_upgrade();
    let (tx, rx) = mpsc::channel();
    let notes = Notifier::with_events("Olo Doctor", "olo-doctor", move |e| {
        let _ = tx.send(e);
    });
    std::thread::spawn(move || {
        for e in rx {
            if let NoteEvent::Action { .. } = e {
                open_doctor();
            }
        }
    });
    std::thread::sleep(SETTLE);
    look(&notes);
    tell_heals(&notes);
    let watching = notes.clone();
    let _watch = olo_kit_base::watch::watch([PathBuf::from(HEAL_REPORT)], move |_| tell_heals(&watching));
    // Then again every quarter of an hour: a problem that came since is
    // told, and Deskbar's tray follows what's found.
    loop {
        std::thread::sleep(EVERY);
        look(&notes);
    }
}

pub fn run() {
    std::thread::sleep(SETTLE);
    let (tx, rx) = mpsc::channel();
    let notes = Notifier::with_events("Olo Doctor", "olo-doctor", move |e| {
        let _ = tx.send(e);
    });
    if !look(&notes) {
        return;
    }
    // Blocked here (nothing polled) until the person answers or it goes.
    while let Ok(e) = rx.recv() {
        match e {
            NoteEvent::Action { .. } => return open_doctor(),
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
        let told = Told { ids: ["remapper:keyd".to_owned(), "autosuspend:3-1.1".to_owned()].into(), heals: 0 };
        let (new, remember) = news(&now, &told);
        assert_eq!(new.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["rule:input"], "keyd was told; advice never is");
        assert_eq!(remember.ids, ["remapper:keyd".to_owned(), "rule:input".to_owned()].into(), "the receiver's, gone, is forgotten");
        let (new, _) = news(&now, &remember);
        assert!(new.is_empty());
    }
}
