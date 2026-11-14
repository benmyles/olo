//! At login: the session runs `olo-doctor --watch` beside Deskbar. After
//! the session has settled it looks the computer over, and when there is
//! a problem the person hasn't been told about it says so in a
//! notification, whose click opens Olo Doctor. A problem is told once;
//! one fixed and back later is told again. Then it follows the Wi-Fi
//! healer's report (`/run/olo/wifi-heal.json`, written by the root
//! service, apps/wifi-heal), by inotify, and tells of each time it reset
//! the Wi-Fi chip, and what came of it. (`--notify` is the look alone.)
//! The same look has what Olo suggests (suggest.rs), and each suggestion
//! is told once ever (its click opens Olo Doctor too): one not taken
//! stays in the window, untold again, and "Update brush" after "Use
//! brush" is another. A suggestion with nothing to apply yet (brush
//! before Olo's build of it) waits to be told until it has. What's
//! ignored is never told.
//! And it follows the crash reports Olo's programs leave (`olo_crash`,
//! by inotify on their folder): each new one told once ("Viewer
//! crashed", what and in which thread), a click opening Logs on that
//! program's lines around it.

use std::{collections::BTreeSet, path::PathBuf, sync::mpsc, time::Duration};

use olo_kit_base::notify::{Note, NoteEvent, NoteId, Notifier};
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
    /// The suggestions told of (`told_as`), never forgotten.
    suggested: BTreeSet<String>,
    /// Crash reports told of (their file names; one gone is forgotten).
    crashes: BTreeSet<String>,
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
    /// What was reset: a network interface, or a Bluetooth controller
    /// ("hci0").
    interface: String,
    why: String,
    rejoined: Option<String>,
    error: Option<String>,
}

fn told_path() -> PathBuf {
    olo_xdg::state_dir().join("doctor.json")
}

/// The problems to tell of now, and what is remembered as told after.
fn news<'a>(findings: &'a [Finding], told: &Told) -> (Vec<&'a Finding>, Told) {
    let problems: Vec<&Finding> = findings.iter().filter(|f| f.severity.is_problem()).collect();
    let new = problems.iter().filter(|f| !told.ids.contains(&f.id)).copied().collect();
    // What isn't found now is forgotten, to be told again if it comes back.
    (new, Told { ids: problems.iter().map(|f| f.id.clone()).collect(), heals: told.heals, suggested: told.suggested.clone(), crashes: told.crashes.clone() })
}

/// What a suggestion is remembered as told by: its id and title, so a
/// different suggestion under one id ("Update brush") is told too.
fn told_as(f: &Finding) -> String {
    format!("{}\t{}", f.id, f.title)
}

/// The suggestions to tell of now (each once ever, once it can be
/// applied), what's told added to `told`.
fn new_suggestions<'a>(findings: &'a [Finding], told: &mut Told) -> Vec<&'a Finding> {
    findings.iter().filter(|f| f.severity == Severity::Suggestion && f.fix.is_some() && told.suggested.insert(told_as(f))).collect()
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

/// Tell of the suggestions in `findings` not told before (one note,
/// the suggestion box its picture).
fn tell_suggestions(notes: &Notifier, findings: &[Finding]) {
    let mut told = read_told();
    let new = new_suggestions(findings, &mut told);
    if new.is_empty() {
        return;
    }
    write_told(&told);
    let summary = if new.len() == 1 { "Olo has a suggestion".to_owned() } else { format!("Olo has {} suggestions", new.len()) };
    let body = new.iter().map(|f| f.title.as_str()).collect::<Vec<_>>().join("\n");
    notes.show(None, Note::new(summary).body(body).icon("suggestions").action("default", "Show").action("show", "Show in Olo Doctor"));
}

/// Look the computer over, and tell of problems not told before (and,
/// `suggestions`, of suggestions); whether a problem was told.
fn look(notes: &Notifier, suggestions: bool) -> bool {
    let report = checks::look();
    let mut told = read_told();
    let (new, remember) = news(&report.findings, &told);
    told.ids = remember.ids;
    write_told(&told);
    if suggestions {
        tell_suggestions(notes, &report.findings);
    }
    if new.is_empty() {
        return false;
    }
    let critical = new.iter().any(|f| f.severity == Severity::Critical);
    let summary = match (new.len(), critical) {
        (1, true) => "Olo Doctor found a critical problem".to_owned(),
        (1, false) => "Olo Doctor found a problem".to_owned(),
        (n, _) => format!("Olo Doctor found {n} problems"),
    };
    let body = new.iter().map(|f| f.title.as_str()).collect::<Vec<_>>().join("\n");
    notes.show(None, Note::new(summary).body(body).icon("doctor").action("default", "Show").action("show", "Show in Olo Doctor"));
    true
}

/// Crashes at most told one by one in a look; more are one note.
const CRASHES_AT_ONCE: usize = 3;

/// The crashes to tell of (not told before), what's told then (those
/// still there).
fn new_crashes(all: &[crate::crashes::Crash], told: &Told) -> (Vec<crate::crashes::Crash>, BTreeSet<String>) {
    let new = all.iter().filter(|c| !told.crashes.contains(&c.file)).cloned().collect();
    (new, all.iter().map(|c| c.file.clone()).collect())
}

/// Which crash each crash note is, for its click.
type Shown = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<NoteId, crate::crashes::Crash>>>;

/// Tell of the crashes not told before.
fn tell_crashes(notes: &Notifier, shown: &Shown) {
    let all = crate::crashes::list();
    let mut told = read_told();
    let (new, remember) = new_crashes(&all, &told);
    if new.is_empty() && remember == told.crashes {
        return;
    }
    told.crashes = remember;
    write_told(&told);
    if new.len() > CRASHES_AT_ONCE {
        let names: BTreeSet<String> = new.iter().map(|c| crate::crashes::name(&c.program)).collect();
        let body = names.into_iter().collect::<Vec<_>>().join(", ");
        let id = notes.show(None, Note::new(format!("{} crashes", new.len())).body(body).icon("doctor").action("default", "Show").action("show", "Show in Logs"));
        shown.lock().unwrap_or_else(|e| e.into_inner()).insert(id, new[0].clone());
        return;
    }
    for c in new {
        let note = Note::new(format!("{} crashed", crate::crashes::name(&c.program)))
            .body(crate::crashes::summary(&c))
            .icon(crate::crashes::icon(&c.program))
            .action("default", "Show")
            .action("show", "Show in Logs");
        let id = notes.show(None, note);
        shown.lock().unwrap_or_else(|e| e.into_inner()).insert(id, c);
    }
}

/// Logs, on the program's lines around the crash.
fn open_logs(c: &crate::crashes::Crash) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let beside = std::env::current_exe().ok().map(|p| PathBuf::from(p.to_string_lossy().trim_end_matches(" (deleted)")).with_file_name("olo-logs"));
    let exe = beside.filter(|p| p.is_file()).unwrap_or_else(|| "olo-logs".into());
    let _ = olo_xdg::open::spawn_app(&[exe.into_os_string(), "--query".into(), crate::crashes::logs_query(c, now).into()], "olo.logs");
}

/// What the healer did since last told (Wi-Fi's or Bluetooth's), in a
/// notification each.
fn tell_heals(notes: &Notifier) {
    let Some(report) = std::fs::read_to_string(HEAL_REPORT).ok().and_then(|t| serde_json::from_str::<Heals>(&t).ok()) else { return };
    let mut told = read_told();
    let since = told.heals;
    for h in report.heals.iter().filter(|h| h.at > since) {
        let bluetooth = h.interface.starts_with("hci");
        let note = match (&h.rejoined, &h.error) {
            (Some(_), _) if bluetooth => Note::new("Olo fixed Bluetooth").body(format!("Bluetooth's hardware had stopped answering ({}). Olo started it again: devices connect again.", h.why)),
            (Some(network), _) => Note::new("Olo fixed the Wi-Fi").body(format!("The Wi-Fi chip had stopped joining networks ({}). Olo started it again and joined {network}.", h.why)),
            (None, Some(e)) if bluetooth => Note::new("Bluetooth isn't answering").body(format!("{e}.")),
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
    let shown: Shown = Shown::default();
    let crash_notes = Notifier::with_events("Olo Doctor", "olo-doctor", {
        let shown = shown.clone();
        move |e| {
            if let NoteEvent::Action { id, .. } = e {
                let c = shown.lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned();
                if let Some(c) = c {
                    open_logs(&c);
                }
            }
        }
    });
    std::thread::sleep(SETTLE);
    look(&notes, true);
    tell_heals(&notes);
    // Crashes as they're reported (the folder made, so it can be
    // watched before the first).
    let crashes_dir = olo_crash::dir();
    let _ = std::fs::create_dir_all(&crashes_dir);
    tell_crashes(&crash_notes, &shown);
    let _crash_watch = olo_kit_base::watch::watch([crashes_dir], move |_| tell_crashes(&crash_notes, &shown));
    let watching = notes.clone();
    let _watch = olo_kit_base::watch::watch([PathBuf::from(HEAL_REPORT)], move |_| tell_heals(&watching));
    // Then again every quarter of an hour: a problem that came since is
    // told, and Deskbar's tray follows what's found.
    loop {
        std::thread::sleep(EVERY);
        look(&notes, true);
    }
}

pub fn run() {
    std::thread::sleep(SETTLE);
    let (tx, rx) = mpsc::channel();
    let notes = Notifier::with_events("Olo Doctor", "olo-doctor", move |e| {
        let _ = tx.send(e);
    });
    if !look(&notes, false) {
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
        let now = [finding("remapper:keyd", Severity::Problem), finding("login", Severity::Advice), finding("rule:input", Severity::Critical), finding("tool:just", Severity::Suggestion)];
        let told = Told { ids: ["remapper:keyd".to_owned(), "autosuspend:3-1.1".to_owned()].into(), ..Told::default() };
        let (new, remember) = news(&now, &told);
        assert_eq!(new.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["rule:input"], "keyd was told; advice never is");
        assert_eq!(remember.ids, ["remapper:keyd".to_owned(), "rule:input".to_owned()].into(), "the receiver's, gone, is forgotten");
        let (new, _) = news(&now, &remember);
        assert!(new.is_empty());
    }

    #[test]
    fn a_suggestion_is_told_once_ever() {
        use crate::checks::Fix;
        let applicable = |id: &str, title: &str| Finding {
            id: id.into(),
            severity: Severity::Suggestion,
            title: title.into(),
            detail: String::new(),
            fix: Some(Fix { label: "Do".into(), root: false, steps: vec![] }),
        };
        let mut told = Told::default();
        let mut fixable_problem = applicable("rtkit", "rtkit isn't running");
        fixable_problem.severity = Severity::Problem;
        let now = [applicable("tool:just", "Install just"), applicable("brush-shell", "Use brush as your shell"), finding("tool:uv", Severity::Suggestion), fixable_problem];
        let new = new_suggestions(&now, &mut told);
        assert_eq!(new.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["tool:just", "brush-shell"], "uv has nothing to apply yet; a problem isn't a suggestion");
        assert!(new_suggestions(&now, &mut told).is_empty(), "told once");
        // Gone (installed) and back: not told again.
        assert!(new_suggestions(&now[..1], &mut told).is_empty());
        assert!(new_suggestions(&now, &mut told).is_empty());
        // Another suggestion under brush's id is.
        let update = [applicable("brush-shell", "Update brush")];
        assert_eq!(new_suggestions(&update, &mut told).len(), 1);
        // A problem's look keeps what's been suggested.
        let (_, remember) = news(&[], &told);
        assert_eq!(remember.suggested, told.suggested);
    }

    #[test]
    fn a_crash_is_told_once_and_forgotten_when_its_report_goes() {
        let crash = |file: &str| crate::crashes::Crash {
            file: file.into(),
            path: file.into(),
            program: "olo-viewer".into(),
            pid: 1,
            at_ms: 0,
            started_ms: 0,
            kind: "panic".into(),
            thread: "main".into(),
            what: "boom".into(),
            place: String::new(),
            backtrace: String::new(),
        };
        let mut told = Told::default();
        let (new, remember) = new_crashes(&[crash("a.json"), crash("b.json")], &told);
        assert_eq!(new.len(), 2);
        told.crashes = remember;
        let (new, remember) = new_crashes(&[crash("b.json"), crash("c.json")], &told);
        assert_eq!(new.iter().map(|c| c.file.as_str()).collect::<Vec<_>>(), ["c.json"], "b was told");
        assert_eq!(remember, ["b.json".to_owned(), "c.json".to_owned()].into(), "a's report went: forgotten");
    }
}
